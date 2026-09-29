//! Backups: the database, the stored files and, if wanted, the secret key,
//! in one `.tar.gz` archive.
//!
//! The database is copied with `VACUUM INTO`, which gives a consistent
//! snapshot without stopping the server. Admins download archives from the
//! browser or let Sideporch write them to a directory on a schedule, keeping
//! the newest few. `sideporch restore` unpacks one into an empty data
//! directory.

use std::{
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use rusqlite::Connection;

use crate::{
    AppState,
    error::{AppError, AppResult},
    now_ms, store,
};

const DATABASE: &str = "sideporch.db";
const KEY_FILE: &str = "secret.key";
const PREFIX: &str = "sideporch-";
const SUFFIX: &str = ".tar.gz";
/// How often the schedule is checked.
const CHECK: Duration = Duration::from_secs(10 * 60);
const HOUR_MS: i64 = 60 * 60 * 1000;

/// How often scheduled backups run: off, or every so many hours.
pub const INTERVALS: &[(u32, &str)] = &[
    (0, "Off"),
    (6, "Every 6 hours"),
    (12, "Every 12 hours"),
    (24, "Daily"),
    (168, "Weekly"),
];

#[derive(Debug, Clone)]
pub struct Settings {
    pub every_hours: u32,
    pub keep: u32,
    /// Where scheduled backups go; relative paths are in the data directory.
    pub dir: String,
    pub include_key: bool,
}

#[derive(Debug, Clone, Default)]
/// The last scheduled or manual run.
pub struct Status {
    pub at: Option<i64>,
    pub file: Option<String>,
    pub error: Option<String>,
}

fn number(conn: &Connection, key: &str, default: u32) -> AppResult<u32> {
    Ok(store::setting(conn, key)?
        .and_then(|value| value.parse().ok())
        .unwrap_or(default))
}

impl Settings {
    pub fn load(conn: &Connection) -> AppResult<Self> {
        Ok(Self {
            every_hours: number(conn, "backup.every_hours", 0)?,
            keep: number(conn, "backup.keep", 7)?,
            dir: store::setting(conn, "backup.dir")?.unwrap_or_else(|| "backups".to_owned()),
            include_key: store::setting(conn, "backup.include_key")?.as_deref() != Some("false"),
        })
    }

    pub fn save(&self, conn: &Connection) -> AppResult<()> {
        if !INTERVALS
            .iter()
            .any(|(hours, _)| *hours == self.every_hours)
        {
            return Err(AppError::bad_request("Pick how often to back up."));
        }
        if !(1..=100).contains(&self.keep) {
            return Err(AppError::bad_request("Keep between 1 and 100 backups."));
        }
        if self.dir.trim().is_empty() || self.dir.contains('\0') {
            return Err(AppError::bad_request("Enter a directory for backups."));
        }
        store::set_setting(conn, "backup.every_hours", &self.every_hours.to_string())?;
        store::set_setting(conn, "backup.keep", &self.keep.to_string())?;
        store::set_setting(conn, "backup.dir", self.dir.trim())?;
        store::set_setting(
            conn,
            "backup.include_key",
            if self.include_key { "true" } else { "false" },
        )
    }

    pub fn directory(&self, data_dir: &Path) -> PathBuf {
        data_dir.join(self.dir.trim())
    }
}

impl Status {
    pub fn load(conn: &Connection) -> AppResult<Self> {
        Ok(Self {
            at: store::setting(conn, "backup.last_at")?.and_then(|at| at.parse().ok()),
            file: store::setting(conn, "backup.last_file")?,
            error: store::setting(conn, "backup.last_error")?,
        })
    }
}

/// A file name for a backup made at `now`.
pub fn file_name(now: i64) -> String {
    let when = jiff::Timestamp::from_millisecond(now).unwrap_or_default();
    format!("{PREFIX}{}{SUFFIX}", when.strftime("%Y%m%d-%H%M%S"))
}

/// Whether `name` is a backup this module wrote.
pub fn is_backup_name(name: &str) -> bool {
    name.strip_prefix(PREFIX)
        .and_then(|rest| rest.strip_suffix(SUFFIX))
        .is_some_and(|stamp| {
            stamp.len() == 15 && stamp.chars().all(|c| c.is_ascii_digit() || c == '-')
        })
}

fn random_suffix() -> AppResult<String> {
    crate::auth::random_token().map(|token| token.chars().take(12).collect())
}

/// Copies the database into a new file in the data directory. Runs on the
/// main connection, briefly.
pub fn snapshot(conn: &Connection, data_dir: &Path) -> AppResult<PathBuf> {
    let path = data_dir.join(format!(".backup-{}.db", random_suffix()?));
    let target = path
        .to_str()
        .ok_or_else(|| AppError::internal("the data directory path is not UTF-8"))?;
    conn.execute("VACUUM INTO ?1", [target])?;
    Ok(path)
}

fn valid_hash(name: &str) -> bool {
    name.len() == 64 && name.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Packs a database snapshot, the stored files and optionally the key into
/// `out`, and deletes the snapshot. Blocks.
pub fn pack(snapshot: &Path, data_dir: &Path, include_key: bool, out: impl Write) -> AppResult<()> {
    let result = (|| -> std::io::Result<()> {
        let mut archive = tar::Builder::new(GzEncoder::new(out, Compression::default()));
        archive.append_path_with_name(snapshot, DATABASE)?;
        let files = data_dir.join("files");
        if let Ok(prefixes) = std::fs::read_dir(&files) {
            for prefix in prefixes.flatten() {
                let prefix_name = prefix.file_name().to_string_lossy().into_owned();
                let Ok(entries) = std::fs::read_dir(prefix.path()) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    // A file deleted since the snapshot is simply skipped.
                    if valid_hash(&name) && entry.path().is_file() {
                        archive.append_path_with_name(
                            entry.path(),
                            format!("files/{prefix_name}/{name}"),
                        )?;
                    }
                }
            }
        }
        let key = data_dir.join(KEY_FILE);
        if include_key && key.is_file() {
            archive.append_path_with_name(&key, KEY_FILE)?;
        }
        archive.into_inner()?.finish()?.flush()
    })();
    drop(std::fs::remove_file(snapshot));
    result.map_err(AppError::internal)
}

/// Makes a complete archive at `path`.
pub async fn write_to(state: &AppState, path: PathBuf, include_key: bool) -> AppResult<()> {
    let data_dir = state.data_dir.clone();
    let snapshot_dir = data_dir.clone();
    let snapshot = state
        .db
        .call(move |conn| snapshot(conn, &snapshot_dir))
        .await?;
    tokio::task::spawn_blocking(move || {
        let partial = path.with_extension("partial");
        let file = File::create(&partial).map_err(AppError::internal)?;
        pack(
            &snapshot,
            &data_dir,
            include_key,
            std::io::BufWriter::new(file),
        )?;
        std::fs::rename(&partial, &path).map_err(AppError::internal)
    })
    .await
    .map_err(AppError::internal)?
}

/// A backup in the backup directory.
pub struct Stored {
    pub name: String,
    pub bytes: u64,
}

/// The backups in `dir`, newest first.
pub fn list(dir: &Path) -> Vec<Stored> {
    let mut stored: Vec<Stored> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let bytes = entry.metadata().ok()?.len();
                    is_backup_name(&name).then_some(Stored { name, bytes })
                })
                .collect()
        })
        .unwrap_or_default();
    stored.sort_by(|a, b| b.name.cmp(&a.name));
    stored
}

