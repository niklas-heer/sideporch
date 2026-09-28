//! A [Model Context Protocol](https://modelcontextprotocol.io) server, so AI
//! agents such as Claude Code can write, test and manage automations.
//!
//! It speaks the Streamable HTTP transport without sessions: each POST to
//! `/mcp` carries one JSON-RPC message and gets one JSON answer. Clients
//! authenticate with an API token that an admin creates under Settings;
//! tokens act as that admin and stop working if they lose admin rights.

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
    automations::{TestTrigger, api, tooling},
    error::{AppError, AppResult},
    now_ms,
    routes::{base_url, run_test},
    store::{self, AutomationEdit},
};

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
                "Send an API token as `Authorization: Bearer <token>`. Admins create tokens under Settings → MCP.",
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
    let answer = match method {
        "initialize" => rpc_result(&id, &initialize(&params)),
        "ping" => rpc_result(&id, &json!({})),
        "tools/list" => rpc_result(&id, &json!({ "tools": tools() })),
        "tools/call" => match call_tool(&state, &headers, &caller, &params).await {
            Ok(result) => rpc_result(&id, &result),
            Err(ToolError::Unknown(name)) => {
                rpc_error(&id, -32602, &format!("Unknown tool: {name}"))
            }
            Err(ToolError::Failed(message)) => rpc_result(
                &id,
                &json!({ "content": [{ "type": "text", "text": message }], "isError": true }),
            ),
        },
        _ => rpc_error(&id, -32601, &format!("Method not found: {method}")),
    };
    Json(answer).into_response()
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
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "sideporch", "title": "Sideporch", "version": env!("CARGO_PKG_VERSION") },
        "instructions": format!(
            "This server manages automations in a Sideporch team chat: Lua scripts that answer messages, \
    react to reactions, receive webhooks and run on timers. Before saving, lint with lint_lua and try the \
    script with test_automation, which changes nothing. New automations start switched off unless you pass \
    enabled: true; tell the person to review and switch them on.\n\n{}",
            api::reference()
        ),
    })
}

/// How a tool affects Sideporch, for clients that ask before changes.
#[derive(Clone, Copy)]
enum Effect {
    Reads,
    Changes,
    Deletes,
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
            "openWorldHint": false,
        },
    })
}

fn object(properties: &Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": properties, "required": required, "additionalProperties": false })
}

fn trigger_schema() -> Value {
    json!({
        "type": "object",
        "description": "The event to simulate. kind is one of load, message, reaction, webhook, timer.",
        "properties": {
            "kind": { "type": "string", "enum": ["load", "message", "reaction", "webhook", "timer"] },
            "text": { "type": "string", "description": "Message text (message, reaction)" },
            "channel": { "type": "string", "description": "Channel name (message, reaction); default general" },
            "author": { "type": "string", "description": "Author display name (message)" },
            "emoji": { "type": "string", "description": "Emoji name (reaction); default thumbsup" },
            "added": { "type": "boolean", "description": "Reaction added (true) or removed (reaction)" },
            "method": { "type": "string", "description": "HTTP method (webhook); default POST" },
            "path": { "type": "string", "description": "Path after the webhook token, such as /deploy (webhook)" },
            "body": { "type": "string", "description": "Request body (webhook)" }
        },
        "required": ["kind"]
    })
}

