//! Queries over the `SQLite` database. Functions here are synchronous and
//! run inside [`crate::db::Db::call`].

use std::{collections::HashMap, fmt::Write as _};

use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::{error::AppResult, webhook::Attachment};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelKind {
    Public,
    /// A named channel only its members see.
    Private,
    Direct,
}

#[derive(Debug, Clone)]
pub struct Channel {
    pub id: i64,
    pub kind: ChannelKind,
    pub name: String,
    pub topic: String,
    /// The reader left this public channel, so it is not in their sidebar.
    pub left: bool,
    /// The reader muted it.
    pub muted: bool,
    pub created_by: Option<i64>,
    /// Who may start threads, reply and react.
    pub posting: Posting,
    /// Whether the reader manages it: they are listed as a manager, or an
    /// admin.
    pub manager: bool,
}

/// Who may do something in a channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Policy {
    #[default]
    Everyone,
    Managers,
}

impl Policy {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "everyone" => Some(Self::Everyone),
            "managers" => Some(Self::Managers),
            _ => None,
        }
    }

    pub const fn key(self) -> &'static str {
        match self {
            Self::Everyone => "everyone",
            Self::Managers => "managers",
        }
    }
}

/// A channel's rules for new posts, thread replies and reactions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Posting {
    pub post: Policy,
    pub reply: Policy,
    pub react: Policy,
}

impl Channel {
    const fn allows(&self, policy: Policy) -> bool {
        matches!(policy, Policy::Everyone) || self.manager
    }

    /// Whether the reader may write here: a new post, or a thread reply.
    pub const fn may_write(&self, reply: bool) -> bool {
        self.allows(if reply {
            self.posting.reply
        } else {
            self.posting.post
        })
    }

    pub const fn may_react(&self) -> bool {
        self.allows(self.posting.react)
    }

    /// Why the reader can't write, for error messages.
    pub fn write_refusal(&self, reply: bool) -> String {
        if reply {
            format!(
                "Only the managers of #{} reply in threads there.",
                self.name
            )
        } else if self.posting.reply == Policy::Everyone {
            format!(
                "Only the managers of #{} start new posts. Reply in a thread instead.",
                self.name
            )
        } else {
            format!("Only the managers of #{} post there.", self.name)
        }
    }
}

#[derive(Debug, Clone)]
pub struct SidebarItem {
    pub channel_id: i64,
    pub label: String,
    pub unread: bool,
    pub muted: bool,
    pub look: SidebarLook,
}

/// Which icon a sidebar entry gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarLook {
    Channel,
    Private,
    /// Only managers start new posts.
    Announcement,
    Direct,
}

#[derive(Debug, Clone)]
pub struct Sidebar {
    pub channels: Vec<SidebarItem>,
    pub direct: Vec<SidebarItem>,
    /// Whether there are mentions or replies the person hasn't seen.
    pub activity: bool,
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
    /// Deactivated people can't sign in and get no notifications.
    pub deactivated: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ProfileLink {
    pub label: String,
    pub url: String,
}

const USER_COLUMNS: &str = "id, username, display_name, is_admin, avatar_file_id, status_emoji, \
     status_text, bio, links, favorite_emoji, created_at, deactivated_at IS NOT NULL";

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
        deactivated: row.get(11)?,
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

/// Someone's theme and appearance; empty values mean the team's default.
pub fn set_user_appearance(
    conn: &Connection,
    id: i64,
    theme: &str,
    appearance: &str,
) -> AppResult<()> {
    conn.execute(
        "UPDATE users SET theme = ?1, appearance = ?2 WHERE id = ?3",
        params![theme, appearance, id],
    )?;
    Ok(())
}

/// Someone's own theme and appearance, empty when they use the default.
pub fn user_appearance(conn: &Connection, id: i64) -> AppResult<(String, String)> {
    Ok(conn.query_row(
        "SELECT theme, appearance FROM users WHERE id = ?1",
        [id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?)
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
    pub edited_at: Option<i64>,
    /// Set when a message with replies was deleted; its thread stays.
    pub deleted: bool,
    /// Who pinned it, if it is pinned.
    pub pinned_by: Option<String>,
    /// What its first link shows.
    pub preview: Option<LinkPreview>,
    pub poll: Option<Poll>,
    /// Buttons an automation put under its message.
    pub buttons: Vec<Button>,
    /// The automation that posted it.
    pub automation_id: Option<i64>,
}

/// A poll: the message text is its question.
#[derive(Debug, Clone, Default)]
pub struct Poll {
    pub kind: crate::polls::Kind,
    pub options: Vec<PollOption>,
    /// Everyone's ranking, in ranked polls.
    pub ballots: Vec<Ballot>,
    pub closed: bool,
}

/// An option and who picked it. In ranked polls, who ranked it first.
#[derive(Debug, Clone, Default)]
pub struct PollOption {
    pub label: String,
    pub voters: Vec<i64>,
    pub names: Vec<String>,
}

/// One person's ranking: option indexes, favorite first.
#[derive(Debug, Clone, Default)]
pub struct Ballot {
    pub user_id: i64,
    pub name: String,
    pub ranking: Vec<usize>,
}

impl Poll {
    /// How many people voted.
    pub fn voters(&self) -> usize {
        match self.kind {
            crate::polls::Kind::Ranked => self.ballots.len(),
            crate::polls::Kind::Single => {
                self.options.iter().map(|option| option.voters.len()).sum()
            }
            crate::polls::Kind::Multiple => {
                let mut everyone: Vec<i64> = self
                    .options
                    .iter()
                    .flat_map(|option| option.voters.iter().copied())
                    .collect();
                everyone.sort_unstable();
                everyone.dedup();
                everyone.len()
            }
        }
    }

    /// Counts a ranked poll.
    pub fn outcome(&self) -> crate::polls::Outcome {
        let ballots: Vec<Vec<usize>> = self
            .ballots
            .iter()
            .map(|ballot| ballot.ranking.clone())
            .collect();
        crate::polls::instant_runoff(self.options.len(), &ballots)
    }

    pub fn ballot_of(&self, user_id: i64) -> Option<&Ballot> {
        self.ballots.iter().find(|ballot| ballot.user_id == user_id)
    }
}

/// A button under an automation's message. Clicking it tells the
/// automation, with `value`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct Button {
    pub label: String,
    pub value: String,
    /// `primary`, `danger`, or empty.
    #[serde(default)]
    pub style: String,
}

/// The title, description and image of a linked page.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct LinkPreview {
    pub url: String,
    pub title: String,
    pub description: Option<String>,
    pub image: Option<String>,
    pub site: Option<String>,
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
    /// A poll; the body is its question.
    pub poll: Option<&'a crate::polls::Spec>,
    pub buttons: &'a [Button],
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
            "SELECT id, password_hash FROM users WHERE username = ?1 AND deactivated_at IS NULL",
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
    conn.execute("DELETE FROM password_resets WHERE expires_at <= ?1", [now])?;
    Ok(conn.execute("DELETE FROM sessions WHERE expires_at <= ?1", [now])?)
}

pub fn password_hash(conn: &Connection, user_id: i64) -> AppResult<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT password_hash FROM users WHERE id = ?1",
            [user_id],
            |row| row.get(0),
        )
        .optional()?)
}

