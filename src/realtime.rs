//! Live updates over WebSocket. Every event is rendered once on the server
//! and fanned out to the connections that may see it.
//!
//! Each browser says which channel it shows. Only those browsers get new
//! messages in full, edits, reactions and typing; the rest get a short
//! notice that the channel has something new, once until they read it, or
//! every time they are mentioned. That keeps the work per message close to
//! the number of people looking, not everyone online.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    extract::{
        State,
        ws::{Message as WsMessage, WebSocket, WebSocketUpgrade},
    },
    response::Response,
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::{AppState, auth::CurrentUser, store};

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// A new message. `html` is the rendered message; `reply_count` is the
    /// parent's new reply count when the message is a thread reply.
    Message {
        channel_id: i64,
        id: i64,
        parent_id: Option<i64>,
        author: String,
        created_at: i64,
        html: String,
        reply_count: Option<i64>,
        /// People for whom this lands on their Activity page.
        activity: Vec<i64>,
    },
    /// A message changed: edited, pinned, deleted with replies left, or
    /// its preview or poll updated. `html` replaces the message.
    MessageChanged {
        channel_id: i64,
        id: i64,
        parent_id: Option<i64>,
        html: String,
    },
    /// A message is gone. `reply_count` is the parent's new count for a
    /// deleted reply.
    MessageDeleted {
        channel_id: i64,
        id: i64,
        parent_id: Option<i64>,
        reply_count: Option<i64>,
    },
    /// Someone is writing in a channel or, with `parent_id`, a thread.
    Typing {
        channel_id: i64,
        parent_id: Option<i64>,
        user_id: i64,
        name: String,
    },
    /// A message's reactions changed. `html` replaces its reaction bar.
    Reactions {
        channel_id: i64,
        message_id: i64,
        html: String,
    },
}

impl Event {
    const fn channel_id(&self) -> i64 {
        match self {
            Self::Message { channel_id, .. }
            | Self::MessageChanged { channel_id, .. }
            | Self::MessageDeleted { channel_id, .. }
            | Self::Typing { channel_id, .. }
            | Self::Reactions { channel_id, .. } => *channel_id,
        }
    }
}

/// What browsers not showing the channel learn about a new message.
#[derive(Serialize)]
struct Notice<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    channel_id: i64,
    id: i64,
    parent_id: Option<i64>,
    author: &'a str,
    activity: &'a [i64],
}

struct Envelope {
    /// `None` means every signed-in user may receive the event.
    audience: Option<Vec<i64>>,
    channel_id: i64,
    /// The event in full, for browsers showing the channel.
    json: String,
    /// For new messages: the short notice for everyone else.
    notice: Option<String>,
    /// Who the message mentions or answers; they always get the notice.
    activity: Vec<i64>,
    /// Who wrote it; their other browsers need no notice.
    author: Option<i64>,
}

#[derive(Clone)]
pub struct Hub {
    sender: broadcast::Sender<Arc<Envelope>>,
    /// Open, visible browser tabs per user. Push notifications skip people
    /// who are looking at Sideporch right now.
    visible: Arc<Mutex<HashMap<i64, usize>>>,
}

impl Default for Hub {
    fn default() -> Self {
        Self {
            sender: broadcast::channel(512).0,
            visible: Arc::default(),
        }
    }
}

impl Hub {
    /// How many people have Sideporch open and visible right now.
    pub fn online_count(&self) -> usize {
        self.visible.lock().map_or(0, |visible| {
            visible.values().filter(|count| **count > 0).count()
        })
    }

    pub fn is_watching(&self, user_id: i64) -> bool {
        self.visible
            .lock()
            .is_ok_and(|visible| visible.get(&user_id).is_some_and(|count| *count > 0))
    }

    fn set_visible(&self, user_id: i64, was: bool, now: bool) {
        if was == now {
            return;
        }
        if let Ok(mut visible) = self.visible.lock() {
            let count = visible.entry(user_id).or_default();
            *count = if now {
                count.saturating_add(1)
            } else {
                count.saturating_sub(1)
            };
        }
    }

    pub fn publish(&self, audience: Option<Vec<i64>>, event: &Event) {
        let (notice, activity, author) = match event {
            Event::Message {
                channel_id,
                id,
                parent_id,
                author,
                activity,
                ..
            } => (
                serde_json::to_string(&Notice {
                    kind: "message",
                    channel_id: *channel_id,
                    id: *id,
                    parent_id: *parent_id,
                    author,
                    activity,
                })
                .ok(),
                activity.clone(),
                author.strip_prefix("u:").and_then(|id| id.parse().ok()),
            ),
            _ => (None, Vec::new(), None),
        };
        match serde_json::to_string(event) {
            // Sending fails only when nobody is connected, which is fine.
            Ok(json) => drop(self.sender.send(Arc::new(Envelope {
                audience,
                channel_id: event.channel_id(),
                json,
                notice,
                activity,
                author,
            }))),
            Err(error) => tracing::error!(%error, "could not encode event"),
        }
    }
}

/// Messages the browser sends over the socket.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMessage {
    Read {
        channel_id: i64,
        message_id: i64,
    },
    Visibility {
        visible: bool,
    },
    Typing {
        channel_id: i64,
        parent_id: Option<i64>,
    },
    /// The browser's time zone, such as `Europe/Berlin`.
    Timezone {
        name: String,
    },
    /// The channel the page shows, if any.
    View {
        channel_id: Option<i64>,
    },
}

