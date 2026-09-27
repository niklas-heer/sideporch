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
    pub files: Vec<FileRef>,
    pub reactions: Vec<Reaction>,
}

/// A file attached to a message. The bytes are served from `/files/{id}`.
#[derive(Debug, Clone)]
pub struct FileRef {
    pub id: i64,
    pub name: String,
    pub mime: String,
    pub size: i64,
}

/// Everyone who reacted to a message with one emoji.
#[derive(Debug, Clone)]
pub struct Reaction {
    pub emoji: String,
    pub user_ids: Vec<i64>,
    pub names: Vec<String>,
}

pub struct NewMessage<'a> {
    pub channel_id: i64,
    pub parent_id: Option<i64>,
    pub user_id: Option<i64>,
    pub webhook_id: Option<i64>,
    pub bot_name: Option<&'a str>,
    pub bot_icon: Option<&'a str>,
    pub automation_id: Option<i64>,
    pub body: &'a str,
    pub attachments: &'a [Attachment],
    pub files: &'a [i64],
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
        files: Vec::new(),
        reactions: Vec::new(),
    })
}

/// Loads files and reactions for already loaded messages.
fn hydrate(conn: &Connection, messages: &mut [Message]) -> AppResult<()> {
    if messages.is_empty() {
        return Ok(());
    }
    let ids = serde_json::to_string(&messages.iter().map(|m| m.id).collect::<Vec<_>>())
        .map_err(crate::error::AppError::internal)?;
    let mut statement = conn.prepare(
        "SELECT mf.message_id, f.id, f.name, f.mime, f.size
         FROM message_files mf JOIN files f ON f.id = mf.file_id
         WHERE mf.message_id IN (SELECT value FROM json_each(?1))
         ORDER BY mf.message_id, mf.position",
    )?;
    let files = statement.query_map([&ids], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            FileRef {
                id: row.get(1)?,
                name: row.get(2)?,
                mime: row.get(3)?,
                size: row.get(4)?,
            },
        ))
    })?;
    for file in files {
        let (message_id, file) = file?;
        if let Some(message) = messages.iter_mut().find(|m| m.id == message_id) {
            message.files.push(file);
        }
    }
    let mut statement = conn.prepare(
        "SELECT r.message_id, r.emoji, r.user_id, COALESCE(u.display_name, 'Someone')
         FROM reactions r LEFT JOIN users u ON u.id = r.user_id
         WHERE r.message_id IN (SELECT value FROM json_each(?1))
         ORDER BY r.message_id, r.created_at, r.user_id",
    )?;
    let rows = statement.query_map([&ids], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    for row in rows {
        let (message_id, emoji, user_id, name) = row?;
        let Some(message) = messages.iter_mut().find(|m| m.id == message_id) else {
            continue;
        };
        if let Some(reaction) = message.reactions.iter_mut().find(|r| r.emoji == emoji) {
            reaction.user_ids.push(user_id);
            reaction.names.push(name);
        } else {
            message.reactions.push(Reaction {
                emoji,
                user_ids: vec![user_id],
                names: vec![name],
            });
        }
    }
    Ok(())
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
    hydrate(conn, &mut messages)?;
    Ok(messages)
}

pub fn message(conn: &Connection, id: i64) -> AppResult<Option<Message>> {
    let found = conn
        .query_row(
            &format!("{MESSAGE_SELECT} WHERE m.id = ?1"),
            [id],
            message_from_row,
        )
        .optional()?;
    let mut found: Vec<Message> = found.into_iter().collect();
    hydrate(conn, &mut found)?;
    Ok(found.pop())
}

