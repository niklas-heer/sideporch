//! Automation pages, the editor's JSON endpoints, and automation webhooks.

use axum::{
    Form, Json, Router,
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
    routing::{any, get, post},
};
use maud::Markup;
use serde::{Deserialize, Serialize};

use super::{base_url, shell_data};
use crate::{
    AppState, ai,
    auth::CurrentUser,
    automations::{self, TestTrigger, WebhookRequest, tooling},
    error::{AppError, AppResult},
    now_ms,
    store::{self, AutomationEdit},
    views::{self, Shell},
};

/// Longest script Sideporch accepts, in bytes.
pub const MAX_SOURCE_BYTES: usize = 100_000;
/// Runs shown in the editor.
const SHOWN_RUNS: i64 = 30;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/automations", get(list).post(create))
        .route("/automations/new", get(new))
        .route("/automations/lint", post(lint))
        .route("/automations/format", post(format))
        .route("/automations/test", post(test))
        .route("/automations/ai", post(write_with_ai))
        .route("/automations/{automation_id}", get(edit).post(update))
        .route("/automations/{automation_id}/delete", post(delete))
        .route(
            "/automations/{automation_id}/webhook-token",
            post(new_hook_token),
        )
        .route(
            "/automations/{automation_id}/versions/{version_id}/restore",
            post(restore),
        )
        .route("/hooks/automations/{token}", any(hook))
        .route("/hooks/automations/{token}/{*path}", any(hook_path))
}

