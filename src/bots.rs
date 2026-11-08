//! Bots: named accounts that scripts and other programs use through the chat
//! API. Each bot has tokens, and each token has scopes and the public
//! channels it may use. A token is kept only as its SHA-256 hash, the way MCP
//! tokens are, and is shown once when it's made.
//!
//! Bots reach public channels only. Private channels and direct messages are
//! never readable or writable by a bot, even when a token lists them: the
//! channel is looked up again on every request, so a channel made private
//! later stops working at once.

use rusqlite::{Connection, OptionalExtension, params};

use crate::{
    auth::same_bytes,
    error::{AppError, AppResult},
    store::{self, Message, Policy, Posting},
};

/// The scopes a token can have.
pub const SCOPES: [&str; 3] = ["channels:read", "messages:read", "messages:write"];

/// Last-use times are written at most this often (in milliseconds), so a
/// busy bot doesn't write to the database on every request.
const USE_RECORDED_EVERY_MS: i64 = 5 * 60 * 1000;

/// The channels a token may use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelScope {
    /// Every public channel.
    All,
    /// These public channels, by id.
    Only(Vec<i64>),
}

impl ChannelScope {
    pub fn allows(&self, channel_id: i64) -> bool {
        match self {
            Self::All => true,
            Self::Only(ids) => ids.contains(&channel_id),
        }
    }
}

/// A request's token, checked: which bot it acts as and what it may do.
#[derive(Debug, Clone)]
pub struct Caller {
    pub token_id: i64,
    pub label: String,
    pub expires_at: Option<i64>,
    pub bot_id: i64,
    /// The bot's handle, such as `deploy-bot`.
    pub name: String,
    /// What the bot's messages show as.
    pub display_name: String,
    pub icon: Option<String>,
    pub scopes: Vec<String>,
    pub channels: ChannelScope,
}

impl Caller {
    pub fn has(&self, scope: &str) -> bool {
        self.scopes.iter().any(|granted| granted == scope)
    }
}

/// A public channel as a bot sees it.
#[derive(Debug, Clone)]
pub struct PublicChannel {
    pub id: i64,
    pub name: String,
    pub posting: Posting,
}

/// What a token row holds, as the page lists it.
pub struct TokenRow {
    pub id: i64,
    pub label: String,
    pub scopes: Vec<String>,
    /// The channels' names, or `None` for every public channel.
    pub channels: Option<Vec<String>>,
    pub expires_at: Option<i64>,
    pub last_used_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

/// A bot and its tokens, as the page lists them.
pub struct BotRow {
    pub id: i64,
    pub name: String,
    pub display_name: String,
    pub created_at: i64,
    pub revoked_at: Option<i64>,
    pub tokens: Vec<TokenRow>,
}

/// A bot to make.
pub struct NewBot {
    pub name: String,
    pub display_name: String,
    pub icon: Option<String>,
    pub created_by: i64,
    pub now: i64,
}

/// A token to make for a bot.
pub struct NewToken {
    pub label: String,
    pub scopes: Vec<String>,
    pub channels: ChannelScope,
    pub token_hash: Vec<u8>,
    pub expires_at: Option<i64>,
    pub now: i64,
}

/// Which messages to read: the newest, the ones before a message, or the
/// ones after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Newest,
    Before(i64),
    After(i64),
}

/// The bot a token belongs to, if the token works now: not revoked, not
/// expired, and its bot not revoked. Records the use, at most every few
/// minutes. The hash is looked up by index and then compared without
/// stopping at the first difference.
pub fn authenticate(conn: &Connection, token_hash: &[u8], now: i64) -> AppResult<Option<Caller>> {
    let Some(found) = conn
        .query_row(
            "SELECT t.id, t.token_hash, t.label, t.scopes, t.channel_scope, t.expires_at, t.last_used_at,
                    b.id, b.name, b.display_name, b.icon
             FROM bot_tokens t JOIN bots b ON b.id = t.bot_id
             WHERE t.token_hash = ?1 AND t.revoked_at IS NULL AND b.revoked_at IS NULL",
            [token_hash],
            |row| {
                Ok(Found {
                    token_id: row.get(0)?,
                    stored_hash: row.get(1)?,
                    label: row.get(2)?,
                    scopes: row.get(3)?,
                    channel_scope: row.get(4)?,
                    expires_at: row.get(5)?,
                    last_used_at: row.get(6)?,
                    bot_id: row.get(7)?,
                    name: row.get(8)?,
                    display_name: row.get(9)?,
                    icon: row.get(10)?,
                })
            },
        )
        .optional()?
    else {
        return Ok(None);
    };
    if !same_bytes(&found.stored_hash, token_hash) {
        return Ok(None);
    }
    if found.expires_at.is_some_and(|expires| expires <= now) {
        return Ok(None);
    }
    let channels = if found.channel_scope == "list" {
        let mut statement = conn.prepare(
            "SELECT channel_id FROM bot_token_channels WHERE token_id = ?1 ORDER BY channel_id",
        )?;
        let ids = statement
            .query_map([found.token_id], |row| row.get(0))?
            .collect::<Result<Vec<i64>, _>>()?;
        ChannelScope::Only(ids)
    } else {
        ChannelScope::All
    };
    if found
        .last_used_at
        .is_none_or(|used| now.saturating_sub(used) >= USE_RECORDED_EVERY_MS)
    {
        conn.execute(
            "UPDATE bot_tokens SET last_used_at = ?1 WHERE id = ?2",
            params![now, found.token_id],
        )?;
    }
    Ok(Some(Caller {
        token_id: found.token_id,
        label: found.label,
        expires_at: found.expires_at,
        bot_id: found.bot_id,
        name: found.name,
        display_name: found.display_name,
        icon: found.icon,
        scopes: found.scopes.split_whitespace().map(str::to_owned).collect(),
        channels,
    }))
}