pub fn replies(conn: &Connection, parent_id: i64) -> AppResult<Vec<Message>> {
    let mut statement = conn.prepare(&format!(
        "{MESSAGE_SELECT} WHERE m.parent_id = ?1 ORDER BY m.id"
    ))?;
    let mut replies = statement
        .query_map([parent_id], message_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    hydrate(conn, &mut replies)?;
    Ok(replies)
}

pub fn insert_message(conn: &Connection, new: &NewMessage<'_>) -> AppResult<i64> {
    let attachments = if new.attachments.is_empty() {
        None
    } else {
        Some(serde_json::to_string(new.attachments).map_err(crate::error::AppError::internal)?)
    };
    conn.execute(
        "INSERT INTO messages (channel_id, parent_id, user_id, webhook_id, automation_id, bot_name, bot_icon_url, body, attachments, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            new.channel_id,
            new.parent_id,
            new.user_id,
            new.webhook_id,
            new.automation_id,
            new.bot_name,
            new.bot_icon,
            new.body,
            attachments,
            new.created_at
        ],
    )?;
    let id = conn.last_insert_rowid();
    let mut searchable = vec![new.body.to_owned()];
    for (position, file_id) in (0_i64..).zip(new.files) {
        conn.execute(
            "INSERT INTO message_files (message_id, file_id, position) VALUES (?1, ?2, ?3)",
            params![id, file_id, position],
        )?;
        let name: String =
            conn.query_row("SELECT name FROM files WHERE id = ?1", [file_id], |row| {
                row.get(0)
            })?;
        searchable.push(name);
    }
    for attachment in new.attachments {
        searchable.extend(attachment.searchable_text());
    }
    conn.execute(
        "INSERT INTO messages_fts (rowid, content) VALUES (?1, ?2)",
        params![id, searchable.join("\n")],
    )?;
    Ok(id)
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

// Files

pub fn insert_file(
    conn: &Connection,
    name: &str,
    mime: &str,
    data: &[u8],
    uploaded_by: i64,
    now: i64,
) -> AppResult<i64> {
    conn.execute(
        "INSERT INTO files (name, mime, size, data, uploaded_by, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![name, mime, i64::try_from(data.len()).unwrap_or(i64::MAX), data, uploaded_by, now],
    )?;
    Ok(conn.last_insert_rowid())
}

/// A stored file with its bytes.
pub struct StoredFile {
    pub name: String,
    pub mime: String,
    pub data: Vec<u8>,
}

/// Returns the file if `user_id` may see it: they uploaded it, it is a
/// custom emoji, or it is attached to a message in a channel they can read.
pub fn readable_file(
    conn: &Connection,
    file_id: i64,
    user_id: i64,
) -> AppResult<Option<StoredFile>> {
    Ok(conn
        .query_row(
            "SELECT f.name, f.mime, f.data FROM files f
             WHERE f.id = ?1 AND (
                 f.uploaded_by = ?2
                 OR EXISTS (SELECT 1 FROM custom_emoji e WHERE e.file_id = f.id)
                 OR EXISTS (
                     SELECT 1 FROM message_files mf
                     JOIN messages m ON m.id = mf.message_id
                     JOIN channels c ON c.id = m.channel_id
                     WHERE mf.file_id = f.id AND (c.kind = 'public' OR EXISTS (
                         SELECT 1 FROM channel_members cm WHERE cm.channel_id = c.id AND cm.user_id = ?2))))",
            params![file_id, user_id],
            |row| {
                Ok(StoredFile {
                    name: row.get(0)?,
                    mime: row.get(1)?,
                    data: row.get(2)?,
                })
            },
        )
        .optional()?)
}

/// Files the user uploaded that are not yet attached to anything.
pub fn owns_unattached_file(conn: &Connection, file_id: i64, user_id: i64) -> AppResult<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM files f WHERE f.id = ?1 AND f.uploaded_by = ?2
             AND NOT EXISTS (SELECT 1 FROM message_files mf WHERE mf.file_id = f.id)
             AND NOT EXISTS (SELECT 1 FROM custom_emoji e WHERE e.file_id = f.id)",
            params![file_id, user_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

// Custom emoji

pub struct CustomEmoji {
    pub name: String,
    pub file_id: i64,
    pub created_by: Option<i64>,
    pub creator: Option<String>,
}

pub fn custom_emoji(conn: &Connection) -> AppResult<Vec<CustomEmoji>> {
    let mut statement = conn.prepare(
        "SELECT e.name, e.file_id, e.created_by, u.display_name
         FROM custom_emoji e LEFT JOIN users u ON u.id = e.created_by ORDER BY e.name",
    )?;
    let emoji = statement.query_map([], |row| {
        Ok(CustomEmoji {
            name: row.get(0)?,
            file_id: row.get(1)?,
            created_by: row.get(2)?,
            creator: row.get(3)?,
        })
    })?;
    Ok(emoji.collect::<Result<_, _>>()?)
}

pub fn add_custom_emoji(
    conn: &Connection,
    name: &str,
    file_id: i64,
    user_id: i64,
    now: i64,
) -> AppResult<()> {
    conn.execute(
        "INSERT INTO custom_emoji (name, file_id, created_by, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![name, file_id, user_id, now],
    )?;
    Ok(())
}

pub fn delete_custom_emoji(conn: &Connection, name: &str) -> AppResult<()> {
    let file_id: Option<i64> = conn
        .query_row(
            "SELECT file_id FROM custom_emoji WHERE name = ?1",
            [name],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(file_id) = file_id {
        conn.execute("DELETE FROM files WHERE id = ?1", [file_id])?;
    }
    Ok(())
}

/// Everything message rendering needs: custom emoji and usernames.
pub fn render_context(conn: &Connection) -> AppResult<crate::markup::Context> {
    let mut ctx = crate::markup::Context::default();
    for emoji in custom_emoji(conn)? {
        ctx.custom_emoji
            .insert(emoji.name, format!("/files/{}", emoji.file_id));
    }
    let mut statement = conn.prepare("SELECT lower(username) FROM users")?;
    for username in statement.query_map([], |row| row.get::<_, String>(0))? {
        ctx.usernames.insert(username?);
    }
    Ok(ctx)
}

// Reactions

/// Adds the reaction, or removes it if the user already reacted that way.
pub fn toggle_reaction(
    conn: &Connection,
    message_id: i64,
    user_id: i64,
    emoji: &str,
    now: i64,
) -> AppResult<()> {
    let removed = conn.execute(
        "DELETE FROM reactions WHERE message_id = ?1 AND user_id = ?2 AND emoji = ?3",
        params![message_id, user_id, emoji],
    )?;
    if removed == 0 {
        conn.execute(
            "INSERT INTO reactions (message_id, user_id, emoji, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![message_id, user_id, emoji, now],
        )?;
    }
    Ok(())
}

// Search

pub struct SearchHit {
    pub message: Message,
    pub channel: String,
    pub is_direct: bool,
    pub snippet: String,
}

/// Messages matching an FTS5 `query` that `user_id` may read, newest first.
/// The snippet marks matches with U+0001 and U+0002.
pub fn search(
    conn: &Connection,
    user_id: i64,
    query: &str,
    limit: u32,
) -> AppResult<Vec<SearchHit>> {
    let mut statement = conn.prepare(&format!(
        "{MESSAGE_SELECT}, messages_fts f, channels c
         WHERE f.rowid = m.id AND c.id = m.channel_id AND messages_fts MATCH ?1
           AND (c.kind = 'public' OR EXISTS (
               SELECT 1 FROM channel_members cm WHERE cm.channel_id = c.id AND cm.user_id = ?2))
         ORDER BY m.id DESC LIMIT ?3"
    ).replace(
        "(SELECT COUNT(*) FROM messages r WHERE r.parent_id = m.id)",
        "(SELECT COUNT(*) FROM messages r WHERE r.parent_id = m.id),
         snippet(messages_fts, 0, char(1), char(2), '…', 16), c.kind,
         COALESCE(c.name, (SELECT u2.display_name FROM channel_members o JOIN users u2 ON u2.id = o.user_id
                           WHERE o.channel_id = c.id AND o.user_id != ?2), 'yourself')",
    ))?;
    let hits = statement.query_map(params![query, user_id, limit], |row| {
        let kind: String = row.get(12)?;
        Ok(SearchHit {
            message: message_from_row(row)?,
            snippet: row.get(11)?,
            is_direct: kind == "dm",
            channel: row.get(13)?,
        })
    })?;
    Ok(hits.collect::<Result<_, _>>()?)
}

// Settings

pub fn setting(conn: &Connection, key: &str) -> AppResult<Option<String>> {
    Ok(conn
        .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
            row.get(0)
        })
        .optional()?)
}

pub fn set_setting(conn: &Connection, key: &str, value: &str) -> AppResult<()> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

// Push subscriptions

#[derive(Debug, Clone)]
pub struct PushSubscription {
    pub endpoint: String,
    pub user_id: i64,
    pub p256dh: String,
    pub auth: String,
}

pub fn save_push_subscription(
    conn: &Connection,
    subscription: &PushSubscription,
    now: i64,
) -> AppResult<()> {
    conn.execute(
        "INSERT INTO push_subscriptions (endpoint, user_id, p256dh, auth, created_at) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (endpoint) DO UPDATE SET user_id = excluded.user_id, p256dh = excluded.p256dh, auth = excluded.auth",
        params![subscription.endpoint, subscription.user_id, subscription.p256dh, subscription.auth, now],
    )?;
    Ok(())
}

pub fn delete_push_subscription(
    conn: &Connection,
    endpoint: &str,
    user_id: Option<i64>,
) -> AppResult<()> {
    conn.execute(
        "DELETE FROM push_subscriptions WHERE endpoint = ?1 AND (?2 IS NULL OR user_id = ?2)",
        params![endpoint, user_id],
    )?;
    Ok(())
}

pub fn push_subscriptions(conn: &Connection, user_ids: &[i64]) -> AppResult<Vec<PushSubscription>> {
    let ids = serde_json::to_string(user_ids).map_err(crate::error::AppError::internal)?;
    let mut statement = conn.prepare(
        "SELECT endpoint, user_id, p256dh, auth FROM push_subscriptions
         WHERE user_id IN (SELECT value FROM json_each(?1))",
    )?;
    let subscriptions = statement.query_map([ids], |row| {
        Ok(PushSubscription {
            endpoint: row.get(0)?,
            user_id: row.get(1)?,
            p256dh: row.get(2)?,
            auth: row.get(3)?,
        })
    })?;
    Ok(subscriptions.collect::<Result<_, _>>()?)
}

/// Who should be notified about a new message: the other members of a
/// direct conversation, people in the thread, and anyone `@mentioned`.
pub fn notification_targets(
    conn: &Connection,
    message: &Message,
    sender: Option<i64>,
) -> AppResult<Vec<i64>> {
    let members = audience(conn, message.channel_id)?;
    let public = members.is_none();
    let mut targets: Vec<i64> = members.unwrap_or_default();
    if let Some(parent) = message.parent_id {
        let mut statement = conn.prepare(
            "SELECT user_id FROM messages WHERE (id = ?1 OR parent_id = ?1) AND user_id IS NOT NULL",
        )?;
        for user in statement.query_map([parent], |row| row.get::<_, i64>(0))? {
            targets.push(user?);
        }
    }
    let text = message.body.to_lowercase();
    let everyone = public && (mentions(&text, "channel") || mentions(&text, "here"));
    let mut statement = conn.prepare("SELECT id, lower(username) FROM users")?;
    for user in statement.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })? {
        let (id, username) = user?;
        if everyone || mentions(&text, &username) {
            targets.push(id);
        }
    }
    targets.sort_unstable();
    targets.dedup();
    targets.retain(|id| Some(*id) != sender);
    Ok(targets)
}

