//! Sideporch: a small, self-hosted team chat in one binary.
//!
//! [`Sideporch::open`] prepares the data directory and database;
//! [`Sideporch::router`] is the complete web application.

mod ai;
mod assets;
mod auth;
mod automations;
mod backup;
mod blobs;
mod db;
mod emoji;
mod error;
mod files;
mod gifs;
mod icons;
mod import;
mod later;
mod markdown;
mod markup;
mod mcp;
mod messages;
mod previews;
mod push;
mod realtime;
mod routes;
mod search;
mod secrets;
mod store;
mod system;
mod views;
mod webhook;

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use axum::Router;
use sha2::{Digest, Sha256};

use crate::{ai::Ai, automations::Automations, db::Db, push::Push, realtime::Hub, secrets::Vault};
pub use crate::{backup::restore, error::AppError as Error};

/// Where Sideporch keeps its data and how people reach it.
#[derive(Debug, Clone)]
pub struct Config {
    /// Directory holding `sideporch.db`. Created if missing.
    pub data_dir: PathBuf,
    /// The public base URL, such as `https://chat.example.com`. Used for
    /// invite and webhook links; derived from each request when unset.
    pub public_url: Option<String>,
    /// Require the one-time setup link to create the first account. By
    /// default the first person to open Sideporch creates it; lock setup
    /// when the server is reachable by others before you set it up.
    pub require_setup_link: bool,
    /// Where the GIPHY API lives; tests point it at a local fake.
    #[doc(hidden)]
    pub gif_api_base: Option<String>,
    /// Accept plain-HTTP push endpoints. Browsers only use HTTPS ones; this
    /// exists so tests can run a local push service.
    #[doc(hidden)]
    pub allow_insecure_push: bool,
    /// Let link previews reach private addresses; tests serve pages locally.
    #[doc(hidden)]
    pub allow_private_link_previews: bool,
}

#[derive(Clone)]
pub(crate) struct AppState {
    db: Db,
    hub: Hub,
    push: Arc<Push>,
    ai: Arc<Ai>,
    vault: Arc<Vault>,
    blobs: blobs::Blobs,
    gifs: Arc<gifs::Gifs>,
    monitor: Arc<system::Monitor>,
    /// Fetches link previews, guarded like automations' requests.
    links: Arc<automations::http::Http>,
    data_dir: PathBuf,
    automations: Automations,
    public_url: Option<String>,
    secure_cookies: bool,
    /// How the first account can be created, while none exists.
    setup: Arc<Mutex<Setup>>,
    /// Where the setup link is kept for `sideporch setup-link`.
    setup_file: PathBuf,
}

/// Whether and how the first account can still be created.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Setup {
    Done,
    /// Whoever opens `/setup` first creates the admin account.
    Open,
    /// Only the one-time link `/setup/<token>` works.
    Link(String),
}

impl AppState {
    fn setup(&self) -> Setup {
        self.setup.lock().map_or(Setup::Done, |setup| setup.clone())
    }

    fn setup_token_matches(&self, candidate: &str) -> bool {
        matches!(self.setup(), Setup::Link(token)
            if Sha256::digest(&token) == Sha256::digest(candidate))
    }

    fn finish_setup(&self) {
        if let Ok(mut setup) = self.setup.lock() {
            *setup = Setup::Done;
        }
        if let Err(error) = std::fs::remove_file(&self.setup_file)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(%error, "could not remove the used setup link");
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
        let blobs = blobs::Blobs::open(&config.data_dir)?;
        let store_blobs = blobs.clone();
        let (moved, removed) = db
            .call(move |conn| {
                Ok((
                    store_blobs.migrate(conn)?,
                    store_blobs.collect_garbage(conn)?,
                ))
            })
            .await?;
        if moved > 0 || removed > 0 {
            tracing::info!(moved, removed, "tidied file storage in the data directory");
        }
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
        let setup = if users > 0 {
            Setup::Done
        } else if config.require_setup_link {
            Setup::Link(auth::random_token()?)
        } else {
            Setup::Open
        };
        let state = AppState {
            db,
            hub: Hub::default(),
            push: Arc::new(push),
            ai: Arc::new(Ai::new()?),
            vault: Arc::new(Vault::open(&config.data_dir)?),
            blobs,
            gifs: Arc::new(gifs::Gifs::new(config.gif_api_base.clone())?),
            monitor: system::Monitor::start(),
            links: Arc::new(automations::http::Http::new(
                config.allow_private_link_previews,
            )?),
            data_dir: config.data_dir.clone(),
            automations: Automations::start(&db_path),
            secure_cookies: public_url
                .as_deref()
                .is_some_and(|url| url.starts_with("https://")),
            public_url,
            setup: Arc::new(Mutex::new(setup)),
            setup_file: setup_link_file(&config.data_dir),
        };
        state.automations.serve(state.clone());
        later::start(state.clone());
        backup::start(state.clone());
        state.automations.reload(&state).await?;
        Ok(Self { state })
    }

    /// The path of the setup page, while no account exists yet: `/setup`,
    /// or the one-time `/setup/<token>` when setup requires the link.
    #[must_use]
    pub fn setup_path(&self) -> Option<String> {
        match self.state.setup() {
            Setup::Done => None,
            Setup::Open => Some("/setup".to_owned()),
            Setup::Link(token) => Some(format!("/setup/{token}")),
        }
    }

    /// Whether the setup path is a secret one-time link.
    #[must_use]
    pub fn setup_is_secret(&self) -> bool {
        matches!(self.state.setup(), Setup::Link(_))
    }

    /// Saves the setup link, built on `base_url`, to a file only the
    /// server's user can read, and returns the file. A one-time link grants
    /// the first admin account, so it stays out of shared logs. Returns
    /// `None` once an account exists.
    ///
    /// # Errors
    ///
    /// Fails if the file cannot be written.
    pub fn save_setup_link(&self, base_url: &str) -> std::io::Result<Option<PathBuf>> {
        let Some(path) = self.setup_path() else {
            return Ok(None);
        };
        write_private(&self.state.setup_file, &format!("{base_url}{path}"))?;
        Ok(Some(self.state.setup_file.clone()))
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

/// The file that holds the first-account setup link while one is pending.
#[must_use]
pub fn setup_link_file(data_dir: &std::path::Path) -> PathBuf {
    data_dir.join("setup-link")
}

/// Writes `contents` to a file that only the current user can read.
fn write_private(path: &std::path::Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(path)?;
    // An existing file keeps its mode on open; tighten it either way.
    #[cfg(unix)]
    std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    file.write_all(contents.as_bytes())?;
    file.write_all(b"\n")
}

pub(crate) fn now_ms() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}
