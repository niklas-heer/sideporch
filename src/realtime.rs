//! Live updates over WebSocket. Every event is rendered once on the server
//! and fanned out to the connections that may see it.

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
    /// A message's reactions changed. `html` replaces its reaction bar.
    Reactions {
        channel_id: i64,
        message_id: i64,
        html: String,
    },
}

struct Envelope {
    /// `None` means every signed-in user may receive the event.
    audience: Option<Vec<i64>>,
    json: String,
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
        match serde_json::to_string(event) {
            // Sending fails only when nobody is connected, which is fine.
            Ok(json) => drop(self.sender.send(Arc::new(Envelope { audience, json }))),
            Err(error) => tracing::error!(%error, "could not encode event"),
        }
    }
}

/// Messages the browser sends over the socket.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMessage {
    Read { channel_id: i64, message_id: i64 },
    Visibility { visible: bool },
}

pub async fn connect(
    ws: WebSocketUpgrade,
    user: CurrentUser,
    State(state): State<AppState>,
) -> Response {
    ws.on_upgrade(move |socket| serve(socket, state, user.id))
}

async fn serve(socket: WebSocket, state: AppState, user_id: i64) {
    let mut events = state.hub.sender.subscribe();
    let (mut sink, mut stream) = socket.split();
    let mut keepalive = tokio::time::interval(Duration::from_secs(25));
    let mut visible = false;
    loop {
        tokio::select! {
            event = events.recv() => match event {
                Ok(envelope) => {
                    let visible = envelope.audience.as_ref().is_none_or(|members| members.contains(&user_id));
                    if visible && sink.send(WsMessage::Text(envelope.json.clone().into())).await.is_err() {
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
                        mark_read(&state, user_id, channel_id, message_id).await;
                    }
                    Ok(ClientMessage::Visibility { visible: now }) => {
                        state.hub.set_visible(user_id, visible, now);
                        visible = now;
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
