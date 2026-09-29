//! Knowing about new releases, reminding admins, and installing them.
//!
//! Every few hours Sideporch asks GitHub which releases exist. Admins see
//! newer ones under Admin → Updates, and a reminder that grows more
//! insistent the longer the server stays behind, or right away when a newer
//! release fixes a security problem. Servers installed from a release
//! archive can replace their own program: the release's checksums must be
//! signed with the release key built into Sideporch, and the archive must
//! match them, before anything is installed.

pub mod install;
pub mod release;
pub mod signature;

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

pub use self::{
    install::Method,
    release::{Release, Urgency, Version},
};
use crate::{AppState, automations::http::Http, error::AppResult, now_ms, store};

/// The build target this program was compiled for (see build.rs).
pub const TARGET: &str = env!("SIDEPORCH_TARGET");
/// How often to ask for new releases.
const CHECK_EVERY_MS: i64 = 6 * 60 * 60 * 1000;
/// How often the background task looks at what's due.
const TICK: Duration = Duration::from_mins(15);
/// A release list, a checksum file and a signature are small.
const MAX_LIST: usize = 512 * 1024;
const MAX_SMALL_FILE: usize = 64 * 1024;
/// Release archives are a few megabytes.
const MAX_ARCHIVE: u64 = 256 * 1024 * 1024;

/// Where releases come from. Tests point this at a local fake.
#[derive(Debug, Clone)]
pub struct Source {
    /// The repository in GitHub's API.
    pub api: String,
    /// Where release files are, followed by `/vX.Y.Z/<file>`.
    pub downloads: String,
    /// The minisign key release checksums are signed with.
    pub public_key: String,
    /// Allow a source on this machine or network; only tests serve one.
    pub local: bool,
}

impl Default for Source {
    fn default() -> Self {
        Self {
            api: "https://api.github.com/repos/niklas-heer/sideporch".to_owned(),
            downloads: "https://github.com/niklas-heer/sideporch/releases/download".to_owned(),
            public_key: signature::RELEASE_KEY.to_owned(),
            local: false,
        }
    }
}

/// Which releases to install without asking, where Sideporch can.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoInstall {
    Off,
    /// Releases that fix security problems.
    Security,
    /// Every release, installed between 3 and 5 in the morning.
    All,
}

impl AutoInstall {
    #[must_use]
    pub fn parse(key: &str) -> Option<Self> {
        match key {
            "off" => Some(Self::Off),
            "security" => Some(Self::Security),
            "all" => Some(Self::All),
            _ => None,
        }
    }

    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Security => "security",
            Self::All => "all",
        }
    }
}

/// What admins chose under Admin → Updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    /// Ask GitHub for new releases.
    pub check: bool,
    pub install: AutoInstall,
}

impl Settings {
    /// # Errors
    ///
    /// Fails if the database can't be read.
    pub fn load(conn: &Connection) -> AppResult<Self> {
        Ok(Self {
            check: store::setting(conn, "updates.check")?.as_deref() != Some("off"),
            install: store::setting(conn, "updates.install")?
                .as_deref()
                .and_then(AutoInstall::parse)
                .unwrap_or(AutoInstall::Security),
        })
    }

    /// # Errors
    ///
    /// Fails if the database can't be written.
    pub fn save(self, conn: &Connection) -> AppResult<()> {
        store::set_setting(conn, "updates.check", if self.check { "on" } else { "off" })?;
        store::set_setting(conn, "updates.install", self.install.key())
    }
}

/// The last check, kept with the settings so reminders survive a restart.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Known {
    checked_at: Option<i64>,
    releases: Vec<Release>,
}

/// What admins see about updates.
#[derive(Debug, Clone, Default)]
pub struct Status {
    pub checked_at: Option<i64>,
    /// Why the last check failed.
    pub error: Option<String>,
    /// Releases newer than this one, newest first.
    pub newer: Vec<Release>,
    /// Being downloaded and checked now.
    pub installing: Option<Version>,
    /// Installed, and running once Sideporch restarts.
    pub installed: Option<Version>,
    /// Why the last installation failed.
    pub install_error: Option<String>,
}

/// A reminder for an admin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub latest: Version,
    pub urgency: Urgency,
}

