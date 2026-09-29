+++
title = "API reference"
description = "Every sideporch function automations can call, and the tables their handlers receive."
weight = 9
aliases = ["/docs/integrations/automation-api/"]

[extra]
edit_path = "src/automations/api.rs"
+++

<!-- Generated from src/automations/api.rs. Change the API there, then run
     SIDEPORCH_BLESS=1 cargo test website_page_is_current -->

{% raw %}
Each handler call may run 2,000,000 Lua instructions, post 20 messages or reactions and make 10 HTTP requests; each script may use 16 MB of memory. Scripts have Lua 5.4's `string`, `table`, `math`, `utf8` and `coroutine` libraries, the base functions and `require` for libraries, but no file or process access; the network is reachable only through `sideporch.http`. Messages are GitHub-flavored Markdown. `print(...)` writes to the automation's run log.

## Functions

- `sideporch.on(event, filter_or_handler, handler?)`: Calls `handler(event)` when `event` happens: `"message"` (new messages in public channels), `"message_changed"` (edited; the table has the new text), `"message_deleted"` (the table has what it said), `"reaction_added"`, `"reaction_removed"`, `"reaction"` (both), `"member_joined"`, `"channel_created"` or `"button"` (someone clicked a button under one of this automation's messages; only the automation that posted it hears about it, and `pattern` matches the button's value). An optional filter table between them narrows it down: `channel` (name), `pattern` (a Lua pattern the message text must match), `emoji`, `user` (username) and `thread` (`true` for replies in threads, `false` for the rest). Posts and reactions from automations never trigger handlers. The event table's `event` field names the event.
- `sideporch.cron(expression, options_or_handler, handler?)`: Calls `handler()` on a cron schedule: `minute hour day-of-month month day-of-week`, with ranges, lists, steps (`*/15`), names (`mon-fri`, `jan`) and shortcuts such as `@hourly` and `@daily`. Times are in the instance's automation time zone, or pass `{ timezone = "Europe/Berlin" }` before the handler.
- `sideporch.command(name, options_or_handler, handler?)`: Registers the slash command `/name`, which people type in any channel. `handler(cmd)` gets `cmd.text` (everything after the name), `cmd.args` (its words), `cmd.user`, `cmd.username`, `cmd.channel` and `cmd.channel_id`. Answer privately with `sideporch.respond`, or publicly with `sideporch.post` or `sideporch.reply(cmd, …)`. Options: `description` and `usage` for `/help` and the composer's suggestions. Each command name belongs to one automation.
- `sideporch.respond(cmd, text)`: Answers a slash command with Markdown that only the person who typed it sees.
- `sideporch.on_message(handler)`: Calls `handler(msg)` for every new message in a public channel. Messages from automations never trigger it.
- `sideporch.on_reaction(handler)`: Calls `handler(event)` when someone adds or removes a reaction on a message in a public channel.
- `sideporch.on_webhook(handler)`: Calls `handler(request)` for each HTTP request to this automation's webhook URL, shown in the editor. What the handler returns becomes the response. A script can register one webhook handler.
- `sideporch.every(seconds, handler)`: Calls `handler()` every `seconds` seconds, at least 10. The first call comes one interval after the script loads.
- `sideporch.post(channel, text, options?)`: Posts `text` to the public channel named `channel` (with or without `#`), under the automation's name. `options.thread` is a message id to answer in that thread; `options.buttons` adds up to five buttons, each `{ label = "Approve", value = "approve", style = "primary" }` (style is `primary`, `danger` or left out), which people click to trigger `sideporch.on("button", …)`. Text is GitHub-flavored Markdown, such as `**bold**`, `[a link](https://example.com)`, tables, and ```` ```mermaid ```` diagrams.
- `sideporch.reply(message, text, options?)`: Answers `message` in its thread. `message` is a message table from `on_message` or a reaction event's `message`. `options.buttons` works as for `sideporch.post`.
- `sideporch.update(message, text?, options?)`: Changes a message this automation posted, such as `click.message` in a button handler: new text (or `nil` to keep it), and `options.buttons` to replace its buttons (`{}` removes them).
- `sideporch.react(message, emoji)`: Adds the reaction `emoji` (a name such as `"white_check_mark"` or a custom emoji) to `message`. Reacting twice with the same emoji has no further effect.
- `sideporch.get(key)`: Returns the string saved under `key` for this automation, or `nil`. Saved data survives restarts and edits.
- `sideporch.set(key, value?)`: Saves `value` under `key` as a string (numbers and booleans are converted; use `sideporch.json.encode` for tables). `nil` deletes the key. Keys take up to 200 bytes and values up to 64 kB.
- `sideporch.now()`: Returns the current Unix time in seconds.
- `sideporch.http.get(url, options?)`: Sends a GET request and returns the response: `status`, `ok` (true for 2xx), `headers` (lower-case names), `body`, and `json` when the body is JSON. Options: `headers` and `timeout` (seconds, default 10, at most 30). Network errors raise an error; use `pcall` to handle them. Redirects are not followed. Private and loopback addresses are refused unless an admin allows them.
- `sideporch.http.post(url, body?, options?)`: Sends a POST request. A table body is sent as JSON; a string as is. Returns the same response table as `sideporch.http.get`.
- `sideporch.http.request(options)`: Sends any request: `method`, `url`, `headers`, and `body` (a string) or `json` (a table), plus `timeout`. A call may make 10 requests.
- `sideporch.secret(name)`: Returns the secret `name`, such as an API token, or `nil`. Admins store secrets under Automations → Secrets, or set `SIDEPORCH_SECRET_<NAME>` in the environment. Secret values are replaced in run logs.
- `require(name)`: Loads the library automation `name` once and returns what it returns, usually a table of functions. Libraries use the same `sideporch` API, acting for the automation that loaded them.
- `sideporch.json.encode(value)`: Returns `value` as a JSON string. Empty tables become `[]` only when made with `sideporch.json.array()`; otherwise `{}`.
- `sideporch.json.decode(text)`: Parses a JSON string into Lua values. JSON `null` becomes `sideporch.json.null`. Fails on invalid JSON; use `pcall` to handle that.
- `sideporch.json.array(items?)`: Marks a table as a JSON array, so an empty one encodes as `[]`.

