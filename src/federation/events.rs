//! What servers tell each other about shared channels.
//!
//! Events always name a channel by its id on the host. Messages are named
//! by a uid, `handle#id`, where `handle` is the server they were written on.
//! A plain `@name` in a message means someone on the server that sent the
//! event; see [`localize`].

use serde::{Deserialize, Serialize};

/// Someone on a Sideporch server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Person {
    /// Their server's handle, like `chat.example.org`.
    pub server: String,
    /// Their server's URL.
    pub url: String,
    /// Their id on their server.
    pub id: i64,
    /// Their username on their server, without the server.
    pub username: String,
    pub display_name: String,
}

/// A file attached to a message, to fetch from the server that sent the
/// event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRef {
    pub id: i64,
    pub name: String,
    pub mime: String,
    pub size: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// The host offers a channel. `direct` makes it a conversation between
    /// two people, which opens without an admin.
    Share {
        channel: i64,
        name: String,
        topic: String,
        private: bool,
        direct: Option<Direct>,
    },
    /// A guest took the channel; `copy` is its channel there.
    ShareAccepted {
        channel: i64,
        copy: i64,
    },
    ShareDeclined {
        channel: i64,
    },
    /// The channel isn't shared between the two servers any more.
    ShareEnded {
        channel: i64,
    },
    Message {
        channel: i64,
        uid: String,
        parent: Option<String>,
        /// A person wrote it; otherwise `bot` names the bot or automation.
        author: Option<Person>,
        bot: Option<String>,
        body: String,
        /// Written in Slack's format by a webhook.
        #[serde(default)]
        slack_format: bool,
        /// Slack-style attachments from a webhook.
        #[serde(default)]
        attachments: Vec<serde_json::Value>,
        files: Vec<FileRef>,
        created_at: i64,
    },
    Edited {
        channel: i64,
        uid: String,
        body: String,
    },
    Deleted {
        channel: i64,
        uid: String,
    },
    Reaction {
        channel: i64,
        uid: String,
        person: Person,
        emoji: String,
        added: bool,
    },
    Joined {
        channel: i64,
        person: Person,
    },
    Left {
        channel: i64,
        person: Person,
    },
}

/// A direct conversation's two people.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Direct {
    /// Who started it, on the host.
    pub from: Person,
    /// Whom it's with: their id on the receiving server.
    pub to: i64,
}

/// What servers send each other in one request.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Batch {
    pub events: Vec<Event>,
}

/// Rewrites mentions in `text` from the sender's point of view to ours:
/// `@name` (someone on `sender`) becomes `@name@sender`, and
/// `@name@own` (someone here) becomes `@name`. `@here` and `@channel` stay.
pub fn localize(text: &str, sender: &str, own: &str) -> String {
    rewrite(text, |name, server| match server {
        None if matches!(name, "here" | "channel") => None,
        None => Some(format!("@{name}@{sender}")),
        Some(server) if server.eq_ignore_ascii_case(own) => Some(format!("@{name}")),
        Some(_) => None,
    })
}

/// Calls `change` with each `@name` or `@name@server` in `text` outside
/// code, and puts what it returns in its place.
fn rewrite(text: &str, change: impl Fn(&str, Option<&str>) -> Option<String>) -> String {
    let name_char = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.');
    let server_char = |c: char| name_char(c) || c == ':';
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let mut previous: Option<char> = None;
    while let Some(c) = rest.chars().next() {
        // Code, inline or fenced, runs to the next run of as many backticks.
        if c == '`' {
            let ticks = rest
                .len()
                .saturating_sub(rest.trim_start_matches('`').len());
            let fence = rest.get(..ticks).unwrap_or("`");
            let after = rest.get(ticks..).unwrap_or("");
            let end = after.find(fence).map_or(rest.len(), |at| {
                ticks.saturating_add(at).saturating_add(ticks)
            });
            out.push_str(rest.get(..end).unwrap_or(rest));
            previous = Some('`');
            rest = rest.get(end..).unwrap_or("");
            continue;
        }
        let at_word_start = !previous.is_some_and(name_char);
        if c == '@' && at_word_start {
            let tail = rest.get(1..).unwrap_or("");
            let end = tail.find(|c: char| !name_char(c)).unwrap_or(tail.len());
            let name = tail.get(..end).unwrap_or("").trim_end_matches('.');
            if !name.is_empty() {
                let after = tail.get(name.len()..).unwrap_or("");
                let server = after
                    .strip_prefix('@')
                    .map(|server| {
                        let end = server
                            .find(|c: char| !server_char(c))
                            .unwrap_or(server.len());
                        server.get(..end).unwrap_or("").trim_end_matches(['.', ':'])
                    })
                    .filter(|server| !server.is_empty());
                let length = 1_usize
                    .saturating_add(name.len())
                    .saturating_add(server.map_or(0, |server| server.len().saturating_add(1)));
                let original = rest.get(..length).unwrap_or(rest);
                out.push_str(&change(name, server).unwrap_or_else(|| original.to_owned()));
                previous = original.chars().last();
                rest = rest.get(length..).unwrap_or("");
                continue;
            }
        }
        out.push(c);
        previous = Some(c);
        rest = rest.get(c.len_utf8()..).unwrap_or("");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mentions_are_rewritten_for_the_receiving_server() {
        let text = "@ada, ask @bea@b.example and @cy@c.example. @here! mail@x.y";
        assert_eq!(
            localize(text, "a.example", "b.example"),
            "@ada@a.example, ask @bea and @cy@c.example. @here! mail@x.y"
        );
        // Code stays as written.
        assert_eq!(
            localize("`@ada` 🎉 @ada\n```\n@ada\n```", "a.example", "b.example"),
            "`@ada` 🎉 @ada@a.example\n```\n@ada\n```"
        );
    }

    #[test]
    fn events_have_a_stable_form() {
        let event = Event::Deleted {
            channel: 3,
            uid: "a.example#7".to_owned(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(
            json,
            r#"{"type":"deleted","channel":3,"uid":"a.example#7"}"#
        );
        assert_eq!(serde_json::from_str::<Event>(&json).unwrap(), event);
    }
}