/// The token row joined to its bot, before the checks.
struct Found {
    token_id: i64,
    stored_hash: Vec<u8>,
    label: String,
    scopes: String,
    channel_scope: String,
    expires_at: Option<i64>,
    last_used_at: Option<i64>,
    bot_id: i64,
    name: String,
    display_name: String,
    icon: Option<String>,
}

/// A public channel, or `None` for private channels, direct messages and
/// channels that don't exist.
pub fn public_channel(conn: &Connection, channel_id: i64) -> AppResult<Option<PublicChannel>> {
    Ok(conn
        .query_row(
            "SELECT id, name, post_policy, reply_policy, react_policy FROM channels
             WHERE id = ?1 AND kind = 'public' AND private = 0",
            [channel_id],
            |row| {
                let policy = |index: usize| -> rusqlite::Result<Policy> {
                    let key: String = row.get(index)?;
                    Ok(Policy::parse(&key).unwrap_or_default())
                };
                Ok(PublicChannel {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    posting: Posting {
                        post: policy(2)?,
                        reply: policy(3)?,
                        react: policy(4)?,
                    },
                })
            },
        )
        .optional()?)
}

/// Every public channel: id, name and topic, by name.
pub fn public_channels(conn: &Connection) -> AppResult<Vec<(i64, String, String)>> {
    let mut statement = conn.prepare(
        "SELECT id, name, topic FROM channels WHERE kind = 'public' AND private = 0
         ORDER BY name COLLATE NOCASE",
    )?;
    let channels = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<Result<_, _>>()?;
    Ok(channels)
}

/// Messages of a channel, oldest first, up to `limit`. The flag says whether
/// more messages lie beyond the page in the direction it was read.
pub fn messages(
    conn: &Connection,
    channel_id: i64,
    page: Page,
    limit: usize,
) -> AppResult<(Vec<Message>, bool)> {
    let fetch = i64::try_from(limit.saturating_add(1)).unwrap_or(i64::MAX);
    let mut messages = match page {
        Page::After(after) => {
            let mut statement = conn.prepare(&format!(
                "{} WHERE m.channel_id = ?1 AND m.id > ?2 AND m.deleted_at IS NULL
                 ORDER BY m.id ASC LIMIT ?3",
                store::MESSAGE_SELECT
            ))?;
            statement
                .query_map(params![channel_id, after, fetch], store::message_from_row)?
                .collect::<Result<Vec<_>, _>>()?
        }
        Page::Newest | Page::Before(_) => {
            let before = match page {
                Page::Before(before) => Some(before),
                _ => None,
            };
            let mut statement = conn.prepare(&format!(
                "{} WHERE m.channel_id = ?1 AND (?2 IS NULL OR m.id < ?2) AND m.deleted_at IS NULL
                 ORDER BY m.id DESC LIMIT ?3",
                store::MESSAGE_SELECT
            ))?;
            statement
                .query_map(params![channel_id, before, fetch], store::message_from_row)?
                .collect::<Result<Vec<_>, _>>()?
        }
    };
    let has_more = messages.len() > limit;
    messages.truncate(limit);
    if matches!(page, Page::Newest | Page::Before(_)) {
        messages.reverse();
    }
    Ok((messages, has_more))
}