const fn require_admin(user: &CurrentUser) -> AppResult<()> {
    if user.is_admin {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

async fn list(user: CurrentUser, State(state): State<AppState>) -> AppResult<Markup> {
    require_admin(&user)?;
    let list = state.db.call(|conn| store::automations(conn)).await?;
    let sidebar = shell_data(&state, user.id).await?;
    let shell = Shell {
        user: &user,
        sidebar: &sidebar,
        current: None,
    };
    let triggers = state.automations.triggers();
    Ok(views::automations::list_page(&shell, &list, &triggers))
}

async fn render_editor(
    state: &AppState,
    user: &CurrentUser,
    editor: &views::automations::Editor<'_>,
) -> AppResult<Markup> {
    let sidebar = shell_data(state, user.id).await?;
    let shell = Shell {
        user,
        sidebar: &sidebar,
        current: None,
    };
    Ok(views::automations::editor_page(&shell, editor))
}

#[derive(Deserialize)]
struct NewQuery {
    kind: Option<String>,
}

async fn new(
    user: CurrentUser,
    State(state): State<AppState>,
    Query(query): Query<NewQuery>,
) -> AppResult<Markup> {
    require_admin(&user)?;
    let ai_ready = ai_ready(&state).await?;
    let library = query.kind.as_deref() == Some(automations::KIND_LIBRARY);
    let editor = views::automations::Editor {
        source: if library {
            automations::LIBRARY_EXAMPLE
        } else {
            automations::EXAMPLE
        },
        enabled: !library,
        library,
        ai_ready,
        ..views::automations::Editor::default()
    };
    render_editor(&state, &user, &editor).await
}

async fn edit(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(automation_id): Path<i64>,
    headers: HeaderMap,
) -> AppResult<Markup> {
    require_admin(&user)?;
    let (automation, runs, versions, ai_ready) = state
        .db
        .call(move |conn| {
            let automation = store::automation(conn, automation_id)?.ok_or(AppError::NotFound)?;
            Ok((
                automation,
                store::automation_runs(conn, automation_id, SHOWN_RUNS)?,
                store::automation_versions(conn, automation_id)?,
                ai::configured(conn)?,
            ))
        })
        .await?;
    let triggers = state.automations.triggers().remove(&automation_id);
    let hook_url = format!(
        "{}/hooks/automations/{}",
        base_url(&state, &headers),
        automation.hook_token
    );
    let editor = views::automations::Editor {
        id: Some(automation.id),
        name: &automation.name,
        source: &automation.source,
        enabled: automation.enabled,
        last_error: automation.last_error.as_deref(),
        form_error: None,
        hook_url: Some(&hook_url),
        runs: &runs,
        versions: &versions,
        ai_ready,
        library: automation.kind == automations::KIND_LIBRARY,
        triggers: triggers.as_ref(),
    };
    render_editor(&state, &user, &editor).await
}

#[derive(Deserialize)]
struct AutomationForm {
    name: String,
    source: String,
    enabled: Option<String>,
    /// Only read when creating.
    kind: Option<String>,
}

/// A change to an automation, from the editor, a restore, or MCP.
pub struct Change {
    pub id: Option<i64>,
    pub name: String,
    pub source: String,
    /// `None` keeps an existing automation's switch; new ones start off.
    pub enabled: Option<bool>,
    /// Only used when creating.
    pub kind: String,
    pub user_id: i64,
    pub saved_with: String,
}

/// Checks and saves a change, then restarts the automations. Returns the
/// automation's id, or why the change was refused.
pub async fn apply_change(state: &AppState, change: Change) -> AppResult<Result<i64, String>> {
    let name: String = change.name.trim().chars().take(80).collect();
    if name.is_empty() {
        return Ok(Err("Give the automation a name.".to_owned()));
    }
    if change.source.len() > MAX_SOURCE_BYTES {
        return Ok(Err("Keep the script under 100 kB.".to_owned()));
    }
    let now = now_ms();
    let saved = state
        .db
        .call(move |conn| {
            let (kind, enabled) = match change.id {
                Some(id) => {
                    let current = store::automation(conn, id)?.ok_or(AppError::NotFound)?;
                    (current.kind, change.enabled.unwrap_or(current.enabled))
                }
                None if change.kind == automations::KIND_LIBRARY => {
                    (change.kind, false)
                }
                None => (automations::KIND_AUTOMATION.to_owned(), change.enabled.unwrap_or(false)),
            };
            if kind == automations::KIND_LIBRARY {
                if !automations::valid_library_name(&name) {
                    return Ok(Err(
                        "Name libraries like Lua modules: lowercase letters, digits and underscores, starting with a letter, such as github_api.".to_owned(),
                    ));
                }
                if store::library_name_taken(conn, &name, change.id)? {
                    return Ok(Err(format!("There is already a library named {name}.")));
                }
            }
            Ok(Ok(store::save_automation(
                conn,
                change.id,
                &AutomationEdit {
                    name: &name,
                    source: &change.source,
                    // Libraries never run on their own.
                    enabled: enabled && kind != automations::KIND_LIBRARY,
                    user_id: change.user_id,
                    saved_with: &change.saved_with,
                    kind: &kind,
                },
                now,
            )?))
        })
        .await?;
    if saved.is_ok() {
        state.automations.reload(state).await?;
    }
    Ok(saved)
}

async fn save(
    state: &AppState,
    user: &CurrentUser,
    id: Option<i64>,
    form: AutomationForm,
) -> AppResult<Response> {
    require_admin(user)?;
    let enabled = form.enabled.is_some();
    let library = form.kind.as_deref() == Some(automations::KIND_LIBRARY);
    let change = Change {
        id,
        name: form.name.clone(),
        source: form.source.clone(),
        enabled: Some(enabled),
        kind: form
            .kind
            .clone()
            .unwrap_or_else(|| automations::KIND_AUTOMATION.to_owned()),
        user_id: user.id,
        saved_with: "editor".to_owned(),
    };
    match apply_change(state, change).await? {
        Ok(saved) => Ok(Redirect::to(&format!("/automations/{saved}")).into_response()),
        Err(error) => {
            let library = match id {
                Some(id) => state
                    .db
                    .call(move |conn| store::automation(conn, id))
                    .await?
                    .is_some_and(|automation| automation.kind == automations::KIND_LIBRARY),
                None => library,
            };
            let editor = views::automations::Editor {
                id,
                name: &form.name,
                source: &form.source,
                enabled,
                library,
                form_error: Some(&error),
                ai_ready: ai_ready(state).await?,
                ..views::automations::Editor::default()
            };
            let page = render_editor(state, user, &editor).await?;
            Ok((StatusCode::BAD_REQUEST, page).into_response())
        }
    }
}

async fn create(
    user: CurrentUser,
    State(state): State<AppState>,
    Form(form): Form<AutomationForm>,
) -> AppResult<Response> {
    save(&state, &user, None, form).await
}

async fn update(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(automation_id): Path<i64>,
    Form(form): Form<AutomationForm>,
) -> AppResult<Response> {
    save(&state, &user, Some(automation_id), form).await
}

async fn delete(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(automation_id): Path<i64>,
) -> AppResult<Response> {
    require_admin(&user)?;
    state
        .db
        .call(move |conn| store::delete_automation(conn, automation_id))
        .await?;
    state.automations.reload(&state).await?;
    Ok(Redirect::to("/automations").into_response())
}

/// Saves an earlier version of an automation's script as its newest.
pub async fn restore_version(
    state: &AppState,
    automation_id: i64,
    version_id: i64,
    user_id: i64,
    saved_with: &str,
) -> AppResult<()> {
    let (automation, version) = state
        .db
        .call(move |conn| {
            let automation = store::automation(conn, automation_id)?.ok_or(AppError::NotFound)?;
            let version = store::automation_versions(conn, automation_id)?
                .into_iter()
                .find(|version| version.id == version_id)
                .ok_or(AppError::NotFound)?;
            Ok((automation, version))
        })
        .await?;
    apply_change(
        state,
        Change {
            id: Some(automation_id),
            name: automation.name,
            source: version.source,
            enabled: None,
            kind: automation.kind,
            user_id,
            saved_with: saved_with.to_owned(),
        },
    )
    .await?
    .map_err(AppError::bad_request)?;
    Ok(())
}

async fn restore(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((automation_id, version_id)): Path<(i64, i64)>,
) -> AppResult<Response> {
    require_admin(&user)?;
    restore_version(&state, automation_id, version_id, user.id, "restore").await?;
    Ok(Redirect::to(&format!("/automations/{automation_id}")).into_response())
}

async fn new_hook_token(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(automation_id): Path<i64>,
) -> AppResult<Response> {
    require_admin(&user)?;
    state
        .db
        .call(move |conn| {
            store::automation(conn, automation_id)?.ok_or(AppError::NotFound)?;
            store::new_hook_token(conn, automation_id)
        })
        .await?;
    Ok(Redirect::to(&format!("/automations/{automation_id}#webhook")).into_response())
}

// The editor's JSON endpoints

#[derive(Deserialize)]
struct SourceInput {
    source: String,
}

fn check_size(source: &str) -> AppResult<()> {
    if source.len() > MAX_SOURCE_BYTES {
        return Err(AppError::bad_request("The script is larger than 100 kB."));
    }
    Ok(())
}

#[derive(Serialize)]
struct LintOutput {
    diagnostics: Vec<tooling::Diagnostic>,
}

async fn lint(user: CurrentUser, Json(input): Json<SourceInput>) -> AppResult<Json<LintOutput>> {
    require_admin(&user)?;
    check_size(&input.source)?;
    let diagnostics = tokio::task::spawn_blocking(move || tooling::lint(&input.source))
        .await
        .map_err(AppError::internal)?;
    Ok(Json(LintOutput { diagnostics }))
}

#[derive(Serialize)]
struct FormatOutput {
    source: Option<String>,
    diagnostics: Vec<tooling::Diagnostic>,
}

async fn format(
    user: CurrentUser,
    Json(input): Json<SourceInput>,
) -> AppResult<Json<FormatOutput>> {
    require_admin(&user)?;
    check_size(&input.source)?;
    let result = tokio::task::spawn_blocking(move || tooling::format(&input.source))
        .await
        .map_err(AppError::internal)?;
    Ok(Json(match result {
        Ok(source) => FormatOutput {
            source: Some(source),
            diagnostics: Vec::new(),
        },
        Err(diagnostics) => FormatOutput {
            source: None,
            diagnostics,
        },
    }))
}

#[derive(Deserialize)]
struct TestInput {
    source: String,
    #[serde(default)]
    name: String,
    /// Uses this automation's saved data, without changing it.
    automation_id: Option<i64>,
    trigger: TestTrigger,
    /// Whether HTTP requests really go out.
    #[serde(default = "yes")]
    http: bool,
}

const fn yes() -> bool {
    true
}

async fn test(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(input): Json<TestInput>,
) -> AppResult<Json<automations::TestReport>> {
    require_admin(&user)?;
    check_size(&input.source)?;
    Ok(Json(
        run_test(
            &state,
            input.automation_id,
            input.name,
            input.source,
            input.trigger,
            input.http,
        )
        .await?,
    ))
}

/// Dry-runs `source`, with the saved data of `automation_id` if given.
/// HTTP requests go out when `http` is set.
pub async fn run_test(
    state: &AppState,
    automation_id: Option<i64>,
    name: String,
    source: String,
    trigger: TestTrigger,
    http: bool,
) -> AppResult<automations::TestReport> {
    let vault = std::sync::Arc::clone(&state.vault);
    let context = state
        .db
        .call(move |conn| automations::TestContext::load(conn, &vault, automation_id, http))
        .await?;
    let name = if name.trim().is_empty() {
        "Automation".to_owned()
    } else {
        name
    };
    tokio::task::spawn_blocking(move || automations::test(&name, &source, context, &trigger))
        .await
        .map_err(AppError::internal)
}

async fn ai_ready(state: &AppState) -> AppResult<bool> {
    state.db.call(|conn| ai::configured(conn)).await
}

#[derive(Deserialize)]
struct AiInput {
    prompt: String,
    #[serde(default)]
    source: String,
    #[serde(default)]
    name: String,
    /// `library` when writing a library.
    kind: Option<String>,
    automation_id: Option<i64>,
}

async fn write_with_ai(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(input): Json<AiInput>,
) -> AppResult<Json<ai::Draft>> {
    require_admin(&user)?;
    check_size(&input.source)?;
    let prompt = input.prompt.trim();
    if prompt.is_empty() || prompt.chars().count() > 4_000 {
        return Err(AppError::bad_request(
            "Describe what the script should do, in up to 4,000 characters.",
        ));
    }
    let vault = std::sync::Arc::clone(&state.vault);
    let editing = input.automation_id;
    let (provider, channels, automations) = state
        .db
        .call(move |conn| {
            Ok((
                ai::provider(conn, &vault)?,
                store::public_channel_names(conn)?,
                store::automations(conn)?,
            ))
        })
        .await?;
    let provider =
        provider.ok_or_else(|| AppError::bad_request("No AI provider is connected yet."))?;
    let libraries = automations
        .into_iter()
        .filter(|automation| {
            automation.kind == automations::KIND_LIBRARY && Some(automation.id) != editing
        })
        .map(|library| (library.name, library.source))
        .collect();
    let request = ai::ScriptRequest {
        prompt: prompt.to_owned(),
        name: input.name,
        source: input.source,
        channels,
        libraries,
        library: input.kind.as_deref() == Some(automations::KIND_LIBRARY),
    };
    Ok(Json(
        ai::write_script(&state.ai, &provider, &request).await?,
    ))
}

// Automation webhooks

async fn hook(
    State(state): State<AppState>,
    Path(token): Path<String>,
    method: Method,
    Query(query): Query<Vec<(String, String)>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    answer_hook(
        &state,
        &token,
        String::new(),
        method,
        query,
        &headers,
        &body,
    )
    .await
}

async fn hook_path(
    State(state): State<AppState>,
    Path((token, path)): Path<(String, String)>,
    method: Method,
    Query(query): Query<Vec<(String, String)>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    answer_hook(
        &state,
        &token,
        format!("/{path}"),
        method,
        query,
        &headers,
        &body,
    )
    .await
}

async fn answer_hook(
    state: &AppState,
    token: &str,
    path: String,
    method: Method,
    query: Vec<(String, String)>,
    headers: &HeaderMap,
    body: &[u8],
) -> Response {
    let lookup = token.to_owned();
    let found = state
        .db
        .call(move |conn| store::automation_by_hook_token(conn, &lookup))
        .await;
    let automation_id = match found {
        Ok(Some(id)) => id,
        Ok(None) => {
            return (StatusCode::NOT_FOUND, "No automation has this webhook URL.").into_response();
        }
        Err(error) => return error.into_response(),
    };
    let request = WebhookRequest {
        method: method.to_string(),
        path,
        query,
        headers: headers
            .iter()
            // The session cookie is none of the script's business.
            .filter(|(name, _)| *name != header::COOKIE)
            .filter_map(|(name, value)| Some((name.to_string(), value.to_str().ok()?.to_owned())))
            .collect(),
        body: String::from_utf8_lossy(body).into_owned(),
    };
    let response = state.automations.webhook(automation_id, request).await;
    let status = StatusCode::from_u16(response.status).unwrap_or(StatusCode::OK);
    let content_type = HeaderValue::from_str(&response.content_type)
        .unwrap_or(HeaderValue::from_static("text/plain; charset=utf-8"));
    (
        status,
        [(header::CONTENT_TYPE, content_type)],
        response.body,
    )
        .into_response()
}