pub fn set_password(conn: &Connection, user_id: i64, hash: &str) -> AppResult<()> {
    conn.execute(
        "UPDATE users SET password_hash = ?1 WHERE id = ?2",
        params![hash, user_id],
    )?;
    conn.execute("DELETE FROM password_resets WHERE user_id = ?1", [user_id])?;
    Ok(())
}

/// Signs someone out everywhere, except the session with `keep`.
pub fn end_sessions(conn: &Connection, user_id: i64, keep: Option<&[u8]>) -> AppResult<()> {
    conn.execute(
        "DELETE FROM sessions WHERE user_id = ?1 AND token_hash IS NOT ?2",
        params![user_id, keep],
    )?;
    Ok(())
}

/// Replaces any earlier reset link for the user with a new one.
pub fn create_password_reset(
    conn: &Connection,
    token_hash: &[u8],
    user_id: i64,
    created_by: i64,
    now: i64,
    expires_at: i64,
) -> AppResult<()> {
    conn.execute("DELETE FROM password_resets WHERE user_id = ?1", [user_id])?;
    conn.execute(
        "INSERT INTO password_resets (token_hash, user_id, created_by, created_at, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![token_hash, user_id, created_by, now, expires_at],
    )?;
    Ok(())
}

/// Who a valid reset link is for.
pub fn password_reset_user(
    conn: &Connection,
    token_hash: &[u8],
    now: i64,
) -> AppResult<Option<User>> {
    let user_id: Option<i64> = conn
        .query_row(
            "SELECT r.user_id FROM password_resets r JOIN users u ON u.id = r.user_id
             WHERE r.token_hash = ?1 AND r.expires_at > ?2 AND u.deactivated_at IS NULL",
            params![token_hash, now],
            |row| row.get(0),
        )
        .optional()?;
    user_id.map_or(Ok(None), |id| user(conn, id))
}

/// Deactivates someone, signing them out everywhere, or reactivates them.
pub fn set_deactivated(conn: &Connection, user_id: i64, at: Option<i64>) -> AppResult<()> {
    conn.execute(
        "UPDATE users SET deactivated_at = ?1 WHERE id = ?2",
        params![at, user_id],
    )?;
    if at.is_some() {
        end_sessions(conn, user_id, None)?;
        conn.execute(
            "DELETE FROM push_subscriptions WHERE user_id = ?1",
            [user_id],
        )?;
        conn.execute("DELETE FROM password_resets WHERE user_id = ?1", [user_id])?;
    }
    Ok(())
}

pub fn set_admin(conn: &Connection, user_id: i64, is_admin: bool) -> AppResult<()> {
    conn.execute(
        "UPDATE users SET is_admin = ?1 WHERE id = ?2",
        params![is_admin, user_id],
    )?;
    Ok(())
}

// Channels

/// Whether `?{n}` may read channel `c`: public channels are open to
/// everyone; private channels and direct messages to their members.
fn can_read(user_param: u8) -> String {
    format!(
        "((c.kind = 'public' AND c.private = 0) OR EXISTS (
             SELECT 1 FROM channel_members cm WHERE cm.channel_id = c.id AND cm.user_id = ?{user_param}))"
    )
}

pub fn create_channel(
    conn: &Connection,
    name: &str,
    private: bool,
    created_by: i64,
    now: i64,
) -> AppResult<i64> {
    conn.execute(
        "INSERT INTO channels (kind, name, private, created_by, created_at) VALUES ('public', ?1, ?2, ?3, ?4)",
        params![name, private, created_by, now],
    )?;
    let id = conn.last_insert_rowid();
    if private {
        add_member(conn, id, created_by)?;
    }
    set_manager(conn, id, created_by, true)?;
    Ok(id)
}

/// Whether any channel, public or private, has `name`.
pub fn channel_name_taken(conn: &Connection, name: &str) -> AppResult<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM channels WHERE kind = 'public' AND name = ?1",
            [name],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// `channel_id` if it is a public channel.
pub fn public_channel(conn: &Connection, channel_id: i64) -> AppResult<Option<i64>> {
    Ok(conn
        .query_row(
            "SELECT id FROM channels WHERE kind = 'public' AND private = 0 AND id = ?1",
            [channel_id],
            |row| row.get(0),
        )
        .optional()?)
}

/// Names of all public channels, alphabetically.
pub fn public_channel_names(conn: &Connection) -> AppResult<Vec<String>> {
    let mut statement = conn
        .prepare("SELECT name FROM channels WHERE kind = 'public' AND private = 0 ORDER BY name")?;
    let names = statement.query_map([], |row| row.get(0))?;
    Ok(names.collect::<Result<_, _>>()?)
}

