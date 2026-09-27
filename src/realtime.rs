//! Live updates over WebSocket. Every event is rendered once on the server
//! and fanned out to the connections that may see it.

use std::{sync::Arc, time::Duration};

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
}

struct Envelope {
    /// `None` means every signed-in user may receive the event.
    audience: Option<Vec<i64>>,
    json: String,
}

#[derive(Clone)]
pub struct Hub {
    sender: broadcast::Sender<Arc<Envelope>>,
}

impl Default for Hub {
    fn default() -> Self {
        Self {
            sender: broadcast::channel(512).0,
        }
    }
}

impl Hub {
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
                Some(Ok(WsMessage::Text(text))) => {
                    if let Ok(ClientMessage::Read { channel_id, message_id }) = serde_json::from_str(&text) {
                        mark_read(&state, user_id, channel_id, message_id).await;
                    }
                }
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
