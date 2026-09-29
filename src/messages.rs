//! The one path every new message takes, whether a person, a webhook or an
//! automation sent it: store and index it, show it live, notify people, and
//! let automations react.

use crate::{
    AppState,
    automations::{Event, MessageEvent, ReactionEvent},
    error::{AppError, AppResult},
    markup, now_ms, push, realtime,
    store::{self, Message, NewMessage},
    views,
    webhook::Attachment,
};

/// Who is posting.
pub enum Sender {
    User(i64),
    Webhook {
        id: i64,
        name: String,
        icon: Option<String>,
    },
    Automation {
        id: i64,
        name: String,
    },
    /// A named bot without an account: reminders, and answers to outgoing
    /// webhooks. `icon` is an emoji code or an image URL.
    Bot {
        name: String,
        icon: Option<String>,
    },
}

pub struct Draft {
    pub channel_id: i64,
    pub parent_id: Option<i64>,
    pub sender: Sender,
    pub body: String,
    pub attachments: Vec<Attachment>,
    pub files: Vec<i64>,
    pub gif: Option<store::Gif>,
    /// A poll; the body is the question.
    pub poll: Option<crate::polls::Spec>,
    pub buttons: Vec<store::Button>,
}

struct Posted {
    message: Message,
    reply_count: Option<i64>,
    audience: Option<Vec<i64>>,
    ctx: std::sync::Arc<markup::Context>,
    recipients: store::Recipients,
    automation_event: Option<MessageEvent>,
}

/// Where a message from another server came from.
pub struct Origin {
    /// The server that sent it.
    pub instance_id: i64,
    /// What servers call it: `handle#id` on the server it was written on.
    pub uid: String,
}

/// Stores `draft` and delivers it. The caller has already checked that the
/// sender may post in the channel.
pub async fn post(state: &AppState, draft: Draft) -> AppResult<Message> {
    post_from(state, draft, None).await
}

/// Stores `draft`, from this server or, with `origin`, from another one,
/// delivers it, and passes it on to the servers the channel is shared with.
pub async fn post_from(
    state: &AppState,
    draft: Draft,
    origin: Option<Origin>,
) -> AppResult<Message> {
    let now = now_ms();
    let from = origin.as_ref().map(|origin| origin.instance_id);
    let posted = state
        .db
        .call(move |conn| {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let parent_id = check_draft(&tx, &draft)?;
            let (user_id, webhook_id, automation_id, bot_name, bot_icon) = match &draft.sender {
                Sender::User(id) => (Some(*id), None, None, None, None),
                Sender::Webhook { id, name, icon } => {
                    (None, Some(*id), None, Some(name.as_str()), icon.as_deref())
                }
                Sender::Automation { id, name } => {
                    (None, None, Some(*id), Some(name.as_str()), None)
                }
                Sender::Bot { name, icon } => {
                    (None, None, None, Some(name.as_str()), icon.as_deref())
                }
            };
            let id = store::insert_message(
                &tx,
                &NewMessage {
                    channel_id: draft.channel_id,
                    parent_id,
                    user_id,
                    webhook_id,
                    automation_id,
                    bot_name,
                    bot_icon,
                    body: &draft.body,
                    attachments: &draft.attachments,
                    files: &draft.files,
                    gif: draft.gif.as_ref(),
                    poll: draft.poll.as_ref(),
                    buttons: &draft.buttons,
                    created_at: now,
                },
            )?;
            if let Some(origin) = &origin {
                tx.execute(
                    "UPDATE messages SET remote_uid = ?1 WHERE id = ?2",
                    rusqlite::params![origin.uid, id],
                )?;
            } else if let Some(user_id) = user_id {
                store::mark_read(&tx, user_id, draft.channel_id, id)?;
                // Taking part is how people earn trust.
                crate::community::refresh(&tx, user_id, now)?;
            }
            tx.commit()?;

            let message = store::message(conn, id)?
                .ok_or_else(|| AppError::internal("new message not found"))?;
            let reply_count = match message.parent_id {
                Some(parent) => store::message(conn, parent)?.map(|parent| parent.reply_count),
                None => None,
            };
            let audience = store::audience(conn, draft.channel_id)?;
            let automation_event = match (&draft.sender, &audience) {
                // Automations see public channels only, and never their own posts.
                (Sender::Automation { .. }, _) | (_, Some(_)) => None,
                _ => Some(MessageEvent::new(conn, &message)?),
            };
            let recipients = store::recipients(conn, &message, user_id)?;
            store::record_activity(conn, message.id, &recipients)?;
            Ok(Posted {
                reply_count,
                audience,
                ctx: store::render_context(conn)?,
                recipients,
                automation_event,
                message,
            })
        })
        .await?;

    deliver(state, &posted);
    crate::federation::outbound::posted(state, &posted.message, from).await;
    let Posted {
        message,
        automation_event,
        ..
    } = posted;
    // People's links get previews; bots and webhooks format their own.
    if matches!(message.author, store::Author::User { .. }) && message.body.contains("http") {
        crate::previews::attach(state, &state.links, message.id, &message.body);
    }
    crate::outgoing::dispatch(
        state,
        &message,
        state.public_url.clone().unwrap_or_default(),
    );
    if let Some(event) = automation_event {
        state.automations.event(Event::Message(event));
    }
    Ok(message)
}

