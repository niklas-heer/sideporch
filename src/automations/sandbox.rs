//! One sandboxed Lua state per script, with the `sideporch` API.
//!
//! The same sandbox runs live scripts and dry runs from the editor. A
//! [`Sink`] decides whether actions happen or are only described, and a
//! [`Storage`] whether `sideporch.set` writes to the database or to a
//! scratch copy.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use mlua::{
    Function, HookTriggers, IntoLuaMulti, Lua, LuaOptions, LuaSerdeExt as _, StdLib, Table, Value,
    VmState,
};
use rusqlite::Connection;
use serde::Serialize;
use tokio::sync::mpsc as async_mpsc;

use super::{MessageEvent, ReactionEvent, WebhookRequest};
use crate::store;

/// Instructions a single call may run, checked every 1,000 instructions.
const INSTRUCTION_BUDGET: u32 = 2_000_000;
const HOOK_INTERVAL: u32 = 1_000;
const MEMORY_LIMIT: usize = 16 * 1024 * 1024;
/// Messages and reactions a single call may send.
const ACTION_LIMIT: usize = 20;
const MIN_INTERVAL_SECS: u64 = 10;
const MAX_VALUE_BYTES: usize = 64 * 1024;
const MAX_LOG_LINES: usize = 200;
const MAX_LOG_LINE: usize = 2_000;
const MAX_RESPONSE_BYTES: usize = 256 * 1024;
/// The chunk name, so errors read `script:3: …` and give the line.
const CHUNK: &str = "=script";

/// Something a script asked Sideporch to do.
#[derive(Debug, Clone)]
pub enum Action {
    Post {
        automation_id: i64,
        name: String,
        channel: ChannelRef,
        text: String,
        thread: Option<i64>,
    },
    React {
        automation_id: i64,
        channel_id: i64,
        message_id: i64,
        emoji: String,
    },
}

#[derive(Debug, Clone)]
pub enum ChannelRef {
    /// A channel id, with the channel's name for run logs if known.
    Id(i64, Option<String>),
    Name(String),
}

impl Action {
    /// One line for run logs and dry runs.
    pub fn describe(&self) -> String {
        match self {
            Self::Post {
                channel,
                text,
                thread,
                ..
            } => {
                let place = match channel {
                    ChannelRef::Name(name) => format!("#{}", name.trim_start_matches('#')),
                    ChannelRef::Id(_, Some(name)) => format!("#{name}"),
                    ChannelRef::Id(id, None) => format!("channel {id}"),
                };
                let thread = thread.map_or_else(String::new, |id| format!(" (thread {id})"));
                format!("→ post in {place}{thread}: {text}")
            }
            Self::React {
                message_id, emoji, ..
            } => format!("→ react :{emoji}: to message {message_id}"),
        }
    }
}

/// Where a script's actions go.
#[derive(Clone)]
pub enum Sink {
    Live(async_mpsc::UnboundedSender<Action>),
    /// Collects actions instead of carrying them out.
    Dry(Arc<Mutex<Vec<Action>>>),
}

/// Where `sideporch.get` and `sideporch.set` keep data.
#[derive(Clone)]
pub enum Storage {
    Live(Arc<Mutex<Connection>>),
    /// A scratch copy, so dry runs never change saved data.
    Dry(Arc<Mutex<HashMap<String, String>>>),
}

impl Storage {
    fn get(&self, id: i64, key: &str) -> mlua::Result<Option<String>> {
        match self {
            Self::Live(conn) => {
                let conn = conn.lock().map_err(|_| unavailable())?;
                store::automation_value(&conn, id, key).map_err(mlua::Error::runtime)
            }
            Self::Dry(data) => Ok(data.lock().map_err(|_| unavailable())?.get(key).cloned()),
        }
    }

    fn set(&self, id: i64, key: &str, value: Option<&str>) -> mlua::Result<()> {
        match self {
            Self::Live(conn) => {
                let conn = conn.lock().map_err(|_| unavailable())?;
                store::set_automation_value(&conn, id, key, value).map_err(mlua::Error::runtime)
            }
            Self::Dry(data) => {
                let mut data = data.lock().map_err(|_| unavailable())?;
                match value {
                    Some(value) => data.insert(key.to_owned(), value.to_owned()),
                    None => data.remove(key),
                };
                drop(data);
                Ok(())
            }
        }
    }
}

