//! Sideporch: a small, self-hosted team chat in one binary.
//!
//! [`Sideporch::open`] prepares the data directory and database;
//! [`Sideporch::router`] is the complete web application.

mod access;
mod ai;
mod assets;
mod auth;
mod automations;
mod backup;
mod blobs;
mod cloudflare;
mod community;
mod db;
mod demo;
mod emoji;
mod error;
mod federation;
mod files;
mod gifs;
mod icons;
mod import;
mod later;
mod mail;
mod markdown;
mod markup;
mod mcp;
mod messages;
mod outgoing;
mod passkeys;
mod polls;
mod previews;
mod push;
mod realtime;
mod routes;
mod search;
mod secrets;
mod security;
mod speech;
mod statistics;
mod store;
mod system;
mod themes;
mod totp;
pub mod updates;
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
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent switches, set from command-line options"
)]
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
    /// A setup code the operator chose, such as a secret kept in a password
    /// manager. While no account exists, `/setup` asks for it. Easier than
    /// the setup link on platforms without a shell, and used instead of it
    /// when both are set. At least [`MIN_SETUP_CODE`] characters.
    pub setup_code: Option<String>,
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
    /// Where speech models are downloaded from; tests serve them locally.
    #[doc(hidden)]
    pub model_base_url: Option<String>,
    /// Ask GitHub for new releases every few hours, so admins hear about
    /// updates. Admins can also turn this off under Admin → Updates.
    pub update_check: bool,
    /// Where releases come from and which program to replace; tests use a
    /// fake. The server doesn't restart itself after updating then.
    #[doc(hidden)]
    pub update_source: Option<(updates::Source, PathBuf)>,
    /// Let other Sideporch servers be on private addresses; tests run them
    /// on this machine.
    #[doc(hidden)]
    pub allow_private_federation: bool,
    /// The header a trusted reverse proxy puts the client's address in,
    /// such as `X-Forwarded-For` or `Fly-Client-IP`. Without it, the
    /// connection's address counts and such headers are ignored.
    pub client_ip_header: Option<String>,
    /// The Cloudflare Access team domain, such as
    /// `myteam.cloudflareaccess.com` (or just `myteam`). With
    /// [`Config::cloudflare_access_aud`], people whom Cloudflare Access let
    /// in sign in to the account with their address, without a password.
    pub cloudflare_access_team: Option<String>,
    /// The audience (AUD) tag of the Cloudflare Access application in front
    /// of this server. Set it together with the team domain.
    pub cloudflare_access_aud: Option<String>,
    /// Refuse pages to requests that didn't come through Cloudflare Access,
    /// so nobody can go around it by reaching the server directly. The
    /// health check, webhooks, federation and MCP stay open.
    pub cloudflare_access_required: bool,
    /// Where Cloudflare Access's signing keys are fetched; tests serve them
    /// locally.
    #[doc(hidden)]
    pub cloudflare_access_certs_url: Option<String>,
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
    /// Passkey challenges handed out and not answered yet.
    ceremonies: passkeys::Ceremonies,
    /// Reading aloud and dictation.
    speech: Arc<speech::Speech>,
    /// New releases, and installing them.
    updates: Arc<updates::Updates>,
    /// Other Sideporch servers.
    federation: federation::Shared,
    /// Recent statistics, so big servers don't count them for every visit.
    statistics: Arc<statistics::Cache>,
    /// Client addresses, sign-in limits and bans.
    access: Arc<access::Access>,
    /// Signing in through Cloudflare Access, when it's set up.
    cloudflare: Option<Arc<cloudflare::CloudflareAccess>>,
}

/// The shortest setup code Sideporch accepts. Anyone can try codes while
/// no account exists, so a code must be too long to guess.
pub const MIN_SETUP_CODE: usize = 16;

/// Whether and how the first account can still be created.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Setup {
    Done,
    /// Whoever opens `/setup` first creates the admin account.
    Open,
    /// `/setup` asks for this secret. A random one also works as the
    /// one-time link `/setup/<token>`; one the operator `chose` doesn't, so
    /// it stays out of browser history and access logs.
    Secret {
        token: String,
        chosen: bool,
    },
}

impl AppState {
    fn setup(&self) -> Setup {
        self.setup.lock().map_or(Setup::Done, |setup| setup.clone())
    }

