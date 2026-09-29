+++
title = "Events, schedules, commands and webhooks"
description = "Everything an automation can react to: what happens in channels, button clicks, schedules, slash commands and requests to its webhook URL."
weight = 3
+++

An automation does nothing by itself. When it loads, it registers handlers: functions Sideporch calls when something happens. A script can register as many as it likes, of every kind.

## Events

`sideporch.on(event, handler)` calls `handler` each time `event` happens in a public channel:

| Event | When | The handler gets |
| --- | --- | --- |
| `message` | Someone posts a message or reply | the message: `text`, `channel`, `author`, `username`, `thread_id`, `is_bot`, … |
| `message_changed` | A message is edited | the message, with its new text |
| `message_deleted` | A message is deleted | the message, with what it said |
| `reaction_added` | Someone adds a reaction | `emoji`, `user`, `username`, and the `message` |
| `reaction_removed` | Someone removes one | the same |
| `reaction` | Either of the two | the same, with `added` telling which |
| `member_joined` | Someone joins the server | `user` and `username` |
| `channel_created` | A public channel is created | `channel`, `channel_id`, `user`, `username` |
| `button` | Someone clicks a button this automation posted | see [buttons](#buttons) |

The [API reference](@/docs/integrations/automations/api.md#tables) lists every field. Messages and reactions from automations never trigger handlers, so automations can't set each other off in a loop.

### Filters

Put a table between the event and the handler to narrow it down. Every key is optional, and all given must match:

| Filter | Matches when |
| --- | --- |
| `channel = "alerts"` | it's in #alerts |
| `pattern = "^!deploy"` | the message text matches this [Lua pattern](https://www.lua.org/manual/5.4/manual.html#6.4.1) |
| `emoji = "eyes"` | the reaction is this emoji |
| `user = "ada"` | this person did it (their username) |
| `thread = true` | it's a reply in a thread; `false` for everything outside threads |

```lua
-- Someone looks into an alert: say so in the thread.
sideporch.on("reaction_added", { channel = "alerts", emoji = "eyes" }, function(event)
  sideporch.reply(event.message, event.user .. " is looking into it.")
end)

-- Only messages outside threads that mention a ticket, like ENG-142.
sideporch.on("message", { pattern = "%u+%-%d+", thread = false }, function(msg)
  local ticket = msg.text:match("%u+%-%d+")
  sideporch.reply(msg, "Tracking [" .. ticket .. "](https://tracker.example.com/" .. ticket .. ")")
end)
```

Lua patterns are like simple regular expressions: `%d` is a digit, `%u` an uppercase letter, `%a` a letter, `%s` a space, `+` one or more, `-` a lazy repeat, and `%` escapes `.`, `-` and other magic characters. Patterns are case-sensitive; for a case-insensitive check, test `msg.text:lower()` inside the handler.

### What handlers can do

Handlers answer with the same few functions:

- `sideporch.post(channel, text)` posts a new message; `{ thread = id }` as a third argument posts in a thread.
- `sideporch.reply(message, text)` answers in the message's thread.
- `sideporch.react(message, emoji)` adds a reaction.
- `sideporch.update(message, text)` changes a message this automation posted.

Text is [Markdown](@/docs/using/chatting.md), including `@mentions` and `:emoji:`.

## Buttons

`sideporch.post` and `sideporch.reply` can put up to five buttons under a message. A click arrives as a `button` event for the automation that posted it, with `value`, `label`, who clicked (`user`, `username`) and the `message`. A `pattern` filter matches the button's value.

```lua
sideporch.command("lunch", { description = "Ask who's in for lunch" }, function(cmd)
  sideporch.post(cmd.channel, "Lunch at 12:30, who's in?", {
    buttons = {
      { label = "I'm in", value = "in", style = "primary" },
      { label = "Not today", value = "out" },
    },
  })
end)

sideporch.on("button", { pattern = "^in$" }, function(click)
  local text = click.message.text .. "\n- " .. click.user
  sideporch.update(click.message, text)
end)
```

`style` is `primary`, `danger`, or left out. `sideporch.update(message, text, { buttons = {} })` removes the buttons, for example once someone approved. The [deploy approval example](@/docs/integrations/automations/examples.md#deploy-approvals-with-buttons) puts it together.

## Schedules

`sideporch.cron(expression, handler)` runs on a schedule in [cron syntax](https://crontab.guru): five fields for the minute, hour, day of the month, month and day of the week.

| Expression | Runs |
| --- | --- |
| `0 9 * * mon-fri` | 9:00 every weekday |
| `*/15 * * * *` | every 15 minutes |
| `30 17 * * fri` | Fridays at 17:30 |
| `0 8 1 * *` | 8:00 on the first of each month |
| `@hourly`, `@daily`, `@weekly` | at the start of each hour, day or week |

Fields take lists (`mon,wed,fri`), ranges (`9-17`), steps (`*/5`) and names (`jan`, `sun`). As in classic cron, when both the day of the month and the day of the week are set, a day matching either one runs. Times are in the time zone set under **Automations → Settings**, or in one you pass:

```lua
sideporch.cron("0 9 * * mon", { timezone = "America/New_York" }, function()
  sideporch.post("general", "New week, new ideas. What's on your list?")
end)
```

`sideporch.every(seconds, handler)` runs every so many seconds instead, at least 10. The first run comes one interval after the script loads, and every save restarts the count.

```lua
sideporch.every(300, function()
  print("five minutes passed")
end)
```

The editor's **Listens to** panel shows when each schedule runs next.

## Slash commands

`sideporch.command(name, handler)` adds `/name`, which anyone can type in a channel or conversation. The message box suggests it, and `/help` lists it with its description.

```lua
sideporch.command("oncall", { description = "Who is on call this week", usage = "[team]" }, function(cmd)
  local team = cmd.args[1] or "platform"
  local schedule = { platform = "@ada", web = "@linus" }
  sideporch.respond(cmd, "On call for **" .. team .. "**: " .. (schedule[team] or "nobody yet"))
end)
```

The handler gets `cmd.text` (everything after the name), `cmd.args` (its words), who typed it (`cmd.user`, `cmd.username`) and where (`cmd.channel`, `cmd.channel_id`, `cmd.thread_id`). `sideporch.respond` answers only the person who typed it; `sideporch.post` and `sideporch.reply(cmd, text)` answer for everyone. A command name belongs to one automation; a second automation can't take it.

## Webhooks

Each automation has its own URL, shown in the editor under **Webhook**. `sideporch.on_webhook(handler)` answers requests to it. Monitors, CI, forms and other services can call it with any method.

```lua
sideporch.on_webhook(function(request)
  if request.method ~= "POST" or not request.json then
    return { status = 400, body = "Send JSON" }
  end
  sideporch.post("deploys", "Deploying **" .. tostring(request.json.service) .. "**")
  return { json = { ok = true } }
end)
```

The handler gets the `method`, the `path` below the URL (`/deploy` for `…/hooks/automations/<token>/deploy`), the `query` parameters, the `headers` (lower-case names), the raw `body`, and `json` when the body is JSON. What it returns becomes the response:

| The handler returns | Sideporch answers |
| --- | --- |
| nothing | 204 No Content |
| a string | 200 with that text |
| `{ status = 202, body = "queued" }` | that status and text |
| `{ json = { ok = true } }` | 200 with that JSON |
| `{ status = 202, json = { id = 7 } }` | that status and JSON |
| `{ body = "<p>Hi</p>", content_type = "text/html" }` | the text with that content type |

Requests wait up to 15 seconds for the script. One script registers one webhook handler; use `request.path` to tell several kinds of requests apart.

For services that speak Slack's webhook format, such as monitors like Gatus, an [incoming webhook](@/docs/integrations/incoming-webhooks.md) is simpler: no script needed.
