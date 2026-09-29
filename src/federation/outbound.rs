//! Telling other servers what happened in shared channels.
//!
//! The host of a channel passes every event on to all its guests, except
//! the one it came from. A guest tells only the host, and only about what
//! happened on the guest itself.

use rusqlite::Connection;

use super::{
    data::{self, Own, Role, ShareStatus},
    events::{Event, FileRef},
};
use crate::{
    AppState,
    error::AppResult,
    now_ms,
    store::{self, Author, Message},
};

/// Where an event about `channel_id` goes: each server, and the host's id
/// for the channel. `from` is the server the event came from, if any.
pub fn targets(
    conn: &Connection,
    channel_id: i64,
    from: Option<i64>,
) -> AppResult<Vec<(i64, i64)>> {
    let mut targets = Vec::new();
    for share in data::shares_of(conn, channel_id)? {
        if share.status != ShareStatus::Active {
            continue;
        }
        match share.role {
            Role::Guest => {
                if let (None, Some(host_channel)) = (from, share.remote_channel_id) {
                    targets.push((share.instance_id, host_channel));
                }
            }
            Role::Host => {
                if from != Some(share.instance_id) {
                    targets.push((share.instance_id, channel_id));
                }
            }
        }
    }
    Ok(targets)
}

/// The uid other servers know a message by.
pub fn uid_of(conn: &Connection, own: &Own<'_>, message_id: i64) -> AppResult<String> {
    let remote: Option<String> = conn.query_row(
        "SELECT remote_uid FROM messages WHERE id = ?1",
        [message_id],
        |row| row.get(0),
    )?;
    Ok(remote.unwrap_or_else(|| format!("{}#{message_id}", own.handle)))
}

/// The message here that other servers call `uid`, in `channel_id`.
pub fn resolve(
    conn: &Connection,
    own: &Own<'_>,
    channel_id: i64,
    uid: &str,
) -> AppResult<Option<i64>> {
    use rusqlite::OptionalExtension as _;
    if let Some(id) = uid
        .strip_prefix(own.handle)
        .and_then(|rest| rest.strip_prefix('#'))
        .and_then(|id| id.parse::<i64>().ok())
    {
        return Ok(conn
            .query_row(
                "SELECT id FROM messages WHERE id = ?1 AND channel_id = ?2 AND remote_uid IS NULL",
                [id, channel_id],
                |row| row.get(0),
            )
            .optional()?);
    }
    Ok(conn
        .query_row(
            "SELECT id FROM messages WHERE channel_id = ?1 AND remote_uid = ?2",
            rusqlite::params![channel_id, uid],
            |row| row.get(0),
        )
        .optional()?)
}

/// A message as an event naming `channel` (the host's id).
pub fn message_event(
    conn: &Connection,
    own: &Own<'_>,
    message: &Message,
    channel: i64,
) -> AppResult<Event> {
    let (author, bot) = match &message.author {
        Author::User { id, .. } => (data::person(conn, own, *id)?, None),
        Author::Bot { name, .. } => (None, Some(name.clone())),
        Author::Removed => (None, Some("Someone".to_owned())),
    };
    let parent = match message.parent_id {
        Some(parent) => Some(uid_of(conn, own, parent)?),
        None => None,
    };
    Ok(Event::Message {
        channel,
        uid: uid_of(conn, own, message.id)?,
        parent,
        author,
        bot,
        body: message.body.clone(),
        slack_format: message.slack_format,
        attachments: message
            .attachments
            .iter()
            .filter_map(|attachment| serde_json::to_value(attachment).ok())
            .collect(),
        files: message
            .files
            .iter()
            .map(|file| FileRef {
                id: file.id,
                name: file.name.clone(),
                mime: file.mime.clone(),
                size: file.size,
            })
            .collect(),
        created_at: message.created_at,
    })
}

