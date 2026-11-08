+++
title = "How a message travels"
description = "What happens between pressing send and the message showing on everyone's screen."
weight = 1
+++

Every new message takes the same path, whether a person, a webhook, an automation or another server sent it.

{% <diagram caption="Sending a message, from the browser to everyone who should see it."> %}
sequenceDiagram
  participant You as Your browser
  participant S as Sideporch
  participant DB as SQLite
  participant Hub as Live connections
  participant Push as Push services
  participant Auto as Automations
  You->>S: POST the message
  S->>S: May they post? Permissions, time-outs, limits, repeats
  S->>DB: One transaction: store, index for search, mark read, count toward trust
  S->>Hub: Rendered message
  Hub-->>You: The message, in place
  Hub-->>You: Others looking at the channel get it in full
  Hub-->>You: Everyone else: "the channel has something new"
  S->>Push: Notify people who were mentioned or follow the thread and aren't looking
  S->>Auto: A message event, for public channels
  S->>S: Link previews, outgoing webhooks, connected servers
{% </diagram> %}

## Step by step

1. **Checks.** Sideporch checks that the sender may post there: whether the channel lets them start posts or reply, whether they're timed out, whether their trust level allows links, uploads or `@channel`, how many messages a newcomer sent this minute, and whether they sent the same text twice in the last ten minutes.
2. **Storing.** In one database transaction, the message is stored, added to the full-text search index, marked read for its author, and counted toward the author's trust level. Either all of it happens or none.
3. **Showing it live.** The message is rendered to HTML once. Browsers that show the channel get that HTML and put it in place; browsers elsewhere get a short notice that the channel has something new, once until they look, or every time they're mentioned. The work grows with the number of people looking, not with everyone online.
4. **Notifying.** People it concerns (mentions, replies in threads they're part of, direct messages) get a Web Push notification, unless they're looking at Sideporch right now. Sideporch sends pushes itself, signed with its own key, so no third-party notification service is involved.
5. **Reacting.** For public channels, automations that listen for messages get an event. Messages from automations never trigger automations, so they can't loop.
6. **Afterwards.** Links from people get a preview, fetched by the server. Outgoing webhooks for the channel are called. If the channel is shared with other servers, the message goes into the queue for them.

## Typing and reading

Typing indicators and read positions travel over the same WebSocket in the other direction. Typing is sent at most every few seconds, only to the people looking at that channel, and to the few people of a direct message wherever they are. Read positions decide which channels show as unread.

## Sending again safely

Each send carries an id that the browser makes, and Sideporch keeps it with the message. When a send gets no answer, because the connection dropped or the server failed, the browser sends it again with the same id, a few times with growing waits. The message is stored once per id: a repeat returns the message stored the first time, and nothing is shown, pushed, sent to webhooks or passed to connected servers a second time. Text and files stay in the box until the server answers, and a message is only sent again when its text and files haven't changed. Slash commands are never sent again, because they act when they run.

## When the connection drops

The page keeps what you were doing: scroll position, the open thread, the text in the box and the focus. "Reconnecting…" shows while the browser tries again, waiting longer after each failure, with some randomness so that many browsers don't return at once. When the connection is back, the page fetches the messages that arrived in the channel or thread meanwhile, refreshes the messages on screen (so edits, deletions, reactions and reply counts made while you were away appear), and updates the unread marks in the sidebar.

The page reloads only when it can't catch up completely, which happens when more than 100 messages arrived, or when it isn't showing a channel. Sideporch also reloads if it dropped live events for a connection it was too slow to keep up with, because then the server itself can't tell what was missed.

## Messages from other places

- **Webhooks** (Slack-compatible) and **automations** post through the same pipeline, under their own name and picture.
- **Connected servers** send signed events. A shared channel lives on its host server, which passes each message on to the other servers in the order they happened, and retries until they arrive. See [Connect with other servers](@/docs/community/connect/other-servers.md).
