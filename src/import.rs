//! Importing a Slack export: people, channels, direct messages, threads
//! and reactions.
//!
//! A Slack export is a ZIP with `users.json`, `channels.json`, optionally
//! `groups.json` (private channels), `dms.json` and `mpims.json` (group
//! conversations), and a folder per conversation with a JSON file per day.
//! People are matched to existing accounts by username; the rest get
//! accounts with random passwords, so admins hand out reset links. Slack
//! file attachments need Slack's login to download, so messages name them
//! instead. Every message remembers where it came from, so importing the
//! same export twice adds nothing twice.

use std::{
    collections::HashMap,
    io::{Read, Seek},
};

use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use serde_json::Value;

use crate::{
    error::{AppError, AppResult},
    store::{self, NewMessage},
};

/// Largest JSON file read from an export.
const MAX_JSON_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Default)]
pub struct Report {
    /// Usernames of new, active accounts, which need reset links.
    pub people_created: Vec<String>,
    pub people_matched: usize,
    pub channels: usize,
    pub conversations: usize,
    pub messages: usize,
    pub skipped: usize,
    pub reactions: usize,
}

#[derive(Deserialize)]
struct SlackUser {
    id: String,
    name: String,
    #[serde(default)]
    real_name: Option<String>,
    #[serde(default)]
    deleted: bool,
    #[serde(default)]
    is_bot: bool,
    #[serde(default)]
    profile: Option<Value>,
}

#[derive(Deserialize)]
struct SlackChannel {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    members: Vec<String>,
    #[serde(default)]
    topic: Option<Value>,
    #[serde(default)]
    purpose: Option<Value>,
}

#[derive(Deserialize)]
struct SlackMessage {
    #[serde(default)]
    subtype: Option<String>,
    #[serde(default)]
    user: Option<String>,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    bot_profile: Option<Value>,
    #[serde(default)]
    text: String,
    ts: String,
    #[serde(default)]
    thread_ts: Option<String>,
    #[serde(default)]
    reactions: Vec<SlackReaction>,
    #[serde(default)]
    files: Vec<Value>,
}

#[derive(Deserialize)]
struct SlackReaction {
    name: String,
    #[serde(default)]
    users: Vec<String>,
}

/// The ZIP, with paths relative to where `users.json` is.
struct Export<R> {
    zip: zip::ZipArchive<R>,
    root: String,
}

impl<R: Read + Seek> Export<R> {
    fn open(reader: R) -> AppResult<Self> {
        let zip = zip::ZipArchive::new(reader)
            .map_err(|_| AppError::bad_request("That isn't a ZIP file."))?;
        let users = zip
            .file_names()
            .filter(|name| *name == "users.json" || name.ends_with("/users.json"))
            .min_by_key(|name| name.len())
            .ok_or_else(|| {
                AppError::bad_request("There's no users.json in it. Is it a Slack export?")
            })?;
        let root = users.trim_end_matches("users.json").to_owned();
        Ok(Self { zip, root })
    }

    fn json<T: for<'de> Deserialize<'de>>(&mut self, path: &str) -> AppResult<Option<T>> {
        let full = format!("{}{path}", self.root);
        let Ok(file) = self.zip.by_name(&full) else {
            return Ok(None);
        };
        let mut text = String::new();
        file.take(MAX_JSON_BYTES)
            .read_to_string(&mut text)
            .map_err(|_| AppError::bad_request(format!("{path} could not be read.")))?;
        serde_json::from_str(&text)
            .map(Some)
            .map_err(|error| AppError::bad_request(format!("{path} is not valid: {error}")))
    }

    /// The day files of a conversation's folder, in date order.
    fn days(&self, folder: &str) -> Vec<String> {
        let prefix = format!("{}{folder}/", self.root);
        let mut days: Vec<String> = self
            .zip
            .file_names()
            .filter_map(|name| name.strip_prefix(&prefix))
            .filter(|rest| {
                !rest.contains('/')
                    && std::path::Path::new(rest)
                        .extension()
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
            })
            .map(|rest| format!("{folder}/{rest}"))
            .collect();
        days.sort();
        days
    }
}

