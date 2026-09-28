//! One sandboxed Lua state per script, with the `sideporch` API.
//!
//! The same sandbox runs live scripts, dry runs from the editor, and the
//! checks on AI drafts. An [`Environment`] decides where actions go (carried
//! out or only described), where data is kept (the database or a scratch
//! copy), and whether HTTP requests go out.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use jiff::{Timestamp, tz::TimeZone};
use mlua::{
    Function, HookTriggers, IntoLuaMulti, Lua, LuaOptions, LuaSerdeExt as _, StdLib, Table, Value,
    VmState,
};
use rusqlite::Connection;
use serde::Serialize;
use tokio::sync::mpsc as async_mpsc;

use super::{
    cron::Cron,
    events::{
        ChannelEvent, CommandCall, Event, EventKind, MemberEvent, MessageEvent, ReactionEvent,
        WebhookRequest, valid_command_name,
    },
    http::{self, Http},
};
use crate::store;

/// Instructions a single call may run, checked every 1,000 instructions.
const INSTRUCTION_BUDGET: u32 = 2_000_000;
const HOOK_INTERVAL: u32 = 1_000;
const MEMORY_LIMIT: usize = 16 * 1024 * 1024;
/// Messages and reactions a single call may send.
const ACTION_LIMIT: usize = 20;
/// HTTP requests a single call may make.
const HTTP_LIMIT: usize = 10;
const MIN_INTERVAL_SECS: u64 = 10;
const MAX_VALUE_BYTES: usize = 64 * 1024;
const MAX_LOG_LINES: usize = 200;
const MAX_LOG_LINE: usize = 2_000;
const MAX_RESPONSE_BYTES: usize = 256 * 1024;
/// The chunk name, so errors read `script:3: …` and give the line.
const CHUNK: &str = "=script";
const LOADED: &str = "sideporch.loaded";

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

/// Who a script acts as.
#[derive(Debug, Clone)]
pub struct Identity {
    pub id: i64,
    pub name: String,
}

/// Everything a script's state is built from.
#[derive(Clone)]
pub struct Environment {
    pub identity: Identity,
    pub storage: Storage,
    pub sink: Sink,
    /// Library sources by name, for `require`.
    pub libraries: Arc<HashMap<String, String>>,
    pub secrets: Arc<BTreeMap<String, String>>,
    /// `None` switches `sideporch.http` off.
    pub http: Option<Arc<Http>>,
    /// The default time zone for `sideporch.cron`.
    pub timezone: TimeZone,
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

/// Conditions a handler's event must meet.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub channel: Option<String>,
    pub pattern: Option<String>,
    pub emoji: Option<String>,
    pub user: Option<String>,
    pub thread: Option<bool>,
}

impl Filter {
    fn from_table(table: &Table) -> mlua::Result<Self> {
        for pair in table.pairs::<Value, Value>() {
            let (key, _) = pair?;
            let key = key.to_string()?;
            if !matches!(
                key.as_str(),
                "channel" | "pattern" | "emoji" | "user" | "thread"
            ) {
                return Err(mlua::Error::runtime(format!(
                    "sideporch.on: unknown filter `{key}`; use channel, pattern, emoji, user or thread"
                )));
            }
        }
        Ok(Self {
            channel: table
                .get::<Option<String>>("channel")?
                .map(|channel| channel.trim_start_matches('#').to_owned()),
            pattern: table.get("pattern")?,
            emoji: table
                .get::<Option<String>>("emoji")?
                .map(|emoji| emoji.trim_matches(':').to_owned()),
            user: table
                .get::<Option<String>>("user")?
                .map(|user| user.trim_start_matches('@').to_owned()),
            thread: table.get("thread")?,
        })
    }

    fn describe(&self) -> String {
        let mut parts = Vec::new();
        if let Some(channel) = &self.channel {
            parts.push(format!("in #{channel}"));
        }
        if let Some(pattern) = &self.pattern {
            parts.push(format!("matching `{pattern}`"));
        }
        if let Some(emoji) = &self.emoji {
            parts.push(format!("with :{emoji}:"));
        }
        if let Some(user) = &self.user {
            parts.push(format!("by @{user}"));
        }
        match self.thread {
            Some(true) => parts.push("in threads".to_owned()),
            Some(false) => parts.push("outside threads".to_owned()),
            None => {}
        }
        parts.join(" ")
    }

