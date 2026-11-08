//! Catching up after a dropped connection, without reloading the page: the
//! messages a channel or thread gained, the current state of the messages
//! on screen (edits, deletions, reactions and reply counts), and the
//! sidebar's unread marks.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{get, post},
};
use maud::Markup;
use serde::{Deserialize, Serialize};

use crate::{
    AppState,
    auth::CurrentUser,
    error::{AppError, AppResult},
    store,
    views::{self, Render, Shell},
};

/// Most messages one catch-up returns. Past it the page reloads instead.
const CATCH_UP_LIMIT: u32 = 100;
/// Most loaded messages one refresh checks.
const REFRESH_LIMIT: usize = 500;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/c/{channel_id}/since", get(since))
        .route("/c/{channel_id}/messages/refresh", post(refresh))
        .route("/sidebar", get(sidebar))
}

/// A message as the page shows it, with the id the page keys it by.
#[derive(Serialize)]
pub struct Rendered {
    id: i64,
    parent_id: Option<i64>,
    html: String,
}

#[derive(Deserialize)]
pub struct SinceQuery {
    /// The newest message the page shows; only later ones are returned.
    after: i64,
    /// Thread replies instead of the channel's messages.
    thread: Option<i64>,
    limit: Option<u32>,
}

#[derive(Serialize)]
pub struct Caught {
    messages: Vec<Rendered>,
    /// `false` when more came after the last message returned, so the
    /// page must reload to show everything.
    complete: bool,
}

/// The messages in a channel's list, or in a thread, after `after`. Needs
/// the same access as viewing the channel.
async fn since(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(channel_id): Path<i64>,
    Query(query): Query<SinceQuery>,
) -> AppResult<Json<Caught>> {
    let user_id = user.id;
    let limit = query
        .limit
        .unwrap_or(CATCH_UP_LIMIT)
        .clamp(1, CATCH_UP_LIMIT);
    let cap = usize::try_from(limit).unwrap_or(usize::MAX);
    let after = query.after.max(0);
    let thread = query.thread;
    let caught = state
        .db
        .call(move |conn| {
            store::channel_for(conn, channel_id, user_id)?.ok_or(AppError::NotFound)?;
            if let Some(root) = thread {
                store::message(conn, root)?
                    .filter(|root| root.channel_id == channel_id && root.parent_id.is_none())
                    .ok_or(AppError::NotFound)?;
            }
            // One more than the cap, to tell whether anything was left out.
            let mut found =
                store::messages_after(conn, channel_id, thread, after, limit.saturating_add(1))?;
            let complete = found.len() <= cap;
            found.truncate(cap);
            let ctx = store::render_context(conn)?;
            let messages = found
                .iter()
                .map(|message| Rendered {
                    id: message.id,
                    parent_id: message.parent_id,
                    html: views::message_item(
                        message,
                        false,
                        message.parent_id.is_none(),
                        &Render::shared(&ctx),
                    )
                    .into_string(),
                })
                .collect();
            Ok(Caught { messages, complete })
        })
        .await?;
    Ok(Json(caught))
}

#[derive(Deserialize)]
pub struct RefreshInput {
    ids: Vec<i64>,
}

#[derive(Serialize)]
pub struct Refreshed {
    /// The current markup of each message that still exists.
    messages: Vec<Rendered>,
    /// Messages that were deleted or moved, which the page removes.
    gone: Vec<i64>,
}

/// Current markup for messages the page already shows, so edits, deletions,
/// reactions and reply counts that happened while disconnected appear.
async fn refresh(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(channel_id): Path<i64>,
    Json(input): Json<RefreshInput>,
) -> AppResult<Json<Refreshed>> {
    if input.ids.len() > REFRESH_LIMIT {
        return Err(AppError::bad_request(
            "The page shows too many messages to refresh at once.",
        ));
    }
    let user_id = user.id;
    let refreshed = state
        .db
        .call(move |conn| {
            store::channel_for(conn, channel_id, user_id)?.ok_or(AppError::NotFound)?;
            let found = store::messages_in(conn, channel_id, &input.ids)?;
            let gone = input
                .ids
                .iter()
                .copied()
                .filter(|id| !found.iter().any(|message| message.id == *id))
                .collect();
            let ctx = store::render_context(conn)?;
            let messages = found
                .iter()
                .map(|message| Rendered {
                    id: message.id,
                    parent_id: message.parent_id,
                    html: views::message_item(
                        message,
                        false,
                        message.parent_id.is_none(),
                        &Render::shared(&ctx),
                    )
                    .into_string(),
                })
                .collect();
            Ok(Refreshed { messages, gone })
        })
        .await?;
    Ok(Json(refreshed))
}

#[derive(Deserialize)]
pub struct SidebarQuery {
    /// The channel the page shows, which is never marked unread.
    channel: Option<i64>,
}

/// The channel list as a page would render it now, for patching unread marks.
async fn sidebar(
    user: CurrentUser,
    State(state): State<AppState>,
    Query(query): Query<SidebarQuery>,
) -> AppResult<Markup> {
    let user_id = user.id;
    let updates = Arc::clone(&state.updates);
    let sidebar = state
        .db
        .call(move |conn| super::sidebar_with_update(conn, &updates, user_id))
        .await?;
    Ok(views::sidebar_fragment(&Shell {
        user: &user,
        sidebar: &sidebar,
        current: query.channel,
    }))
}