fn unavailable() -> mlua::Error {
    mlua::Error::runtime("data storage is unavailable")
}

/// A response to a webhook request.
#[derive(Debug, Clone, Serialize)]
pub struct WebhookResponse {
    pub status: u16,
    pub content_type: String,
    pub body: String,
}

impl WebhookResponse {
    pub fn text(status: u16, body: &str) -> Self {
        Self {
            status,
            content_type: "text/plain; charset=utf-8".to_owned(),
            body: body.to_owned(),
        }
    }
}

pub struct Timer {
    pub every: Duration,
    pub next: Instant,
    pub callback: Function,
}

/// A loaded script and the handlers it registered.
pub struct Script {
    pub id: i64,
    pub lua: Lua,
    pub on_message: Vec<Function>,
    pub on_reaction: Vec<Function>,
    pub on_webhook: Option<Function>,
    pub timers: Vec<Timer>,
}

/// What one call did.
#[derive(Debug, Default)]
pub struct Outcome {
    /// Printed lines and descriptions of actions, in order.
    pub log: Vec<String>,
    pub error: Option<String>,
    pub instructions: u32,
    pub duration: Duration,
}

impl Outcome {
    /// Adds a later call's log and result to this one.
    pub fn absorb(&mut self, other: Self) {
        self.log.extend(other.log);
        if self.error.is_none() {
            self.error = other.error;
        }
        self.instructions = self.instructions.saturating_add(other.instructions);
        self.duration = self.duration.saturating_add(other.duration);
    }
}

/// Per-state data that the `sideporch` functions reach through app data.
#[derive(Default)]
struct Registry {
    on_message: Vec<Function>,
    on_reaction: Vec<Function>,
    on_webhook: Option<Function>,
    timers: Vec<Timer>,
}

/// Limits and output of the current call.
#[derive(Default)]
struct Call {
    instructions: u32,
    actions: usize,
    log: Vec<String>,
}

impl Call {
    fn push(&mut self, line: &str) {
        if self.log.len() < MAX_LOG_LINES {
            self.log.push(line.chars().take(MAX_LOG_LINE).collect());
        } else if self.log.len() == MAX_LOG_LINES {
            self.log.push("… more output was dropped".to_owned());
        }
    }
}

/// Who a script acts as.
#[derive(Debug, Clone)]
pub struct Identity {
    pub id: i64,
    pub name: String,
}

/// Runs a script's top level, which registers its handlers and timers.
pub fn load(
    identity: &Identity,
    source: &str,
    storage: Storage,
    sink: &Sink,
) -> (Option<Script>, Outcome) {
    let lua = match sandbox(identity, storage, sink) {
        Ok(lua) => lua,
        Err(error) => {
            let outcome = Outcome {
                error: Some(clean_error(&error)),
                ..Outcome::default()
            };
            return (None, outcome);
        }
    };
    lua.set_app_data(Registry::default());
    let chunk = lua
        .load(source)
        .set_name(CHUNK)
        .set_mode(mlua::prelude::LuaChunkMode::Text);
    let outcome = measure(&lua, || chunk.exec());
    let registry = lua.remove_app_data::<Registry>().unwrap_or_default();
    if outcome.error.is_some() {
        return (None, outcome);
    }
    let script = Script {
        id: identity.id,
        lua,
        on_message: registry.on_message,
        on_reaction: registry.on_reaction,
        on_webhook: registry.on_webhook,
        timers: registry.timers,
    };
    (Some(script), outcome)
}

/// Calls a handler with a fresh budget and log.
pub fn call(lua: &Lua, function: &Function, args: impl IntoLuaMulti) -> Outcome {
    measure(lua, || function.call::<()>(args))
}

