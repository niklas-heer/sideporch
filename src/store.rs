//! Queries over the `SQLite` database. Functions here are synchronous and
//! run inside [`crate::db::Db::call`].

use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::{error::AppResult, webhook::Attachment};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelKind {
    Public,
    Direct,
}

#[derive(Debug, Clone)]
pub struct Channel {
    pub id: i64,
    pub kind: ChannelKind,
    pub name: String,
    pub topic: String,
}

#[derive(Debug, Clone)]
pub struct SidebarItem {
    pub channel_id: i64,
    pub label: String,
    pub unread: bool,
}

#[derive(Debug, Clone)]
pub struct Sidebar {
    pub channels: Vec<SidebarItem>,
    pub direct: Vec<SidebarItem>,
}

#[derive(Debug, Clone)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub display_name: String,
    pub is_admin: bool,
}

#[derive(Debug, Clone)]
pub enum Author {
    User { id: i64, display_name: String },
    Bot { name: String, icon: Option<String> },
    Removed,
}

#[derive(Debug, Clone)]
pub struct Message {
    pub id: i64,
    pub channel_id: i64,
    pub parent_id: Option<i64>,
    pub author: Author,
    pub body: String,
    pub attachments: Vec<Attachment>,
    pub created_at: i64,
    pub reply_count: i64,
}

pub struct NewMessage<'a> {
    pub channel_id: i64,
    pub parent_id: Option<i64>,
    pub user_id: Option<i64>,
    pub webhook_id: Option<i64>,
    pub bot_name: Option<&'a str>,
    pub bot_icon: Option<&'a str>,
    pub body: &'a str,
    pub attachments: &'a [Attachment],
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct Invite {
    pub token: String,
    pub created_by: String,
    pub expires_at: i64,
    pub uses: i64,
}

#[derive(Debug, Clone)]
pub struct Webhook {
    pub id: i64,
    pub channel_id: i64,
    pub name: String,
    pub token: String,
}

// Users

pub fn user_count(conn: &Connection) -> AppResult<i64> {
    Ok(conn.query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0))?)
}

pub fn username_taken(conn: &Connection, username: &str) -> AppResult<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM users WHERE username = ?1",
            [username],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

