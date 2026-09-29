+++
title = "Automations"
description = "Small Lua scripts that react to messages, run on schedules, answer commands and webhooks, and call other services."
weight = 3
sort_by = "weight"
template = "docs-section.html"
+++

Automations are small scripts that do the busywork in your chat. Like Slack workflows, each one says what should happen when something happens, but in a real language, [Lua](https://www.lua.org/manual/5.4/), so they can do more than a form of steps allows. Admins write them in the browser, under **Automations** in the account menu, test them against made-up events, and switch them on.

```lua
-- Greet everyone who joins.
sideporch.on("member_joined", function(event)
  sideporch.post("general", "Welcome to the porch, " .. event.user .. "! :wave:")
end)

-- Ask for updates every weekday morning.
sideporch.cron("0 9 * * mon-fri", function()
  sideporch.post("standup", "Good morning! What are you working on today?")
end)
```

{{<shot name="automation" alt="The automation editor: a Lua script that asks for deploy approvals, with what it listens to, a test run panel and Ask AI." caption="The editor: the script on the left, what it listens to, test runs and Ask AI on the right." />}}

## What they're good for

- **Welcoming people** and pointing them to the right channels.
- **Reminders and routines**: a standup question every morning, a weekly digest, a nudge when a thread goes quiet.
- **Answers**: slash commands like `/weather Berlin` or `/oncall`, and replies to common questions.
- **Approvals**: a message with **Approve** and **Reject** buttons that updates itself when someone clicks.
- **Connecting services**: a webhook URL for each automation that GitHub, a form or a monitor can call, and requests out to any API.
- **Keeping score**: data that survives restarts, for counters, kudos and state.

The [examples](@/docs/integrations/automations/examples.md) show each of these as a complete script.

## Where to go from here

1. [Write your first automation](@/docs/integrations/automations/first-automation.md): from an empty editor to a script that runs, in five minutes.
2. Learn [the editor](@/docs/integrations/automations/editor.md): completions, the linter, test runs, run logs and history.
3. See what scripts can [react to](@/docs/integrations/automations/triggers.md) and what they can [keep, call and share](@/docs/integrations/automations/data-and-services.md).
4. Borrow from the [examples](@/docs/integrations/automations/examples.md), or [import](@/docs/integrations/automations/sharing.md) automations someone else wrote.
5. Look things up in the [API reference](@/docs/integrations/automations/api.md), and know the [limits](@/docs/integrations/automations/limits.md).

To have a model write automations for you, see [AI and MCP](@/docs/integrations/automations/ai-and-mcp.md).

{% <note> %}
Automations see public channels only, never private channels or direct messages. Only admins can write them.
{% </note> %}
