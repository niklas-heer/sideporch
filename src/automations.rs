//! Automations: small Lua scripts that admins write in the browser.
//!
//! Every enabled script runs in its own sandboxed Lua 5.4 state on one
//! dedicated thread. Scripts react to new messages, reactions, webhook
//! requests and timers through a `sideporch` table, described in
//! [`api`]. [`sandbox`] builds the Lua states, [`tooling`] lints and
//! formats scripts.
//!
//! Each call has an instruction budget and each state a memory limit, so a
//! runaway script fails with an error instead of stalling the server.
//! Messages and reactions from automations never trigger automations.
//! Calls that print, act, or fail are kept in a per-script run log.

pub mod api;
pub mod sandbox;
pub mod tooling;

use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc as async_mpsc, oneshot};

use crate::{
    AppState, db,
    error::{AppError, AppResult},
    messages::{self, Draft, Sender},
    now_ms,
    store::{self, Author, Message},
};
pub use sandbox::WebhookResponse;
use sandbox::{Action, ChannelRef, Identity, Outcome, Script, Sink, Storage};

/// How long a webhook request waits for its script.
const WEBHOOK_TIMEOUT: Duration = Duration::from_secs(10);

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

/// Someone added or removed a reaction.
#[derive(Debug, Clone)]
pub struct ReactionEvent {
    pub emoji: String,
    pub added: bool,
    pub user: String,
    pub username: String,
    pub message: MessageEvent,
}

/// An HTTP request to an automation's webhook URL.
#[derive(Debug, Clone, Default)]
pub struct WebhookRequest {
    pub method: String,
    /// The part of the path after the token, such as `/deploy`, or empty.
    pub path: String,
    pub query: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

enum Command {
    Reload(Vec<store::Automation>, oneshot::Sender<()>),
    Message(MessageEvent),
    Reaction(ReactionEvent),
    Webhook {
        automation_id: i64,
        request: WebhookRequest,
        respond: oneshot::Sender<WebhookResponse>,
    },
}

/// Handle to the automation thread.
#[derive(Clone)]
pub struct Automations {
    commands: mpsc::Sender<Command>,
    actions: Arc<Mutex<Option<async_mpsc::UnboundedReceiver<Action>>>>,
}

impl Automations {
    /// Starts the automation thread. It keeps its own database connection
    /// for script data and run logs.
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

    // Sending fails only if the thread is gone, which shutdown causes.

    pub fn message(&self, event: MessageEvent) {
        drop(self.commands.send(Command::Message(event)));
    }

    pub fn reaction(&self, event: ReactionEvent) {
        drop(self.commands.send(Command::Reaction(event)));
    }

