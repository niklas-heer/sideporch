//! Connecting Sideporch servers, and sharing channels and conversations
//! between them.
//!
//! Two servers connect when an admin on one asks and an admin on the other
//! accepts. Each has an Ed25519 key it publishes at
//! `/.well-known/sideporch`; a request to connect is signed with it, and
//! the receiver checks the key against the one the asking server
//! publishes. Once connected, both pin each other's key and sign every
//! request (see [`keys`]).
//!
//! People from another server appear here as accounts that can't sign in,
//! named `name@server`. A shared channel lives on its host server, which
//! passes everything on to the servers it shares the channel with; each of
//! those keeps a copy. A server may only speak for its own people.

pub mod data;
pub mod events;
pub mod inbound;
pub mod keys;
pub mod outbound;
pub mod outbox;

use std::sync::Arc;

use base64ct::{Base64, Encoding as _};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::{
    automations::http::Http,
    error::{AppError, AppResult},
    now_ms,
    secrets::Vault,
};

/// Answers larger than this from another server are refused.
const MAX_ANSWER: usize = 1024 * 1024;

/// What a server publishes about itself at `/.well-known/sideporch`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Description {
    pub software: String,
    pub version: String,
    pub url: String,
    pub name: String,
    /// Its Ed25519 public key, in base64.
    pub public_key: String,
}

impl Description {
    pub fn key(&self) -> Result<Vec<u8>, String> {
        Base64::decode_vec(&self.public_key)
            .ok()
            .filter(|key| key.len() == 32)
            .ok_or_else(|| "the server's key isn't valid".to_owned())
    }
}

/// The part of a server's URL its people's names carry: `chat.example.org`,
/// or `127.0.0.1:8080` with a port.
pub fn handle_of(url: &str) -> Option<String> {
    let uri: axum::http::Uri = url.parse().ok()?;
    let host = uri.host()?.to_ascii_lowercase();
    Some(match uri.port_u16() {
        Some(port) => format!("{host}:{port}"),
        None => host,
    })
}

/// Normalizes a server's URL as people type it: `chat.example.org/` becomes
/// `https://chat.example.org`.
pub fn normalize_url(text: &str) -> Option<String> {
    let text = text.trim().trim_end_matches('/');
    let url = if text.starts_with("http://") || text.starts_with("https://") {
        text.to_owned()
    } else {
        format!("https://{text}")
    };
    let uri: axum::http::Uri = url.parse().ok()?;
    (uri.host().is_some() && uri.path().trim_end_matches('/').is_empty()).then_some(url)
}

pub struct Federation {
    identity: keys::Identity,
    /// Where people reach this server; federation needs it.
    url: Option<String>,
    http: Http,
    /// Wakes the outbox when there's something to send.
    pub wake: tokio::sync::Notify,
}

impl std::fmt::Debug for Federation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Federation")
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

impl Federation {
    /// Opens this server's key, `sealed` as stored, or makes one the first
    /// time and returns it sealed, to store. Must be called inside the Tokio
    /// runtime.
    pub fn open(
        sealed: Option<&str>,
        vault: &Vault,
        url: Option<String>,
        allow_private: bool,
    ) -> AppResult<(Self, Option<String>)> {
        let (identity, new) = if let Some(sealed) = sealed {
            let encoded = vault.open_text("federation.key", sealed)?;
            let pkcs8 = Base64::decode_vec(&encoded)
                .map_err(|_| AppError::internal("the stored server key is damaged"))?;
            (keys::Identity::from_pkcs8(&pkcs8)?, None)
        } else {
            let (identity, pkcs8) = keys::Identity::generate()?;
            let sealed = vault.seal_text("federation.key", &Base64::encode_string(&pkcs8))?;
            (identity, Some(sealed))
        };
        Ok((
            Self {
                identity,
                url,
                http: Http::new(allow_private)?,
                wake: tokio::sync::Notify::new(),
            },
            new,
        ))
    }

    /// This server's URL, without which it can't connect to others.
    pub fn url(&self) -> Option<&str> {
        self.url.as_deref()
    }

    pub fn handle(&self) -> Option<String> {
        self.url.as_deref().and_then(handle_of)
    }

    pub fn public_key(&self) -> &[u8] {
        self.identity.public_key()
    }

    pub fn fingerprint(&self) -> String {
        keys::fingerprint(self.public_key())
    }

    pub fn description(&self, name: &str) -> Option<Description> {
        Some(Description {
            software: "sideporch".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            url: self.url.clone()?,
            name: name.to_owned(),
            public_key: Base64::encode_string(self.public_key()),
        })
    }