/// Calls a webhook handler and turns its return value into a response.
pub fn call_webhook(
    lua: &Lua,
    function: &Function,
    request: &WebhookRequest,
) -> (Outcome, Option<WebhookResponse>) {
    let mut response = None;
    let outcome = measure(lua, || {
        let value = function.call::<Value>(request_table(lua, request)?)?;
        response = Some(response_from(lua, value)?);
        Ok(())
    });
    (outcome, response)
}

fn measure(lua: &Lua, run: impl FnOnce() -> mlua::Result<()>) -> Outcome {
    lua.set_app_data(Call::default());
    let started = Instant::now();
    let result = run();
    let duration = started.elapsed();
    let call = lua.remove_app_data::<Call>().unwrap_or_default();
    Outcome {
        log: call.log,
        error: result.err().map(|error| clean_error(&error)),
        instructions: call.instructions,
        duration,
    }
}

/// A readable error: `line 3: attempt to index a nil value`.
pub fn clean_error(error: &mlua::Error) -> String {
    match error {
        mlua::Error::CallbackError { cause, traceback } => {
            let message = clean_error(cause);
            if message.starts_with("line ") {
                return message;
            }
            // Errors raised by Rust functions carry the line in the traceback.
            traceback
                .lines()
                .find_map(script_line)
                .map_or_else(|| message.clone(), |line| format!("line {line}: {message}"))
        }
        mlua::Error::WithContext { cause, .. } => clean_error(cause),
        mlua::Error::MemoryError(_) => "stopped: the script used too much memory".to_owned(),
        other => {
            let text = other.to_string();
            let first = text
                .lines()
                .find(|line| !line.trim().is_empty() && !line.starts_with("stack traceback"))
                .unwrap_or("unknown error")
                .trim_start_matches("runtime error: ")
                .trim_start_matches("syntax error: ");
            match first
                .strip_prefix("script:")
                .and_then(|rest| rest.split_once(": "))
            {
                Some((line, message)) if line.chars().all(|c| c.is_ascii_digit()) => {
                    format!("line {line}: {message}")
                }
                _ => first.to_owned(),
            }
        }
    }
}

/// The line number in a traceback entry such as `script:5: in function …`.
fn script_line(entry: &str) -> Option<&str> {
    let rest = entry.trim().strip_prefix("script:")?;
    let (line, _) = rest.split_once(':')?;
    line.chars().all(|c| c.is_ascii_digit()).then_some(line)
}