/// Checks that `draft` may be posted, and returns the thread it goes in.
fn check_draft(conn: &rusqlite::Connection, draft: &Draft) -> AppResult<Option<i64>> {
    let parent_id = match draft.parent_id {
        Some(parent_id) => {
            let parent = store::message(conn, parent_id)?
                .filter(|parent| parent.channel_id == draft.channel_id)
                .ok_or(AppError::NotFound)?;
            // Threads are one level deep; a reply to a reply joins the root.
            Some(parent.parent_id.unwrap_or(parent.id))
        }
        None => None,
    };
    if draft.poll.is_some() && crate::federation::data::is_shared(conn, draft.channel_id)? {
        return Err(AppError::bad_request(
            "Polls aren't shared with other servers yet, so they can't be started in shared channels.",
        ));
    }
    if let Sender::User(user_id) = draft.sender {
        for file_id in &draft.files {
            if !store::owns_unattached_file(conn, *file_id, user_id)? {
                return Err(AppError::bad_request("That file can't be attached."));
            }
        }
    }
    Ok(parent_id)
}

/// Shows a new message live and sends its push notifications.
fn deliver(state: &AppState, posted: &Posted) {
    let html = views::message_item(
        &posted.message,
        false,
        posted.message.parent_id.is_none(),
        &views::Render::shared(&posted.ctx),
    )
    .into_string();
    state.hub.publish(
        posted.audience.clone(),
        &realtime::Event::Message {
            channel_id: posted.message.channel_id,
            id: posted.message.id,
            parent_id: posted.message.parent_id,
            author: views::author_key(&posted.message.author),
            created_at: posted.message.created_at,
            html,
            reply_count: posted.reply_count,
            activity: posted
                .recipients
                .mentioned
                .iter()
                .chain(&posted.recipients.thread)
                .copied()
                .collect(),
        },
    );
    push::notify(state, &posted.message, posted.recipients.notify.clone());
}

/// Shows the current state of a message to everyone who can see it.
pub async fn refresh(state: &AppState, message_id: i64) -> AppResult<()> {
    let found = state
        .db
        .call(move |conn| {
            let Some(message) = store::message(conn, message_id)? else {
                return Ok(None);
            };
            let audience = store::audience(conn, message.channel_id)?;
            Ok(Some((message, audience, store::render_context(conn)?)))
        })
        .await?;
    if let Some((message, audience, ctx)) = found {
        publish_changed(state, audience, &message, &ctx);
    }
    Ok(())
}

fn publish_changed(
    state: &AppState,
    audience: Option<Vec<i64>>,
    message: &Message,
    ctx: &markup::Context,
) {
    let html = views::message_item(
        message,
        false,
        message.parent_id.is_none(),
        &views::Render::shared(ctx),
    )
    .into_string();
    state.hub.publish(
        audience,
        &realtime::Event::MessageChanged {
            channel_id: message.channel_id,
            id: message.id,
            parent_id: message.parent_id,
            html,
        },
    );
}

