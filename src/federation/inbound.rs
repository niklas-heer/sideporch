//! Applying what other servers tell us about shared channels.
//!
//! Every event is checked against who may say it. A guest speaks only for
//! its own people; a host speaks for its people and passes on those of its
//! other guests, but never for ours. Only the host of a channel, or the
//! server someone is on, deletes their messages there.

use rusqlite::{Connection, OptionalExtension as _};

use super::{
    data::{self, Instance, Own, Role, Share, ShareStatus},
    events::{Batch, Event, FileRef, Person, localize},
    outbound,
};
use crate::{
    AppState,
    error::{AppError, AppResult},
    messages::{self, Change, Draft, Origin, Sender},
    now_ms, store,
};

/// Applies a batch from `from` in order. Returns what went wrong with each
/// event that didn't apply; the rest did.
pub async fn receive(state: &AppState, from: &Instance, batch: Batch) -> Vec<String> {
    let mut problems = Vec::new();
    for event in batch.events {
        if let Err(problem) = apply(state, from, event).await {
            tracing::warn!(server = %from.handle, %problem, "ignored an event from another server");
            problems.push(problem);
        }
    }
    problems
}

/// This server as events name it, owned so it can move into closures.
#[derive(Clone)]
struct Here {
    handle: String,
    url: String,
}

impl Here {
    fn own(&self) -> Own<'_> {
        Own {
            handle: &self.handle,
            url: &self.url,
        }
    }
}

fn here(state: &AppState) -> Result<Here, String> {
    Ok(Here {
        handle: state
            .federation
            .handle()
            .ok_or("this server has no public URL")?,
        url: state
            .federation
            .url()
            .ok_or("this server has no public URL")?
            .to_owned(),
    })
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "used as map_err(failed), which passes the error by value"
)]
fn failed(error: AppError) -> String {
    error.to_string()
}

async fn apply(state: &AppState, from: &Instance, event: Event) -> Result<(), String> {
    match event {
        Event::Share {
            channel,
            name,
            topic,
            private,
            direct,
        } => {
            if let Some(direct) = direct {
                open_direct(state, from, channel, direct.from, direct.to).await
            } else {
                let offer = data::Offer {
                    id: 0,
                    instance_id: from.id,
                    remote_channel_id: channel,
                    name: name.chars().take(80).collect(),
                    topic: topic.chars().take(250).collect(),
                    private,
                };
                let now = now_ms();
                state
                    .db
                    .call(move |conn| data::save_offer(conn, &offer, now))
                    .await
                    .map_err(failed)
            }
        }
        Event::ShareAccepted { channel, copy } => accepted(state, from, channel, copy).await,
        Event::ShareDeclined { channel } | Event::ShareEnded { channel } => {
            ended(state, from, channel).await
        }
        Event::Message {
            channel,
            uid,
            parent,
            author,
            bot,
            body,
            slack_format,
            attachments,
            files,
            created_at: _,
        } => {
            let incoming = Incoming {
                channel,
                uid,
                parent,
                author,
                bot,
                body,
                slack_format,
                attachments,
                files,
            };
            message(state, from, incoming).await
        }
        Event::Edited { channel, uid, body } => {
            let here = here(state)?;
            let body = localize(&body, &from.handle, &here.handle);
            change(state, from, channel, &uid, Change::Edit(body)).await
        }
        Event::Deleted { channel, uid } => change(state, from, channel, &uid, Change::Delete).await,
        Event::Reaction {
            channel,
            uid,
            person,
            emoji,
            added,
        } => reaction(state, from, channel, &uid, &person, emoji, added).await,
        Event::Joined { channel, person } => membership(state, from, channel, &person, true).await,
        Event::Left { channel, person } => membership(state, from, channel, &person, false).await,
    }
}

/// Our channel for the host's `channel`, and our side of it, if `from` may
/// talk about it.
fn local_channel(
    conn: &Connection,
    from: &Instance,
    channel: i64,
) -> AppResult<Option<(i64, Role)>> {
    if let Some(share) = data::share(conn, channel, from.id)?
        && share.role == Role::Host
        && share.status == ShareStatus::Active
    {
        return Ok(Some((channel, Role::Host)));
    }
    Ok(data::guest_share(conn, from.id, channel)?
        .filter(|share| share.status == ShareStatus::Active)
        .map(|share| (share.channel_id, Role::Guest)))
}

/// Whether `from` may speak for `person` in a channel where we're `role`.
fn may_speak_for(role: Role, from: &Instance, here: &Here, person: &Person) -> bool {
    match role {
        // A guest speaks for its own people only.
        Role::Host => person.server.eq_ignore_ascii_case(&from.handle),
        // The host passes on others', but never ours.
        Role::Guest => !person.server.eq_ignore_ascii_case(&here.handle),
    }
}