pub fn public_channel_id(conn: &Connection, name: &str) -> AppResult<Option<i64>> {
    Ok(conn
        .query_row(
            "SELECT id FROM channels WHERE kind = 'public' AND private = 0 AND name = ?1",
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

/// Returns the channel if `user_id` may read it. Private channels and
/// direct conversations are visible to their members only.
pub fn channel_for(conn: &Connection, channel_id: i64, user_id: i64) -> AppResult<Option<Channel>> {
    Ok(conn
        .query_row(
            &format!(
                "SELECT c.id, c.kind, c.name, c.topic,
                    (SELECT u.display_name FROM channel_members m JOIN users u ON u.id = m.user_id
                     WHERE m.channel_id = c.id AND m.user_id != ?2),
                    (SELECT u.display_name FROM users u WHERE u.id = ?2),
                    c.private, COALESCE(p.hidden, 0), COALESCE(p.muted, 0), c.created_by,
                    c.post_policy, c.reply_policy, c.react_policy,
                    COALESCE((SELECT is_admin FROM users WHERE id = ?2), 0)
                        OR EXISTS (SELECT 1 FROM channel_managers cm2 WHERE cm2.channel_id = c.id AND cm2.user_id = ?2)
                 FROM channels c
                 LEFT JOIN channel_prefs p ON p.channel_id = c.id AND p.user_id = ?2
                 WHERE c.id = ?1 AND {}",
                can_read(2)
            ),
            params![channel_id, user_id],
            |row| {
                let kind: String = row.get(1)?;
                let name: Option<String> = row.get(2)?;
                let other: Option<String> = row.get(4)?;
                let me: Option<String> = row.get(5)?;
                let private: bool = row.get(6)?;
                let (kind, name) = if kind == "dm" {
                    let label =
                        other.unwrap_or_else(|| format!("{} (you)", me.unwrap_or_default()));
                    (ChannelKind::Direct, label)
                } else if private {
                    (ChannelKind::Private, name.unwrap_or_default())
                } else {
                    (ChannelKind::Public, name.unwrap_or_default())
                };
                Ok(Channel {
                    id: row.get(0)?,
                    kind,
                    name,
                    topic: row.get(3)?,
                    left: row.get(7)?,
                    muted: row.get(8)?,
                    created_by: row.get(9)?,
                    // Direct conversations have no rules.
                    posting: if kind == ChannelKind::Direct {
                        Posting::default()
                    } else {
                        Posting {
                            post: Policy::parse(&row.get::<_, String>(10)?).unwrap_or_default(),
                            reply: Policy::parse(&row.get::<_, String>(11)?).unwrap_or_default(),
                            react: Policy::parse(&row.get::<_, String>(12)?).unwrap_or_default(),
                        }
                    },
                    manager: row.get(13)?,
                })
            },
        )
        .optional()?)
}

pub fn set_posting(conn: &Connection, channel_id: i64, posting: Posting) -> AppResult<()> {
    conn.execute(
        "UPDATE channels SET post_policy = ?1, reply_policy = ?2, react_policy = ?3 WHERE id = ?4",
        params![
            posting.post.key(),
            posting.reply.key(),
            posting.react.key(),
            channel_id
        ],
    )?;
    Ok(())
}

/// A channel's managers, not counting admins.
pub fn channel_managers(conn: &Connection, channel_id: i64) -> AppResult<Vec<User>> {
    let mut statement = conn.prepare(&format!(
        "SELECT {USER_COLUMNS} FROM users WHERE id IN
         (SELECT user_id FROM channel_managers WHERE channel_id = ?1)
         ORDER BY display_name COLLATE NOCASE"
    ))?;
    let managers = statement.query_map([channel_id], user_from_row)?;
    Ok(managers.collect::<Result<_, _>>()?)
}

pub fn set_manager(
    conn: &Connection,
    channel_id: i64,
    user_id: i64,
    manager: bool,
) -> AppResult<()> {
    if manager {
        conn.execute(
            "INSERT OR IGNORE INTO channel_managers (channel_id, user_id) VALUES (?1, ?2)",
            params![channel_id, user_id],
        )?;
    } else {
        conn.execute(
            "DELETE FROM channel_managers WHERE channel_id = ?1 AND user_id = ?2",
            params![channel_id, user_id],
        )?;
    }
    Ok(())
}

/// Members who receive live updates for a private channel or a direct
/// conversation, or `None` for a public channel that everyone can read.
pub fn audience(conn: &Connection, channel_id: i64) -> AppResult<Option<Vec<i64>>> {
    let (kind, private): (String, bool) = conn.query_row(
        "SELECT kind, private FROM channels WHERE id = ?1",
        [channel_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if kind != "dm" && !private {
        return Ok(None);
    }
    members(conn, channel_id)
        .map(|members| Some(members.into_iter().map(|member| member.id).collect()))
}

/// People in a private channel or a direct conversation.
pub fn members(conn: &Connection, channel_id: i64) -> AppResult<Vec<User>> {
    let mut statement = conn.prepare(&format!(
        "SELECT {USER_COLUMNS} FROM users WHERE id IN
         (SELECT user_id FROM channel_members WHERE channel_id = ?1)
         ORDER BY display_name COLLATE NOCASE"
    ))?;
    let members = statement.query_map([channel_id], user_from_row)?;
    Ok(members.collect::<Result<_, _>>()?)
}

pub fn add_member(conn: &Connection, channel_id: i64, user_id: i64) -> AppResult<()> {
    conn.execute(
        "INSERT OR IGNORE INTO channel_members (channel_id, user_id) VALUES (?1, ?2)",
        params![channel_id, user_id],
    )?;
    Ok(())
}

pub fn remove_member(conn: &Connection, channel_id: i64, user_id: i64) -> AppResult<()> {
    conn.execute(
        "DELETE FROM channel_members WHERE channel_id = ?1 AND user_id = ?2",
        params![channel_id, user_id],
    )?;
    Ok(())
}

/// Leaves (hides) or rejoins a public channel for one person.
pub fn set_channel_hidden(
    conn: &Connection,
    user_id: i64,
    channel_id: i64,
    hidden: bool,
) -> AppResult<()> {
    conn.execute(
        "INSERT INTO channel_prefs (user_id, channel_id, hidden) VALUES (?1, ?2, ?3)
         ON CONFLICT (user_id, channel_id) DO UPDATE SET hidden = excluded.hidden",
        params![user_id, channel_id, hidden],
    )?;
    Ok(())
}

pub fn set_channel_muted(
    conn: &Connection,
    user_id: i64,
    channel_id: i64,
    muted: bool,
) -> AppResult<()> {
    conn.execute(
        "INSERT INTO channel_prefs (user_id, channel_id, muted) VALUES (?1, ?2, ?3)
         ON CONFLICT (user_id, channel_id) DO UPDATE SET muted = excluded.muted",
        params![user_id, channel_id, muted],
    )?;
    Ok(())
}

/// A channel in the directory.
pub struct DirectoryEntry {
    pub id: i64,
    pub name: String,
    pub topic: String,
    pub private: bool,
    /// For public channels: whether it is in the person's sidebar.
    pub joined: bool,
    pub messages: i64,
}

/// Every public channel, and the private ones `user_id` is in.
pub fn channel_directory(conn: &Connection, user_id: i64) -> AppResult<Vec<DirectoryEntry>> {
    let mut statement = conn.prepare(&format!(
        "SELECT c.id, c.name, c.topic, c.private, NOT COALESCE(p.hidden, 0),
                (SELECT COUNT(*) FROM messages m WHERE m.channel_id = c.id)
         FROM channels c LEFT JOIN channel_prefs p ON p.channel_id = c.id AND p.user_id = ?1
         WHERE c.kind = 'public' AND {}
         ORDER BY c.name COLLATE NOCASE",
        can_read(1)
    ))?;
    let entries = statement.query_map([user_id], |row| {
        Ok(DirectoryEntry {
            id: row.get(0)?,
            name: row.get(1)?,
            topic: row.get(2)?,
            private: row.get(3)?,
            joined: row.get(4)?,
            messages: row.get(5)?,
        })
    })?;
    Ok(entries.collect::<Result<_, _>>()?)
}

/// Finds or creates the direct conversation between two users.
/// Whether two people already have a conversation.
pub fn direct_channel_exists(conn: &Connection, user: i64, other: i64) -> AppResult<bool> {
    let key = format!("{}:{}", user.min(other), user.max(other));
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM channels WHERE dm_key = ?1)",
        [&key],
        |row| row.get(0),
    )?)
}

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
    // Public channels someone left stay out; muted ones never look unread.
    let mut statement = conn.prepare(&format!(
        "SELECT c.id, c.name, {unread} AND NOT COALESCE(p.muted, 0), c.private, COALESCE(p.muted, 0),
                c.post_policy = 'managers'
         FROM channels c LEFT JOIN channel_prefs p ON p.channel_id = c.id AND p.user_id = ?1
         WHERE c.kind = 'public' AND NOT COALESCE(p.hidden, 0) AND {}
         ORDER BY c.name COLLATE NOCASE",
        can_read(1)
    ))?;
    let channels = statement
        .query_map([user_id], |row| {
            Ok(SidebarItem {
                channel_id: row.get(0)?,
                label: row.get(1)?,
                unread: row.get(2)?,
                muted: row.get(4)?,
                look: if row.get(3)? {
                    SidebarLook::Private
                } else if row.get(5)? {
                    SidebarLook::Announcement
                } else {
                    SidebarLook::Channel
                },
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
                muted: false,
                look: SidebarLook::Direct,
            })
        })?
        .collect::<Result<_, _>>()?;
    let activity = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM activity a JOIN users u ON u.id = a.user_id
         WHERE a.user_id = ?1 AND a.message_id > u.activity_seen_id)",
        [user_id],
        |row| row.get(0),
    )?;
    Ok(Sidebar {
        channels,
        direct,
        activity,
    })
}