pub struct Updates {
    source: Source,
    http: Http,
    /// Checking isn't turned off with `SIDEPORCH_UPDATE_CHECK=false`.
    allowed: bool,
    /// The running program, if known.
    executable: Option<PathBuf>,
    method: Method,
    /// Start the new version after installing it.
    restart: bool,
    status: Mutex<Status>,
    busy: AtomicBool,
}

impl std::fmt::Debug for Updates {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Updates")
            .field("executable", &self.executable)
            .field("method", &self.method)
            .finish_non_exhaustive()
    }
}

impl Updates {
    /// Must be called inside the Tokio runtime. `executable` is the program
    /// to replace; `restart` starts the new one in its place.
    ///
    /// # Errors
    ///
    /// Fails outside a Tokio runtime.
    pub fn new(
        source: Source,
        allowed: bool,
        executable: Option<PathBuf>,
        restart: bool,
    ) -> AppResult<Self> {
        let executable = executable.map(|path| std::fs::canonicalize(&path).unwrap_or(path));
        let in_container =
            Path::new("/.dockerenv").exists() || Path::new("/run/.containerenv").exists();
        let method = executable
            .as_deref()
            .map_or(Method::Archive, |path| Method::detect(path, in_container));
        Ok(Self {
            http: Http::new(source.local)?,
            source,
            allowed,
            executable,
            method,
            restart,
            status: Mutex::new(Status::default()),
            busy: AtomicBool::new(false),
        })
    }

    /// Picks up the last check from the database.
    ///
    /// # Errors
    ///
    /// Fails if the database can't be read.
    pub fn load(&self, conn: &Connection) -> AppResult<()> {
        let known: Known = store::setting(conn, "updates.known")?
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();
        self.with_status(|status| {
            status.checked_at = known.checked_at;
            status.newer = release::newer_than(&known.releases, Version::current());
        });
        Ok(())
    }

    pub fn status(&self) -> Status {
        self.status
            .lock()
            .map(|status| status.clone())
            .unwrap_or_default()
    }

    fn with_status(&self, change: impl FnOnce(&mut Status)) {
        if let Ok(mut status) = self.status.lock() {
            change(&mut status);
        }
    }

    pub const fn method(&self) -> Method {
        self.method
    }

    pub fn executable(&self) -> Option<&Path> {
        self.executable.as_deref()
    }

    /// Whether checking is allowed by the environment.
    pub const fn allowed(&self) -> bool {
        self.allowed
    }

    /// Whether Sideporch starts a new version by itself after installing it.
    pub const fn restarts(&self) -> bool {
        self.restart
    }

    /// Asks GitHub for releases. Returns them, newest first.
    ///
    /// # Errors
    ///
    /// Says why GitHub couldn't be asked.
    pub async fn fetch_releases(&self) -> Result<Vec<Release>, String> {
        let url = format!("{}/releases?per_page=30", self.source.api);
        let (status, body) = self.http.fetch(&url, MAX_LIST).await?;
        if status != 200 {
            return Err(format!("GitHub answered {status}"));
        }
        release::parse_releases(&String::from_utf8_lossy(&body))
    }

    /// Checks for new releases and remembers the answer in the database.
    pub(crate) async fn check(&self, state: &AppState) -> Result<(), String> {
        let result = self.fetch_releases().await;
        let now = now_ms();
        match result {
            Ok(releases) => {
                self.with_status(|status| {
                    status.checked_at = Some(now);
                    status.error = None;
                    status.newer = release::newer_than(&releases, Version::current());
                });
                let known = serde_json::to_string(&Known {
                    checked_at: Some(now),
                    releases,
                })
                .map_err(|error| error.to_string())?;
                state
                    .db
                    .call(move |conn| store::set_setting(conn, "updates.known", &known))
                    .await
                    .map_err(|error| error.to_string())
            }
            Err(error) => {
                tracing::warn!(%error, "could not check for updates");
                self.with_status(|status| {
                    status.checked_at = Some(now);
                    status.error = Some(error.clone());
                });
                Err(error)
            }
        }
    }

