//! The chat API for bots, under `/api/v1/`. A bot's token goes in
//! `Authorization: Bearer`; every answer is JSON, and every failure has the
//! same shape: `{"error": {"code": "…", "message": "…"}}`, with the codes
//! listed in the API documentation.
//!
//! Each request checks, in order: the token (401), its rate limit (429), its
//! scope (403), and for a channel, that the channel is public (404) and in
//! the token's list (403). Posts go through [`messages::post`], the same path
//! every message takes.

use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, Query, State, rejection::QueryRejection},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    AppState,
    access::{self, Counted},
    auth::hash_token,
    bots::{self, Caller, ChannelScope, Page, PublicChannel},
    error::AppError,
    messages::{self, Draft, Sender},
    now_ms,
    routes::MAX_MESSAGE_CHARS,
    store::{Author, Message, Policy},
};

/// Messages returned when a request doesn't say how many.
const DEFAULT_LIMIT: usize = 50;
/// The most messages one request returns.
const MAX_LIMIT: usize = 100;

pub fn router() -> Router<AppState> {
    Router::new().nest(
        "/api/v1",
        Router::new()
            .route("/me", get(me))
            .route("/channels", get(channels))
            .route(
                "/channels/{channel_id}/messages",
                get(list_messages).post(post_message),
            )
            .fallback(not_found),
    )
}

/// A failed request: its status, a stable code and a message for people.
#[derive(Debug)]
pub struct Refusal {
    status: StatusCode,
    code: &'static str,
    message: String,
    retry_after: Option<u64>,
}

impl Refusal {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            retry_after: None,
        }
    }

    fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_request", message)
    }

    fn not_found() -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "Nothing here has that id, or the bot can't see it.",
        )
    }
}

impl From<AppError> for Refusal {
    fn from(error: AppError) -> Self {
        match error {
            AppError::BadRequest(message) => Self::bad_request(message),
            AppError::Forbidden => Self::new(
                StatusCode::FORBIDDEN,
                "forbidden",
                "This token isn't allowed to do that.",
            ),
            AppError::NotFound => Self::not_found(),
            AppError::Gone(message) => Self::new(StatusCode::GONE, "gone", message),
            AppError::Internal(detail) => {
                tracing::error!(%detail, "API request failed");
                Self::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    "Something went wrong on the server. Try again in a moment.",
                )
            }
        }
    }
}

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        let unauthorized = self.status == StatusCode::UNAUTHORIZED;
        let mut response = (
            self.status,
            Json(json!({ "error": { "code": self.code, "message": self.message } })),
        )
            .into_response();
        if unauthorized {
            response
                .headers_mut()
                .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        }
        if let Some(seconds) = self.retry_after {
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from(seconds));
        }
        response
    }
}

type ApiResult<T> = Result<T, Refusal>;

/// Checks the bearer token, and counts the request against its rate limit.
async fn authenticate(state: &AppState, headers: &HeaderMap) -> ApiResult<Caller> {
    let missing = || {
        Refusal::new(
            StatusCode::UNAUTHORIZED,
            "missing_token",
            "Send the bot's token as `Authorization: Bearer <token>`.",
        )
    };
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split_once(' '))
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
        .map(|(_, token)| token.trim())
        .filter(|token| !token.is_empty())
        .ok_or_else(missing)?;
    let token_hash = hash_token(token);
    let now = now_ms();
    let caller = state
        .db
        .call(move |conn| bots::authenticate(conn, &token_hash, now))
        .await?
        .ok_or_else(|| {
            Refusal::new(
                StatusCode::UNAUTHORIZED,
                "invalid_token",
                "This token is wrong, revoked or expired.",
            )
        })?;
    let counted = Counted::ApiToken(caller.token_id);
    if let Err(wait) = state.access.check(&counted, access::API_REQUESTS) {
        return Err(Refusal {
            status: StatusCode::TOO_MANY_REQUESTS,
            code: "rate_limited",
            message: format!(
                "This token made too many requests. Try again in {}.",
                access::wait_text(wait)
            ),
            retry_after: Some(wait.as_secs().max(1)),
        });
    }
    state.access.count(counted);
    Ok(caller)
}