/// What one connection is looking at, to pick what it needs.
#[derive(Default)]
struct Viewer {
    channel: Option<i64>,
    /// Channels this connection was told have something new.
    notified: std::collections::HashSet<i64>,
}

impl Viewer {
    /// The text to send for `envelope`, if any.
    fn pick<'a>(&mut self, user_id: i64, envelope: &'a Envelope) -> Option<&'a str> {
        if envelope
            .audience
            .as_ref()
            .is_some_and(|members| !members.contains(&user_id))
        {
            return None;
        }
        if self.channel == Some(envelope.channel_id) {
            return Some(&envelope.json);
        }
        let notice = envelope.notice.as_deref()?;
        if envelope.author == Some(user_id) {
            return None;
        }
        (envelope.activity.contains(&user_id) || self.notified.insert(envelope.channel_id))
            .then_some(notice)
    }
}

/// The shortest gap between two typing notices from one connection.
const TYPING_INTERVAL: Duration = Duration::from_secs(2);

pub async fn connect(
    ws: WebSocketUpgrade,
    user: CurrentUser,
    State(state): State<AppState>,
) -> Response {
    // Browsers only send small notes, and events go out one at a time, so
    // small buffers suffice. The defaults (128 kB each) would dominate
    // memory with thousands of people online.
    ws.read_buffer_size(4 * 1024)
        .write_buffer_size(16 * 1024)
        .max_message_size(64 * 1024)
        .on_upgrade(move |socket| serve(socket, state, user.id))
}

async fn serve(socket: WebSocket, state: AppState, user_id: i64) {
    let mut events = state.hub.sender.subscribe();
    let (mut sink, mut stream) = socket.split();
    let mut keepalive = tokio::time::interval(Duration::from_secs(25));
    let mut visible = false;
    let mut viewer = Viewer::default();
    let mut last_typing: Option<tokio::time::Instant> = None;
    loop {
        tokio::select! {
            event = events.recv() => match event {
                Ok(envelope) => {
                    if let Some(text) = viewer.pick(user_id, &envelope)
                        && sink.send(WsMessage::Text(text.to_owned().into())).await.is_err()
                    {
                        break;
                    }
                }
                // The client fell behind; it reloads to catch up.
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    if sink.send(WsMessage::Text(r#"{"type":"resync"}"#.into())).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            incoming = stream.next() => match incoming {
                Some(Ok(WsMessage::Text(text))) => match serde_json::from_str(&text) {
                    Ok(ClientMessage::Read { channel_id, message_id }) => {
                        viewer.notified.remove(&channel_id);
                        mark_read(&state, user_id, channel_id, message_id).await;
                    }
                    Ok(ClientMessage::View { channel_id }) => {
                        if let Some(channel_id) = channel_id {
                            viewer.notified.remove(&channel_id);
                        }
                        viewer.channel = channel_id;
                    }
                    Ok(ClientMessage::Visibility { visible: now }) => {
                        state.hub.set_visible(user_id, visible, now);
                        visible = now;
                    }
                    Ok(ClientMessage::Timezone { name }) => save_timezone(&state, user_id, name).await,
                    Ok(ClientMessage::Typing { channel_id, parent_id }) => {
                        let now = tokio::time::Instant::now();
                        if last_typing.is_none_or(|last| now.duration_since(last) >= TYPING_INTERVAL) {
                            last_typing = Some(now);
                            announce_typing(&state, user_id, channel_id, parent_id).await;
                        }
                    }
                    Err(_) => {}
                },
                Some(Ok(WsMessage::Close(_)) | Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
            _ = keepalive.tick() => {
                if sink.send(WsMessage::Ping(Vec::new().into())).await.is_err() {
                    break;
                }
            }
        }
    }
    state.hub.set_visible(user_id, visible, false);
}

/// Tells the channel's readers that `user_id` is writing.
async fn announce_typing(state: &AppState, user_id: i64, channel_id: i64, parent_id: Option<i64>) {
    let found = state
        .db
        .call(move |conn| {
            let Some(channel) = store::channel_for(conn, channel_id, user_id)? else {
                return Ok(None);
            };
            let name = store::user(conn, user_id)?.map(|user| user.display_name);
            Ok(name.map(|name| (channel.id, name, store::audience(conn, channel_id))))
        })
        .await;
    if let Ok(Some((channel_id, name, Ok(audience)))) = found {
        state.hub.publish(
            audience,
            &Event::Typing {
                channel_id,
                parent_id,
                user_id,
                name,
            },
        );
    }
}

async fn save_timezone(state: &AppState, user_id: i64, name: String) {
    let Ok(name) = crate::later::valid_zone(&name) else {
        return;
    };
    let saved = state
        .db
        .call(move |conn| store::set_user_timezone(conn, user_id, &name))
        .await;
    if let Err(error) = saved {
        tracing::warn!(?error, "could not save a time zone");
    }
}

async fn mark_read(state: &AppState, user_id: i64, channel_id: i64, message_id: i64) {
    let result = state
        .db
        .call(move |conn| {
            if store::channel_for(conn, channel_id, user_id)?.is_some() {
                store::mark_read(conn, user_id, channel_id, message_id)?;
            }
            Ok(())
        })
        .await;
    if let Err(error) = result {
        tracing::warn!(?error, "could not record read position");
    }
}
