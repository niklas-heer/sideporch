+++
schema_version = 1
id = "01M3J6PYXDJY1DJVFKX0DN39AP"
title = "Run admin-written Lua automations in a bounded sandbox"
date = "2026-09-27"
status = "superseded"
tags = ["automations", "security"]
supersedes = []
superseded_by = ["01M3M77KF4V6HX4AC84FV21HM3"]
depends_on = []
related_to = []
+++
## Decision

Admins can write automations as small Lua scripts in the browser. Sideporch runs them with [mlua](https://crates.io/crates/mlua) and a vendored Lua 5.4, compiled into the binary.

- Every enabled script gets its own Lua state on one dedicated thread. Only the `string`, `table`, `math`, `utf8` and `coroutine` libraries are loaded; `load`, `dofile`, `loadfile`, `require`, `collectgarbage` and `string.dump` are removed. There is no file, process, or network access.
- Each call has a budget of 2,000,000 instructions, enforced by a global hook that also covers coroutines. Each state has a 16 MB memory limit, and each call may post at most 20 messages. A script that exceeds a limit stops with an error that the editor shows.
- Scripts see new messages in public channels only, never direct messages, and never messages from automations, so they cannot loop.
- The API is a `sideporch` table: `on_message`, `every` (at least 10 seconds), `post`, `reply`, `get`/`set` for persistent per-script data, and `now`.

## Context

On 2026-09-27 Niklas suggested "Lua-based automations" alongside Slack-style webhooks.

- **Luau** has a built-in sandbox mode and interrupts, but it is a C++ codebase and a Roblox dialect. Standard Lua 5.4 is what most people know, and its C sources build statically for musl with zig.
- **Rhai** or **WebAssembly plugins** are Rust-native, but they are less approachable for people writing a quick bot in a browser text area.
- **Only webhooks and an external bot API** would push simple automations out to another server, which is at odds with the one-binary goal.

## Consequences

- The Lua thread keeps its own SQLite connection for script data; posting goes through the same message pipeline as people and webhooks.
- A slow script delays other scripts, but not the server. If automations grow heavier, run them on a small pool of threads.
- Outgoing HTTP from scripts is deliberately missing. Adding it later needs its own decision about allow-lists and server-side request forgery.
