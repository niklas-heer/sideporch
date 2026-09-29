//! Automations: small Lua scripts that admins write in the browser.
//!
//! Every enabled automation runs on its own thread ([`worker`]) with its own
//! sandboxed Lua 5.4 state ([`sandbox`]). Scripts react to events, webhook
//! requests, slash commands and schedules through a `sideporch` table
//! described in [`api`]; [`tooling`] lints and formats them. Library
//! automations hold shared code that other scripts `require`.
//!
//! Each call has an instruction budget and each state a memory limit, so a
//! runaway script fails with an error instead of stalling the server.
//! Messages and reactions from automations never trigger automations.
//! Calls that print, act, or fail are kept in a per-script run log, with
//! secret values replaced.

pub mod api;
pub mod bundle;
pub mod cron;
pub mod events;
pub mod http;
pub mod sandbox;
pub mod tooling;
pub mod worker;

use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
    time::Duration,
};

use jiff::tz::TimeZone;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc as async_mpsc, oneshot};

pub use events::{
    ButtonEvent, ChannelEvent, CommandCall, Event, MemberEvent, MessageEvent, ReactionEvent,
    WebhookRequest,
};
use sandbox::{Action, ChannelRef, Outcome, Script};
pub use sandbox::{Triggers, WebhookResponse};
pub use worker::CommandReply;
use worker::{Job, Worker};

use crate::{
    AppState,
    error::{AppError, AppResult},
    messages::{self, Draft, Sender},
    now_ms, secrets, store,
};

/// How long a webhook request or command waits for its script.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(15);

pub const KIND_AUTOMATION: &str = "automation";
pub const KIND_LIBRARY: &str = "library";
/// Longest script Sideporch accepts, in bytes.
pub const MAX_SOURCE_BYTES: usize = 100_000;

/// Library names: what `require` takes.
pub fn valid_library_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && name.len() <= 40
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Instance-wide automation settings.
#[derive(Debug, Clone)]
pub struct Settings {
    /// The default time zone for `sideporch.cron`.
    pub timezone: String,
    /// Whether `sideporch.http` may reach private and loopback addresses.
    pub allow_private_network: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            timezone: "UTC".to_owned(),
            allow_private_network: false,
        }
    }
}

const TIMEZONE_SETTING: &str = "automations.timezone";
const PRIVATE_NETWORK_SETTING: &str = "automations.allow_private_network";

impl Settings {
    pub fn load(conn: &Connection) -> AppResult<Self> {
        Ok(Self {
            timezone: store::setting(conn, TIMEZONE_SETTING)?.unwrap_or_else(|| "UTC".to_owned()),
            allow_private_network: store::setting(conn, PRIVATE_NETWORK_SETTING)?.as_deref()
                == Some("true"),
        })
    }

    pub fn save(&self, conn: &Connection) -> AppResult<()> {
        if TimeZone::get(&self.timezone).is_err() {
            return Err(AppError::bad_request(format!(
                "`{}` is not a time zone. Use a name such as Europe/Berlin.",
                self.timezone
            )));
        }
        store::set_setting(conn, TIMEZONE_SETTING, &self.timezone)?;
        store::set_setting(
            conn,
            PRIVATE_NETWORK_SETTING,
            if self.allow_private_network {
                "true"
            } else {
                "false"
            },
        )
    }

    pub fn zone(&self) -> TimeZone {
        TimeZone::get(&self.timezone).unwrap_or(TimeZone::UTC)
    }
}

/// A registered slash command, for `/help`, the composer and MCP.
#[derive(Debug, Clone, Serialize)]
pub struct CommandInfo {
    pub name: String,
    pub description: String,
    pub usage: String,
    pub automation_id: i64,
    pub automation: String,
}

#[derive(Default)]
struct Workers {
    by_id: HashMap<i64, Worker>,
    commands: BTreeMap<String, CommandInfo>,
}

struct Inner {
    db_path: PathBuf,
    workers: RwLock<Workers>,
    actions: async_mpsc::UnboundedSender<Action>,
    receiver: Mutex<Option<async_mpsc::UnboundedReceiver<Action>>>,
}

/// Runs the enabled automations and routes events to them.
#[derive(Clone)]
pub struct Automations {
    inner: Arc<Inner>,
}