## Tables

### msg (on_message, reply, react)

- `id`: message id
- `channel`: channel name, without `#`
- `channel_id`: channel id
- `text`: the message text as written
- `author`: display name of the author
- `username`: the author's username; `nil` for bots and webhooks
- `is_bot`: `true` for webhook posts
- `thread_id`: id of the thread's first message, or `nil` outside threads

### event (member_joined)

- `user`: display name of the new member
- `username`: their username

### event (channel_created)

- `channel`: the new channel's name
- `channel_id`: its id
- `user`: display name of who created it
- `username`: their username

### cmd (command)

- `name`: the command, without `/`
- `text`: everything typed after the name
- `args`: the words of `text`, as a list
- `user`: display name of who typed it
- `username`: their username
- `channel`: the channel's name, or `""` in direct messages
- `channel_id`: the channel's id
- `thread_id`: the thread it was typed in, or `nil`

### click (button)

- `value`: the button's value
- `label`: the button's label
- `user`: display name of who clicked it
- `username`: their username
- `message`: the message with the button, as a `msg` table

### response (sideporch.http)

- `status`: HTTP status code
- `ok`: `true` for 2xx statuses
- `headers`: table of headers, names in lower case
- `body`: the body as a string
- `json`: the body parsed, when it is JSON

### event (on_reaction)

- `emoji`: emoji name, such as `"thumbsup"`
- `added`: `true` when added, `false` when removed
- `user`: display name of the person reacting
- `username`: their username
- `message`: the message, as a `msg` table

### request (on_webhook)

- `method`: HTTP method, such as `"POST"`
- `path`: the part of the URL after the webhook token, such as `"/deploy"`, or `""`
- `query`: table of query parameters
- `headers`: table of headers, names in lower case
- `body`: the raw body as a string
- `json`: the body parsed as JSON, or `nil`

### webhook response (return value of the on_webhook handler)

- `nil`: answers 204 No Content
- `a string`: answers 200 with that text
- `{ status = 200, body = "text" }`: status code (default 200) and a text body
- `{ json = value }`: a JSON body
{% endraw %}
