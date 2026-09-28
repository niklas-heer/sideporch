//! What happens in Sideporch that scripts can react to.

use rusqlite::Connection;
use serde::Serialize;

use crate::{
    error::AppResult,
    store::{Author, Message},
};

/// The events `sideporch.on` accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Message,
    ReactionAdded,
    ReactionRemoved,
    MemberJoined,
    ChannelCreated,
}

impl EventKind {
    pub const ALL: [Self; 5] = [
        Self::Message,
        Self::ReactionAdded,
        Self::ReactionRemoved,
        Self::MemberJoined,
        Self::ChannelCreated,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Message => "message",
            Self::ReactionAdded => "reaction_added",
            Self::ReactionRemoved => "reaction_removed",
            Self::MemberJoined => "member_joined",
            Self::ChannelCreated => "channel_created",
        }
    }

    /// The kinds a name stands for; `reaction` means added and removed.
    pub fn parse(name: &str) -> Option<Vec<Self>> {
        if name == "reaction" {
            return Some(vec![Self::ReactionAdded, Self::ReactionRemoved]);
        }
        Self::ALL
            .into_iter()
            .find(|kind| kind.name() == name)
            .map(|kind| vec![kind])
    }
}

/// A new message, as scripts see it.
#[derive(Debug, Clone)]
pub struct MessageEvent {
    pub id: i64,
    pub channel_id: i64,
    pub channel: String,
    pub text: String,
    pub author: String,
    pub username: Option<String>,
    pub is_bot: bool,
    pub thread_id: Option<i64>,
}

impl MessageEvent {
    pub fn new(conn: &Connection, message: &Message) -> AppResult<Self> {
        let channel: Option<String> = conn.query_row(
            "SELECT name FROM channels WHERE id = ?1",
            [message.channel_id],
            |row| row.get(0),
        )?;
        let (author, username, is_bot) = match &message.author {
            Author::User { id, display_name } => {
                let username: String =
                    conn.query_row("SELECT username FROM users WHERE id = ?1", [id], |row| {
                        row.get(0)
                    })?;
                (display_name.clone(), Some(username), false)
            }
            Author::Bot { name, .. } => (name.clone(), None, true),
            Author::Removed => ("Former member".to_owned(), None, false),
        };
        Ok(Self {
            id: message.id,
            channel_id: message.channel_id,
            channel: channel.unwrap_or_default(),
            text: message.body.clone(),
            author,
            username,
            is_bot,
            thread_id: message.parent_id,
        })
    }
}

/// Someone added or removed a reaction.
#[derive(Debug, Clone)]
pub struct ReactionEvent {
    pub emoji: String,
    pub added: bool,
    pub user: String,
    pub username: String,
    pub message: MessageEvent,
}

/// Someone created an account.
#[derive(Debug, Clone)]
pub struct MemberEvent {
    pub user: String,
    pub username: String,
}

/// Someone created a public channel.
#[derive(Debug, Clone)]
pub struct ChannelEvent {
    pub channel: String,
    pub channel_id: i64,
    pub user: String,
    pub username: String,
}

#[derive(Debug, Clone)]
pub enum Event {
    Message(MessageEvent),
    Reaction(ReactionEvent),
    MemberJoined(MemberEvent),
    ChannelCreated(ChannelEvent),
}

impl Event {
    pub const fn kind(&self) -> EventKind {
        match self {
            Self::Message(_) => EventKind::Message,
            Self::Reaction(event) if event.added => EventKind::ReactionAdded,
            Self::Reaction(_) => EventKind::ReactionRemoved,
            Self::MemberJoined(_) => EventKind::MemberJoined,
            Self::ChannelCreated(_) => EventKind::ChannelCreated,
        }
    }

    /// The channel it happened in, without `#`.
    pub fn channel(&self) -> Option<&str> {
        match self {
            Self::Message(message) => Some(&message.channel),
            Self::Reaction(reaction) => Some(&reaction.message.channel),
            Self::ChannelCreated(channel) => Some(&channel.channel),
            Self::MemberJoined(_) => None,
        }
    }

    /// The username of whoever caused it.
    pub fn username(&self) -> Option<&str> {
        match self {
            Self::Message(message) => message.username.as_deref(),
            Self::Reaction(reaction) => Some(&reaction.username),
            Self::MemberJoined(member) => Some(&member.username),
            Self::ChannelCreated(channel) => Some(&channel.username),
        }
    }

    /// The text patterns are matched against.
    pub fn text(&self) -> Option<&str> {
        match self {
            Self::Message(message) => Some(&message.text),
            Self::Reaction(reaction) => Some(&reaction.message.text),
            Self::MemberJoined(_) | Self::ChannelCreated(_) => None,
        }
    }

    pub fn emoji(&self) -> Option<&str> {
        match self {
            Self::Reaction(reaction) => Some(&reaction.emoji),
            _ => None,
        }
    }

    /// Whether it concerns a reply in a thread.
    pub const fn in_thread(&self) -> Option<bool> {
        match self {
            Self::Message(message) => Some(message.thread_id.is_some()),
            Self::Reaction(reaction) => Some(reaction.message.thread_id.is_some()),
            Self::MemberJoined(_) | Self::ChannelCreated(_) => None,
        }
    }
}

/// An HTTP request to an automation's webhook URL.
#[derive(Debug, Clone, Default)]
pub struct WebhookRequest {
    pub method: String,
    /// The part of the path after the token, such as `/deploy`, or empty.
    pub path: String,
    pub query: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

/// Someone ran a slash command, such as `/deploy garden`.
#[derive(Debug, Clone, Default)]
pub struct CommandCall {
    pub name: String,
    /// Everything after the command name, trimmed.
    pub text: String,
    pub user: String,
    pub username: String,
    pub channel: String,
    pub channel_id: i64,
    pub thread_id: Option<i64>,
}

impl CommandCall {
    /// Parses `/name rest`, if `text` looks like a command.
    pub fn parse(text: &str) -> Option<(String, String)> {
        let rest = text.trim_start().strip_prefix('/')?;
        let (name, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        let name = name.to_ascii_lowercase();
        valid_command_name(&name).then(|| (name, args.trim().to_owned()))
    }
}

/// Command names: a letter, then letters, digits, `-` and `_`, up to 32.
pub fn valid_command_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && name.len() <= 32
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_commands() {
        assert_eq!(
            CommandCall::parse("/Deploy  garden now "),
            Some(("deploy".to_owned(), "garden now".to_owned()))
        );
        assert_eq!(
            CommandCall::parse("/help"),
            Some(("help".to_owned(), String::new()))
        );
        assert_eq!(CommandCall::parse("/etc/hosts is broken"), None);
        assert_eq!(CommandCall::parse("no command"), None);
        assert_eq!(CommandCall::parse("/2fa"), None);
    }

    #[test]
    fn names_events() {
        assert_eq!(EventKind::parse("reaction").unwrap().len(), 2);
        assert_eq!(
            EventKind::parse("member_joined"),
            Some(vec![EventKind::MemberJoined])
        );
        assert_eq!(EventKind::parse("nope"), None);
    }
}