    /// Why this server can't replace its own program, if it can't.
    pub fn cannot_install(&self) -> Option<String> {
        let Some(executable) = &self.executable else {
            return Some("Sideporch doesn't know where its program is.".to_owned());
        };
        match self.method {
            Method::Homebrew => {
                return Some("Homebrew installed Sideporch, so Homebrew updates it.".to_owned());
            }
            Method::Nix => return Some("Nix installed Sideporch, so Nix updates it.".to_owned()),
            Method::Container => {
                return Some("Sideporch runs in a container; a new image updates it.".to_owned());
            }
            Method::Archive => {}
        }
        let Some(dir) = executable.parent() else {
            return Some("Sideporch doesn't know where its program is.".to_owned());
        };
        let probe = dir.join(format!(".sideporch-write-test-{}", std::process::id()));
        match std::fs::write(&probe, b"") {
            Ok(()) => {
                drop(std::fs::remove_file(&probe));
                None
            }
            Err(_) => Some(format!(
                "Sideporch may not change {}, so it can't replace itself.",
                dir.display()
            )),
        }
    }

    /// The reminder `user_id`, an admin, should see now, if any.
    ///
    /// # Errors
    ///
    /// Fails if the database can't be read.
    pub fn notice(&self, conn: &Connection, user_id: i64, now: i64) -> AppResult<Option<Notice>> {
        let status = self.status();
        let (Some(latest), Some(urgency)) = (
            status.newer.first().map(|release| release.version),
            Urgency::of(&status.newer, now),
        ) else {
            return Ok(None);
        };
        if urgency.hide_for_days().is_none() || status.installed.is_some() {
            return Ok(None);
        }
        let hidden = store::setting(conn, &format!("updates.hidden.{user_id}"))?;
        let still_hidden = hidden.as_deref().is_some_and(|hidden| {
            let mut parts = hidden.split(' ');
            parts.next() == Some(&latest.to_string())
                && parts.next() == Some(urgency.key())
                && parts
                    .next()
                    .and_then(|until| until.parse::<i64>().ok())
                    .is_some_and(|until| now < until)
        });
        Ok((!still_hidden).then_some(Notice { latest, urgency }))
    }

    /// Hides the current reminder from `user_id` for as long as it allows.
    ///
    /// # Errors
    ///
    /// Fails if the database can't be written.
    pub fn hide(&self, conn: &Connection, user_id: i64, now: i64) -> AppResult<()> {
        let Some(Notice { latest, urgency }) = self.notice(conn, user_id, now)? else {
            return Ok(());
        };
        let days = urgency.hide_for_days().unwrap_or(1);
        let until = now.saturating_add(days.saturating_mul(24 * 60 * 60 * 1000));
        store::set_setting(
            conn,
            &format!("updates.hidden.{user_id}"),
            &format!("{latest} {} {until}", urgency.key()),
        )
    }

    /// Downloads `version`, checks it, and replaces the running program.
    /// With `restart`, starts the new version in this one's place.
    ///
    /// # Errors
    ///
    /// Says why the update wasn't installed; the running program stays.
    pub async fn install(self: &Arc<Self>, version: Version) -> Result<(), String> {
        if let Some(reason) = self.cannot_install() {
            return Err(reason);
        }
        if self.busy.swap(true, Ordering::SeqCst) {
            return Err("An update is being installed already.".to_owned());
        }
        self.with_status(|status| {
            status.installing = Some(version);
            status.install_error = None;
        });
        let result = self.replace_with(version).await;
        self.with_status(|status| {
            status.installing = None;
            match &result {
                Ok(()) => status.installed = Some(version),
                Err(error) => status.install_error = Some(error.clone()),
            }
        });
        self.busy.store(false, Ordering::SeqCst);
        match &result {
            Ok(()) => {
                tracing::info!(%version, "installed the update");
                if self.restart {
                    self.restart_soon();
                }
            }
            Err(error) => tracing::warn!(%version, %error, "could not install the update"),
        }
        result
    }