    /// Whether `event` passes. Patterns run as Lua patterns.
    fn matches(&self, lua: &Lua, event: &Event) -> mlua::Result<bool> {
        if let Some(channel) = &self.channel
            && event.channel() != Some(channel.as_str())
        {
            return Ok(false);
        }
        if let Some(emoji) = &self.emoji
            && event.emoji() != Some(emoji.as_str())
        {
            return Ok(false);
        }
        if let Some(user) = &self.user
            && !event
                .username()
                .is_some_and(|name| name.eq_ignore_ascii_case(user))
        {
            return Ok(false);
        }
        if let Some(thread) = self.thread
            && event.in_thread() != Some(thread)
        {
            return Ok(false);
        }
        if let Some(pattern) = &self.pattern {
            let Some(text) = event.text() else {
                return Ok(false);
            };
            let find: Function = lua.globals().get::<Table>("string")?.get("find")?;
            return Ok(!find.call::<Value>((text, pattern.as_str()))?.is_nil());
        }
        Ok(true)
    }
}

pub struct Handler {
    pub kind: EventKind,
    pub filter: Filter,
    pub function: Function,
}

#[derive(Debug, Clone)]
pub enum Schedule {
    Every(Duration),
    Cron(Box<Cron>, TimeZone),
}

impl Schedule {
    /// The first run after `now`.
    pub fn next_after(&self, now: Timestamp) -> Option<Timestamp> {
        match self {
            Self::Every(every) => now
                .checked_add(jiff::SignedDuration::try_from(*every).ok()?)
                .ok(),
            Self::Cron(cron, zone) => cron.next_after(now, zone),
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Self::Every(every) => format!("every {}", human_seconds(every.as_secs())),
            Self::Cron(cron, zone) => format!(
                "cron `{}` ({})",
                cron.expression(),
                zone.iana_name().unwrap_or("UTC")
            ),
        }
    }
}

fn human_seconds(seconds: u64) -> String {
    match seconds {
        s if s % 86_400 == 0 => format!("{} day(s)", s / 86_400),
        s if s % 3_600 == 0 => format!("{} hour(s)", s / 3_600),
        s if s % 60 == 0 => format!("{} minute(s)", s / 60),
        s => format!("{s} seconds"),
    }
}

pub struct Timer {
    pub schedule: Schedule,
    pub next: Option<Timestamp>,
    pub callback: Function,
}

pub struct CommandHandler {
    pub name: String,
    pub description: String,
    pub usage: String,
    pub function: Function,
}