/// Writes a scheduled backup and removes the oldest beyond `keep`.
pub async fn run_scheduled(state: &AppState) -> AppResult<()> {
    let settings = state.db.call(|conn| Settings::load(conn)).await?;
    let dir = settings.directory(&state.data_dir);
    let now = now_ms();
    let name = file_name(now);
    let result = async {
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(AppError::internal)?;
        write_to(state, dir.join(&name), settings.include_key).await?;
        let keep = usize::try_from(settings.keep).unwrap_or(7);
        for old in list(&dir).into_iter().skip(keep) {
            drop(std::fs::remove_file(dir.join(old.name)));
        }
        AppResult::Ok(())
    }
    .await;
    let error = result.as_ref().err().map(ToString::to_string);
    state
        .db
        .call(move |conn| {
            store::set_setting(conn, "backup.last_at", &now.to_string())?;
            if let Some(error) = error {
                store::set_setting(conn, "backup.last_error", &error)
            } else {
                store::set_setting(conn, "backup.last_file", &name)?;
                store::delete_setting(conn, "backup.last_error")
            }
        })
        .await?;
    result
}

/// Runs scheduled backups when they are due.
pub fn start(state: AppState) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(CHECK);
        loop {
            ticker.tick().await;
            let due = state
                .db
                .call(|conn| {
                    let settings = Settings::load(conn)?;
                    let status = Status::load(conn)?;
                    let every = i64::from(settings.every_hours).saturating_mul(HOUR_MS);
                    Ok(settings.every_hours > 0
                        && status
                            .at
                            .is_none_or(|last| now_ms().saturating_sub(last) >= every))
                })
                .await
                .unwrap_or(false);
            if due && let Err(error) = run_scheduled(&state).await {
                tracing::warn!(?error, "scheduled backup failed");
            }
        }
    });
}

