//! Sideporch: a small, self-hosted team chat in one binary.
//!
//! [`Sideporch::open`] prepares the data directory and database;
//! [`Sideporch::router`] is the complete web application.

mod assets;
mod auth;
mod automations;
mod db;
mod error;
mod files;
mod icons;
mod markup;
mod messages;
mod push;
mod realtime;
mod routes;
mod search;
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
use crate::{automations::Automations, db::Db, push::Push, realtime::Hub};

/// Where Sideporch keeps its data and how people reach it.
#[derive(Debug, Clone)]
pub struct Config {
    /// Directory holding `sideporch.db`. Created if missing.
    pub data_dir: PathBuf,
    /// The public base URL, such as `https://chat.example.com`. Used for
    /// invite and webhook links; derived from each request when unset.
    pub public_url: Option<String>,
    /// Accept plain-HTTP push endpoints. Browsers only use HTTPS ones; this
    /// exists so tests can run a local push service.
    #[doc(hidden)]
    pub allow_insecure_push: bool,
}

#[derive(Clone)]
pub(crate) struct AppState {
    db: Db,
    hub: Hub,
    push: Arc<Push>,
    automations: Automations,
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
        let db_path = config.data_dir.join("sideporch.db");
        let db = Db::open(&db_path)?;
        let now = now_ms();
        let public_url = config
            .public_url
            .map(|url| url.trim_end_matches('/').to_owned())
            .filter(|url| !url.is_empty());
        // Push services use this to reach whoever runs the server.
        let subject = public_url
            .clone()
            .filter(|url| url.starts_with("https://"))
            .unwrap_or_else(|| "https://github.com/niklas-heer/sideporch".to_owned());
        let allow_http = config.allow_insecure_push;
        let (users, push) = db
            .call(move |conn| {
                store::delete_expired_sessions(conn, now)?;
                Ok((
                    store::user_count(conn)?,
                    Push::load(conn, subject, allow_http)?,
                ))
            })
            .await?;
        let setup_token = if users == 0 {
            Some(auth::random_token()?)
        } else {
            None
        };
        let state = AppState {
            db,
            hub: Hub::default(),
            push: Arc::new(push),
            automations: Automations::start(&db_path)?,
            secure_cookies: public_url
                .as_deref()
                .is_some_and(|url| url.starts_with("https://")),
            public_url,
            setup_token: Arc::new(Mutex::new(setup_token)),
        };
        state.automations.serve(state.clone());
        state.automations.reload(&state).await?;
        Ok(Self { state })
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
