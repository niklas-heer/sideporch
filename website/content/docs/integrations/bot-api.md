+++
title = "Bots and the chat API"
description = "Let scripts and other programs post and read public channels as a bot, with tokens that reach only what you allow."
weight = 3
+++

A bot is a named account for a script or another program. It posts as itself, and it reaches the chat through the API under `/api/v1/` with a token. There is no SDK or command-line tool yet: any HTTP client works, and the examples below use `curl`.

Bots reach **public channels only**. Private channels and direct messages are never readable or writable by a bot, even when a token lists them. Sideporch checks each request again, so a channel made private later stops working on the next request.

## Make a bot

Open **Automations → Bots** (`/settings/bots`), beside MCP. Admins can manage bots; so can anyone a role gives *Manage bots*. Give the bot:

- a **handle**, its name in the API: 2 to 32 letters, digits, `-` or `_`, starting with a letter or digit;
- a **display name**, shown on its messages, up to 60 characters;
- optionally an **icon**: an emoji such as `:rocket:` or an image address starting with `https://`.

Then add a token. Each token has:

- a **name**, so you can tell tokens apart;
- **scopes**, what the token may do (below);
- **channels**: every public channel, or only the ones you check;
- an **expiry**: never, or 30, 90 or 365 days.

Sideporch shows the token **once**, on the page after you create it, and stores only a hash of it. Copy it then. Tokens start with `spb_`. The page lists each bot's tokens with their scopes, channels and last use, and you can revoke a token or a whole bot. Revoking takes effect on the next request.

## Scopes

| Scope | Allows |
| --- | --- |
| `channels:read` | Listing the channels the token may use. |
| `messages:read` | Reading messages in those channels. |
| `messages:write` | Posting messages and thread replies there. |

`GET /api/v1/me` needs no scope: it tells a script who it is, and what its token can do.

## Endpoints

Send the token as a bearer token, and use JSON:

```sh
curl -H "Authorization: Bearer $SIDEPORCH_TOKEN" https://chat.example.com/api/v1/me
```

- `GET /api/v1/me`: the bot's handle, display name and icon, the token's scopes and channels, and when it expires.
- `GET /api/v1/channels`: the channels the token may use, with `id`, `name` and `topic`.
- `GET /api/v1/channels/{id}/messages`: messages oldest first, with `id`, `parent_id` (set for thread replies), `body`, `created_at`, `edited_at` and `author`. Use `limit` (default 50, at most 100), and `before` or `after` with a message id to page. The answer's `has_more` says whether more lie beyond the page. Only the names and ids of people are shown, never their email addresses.
- `POST /api/v1/channels/{id}/messages` with `{"body": "…"}`, or `{"body": "…", "thread_id": 123}` to reply in a thread: answers `201` with the new message and the bot's id in `bot_id`.

```sh
curl -X POST -H "Authorization: Bearer $SIDEPORCH_TOKEN" -H "Content-Type: application/json" \
  -d '{"body":"Deployed **v2** to staging"}' \
  https://chat.example.com/api/v1/channels/3/messages
```

Bodies are trimmed. An empty body, or one over 10,000 characters, is refused.

## Errors

Every failure answers with JSON of this shape, and the same `code` each time:

```json
{"error": {"code": "missing_scope", "message": "This token needs the messages:write scope."}}
```

| Status | Code | When |
| --- | --- | --- |
| 401 | `missing_token` | No `Authorization: Bearer` header. |
| 401 | `invalid_token` | The token is wrong, revoked or expired, or its bot is revoked. |
| 400 | `invalid_request` | The body or a query parameter isn't valid. |
| 403 | `missing_scope` | The token lacks the scope the route needs. |
| 403 | `channel_not_allowed` | The channel is public, but the token's list leaves it out. |
| 403 | `posting_not_allowed` | The channel only lets managers post there (see below). |
| 404 | `not_found` | No such route, no public channel with that id, or no such message. Private channels and direct messages look the same as channels that don't exist. |
| 429 | `rate_limited` | More than 120 requests a minute with one token. The `Retry-After` header says when to try again. |

## Where bots can post

A bot is not a manager. It can start posts only in channels where anyone may, and reply in threads only where anyone may. In a channel whose managers alone post, bots get `posting_not_allowed`.

## What happens to a bot's messages

A bot's message goes through the same path as anyone's. It appears live for everyone in the channel, notifies people by push and mentions, is searchable, is shared with other Sideporch servers the channel is shared with, and is seen by automations in public channels. Outgoing webhooks do not send bots' messages; they still send people's messages only.

Each bot message records which bot posted it.

## Limits and what's stored

- A token may make 120 requests a minute. Each token has its own allowance.
- The token's last use is recorded at most every five minutes, so a busy script doesn't write to the database on every request.
- Sideporch keeps a token only as a SHA-256 hash, so a copy of the database doesn't reveal a token that's still in use.