    /// Reads what the server at `url` publishes about itself.
    pub async fn describe(&self, url: &str) -> Result<Description, String> {
        let (status, body) = self
            .http
            .call(
                "GET",
                &format!("{url}/.well-known/sideporch"),
                &[],
                Vec::new(),
                MAX_ANSWER,
            )
            .await?;
        if status != 200 {
            return Err(format!(
                "{url} doesn't look like a Sideporch server (it answered {status})"
            ));
        }
        let description: Description = serde_json::from_slice(&body)
            .map_err(|_| format!("{url} doesn't look like a Sideporch server"))?;
        if description.software != "sideporch" {
            return Err(format!("{url} doesn't look like a Sideporch server"));
        }
        if description.url.trim_end_matches('/') != url {
            return Err(format!("{url} says its address is {}", description.url));
        }
        description.key()?;
        Ok(description)
    }

    /// Sends a signed request to the server at `target` and returns its
    /// answer.
    pub async fn send(
        &self,
        method: &str,
        target: &str,
        path: &str,
        body: Vec<u8>,
    ) -> Result<(u16, Vec<u8>), String> {
        self.send_limited(method, target, path, body, MAX_ANSWER)
            .await
    }

    /// Like [`Self::send`], taking up to `limit` bytes of answer.
    pub async fn send_limited(
        &self,
        method: &str,
        target: &str,
        path: &str,
        body: Vec<u8>,
        limit: usize,
    ) -> Result<(u16, Vec<u8>), String> {
        let sender = self.url.as_deref().ok_or("this server has no public URL")?;
        let mut headers = self
            .identity
            .sign_request(method, path, target, sender, &body, now_ms())
            .map_err(|error| error.to_string())?;
        if !body.is_empty() {
            headers.push(("content-type", "application/json".to_owned()));
        }
        self.http
            .call(method, &format!("{target}{path}"), &headers, body, limit)
            .await
    }

    /// Checks a request another server signed, and returns that server if
    /// it is one we know in one of `statuses` and the request holds.
    pub fn verify(
        &self,
        conn: &Connection,
        headers: &axum::http::HeaderMap,
        method: &str,
        path: &str,
        body: &[u8],
        statuses: &[data::Status],
    ) -> Result<data::Instance, String> {
        let signed =
            keys::Signed::read(|name| headers.get(name).and_then(|value| value.to_str().ok()))?;
        let instance = data::instance_by_url(conn, &signed.server)
            .map_err(|error| error.to_string())?
            .filter(|instance| statuses.contains(&instance.status))
            .ok_or("this server doesn't know the one sending the request")?;
        let key = instance
            .public_key
            .as_deref()
            .ok_or("this server has no key for the one sending the request")?;
        let target = self.url.as_deref().ok_or("this server has no public URL")?;
        signed.check(key, method, path, target, body, now_ms())?;
        if !data::remember_nonce(conn, instance.id, &signed.nonce, now_ms())
            .map_err(|error| error.to_string())?
        {
            return Err("the request was sent before".to_owned());
        }
        Ok(instance)
    }

    /// Checks a request signed with `public_key`, for a server we don't
    /// know yet: asking to connect.
    pub fn verify_with(
        &self,
        headers: &axum::http::HeaderMap,
        public_key: &[u8],
        method: &str,
        path: &str,
        body: &[u8],
    ) -> Result<String, String> {
        let signed =
            keys::Signed::read(|name| headers.get(name).and_then(|value| value.to_str().ok()))?;
        let target = self.url.as_deref().ok_or("this server has no public URL")?;
        signed.check(public_key, method, path, target, body, now_ms())?;
        Ok(signed.server)
    }
}

pub type Shared = Arc<Federation>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_and_urls() {
        assert_eq!(
            handle_of("https://Chat.Example.org").as_deref(),
            Some("chat.example.org")
        );
        assert_eq!(
            handle_of("http://127.0.0.1:8080").as_deref(),
            Some("127.0.0.1:8080")
        );
        assert_eq!(
            normalize_url("chat.example.org/").as_deref(),
            Some("https://chat.example.org")
        );
        assert_eq!(
            normalize_url("http://127.0.0.1:8080").as_deref(),
            Some("http://127.0.0.1:8080")
        );
        assert_eq!(normalize_url("https://chat.example.org/team"), None);
        assert_eq!(normalize_url("not a url"), None);
    }
}