/// How many conversations look unread to `user_id`, for the app icon.
pub fn unread_count(conn: &Connection, user_id: i64) -> AppResult<usize> {
    let sidebar = sidebar(conn, user_id)?;
    Ok(sidebar
        .channels
        .iter()
        .chain(&sidebar.direct)
        .filter(|item| item.unread)
        .count())
}

/// The channel to open after signing in: `general` if it exists.
pub fn home_channel(conn: &Connection) -> AppResult<Option<i64>> {
    Ok(conn
        .query_row(
            "SELECT id FROM channels WHERE kind = 'public' AND private = 0
             ORDER BY name != 'general', id LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()?)
}

// Messages

pub const MESSAGE_SELECT: &str = "SELECT m.id, m.channel_id, m.parent_id, m.user_id, u.display_name,
        m.bot_name, m.bot_icon_url, m.body, m.attachments, m.created_at, (m.webhook_id IS NOT NULL OR m.slack_format),
        u.avatar_file_id, COALESCE(u.status_emoji, ''), m.gif,
        (SELECT COUNT(*) FROM messages r WHERE r.parent_id = m.id),
        m.edited_at, m.deleted_at IS NOT NULL,
        CASE WHEN m.pinned_at IS NULL THEN NULL
             ELSE COALESCE((SELECT p.display_name FROM users p WHERE p.id = m.pinned_by), 'Someone') END,
        m.preview, m.poll, m.buttons, m.automation_id
    FROM messages m LEFT JOIN users u ON u.id = m.user_id";

/// How many columns [`MESSAGE_SELECT`] reads; queries add theirs after.
pub const MESSAGE_COLUMNS: usize = 22;

pub fn message_from_row(row: &Row<'_>) -> rusqlite::Result<Message> {
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
        edited_at: row.get(15)?,
        deleted: row.get(16)?,
        pinned_by: row.get(17)?,
        preview: row
            .get::<_, Option<String>>(18)?
            .and_then(|json| serde_json::from_str(&json).ok()),
        poll: row
            .get::<_, Option<String>>(19)?
            .and_then(|json| crate::polls::Spec::from_json(&json))
            .map(|spec| Poll {
                kind: spec.kind,
                options: spec
                    .options
                    .into_iter()
                    .map(|label| PollOption {
                        label,
                        ..PollOption::default()
                    })
                    .collect(),
                ballots: Vec::new(),
                closed: spec.closed_at.is_some(),
            }),
        buttons: row
            .get::<_, Option<String>>(20)?
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default(),
        automation_id: row.get(21)?,
    })
}

