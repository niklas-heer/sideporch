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
    /// A poll's options; the body is the question.
    pub poll: Vec<String>,
    pub buttons: Vec<store::Button>,
}

struct Posted {
    message: Message,
    reply_count: Option<i64>,
    audience: Option<Vec<i64>>,
    ctx: markup::Context,
    recipients: store::Recipients,
    automation_event: Option<MessageEvent>,
}

/// Stores `draft` and delivers it. The caller has already checked that the
/// sender may post in the channel.
pub async fn post(state: &AppState, draft: Draft) -> AppResult<Message> {
    let now = now_ms();
    let posted = state
        .db
        .call(move |conn| {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let parent_id = match draft.parent_id {
                Some(parent_id) => {
                    let parent = store::message(&tx, parent_id)?
                        .filter(|parent| parent.channel_id == draft.channel_id)
                        .ok_or(AppError::NotFound)?;
                    // Threads are one level deep; a reply to a reply joins the root.
                    Some(parent.parent_id.unwrap_or(parent.id))
                }
                None => None,
            };
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
            if let Some(user_id) = user_id {
                for file_id in &draft.files {
                    if !store::owns_unattached_file(&tx, *file_id, user_id)? {
                        return Err(AppError::bad_request("That file can't be attached."));
                    }
                }
            }
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
                    poll: &draft.poll,
                    buttons: &draft.buttons,
                    created_at: now,
                },
            )?;
            if let Some(user_id) = user_id {
                store::mark_read(&tx, user_id, draft.channel_id, id)?;
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

/// Who may change a message: its author edits it; its author or an admin
/// deletes it.
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
    let is_admin = user.is_admin;
    let now = now_ms();
    let deleting = matches!(change, Change::Delete);
    let (before, after, audience, ctx, reply_count) = state
        .db
        .call(move |conn| {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let message = store::readable_message(&tx, user_id, channel_id, message_id)?
                .filter(|message| !message.deleted)
                .ok_or(AppError::NotFound)?;
            let own = matches!(message.author, store::Author::User { id, .. } if id == user_id);
            match &change {
                Change::Edit(body) => {
                    if !own {
                        return Err(AppError::Forbidden);
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
            ))
        })
        .await?;
    let (message, event) = before;
    if let Some(after) = &after {
        publish_changed(state, audience, after, &ctx);
    } else {
        {
            state.hub.publish(
                audience.clone(),
                &realtime::Event::MessageDeleted {
                    channel_id,
                    id: message_id,
                    parent_id: message.parent_id,
                    reply_count: message.parent_id.map(|_| reply_count.unwrap_or(0)),
                },
            );
            // A deleted thread start goes once its last reply does.
            if let (Some(parent), None) = (message.parent_id, reply_count) {
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
    }
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
    let (message, audience, ctx, event) = state
        .db
        .call(move |conn| {
            store::channel_for(conn, channel_id, user_id)?.ok_or(AppError::NotFound)?;
            let message = store::message(conn, message_id)?
                .filter(|message| message.channel_id == channel_id && !message.deleted)
                .ok_or(AppError::NotFound)?;
            let ctx = store::render_context(conn)?;
            if !ctx.has_emoji(&emoji) {
                return Err(AppError::bad_request("That emoji doesn't exist here."));
            }
            let added = store::toggle_reaction(conn, message.id, user_id, &emoji, now)?;
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
            Ok((message, audience, ctx, event))
        })
        .await?;
    publish_reactions(state, audience, &message, &ctx);
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
