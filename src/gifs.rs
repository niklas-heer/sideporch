//! GIFs: a local library by default, or search through GIPHY or KLIPY.
//!
//! The admin picks the source under Admin → GIFs.
//!
//! - **Local** (the default): GIFs that people add to the team's library,
//!   stored like other files. No outside service is involved.
//! - **GIPHY**: Sideporch searches on people's behalf, so the key stays on
//!   the server, and fetches a posted GIF again by id so a message can only
//!   show GIPHY's media. GIPHY's terms require showing its URLs unchanged,
//!   without caching the media, and crediting "Powered by GIPHY".
//! - **KLIPY**: its terms require API requests and media loads to come from
//!   people's browsers, so the page script searches KLIPY's Tenor-compatible
//!   API directly with the admin's key, and the server only accepts posted
//!   GIFs whose media is on KLIPY's servers. KLIPY's branding is shown too.
//!
//! Tenor closed its API in June 2026.

use std::{fmt::Write as _, time::Duration};

use axum::body::Bytes;
use http_body_util::{BodyExt as _, Empty, Limited};
use hyper_rustls::HttpsConnector;
use hyper_util::{
    client::legacy::{Client, connect::HttpConnector},
    rt::TokioExecutor,
};
use rusqlite::Connection;
use serde::Serialize;
use serde_json::Value;

use crate::{
    error::{AppError, AppResult},
    secrets::Vault,
    store::{self, Gif},
};

const TIMEOUT: Duration = Duration::from_secs(10);
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const RESULTS: u32 = 24;
pub const RATINGS: &[&str] = &["g", "pg", "pg-13", "r"];

const PROVIDER: &str = "gifs.provider";
const GIPHY_KEY: &str = "gifs.api_key";
const KLIPY_KEY: &str = "gifs.klipy_key";
const RATING: &str = "gifs.rating";

/// Where GIFs come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Provider {
    #[default]
    Local,
    Giphy,
    Klipy,
}

impl Provider {
    pub const fn key(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Giphy => "giphy",
            Self::Klipy => "klipy",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "local" => Some(Self::Local),
            "giphy" => Some(Self::Giphy),
            "klipy" => Some(Self::Klipy),
            _ => None,
        }
    }

    /// The credit the picker and messages show.
    pub const fn attribution(self) -> &'static str {
        match self {
            Self::Local => "",
            Self::Giphy => "Powered by GIPHY",
            Self::Klipy => "Powered by KLIPY",
        }
    }
}

/// How GIFs are set up.
#[derive(Debug, Clone, Default)]
pub struct Settings {
    pub provider: Provider,
    pub giphy_key: Option<String>,
    pub klipy_key: Option<String>,
    pub rating: String,
}

impl Settings {
    /// The active service's key.
    pub fn api_key(&self) -> Option<&str> {
        match self.provider {
            Provider::Local => None,
            Provider::Giphy => self.giphy_key.as_deref(),
            Provider::Klipy => self.klipy_key.as_deref(),
        }
    }

    /// KLIPY's content filter for the rating.
    pub fn klipy_filter(&self) -> &'static str {
        match self.rating.as_str() {
            "g" => "high",
            "pg-13" => "low",
            "r" => "off",
            _ => "medium",
        }
    }
}

/// The last characters of a key, for the settings page.
pub fn key_hint(key: &str) -> String {
    let count = key.chars().count();
    key.chars().skip(count.saturating_sub(4)).collect()
}