pub fn create_user(
    conn: &Connection,
    username: &str,
    display_name: &str,
    password_hash: &str,
    is_admin: bool,
    now: i64,
) -> AppResult<i64> {
    conn.execute(
        "INSERT INTO users (username, display_name, password_hash, is_admin, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![username, display_name, password_hash, is_admin, now],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Returns the user's id and password hash.
pub fn login_record(conn: &Connection, username: &str) -> AppResult<Option<(i64, String)>> {
    Ok(conn
        .query_row(
            "SELECT id, password_hash FROM users WHERE username = ?1",
            [username],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?)
}

pub fn users(conn: &Connection) -> AppResult<Vec<User>> {
    let mut statement = conn.prepare(
        "SELECT id, username, display_name, is_admin FROM users ORDER BY display_name COLLATE NOCASE",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(User {
            id: row.get(0)?,
            username: row.get(1)?,
            display_name: row.get(2)?,
            is_admin: row.get(3)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn user_exists(conn: &Connection, id: i64) -> AppResult<bool> {
    Ok(conn
        .query_row("SELECT 1 FROM users WHERE id = ?1", [id], |_| Ok(()))
        .optional()?
        .is_some())
}

pub fn delete_expired_sessions(conn: &Connection, now: i64) -> AppResult<usize> {
    Ok(conn.execute("DELETE FROM sessions WHERE expires_at <= ?1", [now])?)
}

// Channels

pub fn create_channel(conn: &Connection, name: &str, created_by: i64, now: i64) -> AppResult<i64> {
    conn.execute(
        "INSERT INTO channels (kind, name, created_by, created_at) VALUES ('public', ?1, ?2, ?3)",
        params![name, created_by, now],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn public_channel_id(conn: &Connection, name: &str) -> AppResult<Option<i64>> {
    Ok(conn
        .query_row(
            "SELECT id FROM channels WHERE kind = 'public' AND name = ?1",
            [name],
            |row| row.get(0),
        )
        .optional()?)
}

pub fn set_topic(conn: &Connection, channel_id: i64, topic: &str) -> AppResult<()> {
    conn.execute(
        "UPDATE channels SET topic = ?1 WHERE id = ?2",
        params![topic, channel_id],
    )?;
    Ok(())
}

/// Returns the channel if `user_id` may read it. Direct conversations are
/// visible to their members only.
pub fn channel_for(conn: &Connection, channel_id: i64, user_id: i64) -> AppResult<Option<Channel>> {
    Ok(conn
        .query_row(
            "SELECT c.id, c.kind, c.name, c.topic,
                    (SELECT u.display_name FROM channel_members m JOIN users u ON u.id = m.user_id
                     WHERE m.channel_id = c.id AND m.user_id != ?2),
                    (SELECT u.display_name FROM users u WHERE u.id = ?2)
             FROM channels c
             WHERE c.id = ?1 AND (c.kind = 'public' OR EXISTS (
                 SELECT 1 FROM channel_members m WHERE m.channel_id = c.id AND m.user_id = ?2))",
            params![channel_id, user_id],
            |row| {
                let kind: String = row.get(1)?;
                let name: Option<String> = row.get(2)?;
                let other: Option<String> = row.get(4)?;
                let me: Option<String> = row.get(5)?;
                let (kind, name) = if kind == "dm" {
                    let label =
                        other.unwrap_or_else(|| format!("{} (you)", me.unwrap_or_default()));
                    (ChannelKind::Direct, label)
                } else {
                    (ChannelKind::Public, name.unwrap_or_default())
                };
                Ok(Channel {
                    id: row.get(0)?,
                    kind,
                    name,
                    topic: row.get(3)?,
                })
            },
        )
        .optional()?)
}

/// Members who receive live updates for a direct conversation, or `None`
/// for a public channel that everyone can read.
pub fn audience(conn: &Connection, channel_id: i64) -> AppResult<Option<Vec<i64>>> {
    let kind: String = conn.query_row(
        "SELECT kind FROM channels WHERE id = ?1",
        [channel_id],
        |row| row.get(0),
    )?;
    if kind != "dm" {
        return Ok(None);
    }
    let mut statement =
        conn.prepare("SELECT user_id FROM channel_members WHERE channel_id = ?1")?;
    let members = statement.query_map([channel_id], |row| row.get(0))?;
    Ok(Some(members.collect::<Result<_, _>>()?))
}

/// Finds or creates the direct conversation between two users.
pub fn direct_channel(conn: &mut Connection, user: i64, other: i64, now: i64) -> AppResult<i64> {
    let key = format!("{}:{}", user.min(other), user.max(other));
    let tx = conn.transaction()?;
    let existing: Option<i64> = tx
        .query_row("SELECT id FROM channels WHERE dm_key = ?1", [&key], |row| {
            row.get(0)
        })
        .optional()?;
    let id = if let Some(id) = existing {
        id
    } else {
        tx.execute(
            "INSERT INTO channels (kind, dm_key, created_by, created_at) VALUES ('dm', ?1, ?2, ?3)",
            params![key, user, now],
        )?;
        let id = tx.last_insert_rowid();
        for member in [user, other] {
            tx.execute(
                "INSERT OR IGNORE INTO channel_members (channel_id, user_id) VALUES (?1, ?2)",
                params![id, member],
            )?;
        }
        id
    };
    tx.commit()?;
    Ok(id)
}

pub fn sidebar(conn: &Connection, user_id: i64) -> AppResult<Sidebar> {
    let unread = "COALESCE((SELECT MAX(m.id) FROM messages m WHERE m.channel_id = c.id), 0)
                  > COALESCE((SELECT r.last_read_id FROM reads r WHERE r.channel_id = c.id AND r.user_id = ?1), 0)";
    let mut statement = conn.prepare(&format!(
        "SELECT c.id, c.name, {unread} FROM channels c WHERE c.kind = 'public' ORDER BY c.name COLLATE NOCASE"
    ))?;
    let channels = statement
        .query_map([user_id], |row| {
            Ok(SidebarItem {
                channel_id: row.get(0)?,
                label: row.get(1)?,
                unread: row.get(2)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    let mut statement = conn.prepare(&format!(
        "SELECT c.id,
                COALESCE((SELECT u.display_name FROM channel_members o JOIN users u ON u.id = o.user_id
                          WHERE o.channel_id = c.id AND o.user_id != ?1),
                         (SELECT display_name || ' (you)' FROM users WHERE id = ?1)),
                {unread}
         FROM channels c JOIN channel_members me ON me.channel_id = c.id AND me.user_id = ?1
         WHERE c.kind = 'dm'
         ORDER BY COALESCE((SELECT MAX(m.id) FROM messages m WHERE m.channel_id = c.id), 0) DESC"
    ))?;
    let direct = statement
        .query_map([user_id], |row| {
            Ok(SidebarItem {
                channel_id: row.get(0)?,
                label: row.get(1)?,
                unread: row.get(2)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(Sidebar { channels, direct })
}

/// The channel to open after signing in: `general` if it exists.
pub fn home_channel(conn: &Connection) -> AppResult<Option<i64>> {
    Ok(conn
        .query_row(
            "SELECT id FROM channels WHERE kind = 'public' ORDER BY name != 'general', id LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()?)
}

// Messages

const MESSAGE_SELECT: &str = "SELECT m.id, m.channel_id, m.parent_id, m.user_id, u.display_name,
        m.bot_name, m.bot_icon_url, m.body, m.attachments, m.created_at,
        (SELECT COUNT(*) FROM messages r WHERE r.parent_id = m.id)
    FROM messages m LEFT JOIN users u ON u.id = m.user_id";

fn message_from_row(row: &Row<'_>) -> rusqlite::Result<Message> {
    let user_id: Option<i64> = row.get(3)?;
    let display_name: Option<String> = row.get(4)?;
    let bot_name: Option<String> = row.get(5)?;
    let author = match (user_id, display_name, bot_name) {
        (Some(id), Some(display_name), _) => Author::User { id, display_name },
        (_, _, Some(name)) => Author::Bot {
            name,
            icon: row.get(6)?,
        },
        _ => Author::Removed,
    };
    let attachments: Option<String> = row.get(8)?;
    Ok(Message {
        id: row.get(0)?,
        channel_id: row.get(1)?,
        parent_id: row.get(2)?,
        author,
        body: row.get(7)?,
        attachments: attachments
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default(),
        created_at: row.get(9)?,
        reply_count: row.get(10)?,
    })
}

/// The newest top-level messages of a channel, oldest first. With
/// `before`, returns the page of messages older than that id.
pub fn channel_messages(
    conn: &Connection,
    channel_id: i64,
    before: Option<i64>,
    limit: u32,
) -> AppResult<Vec<Message>> {
    let mut statement = conn.prepare(&format!(
        "{MESSAGE_SELECT} WHERE m.channel_id = ?1 AND m.parent_id IS NULL AND (?2 IS NULL OR m.id < ?2)
         ORDER BY m.id DESC LIMIT ?3"
    ))?;
    let mut messages = statement
        .query_map(params![channel_id, before, limit], message_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    messages.reverse();
    Ok(messages)
}

pub fn message(conn: &Connection, id: i64) -> AppResult<Option<Message>> {
    Ok(conn
        .query_row(
            &format!("{MESSAGE_SELECT} WHERE m.id = ?1"),
            [id],
            message_from_row,
        )
        .optional()?)
}

pub fn replies(conn: &Connection, parent_id: i64) -> AppResult<Vec<Message>> {
    let mut statement = conn.prepare(&format!(
        "{MESSAGE_SELECT} WHERE m.parent_id = ?1 ORDER BY m.id"
    ))?;
    let replies = statement.query_map([parent_id], message_from_row)?;
    Ok(replies.collect::<Result<_, _>>()?)
}

pub fn insert_message(conn: &Connection, new: &NewMessage<'_>) -> AppResult<i64> {
    let attachments = if new.attachments.is_empty() {
        None
    } else {
        Some(serde_json::to_string(new.attachments).map_err(crate::error::AppError::internal)?)
    };
    conn.execute(
        "INSERT INTO messages (channel_id, parent_id, user_id, webhook_id, bot_name, bot_icon_url, body, attachments, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            new.channel_id,
            new.parent_id,
            new.user_id,
            new.webhook_id,
            new.bot_name,
            new.bot_icon,
            new.body,
            attachments,
            new.created_at
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Records that `user_id` has seen `channel_id` up to `message_id`.
pub fn mark_read(
    conn: &Connection,
    user_id: i64,
    channel_id: i64,
    message_id: i64,
) -> AppResult<()> {
    conn.execute(
        "INSERT INTO reads (user_id, channel_id, last_read_id) VALUES (?1, ?2, ?3)
         ON CONFLICT (user_id, channel_id) DO UPDATE SET last_read_id = MAX(last_read_id, excluded.last_read_id)",
        params![user_id, channel_id, message_id],
    )?;
    Ok(())
}

pub fn latest_message_id(conn: &Connection, channel_id: i64) -> AppResult<Option<i64>> {
    Ok(conn.query_row(
        "SELECT MAX(id) FROM messages WHERE channel_id = ?1",
        [channel_id],
        |row| row.get(0),
    )?)
}

// Invites

pub fn create_invite(
    conn: &Connection,
    token: &str,
    created_by: i64,
    now: i64,
    expires_at: i64,
) -> AppResult<()> {
    conn.execute(
        "INSERT INTO invites (token, created_by, created_at, expires_at) VALUES (?1, ?2, ?3, ?4)",
        params![token, created_by, now, expires_at],
    )?;
    Ok(())
}

pub fn active_invites(conn: &Connection, now: i64) -> AppResult<Vec<Invite>> {
    let mut statement = conn.prepare(
        "SELECT i.token, u.display_name, i.expires_at, i.uses
         FROM invites i JOIN users u ON u.id = i.created_by
         WHERE i.revoked = 0 AND i.expires_at > ?1 ORDER BY i.created_at DESC",
    )?;
    let invites = statement.query_map([now], |row| {
        Ok(Invite {
            token: row.get(0)?,
            created_by: row.get(1)?,
            expires_at: row.get(2)?,
            uses: row.get(3)?,
        })
    })?;
    Ok(invites.collect::<Result<_, _>>()?)
}

pub fn invite_is_valid(conn: &Connection, token: &str, now: i64) -> AppResult<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM invites WHERE token = ?1 AND revoked = 0 AND expires_at > ?2",
            params![token, now],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

pub fn record_invite_use(conn: &Connection, token: &str) -> AppResult<()> {
    conn.execute(
        "UPDATE invites SET uses = uses + 1 WHERE token = ?1",
        [token],
    )?;
    Ok(())
}

pub fn revoke_invite(conn: &Connection, token: &str) -> AppResult<()> {
    conn.execute("UPDATE invites SET revoked = 1 WHERE token = ?1", [token])?;
    Ok(())
}

// Webhooks

pub fn create_webhook(
    conn: &Connection,
    channel_id: i64,
    name: &str,
    token: &str,
    created_by: i64,
    now: i64,
) -> AppResult<()> {
    conn.execute(
        "INSERT INTO webhooks (channel_id, name, token, created_by, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![channel_id, name, token, created_by, now],
    )?;
    Ok(())
}

fn webhook_from_row(row: &Row<'_>) -> rusqlite::Result<Webhook> {
    Ok(Webhook {
        id: row.get(0)?,
        channel_id: row.get(1)?,
        name: row.get(2)?,
        token: row.get(3)?,
    })
}

pub fn webhooks(conn: &Connection, channel_id: i64) -> AppResult<Vec<Webhook>> {
    let mut statement = conn.prepare(
        "SELECT id, channel_id, name, token FROM webhooks WHERE channel_id = ?1 ORDER BY id",
    )?;
    let hooks = statement.query_map([channel_id], webhook_from_row)?;
    Ok(hooks.collect::<Result<_, _>>()?)
}

pub fn webhook_by_token(conn: &Connection, token: &str) -> AppResult<Option<Webhook>> {
    Ok(conn
        .query_row(
            "SELECT id, channel_id, name, token FROM webhooks WHERE token = ?1",
            [token],
            webhook_from_row,
        )
        .optional()?)
}

pub fn delete_webhook(conn: &Connection, channel_id: i64, webhook_id: i64) -> AppResult<()> {
    conn.execute(
        "DELETE FROM webhooks WHERE id = ?1 AND channel_id = ?2",
        params![webhook_id, channel_id],
    )?;
    Ok(())
}