/// A Slack handle as a Sideporch username: lowercase letters, digits, `.`,
/// `-` and `_`, 2 to 32 characters.
fn username(handle: &str) -> String {
    let cleaned: String = handle
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .take(32)
        .collect();
    if cleaned.chars().count() < 2 {
        format!("{cleaned}_slack")
    } else {
        cleaned
    }
}

/// A channel name Sideporch accepts.
fn channel_name(name: &str) -> String {
    let cleaned: String = name
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' {
                c
            } else {
                '-'
            }
        })
        .take(40)
        .collect();
    if cleaned.is_empty() {
        "imported".to_owned()
    } else {
        cleaned
    }
}

fn profile_text(user: &SlackUser, key: &str) -> Option<String> {
    user.profile
        .as_ref()?
        .get(key)?
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(ToOwned::to_owned)
}

/// `1596036000.000100` in milliseconds.
fn millis(ts: &str) -> i64 {
    let (seconds, fraction) = ts.split_once('.').unwrap_or((ts, ""));
    let seconds: i64 = seconds.parse().unwrap_or(0);
    let millis: i64 = format!("{fraction:0<3}")
        .get(..3)
        .and_then(|digits| digits.parse().ok())
        .unwrap_or(0);
    seconds.saturating_mul(1000).saturating_add(millis)
}

/// Replaces `<@U123>` and `<@U123|name>` with `@username`.
fn mentions(text: &str, names: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<@") {
        out.push_str(rest.get(..start).unwrap_or_default());
        let tail = rest.get(start..).unwrap_or_default();
        let Some(end) = tail.find('>') else {
            out.push_str(tail);
            return out;
        };
        let inner = tail.get(2..end).unwrap_or_default();
        let id = inner.split('|').next().unwrap_or_default();
        match names.get(id) {
            Some(name) => {
                out.push('@');
                out.push_str(name);
            }
            None => out.push_str(tail.get(..=end).unwrap_or_default()),
        }
        rest = tail.get(end.saturating_add(1)..).unwrap_or_default();
    }
    out.push_str(rest);
    out
}

/// What a Slack event becomes, or `None` for joins, leaves and renames.
fn body(message: &SlackMessage, names: &HashMap<String, String>) -> Option<String> {
    if message.subtype.as_deref().is_some_and(|subtype| {
        subtype.starts_with("channel_") || subtype.starts_with("group_") || subtype == "bot_add"
    }) {
        return None;
    }
    let mut text = mentions(&message.text, names);
    let files: Vec<&str> = message
        .files
        .iter()
        .filter_map(|file| file.get("name").or_else(|| file.get("title"))?.as_str())
        .collect();
    if !files.is_empty() {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str("_Shared in Slack: ");
        text.push_str(&files.join(", "));
        text.push('_');
    }
    let text = text.trim().to_owned();
    (!text.is_empty()).then_some(text)
}

struct Importer<'a> {
    conn: &'a Connection,
    now: i64,
    /// Slack user id to Sideporch user id.
    people: HashMap<String, i64>,
    /// Slack user id to Sideporch username, for mentions.
    names: HashMap<String, String>,
    emoji: std::sync::Arc<crate::markup::Context>,
    report: Report,
}