/// Loads poll votes for already loaded messages.
fn hydrate_votes(conn: &Connection, messages: &mut [Message], ids: &str) -> AppResult<()> {
    let mut statement = conn.prepare(
        "SELECT v.message_id, v.option, v.user_id, COALESCE(u.display_name, 'Someone')
         FROM poll_votes v LEFT JOIN users u ON u.id = v.user_id
         WHERE v.message_id IN (SELECT value FROM json_each(?1)) ORDER BY v.created_at",
    )?;
    let votes = statement.query_map([ids], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    for vote in votes {
        let (message_id, option, user_id, name) = vote?;
        let option = messages
            .iter_mut()
            .find(|m| m.id == message_id)
            .and_then(|message| message.poll.as_mut())
            .and_then(|poll| poll.options.get_mut(usize::try_from(option).ok()?));
        if let Some(option) = option {
            option.voters.push(user_id);
            option.names.push(name);
        }
    }
    let mut statement = conn.prepare(
        "SELECT v.message_id, v.option, v.user_id, COALESCE(u.display_name, 'Someone')
         FROM poll_marks v LEFT JOIN users u ON u.id = v.user_id
         WHERE v.message_id IN (SELECT value FROM json_each(?1))
         ORDER BY v.message_id, v.user_id, v.rank, v.option",
    )?;
    let marks = statement.query_map([ids], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    for mark in marks {
        let (message_id, option, user_id, name) = mark?;
        let Some(poll) = messages
            .iter_mut()
            .find(|m| m.id == message_id)
            .and_then(|message| message.poll.as_mut())
        else {
            continue;
        };
        let Some(option) = usize::try_from(option)
            .ok()
            .filter(|option| *option < poll.options.len())
        else {
            continue;
        };
        match poll.kind {
            crate::polls::Kind::Ranked => {
                if let Some(ballot) = poll
                    .ballots
                    .iter_mut()
                    .find(|ballot| ballot.user_id == user_id)
                {
                    ballot.ranking.push(option);
                } else {
                    if let Some(first) = poll.options.get_mut(option) {
                        first.voters.push(user_id);
                        first.names.push(name.clone());
                    }
                    poll.ballots.push(Ballot {
                        user_id,
                        name,
                        ranking: vec![option],
                    });
                }
            }
            _ => {
                if let Some(option) = poll.options.get_mut(option) {
                    option.voters.push(user_id);
                    option.names.push(name);
                }
            }
        }
    }
    Ok(())
}

/// Loads files and reactions for already loaded messages.
pub fn hydrate(conn: &Connection, messages: &mut [Message]) -> AppResult<()> {
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
    hydrate_votes(conn, messages, &ids)?;
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
        "INSERT INTO messages (channel_id, parent_id, user_id, webhook_id, automation_id, bot_name, bot_icon_url, body, attachments, created_at, gif, poll, buttons)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
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
            new.gif.and_then(|gif| serde_json::to_string(gif).ok()),
            new.poll.map(crate::polls::Spec::to_json),
            (!new.buttons.is_empty())
                .then(|| serde_json::to_string(new.buttons).ok())
                .flatten()
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
    if let Some(poll) = new.poll {
        searchable.extend(poll.options.iter().cloned());
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

/// Fills the search index from every message that isn't deleted. Used when
/// the index changes shape.
pub fn rebuild_search_index(conn: &Connection) -> AppResult<()> {
    let mut statement = conn.prepare(
        "SELECT m.id, m.body, m.attachments, m.gif, m.poll,
                (SELECT group_concat(f.name, char(10)) FROM message_files mf JOIN files f ON f.id = mf.file_id
                 WHERE mf.message_id = m.id)
         FROM messages m WHERE m.deleted_at IS NULL",
    )?;
    let mut insert = conn.prepare("INSERT INTO messages_fts (rowid, content) VALUES (?1, ?2)")?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let id: i64 = row.get(0)?;
        let mut searchable = vec![row.get::<_, String>(1)?];
        if let Some(files) = row.get::<_, Option<String>>(5)? {
            searchable.push(files);
        }
        let attachments: Vec<Attachment> = row
            .get::<_, Option<String>>(2)?
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();
        for attachment in &attachments {
            searchable.extend(attachment.searchable_text());
        }
        if let Some(gif) = row
            .get::<_, Option<String>>(3)?
            .and_then(|json| serde_json::from_str::<Gif>(&json).ok())
        {
            searchable.push(gif.title);
        }
        if let Some(poll) = row
            .get::<_, Option<String>>(4)?
            .and_then(|json| crate::polls::Spec::from_json(&json))
        {
            searchable.extend(poll.options);
        }
        insert.execute(params![id, searchable.join("\n")])?;
    }
    Ok(())
}

/// Rewrites a message's search entry from what it holds now.
fn reindex(conn: &Connection, id: i64) -> AppResult<()> {
    conn.execute("DELETE FROM messages_fts WHERE rowid = ?1", [id])?;
    let Some(message) = message(conn, id)? else {
        return Ok(());
    };
    if message.deleted {
        return Ok(());
    }
    let mut searchable = vec![message.body.clone()];
    searchable.extend(message.files.iter().map(|file| file.name.clone()));
    for attachment in &message.attachments {
        searchable.extend(attachment.searchable_text());
    }
    if let Some(gif) = &message.gif {
        searchable.push(gif.title.clone());
    }
    if let Some(poll) = &message.poll {
        searchable.extend(poll.options.iter().map(|option| option.label.clone()));
    }
    conn.execute(
        "INSERT INTO messages_fts (rowid, content) VALUES (?1, ?2)",
        params![id, searchable.join("\n")],
    )?;
    Ok(())
}

/// The message if `user_id` may read its channel.
pub fn readable_message(
    conn: &Connection,
    user_id: i64,
    channel_id: i64,
    message_id: i64,
) -> AppResult<Option<Message>> {
    if channel_for(conn, channel_id, user_id)?.is_none() {
        return Ok(None);
    }
    Ok(message(conn, message_id)?.filter(|message| message.channel_id == channel_id))
}

/// Replaces a message's text and marks it edited.
pub fn edit_message(conn: &Connection, id: i64, body: &str, now: i64) -> AppResult<()> {
    conn.execute(
        "UPDATE messages SET body = ?1, edited_at = ?2 WHERE id = ?3 AND deleted_at IS NULL",
        params![body, now, id],
    )?;
    reindex(conn, id)
}

/// Deletes a message with its files and reactions. A message that starts a
/// thread with replies stays as a placeholder, so the thread does too.
pub fn delete_message(conn: &Connection, id: i64, now: i64) -> AppResult<()> {
    // Files only this message uses go with it.
    conn.execute(
        "DELETE FROM files WHERE id IN (
             SELECT mf.file_id FROM message_files mf WHERE mf.message_id = ?1
             AND NOT EXISTS (SELECT 1 FROM message_files o WHERE o.file_id = mf.file_id AND o.message_id != ?1))",
        [id],
    )?;
    let has_replies: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM messages WHERE parent_id = ?1)",
        [id],
        |row| row.get(0),
    )?;
    if has_replies {
        conn.execute(
            "UPDATE messages SET body = '', attachments = NULL, gif = NULL, preview = NULL, deleted_at = ?1,
                 pinned_at = NULL, pinned_by = NULL WHERE id = ?2",
            params![now, id],
        )?;
        conn.execute("DELETE FROM message_files WHERE message_id = ?1", [id])?;
        conn.execute("DELETE FROM reactions WHERE message_id = ?1", [id])?;
        conn.execute(
            "DELETE FROM automation_reactions WHERE message_id = ?1",
            [id],
        )?;
        conn.execute("DELETE FROM saved_messages WHERE message_id = ?1", [id])?;
        conn.execute("DELETE FROM messages_fts WHERE rowid = ?1", [id])?;
    } else {
        let parent: Option<i64> = conn.query_row(
            "SELECT parent_id FROM messages WHERE id = ?1",
            [id],
            |row| row.get(0),
        )?;
        conn.execute("DELETE FROM messages WHERE id = ?1", [id])?;
        // A placeholder whose last reply is gone has nothing left to show.
        conn.execute(
            "DELETE FROM messages WHERE id = ?1 AND deleted_at IS NOT NULL
             AND NOT EXISTS (SELECT 1 FROM messages r WHERE r.parent_id = ?1)",
            [parent],
        )?;
    }
    Ok(())
}

/// Stores a message's link preview. Returns whether it changed.
pub fn set_preview(conn: &Connection, id: i64, preview: Option<&LinkPreview>) -> AppResult<bool> {
    let json = preview
        .map(serde_json::to_string)
        .transpose()
        .map_err(crate::error::AppError::internal)?;
    Ok(conn.execute(
        "UPDATE messages SET preview = ?1 WHERE id = ?2 AND preview IS NOT ?1 AND deleted_at IS NULL",
        params![json, id],
    )? > 0)
}

/// Votes for `option`, moves the vote there, or takes it back when it is
/// already there.
pub fn vote(
    conn: &Connection,
    message_id: i64,
    user_id: i64,
    option: i64,
    now: i64,
) -> AppResult<()> {
    let removed = conn.execute(
        "DELETE FROM poll_votes WHERE message_id = ?1 AND user_id = ?2 AND option = ?3",
        params![message_id, user_id, option],
    )?;
    if removed == 0 {
        conn.execute(
            "INSERT INTO poll_votes (message_id, user_id, option, created_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (message_id, user_id) DO UPDATE SET option = excluded.option, created_at = excluded.created_at",
            params![message_id, user_id, option, now],
        )?;
    }
    Ok(())
}

/// Picks `option` in a poll where people pick several, or takes it back.
pub fn toggle_mark(
    conn: &Connection,
    message_id: i64,
    user_id: i64,
    option: i64,
    now: i64,
) -> AppResult<()> {
    let removed = conn.execute(
        "DELETE FROM poll_marks WHERE message_id = ?1 AND user_id = ?2 AND option = ?3",
        params![message_id, user_id, option],
    )?;
    if removed == 0 {
        conn.execute(
            "INSERT INTO poll_marks (message_id, user_id, option, rank, created_at) VALUES (?1, ?2, ?3, 0, ?4)",
            params![message_id, user_id, option, now],
        )?;
    }
    Ok(())
}

/// Replaces someone's ranking in a ranked poll; an empty one takes it back.
pub fn set_ranking(
    conn: &Connection,
    message_id: i64,
    user_id: i64,
    ranking: &[usize],
    now: i64,
) -> AppResult<()> {
    conn.execute(
        "DELETE FROM poll_marks WHERE message_id = ?1 AND user_id = ?2",
        params![message_id, user_id],
    )?;
    for (rank, option) in (1_i64..).zip(ranking) {
        conn.execute(
            "INSERT INTO poll_marks (message_id, user_id, option, rank, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![message_id, user_id, i64::try_from(*option).unwrap_or(i64::MAX), rank, now],
        )?;
    }
    Ok(())
}