pub fn settings(conn: &Connection, vault: &Vault) -> AppResult<Settings> {
    let open = |name: &str| -> AppResult<Option<String>> {
        store::setting(conn, name)?
            .map(|sealed| vault.open_text(name, &sealed))
            .transpose()
    };
    let giphy_key = open(GIPHY_KEY)?;
    let klipy_key = open(KLIPY_KEY)?;
    let provider = store::setting(conn, PROVIDER)?
        .and_then(|key| Provider::from_key(&key))
        .unwrap_or_default();
    // A service without a key falls back to the local library.
    let provider = match provider {
        Provider::Giphy if giphy_key.is_none() => Provider::Local,
        Provider::Klipy if klipy_key.is_none() => Provider::Local,
        other => other,
    };
    Ok(Settings {
        provider,
        giphy_key,
        klipy_key,
        rating: store::setting(conn, RATING)?.unwrap_or_else(|| "pg".to_owned()),
    })
}

/// A change to the GIF settings; empty keys keep the saved ones.
pub struct Change<'a> {
    pub provider: Provider,
    pub giphy_key: Option<&'a str>,
    pub klipy_key: Option<&'a str>,
    pub rating: &'a str,
}

pub fn save(conn: &Connection, vault: &Vault, change: &Change<'_>) -> AppResult<()> {
    if !RATINGS.contains(&change.rating) {
        return Err(AppError::bad_request("Pick a rating: g, pg, pg-13 or r."));
    }
    for (name, key) in [(GIPHY_KEY, change.giphy_key), (KLIPY_KEY, change.klipy_key)] {
        if let Some(key) = key {
            store::set_setting(conn, name, &vault.seal_text(name, key)?)?;
        }
    }
    let needed = match change.provider {
        Provider::Local => None,
        Provider::Giphy => Some((GIPHY_KEY, "GIPHY")),
        Provider::Klipy => Some((KLIPY_KEY, "KLIPY")),
    };
    if let Some((name, service)) = needed
        && store::setting(conn, name)?.is_none()
    {
        return Err(AppError::bad_request(format!("Enter a {service} API key.")));
    }
    store::set_setting(conn, PROVIDER, change.provider.key())?;
    store::set_setting(conn, RATING, change.rating)
}

/// Whether a posted KLIPY media URL is on KLIPY's servers.
pub fn is_klipy_media(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    (host == "klipy.com" || host.ends_with(".klipy.com"))
        && !url
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '"')
}

/// One search result, for the picker.
#[derive(Debug, Clone, Serialize)]
pub struct Found {
    pub id: String,
    pub title: String,
    pub preview: String,
    pub width: u32,
    pub height: u32,
}

type HttpClient = Client<HttpsConnector<HttpConnector>, Empty<Bytes>>;

pub struct Gifs {
    client: HttpClient,
    /// Where the API lives; tests point it at a local fake.
    base: String,
}

fn number(value: &Value) -> u32 {
    value
        .as_str()
        .and_then(|text| text.parse().ok())
        .or_else(|| value.as_u64().and_then(|number| u32::try_from(number).ok()))
        .unwrap_or(0)
}

impl Gifs {
    pub fn new(base: Option<String>) -> AppResult<Self> {
        let connector = hyper_rustls::HttpsConnectorBuilder::new()
            .with_provider_and_webpki_roots(rustls::crypto::ring::default_provider())
            .map_err(AppError::internal)?;
        let connector = if base.is_some() {
            connector.https_or_http()
        } else {
            connector.https_only()
        }
        .enable_http1()
        .enable_http2()
        .build();
        Ok(Self {
            client: Client::builder(TokioExecutor::new()).build(connector),
            base: base.unwrap_or_else(|| "https://api.giphy.com".to_owned()),
        })
    }

    async fn get(&self, path_and_query: &str) -> AppResult<Value> {
        let request = axum::http::Request::get(format!("{}{path_and_query}", self.base))
            .body(Empty::new())
            .map_err(AppError::internal)?;
        let response = tokio::time::timeout(TIMEOUT, self.client.request(request))
            .await
            .map_err(|_| AppError::bad_request("GIPHY did not answer in time."))?
            .map_err(|error| AppError::bad_request(format!("Could not reach GIPHY: {error}")))?;
        let status = response.status();
        let bytes = Limited::new(response.into_body(), MAX_RESPONSE_BYTES)
            .collect()
            .await
            .map_err(|_| AppError::bad_request("GIPHY's answer was too large."))?
            .to_bytes();
        if !status.is_success() {
            return Err(AppError::bad_request(format!(
                "GIPHY answered {status}. Check the API key under GIFs."
            )));
        }
        serde_json::from_slice(&bytes).map_err(AppError::internal)
    }