/// Every bot, newest first, with its tokens.
pub fn list(conn: &Connection) -> AppResult<Vec<BotRow>> {
    let mut statement = conn.prepare(
        "SELECT id, name, display_name, created_at, revoked_at FROM bots ORDER BY id DESC",
    )?;
    let mut bots: Vec<BotRow> = statement
        .query_map([], |row| {
            Ok(BotRow {
                id: row.get(0)?,
                name: row.get(1)?,
                display_name: row.get(2)?,
                created_at: row.get(3)?,
                revoked_at: row.get(4)?,
                tokens: Vec::new(),
            })
        })?
        .collect::<Result<_, _>>()?;
    let mut channel_names = std::collections::HashMap::<i64, Vec<String>>::new();
    let mut statement = conn.prepare(
        "SELECT btc.token_id, c.name FROM bot_token_channels btc
         JOIN channels c ON c.id = btc.channel_id ORDER BY c.name COLLATE NOCASE",
    )?;
    for (token_id, name) in statement
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?
    {
        channel_names.entry(token_id).or_default().push(name);
    }
    let mut statement = conn.prepare(
        "SELECT id, bot_id, label, scopes, channel_scope, created_at, expires_at, last_used_at, revoked_at
         FROM bot_tokens ORDER BY id DESC",
    )?;
    let tokens = statement
        .query_map([], |row| {
            let id: i64 = row.get(0)?;
            let scope: String = row.get(4)?;
            let token = TokenRow {
                id,
                label: row.get(2)?,
                scopes: row
                    .get::<_, String>(3)?
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect(),
                channels: (scope == "list")
                    .then(|| channel_names.get(&id).cloned().unwrap_or_default()),
                expires_at: row.get(6)?,
                last_used_at: row.get(7)?,
                revoked_at: row.get(8)?,
            };
            Ok((row.get::<_, i64>(1)?, token))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for (bot_id, token) in tokens {
        if let Some(bot) = bots.iter_mut().find(|bot| bot.id == bot_id) {
            bot.tokens.push(token);
        }
    }
    Ok(bots)
}

/// Makes a bot and returns its id. Names are unique, whatever their case.
pub fn create_bot(conn: &Connection, new: &NewBot) -> AppResult<i64> {
    let taken: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM bots WHERE name = ?1)",
        [&new.name],
        |row| row.get(0),
    )?;
    if taken {
        return Err(AppError::bad_request("Another bot already has that name."));
    }
    conn.execute(
        "INSERT INTO bots (name, display_name, icon, created_by, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![new.name, new.display_name, new.icon, new.created_by, new.now],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Revokes a bot: all its tokens stop working at once.
pub fn revoke_bot(conn: &Connection, bot_id: i64, now: i64) -> AppResult<()> {
    let changed = conn.execute(
        "UPDATE bots SET revoked_at = ?2 WHERE id = ?1 AND revoked_at IS NULL",
        params![bot_id, now],
    )?;
    if changed == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
}

/// Makes a token for a bot. Its channels must be public, and the bot must
/// not be revoked. Returns the token's id.
pub fn create_token(conn: &Connection, bot_id: i64, new: &NewToken) -> AppResult<i64> {
    let live: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM bots WHERE id = ?1 AND revoked_at IS NULL)",
        [bot_id],
        |row| row.get(0),
    )?;
    if !live {
        return Err(AppError::bad_request(
            "That bot is revoked, so it can't get new tokens.",
        ));
    }
    let channel_scope = match &new.channels {
        ChannelScope::All => "all",
        ChannelScope::Only(_) => "list",
    };
    conn.execute(
        "INSERT INTO bot_tokens (bot_id, token_hash, label, scopes, channel_scope, expires_at, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            bot_id,
            new.token_hash,
            new.label,
            new.scopes.join(" "),
            channel_scope,
            new.expires_at,
            new.now
        ],
    )?;
    let token_id = conn.last_insert_rowid();
    if let ChannelScope::Only(ids) = &new.channels {
        if ids.is_empty() {
            return Err(AppError::bad_request("Pick at least one public channel."));
        }
        for channel_id in ids {
            if public_channel(conn, *channel_id)?.is_none() {
                return Err(AppError::bad_request(
                    "Bots can only use public channels. Pick again.",
                ));
            }
            conn.execute(
                "INSERT OR IGNORE INTO bot_token_channels (token_id, channel_id) VALUES (?1, ?2)",
                params![token_id, channel_id],
            )?;
        }
    }
    Ok(token_id)
}

/// Revokes one token, immediately.
pub fn revoke_token(conn: &Connection, token_id: i64, now: i64) -> AppResult<()> {
    let changed = conn.execute(
        "UPDATE bot_tokens SET revoked_at = ?2 WHERE id = ?1 AND revoked_at IS NULL",
        params![token_id, now],
    )?;
    if changed == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
}

