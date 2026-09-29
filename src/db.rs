use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use rusqlite::{Connection, OptionalExtension as _, Transaction, params};

use crate::error::{AppError, AppResult};

/// One step of the schema's history: SQL, or Rust code for changes SQL
/// can't express, such as rebuilding the search index.
enum Migration {
    Sql(&'static str),
    Code(fn(&Transaction<'_>) -> AppResult<()>),
}

/// Schema migrations, applied in order. `PRAGMA user_version` records how
/// many have run, so released entries must never change; append new ones.
const MIGRATIONS: &[Migration] = &[
    Migration::Sql(
        r"
CREATE TABLE users (
    id INTEGER PRIMARY KEY,
    username TEXT NOT NULL UNIQUE COLLATE NOCASE,
    display_name TEXT NOT NULL,
    password_hash TEXT NOT NULL,
    is_admin INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL
);

CREATE TABLE sessions (
    token_hash BLOB PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
);

CREATE TABLE invites (
    token TEXT PRIMARY KEY,
    created_by INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    uses INTEGER NOT NULL DEFAULT 0,
    revoked INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE channels (
    id INTEGER PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('public', 'dm')),
    name TEXT UNIQUE COLLATE NOCASE,
    topic TEXT NOT NULL DEFAULT '',
    dm_key TEXT UNIQUE,
    created_by INTEGER REFERENCES users(id) ON DELETE SET NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE channel_members (
    channel_id INTEGER NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    PRIMARY KEY (channel_id, user_id)
);

CREATE TABLE webhooks (
    id INTEGER PRIMARY KEY,
    channel_id INTEGER NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    token TEXT NOT NULL UNIQUE,
    created_by INTEGER REFERENCES users(id) ON DELETE SET NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE messages (
    id INTEGER PRIMARY KEY,
    channel_id INTEGER NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    parent_id INTEGER REFERENCES messages(id) ON DELETE CASCADE,
    user_id INTEGER REFERENCES users(id) ON DELETE SET NULL,
    webhook_id INTEGER REFERENCES webhooks(id) ON DELETE SET NULL,
    bot_name TEXT,
    bot_icon_url TEXT,
    body TEXT NOT NULL,
    attachments TEXT,
    created_at INTEGER NOT NULL
);
CREATE INDEX messages_by_channel ON messages (channel_id, parent_id, id);
CREATE INDEX messages_by_parent ON messages (parent_id, id);

CREATE TABLE reads (
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    channel_id INTEGER NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    last_read_id INTEGER NOT NULL,
    PRIMARY KEY (user_id, channel_id)
);
",
    ),
    Migration::Sql(
        r"
-- Full-text search. Rows are written by the application (message text,
-- attachment text and file names); the trigger keeps deletions in sync.
CREATE VIRTUAL TABLE messages_fts USING fts5(content, tokenize = 'unicode61 remove_diacritics 2');
INSERT INTO messages_fts (rowid, content) SELECT id, body FROM messages;
CREATE TRIGGER messages_fts_delete AFTER DELETE ON messages BEGIN
    DELETE FROM messages_fts WHERE rowid = old.id;
END;

-- Uploaded files live in the database so one file holds everything.
CREATE TABLE files (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    mime TEXT NOT NULL,
    size INTEGER NOT NULL,
    data BLOB NOT NULL,
    uploaded_by INTEGER REFERENCES users(id) ON DELETE SET NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE message_files (
    message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    PRIMARY KEY (message_id, file_id)
);
CREATE INDEX message_files_by_file ON message_files (file_id);

CREATE TABLE custom_emoji (
    name TEXT PRIMARY KEY,
    file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    created_by INTEGER REFERENCES users(id) ON DELETE SET NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE reactions (
    message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    emoji TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (message_id, user_id, emoji)
);

CREATE TABLE settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE push_subscriptions (
    endpoint TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    p256dh TEXT NOT NULL,
    auth TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE automations (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    source TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    last_error TEXT,
    created_by INTEGER REFERENCES users(id) ON DELETE SET NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE automation_data (
    automation_id INTEGER NOT NULL REFERENCES automations(id) ON DELETE CASCADE,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY (automation_id, key)
);

ALTER TABLE messages ADD COLUMN automation_id INTEGER REFERENCES automations(id) ON DELETE SET NULL;
",
    ),
    Migration::Sql(
        r"
ALTER TABLE automations ADD COLUMN hook_token TEXT;
UPDATE automations SET hook_token = lower(hex(randomblob(20)));
CREATE UNIQUE INDEX automations_by_hook_token ON automations (hook_token);

CREATE TABLE automation_versions (
    id INTEGER PRIMARY KEY,
    automation_id INTEGER NOT NULL REFERENCES automations(id) ON DELETE CASCADE,
    source TEXT NOT NULL,
    saved_by INTEGER REFERENCES users(id) ON DELETE SET NULL,
    saved_with TEXT NOT NULL,
    saved_at INTEGER NOT NULL
);
CREATE INDEX automation_versions_by_automation ON automation_versions (automation_id, id);
INSERT INTO automation_versions (automation_id, source, saved_by, saved_with, saved_at)
    SELECT id, source, created_by, 'editor', updated_at FROM automations;

CREATE TABLE automation_runs (
    id INTEGER PRIMARY KEY,
    automation_id INTEGER NOT NULL REFERENCES automations(id) ON DELETE CASCADE,
    trigger TEXT NOT NULL,
    started_at INTEGER NOT NULL,
    duration_us INTEGER NOT NULL,
    output TEXT NOT NULL,
    error TEXT
);
CREATE INDEX automation_runs_by_automation ON automation_runs (automation_id, id);

CREATE TABLE automation_reactions (
    message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    automation_id INTEGER NOT NULL REFERENCES automations(id) ON DELETE CASCADE,
    emoji TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (message_id, automation_id, emoji)
);

CREATE TABLE api_tokens (
    id INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    token_hash BLOB NOT NULL UNIQUE,
    created_at INTEGER NOT NULL,
    last_used_at INTEGER
);
",
    ),
    Migration::Sql(
        r"
ALTER TABLE automations ADD COLUMN kind TEXT NOT NULL DEFAULT 'automation';

CREATE TABLE secrets (
    name TEXT PRIMARY KEY,
    nonce BLOB NOT NULL,
    value BLOB NOT NULL,
    updated_by INTEGER REFERENCES users(id) ON DELETE SET NULL,
    updated_at INTEGER NOT NULL
);
",
    ),
    Migration::Sql(
        r"
ALTER TABLE files ADD COLUMN sha256 TEXT;
CREATE INDEX files_by_sha256 ON files (sha256);

ALTER TABLE users ADD COLUMN avatar_file_id INTEGER REFERENCES files(id) ON DELETE SET NULL;
ALTER TABLE users ADD COLUMN status_emoji TEXT NOT NULL DEFAULT '';
ALTER TABLE users ADD COLUMN status_text TEXT NOT NULL DEFAULT '';
ALTER TABLE users ADD COLUMN bio TEXT NOT NULL DEFAULT '';
ALTER TABLE users ADD COLUMN links TEXT NOT NULL DEFAULT '[]';
ALTER TABLE users ADD COLUMN favorite_emoji TEXT NOT NULL DEFAULT '';

ALTER TABLE messages ADD COLUMN gif TEXT;
",
    ),
    Migration::Sql(
        r"
CREATE TABLE gif_library (
    id INTEGER PRIMARY KEY,
    file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    tags TEXT NOT NULL DEFAULT '',
    width INTEGER NOT NULL DEFAULT 0,
    height INTEGER NOT NULL DEFAULT 0,
    added_by INTEGER REFERENCES users(id) ON DELETE SET NULL,
    created_at INTEGER NOT NULL,
    uses INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX gif_library_by_file ON gif_library (file_id);
",
    ),
    Migration::Sql(
        r"
ALTER TABLE messages ADD COLUMN edited_at INTEGER;
ALTER TABLE messages ADD COLUMN deleted_at INTEGER;
ALTER TABLE messages ADD COLUMN pinned_at INTEGER;
ALTER TABLE messages ADD COLUMN pinned_by INTEGER REFERENCES users(id) ON DELETE SET NULL;
CREATE INDEX messages_pinned ON messages (channel_id, pinned_at) WHERE pinned_at IS NOT NULL;

CREATE TABLE saved_messages (
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, message_id)
);
",
    ),
    Migration::Sql(
        r"
ALTER TABLE users ADD COLUMN deactivated_at INTEGER;

CREATE TABLE password_resets (
    token_hash BLOB PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_by INTEGER REFERENCES users(id) ON DELETE SET NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
);
",
    ),
    Migration::Sql(
        r"
ALTER TABLE channels ADD COLUMN private INTEGER NOT NULL DEFAULT 0;

-- Per person: `hidden` for a public channel they left, `muted` for one
-- that shouldn't draw attention.
CREATE TABLE channel_prefs (
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    channel_id INTEGER NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    hidden INTEGER NOT NULL DEFAULT 0,
    muted INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (user_id, channel_id)
);
",
    ),
    Migration::Sql(
        r"
-- Mentions and thread replies for each person's Activity page.
CREATE TABLE activity (
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    reason TEXT NOT NULL CHECK (reason IN ('mention', 'reply')),
    PRIMARY KEY (user_id, message_id)
);
CREATE INDEX activity_by_message ON activity (message_id);
ALTER TABLE users ADD COLUMN activity_seen_id INTEGER NOT NULL DEFAULT 0;
",
    ),
    Migration::Sql(
        r"
-- The browser reports each person's time zone, for reading `at 3pm`.
ALTER TABLE users ADD COLUMN timezone TEXT NOT NULL DEFAULT 'UTC';

CREATE TABLE reminders (
    id INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    text TEXT NOT NULL,
    message_id INTEGER REFERENCES messages(id) ON DELETE SET NULL,
    remind_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX reminders_due ON reminders (remind_at);

CREATE TABLE scheduled_messages (
    id INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    channel_id INTEGER NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    parent_id INTEGER REFERENCES messages(id) ON DELETE CASCADE,
    body TEXT NOT NULL,
    send_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX scheduled_messages_due ON scheduled_messages (send_at);
",
    ),
    Migration::Sql(
        r"
ALTER TABLE messages ADD COLUMN preview TEXT;
",
    ),
    Migration::Sql(
        r"
-- Where an imported message came from, so imports never add it twice.
ALTER TABLE messages ADD COLUMN import_id TEXT;
CREATE UNIQUE INDEX messages_by_import_id ON messages (import_id) WHERE import_id IS NOT NULL;
ALTER TABLE messages ADD COLUMN slack_format INTEGER NOT NULL DEFAULT 0;
",
    ),
    Migration::Sql(
        r"
-- A poll's question and options; votes are one per person.
ALTER TABLE messages ADD COLUMN poll TEXT;
CREATE TABLE poll_votes (
    message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    option INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (message_id, user_id)
);

-- Buttons under an automation's message.
ALTER TABLE messages ADD COLUMN buttons TEXT;

CREATE TABLE outgoing_webhooks (
    id INTEGER PRIMARY KEY,
    channel_id INTEGER NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    url TEXT NOT NULL,
    triggers TEXT NOT NULL DEFAULT '',
    token TEXT NOT NULL,
    created_by INTEGER REFERENCES users(id) ON DELETE SET NULL,
    created_at INTEGER NOT NULL,
    last_at INTEGER,
    last_status INTEGER,
    last_error TEXT
);
CREATE INDEX outgoing_webhooks_by_channel ON outgoing_webhooks (channel_id);
",
    ),
    Migration::Sql(
        r"
-- Who may start threads, reply and react in a channel: `everyone`, or
-- `managers` for announcement channels. Admins always manage.
ALTER TABLE channels ADD COLUMN post_policy TEXT NOT NULL DEFAULT 'everyone';
ALTER TABLE channels ADD COLUMN reply_policy TEXT NOT NULL DEFAULT 'everyone';
ALTER TABLE channels ADD COLUMN react_policy TEXT NOT NULL DEFAULT 'everyone';
CREATE TABLE channel_managers (
    channel_id INTEGER NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    PRIMARY KEY (channel_id, user_id)
);
INSERT INTO channel_managers (channel_id, user_id)
    SELECT id, created_by FROM channels WHERE kind = 'public' AND created_by IS NOT NULL;
",
    ),
    Migration::Sql(
        r"
-- A theme name and `system`, `light` or `dark`; empty means the
-- instance's default.
ALTER TABLE users ADD COLUMN theme TEXT NOT NULL DEFAULT '';
ALTER TABLE users ADD COLUMN appearance TEXT NOT NULL DEFAULT '';
",
    ),
    Migration::Sql(
        r"
-- Choices in polls where people pick several options (rank 0) or rank
-- them (1 is the favorite). Single-choice polls keep using poll_votes.
CREATE TABLE poll_marks (
    message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    option INTEGER NOT NULL,
    rank INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (message_id, user_id, option)
);
",
    ),
    Migration::Code(rebuild_search),
    Migration::Sql(
        r"
-- Trust levels: 0 New, 1 Basic, 2 Member, 3 Regular, 4 Leader. People who
-- were already here keep doing everything they could: members start at 2.
ALTER TABLE users ADD COLUMN trust_level INTEGER NOT NULL DEFAULT 1;
ALTER TABLE users ADD COLUMN trust_locked INTEGER NOT NULL DEFAULT 0;
ALTER TABLE users ADD COLUMN days_visited INTEGER NOT NULL DEFAULT 0;
ALTER TABLE users ADD COLUMN last_visit_day TEXT NOT NULL DEFAULT '';
-- Timed out by a moderator until then: they read but don't post.
ALTER TABLE users ADD COLUMN muted_until INTEGER;
UPDATE users SET trust_level = CASE WHEN is_admin THEN 4 ELSE 2 END;
CREATE INDEX messages_by_user ON messages (user_id, created_at);

CREATE TABLE roles (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE COLLATE NOCASE,
    description TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL
);
CREATE TABLE user_roles (
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role_id INTEGER NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
    PRIMARY KEY (user_id, role_id)
);
CREATE TABLE role_permissions (
    role_id INTEGER NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
    permission TEXT NOT NULL,
    PRIMARY KEY (role_id, permission)
);
-- The trust level a permission asks for, when an admin changed it from
-- the default; NULL for admins and roles only.
CREATE TABLE permission_levels (
    permission TEXT PRIMARY KEY,
    min_level INTEGER
);

-- People who asked to join and wait for approval.
CREATE TABLE signups (
    id INTEGER PRIMARY KEY,
    username TEXT NOT NULL UNIQUE COLLATE NOCASE,
    display_name TEXT NOT NULL,
    password_hash TEXT NOT NULL,
    note TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL
);

CREATE TABLE reports (
    id INTEGER PRIMARY KEY,
    message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    reporter_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    reason TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    resolved_at INTEGER,
    resolved_by INTEGER REFERENCES users(id) ON DELETE SET NULL,
    outcome TEXT,
    UNIQUE (message_id, reporter_id)
);
CREATE INDEX reports_open ON reports (resolved_at);
",
    ),
    Migration::Sql(
        r"
-- Passkeys: a public key per device, in SPKI form, and its COSE algorithm.
CREATE TABLE passkeys (
    id INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    credential_id BLOB NOT NULL UNIQUE,
    public_key BLOB NOT NULL,
    algorithm INTEGER NOT NULL,
    sign_count INTEGER NOT NULL DEFAULT 0,
    transports TEXT NOT NULL DEFAULT '',
    name TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    last_used_at INTEGER
);
CREATE INDEX passkeys_by_user ON passkeys (user_id);
ALTER TABLE users ADD COLUMN webauthn_handle BLOB;

-- Authenticator apps: the secret, sealed with the secret key, and the
-- last 30-second step used, so a code works once.
ALTER TABLE users ADD COLUMN totp_secret TEXT;
ALTER TABLE users ADD COLUMN totp_last_step INTEGER NOT NULL DEFAULT 0;
CREATE TABLE recovery_codes (
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    code_hash BLOB NOT NULL,
    used_at INTEGER,
    PRIMARY KEY (user_id, code_hash)
);

-- A confirmed address, for sign-in links and password resets.
ALTER TABLE users ADD COLUMN email TEXT;
CREATE UNIQUE INDEX users_by_email ON users (email) WHERE email IS NOT NULL;
CREATE TABLE login_links (
    token_hash BLOB PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    purpose TEXT NOT NULL,
    email TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
);

-- Between the password and the second step of signing in.
CREATE TABLE pending_logins (
    token_hash BLOB PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0
);
",
    ),
];

/// Recreates the search index with prefix indexes, which make the prefix
/// matching every search does fast, and a view of its vocabulary for
/// spelling suggestions. Poll options become searchable too.
fn rebuild_search(tx: &Transaction<'_>) -> AppResult<()> {
    tx.execute_batch(
        "DROP TABLE messages_fts;
         CREATE VIRTUAL TABLE messages_fts USING fts5(
             content, tokenize = 'unicode61 remove_diacritics 2', prefix = '2 3');
         CREATE VIRTUAL TABLE messages_fts_terms USING fts5vocab(messages_fts, 'row');",
    )?;
    crate::store::rebuild_search_index(tx)?;
    tx.execute_batch("INSERT INTO messages_fts (messages_fts) VALUES ('optimize');")?;
    Ok(())
}

/// The schema version this build writes: one per migration.
pub fn schema_version() -> i64 {
    i64::try_from(MIGRATIONS.len()).unwrap_or(i64::MAX)
}

/// The `SQLite` database. rusqlite is synchronous, so every query runs on
/// Tokio's blocking pool behind one connection.
#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
}

impl Db {
    /// Opens the database and brings its schema up to date. Before changing
    /// a database an older version wrote, it keeps a copy in
    /// `upgrade-backups/` next to it.
    pub fn open(path: &Path) -> AppResult<Self> {
        let mut conn = connect(path)?;
        let applied = user_version(&conn)?;
        check_not_newer(&conn, applied)?;
        if applied > 0 && applied < schema_version() {
            let kept = keep_copy(&conn, path, applied)?;
            tracing::info!(
                from = applied,
                to = schema_version(),
                copy = %kept.display(),
                "upgrading the database; kept a copy of it first"
            );
        }
        migrate(&mut conn, schema_version())?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Runs `f` with exclusive access to the connection.
    pub async fn call<T, F>(&self, f: F) -> AppResult<T>
    where
        F: FnOnce(&mut Connection) -> AppResult<T> + Send + 'static,
        T: Send + 'static,
    {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let mut guard = conn
                .lock()
                .map_err(|_| AppError::internal("database lock poisoned"))?;
            f(&mut guard)
        })
        .await
        .map_err(AppError::internal)?
    }
}

/// Opens a connection with Sideporch's settings. Besides the main one,
/// the automation runtime keeps its own for automation data.
pub fn connect(path: &Path) -> AppResult<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    Ok(conn)
}

fn user_version(conn: &Connection) -> AppResult<i64> {
    Ok(conn.pragma_query_value(None, "user_version", |row| row.get(0))?)
}

/// Which Sideporch release applied each schema version, so a database
/// from a newer release can name it. Not part of the numbered schema.
const HISTORY: &str = "CREATE TABLE IF NOT EXISTS schema_history (
    version INTEGER PRIMARY KEY,
    app_version TEXT NOT NULL,
    applied_at INTEGER NOT NULL
)";

/// Refuses a database written by a newer Sideporch: this build would not
/// know its tables, and could damage data it doesn't understand.
fn check_not_newer(conn: &Connection, applied: i64) -> AppResult<()> {
    if applied <= schema_version() {
        return Ok(());
    }
    let writer: Option<String> = conn
        .query_row(
            "SELECT app_version FROM schema_history WHERE version = ?1",
            [applied],
            |row| row.get(0),
        )
        .optional()
        .ok()
        .flatten();
    let writer = writer.map_or_else(
        || "a newer Sideporch".to_owned(),
        |version| format!("Sideporch {version}"),
    );
    Err(AppError::internal(format!(
        "this database was written by {writer} (schema {applied}), but this is Sideporch {} (schema {}). \
         Run {writer} or later, or restore a backup made with this version; going back would lose data.",
        env!("CARGO_PKG_VERSION"),
        schema_version(),
    )))
}

/// How many pre-upgrade copies to keep.
const KEEP_COPIES: usize = 3;
pub const UPGRADE_DIR: &str = "upgrade-backups";

/// Copies the database into `upgrade-backups/` before migrating it, and
/// removes all but the newest few copies. Uploaded files are stored by
/// content and never changed by migrations, so the database is enough.
fn keep_copy(conn: &Connection, path: &Path, applied: i64) -> AppResult<PathBuf> {
    let dir = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(UPGRADE_DIR);
    std::fs::create_dir_all(&dir).map_err(AppError::internal)?;
    let stamp = jiff::Timestamp::now().strftime("%Y%m%d-%H%M%S");
    let copy = dir.join(format!("sideporch-schema{applied}-{stamp}.db"));
    let target = copy
        .to_str()
        .ok_or_else(|| AppError::internal("the data directory path is not UTF-8"))?;
    if copy.exists() {
        std::fs::remove_file(&copy).map_err(AppError::internal)?;
    }
    conn.execute("VACUUM INTO ?1", [target])?;
    let mut copies: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map_err(AppError::internal)?
        .flatten()
        .map(|entry| entry.path())
        .filter(|file| {
            file.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("sideporch-schema"))
                && file.extension().is_some_and(|extension| extension == "db")
        })
        .collect();
    // Names sort by time within a schema; the modification time orders all.
    copies.sort_by_key(|file| {
        std::cmp::Reverse(
            file.metadata()
                .and_then(|meta| meta.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH),
        )
    });
    for old in copies.into_iter().skip(KEEP_COPIES) {
        drop(std::fs::remove_file(old));
    }
    Ok(copy)
}

/// Applies the migrations after the database's version, up to `target`,
/// each in its own transaction. The version is read inside the
/// transaction, so two servers starting on one database can't both apply
/// a step.
fn migrate(conn: &mut Connection, target: i64) -> AppResult<()> {
    conn.execute_batch(HISTORY)?;
    for (version, migration) in (1_i64..).zip(MIGRATIONS) {
        if version > target {
            break;
        }
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if user_version(&tx)? >= version {
            continue;
        }
        match migration {
            Migration::Sql(sql) => tx.execute_batch(sql)?,
            Migration::Code(step) => step(&tx)?,
        }
        tx.pragma_update(None, "user_version", version)?;
        tx.execute(
            "INSERT OR REPLACE INTO schema_history (version, app_version, applied_at) VALUES (?1, ?2, ?3)",
            params![version, env!("CARGO_PKG_VERSION"), crate::now_ms()],
        )?;
        tx.commit()?;
    }
    let broken: i64 =
        conn.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })?;
    if broken > 0 {
        tracing::warn!(broken, "rows point at rows that no longer exist");
    }
    Ok(())
}

/// Builds a database at an old schema version, for upgrade tests.
#[doc(hidden)]
pub fn create_at_version(path: &Path, version: i64) -> AppResult<()> {
    let mut conn = connect(path)?;
    migrate(&mut conn, version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_version_upgrades_to_the_latest() {
        let dir = tempfile::tempdir().unwrap();
        for version in 0..=schema_version() {
            let path = dir.path().join(format!("v{version}.db"));
            create_at_version(&path, version).unwrap();
            let db = Db::open(&path).unwrap();
            drop(db);
            let conn = connect(&path).unwrap();
            assert_eq!(user_version(&conn).unwrap(), schema_version());
        }
        // Each database older than the latest left a copy behind.
        let copies = std::fs::read_dir(dir.path().join(UPGRADE_DIR))
            .unwrap()
            .count();
        assert_eq!(copies, KEEP_COPIES);
    }

    #[test]
    fn refuses_a_database_from_a_newer_release() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sideporch.db");
        drop(Db::open(&path).unwrap());
        let newer = schema_version() + 1;
        let conn = connect(&path).unwrap();
        conn.pragma_update(None, "user_version", newer).unwrap();
        conn.execute(
            "INSERT INTO schema_history (version, app_version, applied_at) VALUES (?1, '9.9.9', 0)",
            [newer],
        )
        .unwrap();
        drop(conn);
        let error = Db::open(&path).err().unwrap().to_string();
        assert!(error.contains("Sideporch 9.9.9"), "{error}");
        // It was left alone.
        let conn = connect(&path).unwrap();
        assert_eq!(user_version(&conn).unwrap(), newer);
    }
}