fn sandbox(identity: &Identity, storage: Storage, sink: &Sink) -> mlua::Result<Lua> {
    let lua = Lua::new_with(
        StdLib::STRING | StdLib::TABLE | StdLib::MATH | StdLib::UTF8 | StdLib::COROUTINE,
        LuaOptions::default(),
    )?;
    lua.set_memory_limit(MEMORY_LIMIT)?;
    lua.set_global_hook(
        HookTriggers::new().every_nth_instruction(HOOK_INTERVAL),
        |lua, debug| {
            let exhausted = lua.app_data_mut::<Call>().is_some_and(|mut call| {
                call.instructions = call.instructions.saturating_add(HOOK_INTERVAL);
                call.instructions > INSTRUCTION_BUDGET
            });
            if exhausted {
                let message = "stopped: the script ran too long";
                Err(mlua::Error::runtime(debug.current_line().map_or_else(
                    || message.to_owned(),
                    |line| format!("line {line}: {message}"),
                )))
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
    globals.set(
        "print",
        lua.create_function(|lua, values: mlua::Variadic<Value>| {
            let line: Vec<String> = values
                .iter()
                .map(|value| value.to_string().unwrap_or_default())
                .collect();
            log(lua, &line.join("\t"));
            Ok(())
        })?,
    )?;
    let api = lua.create_table()?;
    register_triggers(&lua, &api)?;
    register_actions(&lua, &api, identity, sink)?;
    register_storage(&lua, &api, identity.id, storage)?;
    register_json(&lua, &api)?;
    api.set(
        "now",
        lua.create_function(|_, ()| Ok(jiff::Timestamp::now().as_second()))?,
    )?;
    globals.set("sideporch", api)?;
    Ok(lua)
}

fn log(lua: &Lua, line: &str) {
    if let Some(mut call) = lua.app_data_mut::<Call>() {
        call.push(line);
    }
}

fn registry(lua: &Lua) -> mlua::Result<mlua::AppDataRefMut<'_, Registry>> {
    lua.app_data_mut::<Registry>().ok_or_else(|| {
        mlua::Error::runtime("handlers can only be registered while the script loads")
    })
}

/// `on_message`, `on_reaction`, `on_webhook` and `every`.
fn register_triggers(lua: &Lua, api: &Table) -> mlua::Result<()> {
    api.set(
        "on_message",
        lua.create_function(|lua, handler: Function| {
            registry(lua)?.on_message.push(handler);
            Ok(())
        })?,
    )?;
    api.set(
        "on_reaction",
        lua.create_function(|lua, handler: Function| {
            registry(lua)?.on_reaction.push(handler);
            Ok(())
        })?,
    )?;
    api.set(
        "on_webhook",
        lua.create_function(|lua, handler: Function| {
            // A second handler fails the load, so replacing the first is fine.
            if registry(lua)?.on_webhook.replace(handler).is_some() {
                return Err(mlua::Error::runtime(
                    "sideporch.on_webhook: a script can have only one webhook handler",
                ));
            }
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

/// Counts an action against the current call's limit, logs it, and hands
/// it to the sink.
fn act(lua: &Lua, sink: &Sink, action: Action) -> mlua::Result<()> {
    let actions = {
        let mut call = lua
            .app_data_mut::<Call>()
            .ok_or_else(|| mlua::Error::runtime("actions are not allowed here"))?;
        call.actions = call.actions.saturating_add(1);
        call.actions
    };
    if actions > ACTION_LIMIT {
        return Err(mlua::Error::runtime(
            "stopped: too many posts or reactions in one run",
        ));
    }
    log(lua, &action.describe());
    match sink {
        Sink::Live(actions) => actions
            .send(action)
            .map_err(|_| mlua::Error::runtime("Sideporch is shutting down")),
        Sink::Dry(actions) => {
            actions
                .lock()
                .map_err(|_| mlua::Error::runtime("dry run failed"))?
                .push(action);
            Ok(())
        }
    }
}

/// `post`, `reply` and `react`.
fn register_actions(lua: &Lua, api: &Table, identity: &Identity, sink: &Sink) -> mlua::Result<()> {
    let poster = |sink: Sink| {
        let identity = identity.clone();
        move |lua: &Lua, channel: ChannelRef, text: String, thread: Option<i64>| {
            let text = text.trim();
            if text.is_empty() {
                return Err(mlua::Error::runtime("sideporch.post: the text is empty"));
            }
            act(
                lua,
                &sink,
                Action::Post {
                    automation_id: identity.id,
                    name: identity.name.clone(),
                    channel,
                    text: text.chars().take(10_000).collect(),
                    thread,
                },
            )
        }
    };
    let post = poster(sink.clone());
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
    let reply = poster(sink.clone());
    api.set(
        "reply",
        lua.create_function(move |lua, (message, text): (Table, String)| {
            let channel: i64 = message.get("channel_id")?;
            let thread = message
                .get::<Option<i64>>("thread_id")?
                .or(message.get::<Option<i64>>("id")?);
            let name = message.get::<Option<String>>("channel")?;
            reply(lua, ChannelRef::Id(channel, name), text, thread)
        })?,
    )?;
    let sink = sink.clone();
    let automation_id = identity.id;
    api.set(
        "react",
        lua.create_function(move |lua, (message, emoji): (Table, String)| {
            let emoji = emoji.trim().trim_matches(':').to_owned();
            if emoji.is_empty() {
                return Err(mlua::Error::runtime("sideporch.react: name an emoji"));
            }
            act(
                lua,
                &sink,
                Action::React {
                    automation_id,
                    channel_id: message.get("channel_id")?,
                    message_id: message.get("id")?,
                    emoji,
                },
            )
        })?,
    )
}

/// `get` and `set`, stored per automation.
fn register_storage(lua: &Lua, api: &Table, id: i64, storage: Storage) -> mlua::Result<()> {
    let reader = storage.clone();
    api.set(
        "get",
        lua.create_function(move |_, key: String| reader.get(id, &key))?,
    )?;
    api.set(
        "set",
        lua.create_function(move |_, (key, value): (String, Option<Value>)| {
            let value = match value {
                None | Some(Value::Nil) => None,
                Some(Value::String(text)) => Some(text.to_str()?.to_owned()),
                Some(Value::Table(_)) => {
                    return Err(mlua::Error::runtime(
                        "sideporch.set: save tables with sideporch.json.encode",
                    ));
                }
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
            storage.set(id, &key, value.as_deref())
        })?,
    )
}

/// `sideporch.json`.
fn register_json(lua: &Lua, api: &Table) -> mlua::Result<()> {
    let json = lua.create_table()?;
    json.set(
        "encode",
        lua.create_function(|lua, value: Value| {
            let value: serde_json::Value = lua
                .from_value(value)
                .map_err(|error| mlua::Error::runtime(format!("sideporch.json.encode: {error}")))?;
            Ok(value.to_string())
        })?,
    )?;
    json.set(
        "decode",
        lua.create_function(|lua, text: String| {
            let value: serde_json::Value = serde_json::from_str(&text)
                .map_err(|error| mlua::Error::runtime(format!("sideporch.json.decode: {error}")))?;
            lua.to_value(&value)
        })?,
    )?;
    json.set(
        "array",
        lua.create_function(|lua, items: Option<Table>| {
            let items = match items {
                Some(items) => items,
                None => lua.create_table()?,
            };
            items.set_metatable(Some(lua.array_metatable()))?;
            Ok(items)
        })?,
    )?;
    json.set("null", lua.null())?;
    api.set("json", json)
}

pub fn message_table(lua: &Lua, event: &MessageEvent) -> mlua::Result<Table> {
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

pub fn reaction_table(lua: &Lua, event: &ReactionEvent) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set("emoji", event.emoji.as_str())?;
    table.set("added", event.added)?;
    table.set("user", event.user.as_str())?;
    table.set("username", event.username.as_str())?;
    table.set("message", message_table(lua, &event.message)?)?;
    Ok(table)
}

fn request_table(lua: &Lua, request: &WebhookRequest) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set("method", request.method.as_str())?;
    table.set("path", request.path.as_str())?;
    let query = lua.create_table()?;
    for (name, value) in &request.query {
        query.set(name.as_str(), value.as_str())?;
    }
    table.set("query", query)?;
    let headers = lua.create_table()?;
    for (name, value) in &request.headers {
        headers.set(name.to_ascii_lowercase(), value.as_str())?;
    }
    table.set("headers", headers)?;
    table.set("body", request.body.as_str())?;
    let json = serde_json::from_str::<serde_json::Value>(&request.body)
        .ok()
        .map(|value| lua.to_value(&value))
        .transpose()?;
    table.set("json", json)?;
    Ok(table)
}

fn response_from(lua: &Lua, value: Value) -> mlua::Result<WebhookResponse> {
    let response = match value {
        Value::Nil => WebhookResponse::text(204, ""),
        Value::String(text) => WebhookResponse::text(200, &text.to_str()?),
        Value::Table(table) => {
            let status = table.get::<Option<u16>>("status")?.unwrap_or(200);
            if !(200..=599).contains(&status) {
                return Err(mlua::Error::runtime(
                    "webhook response: status must be between 200 and 599",
                ));
            }
            let json = table.get::<Value>("json")?;
            if json.is_nil() {
                let body = table.get::<Option<String>>("body")?.unwrap_or_default();
                let mut response = WebhookResponse::text(status, &body);
                if let Some(content_type) = table.get::<Option<String>>("content_type")? {
                    response.content_type = content_type;
                }
                response
            } else {
                let value: serde_json::Value = lua.from_value(json)?;
                WebhookResponse {
                    status,
                    content_type: "application/json".to_owned(),
                    body: value.to_string(),
                }
            }
        }
        _ => {
            return Err(mlua::Error::runtime(
                "webhook response: return nil, a string, or a table",
            ));
        }
    };
    if response.body.len() > MAX_RESPONSE_BYTES {
        return Err(mlua::Error::runtime(
            "webhook response: the body is too large",
        ));
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dry(source: &str) -> (Option<Script>, Outcome, Arc<Mutex<Vec<Action>>>) {
        let actions = Arc::new(Mutex::new(Vec::new()));
        let (script, outcome) = load(
            &Identity {
                id: 1,
                name: "Test".to_owned(),
            },
            source,
            Storage::Dry(Arc::default()),
            &Sink::Dry(Arc::clone(&actions)),
        );
        (script, outcome, actions)
    }

    #[test]
    fn errors_name_the_line() {
        let (script, outcome, _) = dry("local x = 1\nlocal y = nil\nprint(y.z)");
        assert!(script.is_none());
        assert_eq!(
            outcome.error.as_deref(),
            Some("line 3: attempt to index a nil value (local 'y')")
        );
        let (_, outcome, _) = dry("\n\nsideporch.every(1, function() end)");
        assert_eq!(
            outcome.error.as_deref(),
            Some("line 3: sideporch.every: use at least 10 seconds")
        );
    }

    #[test]
    fn logs_prints_and_actions() {
        let (script, outcome, actions) = dry(
            "print('loaded', 1)\nsideporch.post('#general', 'hi', { thread = 7 })\nsideporch.on_message(function() end)",
        );
        let script = script.unwrap();
        assert_eq!(script.on_message.len(), 1);
        assert_eq!(
            outcome.log,
            ["loaded\t1", "→ post in #general (thread 7): hi"]
        );
        assert_eq!(actions.lock().unwrap().len(), 1);
    }

    #[test]
    fn stops_runaway_scripts() {
        let (_, outcome, _) = dry("while true do end");
        assert_eq!(
            outcome.error.as_deref(),
            Some("line 1: stopped: the script ran too long")
        );
        let (_, outcome, _) = dry("for i = 1, 25 do sideporch.post('general', 'x') end");
        assert!(
            outcome
                .error
                .unwrap()
                .contains("too many posts or reactions"),
        );
    }

    #[test]
    fn round_trips_json() {
        let (_, outcome, _) = dry(r#"local v = sideporch.json.decode('{"a":[1,2],"b":null}')
print(v.a[2], v.b == sideporch.json.null)
print(sideporch.json.encode({ ok = true }), sideporch.json.encode(sideporch.json.array()))"#);
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.log, ["2\ttrue", "{\"ok\":true}\t[]"]);
    }

    #[test]
    fn answers_webhooks() {
        let (script, _, _) = dry(r#"sideporch.on_webhook(function(req)
  if req.path == "/json" then return { json = { got = req.json.n } } end
  if req.path == "/empty" then return nil end
  return { status = 202, body = req.method .. " " .. req.query.q }
end)"#);
        let script = script.unwrap();
        let handler = script.on_webhook.as_ref().unwrap();
        let request = |path: &str| WebhookRequest {
            method: "POST".to_owned(),
            path: path.to_owned(),
            query: vec![("q".to_owned(), "hi".to_owned())],
            headers: Vec::new(),
            body: r#"{"n": 5}"#.to_owned(),
        };
        let (_, response) = call_webhook(&script.lua, handler, &request(""));
        let response = response.unwrap();
        assert_eq!((response.status, response.body.as_str()), (202, "POST hi"));
        let (_, response) = call_webhook(&script.lua, handler, &request("/json"));
        assert_eq!(response.unwrap().body, r#"{"got":5}"#);
        let (_, response) = call_webhook(&script.lua, handler, &request("/empty"));
        assert_eq!(response.unwrap().status, 204);
    }

    #[test]
    fn refuses_a_second_webhook_handler() {
        let (_, outcome, _) =
            dry("sideporch.on_webhook(function() end)\nsideporch.on_webhook(function() end)");
        assert!(outcome.error.unwrap().contains("only one webhook handler"));
    }
}
