//! A [Model Context Protocol](https://modelcontextprotocol.io) server, so AI
//! agents such as Claude Code can build, test and run automations.
//!
//! It speaks the Streamable HTTP transport without sessions: each POST to
//! `/mcp` carries one JSON-RPC message and gets one JSON answer. Clients
//! authenticate with an API token that an admin creates under Settings;
//! tokens act as that admin and stop working if they lose admin rights.
//!
//! Besides tools, the server offers the API reference and every script as
//! resources, and a prompt for writing an automation.

use std::sync::Arc;

use axum::{
    Json,
    body::Bytes,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    AppState,
    auth::hash_token,
    automations::{self, KIND_AUTOMATION, KIND_LIBRARY, TestTrigger, api, cron::Cron, tooling},
    error::{AppError, AppResult},
    now_ms,
    routes::{Change, apply_change, base_url, restore_version, run_test},
    secrets, store,
};

/// An RFC 6570 URI template, not a format string.
#[allow(clippy::literal_string_with_formatting_args)]
const AUTOMATION_URI_TEMPLATE: &str = "sideporch://automations/{id}";
const PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26"];
const MAX_SOURCE_BYTES: usize = 100_000;

/// Who is calling: the token's admin and the token's name.
struct Caller {
    user_id: i64,
    token_name: String,
}

pub async fn endpoint(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let caller = match authenticate(&state, &headers).await {
        Ok(Some(caller)) => caller,
        Ok(None) => {
            return (
                StatusCode::UNAUTHORIZED,
                [(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"))],
                "Send an API token as `Authorization: Bearer <token>`. Admins create tokens under Automations → MCP.",
            )
                .into_response();
        }
        Err(error) => return error.into_response(),
    };
    let Ok(message) = serde_json::from_slice::<Value>(&body) else {
        return Json(rpc_error(&Value::Null, -32700, "Parse error")).into_response();
    };
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return Json(rpc_error(
            message.get("id").unwrap_or(&Value::Null),
            -32600,
            "Send one JSON-RPC request per POST",
        ))
        .into_response();
    };
    let Some(id) = message.get("id").cloned() else {
        // Notifications, such as notifications/initialized, need no answer.
        return StatusCode::ACCEPTED.into_response();
    };
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let context = Context {
        state: &state,
        caller: &caller,
        hooks: format!("{}/hooks/automations/", base_url(&state, &headers)),
    };
    let answer = match method {
        "initialize" => Ok(initialize(&params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools() })),
        "tools/call" => match call_tool(&context, &params).await {
            Ok(result) => Ok(result),
            Err(ToolError::Unknown(name)) => Err((-32602, format!("Unknown tool: {name}"))),
            Err(ToolError::Failed(message)) => {
                Ok(json!({ "content": [{ "type": "text", "text": message }], "isError": true }))
            }
        },
        "resources/list" => resources(&context).await,
        "resources/templates/list" => Ok(json!({ "resourceTemplates": [{
            "uriTemplate": AUTOMATION_URI_TEMPLATE,
            "name": "automation",
            "title": "Automation script",
            "description": "The Lua source of an automation or library",
            "mimeType": "text/x-lua",
        }] })),
        "resources/read" => read_resource(&context, &params).await,
        "prompts/list" => Ok(json!({ "prompts": [{
            "name": "write_automation",
            "title": "Write an automation",
            "description": "Plan, write, test and save a Sideporch automation for a task.",
            "arguments": [{ "name": "task", "description": "What the automation should do", "required": true }],
        }] })),
        "prompts/get" => prompt(&params),
        _ => Err((-32601, format!("Method not found: {method}"))),
    };
    Json(match answer {
        Ok(result) => rpc_result(&id, &result),
        Err((code, message)) => rpc_error(&id, code, &message),
    })
    .into_response()
}

async fn authenticate(state: &AppState, headers: &HeaderMap) -> AppResult<Option<Caller>> {
    let Some(token) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    else {
        return Ok(None);
    };
    let token_hash = hash_token(token.trim());
    let now = now_ms();
    let found = state
        .db
        .call(move |conn| store::use_api_token(conn, &token_hash, now))
        .await?;
    Ok(found.map(|(user_id, token_name)| Caller {
        user_id,
        token_name,
    }))
}