/// A direct conversation someone on `from` started with one of our people.
async fn open_direct(
    state: &AppState,
    from: &Instance,
    channel: i64,
    starter: Person,
    to: i64,
) -> Result<(), String> {
    let here = here(state)?;
    let from = from.clone();
    let now = now_ms();
    let opened = state
        .db
        .call(move |conn| {
            let refuse = |conn: &Connection| {
                data::enqueue(conn, from.id, &Event::ShareDeclined { channel }, now).map(|()| false)
            };
            if !from.allow_direct || !starter.server.eq_ignore_ascii_case(&from.handle) {
                return refuse(conn);
            }
            let local: Option<i64> = conn
                .query_row(
                    "SELECT id FROM users WHERE id = ?1 AND instance_id IS NULL AND deactivated_at IS NULL",
                    [to],
                    |row| row.get(0),
                )
                .optional()?;
            let Some(local) = local else {
                return refuse(conn);
            };
            let Some(stand_in) = data::account(conn, &here.own(), &starter, now)? else {
                return refuse(conn);
            };
            let conversation = store::direct_channel(conn, local, stand_in, now)?;
            // Already shared the other way round: keep that one.
            if data::shares_of(conn, conversation)?
                .iter()
                .any(|share| share.instance_id != from.id || share.role == Role::Host)
            {
                return refuse(conn);
            }
            data::save_share(
                conn,
                &Share {
                    channel_id: conversation,
                    instance_id: from.id,
                    role: Role::Guest,
                    remote_channel_id: Some(channel),
                    status: ShareStatus::Active,
                },
                now,
            )?;
            data::enqueue(
                conn,
                from.id,
                &Event::ShareAccepted {
                    channel,
                    copy: conversation,
                },
                now,
            )?;
            Ok(true)
        })
        .await
        .map_err(failed)?;
    if opened {
        state.federation.wake.notify_one();
    }
    Ok(())
}

/// A guest took a channel we offered: send it the members and the recent
/// conversation.
async fn accepted(
    state: &AppState,
    from: &Instance,
    channel: i64,
    copy: i64,
) -> Result<(), String> {
    let here = here(state)?;
    let from_id = from.id;
    let now = now_ms();
    state
        .db
        .call(move |conn| {
            let Some(share) = data::share(conn, channel, from_id)? else {
                return Err(AppError::NotFound);
            };
            if share.role != Role::Host || share.status == ShareStatus::Ended {
                return Err(AppError::bad_request("the channel wasn't offered"));
            }
            data::save_share(
                conn,
                &Share {
                    remote_channel_id: Some(copy),
                    status: ShareStatus::Active,
                    ..share
                },
                now,
            )?;
            // A direct conversation's two people are known already.
            let direct: bool = conn.query_row(
                "SELECT kind = 'dm' FROM channels WHERE id = ?1",
                [channel],
                |row| row.get(0),
            )?;
            let private = !direct && store::audience(conn, channel)?.is_some();
            for event in outbound::welcome(conn, &here.own(), channel, private)? {
                data::enqueue(conn, from_id, &event, now)?;
            }
            Ok(())
        })
        .await
        .map_err(failed)?;
    state.federation.wake.notify_one();
    Ok(())
}

/// The channel isn't shared with `from` any more, either way round.
async fn ended(state: &AppState, from: &Instance, channel: i64) -> Result<(), String> {
    let from_id = from.id;
    let now = now_ms();
    state
        .db
        .call(move |conn| {
            data::drop_offer(conn, from_id, channel)?;
            let share = match data::share(conn, channel, from_id)? {
                Some(share) if share.role == Role::Host => Some(share),
                _ => data::guest_share(conn, from_id, channel)?,
            };
            if let Some(share) = share {
                data::save_share(
                    conn,
                    &Share {
                        status: ShareStatus::Ended,
                        ..share
                    },
                    now,
                )?;
            }
            Ok(())
        })
        .await
        .map_err(failed)
}

/// A message event's contents.
struct Incoming {
    channel: i64,
    uid: String,
    parent: Option<String>,
    author: Option<Person>,
    bot: Option<String>,
    body: String,
    slack_format: bool,
    attachments: Vec<serde_json::Value>,
    files: Vec<FileRef>,
}

/// Where a message goes and who wrote it, once checked.
struct Placed {
    channel: i64,
    parent: Option<i64>,
    author: Option<i64>,
}

