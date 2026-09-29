+++
title = "Automations"
description = "Small Lua scripts that react to messages, run on schedules, answer commands and webhooks, and call other services."
weight = 3
+++

Admins write automations in Lua, in the browser, under the lightning icon in the sidebar. Like Slack workflows, each one says what should happen when something happens, but with a real language:

```lua
-- Events, narrowed down with filters.
sideporch.on("message", { channel = "alerts", pattern = "^!ack" }, function(msg)
  sideporch.react(msg, "white_check_mark")
end)

sideporch.on("member_joined", function(event)
  sideporch.post("general", "Welcome to the porch, " .. event.user .. "!")
end)

-- Schedules, in cron syntax and your time zone.
sideporch.cron("0 9 * * mon-fri", function()
  sideporch.post("standup", "Good morning! What are you working on today?")
end)

-- Slash commands, answered privately.
sideporch.command("weather", { description = "Today's weather", usage = "<city>" }, function(cmd)
  local weather = require("weather") -- a library, see below
  local today = weather.today(cmd.args[1] or "Berlin")
  sideporch.respond(cmd, "**" .. today.summary .. "**, " .. today.temperature .. " °C")
end)

-- Webhooks: each automation has its own URL.
sideporch.on_webhook(function(request)
  sideporch.post("deploys", "Deploying **" .. request.json.service .. "**")
  return { json = { ok = true } }
end)
```

{{<shot name="automation" alt="The automation editor: a Lua script that asks for deploy approvals, with what it listens to, a test run panel and Ask AI." caption="The editor, with what the script listens to, a test run and Ask AI." />}}

## What automations can do

- **Events**: `message`, `message_changed`, `message_deleted`, `reaction_added`, `reaction_removed`, `member_joined` and `channel_created`, narrowed down by channel, a text pattern, the emoji, the person, and whether it's in a thread.
- **Buttons**: `sideporch.post` and `sideporch.reply` can put up to five buttons under a message. Clicks arrive as `button` events for that automation, which can change its message with `sideporch.update`, for approvals and the like.
- **Schedules**: `sideporch.cron` with standard five-field expressions (`*/15 * * * *`, `@daily`, `mon-fri`), in the instance's time zone or one you name, and `sideporch.every(seconds, …)`.
- **Slash commands**: `/name` in any channel runs the automation that registered it. Answers from `sideporch.respond` are only visible to the person who typed the command. `/help` lists all commands, and the message box suggests them.
- **Webhooks**: every automation has its own URL; `sideporch.on_webhook` answers requests to it.
- **Outgoing HTTP**: `sideporch.http.get`, `.post` and `.request` call APIs and read JSON.
- **Data**: `sideporch.get` and `sideporch.set` keep data between runs; `sideporch.json` reads and writes JSON.

The [Automation API](@/docs/integrations/automation-api.md) lists every function and the tables handlers receive. Automations see public channels only, never private channels or direct messages.

## Secrets

API tokens live under **Automations → Secrets**, encrypted in the database with a key kept outside it: `secret.key` in the data directory, or the `SIDEPORCH_SECRET_KEY` environment variable. Environment variables named `SIDEPORCH_SECRET_<NAME>` work too. Scripts read them with `sideporch.secret("NAME")`, and their values are hidden in run logs.

## Libraries

A library is shared code, for example a client for an API, that automations load with `require("name")`. Write them next to automations; the `weather` example above uses one.

## Limits and safety

Each automation runs on its own thread, in a sandbox without file or process access. Each run may execute a limited number of instructions, use limited memory, and post and request only so much; the [API reference](@/docs/integrations/automation-api.md) has the numbers.

Requests to private, loopback and link-local addresses are refused unless an admin turns on **Let automations reach private networks** under **Automations → Settings**, so scripts can't probe the server's own network.

## Writing and debugging

The editor is made for it:

- Syntax highlighting, completions for the `sideporch` API, and indentation.
- A linter ([selene](https://github.com/Kampfkarren/selene)) that knows the sandbox, and a formatter ([StyLua](https://github.com/JohnnyMorganz/StyLua)).
- **Test runs** against a simulated message, reaction, command, webhook request, schedule, new member or new channel. They show what the script prints, posts and answers, without changing anything in Sideporch.
- What each automation **listens to**, with its next scheduled runs, a **run log**, a **history** of every saved version with one-click restore, and its **webhook URL**.

Scripts, libraries, their history and their data all live in the database, so a [backup](@/docs/community/backups.md) covers automations too. Keep `secret.key` with it.

To have a model write automations for you, see [AI and MCP](@/docs/integrations/ai-and-mcp.md).