/// Ends voting in a poll.
pub fn close_poll(conn: &Connection, message_id: i64, now: i64) -> AppResult<()> {
    let json: Option<String> = conn
        .query_row(
            "SELECT poll FROM messages WHERE id = ?1",
            [message_id],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    let Some(mut spec) = json.as_deref().and_then(crate::polls::Spec::from_json) else {
        return Err(crate::error::AppError::NotFound);
    };
    spec.closed_at.get_or_insert(now);
    conn.execute(
        "UPDATE messages SET poll = ?1 WHERE id = ?2",
        params![spec.to_json(), message_id],
    )?;
    Ok(())
}

/// Replaces an automation's message text and buttons, without marking it
/// edited.
pub fn update_bot_message(
    conn: &Connection,
    id: i64,
    body: Option<&str>,
    buttons: Option<&[Button]>,
) -> AppResult<()> {
    if let Some(body) = body {
        conn.execute(
            "UPDATE messages SET body = ?1 WHERE id = ?2",
            params![body, id],
        )?;
    }
    if let Some(buttons) = buttons {
        let json = (!buttons.is_empty())
            .then(|| serde_json::to_string(buttons))
            .transpose()
            .map_err(crate::error::AppError::internal)?;
        conn.execute(
            "UPDATE messages SET buttons = ?1 WHERE id = ?2",
            params![json, id],
        )?;
    }
    reindex(conn, id)
}

/// Pins or unpins a message. Returns whether it is pinned now.
pub fn toggle_pin(conn: &Connection, id: i64, user_id: i64, now: i64) -> AppResult<bool> {
    let pinned: bool = conn.query_row(
        "SELECT pinned_at IS NOT NULL FROM messages WHERE id = ?1",
        [id],
        |row| row.get(0),
    )?;
    if pinned {
        conn.execute(
            "UPDATE messages SET pinned_at = NULL, pinned_by = NULL WHERE id = ?1",
            [id],
        )?;
    } else {
        conn.execute(
            "UPDATE messages SET pinned_at = ?1, pinned_by = ?2 WHERE id = ?3 AND deleted_at IS NULL",
            params![now, user_id, id],
        )?;
    }
    Ok(!pinned)
}

/// A channel's pinned messages, most recently pinned first.
pub fn pinned_messages(conn: &Connection, channel_id: i64) -> AppResult<Vec<Message>> {
    let mut statement = conn.prepare(&format!(
        "{MESSAGE_SELECT} WHERE m.channel_id = ?1 AND m.pinned_at IS NOT NULL ORDER BY m.pinned_at DESC"
    ))?;
    let mut messages = statement
        .query_map([channel_id], message_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    hydrate(conn, &mut messages)?;
    Ok(messages)
}

pub fn pinned_count(conn: &Connection, channel_id: i64) -> AppResult<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM messages WHERE channel_id = ?1 AND pinned_at IS NOT NULL",
        [channel_id],
        |row| row.get(0),
    )?)
}

/// Saves a message for later, or forgets it. Returns whether it is saved now.
pub fn toggle_saved(conn: &Connection, user_id: i64, message_id: i64, now: i64) -> AppResult<bool> {
    let removed = conn.execute(
        "DELETE FROM saved_messages WHERE user_id = ?1 AND message_id = ?2",
        params![user_id, message_id],
    )?;
    if removed == 0 {
        conn.execute(
            "INSERT INTO saved_messages (user_id, message_id, created_at) VALUES (?1, ?2, ?3)",
            params![user_id, message_id, now],
        )?;
    }
    Ok(removed == 0)
}