/// The account here that wrote a message in `local`, or why it may not.
fn author_here(
    conn: &rusqlite::Connection,
    role: Role,
    from: &Instance,
    here: &Here,
    local: i64,
    person: &Person,
    now: i64,
) -> AppResult<Result<i64, &'static str>> {
    if !may_speak_for(role, from, here, person) {
        return Ok(Err("the server may not speak for that person"));
    }
    let Some(account) = data::account(conn, &here.own(), person, now)? else {
        return Ok(Err("that person can't post here"));
    };
    if let Some(members) = store::audience(conn, local)?
        && !members.contains(&account)
    {
        // The host keeps membership; a guest follows it.
        if role == Role::Host {
            return Ok(Err("that person isn't in the channel"));
        }
        store::add_member(conn, local, account)?;
    }
    Ok(Ok(account))
}

async fn message(state: &AppState, from: &Instance, incoming: Incoming) -> Result<(), String> {
    let here = here(state)?;
    let checked = {
        let (from, here) = (from.clone(), here.clone());
        let (channel, uid, parent, author) = (
            incoming.channel,
            incoming.uid.clone(),
            incoming.parent.clone(),
            incoming.author.clone(),
        );
        let now = now_ms();
        state
            .db
            .call(move |conn| {
                let Some((local, role)) = local_channel(conn, &from, channel)? else {
                    return Ok(Err("the channel isn't shared with this server"));
                };
                if outbound::resolve(conn, &here.own(), local, &uid)?.is_some() {
                    // Already here: servers may send an event twice.
                    return Ok(Ok(None));
                }
                if uid.starts_with(&format!("{}#", here.handle)) {
                    return Ok(Err("a message can't come back to where it was written"));
                }
                let author = match &author {
                    Some(person) => {
                        match author_here(conn, role, &from, &here, local, person, now)? {
                            Ok(account) => Some(account),
                            Err(refusal) => return Ok(Err(refusal)),
                        }
                    }
                    None => None,
                };
                let parent = match &parent {
                    Some(parent) => match outbound::resolve(conn, &here.own(), local, parent)? {
                        Some(id) => Some(id),
                        None => return Ok(Err("the thread isn't here")),
                    },
                    None => None,
                };
                Ok(Ok(Some(Placed {
                    channel: local,
                    parent,
                    author,
                })))
            })
            .await
            .map_err(failed)?
    };
    let Some(placed) = checked? else {
        return Ok(());
    };
    let files = match placed.author {
        Some(author) => fetch_files(state, from, author, &incoming.files).await,
        None => Vec::new(),
    };
    let sender = match (placed.author, &incoming.bot) {
        (Some(author), _) => Sender::User(author),
        (None, Some(bot)) => Sender::Bot {
            name: bot.chars().take(80).collect(),
            icon: None,
        },
        (None, None) => return Err("the message has no author".to_owned()),
    };
    let body: String = localize(&incoming.body, &from.handle, &here.handle)
        .chars()
        .take(10_000)
        .collect();
    let attachments = incoming
        .attachments
        .into_iter()
        .filter_map(|attachment| serde_json::from_value(attachment).ok())
        .take(20)
        .collect();
    let posted = messages::post_from(
        state,
        Draft {
            channel_id: placed.channel,
            parent_id: placed.parent,
            sender,
            body,
            attachments,
            files,
            gif: None,
            poll: None,
            buttons: Vec::new(),
        },
        Some(Origin {
            instance_id: from.id,
            uid: incoming.uid,
        }),
    )
    .await
    .map_err(failed)?;
    if incoming.slack_format {
        slack_formatted(state, posted.id).await.map_err(failed)?;
    }
    Ok(())
}

/// Marks message `id` as written in Slack's mrkdwn, like the webhook that
/// wrote it on the other server, and shows it again.
async fn slack_formatted(state: &AppState, id: i64) -> AppResult<()> {
    state
        .db
        .call(move |conn| {
            conn.execute("UPDATE messages SET slack_format = 1 WHERE id = ?1", [id])?;
            Ok(())
        })
        .await?;
    messages::refresh(state, id).await
}

/// Fetches a message's files from `from`, which has them, and keeps them
/// here as `author`'s. Files that can't be fetched are left out.
async fn fetch_files(
    state: &AppState,
    from: &Instance,
    author: i64,
    files: &[FileRef],
) -> Vec<i64> {
    let mut ids = Vec::new();
    for file in files.iter().take(crate::files::MAX_FILES) {
        let fetched = state
            .federation
            .send_limited(
                "GET",
                &from.url,
                &format!("/federation/files/{}", file.id),
                Vec::new(),
                crate::files::MAX_FILE_BYTES,
            )
            .await;
        let data = match fetched {
            Ok((200, data)) => data,
            Ok((status, _)) => {
                tracing::warn!(server = %from.handle, status, "could not fetch a shared file");
                continue;
            }
            Err(error) => {
                tracing::warn!(server = %from.handle, %error, "could not fetch a shared file");
                continue;
            }
        };
        let blobs = state.blobs.clone();
        let stored =
            tokio::task::spawn_blocking(move || blobs.put(&data).map(|hash| (hash, data.len())))
                .await
                .map_err(AppError::internal)
                .and_then(|stored| stored);
        let Ok((sha256, size)) = stored else {
            continue;
        };
        let name = crate::files::clean_file_name(&file.name);
        let mime: String = file.mime.chars().take(100).collect();
        let now = now_ms();
        if let Ok(id) = state
            .db
            .call(move |conn| {
                store::insert_file(
                    conn,
                    &store::NewFile {
                        name: &name,
                        mime: &mime,
                        sha256: &sha256,
                        size,
                    },
                    author,
                    now,
                )
            })
            .await
        {
            ids.push(id);
        }
    }
    ids
}