fn rpc_result(id: &Value, result: &Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn rpc_error(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

const INSTRUCTIONS: &str = "This server builds and runs automations in a Sideporch team chat: Lua scripts that react to \
messages, reactions, new members and channels, answer slash commands and webhooks, run on cron schedules, call HTTP APIs, \
and read encrypted secrets. Libraries (kind \"library\") hold shared code that automations load with require(\"name\").\n\n\
Work like a developer: read get_api_reference first; check list_channels, list_automations and list_secrets for context; \
write the script, then lint_lua and test_automation it (tests change nothing in Sideporch) before save_automation. \
New automations start switched off; ask the person before passing enabled: true. Use list_runs to see what a live \
automation did, and run_automation to fire it for real. Never put secret values in scripts: store them with set_secret \
and read them with sideporch.secret(\"NAME\").";

fn initialize(params: &Value) -> Value {
    let requested = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let version = PROTOCOL_VERSIONS
        .iter()
        .find(|version| **version == requested)
        .or(PROTOCOL_VERSIONS.first())
        .copied()
        .unwrap_or_default();
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": {}, "resources": {}, "prompts": {} },
        "serverInfo": { "name": "sideporch", "title": "Sideporch", "version": env!("CARGO_PKG_VERSION") },
        "instructions": format!("{INSTRUCTIONS}\n\n{}", api::reference()),
    })
}

/// How a tool affects Sideporch, for clients that ask before changes.
#[derive(Clone, Copy)]
enum Effect {
    Reads,
    Changes,
    Deletes,
    /// Changes things and reaches the outside world.
    Acts,
}

fn tool(name: &str, title: &str, description: &str, schema: &Value, effect: Effect) -> Value {
    json!({
        "name": name,
        "title": title,
        "description": description,
        "inputSchema": schema,
        "annotations": {
            "readOnlyHint": matches!(effect, Effect::Reads),
            "destructiveHint": matches!(effect, Effect::Deletes),
            "openWorldHint": matches!(effect, Effect::Acts),
        },
    })
}

fn object(properties: &Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": properties, "required": required, "additionalProperties": false })
}

fn trigger_schema() -> Value {
    json!({
        "type": "object",
        "description": "The event to simulate: kind is load, message, reaction, command, webhook, timer (fires every schedule), member_joined or channel_created.",
        "properties": {
            "kind": { "type": "string", "enum": ["load", "message", "reaction", "command", "webhook", "timer", "member_joined", "channel_created"] },
            "text": { "type": "string", "description": "Message text (message, reaction), or the typed command such as `/deploy garden` (command)" },
            "channel": { "type": "string", "description": "Channel name (message, reaction, command, channel_created); default general" },
            "author": { "type": "string", "description": "Author display name (message)" },
            "emoji": { "type": "string", "description": "Emoji name (reaction); default thumbsup" },
            "added": { "type": "boolean", "description": "Reaction added (true) or removed (reaction)" },
            "user": { "type": "string", "description": "Display name of the new member (member_joined)" },
            "method": { "type": "string", "description": "HTTP method (webhook); default POST" },
            "path": { "type": "string", "description": "Path after the webhook token, such as /deploy (webhook)" },
            "body": { "type": "string", "description": "Request body (webhook)" }
        },
        "required": ["kind"]
    })
}

/// Schemas several tools share.
struct Schemas {
    source: Value,
    id: Value,
    version: Value,
    nothing: Value,
    by_id: Value,
    by_source: Value,
}

fn tools() -> Vec<Value> {
    let source = json!({ "type": "string", "description": "The Lua script" });
    let id = json!({ "type": "integer", "description": "The automation's or library's id" });
    let schemas = Schemas {
        by_id: object(&json!({ "id": id }), &["id"]),
        by_source: object(&json!({ "source": source }), &["source"]),
        nothing: object(&json!({}), &[]),
        version: json!({ "type": "integer", "description": "A version id from list_versions" }),
        source,
        id,
    };
    let mut all = context_tools(&schemas);
    all.extend(automation_tools(&schemas));
    all.extend(dev_tools(&schemas));
    all.extend(ops_tools(&schemas));
    all
}

/// Reference and chat context.
fn context_tools(schemas: &Schemas) -> Vec<Value> {
    vec![
        tool(
            "get_api_reference",
            "Automation API reference",
            "The complete Lua API: triggers, filters, actions, HTTP, secrets, storage, JSON, limits and every event table's fields.",
            &schemas.nothing,
            Effect::Reads,
        ),
        tool(
            "list_channels",
            "List public channels",
            "Names of the public channels scripts can post to and hear from.",
            &schemas.nothing,
            Effect::Reads,
        ),
        tool(
            "read_messages",
            "Read a channel",
            "The newest messages of a public channel, oldest first, to see what an automation will react to.",
            &object(
                &json!({ "channel": { "type": "string" }, "limit": { "type": "integer", "minimum": 1, "maximum": 50 } }),
                &["channel"],
            ),
            Effect::Reads,
        ),
        tool(
            "list_commands",
            "List slash commands",
            "Slash commands registered by running automations, with their owners.",
            &schemas.nothing,
            Effect::Reads,
        ),
        tool(
            "explain_cron",
            "Preview a cron schedule",
            "Checks a cron expression and lists its next run times.",
            &object(
                &json!({
                    "expression": { "type": "string", "description": "Such as `0 9 * * mon-fri` or `@hourly`" },
                    "timezone": { "type": "string", "description": "IANA name; default the instance's automation time zone" },
                    "count": { "type": "integer", "minimum": 1, "maximum": 20 }
                }),
                &["expression"],
            ),
            Effect::Reads,
        ),
    ]
}

