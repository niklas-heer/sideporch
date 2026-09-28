+++
schema_version = 1
id = "01M3M77KF4V6HX4AC84FV21HM3"
title = "Run automations as isolated workers with events, schedules, commands, libraries and HTTP"
date = "2026-09-28"
status = "accepted"
tags = ["automations", "architecture", "security"]
supersedes = ["01M3J6PYXDJY1DJVFKX0DN39AP"]
superseded_by = []
depends_on = []
related_to = []
+++
## Decision

Automations are a small platform inside Sideporch, still in sandboxed Lua 5.4 ([mlua](https://crates.io/crates/mlua), vendored):

- **One thread per automation.** Each enabled automation gets a worker thread with its own Lua state and SQLite connection. Events, webhook requests, commands and timers queue on it, so a slow script (waiting for an API, say) delays only itself. Saving an automation, library, secret or setting stops all workers and starts the enabled automations again.
- **Triggers.** `sideporch.on(event, filter?, handler)` covers `message`, `reaction_added`, `reaction_removed`, `member_joined` and `channel_created`, with filters (`channel`, `pattern` as a Lua pattern, `emoji`, `user`, `thread`) checked before the handler runs. `sideporch.cron` takes five-field cron expressions, evaluated with jiff in the instance's time zone or a named one; the tz database is compiled in for the scratch image. `sideporch.every`, `on_webhook`, and `sideporch.command` for slash commands complete the set. A command name belongs to the oldest automation that registers it.
- **Slash commands.** Text that starts with a registered `/name` is not posted; the command's handler runs and its `sideporch.respond` answers go only to the person who typed it. `/help` lists commands. Unregistered `/text` is an ordinary message, so paths such as `/etc/hosts` are not swallowed.
- **Libraries.** Automations of kind `library` hold shared code. `require("name")` runs a library once per script state and caches what it returns; cycles are an error. Libraries never run on their own.
- **Outgoing HTTP.** `sideporch.http.get`, `post` and `request` block the script's worker while the shared Tokio runtime sends the request. Private, loopback, link-local, CGNAT, multicast and other internal addresses are refused by a custom DNS resolver, so a name cannot pass the check and then connect elsewhere; literal IPs are checked separately. Admins can allow internal addresses for home-lab use. No redirects are followed; timeouts default to 10 and cap at 30 seconds; bodies are limited to 1 MB out and 5 MB in; a call may make 10 requests.
- The sandbox rules otherwise hold: only the `string`, `table`, `math`, `utf8` and `coroutine` libraries; `load`, `dofile`, `loadfile`, `collectgarbage` and `string.dump` removed; 2,000,000 instructions and 20 posts or reactions per call; 16 MB per state. Automations only see public channels, and their own posts and reactions never trigger automations.

## Context

On 2026-09-28 Niklas asked for automations defined "like in Slack", where events and schedules decide what happens, plus API calls to external sources, commands, and Lua libraries that other automations can use, "for example, to talk to an external API".

- **One shared thread**, the previous design, would let one script's HTTP call stall every automation. A thread pool with Lua states moving between threads is possible with mlua's `send` feature but adds scheduling for little gain at the expected scale of dozens of automations.
- **Async Lua** (mlua's `async` feature with coroutines on Tokio) would avoid blocking threads, but makes the instruction budget and error handling harder to reason about and is harder for script authors to debug.
- **An allow-list of HTTP hosts** would be safer than an internal-address guard but awkward for the main use, calling arbitrary SaaS APIs. The guard blocks the dangerous targets, the cloud metadata service and the server's own network, by default.

## Consequences

- Idle automations cost a thread each. That is fine for dozens; hundreds would call for a pool.
- A reload interrupts nothing that is running, but timers restart their intervals.
- Scripts that call APIs make test runs reach the outside world; the editor's test panel says so and lets people switch requests off, and MCP test runs send no requests unless asked.