/// The ids among `messages` that `user_id` saved.
pub fn saved_ids(
    conn: &Connection,
    user_id: i64,
    messages: &[&Message],
) -> AppResult<std::collections::HashSet<i64>> {
    let ids = serde_json::to_string(&messages.iter().map(|m| m.id).collect::<Vec<_>>())
        .map_err(crate::error::AppError::internal)?;
    let mut statement = conn.prepare(
        "SELECT message_id FROM saved_messages WHERE user_id = ?1
         AND message_id IN (SELECT value FROM json_each(?2))",
    )?;
    let rows = statement.query_map(params![user_id, ids], |row| row.get(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// A message with where it was posted, for lists across channels.
pub struct Located {
    pub message: Message,
    pub channel: String,
    pub is_direct: bool,
}

/// Where a message lives, as the reader sees it: `#name` or a person.
fn locate(conn: &Connection, user_id: i64, message: Message) -> AppResult<Option<Located>> {
    Ok(
        channel_for(conn, message.channel_id, user_id)?.map(|channel| Located {
            is_direct: channel.kind == ChannelKind::Direct,
            channel: channel.name,
            message,
        }),
    )
}

/// What `user_id` saved, newest first, in channels they can still read.
pub fn saved_messages(conn: &Connection, user_id: i64) -> AppResult<Vec<Located>> {
    let mut statement = conn.prepare(&format!(
        "{MESSAGE_SELECT} JOIN saved_messages s ON s.message_id = m.id
         WHERE s.user_id = ?1 ORDER BY s.created_at DESC LIMIT 200"
    ))?;
    let mut messages = statement
        .query_map([user_id], message_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    hydrate(conn, &mut messages)?;
    let mut located = Vec::new();
    for message in messages {
        located.extend(locate(conn, user_id, message)?);
    }
    Ok(located)
}

// Reminders and scheduled messages

pub fn user_timezone(conn: &Connection, user_id: i64) -> AppResult<String> {
    Ok(conn
        .query_row(
            "SELECT timezone FROM users WHERE id = ?1",
            [user_id],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or_else(|| "UTC".to_owned()))
}

pub fn set_user_timezone(conn: &Connection, user_id: i64, name: &str) -> AppResult<()> {
    conn.execute(
        "UPDATE users SET timezone = ?1 WHERE id = ?2 AND timezone != ?1",
        params![name, user_id],
    )?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct Reminder {
    pub id: i64,
    pub user_id: i64,
    pub text: String,
    /// A link to the message it is about.
    pub link: Option<String>,
    pub remind_at: i64,
}

pub fn add_reminder(
    conn: &Connection,
    user_id: i64,
    text: &str,
    message_id: Option<i64>,
    remind_at: i64,
    now: i64,
) -> AppResult<i64> {
    conn.execute(
        "INSERT INTO reminders (user_id, text, message_id, remind_at, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![user_id, text, message_id, remind_at, now],
    )?;
    Ok(conn.last_insert_rowid())
}

const REMINDER_SELECT: &str = "SELECT r.id, r.user_id, r.text,
        CASE WHEN m.id IS NULL THEN NULL ELSE '/c/' || m.channel_id || '/m/' || m.id END, r.remind_at
    FROM reminders r LEFT JOIN messages m ON m.id = r.message_id";

fn reminder_from_row(row: &Row<'_>) -> rusqlite::Result<Reminder> {
    Ok(Reminder {
        id: row.get(0)?,
        user_id: row.get(1)?,
        text: row.get(2)?,
        link: row.get(3)?,
        remind_at: row.get(4)?,
    })
}

pub fn reminders(conn: &Connection, user_id: i64) -> AppResult<Vec<Reminder>> {
    let mut statement = conn.prepare(&format!(
        "{REMINDER_SELECT} WHERE r.user_id = ?1 ORDER BY r.remind_at"
    ))?;
    let rows = statement.query_map([user_id], reminder_from_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn delete_reminder(conn: &Connection, user_id: i64, id: i64) -> AppResult<()> {
    conn.execute(
        "DELETE FROM reminders WHERE id = ?1 AND user_id = ?2",
        params![id, user_id],
    )?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct Scheduled {
    pub id: i64,
    pub user_id: i64,
    pub channel_id: i64,
    pub parent_id: Option<i64>,
    pub body: String,
    pub send_at: i64,
}

pub struct NewScheduled<'a> {
    pub user_id: i64,
    pub channel_id: i64,
    pub parent_id: Option<i64>,
    pub body: &'a str,
    pub send_at: i64,
}

pub fn add_scheduled(conn: &Connection, new: &NewScheduled<'_>, now: i64) -> AppResult<i64> {
    conn.execute(
        "INSERT INTO scheduled_messages (user_id, channel_id, parent_id, body, send_at, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            new.user_id,
            new.channel_id,
            new.parent_id,
            new.body,
            new.send_at,
            now
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

const SCHEDULED_SELECT: &str =
    "SELECT id, user_id, channel_id, parent_id, body, send_at FROM scheduled_messages";

fn scheduled_from_row(row: &Row<'_>) -> rusqlite::Result<Scheduled> {
    Ok(Scheduled {
        id: row.get(0)?,
        user_id: row.get(1)?,
        channel_id: row.get(2)?,
        parent_id: row.get(3)?,
        body: row.get(4)?,
        send_at: row.get(5)?,
    })
}

/// Someone's scheduled messages with where they go, soonest first.
pub fn scheduled_messages(conn: &Connection, user_id: i64) -> AppResult<Vec<(Scheduled, Channel)>> {
    let mut statement = conn.prepare(&format!(
        "{SCHEDULED_SELECT} WHERE user_id = ?1 ORDER BY send_at"
    ))?;
    let rows = statement
        .query_map([user_id], scheduled_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    let mut located = Vec::new();
    for scheduled in rows {
        if let Some(channel) = channel_for(conn, scheduled.channel_id, user_id)? {
            located.push((scheduled, channel));
        }
    }
    Ok(located)
}

pub fn delete_scheduled(conn: &Connection, user_id: i64, id: i64) -> AppResult<()> {
    conn.execute(
        "DELETE FROM scheduled_messages WHERE id = ?1 AND user_id = ?2",
        params![id, user_id],
    )?;
    Ok(())
}

/// Makes a scheduled message due now.
pub fn send_scheduled_now(conn: &Connection, user_id: i64, id: i64, now: i64) -> AppResult<()> {
    conn.execute(
        "UPDATE scheduled_messages SET send_at = ?1 WHERE id = ?2 AND user_id = ?3",
        params![now, id, user_id],
    )?;
    Ok(())
}

/// Removes and returns every reminder and scheduled message that is due.
pub fn take_due(conn: &mut Connection, now: i64) -> AppResult<(Vec<Reminder>, Vec<Scheduled>)> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let reminders = {
        let mut statement = tx.prepare(&format!(
            "{REMINDER_SELECT} WHERE r.remind_at <= ?1 ORDER BY r.remind_at"
        ))?;
        let rows = statement.query_map([now], reminder_from_row)?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    let scheduled = {
        let mut statement = tx.prepare(&format!(
            "{SCHEDULED_SELECT} WHERE send_at <= ?1 ORDER BY send_at, id"
        ))?;
        let rows = statement.query_map([now], scheduled_from_row)?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    tx.execute("DELETE FROM reminders WHERE remind_at <= ?1", [now])?;
    tx.execute("DELETE FROM scheduled_messages WHERE send_at <= ?1", [now])?;
    tx.commit()?;
    Ok((reminders, scheduled))
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

// Outgoing webhooks

#[derive(Debug, Clone)]
pub struct OutgoingWebhook {
    pub id: i64,
    pub name: String,
    pub url: String,
    /// Words a message must start with, or empty for every message.
    pub triggers: Vec<String>,
    pub token: String,
    pub last_at: Option<i64>,
    pub last_error: Option<String>,
}

pub struct NewOutgoingWebhook<'a> {
    pub channel_id: i64,
    pub name: &'a str,
    pub url: &'a str,
    pub triggers: &'a [String],
    pub token: &'a str,
}

pub fn create_outgoing_webhook(
    conn: &Connection,
    hook: &NewOutgoingWebhook<'_>,
    created_by: i64,
    now: i64,
) -> AppResult<()> {
    conn.execute(
        "INSERT INTO outgoing_webhooks (channel_id, name, url, triggers, token, created_by, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            hook.channel_id,
            hook.name,
            hook.url,
            hook.triggers.join(" "),
            hook.token,
            created_by,
            now
        ],
    )?;
    Ok(())
}

pub fn outgoing_webhooks(conn: &Connection, channel_id: i64) -> AppResult<Vec<OutgoingWebhook>> {
    let mut statement = conn.prepare(
        "SELECT id, name, url, triggers, token, last_at, last_error
         FROM outgoing_webhooks WHERE channel_id = ?1 ORDER BY id",
    )?;
    let hooks = statement.query_map([channel_id], |row| {
        let triggers: String = row.get(3)?;
        Ok(OutgoingWebhook {
            id: row.get(0)?,
            name: row.get(1)?,
            url: row.get(2)?,
            triggers: triggers.split_whitespace().map(ToOwned::to_owned).collect(),
            token: row.get(4)?,
            last_at: row.get(5)?,
            last_error: row.get(6)?,
        })
    })?;
    Ok(hooks.collect::<Result<_, _>>()?)
}

pub fn delete_outgoing_webhook(conn: &Connection, channel_id: i64, id: i64) -> AppResult<()> {
    conn.execute(
        "DELETE FROM outgoing_webhooks WHERE id = ?1 AND channel_id = ?2",
        params![id, channel_id],
    )?;
    Ok(())
}

pub fn record_outgoing_result(
    conn: &Connection,
    id: i64,
    now: i64,
    status: Option<u16>,
    error: Option<&str>,
) -> AppResult<()> {
    conn.execute(
        "UPDATE outgoing_webhooks SET last_at = ?1, last_status = ?2, last_error = ?3 WHERE id = ?4",
        params![now, status, error, id],
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
                     WHERE mf.file_id = f.id AND ((c.kind = 'public' AND c.private = 0) OR EXISTS (
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
///
/// Every message and page needs it, and with thousands of accounts building
/// it is real work, so it is kept per database until an account or custom
/// emoji is added or removed. Accounts are never deleted and usernames
/// never change, so the newest account id tells whether any were added.
pub fn render_context(conn: &Connection) -> AppResult<std::sync::Arc<crate::markup::Context>> {
    use std::{
        collections::HashMap as Map,
        sync::{Arc, LazyLock, Mutex},
    };
    type Cached = (String, Arc<crate::markup::Context>);
    static CACHE: LazyLock<Mutex<Map<String, Cached>>> = LazyLock::new(Mutex::default);

    let fingerprint: String = conn.query_row(
        "SELECT COALESCE((SELECT MAX(id) FROM users), 0) || ':' ||
                (SELECT COUNT(*) || ':' || COALESCE(MAX(created_at), 0) FROM custom_emoji)",
        [],
        |row| row.get(0),
    )?;
    let key = conn.path().unwrap_or_default().to_owned();
    if let Ok(cache) = CACHE.lock()
        && let Some((cached, ctx)) = cache.get(&key)
        && *cached == fingerprint
    {
        return Ok(Arc::clone(ctx));
    }
    let mut ctx = crate::markup::Context::default();
    for emoji in custom_emoji(conn)? {
        ctx.custom_emoji
            .insert(emoji.name, format!("/files/{}", emoji.file_id));
    }
    let mut statement = conn.prepare("SELECT lower(username) FROM users")?;
    for username in statement.query_map([], |row| row.get::<_, String>(0))? {
        ctx.usernames.insert(username?);
    }
    let ctx = Arc::new(ctx);
    // An in-memory database has no path, so it can't share a cache entry.
    if !key.is_empty()
        && let Ok(mut cache) = CACHE.lock()
    {
        cache.insert(key, (fingerprint, Arc::clone(&ctx)));
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

/// Who hears about a new message.
#[derive(Debug, Default)]
pub struct Recipients {
    /// Push notifications: the other members of a direct conversation,
    /// people in the thread, and anyone mentioned.
    pub notify: Vec<i64>,
    /// People mentioned by name, or by `@channel` and `@here`.
    pub mentioned: Vec<i64>,
    /// People who took part in the thread, for their Activity page.
    pub thread: Vec<i64>,
}

/// Works out who hears about `message`. Only people who can read the
/// channel are included, and never the sender.
pub fn recipients(
    conn: &Connection,
    message: &Message,
    sender: Option<i64>,
) -> AppResult<Recipients> {
    let members = audience(conn, message.channel_id)?;
    let readers: Option<std::collections::HashSet<i64>> = members
        .as_ref()
        .map(|members| members.iter().copied().collect());
    let direct = members.is_some()
        && conn.query_row(
            "SELECT kind = 'dm' FROM channels WHERE id = ?1",
            [message.channel_id],
            |row| row.get::<_, bool>(0),
        )?;
    let mut thread = Vec::new();
    if let Some(parent) = message.parent_id {
        let mut statement = conn.prepare(
            "SELECT DISTINCT user_id FROM messages WHERE (id = ?1 OR parent_id = ?1) AND user_id IS NOT NULL",
        )?;
        for user in statement.query_map([parent], |row| row.get::<_, i64>(0))? {
            thread.push(user?);
        }
    }
    let text = message.body.to_lowercase();
    let everyone = mentions(&text, "channel") || mentions(&text, "here");
    let mut mentioned = Vec::new();
    // Only look at everyone for @channel and @here; otherwise only at the
    // names written in the message.
    let names: Vec<&str> = if everyone {
        Vec::new()
    } else {
        text.split('@')
            .skip(1)
            .filter_map(|rest| {
                let end = rest
                    .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')))
                    .unwrap_or(rest.len());
                rest.get(..end).map(|name| name.trim_end_matches('.'))
            })
            .filter(|name| !name.is_empty())
            .collect()
    };
    let names = serde_json::to_string(&names).map_err(crate::error::AppError::internal)?;
    // Muted and left channels only reach people mentioned by name.
    let mut statement = conn.prepare(
        "SELECT u.id, lower(u.username), COALESCE(p.muted OR p.hidden, 0) FROM users u
         LEFT JOIN channel_prefs p ON p.user_id = u.id AND p.channel_id = ?1
         WHERE u.deactivated_at IS NULL
           AND (?2 OR lower(u.username) IN (SELECT value FROM json_each(?3)))",
    )?;
    for user in statement.query_map(params![message.channel_id, everyone, names], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, bool>(2)?,
        ))
    })? {
        let (id, username, quiet) = user?;
        if (everyone && !quiet && !direct) || mentions(&text, &username) {
            mentioned.push(id);
        }
    }
    let keep = |ids: &mut Vec<i64>| {
        ids.sort_unstable();
        ids.dedup();
        ids.retain(|id| {
            Some(*id) != sender && readers.as_ref().is_none_or(|readers| readers.contains(id))
        });
    };
    keep(&mut thread);
    keep(&mut mentioned);
    let mut notify: Vec<i64> = if direct {
        members.unwrap_or_default()
    } else {
        Vec::new()
    };
    notify.extend(&thread);
    notify.extend(&mentioned);
    keep(&mut notify);
    Ok(Recipients {
        notify,
        mentioned,
        thread,
    })
}

/// Adds a new message to the Activity pages of the people it concerns.
pub fn record_activity(
    conn: &Connection,
    message_id: i64,
    recipients: &Recipients,
) -> AppResult<()> {
    for (users, reason) in [
        (&recipients.thread, "reply"),
        (&recipients.mentioned, "mention"),
    ] {
        for user in users {
            conn.execute(
                "INSERT INTO activity (user_id, message_id, reason) VALUES (?1, ?2, ?3)
                 ON CONFLICT (user_id, message_id) DO UPDATE SET reason = excluded.reason",
                params![user, message_id, reason],
            )?;
        }
    }
    Ok(())
}

/// One entry on someone's Activity page.
pub struct ActivityItem {
    pub located: Located,
    pub mention: bool,
    pub new: bool,
}

/// Recent mentions and thread replies for `user_id`, newest first. Marks
/// them seen.
pub fn activity(conn: &Connection, user_id: i64) -> AppResult<Vec<ActivityItem>> {
    let seen: i64 = conn.query_row(
        "SELECT activity_seen_id FROM users WHERE id = ?1",
        [user_id],
        |row| row.get(0),
    )?;
    let mut statement = conn.prepare(
        &format!(
            "{MESSAGE_SELECT} JOIN activity a ON a.message_id = m.id
         WHERE a.user_id = ?1 ORDER BY m.id DESC LIMIT 100"
        )
        .replace(
            "FROM messages m LEFT JOIN",
            ", a.reason = 'mention' FROM messages m LEFT JOIN",
        ),
    )?;
    let rows = statement
        .query_map([user_id], |row| {
            Ok((message_from_row(row)?, row.get::<_, bool>(MESSAGE_COLUMNS)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let (mut messages, reasons): (Vec<Message>, Vec<bool>) = rows.into_iter().unzip();
    hydrate(conn, &mut messages)?;
    let mut items = Vec::new();
    for (message, mention) in messages.into_iter().zip(reasons) {
        let new = message.id > seen;
        if let Some(located) = locate(conn, user_id, message)? {
            items.push(ActivityItem {
                located,
                mention,
                new,
            });
        }
    }
    conn.execute(
        "UPDATE users SET activity_seen_id = MAX(activity_seen_id,
             COALESCE((SELECT MAX(message_id) FROM activity WHERE user_id = ?1), 0)) WHERE id = ?1",
        [user_id],
    )?;
    Ok(items)
}

/// Whether lowercase `text` notifies a whole channel.
pub fn mentions_everyone(text: &str) -> bool {
    mentions(text, "channel") || mentions(text, "here")
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
             WHERE t.token_hash = ?1 AND u.is_admin = 1 AND u.deactivated_at IS NULL",
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