    /// Trending GIFs for an empty query, search results otherwise.
    pub async fn search(
        &self,
        settings: &Settings,
        query: &str,
        offset: u32,
    ) -> AppResult<Vec<Found>> {
        let query = query.trim();
        let mut path = if query.is_empty() {
            "/v1/gifs/trending?".to_owned()
        } else {
            format!(
                "/v1/gifs/search?q={}&",
                encode(&query.chars().take(50).collect::<String>())
            )
        };
        // Writing to a String cannot fail.
        let _ = write!(
            path,
            "api_key={}&limit={RESULTS}&offset={}&rating={}&bundle=messaging_non_clips",
            encode(settings.giphy_key.as_deref().unwrap_or_default()),
            offset.min(4_999),
            encode(&settings.rating)
        );
        let answer = self.get(&path).await?;
        Ok(answer
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|gif| {
                let preview = gif.pointer("/images/fixed_width")?;
                Some(Found {
                    id: gif.get("id")?.as_str()?.to_owned(),
                    title: gif
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or("GIF")
                        .to_owned(),
                    preview: preview.get("url")?.as_str()?.to_owned(),
                    width: number(preview.get("width")?),
                    height: number(preview.get("height")?),
                })
            })
            .collect())
    }

    /// One GIF by id, with the rendition messages show.
    pub async fn gif(&self, settings: &Settings, id: &str) -> AppResult<Gif> {
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(AppError::bad_request("That is not a GIPHY id."));
        }
        let answer = self
            .get(&format!(
                "/v1/gifs/{id}?api_key={}",
                encode(settings.giphy_key.as_deref().unwrap_or_default())
            ))
            .await?;
        let data = answer
            .get("data")
            .ok_or_else(|| AppError::bad_request("GIPHY has no GIF with that id."))?;
        let rendition = data
            .pointer("/images/downsized_medium")
            .filter(|rendition| rendition.get("url").is_some())
            .or_else(|| data.pointer("/images/original"))
            .ok_or_else(|| AppError::bad_request("GIPHY sent no image for that GIF."))?;
        let url = rendition
            .get("url")
            .and_then(Value::as_str)
            .filter(|url| url.starts_with("https://"))
            .ok_or_else(|| AppError::bad_request("GIPHY sent no image for that GIF."))?;
        Ok(Gif {
            provider: "giphy".to_owned(),
            id: id.to_owned(),
            title: data
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("GIF")
                .to_owned(),
            url: url.to_owned(),
            width: rendition.get("width").map_or(0, number),
            height: rendition.get("height").map_or(0, number),
        })
    }
}

/// Percent-encodes a query parameter value.
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_queries() {
        assert_eq!(encode("happy dance & more"), "happy%20dance%20%26%20more");
        assert_eq!(number(&serde_json::json!("200")), 200);
        assert_eq!(number(&serde_json::json!(120)), 120);
    }

    #[test]
    fn accepts_only_klipy_media() {
        assert!(is_klipy_media("https://static.klipy.com/ii/a.gif"));
        assert!(is_klipy_media("https://klipy.com/a.gif"));
        assert!(!is_klipy_media("http://static.klipy.com/a.gif"));
        assert!(!is_klipy_media("https://klipy.com.evil.example/a.gif"));
        assert!(!is_klipy_media("https://evilklipy.com/a.gif"));
        assert!(!is_klipy_media(
            "https://static.klipy.com/a.gif\" onerror=\"x"
        ));
    }
}