/// Automations, libraries and their history.
fn automation_tools(schemas: &Schemas) -> Vec<Value> {
    vec![
        tool(
            "list_automations",
            "List automations and libraries",
            "Every automation and library: id, kind, name, status, last error, webhook URL, and what each running automation listens to, including the next scheduled runs.",
            &schemas.nothing,
            Effect::Reads,
        ),
        tool(
            "get_automation",
            "Get an automation",
            "One automation's or library's script, status, triggers, webhook URL and its most recent runs.",
            &schemas.by_id,
            Effect::Reads,
        ),
        tool(
            "save_automation",
            "Create or update an automation or library",
            "Creates (without id) or updates (with id) a script. Refuses scripts with lint errors. kind is `automation` (default) or `library` and only applies when creating; library names are Lua module names such as github_api. New automations start switched off unless enabled is true; updates keep the switch unless enabled is given. Every save is kept in the history and restarts the automations.",
            &object(
                &json!({
                    "id": schemas.id,
                    "name": { "type": "string", "description": "Shown as the author of its posts; up to 80 characters" },
                    "source": schemas.source,
                    "kind": { "type": "string", "enum": [KIND_AUTOMATION, KIND_LIBRARY] },
                    "enabled": { "type": "boolean", "description": "Whether the automation runs" }
                }),
                &["name", "source"],
            ),
            Effect::Changes,
        ),
        tool(
            "set_enabled",
            "Switch an automation on or off",
            "Starts or stops an automation without changing its script.",
            &object(
                &json!({ "id": schemas.id, "enabled": { "type": "boolean" } }),
                &["id", "enabled"],
            ),
            Effect::Changes,
        ),
        tool(
            "delete_automation",
            "Delete an automation",
            "Deletes an automation or library with its history, runs and saved data. This cannot be undone.",
            &schemas.by_id,
            Effect::Deletes,
        ),
        tool(
            "list_versions",
            "List saved versions",
            "Every saved version of a script, newest first, with who saved it and how.",
            &schemas.by_id,
            Effect::Reads,
        ),
        tool(
            "get_version",
            "Read a saved version",
            "The schemas.source of one saved version.",
            &object(
                &json!({ "id": schemas.id, "version_id": schemas.version }),
                &["id", "version_id"],
            ),
            Effect::Reads,
        ),
        tool(
            "restore_version",
            "Restore a saved version",
            "Saves an earlier version as the newest one and restarts the automations.",
            &object(
                &json!({ "id": schemas.id, "version_id": schemas.version }),
                &["id", "version_id"],
            ),
            Effect::Changes,
        ),
        tool(
            "rotate_webhook_url",
            "Make a new webhook URL",
            "Replaces an automation's webhook URL; the old one stops working.",
            &schemas.by_id,
            Effect::Changes,
        ),
    ]
}

/// Writing, testing and running scripts.
fn dev_tools(schemas: &Schemas) -> Vec<Value> {
    vec![
        tool(
            "lint_lua",
            "Lint a script",
            "Syntax errors and lint findings with line and column. Knows the sandbox and the sideporch API.",
            &schemas.by_source,
            Effect::Reads,
        ),
        tool(
            "format_lua",
            "Format a script",
            "The script formatted in Sideporch's style (StyLua, two-space indentation).",
            &schemas.by_source,
            Effect::Reads,
        ),
        tool(
            "test_automation",
            "Test a script",
            "Runs a script in a fresh sandbox against a simulated event and returns what it printed, the posts, reactions and private answers it would make, what it listens to, and any error. Nothing in Sideporch changes. HTTP requests are only made when http is true, because they reach the outside world. Pass schemas.source, or id to test a saved script with its saved data.",
            &object(
                &json!({ "source": schemas.source, "id": schemas.id, "trigger": trigger_schema(), "http": { "type": "boolean", "description": "Make real HTTP requests; default false" } }),
                &["trigger"],
            ),
            Effect::Reads,
        ),
        tool(
            "run_automation",
            "Run an automation now",
            "Fires a running automation for real with a simulated event, such as timer to run its schedules now. Posts, reactions and HTTP requests happen.",
            &object(
                &json!({ "id": schemas.id, "trigger": trigger_schema() }),
                &["id", "trigger"],
            ),
            Effect::Acts,
        ),
        tool(
            "list_runs",
            "Recent runs",
            "An automation's recent runs that printed, acted, answered or failed, newest first, with secrets hidden.",
            &object(
                &json!({ "id": schemas.id, "limit": { "type": "integer", "minimum": 1, "maximum": 100 } }),
                &["id"],
            ),
            Effect::Reads,
        ),
    ]
}