fn require(caller: &Caller, scope: &str) -> ApiResult<()> {
    if caller.has(scope) {
        Ok(())
    } else {
        Err(Refusal::new(
            StatusCode::FORBIDDEN,
            "missing_scope",
            format!("This token needs the {scope} scope."),
        ))
    }
}

/// The public channel a request names, if the token may use it. Private
/// channels and direct messages answer as missing, so they don't reveal
/// themselves.
async fn reachable_channel(
    state: &AppState,
    caller: &Caller,
    raw_id: &str,
) -> ApiResult<PublicChannel> {
    let channel_id: i64 = raw_id.parse().map_err(|_| Refusal::not_found())?;
    let channel = state
        .db
        .call(move |conn| bots::public_channel(conn, channel_id))
        .await?
        .ok_or_else(Refusal::not_found)?;
    if !caller.channels.allows(channel.id) {
        return Err(Refusal::new(
            StatusCode::FORBIDDEN,
            "channel_not_allowed",
            "This token can't use that channel.",
        ));
    }
    Ok(channel)
}

async fn not_found() -> Refusal {
    Refusal::new(
        StatusCode::NOT_FOUND,
        "not_found",
        "No API route has that path.",
    )
}

async fn me(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let caller = authenticate(&state, &headers).await?;
    let channels = match &caller.channels {
        ChannelScope::All => json!("all"),
        ChannelScope::Only(ids) => json!(ids),
    };
    Ok(Json(json!({
        "bot": {
            "name": caller.name,
            "display_name": caller.display_name,
            "icon": caller.icon,
        },
        "token": { "id": caller.token_id, "label": caller.label },
        "scopes": caller.scopes,
        "channels": channels,
        "expires_at": caller.expires_at,
    })))
}

async fn channels(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let caller = authenticate(&state, &headers).await?;
    require(&caller, "channels:read")?;
    let public = state.db.call(|conn| bots::public_channels(conn)).await?;
    let channels: Vec<Value> = public
        .into_iter()
        .filter(|(id, _, _)| caller.channels.allows(*id))
        .map(|(id, name, topic)| json!({ "id": id, "name": name, "topic": topic }))
        .collect();
    Ok(Json(json!({ "channels": channels })))
}

#[derive(Deserialize)]
struct ListParams {
    before: Option<String>,
    after: Option<String>,
    limit: Option<String>,
}

fn cursor(raw: Option<&str>, name: &str) -> ApiResult<Option<i64>> {
    raw.map(|value| {
        value
            .parse::<i64>()
            .map_err(|_| Refusal::bad_request(format!("`{name}` must be a message id.")))
    })
    .transpose()
}

fn parse_limit(raw: Option<&str>) -> ApiResult<usize> {
    let Some(raw) = raw else {
        return Ok(DEFAULT_LIMIT);
    };
    let limit: usize = raw
        .parse()
        .ok()
        .filter(|limit| *limit >= 1)
        .ok_or_else(|| Refusal::bad_request("`limit` must be a whole number, 1 or more."))?;
    Ok(limit.min(MAX_LIMIT))
}

async fn list_messages(
    State(state): State<AppState>,
    Path(channel): Path<String>,
    headers: HeaderMap,
    query: Result<Query<ListParams>, QueryRejection>,
) -> ApiResult<Json<Value>> {
    let caller = authenticate(&state, &headers).await?;
    require(&caller, "messages:read")?;
    let channel = reachable_channel(&state, &caller, &channel).await?;
    let params = query
        .map(|Query(params)| params)
        .map_err(|_| Refusal::bad_request("The query string isn't valid."))?;
    let page = match (
        cursor(params.before.as_deref(), "before")?,
        cursor(params.after.as_deref(), "after")?,
    ) {
        (Some(_), Some(_)) => {
            return Err(Refusal::bad_request("Use `before` or `after`, not both."));
        }
        (Some(before), None) => Page::Before(before),
        (None, Some(after)) => Page::After(after),
        (None, None) => Page::Newest,
    };
    let limit = parse_limit(params.limit.as_deref())?;
    let channel_id = channel.id;
    let (messages, has_more) = state
        .db
        .call(move |conn| bots::messages(conn, channel_id, page, limit))
        .await?;
    let messages: Vec<Value> = messages.iter().map(message_json).collect();
    Ok(Json(json!({ "messages": messages, "has_more": has_more })))
}

