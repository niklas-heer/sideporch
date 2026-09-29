+++
title = "Limits and safety"
description = "What the sandbox allows, how much a script may do, and what keeps automations from harming the server."
weight = 8
+++

Automations are written by admins, but they still run in a sandbox, so a mistake in a script can't take the server down or read what it shouldn't.

## The sandbox

- Each automation runs on its own thread, in its own Lua 5.4 state. One automation failing or looping doesn't stop the others.
- Scripts have Lua's `string`, `table`, `math`, `utf8` and `coroutine` libraries and the base functions, but no `os`, `io` or `debug`, and no `load`, `dofile` or `loadfile`: no files, no processes, no access to the machine.
- The network is reachable only through `sideporch.http`. Requests to private, loopback and link-local addresses are refused unless an admin turns on **Let automations reach private networks** under **Automations → Settings**.
- Automations see public channels only, never private channels or direct messages, and can't read who is in them.
- Posts and reactions from automations never trigger other automations.

## How much a script may do

| What | Limit |
| --- | --- |
| Lua instructions per handler call | 2,000,000 |
| Messages and reactions per handler call | 20 |
| HTTP requests per handler call | 10 |
| HTTP timeout | 10 seconds by default, at most 30 |
| Memory per automation | 16 MB |
| Script length | 100 kB |
| Saved data | keys up to 200 bytes, values up to 64 kB |
| `sideporch.every` | at least every 10 seconds |
| Buttons per message | 5 |
| Waiting for a webhook or command answer | 15 seconds |
| Runs kept in the log | the newest 100 |
| Versions kept in the history | the newest 50 |

A handler that goes over a limit stops with an error, which shows in the editor and in **Recent runs**. Test runs report how many instructions a run used, to see how close a script comes.

## When something goes wrong

- **A syntax error** keeps the script from loading. The editor underlines it before you save.
- **An error in a handler** stops that call only; the next event calls it again. The error shows at the top of the editor until the next save.
- **An error while loading**, for example in code outside any handler, keeps the automation from running at all until it's fixed.

Scripts, libraries, their history and their data all live in the database, so a [backup](@/docs/community/server/backups.md) covers automations too. Keep `secret.key` with it, or stored secrets can't be read after a restore.
