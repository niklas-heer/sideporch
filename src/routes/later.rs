//! Reminders and scheduled messages: the Scheduled page, reminders about
//! a message, and messages written now to send later.

use axum::{
    Form, Json, Router,
    extract::{Path, State},
    http::HeaderMap,
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use maud::Markup;
use serde::Deserialize;
use serde_json::json;

use super::{MAX_MESSAGE_CHARS, message_href, shell_data, wants_no_content};
use crate::{
    AppState,
    auth::CurrentUser,
    error::{AppError, AppResult},
    later, now_ms, store,
    views::{self, Shell},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/scheduled", get(page))
        .route("/scheduled/{id}/cancel", post(cancel_message))
        .route("/scheduled/{id}/send", post(send_now))
        .route("/reminders/{id}/cancel", post(cancel_reminder))
        .route("/c/{channel_id}/m/{message_id}/remind", post(remind))
}

async fn page(user: CurrentUser, State(state): State<AppState>) -> AppResult<Markup> {
    let user_id = user.id;
    let (reminders, scheduled, zone) = state
        .db
        .call(move |conn| {
            Ok((
                store::reminders(conn, user_id)?,
                store::scheduled_messages(conn, user_id)?,
                store::user_timezone(conn, user_id)?,
            ))
        })
        .await?;
    let sidebar = shell_data(&state, user.id).await?;
    let shell = Shell {
        user: &user,
        sidebar: &sidebar,
        current: None,
    };
    Ok(views::later::page(
        &shell,
        &reminders,
        &scheduled,
        &later::zone(&zone),
    ))
}

async fn cancel_message(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Redirect> {
    let user_id = user.id;
    state
        .db
        .call(move |conn| store::delete_scheduled(conn, user_id, id))
        .await?;
    Ok(Redirect::to("/scheduled"))
}

async fn send_now(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Redirect> {
    let user_id = user.id;
    let now = now_ms();
    state
        .db
        .call(move |conn| store::send_scheduled_now(conn, user_id, id, now))
        .await?;
    later::deliver_due(&state).await?;
    Ok(Redirect::to("/scheduled"))
}

async fn cancel_reminder(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Redirect> {
    let user_id = user.id;
    state
        .db
        .call(move |conn| store::delete_reminder(conn, user_id, id))
        .await?;
    Ok(Redirect::to("/scheduled"))
}

#[derive(Deserialize)]
struct RemindForm {
    when: String,
}

/// Reminds the user about a message later.
async fn remind(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((channel_id, message_id)): Path<(i64, i64)>,
    headers: HeaderMap,
    Form(form): Form<RemindForm>,
) -> AppResult<Response> {
    let user_id = user.id;
    let now = now_ms();
    let (message, when) = state
        .db
        .call(move |conn| {
            let message = store::readable_message(conn, user_id, channel_id, message_id)?
                .ok_or(AppError::NotFound)?;
            let when = later::parse(&form.when, &later::now_for(conn, user_id)?)
                .ok_or_else(|| AppError::bad_request("That time is in the past or unclear."))?;
            let excerpt: String = message.body.chars().take(120).collect();
            let text = if excerpt.is_empty() {
                "a message".to_owned()
            } else {
                format!("“{excerpt}”")
            };
            store::add_reminder(
                conn,
                user_id,
                &text,
                Some(message_id),
                when.timestamp().as_millisecond(),
                now,
            )?;
            Ok((message, when))
        })
        .await?;
    let description = later::describe(&when);
    if wants_no_content(&headers) {
        return Ok(Json(json!({ "at": description })).into_response());
    }
    Ok(Redirect::to(&message_href(&message)).into_response())
}

/// Stores a message to send later and answers with a private notice.
pub async fn schedule(
    state: &AppState,
    user_id: i64,
    channel_id: i64,
    parent_id: Option<i64>,
    body: String,
    send_at: &str,
) -> AppResult<String> {
    if body.is_empty() {
        return Err(AppError::bad_request(
            "Write the message you want to send later.",
        ));
    }
    if body.chars().count() > MAX_MESSAGE_CHARS {
        return Err(AppError::bad_request(
            "Messages can be at most 10,000 characters long.",
        ));
    }
    let phrase = send_at.to_owned();
    let now = now_ms();
    let when = state
        .db
        .call(move |conn| {
            let when = later::parse(&phrase, &later::now_for(conn, user_id)?).ok_or_else(|| {
                AppError::bad_request("Pick a time in the future, such as `tomorrow at 9:00`.")
            })?;
            store::add_scheduled(
                conn,
                &store::NewScheduled {
                    user_id,
                    channel_id,
                    parent_id,
                    body: &body,
                    send_at: when.timestamp().as_millisecond(),
                },
                now,
            )?;
            Ok(when)
        })
        .await?;
    Ok(format!(
        "Scheduled for {}. Change or cancel it under [Scheduled](/scheduled).",
        later::describe(&when)
    ))
}
