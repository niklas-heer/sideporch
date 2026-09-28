//! Web Push notifications (RFC 8030, 8291 and 8292).
//!
//! Browsers subscribe through the service worker in `assets/sw.js`. The
//! server signs requests with its own VAPID key, stored in the database,
//! and sends encrypted payloads over HTTPS using bundled Mozilla root
//! certificates, so it needs nothing from the host system.

use std::time::Duration;

use axum::{
    Json,
    body::Bytes,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use base64ct::{Base64UrlUnpadded, Encoding as _};
use http_body_util::Full;
use hyper_rustls::HttpsConnector;
use hyper_util::{
    client::legacy::{Client, connect::HttpConnector},
    rt::TokioExecutor,
};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use web_push_native::{
    Auth, WebPushBuilder, jwt_simple::algorithms::ECDSAP256PublicKeyLike as _,
    jwt_simple::algorithms::ES256KeyPair, p256::PublicKey,
};

use crate::{
    AppState,
    auth::CurrentUser,
    error::{AppError, AppResult},
    now_ms,
    store::{self, Author, Message, PushSubscription},
};

const KEY_SETTING: &str = "vapid_private_key";

type HttpClient = Client<HttpsConnector<HttpConnector>, Full<Bytes>>;

pub struct Push {
    key: ES256KeyPair,
    /// The VAPID public key browsers need, base64url-encoded.
    pub public_key: String,
    subject: String,
    client: HttpClient,
    allow_http: bool,
}

impl Push {
    /// Loads the VAPID key, creating one on first start.
    ///
    /// `subject` identifies this server to push services (an `https:` or
    /// `mailto:` URL). `allow_http` permits plain-HTTP push endpoints and
    /// exists for tests only.
    pub fn load(conn: &Connection, subject: String, allow_http: bool) -> AppResult<Self> {
        let key = if let Some(encoded) = store::setting(conn, KEY_SETTING)? {
            Base64UrlUnpadded::decode_vec(&encoded)
                .ok()
                .and_then(|raw| ES256KeyPair::from_bytes(&raw).ok())
                .ok_or_else(|| AppError::internal("stored VAPID key is invalid"))?
        } else {
            let key = ES256KeyPair::generate();
            store::set_setting(
                conn,
                KEY_SETTING,
                &Base64UrlUnpadded::encode_string(&key.to_bytes()),
            )?;
            key
        };
        let public_key = Base64UrlUnpadded::encode_string(
            &key.public_key().public_key().to_bytes_uncompressed(),
        );
        let tls = hyper_rustls::HttpsConnectorBuilder::new()
            .with_provider_and_webpki_roots(rustls::crypto::ring::default_provider())
            .map_err(AppError::internal)?;
        let tls = if allow_http {
            tls.https_or_http()
        } else {
            tls.https_only()
        };
        let connector = tls.enable_http1().enable_http2().build();
        Ok(Self {
            key,
            public_key,
            subject,
            client: Client::builder(TokioExecutor::new()).build(connector),
            allow_http,
        })
    }

    fn accepts_endpoint(&self, endpoint: &str) -> bool {
        endpoint.starts_with("https://") || (self.allow_http && endpoint.starts_with("http://"))
    }

    async fn send(&self, subscription: &PushSubscription, payload: &[u8]) -> Delivery {
        match self.try_send(subscription, payload).await {
            Ok(status) if status.is_success() => Delivery::Sent,
            // The browser unsubscribed or the subscription expired.
            Ok(StatusCode::NOT_FOUND | StatusCode::GONE) => Delivery::Expired,
            Ok(status) => Delivery::Failed(format!("push service answered {status}")),
            Err(error) => Delivery::Failed(error.to_string()),
        }
    }

    async fn try_send(
        &self,
        subscription: &PushSubscription,
        payload: &[u8],
    ) -> Result<StatusCode, Box<dyn std::error::Error + Send + Sync>> {
        let p256dh = Base64UrlUnpadded::decode_vec(&subscription.p256dh)
            .map_err(|_| "invalid p256dh key")?;
        let auth =
            Base64UrlUnpadded::decode_vec(&subscription.auth).map_err(|_| "invalid auth secret")?;
        if auth.len() != 16 {
            return Err("invalid auth secret".into());
        }
        let request = WebPushBuilder::new(
            subscription.endpoint.parse()?,
            PublicKey::from_sec1_bytes(&p256dh)?,
            Auth::clone_from_slice(&auth),
        )
        .with_valid_duration(Duration::from_secs(24 * 60 * 60))
        .with_vapid(&self.key, &self.subject)
        .build(payload.to_vec())?;
        let (parts, body) = request.into_parts();
        let request = axum::http::Request::from_parts(parts, Full::new(Bytes::from(body)));
        let response =
            tokio::time::timeout(Duration::from_secs(15), self.client.request(request)).await??;
        Ok(response.status())
    }
}

enum Delivery {
    Sent,
    Expired,
    Failed(String),
}

/// The JSON the service worker turns into a notification.
#[derive(Serialize, Clone)]
struct Payload {
    title: String,
    body: String,
    url: String,
    tag: String,
    /// When the message was sent, in milliseconds.
    timestamp: i64,
    /// Unread conversations for the app icon; set per person.
    badge: usize,
}

/// Notifies `targets` about `message` in the background, skipping people
/// who are looking at Sideporch right now.
pub fn notify(state: &AppState, message: &Message, targets: Vec<i64>) {
    let targets: Vec<i64> = targets
        .into_iter()
        .filter(|user| !state.hub.is_watching(*user))
        .collect();
    if targets.is_empty() {
        return;
    }
    let author = match &message.author {
        Author::User { display_name, .. } => display_name.clone(),
        Author::Bot { name, .. } => name.clone(),
        Author::Removed => "Someone".to_owned(),
    };
    let mut body: String = if message.body.is_empty() {
        message.files.first().map_or_else(
            || "New message".to_owned(),
            |file| format!("Shared {}", file.name),
        )
    } else {
        message.body.chars().take(180).collect()
    };
    if message.body.chars().count() > 180 {
        body.push('…');
    }
    let url = message.parent_id.map_or_else(
        || format!("/c/{}", message.channel_id),
        |parent| format!("/c/{}/t/{parent}", message.channel_id),
    );
    let payload = Payload {
        title: author,
        body,
        url,
        tag: format!("channel-{}", message.channel_id),
        timestamp: message.created_at,
        badge: 0,
    };
    let state = state.clone();
    tokio::spawn(async move {
        if let Err(error) = deliver(&state, targets, &payload).await {
            tracing::warn!(?error, "could not send push notifications");
        }
    });
}

async fn deliver(state: &AppState, targets: Vec<i64>, payload: &Payload) -> AppResult<()> {
    let (subscriptions, unread) = state
        .db
        .call(move |conn| {
            let subscriptions = store::push_subscriptions(conn, &targets)?;
            let unread = targets
                .iter()
                .map(|user| Ok((*user, store::unread_count(conn, *user)?)))
                .collect::<AppResult<std::collections::HashMap<i64, usize>>>()?;
            Ok((subscriptions, unread))
        })
        .await?;
    for subscription in subscriptions {
        let payload = serde_json::to_vec(&Payload {
            badge: unread.get(&subscription.user_id).copied().unwrap_or(0),
            ..payload.clone()
        })
        .map_err(AppError::internal)?;
        match state.push.send(&subscription, &payload).await {
            Delivery::Sent => {}
            Delivery::Expired => {
                let endpoint = subscription.endpoint.clone();
                state
                    .db
                    .call(move |conn| store::delete_push_subscription(conn, &endpoint, None))
                    .await?;
            }
            Delivery::Failed(error) => tracing::warn!(%error, "push delivery failed"),
        }
    }
    Ok(())
}

// Endpoints for the browser.

#[derive(Deserialize)]
pub struct SubscriptionJson {
    endpoint: String,
    #[serde(default)]
    keys: Option<SubscriptionKeys>,
}

#[derive(Deserialize)]
pub struct SubscriptionKeys {
    p256dh: String,
    auth: String,
}

pub async fn public_key(_: CurrentUser, State(state): State<AppState>) -> Response {
    state.push.public_key.clone().into_response()
}

pub async fn subscribe(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(subscription): Json<SubscriptionJson>,
) -> AppResult<StatusCode> {
    let keys = subscription
        .keys
        .ok_or_else(|| AppError::bad_request("The subscription has no keys."))?;
    let valid_keys = Base64UrlUnpadded::decode_vec(&keys.p256dh)
        .ok()
        .is_some_and(|key| PublicKey::from_sec1_bytes(&key).is_ok())
        && Base64UrlUnpadded::decode_vec(&keys.auth).is_ok_and(|auth| auth.len() == 16);
    if !valid_keys
        || subscription.endpoint.len() > 2048
        || !state.push.accepts_endpoint(&subscription.endpoint)
    {
        return Err(AppError::bad_request("That push subscription isn't valid."));
    }
    let record = PushSubscription {
        endpoint: subscription.endpoint,
        user_id: user.id,
        p256dh: keys.p256dh,
        auth: keys.auth,
    };
    let now = now_ms();
    state
        .db
        .call(move |conn| store::save_push_subscription(conn, &record, now))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn unsubscribe(
    user: CurrentUser,
    State(state): State<AppState>,
    Json(subscription): Json<SubscriptionJson>,
) -> AppResult<StatusCode> {
    let user_id = user.id;
    state
        .db
        .call(move |conn| {
            store::delete_push_subscription(conn, &subscription.endpoint, Some(user_id))
        })
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
