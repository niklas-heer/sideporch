//! Queries over the `SQLite` database. Functions here are synchronous and
//! run inside [`crate::db::Db::call`].

use std::{collections::HashMap, fmt::Write as _};

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
    pub avatar_file_id: Option<i64>,
    pub status_emoji: String,
    pub status_text: String,
    pub bio: String,
    pub links: Vec<ProfileLink>,
    /// Shortcode names, in the order the person chose.
    pub favorite_emoji: Vec<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ProfileLink {
    pub label: String,
    pub url: String,
}

const USER_COLUMNS: &str = "id, username, display_name, is_admin, avatar_file_id, status_emoji, \
     status_text, bio, links, favorite_emoji, created_at";

fn user_from_row(row: &Row<'_>) -> rusqlite::Result<User> {
    let links: String = row.get(8)?;
    let favorites: String = row.get(9)?;
    Ok(User {
        id: row.get(0)?,
        username: row.get(1)?,
        display_name: row.get(2)?,
        is_admin: row.get(3)?,
        avatar_file_id: row.get(4)?,
        status_emoji: row.get(5)?,
        status_text: row.get(6)?,
        bio: row.get(7)?,
        links: serde_json::from_str(&links).unwrap_or_default(),
        favorite_emoji: favorites
            .split_whitespace()
            .map(ToOwned::to_owned)
            .collect(),
        created_at: row.get(10)?,
    })
}

pub fn user(conn: &Connection, id: i64) -> AppResult<Option<User>> {
    Ok(conn
        .query_row(
            &format!("SELECT {USER_COLUMNS} FROM users WHERE id = ?1"),
            [id],
            user_from_row,
        )
        .optional()?)
}

/// What someone can change about their profile.
pub struct ProfileEdit {
    pub display_name: String,
    pub status_emoji: String,
    pub status_text: String,
    pub bio: String,
    pub links: Vec<ProfileLink>,
    pub favorite_emoji: Vec<String>,
}

pub fn update_profile(conn: &Connection, id: i64, edit: &ProfileEdit) -> AppResult<()> {
    let links = serde_json::to_string(&edit.links).map_err(crate::error::AppError::internal)?;
    conn.execute(
        "UPDATE users SET display_name = ?1, status_emoji = ?2, status_text = ?3, bio = ?4,
             links = ?5, favorite_emoji = ?6 WHERE id = ?7",
        params![
            edit.display_name,
            edit.status_emoji,
            edit.status_text,
            edit.bio,
            links,
            edit.favorite_emoji.join(" "),
            id
        ],
    )?;
    Ok(())
}

pub fn set_avatar(conn: &Connection, id: i64, file_id: Option<i64>) -> AppResult<()> {
    conn.execute(
        "UPDATE users SET avatar_file_id = ?1 WHERE id = ?2",
        params![file_id, id],
    )?;
    Ok(())
}

/// The emoji the picker shows first for someone: their favorites, or else
/// the ones they use most, filled up with popular defaults.
pub fn picker_emoji(conn: &Connection, user_id: i64) -> AppResult<Vec<String>> {
    const SHOWN: usize = 12;
    let favorites: String = conn
        .query_row(
            "SELECT favorite_emoji FROM users WHERE id = ?1",
            [user_id],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or_default();
    let mut names: Vec<String> = favorites
        .split_whitespace()
        .map(ToOwned::to_owned)
        .collect();
    if names.is_empty() {
        names = most_used_emoji(conn, user_id, 12)?;
        for (name, _) in crate::markup::BUILTIN_EMOJI {
            if names.len() >= SHOWN {
                break;
            }
            if !names.iter().any(|known| known == name) {
                names.push((*name).to_owned());
            }
        }
    }
    Ok(names)
}

/// The emoji someone reacted with most, most used first.
pub fn most_used_emoji(conn: &Connection, user_id: i64, limit: u32) -> AppResult<Vec<String>> {
    let mut statement = conn.prepare(
        "SELECT emoji FROM reactions WHERE user_id = ?1
         GROUP BY emoji ORDER BY COUNT(*) DESC, MAX(created_at) DESC LIMIT ?2",
    )?;
    let names = statement.query_map(params![user_id, limit], |row| row.get(0))?;
    Ok(names.collect::<Result<_, _>>()?)
}

#[derive(Debug, Clone)]
pub enum Author {
    User {
        id: i64,
        display_name: String,
        avatar: Option<i64>,
        status_emoji: String,
    },
    Bot {
        name: String,
        icon: Option<String>,
    },
    Removed,
}

#[derive(Debug, Clone)]
pub struct Message {
    pub id: i64,
    pub channel_id: i64,
    pub parent_id: Option<i64>,
    pub author: Author,
    pub body: String,
    /// Written in Slack's mrkdwn by a webhook; everything else is Markdown.
    pub slack_format: bool,
    /// A GIF from a GIF service, shown from the service's own URL.
    pub gif: Option<Gif>,
    pub attachments: Vec<Attachment>,
    pub created_at: i64,
    pub reply_count: i64,
    pub files: Vec<FileRef>,
    pub reactions: Vec<Reaction>,
}

/// A GIF from a GIF service. Its media stays at the service's URLs, as the
/// service's terms require.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct Gif {
    pub provider: String,
    pub id: String,
    pub title: String,
    pub url: String,
    pub width: u32,
    pub height: u32,
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
    /// Names of automations that reacted.
    pub bots: Vec<String>,
}