fn tools() -> Vec<Value> {
    let source = json!({ "type": "string", "description": "The Lua script" });
    let id = json!({ "type": "integer", "description": "The automation's id" });
    let nothing = object(&json!({}), &[]);
    let by_id = object(&json!({ "id": id }), &["id"]);
    let by_source = object(&json!({ "source": source }), &["source"]);
    vec![
        tool(
            "get_api_reference",
            "Automation API reference",
            "The complete Lua API automations can use, with limits and the fields of every event table.",
            &nothing,
            Effect::Reads,
        ),
        tool(
            "list_automations",
            "List automations",
            "All automations with their status, last error and webhook URL.",
            &nothing,
            Effect::Reads,
        ),
        tool(
            "get_automation",
            "Get an automation",
            "One automation's script, status, webhook URL and its most recent runs.",
            &by_id,
            Effect::Reads,
        ),
        tool(
            "list_channels",
            "List public channels",
            "Names of the public channels scripts can post to and hear from.",
            &nothing,
            Effect::Reads,
        ),
        tool(
            "lint_lua",
            "Lint a script",
            "Syntax errors and lint findings for a script, with line and column. Knows the sandbox and the sideporch API.",
            &by_source,
            Effect::Reads,
        ),
        tool(
            "format_lua",
            "Format a script",
            "The script formatted in Sideporch's style (StyLua, two-space indentation).",
            &by_source,
            Effect::Reads,
        ),
        tool(
            "test_automation",
            "Test a script",
            "Runs a script against a simulated event in a fresh sandbox and returns its printed output, the posts and reactions it would make, and any error. Nothing in Sideporch changes. Pass source, or id to test a saved automation (with its saved data).",
            &object(
                &json!({ "source": source, "id": id, "trigger": trigger_schema() }),
                &["trigger"],
            ),
            Effect::Reads,
        ),
        tool(
            "save_automation",
            "Create or update an automation",
            "Creates an automation (without id) or updates one (with id). Refuses scripts with lint errors. New automations start switched off unless enabled is true; updates keep the current switch unless enabled is given. Every save is kept in the automation's history.",
            &object(
                &json!({
                    "id": id,
                    "name": { "type": "string", "description": "Shown as the author of its posts; up to 80 characters" },
                    "source": source,
                    "enabled": { "type": "boolean", "description": "Whether the automation runs" }
                }),
                &["name", "source"],
            ),
            Effect::Changes,
        ),
        tool(
            "list_runs",
            "Recent runs",
            "An automation's recent runs that printed, acted, answered a webhook or failed, newest first.",
            &object(
                &json!({ "id": id, "limit": { "type": "integer", "minimum": 1, "maximum": 100 } }),
                &["id"],
            ),
            Effect::Reads,
        ),
        tool(
            "delete_automation",
            "Delete an automation",
            "Deletes an automation with its history, runs and saved data. This cannot be undone.",
            &by_id,
            Effect::Deletes,
        ),
    ]
}

enum ToolError {
    Unknown(String),
    Failed(String),
}

impl From<AppError> for ToolError {
    fn from(error: AppError) -> Self {
        Self::Failed(error.to_string())
    }
}

fn arguments<T: for<'de> Deserialize<'de>>(params: &Value) -> Result<T, ToolError> {
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    serde_json::from_value(arguments)
        .map_err(|error| ToolError::Failed(format!("Invalid arguments: {error}")))
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
        return Err(ToolError::Failed(
            "The script is larger than 100 kB.".to_owned(),
        ));
    }
    Ok(())
}

#[derive(Deserialize)]
struct IdArgs {
    id: i64,
}

#[derive(Deserialize)]
struct SourceArgs {
    source: String,
}

#[derive(Deserialize)]
struct TestArgs {
    source: Option<String>,
    id: Option<i64>,
    trigger: TestTrigger,
}

#[derive(Deserialize)]
struct SaveArgs {
    id: Option<i64>,
    name: String,
    source: String,
    enabled: Option<bool>,
}

#[derive(Deserialize)]
struct RunsArgs {
    id: i64,
    limit: Option<i64>,
}

async fn call_tool(
    state: &AppState,
    headers: &HeaderMap,
    caller: &Caller,
    params: &Value,
) -> Result<Value, ToolError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let hooks = format!("{}/hooks/automations/", base_url(state, headers));
    match name {
        "get_api_reference" => Ok(text(&Value::String(api::reference()))),
        "list_automations" => {
            let automations = state.db.call(|conn| store::automations(conn)).await?;
            let list: Vec<Value> = automations
                .iter()
                .map(|automation| {
                    json!({
                        "id": automation.id,
                        "name": automation.name,
                        "enabled": automation.enabled,
                        "last_error": automation.last_error,
                        "webhook_url": format!("{hooks}{}", automation.hook_token),
                    })
                })
                .collect();
            Ok(text(&json!({ "automations": list })))
        }
        "get_automation" => get_automation(state, &hooks, arguments(params)?).await,
        "list_channels" => {
            let channels = state
                .db
                .call(|conn| store::public_channel_names(conn))
                .await?;
            Ok(text(&json!({ "channels": channels })))
        }
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
                Err(diagnostics) => Err(ToolError::Failed(
                    diagnostics
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("\n"),
                )),
            }
        }
        "test_automation" => test(state, arguments(params)?).await,
        "save_automation" => save(state, caller, arguments(params)?).await,
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
        "delete_automation" => delete(state, arguments(params)?).await,
        other => Err(ToolError::Unknown(other.to_owned())),
    }
}

