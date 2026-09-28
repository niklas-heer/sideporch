//! Admin settings: the AI provider, MCP API tokens, secrets, and
//! automation settings.

use axum::{
    Form, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use maud::Markup;
use serde::Deserialize;

use super::{base_url, shell_data};
use crate::{
    AppState,
    ai::{self, Protocol, Provider},
    auth::{self, CurrentUser},
    automations,
    error::{AppError, AppResult},
    mcp, now_ms, secrets, store,
    views::{self, Shell},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/settings/ai", get(ai_page).post(save_ai))
        .route("/settings/ai/remove", post(remove_ai))
        .route("/settings/mcp", get(mcp_page))
        .route("/settings/mcp/tokens", post(create_token))
        .route("/settings/mcp/tokens/{token_id}/delete", post(delete_token))
        .route("/settings/secrets", get(secrets_page).post(save_secret))
        .route("/settings/secrets/{name}/delete", post(delete_secret))
        .route(
            "/settings/automations",
            get(automation_settings).post(save_automation_settings),
        )
        .route("/mcp", post(mcp::endpoint))
}

async fn render_secrets(
    state: &AppState,
    user: &CurrentUser,
    error: Option<&str>,
    saved: Option<&str>,
) -> AppResult<Markup> {
    let (sidebar, (list, automations)) = tokio::try_join!(
        shell_data(state, user.id),
        state
            .db
            .call(|conn| Ok((secrets::list(conn)?, store::automations(conn)?))),
    )?;
    let shell = Shell {
        user,
        sidebar: &sidebar,
        current: None,
    };
    // Which automations mention each secret, as a hint for cleaning up.
    let listed: Vec<(secrets::SecretInfo, Vec<String>)> = list
        .into_iter()
        .map(|secret| {
            let quoted = format!("\"{}\"", secret.name);
            let users = automations
                .iter()
                .filter(|automation| automation.source.contains(&quoted))
                .map(|automation| automation.name.clone())
                .collect();
            (secret, users)
        })
        .collect();
    Ok(views::settings::secrets_page(&shell, &listed, error, saved))
}

async fn secrets_page(user: CurrentUser, State(state): State<AppState>) -> AppResult<Markup> {
    require_admin(&user)?;
    render_secrets(&state, &user, None, None).await
}

#[derive(Deserialize)]
struct SecretForm {
    name: String,
    value: String,
}

async fn save_secret(
    user: CurrentUser,
    State(state): State<AppState>,
    Form(form): Form<SecretForm>,
) -> AppResult<Response> {
    require_admin(&user)?;
    let name = form.name.trim().to_owned();
    let vault = std::sync::Arc::clone(&state.vault);
    let user_id = user.id;
    let now = now_ms();
    let stored = name.clone();
    let result = state
        .db
        .call(
            move |conn| match secrets::set(conn, &vault, &stored, &form.value, user_id, now) {
                Ok(()) => Ok(Ok(())),
                Err(AppError::BadRequest(message)) => Ok(Err(message)),
                Err(other) => Err(other),
            },
        )
        .await?;
    match result {
        Ok(()) => {
            state.automations.reload(&state).await?;
            Ok(render_secrets(&state, &user, None, Some(&name))
                .await?
                .into_response())
        }
        Err(error) => Ok((
            StatusCode::BAD_REQUEST,
            render_secrets(&state, &user, Some(&error), None).await?,
        )
            .into_response()),
    }
}

async fn delete_secret(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> AppResult<Response> {
    require_admin(&user)?;
    state
        .db
        .call(move |conn| secrets::delete(conn, &name))
        .await?;
    state.automations.reload(&state).await?;
    Ok(Redirect::to("/settings/secrets").into_response())
}

async fn render_automation_settings(
    state: &AppState,
    user: &CurrentUser,
    error: Option<&str>,
    saved: bool,
) -> AppResult<Markup> {
    let (sidebar, settings) = tokio::try_join!(
        shell_data(state, user.id),
        state.db.call(|conn| automations::Settings::load(conn)),
    )?;
    let shell = Shell {
        user,
        sidebar: &sidebar,
        current: None,
    };
    Ok(views::settings::automation_settings_page(
        &shell, &settings, error, saved,
    ))
}

async fn automation_settings(
    user: CurrentUser,
    State(state): State<AppState>,
) -> AppResult<Markup> {
    require_admin(&user)?;
    render_automation_settings(&state, &user, None, false).await
}

#[derive(Deserialize)]
struct AutomationSettingsForm {
    timezone: String,
    allow_private_network: Option<String>,
}

async fn save_automation_settings(
    user: CurrentUser,
    State(state): State<AppState>,
    Form(form): Form<AutomationSettingsForm>,
) -> AppResult<Response> {
    require_admin(&user)?;
    let settings = automations::Settings {
        timezone: form.timezone.trim().to_owned(),
        allow_private_network: form.allow_private_network.is_some(),
    };
    let result = state
        .db
        .call(move |conn| match settings.save(conn) {
            Ok(()) => Ok(Ok(())),
            Err(AppError::BadRequest(message)) => Ok(Err(message)),
            Err(other) => Err(other),
        })
        .await?;
    match result {
        Ok(()) => {
            state.automations.reload(&state).await?;
            Ok(render_automation_settings(&state, &user, None, true)
                .await?
                .into_response())
        }
        Err(error) => Ok((
            StatusCode::BAD_REQUEST,
            render_automation_settings(&state, &user, Some(&error), false).await?,
        )
            .into_response()),
    }
}

const fn require_admin(user: &CurrentUser) -> AppResult<()> {
    if user.is_admin {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

async fn render_ai(
    state: &AppState,
    user: &CurrentUser,
    error: Option<&str>,
    saved: bool,
) -> AppResult<Markup> {
    let (sidebar, provider) = tokio::try_join!(
        shell_data(state, user.id),
        state.db.call({
            let vault = std::sync::Arc::clone(&state.vault);
            move |conn| ai::provider(conn, &vault)
        }),
    )?;
    let shell = Shell {
        user,
        sidebar: &sidebar,
        current: None,
    };
    Ok(views::settings::ai_page(
        &shell,
        provider.as_ref(),
        error,
        saved,
    ))
}

async fn ai_page(user: CurrentUser, State(state): State<AppState>) -> AppResult<Markup> {
    require_admin(&user)?;
    render_ai(&state, &user, None, false).await
}

#[derive(Deserialize)]
struct AiForm {
    protocol: String,
    #[serde(default)]
    base_url: String,
    model: String,
    #[serde(default)]
    api_key: String,
}

async fn save_ai(
    user: CurrentUser,
    State(state): State<AppState>,
    Form(form): Form<AiForm>,
) -> AppResult<Response> {
    require_admin(&user)?;
    let protocol = Protocol::from_key(&form.protocol);
    let base_url = form.base_url.trim().trim_end_matches('/').to_owned();
    let model = form.model.trim().to_owned();
    let problem = match protocol {
        None => Some("Pick a kind of API."),
        Some(_) if model.is_empty() => Some("Name the model to use."),
        Some(_)
            if !base_url.is_empty()
                && !base_url.starts_with("https://")
                && !base_url.starts_with("http://") =>
        {
            Some("The URL must start with https:// or http://.")
        }
        Some(_) => None,
    };
    let Some(protocol) = protocol.filter(|_| problem.is_none()) else {
        return Ok(render_ai(&state, &user, problem, false)
            .await?
            .into_response());
    };
    let new_key = form.api_key.trim().to_owned();
    let vault = std::sync::Arc::clone(&state.vault);
    state
        .db
        .call(move |conn| {
            // An empty key field keeps the saved key.
            let api_key = if new_key.is_empty() {
                ai::provider(conn, &vault)?
                    .map(|provider| provider.api_key)
                    .unwrap_or_default()
            } else {
                new_key
            };
            ai::save_provider(
                conn,
                &vault,
                &Provider {
                    protocol,
                    base_url: if base_url.is_empty() {
                        protocol.default_base_url().to_owned()
                    } else {
                        base_url
                    },
                    model,
                    api_key,
                },
            )
        })
        .await?;
    Ok(render_ai(&state, &user, None, true).await?.into_response())
}

async fn remove_ai(user: CurrentUser, State(state): State<AppState>) -> AppResult<Response> {
    require_admin(&user)?;
    state.db.call(|conn| ai::remove_provider(conn)).await?;
    Ok(Redirect::to("/settings/ai").into_response())
}

async fn render_mcp(
    state: &AppState,
    user: &CurrentUser,
    headers: &HeaderMap,
    new_token: Option<&str>,
) -> AppResult<Markup> {
    let (sidebar, tokens) = tokio::try_join!(
        shell_data(state, user.id),
        state.db.call(|conn| store::api_tokens(conn)),
    )?;
    let shell = Shell {
        user,
        sidebar: &sidebar,
        current: None,
    };
    let endpoint = format!("{}/mcp", base_url(state, headers));
    Ok(views::settings::mcp_page(
        &shell, &endpoint, &tokens, new_token,
    ))
}

async fn mcp_page(
    user: CurrentUser,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<Markup> {
    require_admin(&user)?;
    render_mcp(&state, &user, &headers, None).await
}

#[derive(Deserialize)]
struct TokenForm {
    name: String,
}

async fn create_token(
    user: CurrentUser,
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<TokenForm>,
) -> AppResult<Markup> {
    require_admin(&user)?;
    let name: String = form.name.trim().chars().take(60).collect();
    let name = if name.is_empty() {
        "AI agent".to_owned()
    } else {
        name
    };
    let token = format!("sp_{}", auth::random_token()?);
    let token_hash = auth::hash_token(&token);
    let user_id = user.id;
    let now = now_ms();
    state
        .db
        .call(move |conn| store::create_api_token(conn, user_id, &name, &token_hash, now))
        .await?;
    render_mcp(&state, &user, &headers, Some(&token)).await
}

async fn delete_token(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(token_id): Path<i64>,
) -> AppResult<Response> {
    require_admin(&user)?;
    state
        .db
        .call(move |conn| store::delete_api_token(conn, token_id))
        .await?;
    Ok(Redirect::to("/settings/mcp").into_response())
}
