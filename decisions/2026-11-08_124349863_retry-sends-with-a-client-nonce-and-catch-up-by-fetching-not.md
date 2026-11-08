+++
schema_version = 1
id = "01M6XJZEQ7A95DD8X6WQ5JMMXF"
title = "Retry sends with a client nonce and catch up by fetching, not with an event log"
date = "2026-11-08"
status = "proposed"
tags = []
supersedes = []
superseded_by = []
depends_on = []
related_to = []
+++
## Decision

Sideporch makes sends safe to retry with a per-send id from the browser, and catches a page up after a dropped connection by fetching what it missed, not by keeping an event log:

- **Sends**: the browser makes a random id (`client_nonce`) per message and keeps it until the server answers. The server stores it with the message, scoped to the author by a partial unique index on `(user_id, client_nonce)`. A send with an id already stored returns the message stored the first time and delivers nothing again: no live event, push, automation event, outgoing webhook or federation send.
- **Catching up**: after a dropped socket the page asks for the messages after the newest one it shows (`GET /c/{channel}/since`, optionally in a thread), capped at 100 with a `complete` flag, and refreshes the messages on screen in one batch (`POST /c/{channel}/messages/refresh`) so edits, deletions, reactions and reply counts arrive. It reloads when the answer is incomplete or the page isn't a channel. A `resync` from the server still reloads.

## Context

ClickClack documents durable event replay and idempotent creation with a client nonce ([realtime](https://github.com/openclaw/clickclack/blob/main/docs/features/realtime.md), [messages](https://github.com/openclaw/clickclack/blob/main/docs/features/messages.md)). Replaying a log of every event would need a new table for every kind of change, retention rules, and a second source of truth beside the messages table. Sideporch already keeps the messages, reactions and edits in SQLite and renders them on the server, so the page can ask for what it lacks and the server answers from data it already has.

- **An event log** would replay edits and reactions exactly, but it duplicates state the messages tables already hold, needs retention and a cursor per connection, and changes every event path (live, push, automations).
- **Reloading after every drop** (the previous behaviour) is simple and always correct, but it loses the scroll position, the open thread and focus.
- **A nonce scoped to the whole server** would let one person's id collide with, or reveal, another's sends. Scoping it to the author costs nothing extra.

## Consequences

- Messages gained a nullable `client_nonce` column and a partial unique index. Older messages keep `NULL`.
- Retried sends are answered as the first was: a `204` for the page, or the same redirect. The response carries no message id, so the page can't tell a retry from a first send.
- Edits and deletions are only fetched for messages on screen at reconnect time. Messages a page never loaded are fetched by the catch-up query, which returns them as they are now.
- More than 100 messages missed, or a page that isn't a channel, means a reload. Busy channels may reload after long outages.
- A retry is answered before its files are uploaded again. Only two copies racing at once can both upload; the one that loses deletes its uploads.