/// Queues `make(channel)` for every server that should hear about
/// something in `channel_id`, and wakes the outbox.
pub async fn tell(
    state: &AppState,
    channel_id: i64,
    from: Option<i64>,
    make: impl Fn(&Connection, &Own<'_>, i64) -> AppResult<Option<Event>> + Send + 'static,
) -> AppResult<()> {
    let (Some(handle), Some(url)) = (
        state.federation.handle(),
        state.federation.url().map(ToOwned::to_owned),
    ) else {
        return Ok(());
    };
    let now = now_ms();
    let queued = state
        .db
        .call(move |conn| {
            let targets = targets(conn, channel_id, from)?;
            if targets.is_empty() {
                return Ok(false);
            }
            let own = Own {
                handle: &handle,
                url: &url,
            };
            for (instance, channel) in targets {
                if let Some(event) = make(conn, &own, channel)? {
                    data::enqueue(conn, instance, &event, now)?;
                }
            }
            Ok(true)
        })
        .await?;
    if queued {
        state.federation.wake.notify_one();
    }
    Ok(())
}

/// After a message was posted in a channel, here or on `from`.
pub async fn posted(state: &AppState, message: &Message, from: Option<i64>) {
    let message = message.clone();
    let channel_id = message.channel_id;
    if let Err(error) = tell(state, channel_id, from, move |conn, own, channel| {
        message_event(conn, own, &message, channel).map(Some)
    })
    .await
    {
        tracing::warn!(%error, "could not pass a message on to other servers");
    }
}

/// After a message was edited or deleted, here or on `from`. A deleted
/// message may be gone, so `remote_uid` is what it had before.
pub async fn changed(
    state: &AppState,
    channel_id: i64,
    message_id: i64,
    remote_uid: Option<String>,
    from: Option<i64>,
) {
    if let Err(error) = tell(state, channel_id, from, move |conn, own, channel| {
        let uid = remote_uid
            .clone()
            .unwrap_or_else(|| format!("{}#{message_id}", own.handle));
        Ok(Some(match store::message(conn, message_id)? {
            Some(message) if !message.deleted => Event::Edited {
                channel,
                uid,
                body: message.body,
            },
            _ => Event::Deleted { channel, uid },
        }))
    })
    .await
    {
        tracing::warn!(%error, "could not pass a change on to other servers");
    }
}

/// After `user_id` added or removed a reaction, here or on `from`.
pub async fn reacted(
    state: &AppState,
    channel_id: i64,
    message_id: i64,
    user_id: i64,
    emoji: String,
    added: bool,
    from: Option<i64>,
) {
    if let Err(error) = tell(state, channel_id, from, move |conn, own, channel| {
        let Some(person) = data::person(conn, own, user_id)? else {
            return Ok(None);
        };
        Ok(Some(Event::Reaction {
            channel,
            uid: uid_of(conn, own, message_id)?,
            person,
            emoji: emoji.clone(),
            added,
        }))
    })
    .await
    {
        tracing::warn!(%error, "could not pass a reaction on to other servers");
    }
}

/// After someone joined or left a private shared channel, here or on
/// `from`.
pub async fn membership(
    state: &AppState,
    channel_id: i64,
    user_id: i64,
    joined: bool,
    from: Option<i64>,
) {
    if let Err(error) = tell(state, channel_id, from, move |conn, own, channel| {
        let Some(person) = data::person(conn, own, user_id)? else {
            return Ok(None);
        };
        Ok(Some(if joined {
            Event::Joined { channel, person }
        } else {
            Event::Left { channel, person }
        }))
    })
    .await
    {
        tracing::warn!(%error, "could not pass membership on to other servers");
    }
}

/// Everything a guest needs when it takes a channel: its members, if
/// private, and the recent conversation.
pub fn welcome(
    conn: &Connection,
    own: &Own<'_>,
    channel_id: i64,
    private: bool,
) -> AppResult<Vec<Event>> {
    let mut events = Vec::new();
    if private {
        for member in store::members(conn, channel_id)? {
            if let Some(person) = data::person(conn, own, member.id)? {
                events.push(Event::Joined {
                    channel: channel_id,
                    person,
                });
            }
        }
    }
    // The newest 50 conversations, each with its thread, oldest first.
    let mut statement = conn.prepare(
        "SELECT id FROM (SELECT id FROM messages WHERE channel_id = ?1 AND parent_id IS NULL
             AND deleted_at IS NULL ORDER BY id DESC LIMIT 50) ORDER BY id",
    )?;
    let roots: Vec<i64> = statement
        .query_map([channel_id], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    for root in roots {
        let mut thread = vec![root];
        let mut replies = conn.prepare(
            "SELECT id FROM messages WHERE parent_id = ?1 AND deleted_at IS NULL ORDER BY id",
        )?;
        for reply in replies.query_map([root], |row| row.get::<_, i64>(0))? {
            thread.push(reply?);
        }
        for id in thread {
            if let Some(message) = store::message(conn, id)? {
                events.push(message_event(conn, own, &message, channel_id)?);
            }
        }
    }
    Ok(events)
}