/// How long people can edit their messages, in minutes; empty or 0 for no
/// limit, the default.
const EDIT_WINDOW: &str = "messages.edit_minutes";

/// The choices admins have.
pub const EDIT_WINDOWS: &[(i64, &str)] = &[
    (0, "No limit"),
    (15, "15 minutes"),
    (60, "1 hour"),
    (360, "6 hours"),
    (1440, "1 day"),
    (10_080, "1 week"),
];

pub fn edit_window(conn: &rusqlite::Connection) -> AppResult<Option<i64>> {
    Ok(store::setting(conn, EDIT_WINDOW)?
        .and_then(|minutes| minutes.parse::<i64>().ok())
        .filter(|minutes| *minutes > 0))
}

pub fn set_edit_window(conn: &rusqlite::Connection, minutes: i64) -> AppResult<()> {
    if !EDIT_WINDOWS.iter().any(|(choice, _)| *choice == minutes) {
        return Err(AppError::bad_request(
            "Pick how long messages can be edited.",
        ));
    }
    store::set_setting(conn, EDIT_WINDOW, &minutes.to_string())
}

/// Whether a message sent at `created_at` can still be edited at `now`.
pub fn editable(window: Option<i64>, created_at: i64, now: i64) -> bool {
    window.is_none_or(|minutes| now.saturating_sub(created_at) <= minutes.saturating_mul(60_000))
}

/// Who may change a message: its author edits it; its author or a
/// moderator (admins are) deletes it.
pub enum Change {
    Edit(String),
    Delete,
}

/// Edits or deletes a message after checking the user may, shows the
/// change live, and tells automations about public channels.
pub async fn change(
    state: &AppState,
    user: &crate::auth::CurrentUser,
    channel_id: i64,
    message_id: i64,
    change: Change,
) -> AppResult<()> {
    let user_id = user.id;
    let is_admin = user.may(crate::community::Permission::Moderate);
    let now = now_ms();
    let deleting = matches!(change, Change::Delete);
    let (before, after, audience, ctx, reply_count, remote_uid) = state
        .db
        .call(move |conn| {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let message = store::readable_message(&tx, user_id, channel_id, message_id)?
                .filter(|message| !message.deleted)
                .ok_or(AppError::NotFound)?;
            let own = matches!(message.author, store::Author::User { id, .. } if id == user_id);
            let remote_uid: Option<String> = tx.query_row(
                "SELECT remote_uid FROM messages WHERE id = ?1",
                [message_id],
                |row| row.get(0),
            )?;
            match &change {
                Change::Edit(body) => {
                    if !own {
                        return Err(AppError::Forbidden);
                    }
                    let window = edit_window(&tx)?;
                    if !editable(window, message.created_at, now) {
                        let label = EDIT_WINDOWS
                            .iter()
                            .find(|(minutes, _)| Some(*minutes) == window)
                            .map_or("a while", |(_, label)| label);
                        return Err(AppError::bad_request(format!(
                            "Messages can only be edited for {label} after sending."
                        )));
                    }
                    store::edit_message(&tx, message_id, body, now)?;
                }
                Change::Delete => {
                    if !own && !is_admin {
                        return Err(AppError::Forbidden);
                    }
                    store::delete_message(&tx, message_id, now)?;
                }
            }
            tx.commit()?;
            let after = store::message(conn, message_id)?;
            let reply_count = match message.parent_id {
                Some(parent) => store::message(conn, parent)?.map(|parent| parent.reply_count),
                None => None,
            };
            let audience = store::audience(conn, channel_id)?;
            let before = if audience.is_none() {
                Some(MessageEvent::new(conn, &message)?)
            } else {
                None
            };
            Ok((
                (message, before),
                after,
                audience,
                store::render_context(conn)?,
                reply_count,
                remote_uid,
            ))
        })
        .await?;
    crate::federation::outbound::changed(state, channel_id, message_id, remote_uid, None).await;
    let (message, event) = before;
    publish_change(
        state,
        channel_id,
        message_id,
        message.parent_id,
        after.as_ref(),
        audience,
        &ctx,
        reply_count,
    );
    if let Some(mut event) = event {
        if deleting {
            state.automations.event(Event::MessageDeleted(event));
        } else if let Some(after) = after {
            crate::previews::attach(state, &state.links, after.id, &after.body);
            event.text = after.body;
            state.automations.event(Event::MessageChanged(event));
        }
    }
    Ok(())
}