    /// Whether `candidate` is the setup secret. Hashing both sides keeps
    /// the comparison from leaking how much of a guess was right.
    fn setup_code_matches(&self, candidate: &str) -> bool {
        matches!(self.setup(), Setup::Secret { token, .. }
            if Sha256::digest(&token) == Sha256::digest(candidate.trim()))
    }

    /// Whether `candidate` is the random one-time setup link's token.
    fn setup_link_matches(&self, candidate: &str) -> bool {
        matches!(self.setup(), Setup::Secret { token, chosen: false }
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

/// How the first account can be created when the server starts with
/// `users` accounts.
fn initial_setup(
    users: i64,
    require_setup_link: bool,
    setup_code: Option<String>,
) -> Result<Setup, Error> {
    let setup_code = setup_code
        .map(|code| code.trim().to_owned())
        .filter(|code| !code.is_empty());
    if let Some(code) = &setup_code
        && code.chars().count() < MIN_SETUP_CODE
    {
        return Err(Error::internal(format!(
            "the setup code must have at least {MIN_SETUP_CODE} characters, so nobody can guess it; generate one, for example with `openssl rand -base64 24`"
        )));
    }
    Ok(if users > 0 {
        Setup::Done
    } else if let Some(token) = setup_code {
        if require_setup_link {
            tracing::info!("a setup code is set, so it is used instead of a setup link");
        }
        Setup::Secret {
            token,
            chosen: true,
        }
    } else if require_setup_link {
        Setup::Secret {
            token: auth::random_token()?,
            chosen: false,
        }
    } else {
        Setup::Open
    })
}

/// Cloudflare Access sign-in, when the team domain and audience tag are
/// both set. One without the other is a mistake worth stopping for.
fn cloudflare_access(config: &Config) -> Result<Option<cloudflare::CloudflareAccess>, Error> {
    let present = |value: &Option<String>| {
        value
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    };
    match (
        present(&config.cloudflare_access_team),
        present(&config.cloudflare_access_aud),
    ) {
        (Some(team), Some(aud)) => Ok(Some(cloudflare::CloudflareAccess::new(
            &team,
            &aud,
            config.cloudflare_access_required,
            config.cloudflare_access_certs_url.clone(),
        )?)),
        (None, None) if config.cloudflare_access_required => Err(Error::internal(
            "Cloudflare Access is required, but its team domain and audience tag aren't set",
        )),
        (None, None) => Ok(None),
        _ => Err(Error::internal(
            "set both the Cloudflare Access team domain and its audience (AUD) tag, or neither",
        )),
    }
}

/// An opened Sideporch instance.
pub struct Sideporch {
    state: AppState,
}

/// Opens this server's federation key, making and storing one the first
/// time.
async fn open_federation(
    db: &Db,
    vault: &Vault,
    public_url: Option<String>,
    allow_private: bool,
) -> Result<federation::Federation, Error> {
    let sealed = db.call(|conn| federation::data::sealed_key(conn)).await?;
    let (federation, new_key) =
        federation::Federation::open(sealed.as_deref(), vault, public_url, allow_private)?;
    if let Some(sealed) = new_key {
        db.call(move |conn| federation::data::set_sealed_key(conn, &sealed))
            .await?;
    }
    Ok(federation)
}

impl Sideporch {
    /// Opens (or creates) the instance in `config.data_dir`.
    ///
    /// # Errors
    ///
    /// Fails if the data directory or database cannot be opened.
    pub async fn open(config: Config) -> Result<Self, Error> {
        let cloudflare = cloudflare_access(&config)?.map(Arc::new);
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
        let vault = Arc::new(Vault::open(&config.data_dir)?);
        let federation = open_federation(
            &db,
            &vault,
            public_url.clone(),
            config.allow_private_federation,
        )
        .await?;
        let setup = initial_setup(users, config.require_setup_link, config.setup_code)?;
        let state = AppState {
            db,
            hub: Hub::default(),
            push: Arc::new(push),
            ai: Arc::new(Ai::new()?),
            vault,
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
            ceremonies: passkeys::Ceremonies::default(),
            speech: speech::Speech::new(&config.data_dir, config.model_base_url.clone()),
            updates: Arc::new(match config.update_source {
                Some((source, executable)) => {
                    updates::Updates::new(source, config.update_check, Some(executable), false)?
                }
                None => updates::Updates::new(
                    updates::Source::default(),
                    config.update_check,
                    std::env::current_exe().ok(),
                    true,
                )?,
            }),
            federation: Arc::new(federation),
            statistics: Arc::default(),
            access: Arc::new(access::Access::new(config.client_ip_header.as_deref())?),
            cloudflare,
        };
        let updates = Arc::clone(&state.updates);
        state.db.call(move |conn| updates.load(conn)).await?;
        state.access.reload(&state).await?;
        start_background_work(&state);
        state.automations.reload(&state).await?;
        Ok(Self { state })
    }

    /// The path of the setup page, while no account exists yet: `/setup`,
    /// or the one-time `/setup/<token>` when setup requires the link.
    #[must_use]
    pub fn setup_path(&self) -> Option<String> {
        match self.state.setup() {
            Setup::Done => None,
            Setup::Open | Setup::Secret { chosen: true, .. } => Some("/setup".to_owned()),
            Setup::Secret {
                token,
                chosen: false,
            } => Some(format!("/setup/{token}")),
        }
    }

    /// Whether the setup path is a secret one-time link.
    #[must_use]
    pub fn setup_is_secret(&self) -> bool {
        matches!(self.state.setup(), Setup::Secret { chosen: false, .. })
    }

    /// Whether setup asks for a code the operator chose.
    #[must_use]
    pub fn setup_needs_code(&self) -> bool {
        matches!(self.state.setup(), Setup::Secret { chosen: true, .. })
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
        // A chosen code is already kept wherever the operator keeps secrets.
        if self.setup_needs_code() {
            return Ok(None);
        }
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

    /// The router as a service that knows each connection's address, for
    /// `axum::serve`.
    #[must_use]
    pub fn service(
        &self,
    ) -> axum::extract::connect_info::IntoMakeServiceWithConnectInfo<Router, std::net::SocketAddr>
    {
        self.router()
            .into_make_service_with_connect_info::<std::net::SocketAddr>()
    }
}

/// Starts what runs next to requests: automations, reminders, backups,
/// update checks, the federation outbox and cleanups.
fn start_background_work(state: &AppState) {
    state.speech.start_unloading();
    state.automations.serve(state.clone());
    later::start(state.clone());
    backup::start(state.clone());
    updates::start(state.clone());
    federation::outbox::start(state.clone());
    access::start(state.clone());
    cloudflare::start(state);
    demo::start(state.clone());
}

/// Makes a one-time password reset link for `username`, for 24 hours.
///
/// Returns its path, `/reset/<token>`. This is `sideporch reset-link`: the
/// way back in for someone, often the only admin, who lost their passkey
/// or password, from a shell on the server.
///
/// # Errors
///
/// Fails if the database can't be opened or no account here has that
/// username.
pub fn reset_link(data_dir: &std::path::Path, username: &str) -> Result<String, Error> {
    let path = data_dir.join("sideporch.db");
    if !path.exists() {
        return Err(Error::internal(format!(
            "{} holds no Sideporch database",
            data_dir.display()
        )));
    }
    let conn = db::connect(&path)?;
    let username = username.trim().trim_start_matches('@').to_lowercase();
    let user_id: Option<i64> = rusqlite::OptionalExtension::optional(conn.query_row(
        "SELECT id FROM users WHERE username = ?1 AND instance_id IS NULL AND deactivated_at IS NULL",
        [&username],
        |row| row.get(0),
    ))?;
    let user_id = user_id.ok_or_else(|| {
        Error::internal(format!(
            "no active account here has the username {username}"
        ))
    })?;
    let token = auth::random_token()?;
    let now = now_ms();
    let expires = now.saturating_add(24 * 60 * 60 * 1000);
    conn.execute("DELETE FROM password_resets WHERE user_id = ?1", [user_id])?;
    conn.execute(
        "INSERT INTO password_resets (token_hash, user_id, created_by, created_at, expires_at)
         VALUES (?1, ?2, NULL, ?3, ?4)",
        rusqlite::params![auth::hash_token(&token), user_id, now, expires],
    )?;
    Ok(format!("/reset/{token}"))
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

/// Creates a database at an old schema version, so tests can check that
/// Sideporch upgrades it.
///
/// # Errors
///
/// Fails if the database cannot be written.
#[doc(hidden)]
pub fn create_database_at_schema(path: &std::path::Path, version: i64) -> Result<(), Error> {
    db::create_at_version(path, version)
}

pub(crate) fn now_ms() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}