/// Saved data, secrets and settings.
fn ops_tools(schemas: &Schemas) -> Vec<Value> {
    vec![
        tool(
            "list_data",
            "Read saved data",
            "The keys and values an automation saved with sideporch.set.",
            &schemas.by_id,
            Effect::Reads,
        ),
        tool(
            "set_data",
            "Change saved data",
            "Sets a key an automation reads with sideporch.get, or deletes it when value is null.",
            &object(
                &json!({ "id": schemas.id, "key": { "type": "string" }, "value": { "type": ["string", "null"] } }),
                &["id", "key", "value"],
            ),
            Effect::Changes,
        ),
        tool(
            "list_secrets",
            "List secrets",
            "Names of the secrets scripts can read with sideporch.secret, and where they come from. Values are never shown.",
            &schemas.nothing,
            Effect::Reads,
        ),
        tool(
            "set_secret",
            "Store a secret",
            "Stores or replaces an encrypted secret, such as an API token, and restarts the automations. Names use letters, digits and underscores.",
            &object(
                &json!({ "name": { "type": "string" }, "value": { "type": "string" } }),
                &["name", "value"],
            ),
            Effect::Changes,
        ),
        tool(
            "delete_secret",
            "Delete a secret",
            "Deletes a stored secret. Secrets from environment variables cannot be deleted here.",
            &object(&json!({ "name": { "type": "string" } }), &["name"]),
            Effect::Deletes,
        ),
        tool(
            "get_settings",
            "Read automation settings",
            "The default time zone for schedules, whether HTTP may reach private networks, and the server's version and URL.",
            &schemas.nothing,
            Effect::Reads,
        ),
        tool(
            "update_settings",
            "Change automation settings",
            "Sets the default schedule time zone and whether HTTP may reach private networks, then restarts the automations.",
            &object(
                &json!({ "timezone": { "type": "string" }, "allow_private_network": { "type": "boolean" } }),
                &[],
            ),
            Effect::Changes,
        ),
    ]
}

enum ToolError {
    Unknown(String),
    Failed(String),
}

impl From<AppError> for ToolError {
    fn from(error: AppError) -> Self {
        Self::Failed(match error {
            AppError::NotFound => "Not found.".to_owned(),
            other => other.to_string(),
        })
    }
}

fn failed(message: impl Into<String>) -> ToolError {
    ToolError::Failed(message.into())
}

type ToolResult = Result<Value, ToolError>;

fn arguments<T: for<'de> Deserialize<'de>>(params: &Value) -> Result<T, ToolError> {
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    serde_json::from_value(arguments).map_err(|error| failed(format!("Invalid arguments: {error}")))
}

fn text(value: &Value) -> Value {
    let text = match value {
        Value::String(text) => text.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_default(),
    };
    if value.is_object() {
        json!({ "content": [{ "type": "text", "text": text }], "structuredContent": value })
    } else {
        json!({ "content": [{ "type": "text", "text": text }] })
    }
}

fn check_size(source: &str) -> Result<(), ToolError> {
    if source.len() > MAX_SOURCE_BYTES {
        return Err(failed("The script is larger than 100 kB."));
    }
    Ok(())
}

struct Context<'a> {
    state: &'a AppState,
    caller: &'a Caller,
    /// The base of webhook URLs.
    hooks: String,
}

async fn call_tool(context: &Context<'_>, params: &Value) -> ToolResult {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match name {
        "get_api_reference" => Ok(text(&Value::String(api::reference()))),
        "list_channels" | "read_messages" | "list_commands" | "explain_cron" | "get_settings" => {
            context_tool(context, name, params).await
        }
        "list_automations" | "get_automation" | "list_versions" | "get_version" | "list_runs"
        | "list_data" | "list_secrets" => read_tool(context, name, params).await,
        "save_automation" | "set_enabled" | "delete_automation" | "restore_version"
        | "rotate_webhook_url" | "set_data" | "set_secret" | "delete_secret"
        | "update_settings" => write_tool(context, name, params).await,
        "lint_lua" | "format_lua" | "test_automation" | "run_automation" => {
            dev_tool(context, name, params).await
        }
        other => Err(ToolError::Unknown(other.to_owned())),
    }
}

#[derive(Deserialize)]
struct IdArgs {
    id: i64,
}

#[derive(Deserialize)]
struct VersionArgs {
    id: i64,
    version_id: i64,
}

#[derive(Deserialize)]
struct SourceArgs {
    source: String,
}