/// What a script listens to, for the automation list, MCP and dispatch.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Triggers {
    pub events: Vec<EventTrigger>,
    pub schedules: Vec<ScheduleTrigger>,
    pub commands: Vec<CommandTrigger>,
    pub webhook: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct EventTrigger {
    pub event: EventKind,
    pub description: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScheduleTrigger {
    pub description: String,
    /// Milliseconds since the Unix epoch.
    pub next_run: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommandTrigger {
    pub name: String,
    pub description: String,
    pub usage: String,
}

impl Triggers {
    pub fn listens_to(&self, kind: EventKind) -> bool {
        self.events.iter().any(|trigger| trigger.event == kind)
    }

    pub const fn is_empty(&self) -> bool {
        self.events.is_empty()
            && self.schedules.is_empty()
            && self.commands.is_empty()
            && !self.webhook
    }
}

/// A loaded script and everything it registered.
pub struct Script {
    pub lua: Lua,
    pub handlers: Vec<Handler>,
    pub webhook: Option<Function>,
    pub timers: Vec<Timer>,
    pub commands: Vec<CommandHandler>,
    /// For a library, the names its module table exports.
    pub exports: Vec<String>,
}

impl Script {
    pub fn triggers(&self) -> Triggers {
        Triggers {
            events: self
                .handlers
                .iter()
                .map(|handler| {
                    let filter = handler.filter.describe();
                    EventTrigger {
                        event: handler.kind,
                        description: if filter.is_empty() {
                            handler.kind.name().to_owned()
                        } else {
                            format!("{} {filter}", handler.kind.name())
                        },
                    }
                })
                .collect(),
            schedules: self
                .timers
                .iter()
                .map(|timer| ScheduleTrigger {
                    description: timer.schedule.describe(),
                    next_run: timer.next.map(Timestamp::as_millisecond),
                })
                .collect(),
            commands: self
                .commands
                .iter()
                .map(|command| CommandTrigger {
                    name: command.name.clone(),
                    description: command.description.clone(),
                    usage: command.usage.clone(),
                })
                .collect(),
            webhook: self.webhook.is_some(),
        }
    }
}

/// What one call did.
#[derive(Debug, Default)]
pub struct Outcome {
    /// Printed lines and descriptions of actions and requests, in order.
    pub log: Vec<String>,
    pub error: Option<String>,
    pub instructions: u32,
    pub duration: Duration,
    /// Private answers from `sideporch.respond`.
    pub responses: Vec<String>,
}

impl Outcome {
    /// Adds a later call's log and result to this one.
    pub fn absorb(&mut self, other: Self) {
        self.log.extend(other.log);
        self.responses.extend(other.responses);
        if self.error.is_none() {
            self.error = other.error;
        }
        self.instructions = self.instructions.saturating_add(other.instructions);
        self.duration = self.duration.saturating_add(other.duration);
    }

    pub fn failed(error: &mlua::Error) -> Self {
        Self {
            error: Some(clean_error(error)),
            ..Self::default()
        }
    }
}

/// What the script registers while its top level runs.
#[derive(Default)]
struct Registry {
    handlers: Vec<Handler>,
    webhook: Option<Function>,
    timers: Vec<Timer>,
    commands: Vec<CommandHandler>,
}

/// Limits and output of the current call.
#[derive(Default)]
struct Call {
    instructions: u32,
    actions: usize,
    requests: usize,
    log: Vec<String>,
    /// `Some` while a command runs, collecting `sideporch.respond`.
    responses: Option<Vec<String>>,
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

/// Libraries being loaded, to catch `require` cycles.
#[derive(Default)]
struct Loading(HashSet<String>);

/// Runs a script's top level, which registers its handlers and timers. For
/// a library, it also records what the returned module exports.
pub fn load(environment: &Environment, source: &str) -> (Option<Script>, Outcome) {
    let lua = match sandbox(environment) {
        Ok(lua) => lua,
        Err(error) => return (None, Outcome::failed(&error)),
    };
    lua.set_app_data(Registry::default());
    let chunk = lua
        .load(source)
        .set_name(CHUNK)
        .set_mode(mlua::prelude::LuaChunkMode::Text);
    let mut returned = Value::Nil;
    let outcome = measure(&lua, false, || {
        returned = chunk.call::<Value>(())?;
        Ok(())
    });
    let registry = lua.remove_app_data::<Registry>().unwrap_or_default();
    if outcome.error.is_some() {
        return (None, outcome);
    }
    let exports = match &returned {
        Value::Table(module) => {
            let mut names: Vec<String> = module
                .pairs::<Value, Value>()
                .filter_map(|pair| pair.ok()?.0.to_string().ok())
                .collect();
            names.sort();
            names
        }
        _ => Vec::new(),
    };
    let now = Timestamp::now();
    let timers = registry
        .timers
        .into_iter()
        .map(|mut timer| {
            timer.next = timer.schedule.next_after(now);
            timer
        })
        .collect();
    let script = Script {
        lua,
        handlers: registry.handlers,
        webhook: registry.webhook,
        timers,
        commands: registry.commands,
        exports,
    };
    (Some(script), outcome)
}

/// Calls a handler with a fresh budget and log.
pub fn call(lua: &Lua, function: &Function, args: impl IntoLuaMulti) -> Outcome {
    measure(lua, false, || function.call::<()>(args))
}

/// Runs the handlers that `event` passes, in registration order.
pub fn dispatch(script: &Script, event: &Event) -> Vec<Outcome> {
    let lua = &script.lua;
    script
        .handlers
        .iter()
        .filter(|handler| handler.kind == event.kind())
        .filter_map(|handler| {
            let mut ran = false;
            let outcome = measure(lua, false, || {
                if !handler.filter.matches(lua, event)? {
                    return Ok(());
                }
                ran = true;
                handler.function.call::<()>(event_table(lua, event)?)
            });
            (ran || outcome.error.is_some()).then_some(outcome)
        })
        .collect()
}

/// Runs a command handler. `sideporch.respond` answers go in the outcome.
pub fn run_command(script: &Script, command: &CommandCall) -> Option<Outcome> {
    let handler = script
        .commands
        .iter()
        .find(|handler| handler.name == command.name)?;
    let lua = &script.lua;
    Some(measure(lua, true, || {
        handler.function.call::<()>(command_table(lua, command)?)
    }))
}

/// Calls a webhook handler and turns its return value into a response.
pub fn call_webhook(
    lua: &Lua,
    function: &Function,
    request: &WebhookRequest,
) -> (Outcome, Option<WebhookResponse>) {
    let mut response = None;
    let outcome = measure(lua, false, || {
        let value = function.call::<Value>(request_table(lua, request)?)?;
        response = Some(response_from(lua, value)?);
        Ok(())
    });
    (outcome, response)
}

fn measure(lua: &Lua, command: bool, run: impl FnOnce() -> mlua::Result<()>) -> Outcome {
    lua.set_app_data(Call {
        responses: command.then(Vec::new),
        ..Call::default()
    });
    let started = Instant::now();
    let result = run();
    let duration = started.elapsed();
    let call = lua.remove_app_data::<Call>().unwrap_or_default();
    Outcome {
        log: call.log,
        error: result.err().map(|error| clean_error(&error)),
        instructions: call.instructions,
        duration,
        responses: call.responses.unwrap_or_default(),
    }
}

/// A readable error: `line 3: attempt to index a nil value`.
pub fn clean_error(error: &mlua::Error) -> String {
    match error {
        mlua::Error::CallbackError { cause, traceback } => {
            let message = clean_error(cause);
            if message.starts_with("line ") || message.contains(".lua:") {
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

fn sandbox(environment: &Environment) -> mlua::Result<Lua> {
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
    for name in ["dofile", "loadfile", "load", "collectgarbage"] {
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
    register_require(&lua, &environment.libraries)?;
    let api = lua.create_table()?;
    register_triggers(&lua, &api, &environment.timezone)?;
    register_actions(&lua, &api, &environment.identity, &environment.sink)?;
    register_storage(
        &lua,
        &api,
        environment.identity.id,
        environment.storage.clone(),
    )?;
    register_json(&lua, &api)?;
    register_http(&lua, &api, environment.http.clone())?;
    let secrets = Arc::clone(&environment.secrets);
    api.set(
        "secret",
        lua.create_function(move |_, name: String| Ok(secrets.get(&name).cloned()))?,
    )?;
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

/// `require(name)` loads a library automation once per script.
fn register_require(lua: &Lua, libraries: &Arc<HashMap<String, String>>) -> mlua::Result<()> {
    lua.set_named_registry_value(LOADED, lua.create_table()?)?;
    lua.set_app_data(Loading::default());
    let libraries = Arc::clone(libraries);
    lua.globals().set(
        "require",
        lua.create_function(move |lua, name: String| {
            let loaded: Table = lua.named_registry_value(LOADED)?;
            let cached = loaded.get::<Value>(name.as_str())?;
            if !cached.is_nil() {
                return Ok(cached);
            }
            let Some(source) = libraries.get(&name) else {
                return Err(mlua::Error::runtime(format!(
                    "require: there is no library named `{name}`"
                )));
            };
            let first = lua
                .app_data_mut::<Loading>()
                .is_some_and(|mut loading| loading.0.insert(name.clone()));
            if !first {
                return Err(mlua::Error::runtime(format!(
                    "require: `{name}` requires itself through other libraries"
                )));
            }
            let result = lua
                .load(source.as_str())
                .set_name(format!("={name}.lua"))
                .set_mode(mlua::prelude::LuaChunkMode::Text)
                .call::<Value>(name.as_str());
            if let Some(mut loading) = lua.app_data_mut::<Loading>() {
                loading.0.remove(&name);
            }
            let module = match result? {
                Value::Nil => Value::Boolean(true),
                other => other,
            };
            loaded.set(name.as_str(), module.clone())?;
            Ok(module)
        })?,
    )
}

/// Registers the triggers a script can use: events, schedules and commands.
fn register_triggers(lua: &Lua, api: &Table, timezone: &TimeZone) -> mlua::Result<()> {
    register_events(lua, api)?;
    register_schedules(lua, api, timezone)?;
    register_commands(lua, api)
}

fn add_handlers(
    lua: &Lua,
    kinds: &[EventKind],
    filter: &Filter,
    function: &Function,
) -> mlua::Result<()> {
    let mut registry = registry(lua)?;
    for kind in kinds {
        registry.handlers.push(Handler {
            kind: *kind,
            filter: filter.clone(),
            function: function.clone(),
        });
    }
    drop(registry);
    Ok(())
}

/// `on`, `on_message`, `on_reaction` and `on_webhook`.
fn register_events(lua: &Lua, api: &Table) -> mlua::Result<()> {
    api.set(
        "on",
        lua.create_function(
            |lua, (name, second, third): (String, Value, Option<Function>)| {
                let kinds = EventKind::parse(&name).ok_or_else(|| {
                    let names: Vec<&str> = EventKind::ALL.iter().map(|kind| kind.name()).collect();
                    mlua::Error::runtime(format!(
                        "sideporch.on: unknown event `{name}`; use {} or reaction",
                        names.join(", ")
                    ))
                })?;
                let (filter, function) = match (second, third) {
                    (Value::Function(function), None) => (Filter::default(), function),
                    (Value::Table(filter), Some(function)) => (Filter::from_table(&filter)?, function),
                    _ => {
                        return Err(mlua::Error::runtime(
                            "sideporch.on: pass the event name, optionally a filter table, and a handler function",
                        ));
                    }
                };
                add_handlers(lua, &kinds, &filter, &function)
            },
        )?,
    )?;
    api.set(
        "on_message",
        lua.create_function(|lua, handler: Function| {
            add_handlers(lua, &[EventKind::Message], &Filter::default(), &handler)
        })?,
    )?;
    api.set(
        "on_reaction",
        lua.create_function(|lua, handler: Function| {
            add_handlers(
                lua,
                &[EventKind::ReactionAdded, EventKind::ReactionRemoved],
                &Filter::default(),
                &handler,
            )
        })?,
    )?;
    api.set(
        "on_webhook",
        lua.create_function(|lua, handler: Function| {
            // A second handler fails the load, so replacing the first is fine.
            if registry(lua)?.webhook.replace(handler).is_some() {
                return Err(mlua::Error::runtime(
                    "sideporch.on_webhook: a script can have only one webhook handler",
                ));
            }
            Ok(())
        })?,
    )
}

/// `every` and `cron`.
fn register_schedules(lua: &Lua, api: &Table, timezone: &TimeZone) -> mlua::Result<()> {
    api.set(
        "every",
        lua.create_function(|lua, (seconds, callback): (u64, Function)| {
            if seconds < MIN_INTERVAL_SECS {
                return Err(mlua::Error::runtime(
                    "sideporch.every: use at least 10 seconds",
                ));
            }
            registry(lua)?.timers.push(Timer {
                schedule: Schedule::Every(Duration::from_secs(seconds)),
                next: None,
                callback,
            });
            Ok(())
        })?,
    )?;
    let default_zone = timezone.clone();
    api.set(
        "cron",
        lua.create_function(
            move |lua, (expression, second, third): (String, Value, Option<Function>)| {
                let (options, callback) = match (second, third) {
                    (Value::Function(callback), None) => (None, callback),
                    (Value::Table(options), Some(callback)) => (Some(options), callback),
                    _ => {
                        return Err(mlua::Error::runtime(
                            "sideporch.cron: pass an expression, optionally { timezone = \"…\" }, and a function",
                        ));
                    }
                };
                let cron = Cron::parse(&expression)
                    .map_err(|error| mlua::Error::runtime(format!("sideporch.cron: {error}")))?;
                let zone = match options
                    .map(|options| options.get::<Option<String>>("timezone"))
                    .transpose()?
                    .flatten()
                {
                    Some(name) => TimeZone::get(&name).map_err(|_| {
                        mlua::Error::runtime(format!(
                            "sideporch.cron: unknown time zone `{name}`; use a name such as Europe/Berlin"
                        ))
                    })?,
                    None => default_zone.clone(),
                };
                registry(lua)?.timers.push(Timer {
                    schedule: Schedule::Cron(Box::new(cron), zone),
                    next: None,
                    callback,
                });
                Ok(())
            },
        )?,
    )
}

/// `command`.
fn register_commands(lua: &Lua, api: &Table) -> mlua::Result<()> {
    api.set(
        "command",
        lua.create_function(
            |lua, (name, second, third): (String, Value, Option<Function>)| {
                let (options, function) = match (second, third) {
                    (Value::Function(function), None) => (None, function),
                    (Value::Table(options), Some(function)) => (Some(options), function),
                    _ => {
                        return Err(mlua::Error::runtime(
                            "sideporch.command: pass a name, optionally { description = …, usage = … }, and a handler",
                        ));
                    }
                };
                let name = name.trim().trim_start_matches('/').to_ascii_lowercase();
                if !valid_command_name(&name) {
                    return Err(mlua::Error::runtime(format!(
                        "sideporch.command: `{name}` is not a command name; use a letter, then letters, digits, - or _"
                    )));
                }
                let text = |key: &str| -> mlua::Result<String> {
                    Ok(options
                        .as_ref()
                        .map(|options| options.get::<Option<String>>(key))
                        .transpose()?
                        .flatten()
                        .unwrap_or_default())
                };
                let (description, usage) = (text("description")?, text("usage")?);
                let mut registry = registry(lua)?;
                if registry.commands.iter().any(|command| command.name == name) {
                    return Err(mlua::Error::runtime(format!(
                        "sideporch.command: /{name} is registered twice"
                    )));
                }
                registry.commands.push(CommandHandler {
                    name,
                    description,
                    usage,
                    function,
                });
                drop(registry);
                Ok(())
            },
        )?,
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

/// `post`, `reply`, `react` and `respond`.
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
    let react_sink = sink.clone();
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
                &react_sink,
                Action::React {
                    automation_id,
                    channel_id: message.get("channel_id")?,
                    message_id: message.get("id")?,
                    emoji,
                },
            )
        })?,
    )?;
    api.set(
        "respond",
        lua.create_function(|lua, (_command, text): (Table, String)| {
            let text = text.trim();
            if text.is_empty() {
                return Err(mlua::Error::runtime("sideporch.respond: the text is empty"));
            }
            log(lua, &format!("→ respond privately: {text}"));
            let mut call = lua
                .app_data_mut::<Call>()
                .ok_or_else(|| mlua::Error::runtime("sideporch.respond: not in a command"))?;
            let responses = call.responses.as_mut().ok_or_else(|| {
                mlua::Error::runtime("sideporch.respond only works in a command handler")
            })?;
            if responses.len() >= ACTION_LIMIT {
                return Err(mlua::Error::runtime(
                    "stopped: too many responses in one run",
                ));
            }
            responses.push(text.chars().take(10_000).collect());
            drop(call);
            Ok(())
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

/// Turns Lua request options into a request.
fn http_request(lua: &Lua, options: &Table) -> mlua::Result<http::Request> {
    let url: String = options
        .get::<Option<String>>("url")?
        .ok_or_else(|| mlua::Error::runtime("sideporch.http: the request needs a url"))?;
    let method = options
        .get::<Option<String>>("method")?
        .unwrap_or_else(|| "GET".to_owned());
    let mut headers = Vec::new();
    if let Some(table) = options.get::<Option<Table>>("headers")? {
        for pair in table.pairs::<String, String>() {
            headers.push(pair?);
        }
    }
    let json = options.get::<Value>("json")?;
    let body = if json.is_nil() {
        options
            .get::<Option<mlua::LuaString>>("body")?
            .map(|body| body.as_bytes().to_vec())
            .unwrap_or_default()
    } else {
        let value: serde_json::Value = lua.from_value(json)?;
        if !headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        {
            headers.push(("Content-Type".to_owned(), "application/json".to_owned()));
        }
        value.to_string().into_bytes()
    };
    let timeout = options
        .get::<Option<f64>>("timeout")?
        .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
        .map_or(http::DEFAULT_TIMEOUT, |seconds| {
            Duration::from_secs_f64(seconds.min(http::MAX_TIMEOUT.as_secs_f64()))
        });
    Ok(http::Request {
        method,
        url,
        headers,
        body,
        timeout,
    })
}

fn http_response(lua: &Lua, response: &http::Response) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set("status", response.status)?;
    table.set("ok", (200..300).contains(&response.status))?;
    table.set("body", response.body.as_str())?;
    let headers = lua.create_table()?;
    let mut is_json = false;
    for (name, value) in &response.headers {
        if name == "content-type" && value.contains("json") {
            is_json = true;
        }
        headers.set(name.as_str(), value.as_str())?;
    }
    table.set("headers", headers)?;
    if is_json && let Ok(value) = serde_json::from_str::<serde_json::Value>(&response.body) {
        table.set("json", lua.to_value(&value)?)?;
    }
    Ok(table)
}

/// `sideporch.http.request`, `get` and `post`.
fn register_http(lua: &Lua, api: &Table, client: Option<Arc<Http>>) -> mlua::Result<()> {
    let send = move |lua: &Lua, options: &Table| -> mlua::Result<Table> {
        let Some(client) = &client else {
            return Err(mlua::Error::runtime(
                "sideporch.http: requests are switched off in this run",
            ));
        };
        let requests = {
            let mut call = lua
                .app_data_mut::<Call>()
                .ok_or_else(|| mlua::Error::runtime("sideporch.http: not allowed here"))?;
            call.requests = call.requests.saturating_add(1);
            call.requests
        };
        if requests > HTTP_LIMIT {
            return Err(mlua::Error::runtime(
                "stopped: more than 10 HTTP requests in one run",
            ));
        }
        let request = http_request(lua, options)?;
        let shown = format!(
            "{} {}",
            request.method.to_uppercase(),
            request.url.split('?').next().unwrap_or_default()
        );
        match client.send(request) {
            Ok(response) => {
                log(
                    lua,
                    &format!(
                        "→ {shown} {} ({} ms)",
                        response.status,
                        response.elapsed.as_millis()
                    ),
                );
                http_response(lua, &response)
            }
            Err(error) => {
                log(lua, &format!("→ {shown} failed: {error}"));
                Err(mlua::Error::runtime(format!("sideporch.http: {error}")))
            }
        }
    };
    let send = Arc::new(send);
    let table = lua.create_table()?;
    let request = Arc::clone(&send);
    table.set(
        "request",
        lua.create_function(move |lua, options: Table| request(lua, &options))?,
    )?;
    let get = Arc::clone(&send);
    table.set(
        "get",
        lua.create_function(move |lua, (url, options): (String, Option<Table>)| {
            let options = match options {
                Some(options) => options,
                None => lua.create_table()?,
            };
            options.set("url", url)?;
            options.set("method", "GET")?;
            get(lua, &options)
        })?,
    )?;
    let post = send;
    table.set(
        "post",
        lua.create_function(
            move |lua, (url, body, options): (String, Value, Option<Table>)| {
                let options = match options {
                    Some(options) => options,
                    None => lua.create_table()?,
                };
                options.set("url", url)?;
                options.set("method", "POST")?;
                match body {
                    Value::Table(table) => options.set("json", table)?,
                    Value::Nil => {}
                    other => options.set("body", other)?,
                }
                post(lua, &options)
            },
        )?,
    )?;
    api.set("http", table)
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

fn reaction_table(lua: &Lua, event: &ReactionEvent) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set("emoji", event.emoji.as_str())?;
    table.set("added", event.added)?;
    table.set("user", event.user.as_str())?;
    table.set("username", event.username.as_str())?;
    table.set("message", message_table(lua, &event.message)?)?;
    Ok(table)
}

fn member_table(lua: &Lua, event: &MemberEvent) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set("user", event.user.as_str())?;
    table.set("username", event.username.as_str())?;
    Ok(table)
}

fn channel_table(lua: &Lua, event: &ChannelEvent) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set("channel", event.channel.as_str())?;
    table.set("channel_id", event.channel_id)?;
    table.set("user", event.user.as_str())?;
    table.set("username", event.username.as_str())?;
    Ok(table)
}

fn event_table(lua: &Lua, event: &Event) -> mlua::Result<Table> {
    let table = match event {
        Event::Message(message)
        | Event::MessageChanged(message)
        | Event::MessageDeleted(message) => message_table(lua, message)?,
        Event::Reaction(reaction) => reaction_table(lua, reaction)?,
        Event::MemberJoined(member) => member_table(lua, member)?,
        Event::ChannelCreated(channel) => channel_table(lua, channel)?,
    };
    table.set("event", event.kind().name())?;
    Ok(table)
}

fn command_table(lua: &Lua, command: &CommandCall) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set("name", command.name.as_str())?;
    table.set("text", command.text.as_str())?;
    let args = lua.create_sequence_from(command.text.split_whitespace())?;
    table.set("args", args)?;
    table.set("user", command.user.as_str())?;
    table.set("username", command.username.as_str())?;
    table.set("channel", command.channel.as_str())?;
    table.set("channel_id", command.channel_id)?;
    table.set("thread_id", command.thread_id)?;
    // So `sideporch.reply(cmd, …)` answers where the command was typed.
    table.set("id", command.thread_id)?;
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

/// A dry environment: actions are collected, data is a scratch copy.
pub fn dry_environment(
    name: &str,
    data: HashMap<String, String>,
    libraries: Arc<HashMap<String, String>>,
    secrets: Arc<BTreeMap<String, String>>,
    http: Option<Arc<Http>>,
    timezone: TimeZone,
) -> Environment {
    Environment {
        identity: Identity {
            id: 0,
            name: name.to_owned(),
        },
        storage: Storage::Dry(Arc::new(Mutex::new(data))),
        sink: Sink::Dry(Arc::default()),
        libraries,
        secrets,
        http,
        timezone,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn environment(libraries: &[(&str, &str)]) -> (Environment, Arc<Mutex<Vec<Action>>>) {
        let actions = Arc::new(Mutex::new(Vec::new()));
        let environment = Environment {
            identity: Identity {
                id: 1,
                name: "Test".to_owned(),
            },
            storage: Storage::Dry(Arc::default()),
            sink: Sink::Dry(Arc::clone(&actions)),
            libraries: Arc::new(
                libraries
                    .iter()
                    .map(|(name, source)| ((*name).to_owned(), (*source).to_owned()))
                    .collect(),
            ),
            secrets: Arc::new(BTreeMap::from([("TOKEN".to_owned(), "s3cret".to_owned())])),
            http: None,
            timezone: TimeZone::UTC,
        };
        (environment, actions)
    }

    fn dry(source: &str) -> (Option<Script>, Outcome, Arc<Mutex<Vec<Action>>>) {
        let (environment, actions) = environment(&[]);
        let (script, outcome) = load(&environment, source);
        (script, outcome, actions)
    }

    fn message(text: &str, channel: &str) -> Event {
        Event::Message(MessageEvent {
            id: 7,
            channel_id: 1,
            channel: channel.to_owned(),
            text: text.to_owned(),
            author: "Ada".to_owned(),
            username: Some("ada".to_owned()),
            is_bot: false,
            thread_id: None,
        })
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
        assert_eq!(script.unwrap().handlers.len(), 1);
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
    fn filters_events() {
        let (script, _, _) = dry(
            r##"sideporch.on("message", { channel = "#alerts", pattern = "^!deploy (%w+)" }, function(msg)
  print("deploy", msg.text:match("^!deploy (%w+)"))
end)
sideporch.on("message", function(msg) print("any", msg.event) end)"##,
        );
        let script = script.unwrap();
        let outcomes = dispatch(&script, &message("!deploy garden", "alerts"));
        let logs: Vec<&str> = outcomes
            .iter()
            .flat_map(|o| o.log.iter().map(String::as_str))
            .collect();
        assert_eq!(logs, ["deploy\tgarden", "any\tmessage"]);
        let outcomes = dispatch(&script, &message("!deploy garden", "general"));
        assert_eq!(outcomes.len(), 1, "only the unfiltered handler runs");
        assert_eq!(
            script.triggers().events[0].description,
            "message in #alerts matching `^!deploy (%w+)`"
        );
    }

    #[test]
    fn rejects_unknown_events_and_filters() {
        let (_, outcome, _) = dry("sideporch.on('reactions', function() end)");
        assert!(outcome.error.unwrap().contains("unknown event `reactions`"));
        let (_, outcome, _) = dry("sideporch.on('message', { chanel = 'x' }, function() end)");
        assert!(outcome.error.unwrap().contains("unknown filter `chanel`"));
    }

    #[test]
    fn schedules_cron_jobs() {
        let (script, outcome, _) = dry(
            r#"sideporch.cron("0 9 * * mon-fri", { timezone = "Europe/Berlin" }, function() end)
sideporch.every(3600, function() end)"#,
        );
        assert_eq!(outcome.error, None);
        let triggers = script.unwrap().triggers();
        assert_eq!(
            triggers.schedules[0].description,
            "cron `0 9 * * mon-fri` (Europe/Berlin)"
        );
        assert_eq!(triggers.schedules[1].description, "every 1 hour(s)");
        assert!(
            triggers
                .schedules
                .iter()
                .all(|schedule| schedule.next_run.is_some())
        );
        let (_, outcome, _) = dry(r#"sideporch.cron("61 * * * *", function() end)"#);
        assert!(outcome.error.unwrap().contains("outside 0-59"));
        let (_, outcome, _) =
            dry(r#"sideporch.cron("@daily", { timezone = "Mars/Base" }, function() end)"#);
        assert!(outcome.error.unwrap().contains("unknown time zone"));
    }

    #[test]
    fn runs_commands_with_private_answers() {
        let (script, _, _) = dry(
            r#"sideporch.command("deploy", { description = "Deploy a service", usage = "<service>" }, function(cmd)
  sideporch.respond(cmd, "Deploying " .. (cmd.args[1] or "nothing") .. " for " .. cmd.user)
end)"#,
        );
        let script = script.unwrap();
        assert_eq!(script.triggers().commands[0].usage, "<service>");
        let outcome = run_command(
            &script,
            &CommandCall {
                name: "deploy".to_owned(),
                text: "garden now".to_owned(),
                user: "Ada".to_owned(),
                ..CommandCall::default()
            },
        )
        .unwrap();
        assert_eq!(outcome.responses, ["Deploying garden for Ada"]);
        let (_, outcome, _) = dry("sideporch.respond({}, 'x')");
        assert!(outcome.error.unwrap().contains("only works in a command"));
    }

    #[test]
    fn requires_libraries_once() {
        let (environment, _) = environment(&[
            (
                "greet",
                "print('loading greet')\nlocal M = {}\nfunction M.hello(name) return 'Hello, ' .. name end\nreturn M",
            ),
            ("loop_a", "return require('loop_b')"),
            ("loop_b", "return require('loop_a')"),
        ]);
        let (script, outcome) = load(
            &environment,
            "local greet = require('greet')\nlocal again = require('greet')\nprint(greet.hello('porch'), greet == again)",
        );
        assert!(script.is_some(), "{outcome:?}");
        assert_eq!(outcome.log, ["loading greet", "Hello, porch\ttrue"]);
        let (_, outcome) = load(&environment, "require('missing')");
        assert!(
            outcome
                .error
                .unwrap()
                .contains("no library named `missing`")
        );
        let (_, outcome) = load(&environment, "require('loop_a')");
        assert!(outcome.error.unwrap().contains("requires itself"));
        let (library, _) = load(
            &environment,
            "local M = {}\nfunction M.a() end\nM.b = 1\nreturn M",
        );
        assert_eq!(library.unwrap().exports, ["a", "b"]);
    }

    #[test]
    fn reads_secrets_and_refuses_http_when_off() {
        let (_, outcome, _) = dry("print(sideporch.secret('TOKEN'), sideporch.secret('NOPE'))");
        assert_eq!(outcome.log, ["s3cret\tnil"]);
        let (_, outcome, _) = dry("sideporch.http.get('https://example.com')");
        assert!(outcome.error.unwrap().contains("switched off"));
    }

    #[test]
    fn answers_webhooks() {
        let (script, _, _) = dry(r#"sideporch.on_webhook(function(req)
  if req.path == "/json" then return { json = { got = req.json.n } } end
  if req.path == "/empty" then return nil end
  return { status = 202, body = req.method .. " " .. req.query.q }
end)"#);
        let script = script.unwrap();
        let handler = script.webhook.as_ref().unwrap();
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