impl Importer<'_> {
    fn people(&mut self, users: Vec<SlackUser>) -> AppResult<()> {
        for user in users {
            if user.is_bot || user.id == "USLACKBOT" {
                continue;
            }
            let name = username(&user.name);
            let existing: Option<i64> = self
                .conn
                .query_row("SELECT id FROM users WHERE username = ?1", [&name], |row| {
                    row.get(0)
                })
                .optional()?;
            let id = if let Some(id) = existing {
                self.report.people_matched = self.report.people_matched.saturating_add(1);
                id
            } else {
                let display = profile_text(&user, "display_name")
                    .or_else(|| profile_text(&user, "real_name"))
                    .or_else(|| user.real_name.clone())
                    .unwrap_or_else(|| user.name.clone());
                // Nobody knows this password; an admin sends a reset link.
                let unusable =
                    format!("imported-without-password:{}", crate::auth::random_token()?);
                let id =
                    store::create_user(self.conn, &name, &display, &unusable, false, self.now)?;
                if user.deleted {
                    store::set_deactivated(self.conn, id, Some(self.now))?;
                } else {
                    self.report.people_created.push(name.clone());
                }
                id
            };
            self.people.insert(user.id.clone(), id);
            self.names.insert(user.id, name);
        }
        Ok(())
    }

    /// Finds or creates the channel for a Slack conversation.
    fn channel(&mut self, channel: &SlackChannel, private: bool) -> AppResult<i64> {
        let name = channel_name(channel.name.as_deref().unwrap_or(&channel.id));
        let existing: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM channels WHERE kind = 'public' AND name = ?1",
                [&name],
                |row| row.get(0),
            )
            .optional()?;
        let id = if let Some(id) = existing {
            id
        } else {
            let creator = channel
                .members
                .iter()
                .find_map(|member| self.people.get(member).copied());
            self.conn.execute(
                "INSERT INTO channels (kind, name, private, created_by, created_at) VALUES ('public', ?1, ?2, ?3, ?4)",
                params![name, private, creator, self.now],
            )?;
            let id = self.conn.last_insert_rowid();
            let topic = [&channel.topic, &channel.purpose]
                .into_iter()
                .flatten()
                .find_map(|value| value.get("value")?.as_str().filter(|text| !text.is_empty()))
                .map(|text| text.chars().take(200).collect::<String>());
            if let Some(topic) = topic {
                store::set_topic(self.conn, id, &topic)?;
            }
            self.report.channels = self.report.channels.saturating_add(1);
            id
        };
        if private {
            for member in &channel.members {
                if let Some(user) = self.people.get(member) {
                    store::add_member(self.conn, id, *user)?;
                }
            }
        }
        Ok(id)
    }

    fn direct(&mut self, conversation: &SlackChannel) -> AppResult<Option<i64>> {
        let members: Vec<i64> = conversation
            .members
            .iter()
            .filter_map(|member| self.people.get(member).copied())
            .collect();
        let (first, second) = match members.as_slice() {
            [one] => (*one, *one),
            [one, two] => (*one, *two),
            _ => return Ok(None),
        };
        let key = format!("{}:{}", first.min(second), first.max(second));
        let existing: Option<i64> = self
            .conn
            .query_row("SELECT id FROM channels WHERE dm_key = ?1", [&key], |row| {
                row.get(0)
            })
            .optional()?;
        if let Some(id) = existing {
            return Ok(Some(id));
        }
        self.conn.execute(
            "INSERT INTO channels (kind, dm_key, created_by, created_at) VALUES ('dm', ?1, ?2, ?3)",
            params![key, first, self.now],
        )?;
        let id = self.conn.last_insert_rowid();
        store::add_member(self.conn, id, first)?;
        store::add_member(self.conn, id, second)?;
        self.report.conversations = self.report.conversations.saturating_add(1);
        Ok(Some(id))
    }

    fn message(
        &mut self,
        channel_id: i64,
        conversation: &str,
        message: &SlackMessage,
        threads: &mut HashMap<String, i64>,
    ) -> AppResult<()> {
        let import_id = format!("slack:{conversation}:{}", message.ts);
        let known: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM messages WHERE import_id = ?1",
                [&import_id],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(id) = known {
            threads.insert(message.ts.clone(), id);
            self.report.skipped = self.report.skipped.saturating_add(1);
            return Ok(());
        }
        let Some(text) = body(message, &self.names) else {
            return Ok(());
        };
        let user_id = message
            .user
            .as_ref()
            .and_then(|user| self.people.get(user))
            .copied();
        let bot_name = if user_id.is_none() {
            Some(
                message
                    .username
                    .clone()
                    .or_else(|| {
                        message
                            .bot_profile
                            .as_ref()
                            .and_then(|bot| bot.get("name")?.as_str().map(ToOwned::to_owned))
                    })
                    .unwrap_or_else(|| "Slack".to_owned()),
            )
        } else {
            None
        };
        let parent_id = message
            .thread_ts
            .as_ref()
            .filter(|thread| **thread != message.ts)
            .and_then(|thread| threads.get(thread).copied());
        let id = store::insert_message(
            self.conn,
            &NewMessage {
                channel_id,
                parent_id,
                user_id,
                webhook_id: None,
                bot_name: bot_name.as_deref(),
                bot_icon: None,
                automation_id: None,
                body: &text,
                attachments: &[],
                files: &[],
                gif: None,
                poll: &[],
                buttons: &[],
                created_at: millis(&message.ts),
            },
        )?;
        self.conn.execute(
            "UPDATE messages SET import_id = ?1, slack_format = 1 WHERE id = ?2",
            params![import_id, id],
        )?;
        threads.insert(message.ts.clone(), id);
        self.report.messages = self.report.messages.saturating_add(1);
        for reaction in &message.reactions {
            let emoji = reaction.name.split("::").next().unwrap_or_default();
            if !self.emoji.has_emoji(emoji) {
                continue;
            }
            for user in &reaction.users {
                if let Some(user) = self.people.get(user) {
                    store::toggle_reaction(self.conn, id, *user, emoji, millis(&message.ts))?;
                    self.report.reactions = self.report.reactions.saturating_add(1);
                }
            }
        }
        Ok(())
    }
}

