+++
schema_version = 1
id = "01M3KXCGG2A5ZKEBS3F7BHMRGV"
title = "Let automations react to reactions and answer their own webhooks"
date = "2026-09-28"
status = "accepted"
tags = ["automations", "webhooks"]
supersedes = []
superseded_by = []
depends_on = []
related_to = ["01M3J6PYXDJY1DJVFKX0DN39AP"]
+++
## Decision

Automations react to more than new messages. The `sideporch` API gains:

- `sideporch.on_reaction(handler)`: people adding or removing a reaction on a message in a public channel.
- `sideporch.on_webhook(handler)`: HTTP requests to a secret per-automation URL, `/hooks/automations/<token>` and any path below it. The handler gets the method, path, query, headers (without cookies), body and parsed JSON, and its return value becomes the response. The request waits up to 10 seconds for the automation thread. Admins can rotate the URL.
- `sideporch.react(message, emoji)` and `sideporch.json.encode`/`decode`/`array`.

Reactions from automations are stored in their own table, shown beside people's, and never trigger automations, like their messages. Every call that prints, acts, answers a webhook or fails is kept in a run log (the newest 100 per automation), and every saved script version is kept (the newest 50) and can be restored.

## Context

On 2026-09-28 Niklas asked for automations to "react to webhooks and reactions" so they are a useful addition, and for scripts to be easy to back up.

- Incoming webhooks let external systems (CI, monitoring, home automation) drive a script without Sideporch reaching out, so the sandbox keeps its no-network rule from the Lua sandbox decision.
- Reactions are the cheapest way for people to answer an automation ("react with ✅ to acknowledge").
- Scripts already lived in SQLite. Storing versions and run logs there too keeps the single-database backup complete.

## Consequences

- Outgoing HTTP from scripts is still missing and still needs its own decision.
- A webhook call runs on the single automation thread, so a slow script delays other automations for up to its instruction budget.
- The automation thread now writes run logs on its own connection. All write transactions therefore start with `BEGIN IMMEDIATE`; a deferred transaction could otherwise fail with "database is locked" when it upgrades while the other connection writes.
