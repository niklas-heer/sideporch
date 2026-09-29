//! What people do to a single message: edit, delete, pin and save it, plus
//! permalinks and the lists of pinned and saved messages.

use axum::{
    Form, Json, Router,
    extract::{Path, Query, State},
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
        .route("/c/{channel_id}/m/{message_id}/vote", post(vote))
        .route("/c/{channel_id}/m/{message_id}/rank", post(rank))
        .route(
            "/c/{channel_id}/m/{message_id}/close-poll",
            post(close_poll),
        )
        .route("/c/{channel_id}/m/{message_id}/buttons", post(click))
        .route(
            "/c/{channel_id}/m/{message_id}/preview/remove",
            post(remove_preview),
        )
        .route("/c/{channel_id}/pins", get(pins))
        .route("/saved", get(saved))
        .route("/activity", get(activity))
        .route("/share", get(share_form).post(share))
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
    crate::community::check_content(&user, &body)?;
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

#[derive(Deserialize)]
struct VoteForm {
    option: i64,
}

/// Loads a poll someone may vote in: open, readable, in a channel where
/// they may react.
async fn open_poll(
    state: &AppState,
    user: &CurrentUser,
    channel_id: i64,
    message_id: i64,
) -> AppResult<(store::Message, store::Poll)> {
    crate::community::check_not_timed_out(user)?;
    let message = readable(state, user, channel_id, message_id).await?;
    let user_id = user.id;
    let channel = state
        .db
        .call(move |conn| store::channel_for(conn, channel_id, user_id))
        .await?
        .ok_or(AppError::NotFound)?;
    if !channel.may_react() {
        return Err(AppError::bad_request(
            "Only the channel's managers vote there.",
        ));
    }
    let poll = message
        .poll
        .clone()
        .filter(|_| !message.deleted)
        .ok_or_else(|| AppError::bad_request("That message has no poll."))?;
    if poll.closed {
        return Err(AppError::bad_request("Voting in this poll has ended."));
    }
    Ok((message, poll))
}

/// Votes for an option, or takes the vote back. In polls where people pick
/// several, each option toggles on its own.
async fn vote(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((channel_id, message_id)): Path<(i64, i64)>,
    headers: HeaderMap,
    Form(form): Form<VoteForm>,
) -> AppResult<Response> {
    let (message, poll) = open_poll(&state, &user, channel_id, message_id).await?;
    if usize::try_from(form.option).map_or(true, |option| option >= poll.options.len()) {
        return Err(AppError::bad_request("That poll has no such option."));
    }
    let user_id = user.id;
    let now = now_ms();
    state
        .db
        .call(move |conn| match poll.kind {
            crate::polls::Kind::Single => store::vote(conn, message_id, user_id, form.option, now),
            crate::polls::Kind::Multiple => {
                store::toggle_mark(conn, message_id, user_id, form.option, now)
            }
            crate::polls::Kind::Ranked => Err(AppError::bad_request(
                "Rank the options in this poll instead.",
            )),
        })
        .await?;
    messages::refresh(&state, message_id).await?;
    Ok(done(
        &headers,
        &message_href(&message),
        serde_json::Value::Null,
    ))
}

/// Saves someone's ranking in a ranked poll. The form has a field `r<N>`
/// per option with its rank, 1 for the favorite; empty leaves it out.
async fn rank(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((channel_id, message_id)): Path<(i64, i64)>,
    headers: HeaderMap,
    Form(form): Form<std::collections::HashMap<String, String>>,
) -> AppResult<Response> {
    let (message, poll) = open_poll(&state, &user, channel_id, message_id).await?;
    if poll.kind != crate::polls::Kind::Ranked {
        return Err(AppError::bad_request("This poll isn't ranked."));
    }
    let ranks: Vec<(usize, u32)> = form
        .iter()
        .filter_map(|(key, value)| {
            let option = key.strip_prefix('r')?.parse().ok()?;
            let rank = value.trim().parse().ok()?;
            Some((option, rank))
        })
        .collect();
    let ranking = crate::polls::ranking_from_ranks(poll.options.len(), &ranks);
    let user_id = user.id;
    let now = now_ms();
    state
        .db
        .call(move |conn| store::set_ranking(conn, message_id, user_id, &ranking, now))
        .await?;
    messages::refresh(&state, message_id).await?;
    Ok(done(
        &headers,
        &message_href(&message),
        serde_json::Value::Null,
    ))
}

/// Ends voting: the poll's author or an admin may.
async fn close_poll(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((channel_id, message_id)): Path<(i64, i64)>,
    headers: HeaderMap,
) -> AppResult<Response> {
    let message = readable(&state, &user, channel_id, message_id).await?;
    let own = matches!(message.author, store::Author::User { id, .. } if id == user.id);
    if message.poll.is_none() || message.deleted {
        return Err(AppError::NotFound);
    }
    if !own && !user.may(crate::community::Permission::Moderate) {
        return Err(AppError::Forbidden);
    }
    let now = now_ms();
    state
        .db
        .call(move |conn| store::close_poll(conn, message_id, now))
        .await?;
    messages::refresh(&state, message_id).await?;
    Ok(done(
        &headers,
        &message_href(&message),
        serde_json::Value::Null,
    ))
}

#[derive(Deserialize)]
struct ButtonForm {
    index: usize,
}

/// Tells the automation that posted a message that someone clicked one of
/// its buttons.
async fn click(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((channel_id, message_id)): Path<(i64, i64)>,
    headers: HeaderMap,
    Form(form): Form<ButtonForm>,
) -> AppResult<Response> {
    let user_id = user.id;
    let (message, event) = state
        .db
        .call(move |conn| {
            let message = store::readable_message(conn, user_id, channel_id, message_id)?
                .filter(|message| !message.deleted)
                .ok_or(AppError::NotFound)?;
            let button = message
                .buttons
                .get(form.index)
                .cloned()
                .ok_or_else(|| AppError::bad_request("That button is gone."))?;
            let person = store::user(conn, user_id)?.ok_or(AppError::NotFound)?;
            let event = crate::automations::ButtonEvent {
                value: button.value,
                label: button.label,
                user: person.display_name,
                username: person.username,
                message: crate::automations::MessageEvent::new(conn, &message)?,
            };
            Ok((message, event))
        })
        .await?;
    if let Some(automation_id) = message.automation_id {
        state
            .automations
            .event_for(automation_id, crate::automations::Event::Button(event));
    }
    Ok(done(
        &headers,
        &message_href(&message),
        serde_json::Value::Null,
    ))
}

/// What another app shares, as Android's share sheet sends it.
#[derive(Deserialize, Default)]
struct Shared {
    #[serde(default)]
    title: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    url: String,
}

impl Shared {
    /// The pieces as one message, without repeating the link when the text
    /// already has it.
    fn message(&self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        for part in [self.title.trim(), self.text.trim(), self.url.trim()] {
            if !part.is_empty() && !parts.iter().any(|seen| seen.contains(part)) {
                parts.push(part);
            }
        }
        parts.join("\n")
    }
}

/// Where the installed app lands when someone shares into Sideporch.
async fn share_form(
    user: CurrentUser,
    State(state): State<AppState>,
    Query(shared): Query<Shared>,
) -> AppResult<Markup> {
    let sidebar = super::shell_data(&state, user.id).await?;
    let shell = Shell {
        user: &user,
        sidebar: &sidebar,
        current: None,
    };
    Ok(views::messages::share_page(&shell, &shared.message()))
}

#[derive(Deserialize)]
struct ShareForm {
    channel_id: i64,
    body: String,
}

async fn share(
    user: CurrentUser,
    State(state): State<AppState>,
    Form(form): Form<ShareForm>,
) -> AppResult<Redirect> {
    let body = form.body.trim().to_owned();
    if body.is_empty() || body.chars().count() > MAX_MESSAGE_CHARS {
        return Err(AppError::bad_request(
            "Write something, up to 10,000 characters.",
        ));
    }
    let user_id = user.id;
    let channel_id = form.channel_id;
    let channel = state
        .db
        .call(move |conn| store::channel_for(conn, channel_id, user_id))
        .await?
        .ok_or(AppError::NotFound)?;
    if !channel.may_write(false) {
        return Err(AppError::bad_request(channel.write_refusal(false)));
    }
    messages::post(
        &state,
        messages::Draft {
            channel_id,
            parent_id: None,
            sender: messages::Sender::User(user_id),
            body,
            attachments: Vec::new(),
            files: Vec::new(),
            gif: None,
            poll: None,
            buttons: Vec::new(),
        },
    )
    .await?;
    Ok(Redirect::to(&format!("/c/{channel_id}")))
}