/// Shows an edit, or a deletion when `after` is gone, to everyone who can
/// see the channel.
#[expect(clippy::too_many_arguments, reason = "what a change needs to be shown")]
fn publish_change(
    state: &AppState,
    channel_id: i64,
    message_id: i64,
    parent_id: Option<i64>,
    after: Option<&Message>,
    audience: Option<Vec<i64>>,
    ctx: &markup::Context,
    reply_count: Option<i64>,
) {
    if let Some(after) = after {
        publish_changed(state, audience, after, ctx);
        return;
    }
    state.hub.publish(
        audience.clone(),
        &realtime::Event::MessageDeleted {
            channel_id,
            id: message_id,
            parent_id,
            reply_count: parent_id.map(|_| reply_count.unwrap_or(0)),
        },
    );
    // A deleted thread start goes once its last reply does.
    if let (Some(parent), None) = (parent_id, reply_count) {
        state.hub.publish(
            audience,
            &realtime::Event::MessageDeleted {
                channel_id,
                id: parent,
                parent_id: None,
                reply_count: None,
            },
        );
    }
}

/// Applies an edit or deletion another server made, shows it live, and
/// passes it on to the other servers the channel is shared with. Who may
/// make it was checked by the caller.
pub async fn apply_remote_change(
    state: &AppState,
    channel_id: i64,
    message_id: i64,
    change: Change,
    from: i64,
) -> AppResult<()> {
    let now = now_ms();
    let (parent_id, after, audience, ctx, reply_count, remote_uid) = state
        .db
        .call(move |conn| {
            let message = store::message(conn, message_id)?
                .filter(|message| message.channel_id == channel_id && !message.deleted)
                .ok_or(AppError::NotFound)?;
            let remote_uid: Option<String> = conn.query_row(
                "SELECT remote_uid FROM messages WHERE id = ?1",
                [message_id],
                |row| row.get(0),
            )?;
            match &change {
                Change::Edit(body) => store::edit_message(conn, message_id, body, now)?,
                Change::Delete => store::delete_message(conn, message_id, now)?,
            }
            let reply_count = match message.parent_id {
                Some(parent) => store::message(conn, parent)?.map(|parent| parent.reply_count),
                None => None,
            };
            Ok((
                message.parent_id,
                store::message(conn, message_id)?,
                store::audience(conn, channel_id)?,
                store::render_context(conn)?,
                reply_count,
                remote_uid,
            ))
        })
        .await?;
    publish_change(
        state,
        channel_id,
        message_id,
        parent_id,
        after.as_ref(),
        audience,
        &ctx,
        reply_count,
    );
    crate::federation::outbound::changed(state, channel_id, message_id, remote_uid, Some(from))
        .await;
    Ok(())
}

/// Sets or removes a reaction someone on another server made, shows it
/// live, and passes it on.
pub async fn apply_remote_reaction(
    state: &AppState,
    channel_id: i64,
    message_id: i64,
    user_id: i64,
    emoji: String,
    added: bool,
    from: i64,
) -> AppResult<()> {
    let now = now_ms();
    let reaction = emoji.clone();
    let (message, audience, ctx) = state
        .db
        .call(move |conn| {
            if added {
                conn.execute(
                    "INSERT OR IGNORE INTO reactions (message_id, user_id, emoji, created_at) VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![message_id, user_id, reaction, now],
                )?;
            } else {
                conn.execute(
                    "DELETE FROM reactions WHERE message_id = ?1 AND user_id = ?2 AND emoji = ?3",
                    rusqlite::params![message_id, user_id, reaction],
                )?;
            }
            let message = store::message(conn, message_id)?.ok_or(AppError::NotFound)?;
            Ok((message, store::audience(conn, channel_id)?, store::render_context(conn)?))
        })
        .await?;
    publish_reactions(state, audience, &message, &ctx);
    crate::federation::outbound::reacted(
        state,
        channel_id,
        message_id,
        user_id,
        emoji,
        added,
        Some(from),
    )
    .await;
    Ok(())
}

