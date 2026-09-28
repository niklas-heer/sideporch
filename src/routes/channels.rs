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
    Ok(Redirect::to(&format!("/c/{channel_id}/settings")))
}