#[derive(Deserialize)]
struct ReadArgs {
    channel: String,
    limit: Option<u32>,
}

#[derive(Deserialize)]
struct CronArgs {
    expression: String,
    timezone: Option<String>,
    count: Option<usize>,
}

#[derive(Deserialize)]
struct RunsArgs {
    id: i64,
    limit: Option<i64>,
}

async fn context_tool(context: &Context<'_>, name: &str, params: &Value) -> ToolResult {
    let state = context.state;
    match name {
        "list_channels" => {
            let channels = state
                .db
                .call(|conn| store::public_channel_names(conn))
                .await?;
            Ok(text(&json!({ "channels": channels })))
        }
        "read_messages" => {
            let ReadArgs { channel, limit } = arguments(params)?;
            let limit = limit.unwrap_or(20).clamp(1, 50);
            let name = channel.trim_start_matches('#').to_owned();
            let messages = state
                .db
                .call(move |conn| {
                    let id = store::public_channel_id(conn, &name)?.ok_or_else(|| {
                        AppError::bad_request("No public channel with that name.")
                    })?;
                    store::channel_messages(conn, id, None, limit)
                })
                .await?;
            let list: Vec<Value> = messages
                .iter()
                .map(|message| {
                    let author = match &message.author {
                        store::Author::User { display_name, .. } => display_name.clone(),
                        store::Author::Bot { name, .. } => name.clone(),
                        store::Author::Removed => "Former member".to_owned(),
                    };
                    json!({
                        "id": message.id,
                        "author": author,
                        "text": message.body,
                        "replies": message.reply_count,
                        "at": timestamp(message.created_at),
                    })
                })
                .collect();
            Ok(text(&json!({ "messages": list })))
        }
        "list_commands" => {
            let commands = state.automations.commands();
            Ok(text(&json!({ "commands": commands })))
        }
        "explain_cron" => {
            let CronArgs {
                expression,
                timezone,
                count,
            } = arguments(params)?;
            let settings = state
                .db
                .call(|conn| automations::Settings::load(conn))
                .await?;
            let zone_name = timezone.unwrap_or(settings.timezone);
            let zone = jiff::tz::TimeZone::get(&zone_name)
                .map_err(|_| failed(format!("`{zone_name}` is not a time zone")))?;
            let cron = Cron::parse(&expression).map_err(failed)?;
            let runs: Vec<String> = cron
                .upcoming(
                    jiff::Timestamp::now(),
                    &zone,
                    count.unwrap_or(5).clamp(1, 20),
                )
                .iter()
                .map(|run| run.strftime("%a %Y-%m-%d %H:%M %Z").to_string())
                .collect();
            Ok(text(
                &json!({ "expression": expression, "timezone": zone_name, "next_runs": runs }),
            ))
        }
        _ => {
            let settings = state
                .db
                .call(|conn| automations::Settings::load(conn))
                .await?;
            Ok(text(&json!({
                "timezone": settings.timezone,
                "allow_private_network": settings.allow_private_network,
                "version": env!("CARGO_PKG_VERSION"),
                "webhook_base_url": context.hooks,
            })))
        }
    }
}

fn timestamp(at: i64) -> String {
    jiff::Timestamp::from_millisecond(at)
        .map(|at| at.to_string())
        .unwrap_or_default()
}

fn automation_json(
    automation: &store::Automation,
    hooks: &str,
    triggers: Option<&automations::Triggers>,
) -> Value {
    let mut value = json!({
        "id": automation.id,
        "name": automation.name,
        "kind": automation.kind,
        "enabled": automation.enabled,
        "last_error": automation.last_error,
        "updated_at": timestamp(automation.updated_at),
    });
    if let Some(fields) = value.as_object_mut() {
        if automation.kind == KIND_AUTOMATION {
            fields.insert(
                "webhook_url".to_owned(),
                json!(format!("{hooks}{}", automation.hook_token)),
            );
        } else {
            fields.insert(
                "require".to_owned(),
                json!(format!("require(\"{}\")", automation.name)),
            );
        }
        if let Some(triggers) = triggers {
            fields.insert("listens_to".to_owned(), json!(triggers));
        }
    }
    value
}