/// Adds or removes the user's `emoji` reaction, shows the change live, and
/// tells automations about reactions in public channels.
pub async fn toggle_reaction(
    state: &AppState,
    user_id: i64,
    channel_id: i64,
    message_id: i64,
    emoji: String,
) -> AppResult<()> {
    let now = now_ms();
    let (message, audience, ctx, event, reacted) = state
        .db
        .call(move |conn| {
            let channel =
                store::channel_for(conn, channel_id, user_id)?.ok_or(AppError::NotFound)?;
            if !channel.may_react() {
                return Err(AppError::bad_request(format!(
                    "Only the managers of #{} react there.",
                    channel.name
                )));
            }
            let message = store::message(conn, message_id)?
                .filter(|message| message.channel_id == channel_id && !message.deleted)
                .ok_or(AppError::NotFound)?;
            let ctx = store::render_context(conn)?;
            if !ctx.has_emoji(&emoji) {
                return Err(AppError::bad_request("That emoji doesn't exist here."));
            }
            let added = store::toggle_reaction(conn, message.id, user_id, &emoji, now)?;
            let reacted = (emoji.clone(), added);
            let message = store::message(conn, message_id)?.ok_or(AppError::NotFound)?;
            let audience = store::audience(conn, channel_id)?;
            // Automations see public channels only.
            let event = if audience.is_none() {
                let (user, username) = conn.query_row(
                    "SELECT display_name, username FROM users WHERE id = ?1",
                    [user_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?;
                Some(ReactionEvent {
                    emoji,
                    added,
                    user,
                    username,
                    message: MessageEvent::new(conn, &message)?,
                })
            } else {
                None
            };
            Ok((message, audience, ctx, event, reacted))
        })
        .await?;
    publish_reactions(state, audience, &message, &ctx);
    let (emoji, added) = reacted;
    crate::federation::outbound::reacted(
        state, channel_id, message_id, user_id, emoji, added, None,
    )
    .await;
    if let Some(event) = event {
        state.automations.event(Event::Reaction(event));
    }
    Ok(())
}

/// Adds an automation's reaction to a message in a public channel.
/// Automations' reactions never trigger automations.
pub async fn automation_reaction(
    state: &AppState,
    automation_id: i64,
    channel_id: i64,
    message_id: i64,
    emoji: String,
) -> AppResult<()> {
    let now = now_ms();
    let changed = state
        .db
        .call(move |conn| {
            store::public_channel(conn, channel_id)?.ok_or_else(|| {
                AppError::bad_request("sideporch.react: the message is not in a public channel")
            })?;
            store::message(conn, message_id)?
                .filter(|message| message.channel_id == channel_id)
                .ok_or_else(|| AppError::bad_request("sideporch.react: no such message"))?;
            let ctx = store::render_context(conn)?;
            if !ctx.has_emoji(&emoji) {
                return Err(AppError::bad_request(format!(
                    "sideporch.react: there is no emoji named {emoji}"
                )));
            }
            if !store::add_automation_reaction(conn, message_id, automation_id, &emoji, now)? {
                return Ok(None);
            }
            let message = store::message(conn, message_id)?.ok_or(AppError::NotFound)?;
            Ok(Some((message, store::audience(conn, channel_id)?, ctx)))
        })
        .await?;
    if let Some((message, audience, ctx)) = changed {
        publish_reactions(state, audience, &message, &ctx);
    }
    Ok(())
}

fn publish_reactions(
    state: &AppState,
    audience: Option<Vec<i64>>,
    message: &Message,
    ctx: &markup::Context,
) {
    state.hub.publish(
        audience,
        &realtime::Event::Reactions {
            channel_id: message.channel_id,
            message_id: message.id,
            html: views::reactions_bar(message, &views::Render::shared(ctx)).into_string(),
        },
    );
}
