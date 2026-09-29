+++
title = "Your first automation"
description = "From an empty editor to a script that answers in your chat, step by step."
weight = 1
+++

This walks through a small automation: it answers `!ping` in any channel, and gives everyone a `/roll` command. You need to be an admin.

## 1. Open the editor

Open the account menu at the bottom of the sidebar and choose **Automations**, then **New automation**. The editor opens with an example script; select it all and delete it.

Give the automation a **name**, such as `Porch butler`. Its messages and reactions appear under this name.

## 2. React to a message

Type this into the editor:

```lua
sideporch.on("message", { pattern = "^!ping" }, function(msg)
  sideporch.reply(msg, "Pong! :ping_pong:")
end)
```

`sideporch.on` says what to listen for, here new messages, and the filter narrows it down: `pattern` is a [Lua pattern](https://www.lua.org/manual/5.4/manual.html#6.4.1), and `^!ping` means the message starts with `!ping`. The function runs each time, with the message as `msg`. `sideporch.reply` answers in the message's thread.

While you type, the editor completes `sideporch.` functions (press <kbd>Ctrl</kbd>+<kbd>Space</kbd> to ask for suggestions) and underlines mistakes. Below the editor, each problem is listed with its line; click one to jump there.

## 3. Test it

Press **Test** above the editor (or <kbd>Ctrl</kbd>+<kbd>Enter</kbd>) to get to the **Test run** panel. Choose **A new message**, type `!ping` as the message text, and press **Run test**.

The result lists what the script would do (`→ post in #general (thread …): Pong! …`) without doing it: nothing is posted, and no data changes. Try `hello` instead, and the test says *No handler matched this event*, because the pattern didn't match.

## 4. Add a command

Slash commands are the other common start. Add this below the first handler:

```lua
sideporch.command("roll", { description = "Roll a die", usage = "[sides]" }, function(cmd)
  local sides = tonumber(cmd.args[1]) or 6
  local rolled = math.random(1, sides)
  sideporch.post(cmd.channel, cmd.user .. " rolled a **" .. rolled .. "** (d" .. sides .. ")")
end)
```

Anyone can now type `/roll` or `/roll 20` in a channel. `sideporch.post` answers for everyone to see; `sideporch.respond(cmd, text)` would answer only the person who typed it. Test it with **A slash command** and `/roll 20`.

## 5. Save and switch it on

Tick **Run this automation** and press **Save automation** (or <kbd>Ctrl</kbd>+<kbd>S</kbd>). The **Listens to** panel now shows *on message matching `^!ping`* and the `/roll` command. Go to a channel and write `!ping`, or type `/roll`.

If something goes wrong while it runs, the editor shows the error at the top and in **Recent runs**, with the line it happened on. Anything the script `print`s shows up there too.

## Next

- The [editor](@/docs/integrations/automations/editor.md) has more: formatting, history, a webhook URL and AI help.
- [Events, schedules, commands and webhooks](@/docs/integrations/automations/triggers.md) lists everything a script can react to.
- The [examples](@/docs/integrations/automations/examples.md) are ready to copy.
