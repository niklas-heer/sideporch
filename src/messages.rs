//! The one path every new message takes, whether a person, a webhook or an
//! automation sent it: store and index it, show it live, notify people, and
//! let automations react.

use crate::{
    AppState,
    automations::MessageEvent,
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
}

pub struct Draft {
    pub channel_id: i64,
    pub parent_id: Option<i64>,
    pub sender: Sender,
    pub body: String,
    pub attachments: Vec<Attachment>,
    pub files: Vec<i64>,
}

struct Posted {
    message: Message,
    reply_count: Option<i64>,
    audience: Option<Vec<i64>>,
    ctx: markup::Context,
    notify: Vec<i64>,
    automation_event: Option<MessageEvent>,
}

/// Stores `draft` and delivers it. The caller has already checked that the
/// sender may post in the channel.
pub async fn post(state: &AppState, draft: Draft) -> AppResult<Message> {
    let now = now_ms();
    let posted = state
        .db
        .call(move |conn| {
            let tx = conn.transaction()?;
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
            Ok(Posted {
                reply_count,
                audience,
                ctx: store::render_context(conn)?,
                notify: store::notification_targets(conn, &message, user_id)?,
                automation_event,
                message,
            })
        })
        .await?;

    let html = views::message_item(
        &posted.message,
        false,
        posted.message.parent_id.is_none(),
        &views::Render::shared(&posted.ctx),
    )
    .into_string();
    state.hub.publish(
        posted.audience,
        &realtime::Event::Message {
            channel_id: posted.message.channel_id,
            id: posted.message.id,
            parent_id: posted.message.parent_id,
            author: views::author_key(&posted.message.author),
            created_at: posted.message.created_at,
            html,
            reply_count: posted.reply_count,
        },
    );
    push::notify(state, &posted.message, posted.notify);
    if let Some(event) = posted.automation_event {
        state.automations.message(event);
    }
    Ok(posted.message)
}

/// Adds or removes the user's `emoji` reaction and shows the change live.
pub async fn toggle_reaction(
    state: &AppState,
    user_id: i64,
    channel_id: i64,
    message_id: i64,
    emoji: String,
) -> AppResult<()> {
    let now = now_ms();
    let (message, audience, ctx) = state
        .db
        .call(move |conn| {
            store::channel_for(conn, channel_id, user_id)?.ok_or(AppError::NotFound)?;
            let message = store::message(conn, message_id)?
                .filter(|message| message.channel_id == channel_id)
                .ok_or(AppError::NotFound)?;
            let ctx = store::render_context(conn)?;
            if !ctx.has_emoji(&emoji) {
                return Err(AppError::bad_request("That emoji doesn't exist here."));
            }
            store::toggle_reaction(conn, message.id, user_id, &emoji, now)?;
            let message = store::message(conn, message_id)?.ok_or(AppError::NotFound)?;
            Ok((message, store::audience(conn, channel_id)?, ctx))
        })
        .await?;
    state.hub.publish(
        audience,
        &realtime::Event::Reactions {
            channel_id,
            message_id,
            html: views::reactions_bar(&message, &views::Render::shared(&ctx)).into_string(),
        },
    );
    Ok(())
}
