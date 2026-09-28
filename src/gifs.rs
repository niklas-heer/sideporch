//! GIF search through GIPHY.
//!
//! An admin enables it with a GIPHY API key. People search from the
//! composer; Sideporch asks GIPHY on their behalf, so the key stays on the
//! server, and a posted GIF is fetched again by id so a message can only
//! show GIPHY's own media. GIPHY's terms require showing its URLs as they
//! are, without caching the media, and crediting "Powered by GIPHY"; the
//! picker and messages do both. (Tenor closed its API in June 2026.)

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
const API_KEY: &str = "gifs.api_key";
const RATING: &str = "gifs.rating";

/// How GIF search is set up.
#[derive(Debug, Clone)]
pub struct Settings {
    pub api_key: String,
    pub rating: String,
}

impl Settings {
    /// The key's last characters, for the settings page.
    pub fn key_hint(&self) -> String {
        let count = self.api_key.chars().count();
        self.api_key.chars().skip(count.saturating_sub(4)).collect()
    }
}

pub fn configured(conn: &Connection) -> AppResult<bool> {
    Ok(store::setting(conn, PROVIDER)?.is_some())
}

pub fn settings(conn: &Connection, vault: &Vault) -> AppResult<Option<Settings>> {
    if store::setting(conn, PROVIDER)?.is_none() {
        return Ok(None);
    }
    let Some(sealed) = store::setting(conn, API_KEY)? else {
        return Ok(None);
    };
    Ok(Some(Settings {
        api_key: vault.open_text(API_KEY, &sealed)?,
        rating: store::setting(conn, RATING)?.unwrap_or_else(|| "pg".to_owned()),
    }))
}

pub fn save(
    conn: &Connection,
    vault: &Vault,
    api_key: Option<&str>,
    rating: &str,
) -> AppResult<()> {
    if !RATINGS.contains(&rating) {
        return Err(AppError::bad_request("Pick a rating: g, pg, pg-13 or r."));
    }
    if let Some(key) = api_key {
        store::set_setting(conn, API_KEY, &vault.seal_text(API_KEY, key)?)?;
    } else if store::setting(conn, API_KEY)?.is_none() {
        return Err(AppError::bad_request("Enter a GIPHY API key."));
    }
    store::set_setting(conn, PROVIDER, "giphy")?;
    store::set_setting(conn, RATING, rating)
}

pub fn remove(conn: &Connection) -> AppResult<()> {
    for key in [PROVIDER, API_KEY, RATING] {
        store::delete_setting(conn, key)?;
    }
    Ok(())
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
            encode(&settings.api_key),
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
                encode(&settings.api_key)
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
}