async fn get_automation(
    state: &AppState,
    hooks: &str,
    IdArgs { id }: IdArgs,
) -> Result<Value, ToolError> {
    let (automation, runs) = state
        .db
        .call(move |conn| {
            let automation = store::automation(conn, id)?.ok_or(AppError::NotFound)?;
            Ok((automation, store::automation_runs(conn, id, 10)?))
        })
        .await?;
    Ok(text(&json!({
        "id": automation.id,
        "name": automation.name,
        "enabled": automation.enabled,
        "last_error": automation.last_error,
        "webhook_url": format!("{hooks}{}", automation.hook_token),
        "source": automation.source,
        "recent_runs": runs_json(&runs),
    })))
}

async fn test(state: &AppState, args: TestArgs) -> Result<Value, ToolError> {
    let TestArgs {
        source,
        id,
        trigger,
    } = args;
    let (name, source) = match (source, id) {
        (Some(source), _) => (String::new(), source),
        (None, Some(id)) => {
            let automation = state
                .db
                .call(move |conn| store::automation(conn, id)?.ok_or(AppError::NotFound))
                .await?;
            (automation.name, automation.source)
        }
        (None, None) => return Err(ToolError::Failed("Pass source or id.".to_owned())),
    };
    check_size(&source)?;
    let report = run_test(state, id, name, source, trigger).await?;
    Ok(text(&serde_json::to_value(report).unwrap_or_default()))
}

async fn delete(state: &AppState, IdArgs { id }: IdArgs) -> Result<Value, ToolError> {
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

async fn save(state: &AppState, caller: &Caller, args: SaveArgs) -> Result<Value, ToolError> {
    check_size(&args.source)?;
    let name: String = args.name.trim().chars().take(80).collect();
    if name.is_empty() {
        return Err(ToolError::Failed("Give the automation a name.".to_owned()));
    }
    let source = args.source;
    let checked = source.clone();
    let diagnostics = blocking(move || tooling::lint(&checked)).await?;
    if tooling::has_errors(&diagnostics) {
        let found: Vec<String> = diagnostics.iter().map(ToString::to_string).collect();
        return Err(ToolError::Failed(format!(
            "Not saved: the script has errors.\n{}",
            found.join("\n")
        )));
    }
    let user_id = caller.user_id;
    let saved_with = format!("MCP: {}", caller.token_name);
    let now = now_ms();
    let id = args.id;
    let requested = args.enabled;
    let saved = state
        .db
        .call(move |conn| {
            let enabled = match id {
                Some(id) => {
                    let current = store::automation(conn, id)?.ok_or(AppError::NotFound)?;
                    requested.unwrap_or(current.enabled)
                }
                None => requested.unwrap_or(false),
            };
            store::save_automation(
                conn,
                id,
                &AutomationEdit {
                    name: &name,
                    source: &source,
                    enabled,
                    user_id,
                    saved_with: &saved_with,
                },
                now,
            )
        })
        .await?;
    state.automations.reload(state).await?;
    let automation = state
        .db
        .call(move |conn| store::automation(conn, saved)?.ok_or(AppError::NotFound))
        .await?;
    let warnings: Vec<String> = diagnostics.iter().map(ToString::to_string).collect();
    Ok(text(&json!({
        "id": automation.id,
        "name": automation.name,
        "enabled": automation.enabled,
        "load_error": automation.last_error,
        "warnings": warnings,
    })))
}

fn runs_json(runs: &[store::AutomationRun]) -> Vec<Value> {
    runs.iter()
        .map(|run| {
            json!({
                "trigger": run.trigger,
                "at": jiff::Timestamp::from_millisecond(run.started_at).map(|at| at.to_string()).unwrap_or_default(),
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
        .map_err(|error| ToolError::Failed(error.to_string()))
}
