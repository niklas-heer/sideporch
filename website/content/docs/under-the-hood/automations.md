+++
title = "Automations inside"
description = "How automations run: a worker per script, a Lua sandbox, budgets, and how events reach them."
weight = 3
+++

Each running automation gets its own thread, its own Lua 5.4 state and its own database connection. Events, webhook requests, slash commands and timers reach it through a queue, and it handles them one at a time.

{% <diagram caption="How work reaches automations."> %}
flowchart LR
  M[New messages,<br/>reactions, members] --> R{Router}
  C[Slash commands] --> R
  H[Webhook requests] --> R
  R --> W1[Worker: Deploy approvals]
  R --> W2[Worker: Weather]
  R --> W3[Worker: Welcome]
  T[Timers and cron] --> W1
  W1 --> S1[(Lua state)]
  W2 --> S2[(Lua state)]
  W3 --> S3[(Lua state)]
  W1 --> DB[(SQLite)]
  W2 --> DB
  W3 --> DB
{% </diagram> %}

## Why a thread each

A script that waits for a slow API only delays itself. Chat, other automations and the rest of the server carry on. When a script is saved, its worker finishes its current job and a new one starts with the new code.

## The sandbox

The Lua state has the safe parts of the standard library (`string`, `table`, `math`, `utf8`, `coroutine`) and nothing that touches the machine: no files, no processes, no loading of compiled code. What a script can do goes through the `sideporch` table:

- Actions (posting, replying, reacting, updating) are checked the same way as a person's, and only reach public channels.
- HTTP requests go through a client that refuses private, loopback and link-local addresses unless an admin allows them, so a script can't probe the server's network.
- Secrets are read through `sideporch.secret`, and their values are replaced wherever they would show up in a run log.

## Budgets

Every call has an instruction budget, counted by a hook in the Lua interpreter, and every state a memory limit. A loop that never ends stops with an error after its budget instead of stalling anything. Each call may also post, react and make HTTP requests only so often. The numbers are in [Limits and safety](@/docs/integrations/automations/limits.md).

## Test runs

The editor's test runs use the same sandbox with a different environment: actions are described instead of carried out, saved data is a copy, and HTTP requests go out only when you allow it. That's also how Sideporch checks scripts that AI wrote before showing them.

## One description of the API

The API is described once, in the code. The linter's rules, the editor's completions, the reference in the editor and on this site, the prompt for AI and the MCP server are all generated from that description, so they can't drift apart.
