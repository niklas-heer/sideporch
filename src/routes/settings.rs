//! Admin settings for the AI provider and MCP API tokens.

use axum::{
    Form, Router,
    extract::{Path, State},
    http::HeaderMap,
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
    error::{AppError, AppResult},
    mcp, now_ms, store,
    views::{self, Shell},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/settings/ai", get(ai_page).post(save_ai))
        .route("/settings/ai/remove", post(remove_ai))
        .route("/settings/mcp", get(mcp_page))
        .route("/settings/mcp/tokens", post(create_token))
        .route("/settings/mcp/tokens/{token_id}/delete", post(delete_token))
        .route("/mcp", post(mcp::endpoint))
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
        state.db.call(|conn| ai::provider(conn)),
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
    state
        .db
        .call(move |conn| {
            // An empty key field keeps the saved key.
            let api_key = if new_key.is_empty() {
                ai::provider(conn)?
                    .map(|provider| provider.api_key)
                    .unwrap_or_default()
            } else {
                new_key
            };
            ai::save_provider(
                conn,
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
