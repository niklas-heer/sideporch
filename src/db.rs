use std::{
    path::Path,
    sync::{Arc, Mutex},
};

use rusqlite::Connection;

use crate::error::{AppError, AppResult};

/// Schema migrations, applied in order. `PRAGMA user_version` records how
/// many have run, so released entries must never change; append new ones.
const MIGRATIONS: &[&str] = &[r"
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
"];

/// The `SQLite` database. rusqlite is synchronous, so every query runs on
/// Tokio's blocking pool behind one connection.
#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
}

impl Db {
    pub fn open(path: &Path) -> AppResult<Self> {
        let mut conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
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

fn migrate(conn: &mut Connection) -> AppResult<()> {
    let applied: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    for (version, migration) in (1_i64..).zip(MIGRATIONS) {
        if version <= applied {
            continue;
        }
        let tx = conn.transaction()?;
        tx.execute_batch(migration)?;
        tx.pragma_update(None, "user_version", version)?;
        tx.commit()?;
    }
    Ok(())
}