impl Reaction {
    pub const fn count(&self) -> usize {
        self.user_ids.len().saturating_add(self.bots.len())
    }
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
    pub gif: Option<&'a Gif>,
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
    let mut statement = conn.prepare(&format!(
        "SELECT {USER_COLUMNS} FROM users ORDER BY display_name COLLATE NOCASE"
    ))?;
    let rows = statement.query_map([], user_from_row)?;
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

/// `channel_id` if it is a public channel.
pub fn public_channel(conn: &Connection, channel_id: i64) -> AppResult<Option<i64>> {
    Ok(conn
        .query_row(
            "SELECT id FROM channels WHERE kind = 'public' AND id = ?1",
            [channel_id],
            |row| row.get(0),
        )
        .optional()?)
}

/// Names of all public channels, alphabetically.
pub fn public_channel_names(conn: &Connection) -> AppResult<Vec<String>> {
    let mut statement =
        conn.prepare("SELECT name FROM channels WHERE kind = 'public' ORDER BY name")?;
    let names = statement.query_map([], |row| row.get(0))?;
    Ok(names.collect::<Result<_, _>>()?)
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
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
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
        m.bot_name, m.bot_icon_url, m.body, m.attachments, m.created_at, m.webhook_id IS NOT NULL,
        u.avatar_file_id, COALESCE(u.status_emoji, ''), m.gif,
        (SELECT COUNT(*) FROM messages r WHERE r.parent_id = m.id)
    FROM messages m LEFT JOIN users u ON u.id = m.user_id";

fn message_from_row(row: &Row<'_>) -> rusqlite::Result<Message> {
    let user_id: Option<i64> = row.get(3)?;
    let display_name: Option<String> = row.get(4)?;
    let bot_name: Option<String> = row.get(5)?;
    let author = match (user_id, display_name, bot_name) {
        (Some(id), Some(display_name), _) => Author::User {
            id,
            display_name,
            avatar: row.get(11)?,
            status_emoji: row.get(12)?,
        },
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
        slack_format: row.get(10)?,
        gif: row
            .get::<_, Option<String>>(13)?
            .and_then(|json| serde_json::from_str(&json).ok()),
        reply_count: row.get(14)?,
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
                bots: Vec::new(),
            });
        }
    }
    let mut statement = conn.prepare(
        "SELECT r.message_id, r.emoji, a.name
         FROM automation_reactions r JOIN automations a ON a.id = r.automation_id
         WHERE r.message_id IN (SELECT value FROM json_each(?1))
         ORDER BY r.message_id, r.created_at, r.automation_id",
    )?;
    let rows = statement.query_map([&ids], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    for row in rows {
        let (message_id, emoji, name) = row?;
        let Some(message) = messages.iter_mut().find(|m| m.id == message_id) else {
            continue;
        };
        if let Some(reaction) = message.reactions.iter_mut().find(|r| r.emoji == emoji) {
            reaction.bots.push(name);
        } else {
            message.reactions.push(Reaction {
                emoji,
                user_ids: Vec::new(),
                names: Vec::new(),
                bots: vec![name],
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
        "INSERT INTO messages (channel_id, parent_id, user_id, webhook_id, automation_id, bot_name, bot_icon_url, body, attachments, created_at, gif)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
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
            new.created_at,
            new.gif.and_then(|gif| serde_json::to_string(gif).ok())
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
    if let Some(gif) = new.gif {
        searchable.push(gif.title.clone());
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

/// A file's metadata, for a file already written with [`crate::blobs`].
pub struct NewFile<'a> {
    pub name: &'a str,
    pub mime: &'a str,
    pub sha256: &'a str,
    pub size: usize,
}

pub fn insert_file(
    conn: &Connection,
    file: &NewFile<'_>,
    uploaded_by: i64,
    now: i64,
) -> AppResult<i64> {
    conn.execute(
        "INSERT INTO files (name, mime, size, data, sha256, uploaded_by, created_at)
         VALUES (?1, ?2, ?3, X'', ?4, ?5, ?6)",
        params![
            file.name,
            file.mime,
            i64::try_from(file.size).unwrap_or(i64::MAX),
            file.sha256,
            uploaded_by,
            now
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// A stored file. Its bytes are on disk under `sha256`.
pub struct StoredFile {
    pub name: String,
    pub mime: String,
    pub sha256: String,
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
            "SELECT f.name, f.mime, f.sha256 FROM files f
             WHERE f.id = ?1 AND f.sha256 IS NOT NULL AND (
                 f.uploaded_by = ?2
                 OR EXISTS (SELECT 1 FROM custom_emoji e WHERE e.file_id = f.id)
                 OR EXISTS (SELECT 1 FROM users u WHERE u.avatar_file_id = f.id)
                 OR EXISTS (SELECT 1 FROM gif_library g WHERE g.file_id = f.id)
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
                    sha256: row.get(2)?,
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
             AND NOT EXISTS (SELECT 1 FROM custom_emoji e WHERE e.file_id = f.id)
             AND NOT EXISTS (SELECT 1 FROM gif_library g WHERE g.file_id = f.id)",
            params![file_id, user_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

// The team's GIF library

#[derive(Debug, Clone)]
pub struct LibraryGif {
    pub id: i64,
    pub file_id: i64,
    pub title: String,
    pub tags: String,
    pub width: u32,
    pub height: u32,
    pub added_by: Option<i64>,
    pub adder: Option<String>,
    pub uses: i64,
}

const LIBRARY_COLUMNS: &str = "g.id, g.file_id, g.title, g.tags, g.width, g.height, g.added_by, \
     u.display_name, g.uses FROM gif_library g LEFT JOIN users u ON u.id = g.added_by";

fn library_gif_from_row(row: &Row<'_>) -> rusqlite::Result<LibraryGif> {
    Ok(LibraryGif {
        id: row.get(0)?,
        file_id: row.get(1)?,
        title: row.get(2)?,
        tags: row.get(3)?,
        width: row.get(4)?,
        height: row.get(5)?,
        added_by: row.get(6)?,
        adder: row.get(7)?,
        uses: row.get(8)?,
    })
}

/// Library GIFs whose title or tags contain every word of `query`, most
/// used first. An empty query lists everything.
pub fn library_gifs(conn: &Connection, query: &str, limit: u32) -> AppResult<Vec<LibraryGif>> {
    let words: Vec<String> = query
        .split_whitespace()
        .take(5)
        .map(|word| {
            let escaped = word
                .to_lowercase()
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_");
            format!("%{escaped}%")
        })
        .collect();
    let mut sql = format!("SELECT {LIBRARY_COLUMNS} WHERE 1 = 1");
    for index in 1..=words.len() {
        // Writing to a String cannot fail.
        let _ = write!(
            sql,
            " AND lower(g.title || ' ' || g.tags) LIKE ?{index} ESCAPE '\\'"
        );
    }
    let _ = write!(sql, " ORDER BY g.uses DESC, g.id DESC LIMIT {limit}");
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map(
        rusqlite::params_from_iter(words.iter()),
        library_gif_from_row,
    )?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn library_gif(conn: &Connection, id: i64) -> AppResult<Option<LibraryGif>> {
    Ok(conn
        .query_row(
            &format!("SELECT {LIBRARY_COLUMNS} WHERE g.id = ?1"),
            [id],
            library_gif_from_row,
        )
        .optional()?)
}

pub struct NewLibraryGif<'a> {
    pub file_id: i64,
    pub title: &'a str,
    pub tags: &'a str,
    pub width: u32,
    pub height: u32,
}

pub fn add_library_gif(
    conn: &Connection,
    gif: &NewLibraryGif<'_>,
    user_id: i64,
    now: i64,
) -> AppResult<i64> {
    conn.execute(
        "INSERT INTO gif_library (file_id, title, tags, width, height, added_by, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            gif.file_id,
            gif.title,
            gif.tags,
            gif.width,
            gif.height,
            user_id,
            now
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Removes a library GIF. Its file stays for messages that show it.
/// Removes a GIF and its file, so messages that showed it do not anymore.
pub fn delete_library_gif(conn: &Connection, id: i64) -> AppResult<()> {
    conn.execute(
        "DELETE FROM files WHERE id = (SELECT file_id FROM gif_library WHERE id = ?1)",
        [id],
    )?;
    Ok(())
}

pub fn count_gif_use(conn: &Connection, id: i64) -> AppResult<()> {
    conn.execute("UPDATE gif_library SET uses = uses + 1 WHERE id = ?1", [id])?;
    Ok(())
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
/// Returns whether it was added.
pub fn toggle_reaction(
    conn: &Connection,
    message_id: i64,
    user_id: i64,
    emoji: &str,
    now: i64,
) -> AppResult<bool> {
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
    Ok(removed == 0)
}

/// Adds an automation's reaction. Returns false if it already reacted so.
pub fn add_automation_reaction(
    conn: &Connection,
    message_id: i64,
    automation_id: i64,
    emoji: &str,
    now: i64,
) -> AppResult<bool> {
    let added = conn.execute(
        "INSERT OR IGNORE INTO automation_reactions (message_id, automation_id, emoji, created_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![message_id, automation_id, emoji, now],
    )?;
    Ok(added > 0)
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
        let kind: String = row.get(16)?;
        Ok(SearchHit {
            message: message_from_row(row)?,
            snippet: row.get(15)?,
            is_direct: kind == "dm",
            channel: row.get(17)?,
        })
    })?;
    Ok(hits.collect::<Result<_, _>>()?)
}

/// How much Sideporch holds, for the system page.
#[derive(Debug, Clone, Default)]
pub struct Counts {
    pub users: i64,
    pub channels: i64,
    pub messages: i64,
    pub files: i64,
    pub reactions: i64,
    pub automations: i64,
    pub push_subscriptions: i64,
}

pub fn counts(conn: &Connection) -> AppResult<Counts> {
    let count = |table: &str| -> AppResult<i64> {
        Ok(
            conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })?,
        )
    };
    Ok(Counts {
        users: count("users")?,
        channels: count("channels")?,
        messages: count("messages")?,
        files: count("files")?,
        reactions: count("reactions")?,
        automations: count("automations")?,
        push_subscriptions: count("push_subscriptions")?,
    })
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

pub fn delete_setting(conn: &Connection, key: &str) -> AppResult<()> {
    conn.execute("DELETE FROM settings WHERE key = ?1", [key])?;
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
    /// The secret part of the automation's webhook URL.
    pub hook_token: String,
    /// `automation`, or `library` for code other scripts `require`.
    pub kind: String,
}

const AUTOMATION_COLUMNS: &str =
    "id, name, source, enabled, last_error, updated_at, COALESCE(hook_token, ''), kind";

fn automation_from_row(row: &Row<'_>) -> rusqlite::Result<Automation> {
    Ok(Automation {
        id: row.get(0)?,
        name: row.get(1)?,
        source: row.get(2)?,
        enabled: row.get(3)?,
        last_error: row.get(4)?,
        updated_at: row.get(5)?,
        hook_token: row.get(6)?,
        kind: row.get(7)?,
    })
}

pub fn automations(conn: &Connection) -> AppResult<Vec<Automation>> {
    let mut statement = conn.prepare(&format!(
        "SELECT {AUTOMATION_COLUMNS} FROM automations ORDER BY name COLLATE NOCASE"
    ))?;
    let automations = statement.query_map([], automation_from_row)?;
    Ok(automations.collect::<Result<_, _>>()?)
}

pub fn automation(conn: &Connection, id: i64) -> AppResult<Option<Automation>> {
    Ok(conn
        .query_row(
            &format!("SELECT {AUTOMATION_COLUMNS} FROM automations WHERE id = ?1"),
            [id],
            automation_from_row,
        )
        .optional()?)
}

/// The automation whose webhook URL carries `token`.
pub fn automation_by_hook_token(conn: &Connection, token: &str) -> AppResult<Option<i64>> {
    Ok(conn
        .query_row(
            "SELECT id FROM automations WHERE hook_token = ?1",
            [token],
            |row| row.get(0),
        )
        .optional()?)
}

pub fn new_hook_token(conn: &Connection, id: i64) -> AppResult<()> {
    conn.execute(
        "UPDATE automations SET hook_token = lower(hex(randomblob(20))) WHERE id = ?1",
        [id],
    )?;
    Ok(())
}

/// What to save, and who saved it with which tool.
pub struct AutomationEdit<'a> {
    pub name: &'a str,
    pub source: &'a str,
    pub enabled: bool,
    pub user_id: i64,
    /// `editor`, `AI`, `restore`, or `MCP: <token name>`.
    pub saved_with: &'a str,
    /// Only used when creating: `automation` or `library`.
    pub kind: &'a str,
}

/// Versions kept per automation.
const KEPT_VERSIONS: i64 = 50;

/// Creates or updates an automation and records a version when the source
/// changed. Returns its id.
pub fn save_automation(
    conn: &mut Connection,
    id: Option<i64>,
    edit: &AutomationEdit<'_>,
    now: i64,
) -> AppResult<i64> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let id = if let Some(id) = id {
        tx.execute(
            "UPDATE automations SET name = ?1, source = ?2, enabled = ?3, last_error = NULL, updated_at = ?4 WHERE id = ?5",
            params![edit.name, edit.source, edit.enabled, now, id],
        )?;
        id
    } else {
        tx.execute(
            "INSERT INTO automations (name, source, enabled, created_by, created_at, updated_at, hook_token, kind)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5, lower(hex(randomblob(20))), ?6)",
            params![edit.name, edit.source, edit.enabled, edit.user_id, now, edit.kind],
        )?;
        tx.last_insert_rowid()
    };
    let latest: Option<String> = tx
        .query_row(
            "SELECT source FROM automation_versions WHERE automation_id = ?1 ORDER BY id DESC LIMIT 1",
            [id],
            |row| row.get(0),
        )
        .optional()?;
    if latest.as_deref() != Some(edit.source) {
        tx.execute(
            "INSERT INTO automation_versions (automation_id, source, saved_by, saved_with, saved_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, edit.source, edit.user_id, edit.saved_with, now],
        )?;
        tx.execute(
            "DELETE FROM automation_versions WHERE automation_id = ?1 AND id NOT IN
             (SELECT id FROM automation_versions WHERE automation_id = ?1 ORDER BY id DESC LIMIT ?2)",
            params![id, KEPT_VERSIONS],
        )?;
    }
    tx.commit()?;
    Ok(id)
}

/// Whether another library than `except` is called `name`.
pub fn library_name_taken(conn: &Connection, name: &str, except: Option<i64>) -> AppResult<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM automations WHERE kind = 'library' AND name = ?1 AND id IS NOT ?2",
            params![name, except],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

pub fn delete_automation(conn: &Connection, id: i64) -> AppResult<()> {
    conn.execute("DELETE FROM automations WHERE id = ?1", [id])?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct AutomationVersion {
    pub id: i64,
    pub source: String,
    pub saved_by: Option<String>,
    pub saved_with: String,
    pub saved_at: i64,
}

/// Saved versions, newest first.
pub fn automation_versions(conn: &Connection, id: i64) -> AppResult<Vec<AutomationVersion>> {
    let mut statement = conn.prepare(
        "SELECT v.id, v.source, u.display_name, v.saved_with, v.saved_at
         FROM automation_versions v LEFT JOIN users u ON u.id = v.saved_by
         WHERE v.automation_id = ?1 ORDER BY v.id DESC",
    )?;
    let versions = statement.query_map([id], |row| {
        Ok(AutomationVersion {
            id: row.get(0)?,
            source: row.get(1)?,
            saved_by: row.get(2)?,
            saved_with: row.get(3)?,
            saved_at: row.get(4)?,
        })
    })?;
    Ok(versions.collect::<Result<_, _>>()?)
}

#[derive(Debug, Clone)]
pub struct AutomationRun {
    pub trigger: String,
    pub started_at: i64,
    pub duration_us: i64,
    pub output: String,
    pub error: Option<String>,
}

/// Runs kept per automation.
const KEPT_RUNS: i64 = 100;

pub fn record_automation_run(
    conn: &Connection,
    id: i64,
    trigger: &str,
    started_at: i64,
    duration_us: i64,
    output: &str,
    error: Option<&str>,
) -> AppResult<()> {
    conn.execute(
        "INSERT INTO automation_runs (automation_id, trigger, started_at, duration_us, output, error)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, trigger, started_at, duration_us, output, error],
    )?;
    conn.execute(
        "DELETE FROM automation_runs WHERE automation_id = ?1 AND id NOT IN
         (SELECT id FROM automation_runs WHERE automation_id = ?1 ORDER BY id DESC LIMIT ?2)",
        params![id, KEPT_RUNS],
    )?;
    Ok(())
}

/// The newest `limit` runs, newest first.
pub fn automation_runs(conn: &Connection, id: i64, limit: i64) -> AppResult<Vec<AutomationRun>> {
    let mut statement = conn.prepare(
        "SELECT trigger, started_at, duration_us, output, error FROM automation_runs
         WHERE automation_id = ?1 ORDER BY id DESC LIMIT ?2",
    )?;
    let runs = statement.query_map(params![id, limit], |row| {
        Ok(AutomationRun {
            trigger: row.get(0)?,
            started_at: row.get(1)?,
            duration_us: row.get(2)?,
            output: row.get(3)?,
            error: row.get(4)?,
        })
    })?;
    Ok(runs.collect::<Result<_, _>>()?)
}

/// All saved data of an automation, for dry runs.
pub fn automation_values(conn: &Connection, id: i64) -> AppResult<HashMap<String, String>> {
    let mut statement = conn
        .prepare("SELECT key, value FROM automation_data WHERE automation_id = ?1 LIMIT 10000")?;
    let values = statement.query_map([id], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(values.collect::<Result<_, _>>()?)
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

// API tokens for the MCP endpoint

#[derive(Debug, Clone)]
pub struct ApiToken {
    pub id: i64,
    pub name: String,
    pub owner: String,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
}

pub fn create_api_token(
    conn: &Connection,
    user_id: i64,
    name: &str,
    token_hash: &[u8],
    now: i64,
) -> AppResult<()> {
    conn.execute(
        "INSERT INTO api_tokens (user_id, name, token_hash, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![user_id, name, token_hash, now],
    )?;
    Ok(())
}

pub fn api_tokens(conn: &Connection) -> AppResult<Vec<ApiToken>> {
    let mut statement = conn.prepare(
        "SELECT t.id, t.name, u.display_name, t.created_at, t.last_used_at
         FROM api_tokens t JOIN users u ON u.id = t.user_id ORDER BY t.id DESC",
    )?;
    let tokens = statement.query_map([], |row| {
        Ok(ApiToken {
            id: row.get(0)?,
            name: row.get(1)?,
            owner: row.get(2)?,
            created_at: row.get(3)?,
            last_used_at: row.get(4)?,
        })
    })?;
    Ok(tokens.collect::<Result<_, _>>()?)
}

pub fn delete_api_token(conn: &Connection, id: i64) -> AppResult<()> {
    conn.execute("DELETE FROM api_tokens WHERE id = ?1", [id])?;
    Ok(())
}

/// The admin a token belongs to, as (user id, token name), recording the
/// use. Tokens of people who are no longer admins stop working.
pub fn use_api_token(
    conn: &Connection,
    token_hash: &[u8],
    now: i64,
) -> AppResult<Option<(i64, String)>> {
    let found: Option<(i64, i64, String)> = conn
        .query_row(
            "SELECT t.id, u.id, t.name FROM api_tokens t JOIN users u ON u.id = t.user_id
             WHERE t.token_hash = ?1 AND u.is_admin = 1",
            [token_hash],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((token_id, user_id, name)) = found else {
        return Ok(None);
    };
    conn.execute(
        "UPDATE api_tokens SET last_used_at = ?1 WHERE id = ?2",
        params![now, token_id],
    )?;
    Ok(Some((user_id, name)))
}
