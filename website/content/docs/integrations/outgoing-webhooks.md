+++
title = "Outgoing webhooks"
description = "Send people's messages from a channel to another service, and post its answers back."
weight = 2
+++

Outgoing webhooks send messages from a channel to another service, like Slack's and Mattermost's. Use them to connect a bot you already run.

## Add one

In the channel's settings (the gear icon), add an outgoing webhook with:

- the **URL** Sideporch sends messages to;
- optionally, **trigger words** such as `!deploy`. Then only messages starting with one go out; without them, every message does.

Only people's messages go out, never those of bots or automations, so two bots can't set each other off in a loop.

## What Sideporch sends

A `POST` with JSON:

```json
{
  "token": "…",
  "channel_id": 4,
  "channel_name": "ops",
  "message_id": 1234,
  "thread_id": null,
  "user_id": 7,
  "user_name": "ada",
  "user_display_name": "Ada",
  "text": "!deploy api",
  "trigger_word": "!deploy",
  "timestamp": 1790000000,
  "url": "https://chat.example.com/c/4/m/1234"
}
```

Check `token` against the one shown in the channel's settings to know the request came from your Sideporch. `thread_id` is set when the message is a reply in a thread.

## Answer back

If the service answers with JSON that has `text`, Sideporch posts it in the channel as the webhook, or in the thread when the message was a reply in one. Add `"response_type": "comment"` to answer in the message's thread even when it wasn't. `username` and `icon_url` (or `icon_emoji`) change who the answer appears to be from.

```json
{ "text": "Deploying **api**…", "response_type": "comment" }
```

Requests go through the same guarded client as automations: addresses on private networks are refused unless an admin allows them under **Automations → Settings**.

For anything more involved, write an [automation](@/docs/integrations/automations.md).