fn history<R: Read + Seek>(
    export: &mut Export<R>,
    importer: &mut Importer<'_>,
    folder: &str,
    conversation: &str,
    channel_id: i64,
) -> AppResult<()> {
    let mut messages: Vec<SlackMessage> = Vec::new();
    for day in export.days(folder) {
        let found: Option<Vec<SlackMessage>> = export.json(&day)?;
        messages.extend(found.unwrap_or_default());
    }
    // Oldest first, so thread starts exist before their replies.
    messages.sort_by_key(|message| millis(&message.ts));
    let mut threads = HashMap::new();
    for message in &messages {
        importer.message(channel_id, conversation, message, &mut threads)?;
    }
    // Imported history starts out read.
    importer.conn.execute(
        "INSERT INTO reads (user_id, channel_id, last_read_id)
         SELECT u.id, ?1, (SELECT COALESCE(MAX(id), 0) FROM messages WHERE channel_id = ?1) FROM users u
         WHERE true ON CONFLICT (user_id, channel_id) DO UPDATE SET last_read_id = MAX(last_read_id, excluded.last_read_id)",
        [channel_id],
    )?;
    Ok(())
}

/// Imports a Slack export in one transaction. Blocks.
pub fn slack(conn: &mut Connection, reader: impl Read + Seek, now: i64) -> AppResult<Report> {
    let mut export = Export::open(reader)?;
    let users: Vec<SlackUser> = export.json("users.json")?.unwrap_or_default();
    let channels: Vec<SlackChannel> = export.json("channels.json")?.unwrap_or_default();
    let groups: Vec<SlackChannel> = export.json("groups.json")?.unwrap_or_default();
    let mpims: Vec<SlackChannel> = export.json("mpims.json")?.unwrap_or_default();
    let dms: Vec<SlackChannel> = export.json("dms.json")?.unwrap_or_default();
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let mut importer = Importer {
        conn: &tx,
        now,
        people: HashMap::new(),
        names: HashMap::new(),
        emoji: store::render_context(&tx)?,
        report: Report::default(),
    };
    importer.people(users)?;
    for (list, private) in [(&channels, false), (&groups, true), (&mpims, true)] {
        for channel in list {
            let id = importer.channel(channel, private)?;
            let folder = channel.name.clone().unwrap_or_else(|| channel.id.clone());
            history(&mut export, &mut importer, &folder, &channel.id, id)?;
        }
    }
    for conversation in &dms {
        if let Some(id) = importer.direct(conversation)? {
            history(
                &mut export,
                &mut importer,
                &conversation.id,
                &conversation.id,
                id,
            )?;
        }
    }
    let report = importer.report;
    tx.commit()?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_names_times_and_mentions() {
        assert_eq!(username("Ada.Lovelace"), "ada.lovelace");
        assert_eq!(username("x"), "x_slack");
        assert_eq!(channel_name("Garden Club!"), "garden-club-");
        assert_eq!(millis("1596036000.000100"), 1_596_036_000_000);
        assert_eq!(millis("1596036000.5"), 1_596_036_000_500);
        let names = HashMap::from([("U1".to_owned(), "ada".to_owned())]);
        assert_eq!(
            mentions("hi <@U1> and <@U1|Ada> and <@U9>", &names),
            "hi @ada and @ada and <@U9>"
        );
    }
}