async fn read_tool(context: &Context<'_>, name: &str, params: &Value) -> ToolResult {
    let state = context.state;
    match name {
        "list_automations" => {
            let list = state.db.call(|conn| store::automations(conn)).await?;
            let triggers = state.automations.triggers();
            let list: Vec<Value> = list
                .iter()
                .map(|automation| {
                    automation_json(automation, &context.hooks, triggers.get(&automation.id))
                })
                .collect();
            Ok(text(&json!({ "automations": list })))
        }
        "get_automation" => {
            let IdArgs { id } = arguments(params)?;
            let (automation, runs) = state
                .db
                .call(move |conn| {
                    let automation = store::automation(conn, id)?.ok_or(AppError::NotFound)?;
                    Ok((automation, store::automation_runs(conn, id, 10)?))
                })
                .await?;
            let triggers = state.automations.triggers().remove(&id);
            let mut value = automation_json(&automation, &context.hooks, triggers.as_ref());
            if let Some(fields) = value.as_object_mut() {
                fields.insert("source".to_owned(), json!(automation.source));
                fields.insert("recent_runs".to_owned(), json!(runs_json(&runs)));
            }
            Ok(text(&value))
        }
        "list_versions" | "get_version" => versions_tool(context, name, params).await,
        "list_runs" => {
            let RunsArgs { id, limit } = arguments(params)?;
            let limit = limit.unwrap_or(20).clamp(1, 100);
            let runs = state
                .db
                .call(move |conn| {
                    store::automation(conn, id)?.ok_or(AppError::NotFound)?;
                    store::automation_runs(conn, id, limit)
                })
                .await?;
            Ok(text(&json!({ "runs": runs_json(&runs) })))
        }
        "list_data" => {
            let IdArgs { id } = arguments(params)?;
            let data = state
                .db
                .call(move |conn| {
                    store::automation(conn, id)?.ok_or(AppError::NotFound)?;
                    store::automation_values(conn, id)
                })
                .await?;
            let sorted: std::collections::BTreeMap<String, String> = data.into_iter().collect();
            Ok(text(&json!({ "data": sorted })))
        }
        _ => {
            let list = state.db.call(|conn| secrets::list(conn)).await?;
            let list: Vec<Value> = list
                .iter()
                .map(|secret| {
                    json!({
                        "name": secret.name,
                        "stored": secret.stored,
                        "from_environment": secret.from_environment,
                        "updated_at": secret.updated_at.map(timestamp),
                    })
                })
                .collect();
            Ok(text(&json!({ "secrets": list })))
        }
    }
}

async fn versions_tool(context: &Context<'_>, name: &str, params: &Value) -> ToolResult {
    let state = context.state;
    if name == "list_versions" {
        let IdArgs { id } = arguments(params)?;
        let versions = state
            .db
            .call(move |conn| {
                store::automation(conn, id)?.ok_or(AppError::NotFound)?;
                store::automation_versions(conn, id)
            })
            .await?;
        let list: Vec<Value> = versions
            .iter()
            .map(|version| {
                json!({
                    "version_id": version.id,
                    "saved_at": timestamp(version.saved_at),
                    "saved_by": version.saved_by,
                    "saved_with": version.saved_with,
                    "lines": version.source.lines().count(),
                })
            })
            .collect();
        Ok(text(&json!({ "versions": list })))
    } else {
        let VersionArgs { id, version_id } = arguments(params)?;
        let versions = state
            .db
            .call(move |conn| store::automation_versions(conn, id))
            .await?;
        let version = versions
            .into_iter()
            .find(|version| version.id == version_id)
            .ok_or_else(|| failed("No such version."))?;
        Ok(text(&Value::String(version.source)))
    }
}

#[derive(Deserialize)]
struct SaveArgs {
    id: Option<i64>,
    name: String,
    source: String,
    kind: Option<String>,
    enabled: Option<bool>,
}

#[derive(Deserialize)]
struct EnabledArgs {
    id: i64,
    enabled: bool,
}

#[derive(Deserialize)]
struct DataArgs {
    id: i64,
    key: String,
    value: Option<String>,
}

#[derive(Deserialize)]
struct SecretArgs {
    name: String,
    value: Option<String>,
}

#[derive(Deserialize)]
struct SettingsArgs {
    timezone: Option<String>,
    allow_private_network: Option<bool>,
}

