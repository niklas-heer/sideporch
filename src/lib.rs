//! Sideporch: a small, self-hosted team chat in one binary.
//!
//! [`Sideporch::open`] prepares the data directory and database;
//! [`Sideporch::router`] is the complete web application.

mod assets;
mod auth;
mod db;
mod error;
mod icons;
mod markup;
mod realtime;
mod routes;
mod store;
mod views;
mod webhook;

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use axum::Router;
use sha2::{Digest, Sha256};

pub use crate::error::AppError as Error;
use crate::{db::Db, realtime::Hub};

/// Where Sideporch keeps its data and how people reach it.
#[derive(Debug, Clone)]
pub struct Config {
    /// Directory holding `sideporch.db`. Created if missing.
    pub data_dir: PathBuf,
    /// The public base URL, such as `https://chat.example.com`. Used for
    /// invite and webhook links; derived from each request when unset.
    pub public_url: Option<String>,
}

#[derive(Clone)]
pub(crate) struct AppState {
    db: Db,
    hub: Hub,
    public_url: Option<String>,
    secure_cookies: bool,
    /// One-time token for creating the first account, while none exists.
    setup_token: Arc<Mutex<Option<String>>>,
}

impl AppState {
    fn setup_pending(&self) -> bool {
        self.setup_token.lock().is_ok_and(|token| token.is_some())
    }

    fn setup_token_matches(&self, candidate: &str) -> bool {
        self.setup_token.lock().is_ok_and(|token| {
            token
                .as_deref()
                .is_some_and(|token| Sha256::digest(token) == Sha256::digest(candidate))
        })
    }

    fn finish_setup(&self) {
        if let Ok(mut token) = self.setup_token.lock() {
            *token = None;
        }
    }
}

/// An opened Sideporch instance.
pub struct Sideporch {
    state: AppState,
}

impl Sideporch {
    /// Opens (or creates) the instance in `config.data_dir`.
    ///
    /// # Errors
    ///
    /// Fails if the data directory or database cannot be opened.
    pub async fn open(config: Config) -> Result<Self, Error> {
        std::fs::create_dir_all(&config.data_dir).map_err(Error::internal)?;
        let db = Db::open(&config.data_dir.join("sideporch.db"))?;
        let now = now_ms();
        let users = db
            .call(move |conn| {
                store::delete_expired_sessions(conn, now)?;
                store::user_count(conn)
            })
            .await?;
        let setup_token = if users == 0 {
            Some(auth::random_token()?)
        } else {
            None
        };
        let public_url = config
            .public_url
            .map(|url| url.trim_end_matches('/').to_owned())
            .filter(|url| !url.is_empty());
        Ok(Self {
            state: AppState {
                db,
                hub: Hub::default(),
                secure_cookies: public_url
                    .as_deref()
                    .is_some_and(|url| url.starts_with("https://")),
                public_url,
                setup_token: Arc::new(Mutex::new(setup_token)),
            },
        })
    }

    /// The path of the one-time setup page, while no account exists yet.
    #[must_use]
    pub fn setup_path(&self) -> Option<String> {
        self.state
            .setup_token
            .lock()
            .ok()?
            .as_ref()
            .map(|token| format!("/setup/{token}"))
    }

    /// The configured public URL, if any.
    #[must_use]
    pub fn public_url(&self) -> Option<&str> {
        self.state.public_url.as_deref()
    }

    /// The web application: pages, WebSocket, webhooks and assets.
    pub fn router(&self) -> Router {
        routes::router(self.state.clone())
    }
}

pub(crate) fn now_ms() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}
