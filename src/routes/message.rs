//! What people do to a single message: edit, delete, pin and save it, plus
//! permalinks and the lists of pinned and saved messages.

use axum::{
    Form, Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use maud::Markup;
use serde::Deserialize;
use serde_json::json;

use super::{MAX_MESSAGE_CHARS, wants_no_content};
use crate::{
    AppState,
    auth::CurrentUser,
    error::{AppError, AppResult},
    messages::{self, Change},
    now_ms, store,
    views::{self, Render, Shell},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/c/{channel_id}/m/{message_id}", get(permalink))
        .route("/c/{channel_id}/m/{message_id}/actions", get(actions))
        .route("/c/{channel_id}/m/{message_id}/source", get(source))
        .route("/c/{channel_id}/m/{message_id}/edit", post(edit))
        .route("/c/{channel_id}/m/{message_id}/delete", post(delete))
        .route("/c/{channel_id}/m/{message_id}/pin", post(pin))
        .route("/c/{channel_id}/m/{message_id}/save", post(save))
        .route(
            "/c/{channel_id}/m/{message_id}/preview/remove",
            post(remove_preview),
        )
        .route("/c/{channel_id}/pins", get(pins))
        .route("/saved", get(saved))
        .route("/activity", get(activity))
}

/// Where a message is shown: its thread, or the page of channel history
/// that ends with it.
pub fn message_href(message: &store::Message) -> String {
    message.parent_id.map_or_else(
        || {
            format!(
                "/c/{}?before={}#m{}",
                message.channel_id,
                message.id.saturating_add(1),
                message.id
            )
        },
        |parent| format!("/c/{}/t/{parent}#m{}", message.channel_id, message.id),
    )
}

async fn readable(
    state: &AppState,
    user: &CurrentUser,
    channel_id: i64,
    message_id: i64,
) -> AppResult<store::Message> {
    let user_id = user.id;
    state
        .db
        .call(move |conn| store::readable_message(conn, user_id, channel_id, message_id))
        .await?
        .ok_or(AppError::NotFound)
}

async fn permalink(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((channel_id, message_id)): Path<(i64, i64)>,
) -> AppResult<Redirect> {
    let message = readable(&state, &user, channel_id, message_id).await?;
    Ok(Redirect::to(&message_href(&message)))
}

/// The message's text as written, for editing in place.
async fn source(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((channel_id, message_id)): Path<(i64, i64)>,
) -> AppResult<Json<serde_json::Value>> {
    let message = readable(&state, &user, channel_id, message_id).await?;
    Ok(Json(json!({ "body": message.body })))
}

/// The actions for one message as a page, for browsers without JavaScript.
async fn actions(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((channel_id, message_id)): Path<(i64, i64)>,
) -> AppResult<Markup> {
    let user_id = user.id;
    let (message, sidebar, ctx, saved) = state
        .db
        .call(move |conn| {
            let message = store::readable_message(conn, user_id, channel_id, message_id)?
                .ok_or(AppError::NotFound)?;
            let saved = store::saved_ids(conn, user_id, &[&message])?;
            Ok((
                message,
                store::sidebar(conn, user_id)?,
                store::render_context(conn)?,
                saved,
            ))
        })
        .await?;
    let shell = Shell {
        user: &user,
        sidebar: &sidebar,
        current: Some(channel_id),
    };
    let render = Render::for_user(&ctx, user.id).with_saved(&saved);
    Ok(views::messages::actions_page(&shell, &message, &render))
}

/// Answers a background request with `body`, or a form with a redirect to
/// the message.
fn done(headers: &HeaderMap, target: &str, body: serde_json::Value) -> Response {
    if wants_no_content(headers) {
        if body.is_null() {
            return StatusCode::NO_CONTENT.into_response();
        }
        return Json(body).into_response();
    }
    Redirect::to(target).into_response()
}

#[derive(Deserialize)]
struct EditForm {
    body: String,
}

async fn edit(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((channel_id, message_id)): Path<(i64, i64)>,
    headers: HeaderMap,
    Form(form): Form<EditForm>,
) -> AppResult<Response> {
    let body = form.body.trim().to_owned();
    if body.is_empty() {
        return Err(AppError::bad_request(
            "A message needs some text. Delete it instead.",
        ));
    }
    if body.chars().count() > MAX_MESSAGE_CHARS {
        return Err(AppError::bad_request(
            "Messages can be at most 10,000 characters long.",
        ));
    }
    let message = readable(&state, &user, channel_id, message_id).await?;
    messages::change(&state, &user, channel_id, message_id, Change::Edit(body)).await?;
    Ok(done(
        &headers,
        &message_href(&message),
        serde_json::Value::Null,
    ))
}

async fn delete(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((channel_id, message_id)): Path<(i64, i64)>,
    headers: HeaderMap,
) -> AppResult<Response> {
    let message = readable(&state, &user, channel_id, message_id).await?;
    messages::change(&state, &user, channel_id, message_id, Change::Delete).await?;
    let target = message.parent_id.map_or_else(
        || format!("/c/{channel_id}"),
        |parent| format!("/c/{channel_id}/t/{parent}"),
    );
    Ok(done(&headers, &target, serde_json::Value::Null))
}

async fn pin(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((channel_id, message_id)): Path<(i64, i64)>,
    headers: HeaderMap,
) -> AppResult<Response> {
    let message = readable(&state, &user, channel_id, message_id).await?;
    if message.deleted {
        return Err(AppError::NotFound);
    }
    let user_id = user.id;
    let now = now_ms();
    let pinned = state
        .db
        .call(move |conn| store::toggle_pin(conn, message_id, user_id, now))
        .await?;
    messages::refresh(&state, message_id).await?;
    Ok(done(
        &headers,
        &message_href(&message),
        json!({ "pinned": pinned }),
    ))
}

async fn save(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((channel_id, message_id)): Path<(i64, i64)>,
    headers: HeaderMap,
) -> AppResult<Response> {
    let message = readable(&state, &user, channel_id, message_id).await?;
    let user_id = user.id;
    let now = now_ms();
    let saved = state
        .db
        .call(move |conn| store::toggle_saved(conn, user_id, message_id, now))
        .await?;
    Ok(done(
        &headers,
        &message_href(&message),
        json!({ "saved": saved }),
    ))
}

/// The author or an admin hides a message's link preview.
async fn remove_preview(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((channel_id, message_id)): Path<(i64, i64)>,
    headers: HeaderMap,
) -> AppResult<Response> {
    let message = readable(&state, &user, channel_id, message_id).await?;
    let own = matches!(message.author, store::Author::User { id, .. } if id == user.id);
    if !own && !user.is_admin {
        return Err(AppError::Forbidden);
    }
    state
        .db
        .call(move |conn| store::set_preview(conn, message_id, None))
        .await?;
    messages::refresh(&state, message_id).await?;
    Ok(done(
        &headers,
        &message_href(&message),
        serde_json::Value::Null,
    ))
}

async fn pins(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(channel_id): Path<i64>,
) -> AppResult<Markup> {
    let user_id = user.id;
    let (channel, messages, sidebar, ctx) = state
        .db
        .call(move |conn| {
            let channel =
                store::channel_for(conn, channel_id, user_id)?.ok_or(AppError::NotFound)?;
            Ok((
                channel,
                store::pinned_messages(conn, channel_id)?,
                store::sidebar(conn, user_id)?,
                store::render_context(conn)?,
            ))
        })
        .await?;
    let shell = Shell {
        user: &user,
        sidebar: &sidebar,
        current: Some(channel_id),
    };
    Ok(views::messages::pins_page(
        &shell,
        &channel,
        &messages,
        &Render::for_user(&ctx, user.id),
    ))
}

async fn saved(user: CurrentUser, State(state): State<AppState>) -> AppResult<Markup> {
    let user_id = user.id;
    let (saved, sidebar, ctx) = state
        .db
        .call(move |conn| {
            Ok((
                store::saved_messages(conn, user_id)?,
                store::sidebar(conn, user_id)?,
                store::render_context(conn)?,
            ))
        })
        .await?;
    let shell = Shell {
        user: &user,
        sidebar: &sidebar,
        current: None,
    };
    let ids = saved.iter().map(|item| item.message.id).collect();
    Ok(views::messages::saved_page(
        &shell,
        &saved,
        &Render::for_user(&ctx, user.id).with_saved(&ids),
    ))
}

async fn activity(user: CurrentUser, State(state): State<AppState>) -> AppResult<Markup> {
    let user_id = user.id;
    let (items, sidebar, ctx) = state
        .db
        .call(move |conn| {
            let items = store::activity(conn, user_id)?;
            Ok((
                items,
                store::sidebar(conn, user_id)?,
                store::render_context(conn)?,
            ))
        })
        .await?;
    let shell = Shell {
        user: &user,
        sidebar: &sidebar,
        current: None,
    };
    Ok(views::messages::activity_page(
        &shell,
        &items,
        &Render::for_user(&ctx, user.id),
    ))
}
