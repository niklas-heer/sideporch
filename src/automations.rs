//! Automations: small Lua scripts that admins write in the browser.
//!
//! Every enabled script runs in its own sandboxed Lua 5.4 state on one
//! dedicated thread. Scripts only get the `string`, `table`, `math`, `utf8`
//! and `coroutine` libraries plus a `sideporch` table:
//!
//! ```lua
//! sideporch.on_message(function(msg)          -- new messages in public channels
//!   if msg.text == "!ping" then sideporch.reply(msg, "pong") end
//! end)
//! sideporch.every(3600, function() ... end)   -- repeat every N seconds (at least 10)
//! sideporch.post("general", "Hello")          -- post to a public channel
//! sideporch.get("key") / sideporch.set("key", "value")  -- keep data between runs
//! sideporch.now()                             -- Unix time in seconds
//! ```
//!
//! Each call has an instruction budget and each state a memory limit, so a
//! runaway script fails with an error instead of stalling the server.
//! Messages from automations never trigger automations.

use std::{
    path::Path,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

use mlua::{Function, HookTriggers, Lua, LuaOptions, StdLib, Table, Value, VmState};
use rusqlite::Connection;
use tokio::sync::{mpsc as async_mpsc, oneshot};

use crate::{
    AppState, db,
    error::{AppError, AppResult},
    messages::{self, Draft, Sender},
    store::{self, Author, Message},
};

/// Instructions a single call may run, checked every 1,000 instructions.
const INSTRUCTION_BUDGET: u32 = 2_000_000;
const HOOK_INTERVAL: u32 = 1_000;
const MEMORY_LIMIT: usize = 16 * 1024 * 1024;
/// Messages a single call may post.
const POST_LIMIT: usize = 20;
const MIN_INTERVAL_SECS: u64 = 10;
const MAX_VALUE_BYTES: usize = 64 * 1024;

/// A new message, as scripts see it.
#[derive(Debug, Clone)]
pub struct MessageEvent {
    pub id: i64,
    pub channel_id: i64,
    pub channel: String,
    pub text: String,
    pub author: String,
    pub username: Option<String>,
    pub is_bot: bool,
    pub thread_id: Option<i64>,
}

impl MessageEvent {
    pub fn new(conn: &Connection, message: &Message) -> AppResult<Self> {
        let channel: Option<String> = conn.query_row(
            "SELECT name FROM channels WHERE id = ?1",
            [message.channel_id],
            |row| row.get(0),
        )?;
        let (author, username, is_bot) = match &message.author {
            Author::User { id, display_name } => {
                let username: String =
                    conn.query_row("SELECT username FROM users WHERE id = ?1", [id], |row| {
                        row.get(0)
                    })?;
                (display_name.clone(), Some(username), false)
            }
            Author::Bot { name, .. } => (name.clone(), None, true),
            Author::Removed => ("Former member".to_owned(), None, false),
        };
        Ok(Self {
            id: message.id,
            channel_id: message.channel_id,
            channel: channel.unwrap_or_default(),
            text: message.body.clone(),
            author,
            username,
            is_bot,
            thread_id: message.parent_id,
        })
    }
}

enum Command {
    Reload(Vec<store::Automation>, oneshot::Sender<()>),
    Message(MessageEvent),
}

/// Something a script asked Sideporch to do.
enum Action {
    Post {
        automation_id: i64,
        name: String,
        channel: ChannelRef,
        text: String,
        thread: Option<i64>,
    },
}

enum ChannelRef {
    Id(i64),
    Name(String),
}

/// Handle to the automation thread.
#[derive(Clone)]
pub struct Automations {
    commands: mpsc::Sender<Command>,
    actions: Arc<Mutex<Option<async_mpsc::UnboundedReceiver<Action>>>>,
}

impl Automations {
    /// Starts the automation thread. It keeps its own database connection
    /// for script data.
    pub fn start(db_path: &Path) -> AppResult<Self> {
        let conn = Arc::new(Mutex::new(db::connect(db_path)?));
        let (commands, receiver) = mpsc::channel();
        let (actions, action_receiver) = async_mpsc::unbounded_channel();
        thread::Builder::new()
            .name("sideporch-automations".into())
            .spawn(move || run(&receiver, &conn, &actions))
            .map_err(AppError::internal)?;
        Ok(Self {
            commands,
            actions: Arc::new(Mutex::new(Some(action_receiver))),
        })
    }

    /// Carries out what scripts ask for. Call once, after startup.
    pub fn serve(&self, state: AppState) {
        let receiver = self
            .actions
            .lock()
            .ok()
            .and_then(|mut receiver| receiver.take());
        if let Some(mut receiver) = receiver {
            tokio::spawn(async move {
                while let Some(action) = receiver.recv().await {
                    perform(&state, action).await;
                }
            });
        }
    }

    pub fn message(&self, event: MessageEvent) {
        // Fails only if the thread is gone, which shutdown causes.
        drop(self.commands.send(Command::Message(event)));
    }

    /// Reloads all scripts from the database and waits until they ran their
    /// top level, so load errors are stored when this returns.
    pub async fn reload(&self, state: &AppState) -> AppResult<()> {
        let automations = state.db.call(|conn| store::automations(conn)).await?;
        let (done, loaded) = oneshot::channel();
        self.commands
            .send(Command::Reload(automations, done))
            .map_err(|_| AppError::internal("automation thread stopped"))?;
        // A script that never finishes loading is stopped by its budget.
        drop(tokio::time::timeout(Duration::from_secs(10), loaded).await);
        Ok(())
    }
}

async fn perform(state: &AppState, action: Action) {
    let Action::Post {
        automation_id,
        name,
        channel,
        text,
        thread,
    } = action;
    let channel_id = state
        .db
        .call(move |conn| match channel {
            ChannelRef::Id(id) => Ok(conn
                .query_row(
                    "SELECT id FROM channels WHERE id = ?1 AND kind = 'public'",
                    [id],
                    |row| row.get::<_, i64>(0),
                )
                .ok()),
            ChannelRef::Name(name) => store::public_channel_id(conn, name.trim_start_matches('#')),
        })
        .await;
    let result = match channel_id {
        Ok(Some(channel_id)) => messages::post(
            state,
            Draft {
                channel_id,
                parent_id: thread,
                sender: Sender::Automation {
                    id: automation_id,
                    name,
                },
                body: text,
                attachments: Vec::new(),
                files: Vec::new(),
            },
        )
        .await
        .map(drop),
        Ok(None) => Err(AppError::bad_request(
            "sideporch.post: no public channel with that name",
        )),
        Err(error) => Err(error),
    };
    if let Err(error) = result {
        let message = error.to_string();
        drop(
            state
                .db
                .call(move |conn| store::set_automation_error(conn, automation_id, Some(&message)))
                .await,
        );
    }
}

struct Timer {
    every: Duration,
    next: Instant,
    callback: Function,
}

struct Script {
    id: i64,
    lua: Lua,
    handlers: Vec<Function>,
    timers: Vec<Timer>,
}

/// Per-state data that the `sideporch` functions reach through app data.
struct Registry {
    handlers: Vec<Function>,
    timers: Vec<Timer>,
}

struct Budget {
    used: u32,
    posts: usize,
}

fn run(
    commands: &mpsc::Receiver<Command>,
    conn: &Arc<Mutex<Connection>>,
    actions: &async_mpsc::UnboundedSender<Action>,
) {
    let mut scripts: Vec<Script> = Vec::new();
    loop {
        let now = Instant::now();
        let wait = scripts
            .iter()
            .flat_map(|script| script.timers.iter().map(|timer| timer.next))
            .min()
            .map_or(Duration::from_secs(60), |next| {
                next.saturating_duration_since(now)
            });
        match commands.recv_timeout(wait) {
            Ok(Command::Reload(automations, done)) => {
                scripts = automations
                    .into_iter()
                    .filter(|automation| automation.enabled)
                    .filter_map(|automation| load(&automation, conn, actions))
                    .collect();
                // The caller may have stopped waiting.
                let _ = done.send(());
            }
            Ok(Command::Message(event)) => {
                for script in &scripts {
                    for handler in &script.handlers {
                        let result = message_table(&script.lua, &event)
                            .and_then(|table| call(&script.lua, handler, table));
                        report(conn, script.id, result);
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
        let now = Instant::now();
        for script in &mut scripts {
            for timer in &mut script.timers {
                if timer.next <= now {
                    timer.next = now.checked_add(timer.every).unwrap_or(now);
                    let result = call(&script.lua, &timer.callback, ());
                    report(conn, script.id, result);
                }
            }
        }
    }
}

/// Runs a script's top level, which registers its handlers and timers.
fn load(
    automation: &store::Automation,
    conn: &Arc<Mutex<Connection>>,
    actions: &async_mpsc::UnboundedSender<Action>,
) -> Option<Script> {
    let loaded = sandbox(automation, conn, actions).and_then(|lua| {
        lua.set_app_data(Budget { used: 0, posts: 0 });
        lua.load(automation.source.as_str())
            .set_name(format!("={}", automation.name))
            .set_mode(mlua::prelude::LuaChunkMode::Text)
            .exec()?;
        let registry = lua
            .remove_app_data::<Registry>()
            .ok_or_else(|| mlua::Error::runtime("registry missing"))?;
        Ok(Script {
            id: automation.id,
            lua,
            handlers: registry.handlers,
            timers: registry.timers,
        })
    });
    match loaded {
        Ok(script) => {
            report(conn, automation.id, Ok(()));
            Some(script)
        }
        Err(error) => {
            report(conn, automation.id, Err(error));
            None
        }
    }
}

fn call(lua: &Lua, function: &Function, args: impl mlua::IntoLuaMulti) -> mlua::Result<()> {
    lua.set_app_data(Budget { used: 0, posts: 0 });
    function.call::<()>(args)
}

fn report(conn: &Arc<Mutex<Connection>>, id: i64, result: mlua::Result<()>) {
    let error = result.err().map(|error| clean_error(&error));
    if let Some(error) = &error {
        tracing::warn!(automation = id, %error, "automation failed");
    }
    if let Ok(conn) = conn.lock()
        && let Err(error) = store::set_automation_error(&conn, id, error.as_deref())
    {
        tracing::warn!(?error, "could not record automation status");
    }
}

/// The first line of a Lua error, without the Rust traceback noise.
fn clean_error(error: &mlua::Error) -> String {
    let text = match error {
        mlua::Error::CallbackError { cause, .. } => return clean_error(cause),
        other => other.to_string(),
    };
    text.lines()
        .find(|line| !line.trim().is_empty() && !line.starts_with("stack traceback"))
        .unwrap_or("unknown error")
        .trim_start_matches("runtime error: ")
        .trim_start_matches("syntax error: ")
        .to_owned()
}

fn sandbox(
    automation: &store::Automation,
    conn: &Arc<Mutex<Connection>>,
    actions: &async_mpsc::UnboundedSender<Action>,
) -> mlua::Result<Lua> {
    let lua = Lua::new_with(
        StdLib::STRING | StdLib::TABLE | StdLib::MATH | StdLib::UTF8 | StdLib::COROUTINE,
        LuaOptions::default(),
    )?;
    lua.set_memory_limit(MEMORY_LIMIT)?;
    lua.set_global_hook(
        HookTriggers::new().every_nth_instruction(HOOK_INTERVAL),
        |lua, _| {
            let exhausted = lua.app_data_mut::<Budget>().is_some_and(|mut budget| {
                budget.used = budget.used.saturating_add(HOOK_INTERVAL);
                budget.used > INSTRUCTION_BUDGET
            });
            if exhausted {
                Err(mlua::Error::runtime("stopped: the script ran too long"))
            } else {
                Ok(VmState::Continue)
            }
        },
    )?;
    let globals = lua.globals();
    for name in ["dofile", "loadfile", "load", "require", "collectgarbage"] {
        globals.set(name, Value::Nil)?;
    }
    if let Ok(string) = globals.get::<Table>("string") {
        string.set("dump", Value::Nil)?;
    }
    let script_name = automation.name.clone();
    globals.set(
        "print",
        lua.create_function(move |_, values: mlua::Variadic<Value>| {
            let line: Vec<String> = values
                .iter()
                .map(|value| value.to_string().unwrap_or_default())
                .collect();
            tracing::info!(automation = %script_name, "{}", line.join("\t"));
            Ok(())
        })?,
    )?;
    lua.set_app_data(Registry {
        handlers: Vec::new(),
        timers: Vec::new(),
    });
    let api = lua.create_table()?;
    register_triggers(&lua, &api)?;
    register_posting(&lua, &api, automation, actions)?;
    register_storage(&lua, &api, automation.id, conn)?;
    api.set(
        "now",
        lua.create_function(|_, ()| Ok(jiff::Timestamp::now().as_second()))?,
    )?;
    globals.set("sideporch", api)?;
    Ok(lua)
}

/// `sideporch.on_message` and `sideporch.every`.
fn register_triggers(lua: &Lua, api: &Table) -> mlua::Result<()> {
    api.set(
        "on_message",
        lua.create_function(|lua, handler: Function| {
            registry(lua)?.handlers.push(handler);
            Ok(())
        })?,
    )?;
    api.set(
        "every",
        lua.create_function(|lua, (seconds, callback): (u64, Function)| {
            if seconds < MIN_INTERVAL_SECS {
                return Err(mlua::Error::runtime(
                    "sideporch.every: use at least 10 seconds",
                ));
            }
            let every = Duration::from_secs(seconds);
            let now = Instant::now();
            registry(lua)?.timers.push(Timer {
                every,
                next: now.checked_add(every).unwrap_or(now),
                callback,
            });
            Ok(())
        })?,
    )
}

/// Counts a post against the current call's limit.
fn spend_post(lua: &Lua) -> mlua::Result<()> {
    let posts = {
        let mut budget = lua
            .app_data_mut::<Budget>()
            .ok_or_else(|| mlua::Error::runtime("budget missing"))?;
        budget.posts = budget.posts.saturating_add(1);
        budget.posts
    };
    if posts > POST_LIMIT {
        return Err(mlua::Error::runtime("stopped: too many posts in one run"));
    }
    Ok(())
}

/// `sideporch.post` and `sideporch.reply`.
fn register_posting(
    lua: &Lua,
    api: &Table,
    automation: &store::Automation,
    actions: &async_mpsc::UnboundedSender<Action>,
) -> mlua::Result<()> {
    let poster = |actions: async_mpsc::UnboundedSender<Action>| {
        let automation_id = automation.id;
        let name = automation.name.clone();
        move |lua: &Lua, channel: ChannelRef, text: String, thread: Option<i64>| {
            let text = text.trim();
            if text.is_empty() {
                return Err(mlua::Error::runtime("sideporch.post: the text is empty"));
            }
            spend_post(lua)?;
            actions
                .send(Action::Post {
                    automation_id,
                    name: name.clone(),
                    channel,
                    text: text.chars().take(10_000).collect(),
                    thread,
                })
                .map_err(|_| mlua::Error::runtime("Sideporch is shutting down"))
        }
    };
    let post = poster(actions.clone());
    api.set(
        "post",
        lua.create_function(
            move |lua, (channel, text, options): (String, String, Option<Table>)| {
                let thread = options
                    .map(|options| options.get::<Option<i64>>("thread"))
                    .transpose()?
                    .flatten();
                post(lua, ChannelRef::Name(channel), text, thread)
            },
        )?,
    )?;
    let reply = poster(actions.clone());
    api.set(
        "reply",
        lua.create_function(move |lua, (message, text): (Table, String)| {
            let channel: i64 = message.get("channel_id")?;
            let thread = message
                .get::<Option<i64>>("thread_id")?
                .or(message.get::<Option<i64>>("id")?);
            reply(lua, ChannelRef::Id(channel), text, thread)
        })?,
    )
}

/// `sideporch.get` and `sideporch.set`, stored per automation.
fn register_storage(
    lua: &Lua,
    api: &Table,
    id: i64,
    conn: &Arc<Mutex<Connection>>,
) -> mlua::Result<()> {
    let data = Arc::clone(conn);
    api.set(
        "get",
        lua.create_function(move |_, key: String| {
            let conn = data
                .lock()
                .map_err(|_| mlua::Error::runtime("database unavailable"))?;
            store::automation_value(&conn, id, &key)
                .map_err(|error| mlua::Error::runtime(error.to_string()))
        })?,
    )?;
    let data = Arc::clone(conn);
    api.set(
        "set",
        lua.create_function(move |_, (key, value): (String, Option<Value>)| {
            let value = match value {
                None | Some(Value::Nil) => None,
                Some(Value::String(text)) => Some(text.to_str()?.to_owned()),
                Some(other) => Some(other.to_string()?),
            };
            if key.len() > 200
                || value
                    .as_ref()
                    .is_some_and(|value| value.len() > MAX_VALUE_BYTES)
            {
                return Err(mlua::Error::runtime(
                    "sideporch.set: key or value is too long",
                ));
            }
            let conn = data
                .lock()
                .map_err(|_| mlua::Error::runtime("database unavailable"))?;
            store::set_automation_value(&conn, id, &key, value.as_deref())
                .map_err(|error| mlua::Error::runtime(error.to_string()))
        })?,
    )
}

fn registry(lua: &Lua) -> mlua::Result<mlua::AppDataRefMut<'_, Registry>> {
    lua.app_data_mut::<Registry>().ok_or_else(|| {
        mlua::Error::runtime("handlers can only be registered while the script loads")
    })
}

fn message_table(lua: &Lua, event: &MessageEvent) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set("id", event.id)?;
    table.set("channel_id", event.channel_id)?;
    table.set("channel", event.channel.as_str())?;
    table.set("text", event.text.as_str())?;
    table.set("author", event.author.as_str())?;
    table.set("username", event.username.as_deref())?;
    table.set("is_bot", event.is_bot)?;
    table.set("thread_id", event.thread_id)?;
    Ok(table)
}

/// The script a new automation starts with.
pub const EXAMPLE: &str = r#"-- Runs for every new message in a public channel.
sideporch.on_message(function(msg)
  if msg.text == "!ping" then
    sideporch.reply(msg, "pong")
  end
end)

-- Keeps a count between runs.
sideporch.on_message(function(msg)
  if msg.text == "!count" then
    local count = tonumber(sideporch.get("count") or "0") + 1
    sideporch.set("count", count)
    sideporch.reply(msg, "I've been asked " .. count .. " times.")
  end
end)
"#;