    async fn replace_with(&self, version: Version) -> Result<(), String> {
        let executable = self
            .executable
            .clone()
            .ok_or("Sideporch doesn't know where its program is.")?;
        let dir = executable
            .parent()
            .ok_or("Sideporch doesn't know where its program is.")?
            .to_owned();
        let base = format!("{}/v{version}", self.source.downloads);
        let sums = self.small_file(&format!("{base}/SHA256SUMS")).await?;
        let signature = self
            .small_file(&format!("{base}/SHA256SUMS.minisig"))
            .await
            .map_err(|error| format!("the release has no signature ({error})"))?;
        signature::verify(
            &self.source.public_key,
            &sums,
            &String::from_utf8_lossy(&signature),
            &version.to_string(),
        )?;
        let name = install::archive_name(&version.to_string(), TARGET);
        let sums = String::from_utf8_lossy(&sums);
        let sha256 = install::checksum_for(&sums, &name)
            .ok_or_else(|| format!("the release has no build for this system ({TARGET})"))?;
        let archive = dir.join(format!(".sideporch-{version}.tar.gz"));
        let program = dir.join(format!(".sideporch-{version}.new"));
        let result = async {
            self.http
                .download_up_to(&format!("{base}/{name}"), &archive, MAX_ARCHIVE, sha256)
                .await?;
            install::extract(&archive, &program)?;
            check_runs(&program, version).await?;
            // Keep the version that ran before, to go back by hand.
            let previous = dir.join(format!(
                "{}.previous",
                executable.file_name().unwrap_or_default().to_string_lossy()
            ));
            std::fs::copy(&executable, &previous).map_err(|error| error.to_string())?;
            std::fs::rename(&program, &executable).map_err(|error| error.to_string())
        }
        .await;
        drop(std::fs::remove_file(&archive));
        if result.is_err() {
            drop(std::fs::remove_file(&program));
        }
        result
    }

    async fn small_file(&self, url: &str) -> Result<Vec<u8>, String> {
        let (status, body) = self.http.fetch(url, MAX_SMALL_FILE).await?;
        if status == 200 {
            Ok(body)
        } else {
            Err(format!("{url} answered {status}"))
        }
    }

    /// Starts the new program in this process's place, after a moment for
    /// the page that asked to finish loading.
    fn restart_soon(&self) {
        let Some(executable) = self.executable.clone() else {
            return;
        };
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(1)).await;
            tracing::info!("starting the new version");
            let error = restart(&executable);
            tracing::error!(%error, "could not start the new version; restart Sideporch");
        });
    }
}

/// Runs the new program with `--version` and checks it's the one expected.
async fn check_runs(program: &Path, version: Version) -> Result<(), String> {
    let output = tokio::time::timeout(
        Duration::from_secs(30),
        tokio::process::Command::new(program)
            .arg("--version")
            .output(),
    )
    .await
    .map_err(|_| "the new version didn't start".to_owned())?
    .map_err(|error| format!("the new version doesn't run here: {error}"))?;
    let says = String::from_utf8_lossy(&output.stdout);
    if output.status.success() && says.trim() == format!("sideporch {version}") {
        Ok(())
    } else {
        Err(format!(
            "the new program says “{}”, not sideporch {version}",
            says.trim()
        ))
    }
}

/// Replaces this process with `executable`, given the same arguments.
/// Only returns if that fails.
#[cfg(unix)]
fn restart(executable: &Path) -> std::io::Error {
    use std::os::unix::process::CommandExt as _;
    std::process::Command::new(executable)
        .args(std::env::args_os().skip(1))
        .exec()
}

#[cfg(not(unix))]
fn restart(_executable: &Path) -> std::io::Error {
    std::io::Error::other("restarting is only supported on Unix")
}

/// Checks for releases every few hours and installs the ones admins chose
/// to have installed automatically.
pub(crate) fn start(state: AppState) {
    tokio::spawn(async move {
        // Give the server a minute to settle first.
        tokio::time::sleep(Duration::from_mins(1)).await;
        loop {
            if let Err(error) = tick(&state).await {
                tracing::warn!(%error, "update check");
            }
            tokio::time::sleep(TICK).await;
        }
    });
}

async fn tick(state: &AppState) -> AppResult<()> {
    let updates = Arc::clone(&state.updates);
    let settings = state.db.call(|conn| Settings::load(conn)).await?;
    let status = updates.status();
    let due = status
        .checked_at
        .is_none_or(|at| now_ms().saturating_sub(at) >= CHECK_EVERY_MS);
    if updates.allowed() && settings.check && due {
        drop(updates.check(state).await);
    }
    let status = updates.status();
    let Some(latest) = status.newer.first().map(|release| release.version) else {
        return Ok(());
    };
    let security = status.newer.iter().any(|release| release.security);
    let quiet_hours = (3..5).contains(&jiff::Zoned::now().hour());
    let wanted = match settings.install {
        AutoInstall::Off => false,
        AutoInstall::Security => security,
        AutoInstall::All => security || quiet_hours,
    };
    if wanted
        && status.installed.is_none()
        && status.install_error.is_none()
        && updates.cannot_install().is_none()
    {
        drop(updates.install(latest).await);
    }
    Ok(())
}