async fn write_tool(context: &Context<'_>, name: &str, params: &Value) -> ToolResult {
    let state = context.state;
    let saved_with = format!("MCP: {}", context.caller.token_name);
    match name {
        "save_automation" => save(context, arguments(params)?).await,
        "set_enabled" => {
            let EnabledArgs { id, enabled } = arguments(params)?;
            let automation = state
                .db
                .call(move |conn| store::automation(conn, id)?.ok_or(AppError::NotFound))
                .await?;
            if automation.kind == KIND_LIBRARY {
                return Err(failed("Libraries do not run on their own."));
            }
            let saved = apply_change(
                state,
                Change {
                    id: Some(id),
                    name: automation.name,
                    source: automation.source,
                    enabled: Some(enabled),
                    kind: automation.kind,
                    user_id: context.caller.user_id,
                    saved_with,
                },
            )
            .await?
            .map_err(failed)?;
            status(context, saved).await
        }
        "delete_automation" => {
            let IdArgs { id } = arguments(params)?;
            state
                .db
                .call(move |conn| {
                    store::automation(conn, id)?.ok_or(AppError::NotFound)?;
                    store::delete_automation(conn, id)
                })
                .await?;
            state.automations.reload(state).await?;
            Ok(text(&json!({ "deleted": id })))
        }
        "restore_version" => {
            let VersionArgs { id, version_id } = arguments(params)?;
            restore_version(state, id, version_id, context.caller.user_id, &saved_with).await?;
            status(context, id).await
        }
        "rotate_webhook_url" => {
            let IdArgs { id } = arguments(params)?;
            state
                .db
                .call(move |conn| {
                    store::automation(conn, id)?.ok_or(AppError::NotFound)?;
                    store::new_hook_token(conn, id)
                })
                .await?;
            status(context, id).await
        }
        "set_data" => {
            let DataArgs { id, key, value } = arguments(params)?;
            state
                .db
                .call(move |conn| {
                    store::automation(conn, id)?.ok_or(AppError::NotFound)?;
                    store::set_automation_value(conn, id, &key, value.as_deref())
                })
                .await?;
            Ok(text(&json!({ "ok": true })))
        }
        "set_secret" | "delete_secret" => secret(context, name, arguments(params)?).await,
        _ => {
            let SettingsArgs {
                timezone,
                allow_private_network,
            } = arguments(params)?;
            let settings = state
                .db
                .call(move |conn| {
                    let mut settings = automations::Settings::load(conn)?;
                    if let Some(timezone) = timezone {
                        settings.timezone = timezone;
                    }
                    if let Some(allow) = allow_private_network {
                        settings.allow_private_network = allow;
                    }
                    settings.save(conn)?;
                    Ok(settings)
                })
                .await?;
            state.automations.reload(state).await?;
            Ok(text(&json!({
                "timezone": settings.timezone,
                "allow_private_network": settings.allow_private_network,
            })))
        }
    }
}

async fn secret(context: &Context<'_>, name: &str, args: SecretArgs) -> ToolResult {
    let state = context.state;
    let secret = args.name.trim().to_owned();
    if name == "set_secret" {
        let value = args
            .value
            .ok_or_else(|| failed("Pass the secret's value."))?;
        let vault = Arc::clone(&state.vault);
        let user_id = context.caller.user_id;
        let stored = secret.clone();
        state
            .db
            .call(move |conn| secrets::set(conn, &vault, &stored, &value, user_id, now_ms()))
            .await?;
        state.automations.reload(state).await?;
        return Ok(text(&json!({ "stored": secret })));
    }
    let lookup = secret.clone();
    let deleted = state
        .db
        .call(move |conn| secrets::delete(conn, &lookup))
        .await?;
    if !deleted {
        return Err(failed(format!("There is no stored secret named {secret}.")));
    }
    state.automations.reload(state).await?;
    Ok(text(&json!({ "deleted": secret })))
}

async fn save(context: &Context<'_>, args: SaveArgs) -> ToolResult {
    check_size(&args.source)?;
    let checked = args.source.clone();
    let diagnostics = blocking(move || tooling::lint(&checked)).await?;
    if tooling::has_errors(&diagnostics) {
        let found: Vec<String> = diagnostics.iter().map(ToString::to_string).collect();
        return Err(failed(format!(
            "Not saved: the script has errors.\n{}",
            found.join("\n")
        )));
    }
    let saved = apply_change(
        context.state,
        Change {
            id: args.id,
            name: args.name,
            source: args.source,
            enabled: args.enabled,
            kind: args.kind.unwrap_or_else(|| KIND_AUTOMATION.to_owned()),
            user_id: context.caller.user_id,
            saved_with: format!("MCP: {}", context.caller.token_name),
        },
    )
    .await?
    .map_err(failed)?;
    let mut result = status(context, saved).await?;
    let warnings: Vec<String> = diagnostics.iter().map(ToString::to_string).collect();
    if let Some(fields) = result
        .get_mut("structuredContent")
        .and_then(Value::as_object_mut)
    {
        fields.insert("warnings".to_owned(), json!(warnings));
    }
    Ok(result)
}

/// An automation's state after a change, including load errors.
async fn status(context: &Context<'_>, id: i64) -> ToolResult {
    let automation = context
        .state
        .db
        .call(move |conn| store::automation(conn, id)?.ok_or(AppError::NotFound))
        .await?;
    let triggers = context.state.automations.triggers().remove(&id);
    Ok(text(&automation_json(
        &automation,
        &context.hooks,
        triggers.as_ref(),
    )))
}

#[derive(Deserialize)]
struct TestArgs {
    source: Option<String>,
    id: Option<i64>,
    trigger: TestTrigger,
    #[serde(default)]
    http: bool,
}