/// The body of a message a bot posts.
#[derive(Deserialize)]
struct NewMessage {
    body: String,
    thread_id: Option<i64>,
}

async fn post_message(
    State(state): State<AppState>,
    Path(channel): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let caller = authenticate(&state, &headers).await?;
    require(&caller, "messages:write")?;
    let channel = reachable_channel(&state, &caller, &channel).await?;
    let input: NewMessage = serde_json::from_slice(&body).map_err(|_| {
        Refusal::bad_request(r#"Send JSON like {"body": "text"}, with an optional "thread_id"."#)
    })?;
    let text = input.body.trim().to_owned();
    if text.is_empty() {
        return Err(Refusal::bad_request("The message is empty."));
    }
    if text.chars().count() > MAX_MESSAGE_CHARS {
        return Err(Refusal::bad_request(
            "Messages can be at most 10,000 characters long.",
        ));
    }
    // Bots are not managers: they may write only where anyone may.
    let reply = input.thread_id.is_some();
    let policy = if reply {
        channel.posting.reply
    } else {
        channel.posting.post
    };
    if policy != Policy::Everyone {
        return Err(Refusal::new(
            StatusCode::FORBIDDEN,
            "posting_not_allowed",
            format!(
                "Only the managers of #{} {}, and bots aren't managers.",
                channel.name,
                if reply {
                    "reply in threads there"
                } else {
                    "start new posts there"
                }
            ),
        ));
    }
    let message = messages::post(
        &state,
        Draft {
            channel_id: channel.id,
            parent_id: input.thread_id,
            sender: Sender::ApiBot {
                id: caller.bot_id,
                name: caller.display_name.clone(),
                icon: caller.icon.clone(),
            },
            body: text,
            attachments: Vec::new(),
            files: Vec::new(),
            gif: None,
            poll: None,
            buttons: Vec::new(),
        },
    )
    .await?;
    let mut value = message_json(&message);
    if let Some(object) = value.as_object_mut() {
        object.insert("bot_id".to_owned(), json!(caller.bot_id));
    }
    Ok((StatusCode::CREATED, Json(value)))
}

/// A message as the API shows it: its text, when, where, and who wrote it.
/// Nothing private (no email, no address) is included.
fn message_json(message: &Message) -> Value {
    let author = match &message.author {
        Author::User {
            id, display_name, ..
        } => json!({ "type": "user", "id": id, "name": display_name }),
        Author::Bot { name, icon } => json!({ "type": "bot", "name": name, "icon": icon }),
        Author::Removed => json!({ "type": "removed" }),
    };
    json!({
        "id": message.id,
        "channel_id": message.channel_id,
        "parent_id": message.parent_id,
        "created_at": message.created_at,
        "edited_at": message.edited_at,
        "body": message.body,
        "author": author,
    })
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_LIMIT, MAX_LIMIT, cursor, parse_limit};

    #[test]
    fn limits_default_cap_and_refuse_nonsense() {
        assert_eq!(parse_limit(None).unwrap(), DEFAULT_LIMIT);
        assert_eq!(parse_limit(Some("5")).unwrap(), 5);
        assert_eq!(parse_limit(Some("100000")).unwrap(), MAX_LIMIT);
        assert!(parse_limit(Some("0")).is_err());
        assert!(parse_limit(Some("-3")).is_err());
        assert!(parse_limit(Some("many")).is_err());
    }

    #[test]
    fn cursors_are_message_ids() {
        assert_eq!(cursor(None, "before").unwrap(), None);
        assert_eq!(cursor(Some("42"), "before").unwrap(), Some(42));
        assert!(cursor(Some("4x"), "after").is_err());
    }
}