async fn change(
    state: &AppState,
    from: &Instance,
    channel: i64,
    uid: &str,
    change: Change,
) -> Result<(), String> {
    let here = here(state)?;
    let (from_owned, uid) = (from.clone(), uid.to_owned());
    let deleting = matches!(change, Change::Delete);
    let found = state
        .db
        .call(move |conn| {
            let Some((local, role)) = local_channel(conn, &from_owned, channel)? else {
                return Ok(Err("the channel isn't shared with this server"));
            };
            let Some(id) = outbound::resolve(conn, &here.own(), local, &uid)? else {
                return Ok(Err("the message isn't here"));
            };
            // Who wrote it: the server it came from, by its uid.
            let written_on = uid.split_once('#').map_or("", |(server, _)| server);
            let allowed = match role {
                // A guest changes its own people's messages only.
                Role::Host => written_on.eq_ignore_ascii_case(&from_owned.handle),
                // The host edits what others wrote, and deletes anything in
                // its channel; it never edits ours.
                Role::Guest => deleting || !written_on.eq_ignore_ascii_case(&here.handle),
            };
            if !allowed {
                return Ok(Err("the server may not change that message"));
            }
            Ok(Ok((local, id)))
        })
        .await
        .map_err(failed)??;
    let (local, id) = found;
    messages::apply_remote_change(state, local, id, change, from.id)
        .await
        .map_err(failed)
}

async fn reaction(
    state: &AppState,
    from: &Instance,
    channel: i64,
    uid: &str,
    person: &Person,
    emoji: String,
    added: bool,
) -> Result<(), String> {
    let here = here(state)?;
    let (from_owned, uid, person) = (from.clone(), uid.to_owned(), person.clone());
    let now = now_ms();
    let found = state
        .db
        .call(move |conn| {
            let Some((local, role)) = local_channel(conn, &from_owned, channel)? else {
                return Ok(Err("the channel isn't shared with this server"));
            };
            if !may_speak_for(role, &from_owned, &here, &person) {
                return Ok(Err("the server may not speak for that person"));
            }
            let Some(message) = outbound::resolve(conn, &here.own(), local, &uid)? else {
                return Ok(Err("the message isn't here"));
            };
            let Some(account) = data::account(conn, &here.own(), &person, now)? else {
                return Ok(Err("that person can't react here"));
            };
            Ok(Ok((local, message, account)))
        })
        .await
        .map_err(failed)??;
    let (local, message, account) = found;
    let emoji: String = emoji.chars().take(64).collect();
    messages::apply_remote_reaction(state, local, message, account, emoji, added, from.id)
        .await
        .map_err(failed)
}

async fn membership(
    state: &AppState,
    from: &Instance,
    channel: i64,
    person: &Person,
    joined: bool,
) -> Result<(), String> {
    let here = here(state)?;
    let (from_owned, person) = (from.clone(), person.clone());
    let now = now_ms();
    let found = state
        .db
        .call(move |conn| {
            let Some((local, role)) = local_channel(conn, &from_owned, channel)? else {
                return Ok(Err("the channel isn't shared with this server"));
            };
            // Only private channels have members to keep.
            if store::audience(conn, local)?.is_none() {
                return Ok(Ok(None));
            }
            let ours = person.server.eq_ignore_ascii_case(&here.handle);
            // The host may take our people out of its channel, not put them in.
            let allowed = may_speak_for(role, &from_owned, &here, &person) || (ours && !joined);
            if !allowed {
                return Ok(Err("the server may not speak for that person"));
            }
            let Some(account) = data::account(conn, &here.own(), &person, now)? else {
                return Ok(Ok(None));
            };
            if joined {
                store::add_member(conn, local, account)?;
            } else {
                store::remove_member(conn, local, account)?;
            }
            Ok(Ok(Some((local, account))))
        })
        .await
        .map_err(failed)??;
    if let Some((local, account)) = found {
        outbound::membership(state, local, account, joined, Some(from.id)).await;
    }
    Ok(())
}