/// Whether lowercase `text` contains `@name` as a whole word.
fn mentions(text: &str, name: &str) -> bool {
    let needle = format!("@{name}");
    text.match_indices(&needle).any(|(index, _)| {
        let before = text.get(..index).and_then(|t| t.chars().last());
        let after = text
            .get(index.saturating_add(needle.len())..)
            .and_then(|t| t.chars().next());
        let word = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.');
        !before.is_some_and(word) && !after.is_some_and(|c| word(c) && c != '.')
    })
}

// Automations

#[derive(Debug, Clone)]
pub struct Automation {
    pub id: i64,
    pub name: String,
    pub source: String,
    pub enabled: bool,
    pub last_error: Option<String>,
    pub updated_at: i64,
}

fn automation_from_row(row: &Row<'_>) -> rusqlite::Result<Automation> {
    Ok(Automation {
        id: row.get(0)?,
        name: row.get(1)?,
        source: row.get(2)?,
        enabled: row.get(3)?,
        last_error: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

pub fn automations(conn: &Connection) -> AppResult<Vec<Automation>> {
    let mut statement = conn.prepare(
        "SELECT id, name, source, enabled, last_error, updated_at FROM automations ORDER BY name COLLATE NOCASE",
    )?;
    let automations = statement.query_map([], automation_from_row)?;
    Ok(automations.collect::<Result<_, _>>()?)
}

pub fn automation(conn: &Connection, id: i64) -> AppResult<Option<Automation>> {
    Ok(conn
        .query_row(
            "SELECT id, name, source, enabled, last_error, updated_at FROM automations WHERE id = ?1",
            [id],
            automation_from_row,
        )
        .optional()?)
}

pub fn save_automation(
    conn: &Connection,
    id: Option<i64>,
    name: &str,
    source: &str,
    enabled: bool,
    user_id: i64,
    now: i64,
) -> AppResult<i64> {
    if let Some(id) = id {
        conn.execute(
            "UPDATE automations SET name = ?1, source = ?2, enabled = ?3, last_error = NULL, updated_at = ?4 WHERE id = ?5",
            params![name, source, enabled, now, id],
        )?;
        Ok(id)
    } else {
        conn.execute(
            "INSERT INTO automations (name, source, enabled, created_by, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
            params![name, source, enabled, user_id, now],
        )?;
        Ok(conn.last_insert_rowid())
    }
}

pub fn delete_automation(conn: &Connection, id: i64) -> AppResult<()> {
    conn.execute("DELETE FROM automations WHERE id = ?1", [id])?;
    Ok(())
}

pub fn set_automation_error(conn: &Connection, id: i64, error: Option<&str>) -> AppResult<()> {
    conn.execute(
        "UPDATE automations SET last_error = ?1 WHERE id = ?2",
        params![error, id],
    )?;
    Ok(())
}

pub fn automation_value(conn: &Connection, id: i64, key: &str) -> AppResult<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT value FROM automation_data WHERE automation_id = ?1 AND key = ?2",
            params![id, key],
            |row| row.get(0),
        )
        .optional()?)
}

pub fn set_automation_value(
    conn: &Connection,
    id: i64,
    key: &str,
    value: Option<&str>,
) -> AppResult<()> {
    match value {
        Some(value) => conn.execute(
            "INSERT INTO automation_data (automation_id, key, value) VALUES (?1, ?2, ?3)
             ON CONFLICT (automation_id, key) DO UPDATE SET value = excluded.value",
            params![id, key, value],
        )?,
        None => conn.execute(
            "DELETE FROM automation_data WHERE automation_id = ?1 AND key = ?2",
            params![id, key],
        )?,
    };
    Ok(())
}
