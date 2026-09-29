//! Outgoing webhooks: new messages in a channel, sent as JSON to a URL.
//!
//! Like Slack's and Mattermost's outgoing webhooks, a webhook can listen to
//! every message or only to messages that start with a trigger word. If
//! the receiver answers with JSON that has `text`, Sideporch posts it in
//! the channel, or in the thread when `response_type` is `comment`. Only
//! people's messages go out, so bots can't set off loops. Requests use the
//! automations' HTTP client and their private-network setting.

use std::{sync::Arc, time::Duration};

use serde_json::{Value, json};

use crate::{
    AppState,
    automations::{self, http::Http},
    error::AppResult,
    messages::{self, Draft, Sender},
    now_ms,
    store::{self, Author, Message, OutgoingWebhook},
};

const TIMEOUT: Duration = Duration::from_secs(10);

/// The trigger word `text` starts with, or `Some("")` for a webhook with
/// none. `None` when the webhook doesn't want the message.
pub fn trigger<'a>(hook: &'a OutgoingWebhook, text: &str) -> Option<&'a str> {
    if hook.triggers.is_empty() {
        return Some("");
    }
    let first = text.split_whitespace().next()?.to_lowercase();
    hook.triggers
        .iter()
        .find(|word| first == word.to_lowercase())
        .map(String::as_str)
}

struct Context {
    channel: String,
    user_id: i64,
    username: String,
    display_name: String,
    base: String,
}

fn payload(hook: &OutgoingWebhook, message: &Message, context: &Context, word: &str) -> Value {
    json!({
        "token": hook.token,
        "channel_id": message.channel_id,
        "channel_name": context.channel,
        "message_id": message.id,
        "thread_id": message.parent_id,
        "user_id": context.user_id,
        "user_name": context.username,
        "user_display_name": context.display_name,
        "text": message.body,
        "trigger_word": word,
        "timestamp": message.created_at / 1000,
        "url": format!("{}/c/{}/m/{}", context.base, message.channel_id, message.id),
    })
}

/// Sends `message` to the channel's outgoing webhooks in the background.
pub fn dispatch(state: &AppState, message: &Message, base: String) {
    let Author::User {
        id: user_id,
        display_name,
        ..
    } = &message.author
    else {
        return;
    };
    let state = state.clone();
    let message = message.clone();
    let user_id = *user_id;
    let display_name = display_name.clone();
    tokio::spawn(async move {
        if let Err(error) = deliver(&state, &message, user_id, display_name, base).await {
            tracing::warn!(?error, "outgoing webhook failed");
        }
    });
}

async fn deliver(
    state: &AppState,
    message: &Message,
    user_id: i64,
    display_name: String,
    base: String,
) -> AppResult<()> {
    let channel_id = message.channel_id;
    let (hooks, channel, username, settings) = state
        .db
        .call(move |conn| {
            let hooks = store::outgoing_webhooks(conn, channel_id)?;
            let channel = store::channel_for(conn, channel_id, user_id)?
                .map(|channel| channel.name)
                .unwrap_or_default();
            let username = store::user(conn, user_id)?
                .map(|user| user.username)
                .unwrap_or_default();
            Ok((hooks, channel, username, automations::Settings::load(conn)?))
        })
        .await?;
    let wanted: Vec<(OutgoingWebhook, String)> = hooks
        .iter()
        .filter_map(|hook| Some((hook.clone(), trigger(hook, &message.body)?.to_owned())))
        .collect();
    if wanted.is_empty() {
        return Ok(());
    }
    let http = Arc::new(Http::new(settings.allow_private_network)?);
    let context = Context {
        channel,
        user_id,
        username,
        display_name,
        base,
    };
    for (hook, word) in wanted {
        let body = payload(&hook, message, &context, &word).to_string();
        let client = Arc::clone(&http);
        let url = hook.url.clone();
        let response = tokio::task::spawn_blocking(move || {
            client.send(automations::http::Request {
                method: "POST".to_owned(),
                url,
                headers: vec![("content-type".to_owned(), "application/json".to_owned())],
                body: body.into_bytes(),
                timeout: TIMEOUT,
            })
        })
        .await
        .map_err(crate::error::AppError::internal)?;
        let (status, error) = match &response {
            Ok(response) if (200..300).contains(&response.status) => (Some(response.status), None),
            Ok(response) => (
                Some(response.status),
                Some(format!("answered {}", response.status)),
            ),
            Err(error) => (None, Some(error.clone())),
        };
        let hook_id = hook.id;
        let now = now_ms();
        state
            .db
            .call(move |conn| {
                store::record_outgoing_result(conn, hook_id, now, status, error.as_deref())
            })
            .await?;
        if let Ok(response) = response
            && (200..300).contains(&response.status)
        {
            answer(state, &hook, message, &response.body).await?;
        }
    }
    Ok(())
}

/// Posts the receiver's `text`, if it sent one.
async fn answer(
    state: &AppState,
    hook: &OutgoingWebhook,
    message: &Message,
    body: &str,
) -> AppResult<()> {
    let Ok(reply) = serde_json::from_str::<Value>(body) else {
        return Ok(());
    };
    let Some(text) = reply
        .get("text")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
    else {
        return Ok(());
    };
    let in_thread = reply.get("response_type").and_then(Value::as_str) == Some("comment");
    let parent_id = message
        .parent_id
        .or_else(|| in_thread.then_some(message.id));
    let name = reply
        .get("username")
        .and_then(Value::as_str)
        .map_or_else(|| hook.name.clone(), |name| name.chars().take(80).collect());
    let icon = reply
        .get("icon_url")
        .or_else(|| reply.get("icon_emoji"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    messages::post(
        state,
        Draft {
            channel_id: message.channel_id,
            parent_id,
            sender: Sender::Bot { name, icon },
            body: text.chars().take(10_000).collect(),
            attachments: Vec::new(),
            files: Vec::new(),
            gif: None,
            poll: None,
            buttons: Vec::new(),
        },
    )
    .await
    .map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hook(triggers: &[&str]) -> OutgoingWebhook {
        OutgoingWebhook {
            id: 1,
            name: "Bot".to_owned(),
            url: "https://example.com".to_owned(),
            triggers: triggers.iter().map(|word| (*word).to_owned()).collect(),
            token: "t".to_owned(),
            last_at: None,
            last_error: None,
        }
    }

    #[test]
    fn matches_trigger_words() {
        assert_eq!(trigger(&hook(&[]), "anything"), Some(""));
        assert_eq!(
            trigger(&hook(&["!deploy", "ship"]), "Ship it now"),
            Some("ship")
        );
        assert_eq!(trigger(&hook(&["ship"]), "we ship"), None);
    }
}