/// Where an archive entry may go: the database, the key, or a stored file.
fn restorable(path: &Path) -> bool {
    let parts: Vec<String> = path
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    match parts.as_slice() {
        [name] => name == DATABASE || name == KEY_FILE,
        [files, prefix, name] => {
            files == "files"
                && valid_hash(name)
                && prefix.len() == 2
                && name.get(..2) == Some(prefix.as_str())
        }
        _ => false,
    }
}

/// Unpacks a backup into `data_dir`, which must not hold a database yet
/// unless `force` is set. Returns how many files were restored.
///
/// # Errors
///
/// Fails if the archive is unreadable, holds no database, or the data
/// directory already has one.
pub fn restore(archive: &Path, data_dir: &Path, force: bool) -> Result<usize, String> {
    if data_dir.join(DATABASE).exists() && !force {
        return Err(format!(
            "{} already holds a database. Restore into an empty directory, or pass --force to replace it.",
            data_dir.display()
        ));
    }
    std::fs::create_dir_all(data_dir).map_err(|error| error.to_string())?;
    if archive
        .extension()
        .is_some_and(|extension| extension == "db")
    {
        return restore_database(archive, data_dir);
    }
    let file = File::open(archive).map_err(|error| format!("{}: {error}", archive.display()))?;
    let mut unpacked = tar::Archive::new(GzDecoder::new(file));
    let mut restored = 0_usize;
    let mut database = false;
    for entry in unpacked.entries().map_err(|error| error.to_string())? {
        let mut entry = entry.map_err(|error| error.to_string())?;
        let path = entry
            .path()
            .map_err(|error| error.to_string())?
            .into_owned();
        if !restorable(&path) {
            continue;
        }
        let target = data_dir.join(&path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let mut contents = Vec::new();
        entry
            .read_to_end(&mut contents)
            .map_err(|error| error.to_string())?;
        std::fs::write(&target, contents).map_err(|error| error.to_string())?;
        #[cfg(unix)]
        if path == Path::new(KEY_FILE) {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600))
                .map_err(|error| error.to_string())?;
        }
        database |= path == Path::new(DATABASE);
        restored = restored.saturating_add(1);
    }
    if !database {
        return Err("the archive holds no sideporch.db; is it a Sideporch backup?".to_owned());
    }
    // Leftover journal files from an earlier database would not match.
    for stale in ["sideporch.db-wal", "sideporch.db-shm"] {
        drop(std::fs::remove_file(data_dir.join(stale)));
    }
    Ok(restored)
}

/// Puts back a lone database, such as the copy kept in `upgrade-backups/`
/// before an upgrade. Stored files stay as they are.
fn restore_database(copy: &Path, data_dir: &Path) -> Result<usize, String> {
    let mut header = [0_u8; 16];
    File::open(copy)
        .and_then(|mut file| file.read_exact(&mut header))
        .map_err(|error| format!("{}: {error}", copy.display()))?;
    if &header != b"SQLite format 3\0" {
        return Err(format!("{} is not a Sideporch database", copy.display()));
    }
    std::fs::copy(copy, data_dir.join(DATABASE)).map_err(|error| error.to_string())?;
    for stale in ["sideporch.db-wal", "sideporch.db-shm"] {
        drop(std::fs::remove_file(data_dir.join(stale)));
    }
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_paths() {
        assert_eq!(file_name(0), "sideporch-19700101-000000.tar.gz");
        assert!(is_backup_name("sideporch-20260928-101500.tar.gz"));
        assert!(!is_backup_name("sideporch-../../etc.tar.gz"));
        assert!(restorable(Path::new("sideporch.db")));
        let hash = "ab".repeat(32);
        assert!(restorable(&Path::new("files").join("ab").join(&hash)));
        assert!(!restorable(&Path::new("files").join("cd").join(&hash)));
        assert!(!restorable(Path::new("../sideporch.db")));
        assert!(!restorable(Path::new("/etc/passwd")));
    }
}
