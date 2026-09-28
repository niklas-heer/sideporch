use std::{
    path::Path,
    sync::{Arc, Mutex},
};

use rusqlite::Connection;

use crate::error::{AppError, AppResult};

/// Schema migrations, applied in order. `PRAGMA user_version` records how
/// many have run, so released entries must never change; append new ones.
const MIGRATIONS: &[&str] = &[
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
];

/// The `SQLite` database. rusqlite is synchronous, so every query runs on
/// Tokio's blocking pool behind one connection.
#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
}

impl Db {
    pub fn open(path: &Path) -> AppResult<Self> {
        let mut conn = connect(path)?;
        migrate(&mut conn)?;
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

fn migrate(conn: &mut Connection) -> AppResult<()> {
    let applied: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    for (version, migration) in (1_i64..).zip(MIGRATIONS) {
        if version <= applied {
            continue;
        }
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute_batch(migration)?;
        tx.pragma_update(None, "user_version", version)?;
        tx.commit()?;
    }
    Ok(())
}