    /// Hands a request to the automation's webhook handler and waits for
    /// its response.
    pub async fn webhook(&self, automation_id: i64, request: WebhookRequest) -> WebhookResponse {
        let (respond, response) = oneshot::channel();
        let sent = self.commands.send(Command::Webhook {
            automation_id,
            request,
            respond,
        });
        if sent.is_err() {
            return WebhookResponse::text(503, "Automations are not running.");
        }
        match tokio::time::timeout(WEBHOOK_TIMEOUT, response).await {
            Ok(Ok(response)) => response,
            _ => WebhookResponse::text(504, "The automation did not answer in time."),
        }
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
    let (automation_id, result) = match action {
        Action::Post {
            automation_id,
            name,
            channel,
            text,
            thread,
        } => (
            automation_id,
            post(state, automation_id, name, channel, text, thread).await,
        ),
        Action::React {
            automation_id,
            channel_id,
            message_id,
            emoji,
        } => (
            automation_id,
            messages::automation_reaction(state, automation_id, channel_id, message_id, emoji)
                .await,
        ),
    };
    if let Err(error) = result {
        let message = error.to_string();
        drop(
            state
                .db
                .call(move |conn| {
                    store::set_automation_error(conn, automation_id, Some(&message))?;
                    store::record_automation_run(
                        conn,
                        automation_id,
                        "action",
                        now_ms(),
                        0,
                        "",
                        Some(&message),
                    )
                })
                .await,
        );
    }
}

async fn post(
    state: &AppState,
    automation_id: i64,
    name: String,
    channel: ChannelRef,
    text: String,
    thread: Option<i64>,
) -> AppResult<()> {
    let channel_id = state
        .db
        .call(move |conn| match channel {
            ChannelRef::Id(id, _) => store::public_channel(conn, id),
            ChannelRef::Name(name) => store::public_channel_id(conn, name.trim_start_matches('#')),
        })
        .await?
        .ok_or_else(|| AppError::bad_request("sideporch.post: no public channel with that name"))?;
    messages::post(
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
    .map(drop)
}

fn run(
    commands: &mpsc::Receiver<Command>,
    conn: &Arc<Mutex<Connection>>,
    actions: &async_mpsc::UnboundedSender<Action>,
) {
    let mut scripts: Vec<Script> = Vec::new();
    // Which scripts last failed, so quiet successes only write when that changes.
    let mut failing: HashMap<i64, bool> = HashMap::new();
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
                (scripts, failing) = load_all(automations, conn, actions);
                // The caller may have stopped waiting.
                let _ = done.send(());
            }
            Ok(Command::Message(event)) => {
                for script in &scripts {
                    for handler in &script.on_message {
                        let outcome = match sandbox::message_table(&script.lua, &event) {
                            Ok(table) => sandbox::call(&script.lua, handler, table),
                            Err(error) => failed(&error),
                        };
                        record(conn, &mut failing, script.id, "message", &outcome);
                    }
                }
            }
            Ok(Command::Reaction(event)) => {
                for script in &scripts {
                    for handler in &script.on_reaction {
                        let outcome = match sandbox::reaction_table(&script.lua, &event) {
                            Ok(table) => sandbox::call(&script.lua, handler, table),
                            Err(error) => failed(&error),
                        };
                        record(conn, &mut failing, script.id, "reaction", &outcome);
                    }
                }
            }
            Ok(Command::Webhook {
                automation_id,
                request,
                respond,
            }) => {
                let script = scripts.iter().find(|script| script.id == automation_id);
                let response = answer_webhook(script, &request, conn, &mut failing);
                // The caller may have timed out.
                let _ = respond.send(response);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
        let now = Instant::now();
        for script in &mut scripts {
            for timer in &mut script.timers {
                if timer.next <= now {
                    timer.next = now.checked_add(timer.every).unwrap_or(now);
                    let outcome = sandbox::call(&script.lua, &timer.callback, ());
                    record(conn, &mut failing, script.id, "timer", &outcome);
                }
            }
        }
    }
}

/// Loads every enabled script, and notes which automations have an error.
fn load_all(
    automations: Vec<store::Automation>,
    conn: &Arc<Mutex<Connection>>,
    actions: &async_mpsc::UnboundedSender<Action>,
) -> (Vec<Script>, HashMap<i64, bool>) {
    let mut failing: HashMap<i64, bool> = automations
        .iter()
        .map(|automation| (automation.id, automation.last_error.is_some()))
        .collect();
    let scripts = automations
        .into_iter()
        .filter(|automation| automation.enabled)
        .filter_map(|automation| {
            let identity = Identity {
                id: automation.id,
                name: automation.name,
            };
            let (script, outcome) = sandbox::load(
                &identity,
                &automation.source,
                Storage::Live(Arc::clone(conn)),
                &Sink::Live(actions.clone()),
            );
            record(conn, &mut failing, identity.id, "load", &outcome);
            script
        })
        .collect();
    (scripts, failing)
}

fn answer_webhook(
    script: Option<&Script>,
    request: &WebhookRequest,
    conn: &Arc<Mutex<Connection>>,
    failing: &mut HashMap<i64, bool>,
) -> WebhookResponse {
    let Some(script) = script else {
        return WebhookResponse::text(503, "This automation is switched off or failed to load.");
    };
    let Some(handler) = &script.on_webhook else {
        return WebhookResponse::text(404, "This automation has no webhook handler.");
    };
    let (mut outcome, response) = sandbox::call_webhook(&script.lua, handler, request);
    outcome
        .log
        .insert(0, format!("{} {}", request.method, display_path(request)));
    failing.insert(script.id, outcome.error.is_some());
    record_always(conn, script.id, "webhook", &outcome);
    response.unwrap_or_else(|| WebhookResponse::text(500, "The automation failed."))
}

fn display_path(request: &WebhookRequest) -> String {
    if request.path.is_empty() {
        "/".to_owned()
    } else {
        request.path.clone()
    }
}

fn failed(error: &mlua::Error) -> Outcome {
    Outcome {
        error: Some(sandbox::clean_error(error)),
        ..Outcome::default()
    }
}

/// Stores the script's status, and the call in the run log if it printed,
/// acted, or failed.
fn record(
    conn: &Arc<Mutex<Connection>>,
    failing: &mut HashMap<i64, bool>,
    id: i64,
    trigger: &str,
    outcome: &Outcome,
) {
    let failed_before = failing.insert(id, outcome.error.is_some()).unwrap_or(false);
    if outcome.log.is_empty() && outcome.error.is_none() {
        // A quiet success only clears an earlier error.
        if failed_before
            && let Ok(conn) = conn.lock()
            && let Err(error) = store::set_automation_error(&conn, id, None)
        {
            tracing::warn!(?error, "could not record automation status");
        }
        return;
    }
    record_always(conn, id, trigger, outcome);
}

fn record_always(conn: &Arc<Mutex<Connection>>, id: i64, trigger: &str, outcome: &Outcome) {
    if let Some(error) = &outcome.error {
        tracing::warn!(automation = id, trigger, %error, "automation failed");
    }
    let Ok(conn) = conn.lock() else {
        return;
    };
    let duration = i64::try_from(outcome.duration.as_micros()).unwrap_or(i64::MAX);
    let result = store::set_automation_error(&conn, id, outcome.error.as_deref()).and_then(|()| {
        store::record_automation_run(
            &conn,
            id,
            trigger,
            now_ms(),
            duration,
            &outcome.log.join("\n"),
            outcome.error.as_deref(),
        )
    });
    if let Err(error) = result {
        tracing::warn!(?error, "could not record automation run");
    }
}

/// What a dry run should simulate.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TestTrigger {
    /// Only load the script.
    Load,
    Message {
        #[serde(default)]
        text: String,
        #[serde(default = "default_channel")]
        channel: String,
        #[serde(default = "default_author")]
        author: String,
    },
    Reaction {
        #[serde(default = "default_emoji")]
        emoji: String,
        #[serde(default = "yes")]
        added: bool,
        #[serde(default)]
        text: String,
        #[serde(default = "default_channel")]
        channel: String,
    },
    Webhook {
        #[serde(default = "default_method")]
        method: String,
        #[serde(default)]
        path: String,
        #[serde(default)]
        body: String,
    },
    /// Calls every timer once.
    Timer,
}

fn default_channel() -> String {
    "general".to_owned()
}
fn default_author() -> String {
    "Test Person".to_owned()
}
fn default_emoji() -> String {
    "thumbsup".to_owned()
}
fn default_method() -> String {
    "POST".to_owned()
}
const fn yes() -> bool {
    true
}

/// The result of a dry run.
#[derive(Debug, Serialize)]
pub struct TestReport {
    pub ok: bool,
    pub error: Option<String>,
    /// Printed lines and would-be actions, in order.
    pub log: Vec<String>,
    /// Handlers the script registered.
    pub handlers: Handlers,
    /// How many handlers the simulated event reached.
    pub called: usize,
    pub response: Option<WebhookResponse>,
    pub instructions: u32,
    pub duration_ms: f64,
}

#[derive(Debug, Default, Serialize)]
pub struct Handlers {
    pub message: usize,
    pub reaction: usize,
    pub webhook: bool,
    pub timers: usize,
}

/// Runs `source` in a fresh sandbox against a simulated event. Posts and
/// reactions are only described, and `sideporch.set` changes a copy of
/// `data`, so nothing in Sideporch changes. Blocks; run it off the async
/// threads.
pub fn test(
    name: &str,
    source: &str,
    data: HashMap<String, String>,
    trigger: &TestTrigger,
) -> TestReport {
    let identity = Identity {
        id: 0,
        name: name.to_owned(),
    };
    let (script, mut outcome) = sandbox::load(
        &identity,
        source,
        Storage::Dry(Arc::new(Mutex::new(data))),
        &Sink::Dry(Arc::default()),
    );
    let mut report = TestReport {
        ok: false,
        error: None,
        log: Vec::new(),
        handlers: Handlers::default(),
        called: 0,
        response: None,
        instructions: 0,
        duration_ms: 0.0,
    };
    if let Some(script) = &script {
        report.handlers = Handlers {
            message: script.on_message.len(),
            reaction: script.on_reaction.len(),
            webhook: script.on_webhook.is_some(),
            timers: script.timers.len(),
        };
        if outcome.error.is_none() {
            simulate(script, trigger, &mut outcome, &mut report);
        }
    }
    report.ok = outcome.error.is_none();
    report.error = outcome.error;
    report.log = outcome.log;
    report.instructions = outcome.instructions;
    report.duration_ms = outcome.duration.as_secs_f64() * 1000.0;
    report
}

fn simulate(
    script: &Script,
    trigger: &TestTrigger,
    outcome: &mut Outcome,
    report: &mut TestReport,
) {
    let lua = &script.lua;
    let message = |text: &str, channel: &str, author: &str| MessageEvent {
        id: 1,
        channel_id: 1,
        channel: channel.trim_start_matches('#').to_owned(),
        text: text.to_owned(),
        author: author.to_owned(),
        username: Some("test".to_owned()),
        is_bot: false,
        thread_id: None,
    };
    let mut each = |handlers: &[mlua::Function], args: &dyn Fn() -> mlua::Result<mlua::Table>| {
        for handler in handlers {
            if outcome.error.is_some() {
                return;
            }
            report.called = report.called.saturating_add(1);
            outcome.absorb(match args() {
                Ok(table) => sandbox::call(lua, handler, table),
                Err(error) => failed(&error),
            });
        }
    };
    match trigger {
        TestTrigger::Load => {}
        TestTrigger::Message {
            text,
            channel,
            author,
        } => {
            let event = message(text, channel, author);
            each(&script.on_message, &|| sandbox::message_table(lua, &event));
        }
        TestTrigger::Reaction {
            emoji,
            added,
            text,
            channel,
        } => {
            let event = ReactionEvent {
                emoji: emoji.trim_matches(':').to_owned(),
                added: *added,
                user: default_author(),
                username: "test".to_owned(),
                message: message(text, channel, "Someone"),
            };
            each(&script.on_reaction, &|| {
                sandbox::reaction_table(lua, &event)
            });
        }
        TestTrigger::Timer => {
            let callbacks: Vec<mlua::Function> = script
                .timers
                .iter()
                .map(|timer| timer.callback.clone())
                .collect();
            for callback in &callbacks {
                if outcome.error.is_some() {
                    break;
                }
                report.called = report.called.saturating_add(1);
                outcome.absorb(sandbox::call(lua, callback, ()));
            }
        }
        TestTrigger::Webhook { method, path, body } => {
            if let Some(handler) = &script.on_webhook {
                report.called = 1;
                let request = WebhookRequest {
                    method: method.to_uppercase(),
                    path: path.clone(),
                    query: Vec::new(),
                    headers: vec![("content-type".to_owned(), "application/json".to_owned())],
                    body: body.clone(),
                };
                let (called, response) = sandbox::call_webhook(lua, handler, &request);
                outcome.absorb(called);
                report.response = response;
            }
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dry_runs_describe_actions_without_saving() {
        let source = r##"sideporch.on_message(function(msg)
  local n = tonumber(sideporch.get("n") or "0") + 1
  sideporch.set("n", n)
  print("seen", n)
  sideporch.reply(msg, "#" .. msg.channel .. " " .. msg.text)
end)"##;
        let data = HashMap::from([("n".to_owned(), "4".to_owned())]);
        let report = test(
            "Echo",
            source,
            data,
            &TestTrigger::Message {
                text: "hi".to_owned(),
                channel: "#garden".to_owned(),
                author: default_author(),
            },
        );
        assert!(report.ok, "{report:?}");
        assert_eq!(report.called, 1);
        assert_eq!(
            report.log,
            ["seen\t5", "→ post in #garden (thread 1): #garden hi"]
        );
    }

    #[test]
    fn dry_runs_report_errors_and_handlers() {
        let report = test(
            "Broken",
            "sideporch.on_reaction(function(e) error('nope: ' .. e.emoji) end)",
            HashMap::new(),
            &TestTrigger::Reaction {
                emoji: ":tada:".to_owned(),
                added: true,
                text: String::new(),
                channel: default_channel(),
            },
        );
        assert!(!report.ok);
        assert_eq!(report.handlers.reaction, 1);
        assert_eq!(report.error.as_deref(), Some("line 1: nope: tada"));
    }
}