#[derive(Deserialize)]
struct RunArgs {
    id: i64,
    trigger: TestTrigger,
}

async fn dev_tool(context: &Context<'_>, name: &str, params: &Value) -> ToolResult {
    let state = context.state;
    match name {
        "lint_lua" => {
            let SourceArgs { source } = arguments(params)?;
            check_size(&source)?;
            let diagnostics = blocking(move || tooling::lint(&source)).await?;
            Ok(text(&json!({ "diagnostics": diagnostics })))
        }
        "format_lua" => {
            let SourceArgs { source } = arguments(params)?;
            check_size(&source)?;
            match blocking(move || tooling::format(&source)).await? {
                Ok(formatted) => Ok(text(&Value::String(formatted))),
                Err(diagnostics) => Err(failed(
                    diagnostics
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("\n"),
                )),
            }
        }
        "test_automation" => {
            let TestArgs {
                source,
                id,
                trigger,
                http,
            } = arguments(params)?;
            let (name, source) = match (source, id) {
                (Some(source), _) => (String::new(), source),
                (None, Some(id)) => {
                    let automation = state
                        .db
                        .call(move |conn| store::automation(conn, id)?.ok_or(AppError::NotFound))
                        .await?;
                    (automation.name, automation.source)
                }
                (None, None) => return Err(failed("Pass source or id.")),
            };
            check_size(&source)?;
            let report = run_test(state, id, name, source, trigger, http).await?;
            Ok(text(&serde_json::to_value(report).unwrap_or_default()))
        }
        _ => {
            let RunArgs { id, trigger } = arguments(params)?;
            let report = state.automations.run(id, trigger).await?;
            Ok(text(&serde_json::to_value(report).unwrap_or_default()))
        }
    }
}

fn runs_json(runs: &[store::AutomationRun]) -> Vec<Value> {
    runs.iter()
        .map(|run| {
            json!({
                "trigger": run.trigger,
                "at": timestamp(run.started_at),
                "duration_us": run.duration_us,
                "output": run.output,
                "error": run.error,
            })
        })
        .collect()
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, ToolError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| failed(error.to_string()))
}

async fn resources(context: &Context<'_>) -> Result<Value, (i64, String)> {
    let list = context
        .state
        .db
        .call(|conn| store::automations(conn))
        .await
        .map_err(|error| (-32603, error.to_string()))?;
    let mut resources = vec![json!({
        "uri": "sideporch://reference",
        "name": "reference",
        "title": "Automation API reference",
        "mimeType": "text/markdown",
    })];
    resources.extend(list.iter().map(|automation| {
        json!({
            "uri": format!("sideporch://automations/{}", automation.id),
            "name": automation.name,
            "title": format!("{} ({})", automation.name, automation.kind),
            "mimeType": "text/x-lua",
        })
    }));
    Ok(json!({ "resources": resources }))
}

async fn read_resource(context: &Context<'_>, params: &Value) -> Result<Value, (i64, String)> {
    let uri = params
        .get("uri")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if uri == "sideporch://reference" {
        return Ok(
            json!({ "contents": [{ "uri": uri, "mimeType": "text/markdown", "text": api::reference() }] }),
        );
    }
    let id = uri
        .strip_prefix("sideporch://automations/")
        .and_then(|id| id.parse::<i64>().ok())
        .ok_or_else(|| (-32002, format!("Resource not found: {uri}")))?;
    let automation = context
        .state
        .db
        .call(move |conn| store::automation(conn, id))
        .await
        .map_err(|error| (-32603, error.to_string()))?
        .ok_or_else(|| (-32002, format!("Resource not found: {uri}")))?;
    Ok(json!({ "contents": [{ "uri": uri, "mimeType": "text/x-lua", "text": automation.source }] }))
}

fn prompt(params: &Value) -> Result<Value, (i64, String)> {
    if params.get("name").and_then(Value::as_str) != Some("write_automation") {
        return Err((-32602, "Unknown prompt".to_owned()));
    }
    let task = params
        .pointer("/arguments/task")
        .and_then(Value::as_str)
        .unwrap_or("something useful");
    Ok(json!({
        "description": "Write a Sideporch automation",
        "messages": [{
            "role": "user",
            "content": { "type": "text", "text": format!(
                "Write a Sideporch automation that does this: {task}\n\n\
                Steps: read get_api_reference; look at list_channels, list_automations (reuse libraries) and list_secrets; \
                write the script with a comment above each handler; run lint_lua and fix everything it reports; \
                try it with test_automation for each trigger it handles; then save it with save_automation, switched off, \
                and tell me what it does, what to set up (secrets, channels, the webhook URL) and how to switch it on."
            )},
        }],
    }))
}
