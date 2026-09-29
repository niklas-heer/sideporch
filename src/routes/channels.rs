//! Finding, joining, leaving and muting channels, and who is in a private
//! one.

use axum::{
    Form, Router,
    extract::{Path, State},
    http::HeaderMap,
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use maud::Markup;
use serde::Deserialize;
use serde_json::json;

use super::{managed_channel, shell_data, wants_no_content};
use crate::{
    AppState,
    auth::CurrentUser,
    error::{AppError, AppResult},
    store::{self, ChannelKind},
    views::{self, Shell},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/channels/browse", get(directory))
        .route("/c/{channel_id}/join", post(join))
        .route("/c/{channel_id}/leave", post(leave))
        .route("/c/{channel_id}/mute", post(mute))
        .route("/c/{channel_id}/members", post(add_member))
        .route(
            "/c/{channel_id}/members/{user_id}/remove",
            post(remove_member),
        )
        .route("/c/{channel_id}/permissions", post(set_permissions))
        .route("/c/{channel_id}/managers", post(add_manager))
        .route(
            "/c/{channel_id}/managers/{user_id}/remove",
            post(remove_manager),
        )
        .route("/c/{channel_id}/outgoing", post(add_outgoing))
        .route(
            "/c/{channel_id}/outgoing/{hook_id}/delete",
            post(delete_outgoing),
        )
}

/// A channel whose rules the user may change: they manage it.
async fn managed_by_user(
    state: &AppState,
    user: &CurrentUser,
    channel_id: i64,
) -> AppResult<store::Channel> {
    let channel = managed_channel(state, user, channel_id).await?;
    if channel.manager {
        Ok(channel)
    } else {
        Err(AppError::Forbidden)
    }
}

#[derive(Deserialize)]
struct PermissionsForm {
    post: String,
    reply: String,
    react: String,
}

async fn set_permissions(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(channel_id): Path<i64>,
    Form(form): Form<PermissionsForm>,
) -> AppResult<Redirect> {
    managed_by_user(&state, &user, channel_id).await?;
    let parse = |value: &str| {
        store::Policy::parse(value)
            .ok_or_else(|| AppError::bad_request("Pick everyone or managers."))
    };
    let posting = store::Posting {
        post: parse(&form.post)?,
        reply: parse(&form.reply)?,
        react: parse(&form.react)?,
    };
    state
        .db
        .call(move |conn| store::set_posting(conn, channel_id, posting))
        .await?;
    Ok(Redirect::to(&format!("/c/{channel_id}/settings")))
}

async fn add_manager(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(channel_id): Path<i64>,
    Form(form): Form<MemberForm>,
) -> AppResult<Redirect> {
    let channel = managed_by_user(&state, &user, channel_id).await?;
    let manager = form.user_id;
    state
        .db
        .call(move |conn| {
            let person = store::user(conn, manager)?.ok_or(AppError::NotFound)?;
            if person.deactivated {
                return Err(AppError::bad_request("That account is deactivated."));
            }
            // Managers of a private channel are among its members.
            if channel.kind == ChannelKind::Private {
                store::add_member(conn, channel_id, manager)?;
            }
            store::set_manager(conn, channel_id, manager, true)
        })
        .await?;
    Ok(Redirect::to(&format!("/c/{channel_id}/settings")))
}

async fn remove_manager(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((channel_id, manager)): Path<(i64, i64)>,
) -> AppResult<Redirect> {
    managed_by_user(&state, &user, channel_id).await?;
    state
        .db
        .call(move |conn| store::set_manager(conn, channel_id, manager, false))
        .await?;
    Ok(Redirect::to(&format!("/c/{channel_id}/settings")))
}

#[derive(Deserialize)]
struct OutgoingForm {
    name: String,
    url: String,
    #[serde(default)]
    triggers: String,
}

async fn add_outgoing(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(channel_id): Path<i64>,
    Form(form): Form<OutgoingForm>,
) -> AppResult<Redirect> {
    managed_channel(&state, &user, channel_id).await?;
    let name: String = form.name.trim().chars().take(80).collect();
    let url = form.url.trim().to_owned();
    if name.is_empty() {
        return Err(AppError::bad_request("Give the webhook a name."));
    }
    if !(url.starts_with("https://") || url.starts_with("http://")) || url.len() > 2_000 {
        return Err(AppError::bad_request("Enter an http or https URL."));
    }
    let triggers: Vec<String> = form
        .triggers
        .split([',', ' '])
        .map(str::trim)
        .filter(|word| !word.is_empty())
        .map(|word| word.chars().take(40).collect())
        .take(10)
        .collect();
    let token = crate::auth::random_token()?;
    let user_id = user.id;
    let now = crate::now_ms();
    state
        .db
        .call(move |conn| {
            store::create_outgoing_webhook(
                conn,
                &store::NewOutgoingWebhook {
                    channel_id,
                    name: &name,
                    url: &url,
                    triggers: &triggers,
                    token: &token,
                },
                user_id,
                now,
            )
        })
        .await?;
    Ok(Redirect::to(&format!("/c/{channel_id}/settings")))
}

async fn delete_outgoing(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((channel_id, hook_id)): Path<(i64, i64)>,
) -> AppResult<Redirect> {
    managed_channel(&state, &user, channel_id).await?;
    state
        .db
        .call(move |conn| store::delete_outgoing_webhook(conn, channel_id, hook_id))
        .await?;
    Ok(Redirect::to(&format!("/c/{channel_id}/settings")))
}

async fn directory(user: CurrentUser, State(state): State<AppState>) -> AppResult<Markup> {
    let user_id = user.id;
    let entries = state
        .db
        .call(move |conn| store::channel_directory(conn, user_id))
        .await?;
    let sidebar = shell_data(&state, user.id).await?;
    let shell = Shell {
        user: &user,
        sidebar: &sidebar,
        current: None,
    };
    Ok(views::channels::directory_page(&shell, &entries))
}

async fn join(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(channel_id): Path<i64>,
) -> AppResult<Redirect> {
    let channel = managed_channel(&state, &user, channel_id).await?;
    if channel.kind == ChannelKind::Public {
        let user_id = user.id;
        state
            .db
            .call(move |conn| store::set_channel_hidden(conn, user_id, channel_id, false))
            .await?;
    }
    Ok(Redirect::to(&format!("/c/{channel_id}")))
}

/// Leaves a channel. A public channel only leaves the sidebar and can be
/// joined again; leaving a private one ends the membership.
async fn leave(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(channel_id): Path<i64>,
) -> AppResult<Redirect> {
    let channel = managed_channel(&state, &user, channel_id).await?;
    let user_id = user.id;
    state
        .db
        .call(move |conn| {
            if channel.kind == ChannelKind::Private {
                if store::members(conn, channel_id)?.len() <= 1 {
                    return Err(AppError::bad_request(
                        "You're the last member. Add someone else before you leave, or the channel is lost to everyone.",
                    ));
                }
                store::remove_member(conn, channel_id, user_id)
            } else {
                store::set_channel_hidden(conn, user_id, channel_id, true)
            }
        })
        .await?;
    if channel.kind == ChannelKind::Private {
        crate::federation::outbound::membership(&state, channel_id, user_id, false, None).await;
    }
    Ok(Redirect::to("/channels/browse"))
}

async fn mute(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(channel_id): Path<i64>,
    headers: HeaderMap,
) -> AppResult<Response> {
    let channel = managed_channel(&state, &user, channel_id).await?;
    let user_id = user.id;
    let muted = !channel.muted;
    state
        .db
        .call(move |conn| store::set_channel_muted(conn, user_id, channel_id, muted))
        .await?;
    if wants_no_content(&headers) {
        return Ok(axum::Json(json!({ "muted": muted })).into_response());
    }
    Ok(Redirect::to(&format!("/c/{channel_id}")).into_response())
}

#[derive(Deserialize)]
struct MemberForm {
    user_id: i64,
}

async fn private_channel(
    state: &AppState,
    user: &CurrentUser,
    channel_id: i64,
) -> AppResult<store::Channel> {
    let channel = managed_channel(state, user, channel_id).await?;
    if channel.kind == ChannelKind::Private {
        Ok(channel)
    } else {
        Err(AppError::NotFound)
    }
}

/// Any member of a private channel can add people.
async fn add_member(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(channel_id): Path<i64>,
    Form(form): Form<MemberForm>,
) -> AppResult<Redirect> {
    private_channel(&state, &user, channel_id).await?;
    let member = form.user_id;
    state
        .db
        .call(move |conn| {
            let person = store::user(conn, member)?.ok_or(AppError::NotFound)?;
            if person.deactivated {
                return Err(AppError::bad_request("That account is deactivated."));
            }
            store::add_member(conn, channel_id, member)
        })
        .await?;
    crate::federation::outbound::membership(&state, channel_id, member, true, None).await;
    Ok(Redirect::to(&format!("/c/{channel_id}/settings")))
}

/// Whoever created a private channel and admins can remove people.
async fn remove_member(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((channel_id, member)): Path<(i64, i64)>,
) -> AppResult<Redirect> {
    let channel = private_channel(&state, &user, channel_id).await?;
    if member == user.id {
        return Err(AppError::bad_request("Use Leave channel to leave it."));
    }
    if !user.is_admin && channel.created_by != Some(user.id) {
        return Err(AppError::Forbidden);
    }
    state
        .db
        .call(move |conn| store::remove_member(conn, channel_id, member))
        .await?;
    crate::federation::outbound::membership(&state, channel_id, member, false, None).await;
    Ok(Redirect::to(&format!("/c/{channel_id}/settings")))
}
