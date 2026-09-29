//! Demo mode, for servers anyone may try: every day at a set hour (UTC) the
//! server forgets everything but what an admin chose to keep. Kept are
//! channels marked to keep, with their messages from people who stay;
//! admins and people with a role; automations, custom emoji, the GIF
//! library, bans and settings. Everyone and everything else goes.

use std::time::Duration;

use jiff::Timestamp;
use rusqlite::Connection;

use crate::{AppState, error::AppResult, store};

/// Whether demo mode is on, and when it resets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Demo {
    pub enabled: bool,
    /// The hour of the day, in UTC, the reset runs at.
    pub hour: u8,
}

impl Default for Demo {
    fn default() -> Self {
        Self {
            enabled: false,
            hour: 4,
        }
    }
}

impl Demo {
    pub fn load(conn: &Connection) -> AppResult<Self> {
        let enabled = store::setting(conn, "demo.enabled")?.as_deref() == Some("1");
        let hour = store::setting(conn, "demo.hour")?
            .and_then(|hour| hour.parse().ok())
            .filter(|hour| *hour < 24)
            .unwrap_or(4);
        Ok(Self { enabled, hour })
    }

    pub fn save(self, conn: &Connection) -> AppResult<()> {
        store::set_setting(conn, "demo.enabled", if self.enabled { "1" } else { "0" })?;
        store::set_setting(conn, "demo.hour", &self.hour.to_string())
    }

    /// What people are told, such as "This is a demo: …".
    pub fn notice(self) -> String {
        format!(
            "This is a demo. Every day at {:02}:00 UTC, everything except the kept channels starts over.",
            self.hour
        )
    }
}

/// Marks the channels resets keep; only named channels can be kept.
pub fn set_kept(conn: &Connection, channel_ids: &[i64]) -> AppResult<()> {
    conn.execute("UPDATE channels SET kept = 0", [])?;
    let mut statement =
        conn.prepare("UPDATE channels SET kept = 1 WHERE id = ?1 AND kind = 'public'")?;
    for id in channel_ids {
        statement.execute([id])?;
    }
    Ok(())
}

/// What a reset removed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Removed {
    pub people: i64,
    pub channels: usize,
    pub messages: i64,
}

/// Forgets everything that isn't kept.
pub fn reset(conn: &mut Connection) -> AppResult<Removed> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    // People who go: not admins, no role, from this server.
    tx.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS leaving (id INTEGER PRIMARY KEY);
         DELETE FROM leaving;
         INSERT INTO leaving (id) SELECT id FROM users
             WHERE is_admin = 0 AND instance_id IS NULL
               AND id NOT IN (SELECT user_id FROM user_roles);",
    )?;
    let people: i64 = tx.query_row("SELECT COUNT(*) FROM leaving", [], |row| row.get(0))?;
    let before: i64 = tx.query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))?;
    // Their messages go even from kept channels, then every channel that
    // isn't kept, with its messages, members and webhooks.
    tx.execute(
        "DELETE FROM messages WHERE user_id IN (SELECT id FROM leaving)",
        [],
    )?;
    let channels = tx.execute("DELETE FROM channels WHERE kept = 0", [])?;
    tx.execute("DELETE FROM users WHERE id IN (SELECT id FROM leaving)", [])?;
    tx.execute("DROP TABLE leaving", [])?;
    tx.execute_batch(
        "DELETE FROM invites;
         DELETE FROM signups;
         DELETE FROM reports;
         DELETE FROM messages_fts WHERE rowid NOT IN (SELECT id FROM messages);
         DELETE FROM files WHERE id NOT IN (SELECT file_id FROM message_files)
             AND id NOT IN (SELECT file_id FROM custom_emoji)
             AND id NOT IN (SELECT file_id FROM gif_library)
             AND id NOT IN (SELECT avatar_file_id FROM users WHERE avatar_file_id IS NOT NULL);",
    )?;
    // A server needs a channel to land in.
    let public: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM channels WHERE kind = 'public' AND private = 0)",
        [],
        |row| row.get(0),
    )?;
    if !public {
        tx.execute(
            "INSERT INTO channels (kind, name, created_at) VALUES ('public', 'general', ?1)",
            [crate::now_ms()],
        )?;
    }
    let after: i64 = tx.query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))?;
    store::set_setting(&tx, "demo.last_reset", &Timestamp::now().to_string())?;
    tx.commit()?;
    Ok(Removed {
        people,
        channels,
        messages: before.saturating_sub(after),
    })
}

/// Resets, then tidies up what the reset left: unused file contents on
/// disk, cached statistics, and the automations.
pub async fn reset_now(state: &AppState) -> AppResult<Removed> {
    let removed = state.db.call(reset).await?;
    let blobs = state.blobs.clone();
    let collected = state.db.call(move |conn| blobs.collect_garbage(conn)).await;
    if let Err(error) = collected {
        tracing::warn!(?error, "could not remove unused files after a demo reset");
    }
    state.statistics.clear();
    state.automations.reload(state).await?;
    tracing::info!(
        people = removed.people,
        channels = removed.channels,
        messages = removed.messages,
        "demo reset"
    );
    Ok(removed)
}

/// Whether a reset is due now: demo mode is on, it's the hour, and none ran
/// in the last 23 hours.
fn due(conn: &Connection, now: Timestamp) -> AppResult<bool> {
    let demo = Demo::load(conn)?;
    if !demo.enabled
        || now.to_zoned(jiff::tz::TimeZone::UTC).hour() != i8::try_from(demo.hour).unwrap_or(4)
    {
        return Ok(false);
    }
    let last =
        store::setting(conn, "demo.last_reset")?.and_then(|last| last.parse::<Timestamp>().ok());
    Ok(last.is_none_or(|last| now.duration_since(last) > jiff::SignedDuration::from_hours(23)))
}

/// Checks every minute whether a reset is due.
pub fn start(state: AppState) {
    tokio::spawn(async move {
        let mut every_minute = tokio::time::interval(Duration::from_mins(1));
        loop {
            every_minute.tick().await;
            match state.db.call(|conn| due(conn, Timestamp::now())).await {
                Ok(true) => {
                    if let Err(error) = reset_now(&state).await {
                        tracing::error!(?error, "demo reset failed");
                    }
                }
                Ok(false) => {}
                Err(error) => tracing::warn!(?error, "could not check the demo schedule"),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resets_once_a_day_at_their_hour() {
        let dir = tempfile::tempdir().expect("a directory");
        let path = dir.path().join("sideporch.db");
        drop(crate::db::Db::open(&path).expect("a database"));
        let conn = crate::db::connect(&path).expect("a connection");
        let at = |text: &str| text.parse::<Timestamp>().expect("a time");
        assert!(!due(&conn, at("2026-09-30T04:10:00Z")).expect("checked"));
        Demo {
            enabled: true,
            hour: 4,
        }
        .save(&conn)
        .expect("saved");
        assert!(due(&conn, at("2026-09-30T04:10:00Z")).expect("checked"));
        assert!(!due(&conn, at("2026-09-30T05:10:00Z")).expect("checked"));
        store::set_setting(&conn, "demo.last_reset", "2026-09-30T04:00:30Z").expect("noted");
        assert!(!due(&conn, at("2026-09-30T04:30:00Z")).expect("checked"));
        assert!(due(&conn, at("2026-10-01T04:00:30Z")).expect("checked"));
    }
}
