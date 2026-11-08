//! Admin → Settings → Bots: making bots and their tokens, and revoking them.
//! A token is shown once, when it's made, and only its hash is stored.

use axum::{
    Form, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use maud::Markup;
use rusqlite::TransactionBehavior;
use serde::Deserialize;

use super::shell_data;
use crate::{
    AppState,
    auth::{self, CurrentUser},
    bots::{self, ChannelScope, NewBot, NewToken},
    community::Permission,
    error::{AppError, AppResult},
    now_ms,
    views::{
        Shell,
        settings::{Issued, bots_page as bots_view},
    },
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/settings/bots", get(bots_page).post(create_bot))
        .route("/settings/bots/{bot_id}/revoke", post(revoke_bot))
        .route("/settings/bots/{bot_id}/tokens", post(create_token))
        .route(
            "/settings/bots/tokens/{token_id}/revoke",
            post(revoke_token),
        )
}

#[derive(Deserialize)]
struct BotForm {
    name: String,
    display_name: String,
    #[serde(default)]
    icon: String,
}

/// A form's fields, in order; repeated fields such as checkboxes appear
/// once per value.
type Fields = Vec<(String, String)>;

fn values<'a>(fields: &'a Fields, key: &str) -> Vec<&'a str> {
    fields
        .iter()
        .filter(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
        .collect()
}

fn first<'a>(fields: &'a Fields, key: &str) -> &'a str {
    values(fields, key).first().copied().unwrap_or_default()
}

async fn bots_page(
    user: CurrentUser,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<Markup> {
    user.require(Permission::ManageBots)?;
    render(&state, &user, &headers, None, None).await
}

async fn create_bot(
    user: CurrentUser,
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<BotForm>,
) -> AppResult<Response> {
    user.require(Permission::ManageBots)?;
    let outcome: AppResult<Option<Issued>> = async {
        let name = bots::check_name(&form.name)?;
        let display_name = bots::check_display_name(&form.display_name)?;
        let icon = bots::check_icon(&form.icon)?;
        let new = NewBot {
            name,
            display_name,
            icon,
            created_by: user.id,
            now: now_ms(),
        };
        let session = auth::request_session(&headers);
        let user_id = user.id;
        state
            .db
            .call(move |conn| {
                let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                // A bot is a way in too: check the session again where it's made.
                if !auth::still_signed_in(&tx, session.as_deref(), user_id)? {
                    return Err(AppError::Forbidden);
                }
                bots::create_bot(&tx, &new)?;
                tx.commit()?;
                Ok(())
            })
            .await?;
        Ok(None)
    }
    .await;
    settle(&state, &user, &headers, outcome).await
}

async fn revoke_bot(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(bot_id): Path<i64>,
) -> AppResult<Response> {
    user.require(Permission::ManageBots)?;
    let now = now_ms();
    state
        .db
        .call(move |conn| bots::revoke_bot(conn, bot_id, now))
        .await?;
    Ok(Redirect::to("/settings/bots").into_response())
}

async fn create_token(
    user: CurrentUser,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(bot_id): Path<i64>,
    Form(fields): Form<Fields>,
) -> AppResult<Response> {
    user.require(Permission::ManageBots)?;
    let outcome: AppResult<Option<Issued>> = async {
        let label: String = first(&fields, "label").trim().chars().take(60).collect();
        let label = if label.is_empty() {
            "Bot token".to_owned()
        } else {
            label
        };
        let asked = values(&fields, "scope");
        let scopes = bots::check_scopes(&asked)?;
        let channels = if first(&fields, "channel_scope") == "list" {
            let ids = values(&fields, "channel")
                .into_iter()
                .map(|id| {
                    id.parse::<i64>()
                        .map_err(|_| AppError::bad_request("Pick channels from the list."))
                })
                .collect::<AppResult<Vec<i64>>>()?;
            ChannelScope::Only(ids)
        } else {
            ChannelScope::All
        };
        let now = now_ms();
        let expires_at = bots::check_expiry(first(&fields, "expires"), now)?;
        let token = format!("spb_{}", auth::random_token()?);
        let new = NewToken {
            label,
            scopes,
            channels,
            token_hash: auth::hash_token(&token),
            expires_at,
            now,
        };
        let session = auth::request_session(&headers);
        let user_id = user.id;
        let bot_name = state
            .db
            .call(move |conn| {
                let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                if !auth::still_signed_in(&tx, session.as_deref(), user_id)? {
                    return Err(AppError::Forbidden);
                }
                bots::create_token(&tx, bot_id, &new)?;
                let name: String = tx.query_row(
                    "SELECT display_name FROM bots WHERE id = ?1",
                    [bot_id],
                    |row| row.get(0),
                )?;
                tx.commit()?;
                Ok(name)
            })
            .await?;
        Ok(Some(Issued {
            bot: bot_name,
            token,
        }))
    }
    .await;
    settle(&state, &user, &headers, outcome).await
}

async fn revoke_token(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(token_id): Path<i64>,
) -> AppResult<Response> {
    user.require(Permission::ManageBots)?;
    let now = now_ms();
    state
        .db
        .call(move |conn| bots::revoke_token(conn, token_id, now))
        .await?;
    Ok(Redirect::to("/settings/bots").into_response())
}

/// Shows what a form did: the page with a new token, a redirect when it
/// went through, or the page again with the refusal.
async fn settle(
    state: &AppState,
    user: &CurrentUser,
    headers: &HeaderMap,
    outcome: AppResult<Option<Issued>>,
) -> AppResult<Response> {
    match outcome {
        Ok(Some(issued)) => Ok(render(state, user, headers, None, Some(&issued))
            .await?
            .into_response()),
        Ok(None) => Ok(Redirect::to("/settings/bots").into_response()),
        Err(AppError::BadRequest(message)) => Ok((
            StatusCode::BAD_REQUEST,
            render(state, user, headers, Some(&message), None).await?,
        )
            .into_response()),
        Err(other) => Err(other),
    }
}

async fn render(
    state: &AppState,
    user: &CurrentUser,
    headers: &HeaderMap,
    error: Option<&str>,
    issued: Option<&Issued>,
) -> AppResult<Markup> {
    let (sidebar, bots, channels) = tokio::try_join!(
        shell_data(state, user.id),
        state.db.call(|conn| bots::list(conn)),
        state.db.call(|conn| bots::public_channels(conn)),
    )?;
    let shell = Shell {
        user,
        sidebar: &sidebar,
        current: None,
    };
    let endpoint = format!("{}/api/v1", super::base_url(state, headers));
    Ok(bots_view(
        &shell, &endpoint, &bots, &channels, error, issued,
    ))
}
