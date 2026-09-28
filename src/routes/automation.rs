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
    Ok(views::automations::list_page(&shell, &list))
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

async fn new(user: CurrentUser, State(state): State<AppState>) -> AppResult<Markup> {
    require_admin(&user)?;
    let ai_ready = ai_ready(&state).await?;
    let editor = views::automations::Editor {
        source: automations::EXAMPLE,
        enabled: true,
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
                ai::provider(conn)?.is_some(),
            ))
        })
        .await?;
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
    };
    render_editor(&state, &user, &editor).await
}

#[derive(Deserialize)]
struct AutomationForm {
    name: String,
    source: String,
    enabled: Option<String>,
}

async fn save(
    state: &AppState,
    user: &CurrentUser,
    id: Option<i64>,
    form: AutomationForm,
) -> AppResult<Response> {
    require_admin(user)?;
    let name: String = form.name.trim().chars().take(80).collect();
    let enabled = form.enabled.is_some();
    if name.is_empty() || form.source.len() > MAX_SOURCE_BYTES {
        let editor = views::automations::Editor {
            id,
            name: &name,
            source: &form.source,
            enabled,
            form_error: Some("Give the automation a name, and keep the script under 100 kB."),
            ai_ready: ai_ready(state).await?,
            ..views::automations::Editor::default()
        };
        let page = render_editor(state, user, &editor).await?;
        return Ok((StatusCode::BAD_REQUEST, page).into_response());
    }
    let user_id = user.id;
    let now = now_ms();
    let source = form.source;
    let saved = state
        .db
        .call(move |conn| {
            if let Some(id) = id {
                store::automation(conn, id)?.ok_or(AppError::NotFound)?;
            }
            store::save_automation(
                conn,
                id,
                &AutomationEdit {
                    name: &name,
                    source: &source,
                    enabled,
                    user_id,
                    saved_with: "editor",
                },
                now,
            )
        })
        .await?;
    state.automations.reload(state).await?;
    Ok(Redirect::to(&format!("/automations/{saved}")).into_response())
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

async fn restore(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((automation_id, version_id)): Path<(i64, i64)>,
) -> AppResult<Response> {
    require_admin(&user)?;
    let user_id = user.id;
    let now = now_ms();
    state
        .db
        .call(move |conn| {
            let automation = store::automation(conn, automation_id)?.ok_or(AppError::NotFound)?;
            let version = store::automation_versions(conn, automation_id)?
                .into_iter()
                .find(|version| version.id == version_id)
                .ok_or(AppError::NotFound)?;
            store::save_automation(
                conn,
                Some(automation_id),
                &AutomationEdit {
                    name: &automation.name,
                    source: &version.source,
                    enabled: automation.enabled,
                    user_id,
                    saved_with: "restore",
                },
                now,
            )
        })
        .await?;
    state.automations.reload(&state).await?;
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
        )
        .await?,
    ))
}

/// Dry-runs `source`, with the saved data of `automation_id` if given.
pub async fn run_test(
    state: &AppState,
    automation_id: Option<i64>,
    name: String,
    source: String,
    trigger: TestTrigger,
) -> AppResult<automations::TestReport> {
    let data = match automation_id {
        Some(id) => {
            state
                .db
                .call(move |conn| store::automation_values(conn, id))
                .await?
        }
        None => std::collections::HashMap::new(),
    };
    let name = if name.trim().is_empty() {
        "Automation".to_owned()
    } else {
        name
    };
    tokio::task::spawn_blocking(move || automations::test(&name, &source, data, &trigger))
        .await
        .map_err(AppError::internal)
}

async fn ai_ready(state: &AppState) -> AppResult<bool> {
    state
        .db
        .call(|conn| Ok(ai::provider(conn)?.is_some()))
        .await
}

#[derive(Deserialize)]
struct AiInput {
    prompt: String,
    #[serde(default)]
    source: String,
    #[serde(default)]
    name: String,
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
    let (provider, channels) = state
        .db
        .call(|conn| Ok((ai::provider(conn)?, store::public_channel_names(conn)?)))
        .await?;
    let provider =
        provider.ok_or_else(|| AppError::bad_request("No AI provider is connected yet."))?;
    let request = ai::ScriptRequest {
        prompt: prompt.to_owned(),
        name: input.name,
        source: input.source,
        channels,
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