impl Automations {
    pub fn start(db_path: &Path) -> Self {
        let (actions, receiver) = async_mpsc::unbounded_channel();
        Self {
            inner: Arc::new(Inner {
                db_path: db_path.to_owned(),
                workers: RwLock::new(Workers::default()),
                actions,
                receiver: Mutex::new(Some(receiver)),
            }),
        }
    }

    /// Carries out what scripts ask for. Call once, after startup.
    pub fn serve(&self, state: AppState) {
        let receiver = self
            .inner
            .receiver
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

    /// Stops all workers and starts the enabled automations again, with the
    /// current libraries, secrets and settings. Returns once every script
    /// has loaded, so load errors are stored.
    pub async fn reload(&self, state: &AppState) -> AppResult<()> {
        let vault = Arc::clone(&state.vault);
        let (automations, secrets, settings) = state
            .db
            .call(move |conn| {
                Ok((
                    store::automations(conn)?,
                    secrets::all(conn, &vault)?,
                    Settings::load(conn)?,
                ))
            })
            .await?;
        let libraries: HashMap<String, String> = automations
            .iter()
            .filter(|automation| automation.kind == KIND_LIBRARY)
            .map(|library| (library.name.clone(), library.source.clone()))
            .collect();
        let shared = worker::Shared {
            db_path: self.inner.db_path.clone(),
            actions: self.inner.actions.clone(),
            libraries: Arc::new(libraries),
            secrets: Arc::new(secrets),
            http: Arc::new(http::Http::new(settings.allow_private_network)?),
            timezone: settings.zone(),
        };
        // Oldest first, so an existing command keeps working when a newer
        // automation tries to register the same name.
        let mut runnable: Vec<store::Automation> = automations
            .into_iter()
            .filter(|automation| automation.enabled && automation.kind == KIND_AUTOMATION)
            .collect();
        runnable.sort_by_key(|automation| automation.id);
        let (workers, conflicts) =
            tokio::task::spawn_blocking(move || start_all(&runnable, &shared))
                .await
                .map_err(AppError::internal)??;
        if let Ok(mut current) = self.inner.workers.write() {
            // Dropping the old workers ends their threads.
            *current = workers;
        }
        if !conflicts.is_empty() {
            state
                .db
                .call(move |conn| {
                    for (id, error) in conflicts {
                        store::set_automation_error(conn, id, Some(&error))?;
                        store::record_automation_run(
                            conn,
                            id,
                            "load",
                            now_ms(),
                            0,
                            "",
                            Some(&error),
                        )?;
                    }
                    Ok(())
                })
                .await?;
        }
        Ok(())
    }

    /// Hands `event` to every automation that listens for its kind.
    pub fn event(&self, event: Event) {
        let event = Arc::new(event);
        let Ok(workers) = self.inner.workers.read() else {
            return;
        };
        for worker in workers.by_id.values() {
            let listens = worker
                .triggers
                .lock()
                .is_ok_and(|triggers| triggers.listens_to(event.kind()));
            if listens {
                // Fails only if the worker stopped, which a reload causes.
                drop(worker.jobs.send(Job::Event(Arc::clone(&event))));
            }
        }
    }

    /// Hands `event` to one automation, if it listens for its kind.
    pub fn event_for(&self, automation_id: i64, event: Event) {
        let Ok(workers) = self.inner.workers.read() else {
            return;
        };
        if let Some(worker) = workers.by_id.get(&automation_id)
            && worker
                .triggers
                .lock()
                .is_ok_and(|triggers| triggers.listens_to(event.kind()))
        {
            drop(worker.jobs.send(Job::Event(Arc::new(event))));
        }
    }

    /// Hands a request to the automation's webhook handler and waits for
    /// its response.
    pub async fn webhook(&self, automation_id: i64, request: WebhookRequest) -> WebhookResponse {
        let (respond, response) = oneshot::channel();
        if !self.send(automation_id, Job::Webhook(request, respond)) {
            return WebhookResponse::text(
                503,
                "This automation is switched off or failed to load.",
            );
        }
        match tokio::time::timeout(ANSWER_TIMEOUT, response).await {
            Ok(Ok(response)) => response,
            _ => WebhookResponse::text(504, "The automation did not answer in time."),
        }
    }

    fn send(&self, automation_id: i64, job: Job) -> bool {
        self.inner.workers.read().is_ok_and(|workers| {
            workers
                .by_id
                .get(&automation_id)
                .is_some_and(|worker| worker.loaded && worker.jobs.send(job).is_ok())
        })
    }

    /// Runs a slash command. `None` if no automation registered it.
    pub async fn command(&self, command: CommandCall) -> Option<CommandReply> {
        let automation_id = self
            .inner
            .workers
            .read()
            .ok()?
            .commands
            .get(&command.name)?
            .automation_id;
        let (respond, reply) = oneshot::channel();
        if !self.send(automation_id, Job::Command(command, respond)) {
            return None;
        }
        Some(match tokio::time::timeout(ANSWER_TIMEOUT, reply).await {
            Ok(Ok(reply)) => reply,
            _ => CommandReply {
                error: Some("The command did not answer in time.".to_owned()),
                ..CommandReply::default()
            },
        })
    }

    /// The registered slash commands, by name.
    pub fn commands(&self) -> Vec<CommandInfo> {
        self.inner
            .workers
            .read()
            .map(|workers| workers.commands.values().cloned().collect())
            .unwrap_or_default()
    }

    /// What each running automation listens to.
    pub fn triggers(&self) -> HashMap<i64, Triggers> {
        self.inner
            .workers
            .read()
            .map(|workers| {
                workers
                    .by_id
                    .iter()
                    .filter_map(|(id, worker)| Some((*id, worker.triggers.lock().ok()?.clone())))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Runs the live automation against a simulated event; posts,
    /// reactions and requests really happen.
    pub async fn run(&self, automation_id: i64, trigger: TestTrigger) -> AppResult<RunReport> {
        let (respond, report) = oneshot::channel();
        if !self.send(automation_id, Job::Run(trigger, respond)) {
            return Err(AppError::bad_request(
                "The automation is switched off or failed to load.",
            ));
        }
        tokio::time::timeout(ANSWER_TIMEOUT, report)
            .await
            .map_err(|_| AppError::bad_request("The automation did not finish in time."))?
            .map_err(|_| AppError::internal("the automation stopped"))
    }
}

/// Starts a worker per automation. Returns the workers and, for commands
/// that two automations register, an error for the later one.
fn start_all(
    automations: &[store::Automation],
    shared: &worker::Shared,
) -> AppResult<(Workers, Vec<(i64, String)>)> {
    let mut workers = Workers::default();
    let mut conflicts = Vec::new();
    for automation in automations {
        let worker = worker::start(automation, shared)?;
        let commands = worker
            .triggers
            .lock()
            .map(|triggers| triggers.commands.clone())
            .unwrap_or_default();
        for command in commands {
            if let Some(owner) = workers.commands.get(&command.name) {
                conflicts.push((
                    automation.id,
                    format!(
                        "/{} is already registered by {}",
                        command.name, owner.automation
                    ),
                ));
                continue;
            }
            workers.commands.insert(
                command.name.clone(),
                CommandInfo {
                    name: command.name,
                    description: command.description,
                    usage: command.usage,
                    automation_id: automation.id,
                    automation: automation.name.clone(),
                },
            );
        }
        workers.by_id.insert(automation.id, worker);
    }
    Ok((workers, conflicts))
}

async fn perform(state: &AppState, action: Action) {
    let (automation_id, result) = match action {
        Action::Post {
            automation_id,
            name,
            channel,
            text,
            thread,
            buttons,
        } => (
            automation_id,
            post(state, automation_id, name, channel, (text, buttons), thread).await,
        ),
        Action::Update {
            automation_id,
            message_id,
            text,
            buttons,
        } => (
            automation_id,
            update(state, automation_id, message_id, text, buttons).await,
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

/// Changes a message the automation posted.
async fn update(
    state: &AppState,
    automation_id: i64,
    message_id: i64,
    text: Option<String>,
    buttons: Option<Vec<store::Button>>,
) -> AppResult<()> {
    state
        .db
        .call(move |conn| {
            let message = store::message(conn, message_id)?
                .filter(|message| message.automation_id == Some(automation_id))
                .ok_or_else(|| {
                    AppError::bad_request(
                        "sideporch.update: automations can only change their own messages",
                    )
                })?;
            store::update_bot_message(conn, message.id, text.as_deref(), buttons.as_deref())
        })
        .await?;
    messages::refresh(state, message_id).await
}

async fn post(
    state: &AppState,
    automation_id: i64,
    name: String,
    channel: ChannelRef,
    (text, buttons): (String, Vec<store::Button>),
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
            gif: None,
            poll: None,
            buttons,
        },
    )
    .await
    .map(drop)
}

/// What a dry or live run should simulate.
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
    MemberJoined {
        #[serde(default = "default_author")]
        user: String,
    },
    ChannelCreated {
        #[serde(default = "default_new_channel")]
        channel: String,
    },
    /// `/name args`, typed in `channel`.
    Command {
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
    /// Calls every schedule once.
    Timer,
}

fn default_channel() -> String {
    "general".to_owned()
}
fn default_new_channel() -> String {
    "new-channel".to_owned()
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
    /// Printed lines, requests and would-be actions, in order.
    pub log: Vec<String>,
    /// Private answers from a command.
    pub responses: Vec<String>,
    /// What the script registered.
    pub triggers: Triggers,
    /// For a library, the names its module exports.
    pub exports: Vec<String>,
    /// How many handlers the simulated event reached.
    pub called: usize,
    pub response: Option<WebhookResponse>,
    pub instructions: u32,
    pub duration_ms: f64,
}

/// The result of running a live automation.
#[derive(Debug, Serialize)]
pub struct RunReport {
    pub ok: bool,
    pub error: Option<String>,
    pub log: Vec<String>,
    pub responses: Vec<String>,
    pub called: usize,
    pub response: Option<WebhookResponse>,
}

/// What a dry run may use.
pub struct TestContext {
    pub data: HashMap<String, String>,
    pub libraries: HashMap<String, String>,
    pub secrets: BTreeMap<String, String>,
    pub settings: Settings,
    /// Whether HTTP requests really go out.
    pub http: bool,
}

impl TestContext {
    /// Loads what a dry run of `automation_id` needs.
    pub fn load(
        conn: &Connection,
        vault: &secrets::Vault,
        automation_id: Option<i64>,
        http: bool,
    ) -> AppResult<Self> {
        let libraries = store::automations(conn)?
            .into_iter()
            .filter(|automation| automation.kind == KIND_LIBRARY)
            .map(|library| (library.name, library.source))
            .collect();
        Ok(Self {
            data: automation_id
                .map(|id| store::automation_values(conn, id))
                .transpose()?
                .unwrap_or_default(),
            libraries,
            secrets: secrets::all(conn, vault)?,
            settings: Settings::load(conn)?,
            http,
        })
    }
}

/// Runs `source` in a fresh sandbox against a simulated event. Posts and
/// reactions are only described and `sideporch.set` changes a copy of the
/// saved data, so nothing in Sideporch changes. HTTP requests are real
/// when the context allows them. Blocks, and needs to be called from a
/// thread that belongs to the Tokio runtime's blocking pool.
pub fn test(name: &str, source: &str, context: TestContext, trigger: &TestTrigger) -> TestReport {
    let http = if context.http {
        http::Http::new(context.settings.allow_private_network)
            .ok()
            .map(Arc::new)
    } else {
        None
    };
    let secrets = Arc::new(context.secrets);
    let environment = sandbox::dry_environment(
        name,
        context.data,
        Arc::new(context.libraries),
        Arc::clone(&secrets),
        http,
        context.settings.zone(),
    );
    let (script, mut outcome) = sandbox::load(&environment, source);
    let mut report = TestReport {
        ok: false,
        error: None,
        log: Vec::new(),
        responses: Vec::new(),
        triggers: Triggers::default(),
        exports: Vec::new(),
        called: 0,
        response: None,
        instructions: 0,
        duration_ms: 0.0,
    };
    if let Some(script) = &script {
        report.triggers = script.triggers();
        report.exports.clone_from(&script.exports);
        if outcome.error.is_none() {
            let (simulated, response, called) = simulate(script, trigger);
            outcome.absorb(simulated);
            report.response = response;
            report.called = called;
        }
    }
    worker::redact(&mut outcome, &secrets);
    report.ok = outcome.error.is_none();
    report.error = outcome.error;
    report.log = outcome.log;
    report.responses = outcome.responses;
    report.instructions = outcome.instructions;
    report.duration_ms = outcome.duration.as_secs_f64() * 1000.0;
    report
}

/// Fires a simulated event at a loaded script. Returns what happened, the
/// webhook response if any, and how many handlers ran.
fn simulate(script: &Script, trigger: &TestTrigger) -> (Outcome, Option<WebhookResponse>, usize) {
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
    let mut outcome = Outcome::default();
    let mut called = 0_usize;
    let mut response = None;
    let mut run_event = |event: &Event, outcome: &mut Outcome| {
        for part in sandbox::dispatch(script, event) {
            called = called.saturating_add(1);
            outcome.absorb(part);
        }
    };
    match trigger {
        TestTrigger::Load => {}
        TestTrigger::Message {
            text,
            channel,
            author,
        } => run_event(
            &Event::Message(message(text, channel, author)),
            &mut outcome,
        ),
        TestTrigger::Reaction {
            emoji,
            added,
            text,
            channel,
        } => run_event(
            &Event::Reaction(ReactionEvent {
                emoji: emoji.trim_matches(':').to_owned(),
                added: *added,
                user: default_author(),
                username: "test".to_owned(),
                message: message(text, channel, "Someone"),
            }),
            &mut outcome,
        ),
        TestTrigger::MemberJoined { user } => run_event(
            &Event::MemberJoined(MemberEvent {
                user: user.clone(),
                username: "test".to_owned(),
            }),
            &mut outcome,
        ),
        TestTrigger::ChannelCreated { channel } => run_event(
            &Event::ChannelCreated(ChannelEvent {
                channel: channel.trim_start_matches('#').to_owned(),
                channel_id: 2,
                user: default_author(),
                username: "test".to_owned(),
            }),
            &mut outcome,
        ),
        TestTrigger::Command { text, channel } => {
            let (part, ran) = simulate_command(script, text, channel);
            called = ran;
            outcome.absorb(part);
        }
        TestTrigger::Timer => {
            for timer in &script.timers {
                if outcome.error.is_some() {
                    break;
                }
                called = called.saturating_add(1);
                outcome.absorb(sandbox::call(&script.lua, &timer.callback, ()));
            }
        }
        TestTrigger::Webhook { method, path, body } => {
            if let Some(handler) = &script.webhook {
                called = 1;
                let request = WebhookRequest {
                    method: method.to_uppercase(),
                    path: path.clone(),
                    query: Vec::new(),
                    headers: vec![("content-type".to_owned(), "application/json".to_owned())],
                    body: body.clone(),
                };
                let (part, answer) = sandbox::call_webhook(&script.lua, handler, &request);
                outcome.absorb(part);
                response = answer;
            }
        }
    }
    (outcome, response, called)
}

/// Runs a typed command, such as `/deploy garden`, against `script`.
fn simulate_command(script: &Script, text: &str, channel: &str) -> (Outcome, usize) {
    let typed = if text.trim_start().starts_with('/') {
        text.to_owned()
    } else {
        format!("/{text}")
    };
    let Some((name, args)) = CommandCall::parse(&typed) else {
        return (
            Outcome {
                error: Some(format!("`{typed}` is not a command")),
                ..Outcome::default()
            },
            0,
        );
    };
    let call = CommandCall {
        name,
        text: args,
        user: default_author(),
        username: "test".to_owned(),
        channel: channel.trim_start_matches('#').to_owned(),
        channel_id: 1,
        thread_id: None,
    };
    sandbox::run_command(script, &call)
        .map_or_else(|| (Outcome::default(), 0), |outcome| (outcome, 1))
}

/// The script a new automation starts with.
pub const EXAMPLE: &str = r#"-- Answers !ping in a thread.
sideporch.on("message", { pattern = "^!ping" }, function(msg)
  sideporch.reply(msg, "pong")
end)

-- Counts how often someone asked, across restarts.
sideporch.command("count", { description = "Count how often you asked" }, function(cmd)
  local count = tonumber(sideporch.get("count") or "0") + 1
  sideporch.set("count", count)
  sideporch.respond(cmd, "I've been asked " .. count .. " times.")
end)

-- Says good morning on weekdays at 9:00.
sideporch.cron("0 9 * * mon-fri", function()
  sideporch.post("general", "Good morning, porch! :sunny:")
end)
"#;

/// The script a new library starts with.
pub const LIBRARY_EXAMPLE: &str = r#"-- Other automations use this library with require("name").
local M = {}

-- Fetches a JSON document and returns it as a table.
function M.fetch(url)
  local response = sideporch.http.get(url)
  if not response.ok then
    error("request failed with status " .. response.status)
  end
  return response.json
end

return M
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn context(data: HashMap<String, String>) -> TestContext {
        TestContext {
            data,
            libraries: HashMap::new(),
            secrets: BTreeMap::from([("TOKEN".to_owned(), "s3cret-value".to_owned())]),
            settings: Settings::default(),
            http: false,
        }
    }

    /// Markdown files under `dir`, recursively.
    fn pages(dir: &std::path::Path, found: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("read the docs") {
            let path = entry.expect("a docs entry").path();
            if path.is_dir() {
                pages(&path, found);
            } else if path.extension().is_some_and(|extension| extension == "md") {
                found.push(path);
            }
        }
    }

    /// The fenced Lua blocks in a page.
    fn lua_blocks(text: &str) -> Vec<String> {
        let mut blocks = Vec::new();
        let mut current: Option<String> = None;
        for line in text.lines() {
            match (&mut current, line.trim()) {
                (None, "```lua") => current = Some(String::new()),
                (Some(block), "```") => {
                    blocks.push(std::mem::take(block));
                    current = None;
                }
                (Some(block), _) => {
                    block.push_str(line);
                    block.push('\n');
                }
                (None, _) => {}
            }
        }
        blocks
    }

    /// Every Lua example in the documentation passes the linter and loads.
    /// A block starting with `-- library: name` is a library the page's
    /// other blocks may `require`.
    #[test]
    fn documentation_examples_lint_and_load() {
        let mut found = Vec::new();
        pages(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("website/content/docs"),
            &mut found,
        );
        let mut checked = 0;
        for page in found {
            let text = std::fs::read_to_string(&page).expect("read a page");
            let blocks = lua_blocks(&text);
            let libraries: HashMap<String, String> = blocks
                .iter()
                .filter_map(|block| {
                    let name = block.lines().next()?.strip_prefix("-- library: ")?;
                    Some((name.trim().to_owned(), block.clone()))
                })
                .collect();
            for block in blocks {
                let problems = tooling::lint(&block);
                assert!(
                    problems.is_empty(),
                    "{}: {problems:#?}\n{block}",
                    page.display()
                );
                let mut context = context(HashMap::new());
                context.libraries.clone_from(&libraries);
                let report = test("Example", &block, context, &TestTrigger::Load);
                assert!(report.ok, "{}: {report:#?}\n{block}", page.display());
                checked += 1;
            }
        }
        assert!(checked >= 20, "only {checked} examples found");
    }

    #[test]
    fn dry_runs_describe_actions_without_saving() {
        let source = r##"sideporch.on_message(function(msg)
  local n = tonumber(sideporch.get("n") or "0") + 1
  sideporch.set("n", n)
  print("seen", n)
  sideporch.reply(msg, "#" .. msg.channel .. " " .. msg.text)
end)"##;
        let report = test(
            "Echo",
            source,
            context(HashMap::from([("n".to_owned(), "4".to_owned())])),
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
    fn dry_runs_report_errors_and_hide_secrets() {
        let report = test(
            "Broken",
            "sideporch.on_reaction(function(e) error('nope: ' .. e.emoji .. ' ' .. sideporch.secret('TOKEN')) end)",
            context(HashMap::new()),
            &TestTrigger::Reaction {
                emoji: ":tada:".to_owned(),
                added: true,
                text: String::new(),
                channel: default_channel(),
            },
        );
        assert!(!report.ok);
        assert_eq!(report.triggers.events.len(), 2);
        assert_eq!(
            report.error.as_deref(),
            Some("line 1: nope: tada [secret TOKEN]")
        );
    }

    #[test]
    fn dry_runs_commands() {
        let report = test(
            "Example",
            EXAMPLE,
            context(HashMap::new()),
            &TestTrigger::Command {
                text: "/count".to_owned(),
                channel: default_channel(),
            },
        );
        assert!(report.ok, "{report:?}");
        assert_eq!(report.responses, ["I've been asked 1 times."]);
        assert_eq!(report.triggers.commands[0].name, "count");
        assert_eq!(report.triggers.schedules.len(), 1);
    }

    #[test]
    fn checks_library_names() {
        assert!(valid_library_name("github") && valid_library_name("home_assistant2"));
        assert!(
            !valid_library_name("GitHub")
                && !valid_library_name("my-lib")
                && !valid_library_name("2x")
        );
    }
}