/// A bot's handle: 2 to 32 characters, letters, digits, `-` and `_`,
/// starting with a letter or digit. Stored in lower case.
pub fn check_name(name: &str) -> AppResult<String> {
    let name = name.trim().to_lowercase();
    let length = name.chars().count();
    let well_formed = (2..=32).contains(&length)
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    if !well_formed {
        return Err(AppError::bad_request(
            "Bot names are 2 to 32 letters, digits, - or _, starting with a letter or digit.",
        ));
    }
    Ok(name)
}

/// What a bot's messages show as: 1 to 60 characters.
pub fn check_display_name(name: &str) -> AppResult<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 60 {
        return Err(AppError::bad_request(
            "Give the bot a display name of up to 60 characters.",
        ));
    }
    Ok(name.to_owned())
}

/// An icon for the bot's messages: an image address, or an emoji such as
/// `:tada:`. Empty means none.
pub fn check_icon(icon: &str) -> AppResult<Option<String>> {
    let icon = icon.trim();
    if icon.is_empty() {
        return Ok(None);
    }
    let emoji = icon
        .strip_prefix(':')
        .and_then(|rest| rest.strip_suffix(':'))
        .is_some_and(|name| {
            !name.is_empty()
                && name.chars().all(|c| {
                    c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-' || c == '+'
                })
        });
    let address = (icon.starts_with("https://") || icon.starts_with("http://"))
        && icon.len() <= 500
        && !icon.contains(char::is_whitespace);
    if emoji || address {
        Ok(Some(icon.to_owned()))
    } else {
        Err(AppError::bad_request(
            "The icon is an emoji such as :tada: or an image address starting with https://.",
        ))
    }
}

/// The scopes asked for: known ones, without repeats, and at least one.
pub fn check_scopes(asked: &[&str]) -> AppResult<Vec<String>> {
    let mut scopes: Vec<String> = Vec::new();
    for scope in asked {
        if !SCOPES.contains(scope) {
            return Err(AppError::bad_request("Pick scopes from the list."));
        }
        if !scopes.iter().any(|known| known == scope) {
            scopes.push((*scope).to_owned());
        }
    }
    if scopes.is_empty() {
        return Err(AppError::bad_request("Give the token at least one scope."));
    }
    Ok(scopes)
}

/// When a token expires, from a choice on the page: `never`, or a number of
/// days.
pub fn check_expiry(choice: &str, now: i64) -> AppResult<Option<i64>> {
    const DAY_MS: i64 = 24 * 60 * 60 * 1000;
    if choice == "never" {
        return Ok(None);
    }
    let days: i64 = choice
        .parse()
        .ok()
        .filter(|days| [30, 90, 365].contains(days))
        .ok_or_else(|| AppError::bad_request("Pick when the token expires."))?;
    Ok(Some(now.saturating_add(days.saturating_mul(DAY_MS))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_are_short_lowercase_words() {
        assert_eq!(check_name("Deploy-Bot").unwrap(), "deploy-bot");
        assert!(check_name("a").is_err());
        assert!(check_name("-bot").is_err());
        assert!(check_name("has space").is_err());
        assert!(check_name(&"x".repeat(33)).is_err());
    }

    #[test]
    fn icons_are_emoji_or_addresses() {
        assert_eq!(check_icon("").unwrap(), None);
        assert_eq!(check_icon(":tada:").unwrap().as_deref(), Some(":tada:"));
        assert!(check_icon("https://example.com/a.png").is_ok());
        assert!(check_icon("javascript:alert(1)").is_err());
        assert!(check_icon("https://example.com/a b.png").is_err());
        assert!(check_icon(":Tada:").is_err());
    }

    #[test]
    fn scopes_are_known_and_unique() {
        assert_eq!(
            check_scopes(&["messages:write", "messages:write", "channels:read"]).unwrap(),
            ["messages:write", "channels:read"]
        );
        assert!(check_scopes(&[]).is_err());
        assert!(check_scopes(&["admin"]).is_err());
    }

    #[test]
    fn expiry_is_never_or_a_set_number_of_days() {
        assert_eq!(check_expiry("never", 0).unwrap(), None);
        assert_eq!(
            check_expiry("30", 1).unwrap(),
            Some(1 + 30 * 24 * 60 * 60 * 1000)
        );
        assert!(check_expiry("7", 0).is_err());
        assert!(check_expiry("soon", 0).is_err());
    }

    #[test]
    fn channel_scope_allows_listed_channels_only() {
        assert!(ChannelScope::All.allows(9));
        let listed = ChannelScope::Only(vec![2, 4]);
        assert!(listed.allows(4));
        assert!(!listed.allows(3));
    }

    #[test]
    fn display_names_are_trimmed_and_bounded() {
        assert_eq!(check_display_name("  Deploys ").unwrap(), "Deploys");
        assert!(check_display_name("   ").is_err());
        assert!(check_display_name(&"x".repeat(61)).is_err());
    }
}
