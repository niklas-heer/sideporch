+++
title = "Data, secrets, HTTP and libraries"
description = "Keep data between runs, keep API tokens out of scripts, call other services, and share code between automations."
weight = 4
+++

## Keeping data

Each run starts fresh: local variables are gone when a handler returns, and a save or restart reloads the script. To remember something, save it with `sideporch.set(key, value)` and read it back with `sideporch.get(key)`. Each automation has its own store, which survives restarts, edits and updates.

```lua
sideporch.command("coffee", { description = "Count the coffees" }, function(cmd)
  local count = tonumber(sideporch.get("coffees") or "0") + 1
  sideporch.set("coffees", count)
  sideporch.respond(cmd, "That's coffee number " .. count .. " today. :coffee:")
end)

sideporch.cron("0 0 * * *", function()
  sideporch.set("coffees", nil) -- start over at midnight
end)
```

Values are strings; numbers and booleans are turned into strings, so read numbers back with `tonumber`. Setting `nil` deletes a key. For tables, encode them as JSON:

```lua
sideporch.on("message", { channel = "ideas" }, function(msg)
  local ideas = sideporch.json.decode(sideporch.get("ideas") or "[]")
  table.insert(ideas, { by = msg.author, text = msg.text, at = sideporch.now() })
  sideporch.set("ideas", sideporch.json.encode(ideas))
  sideporch.react(msg, "bulb")
end)
```

Keys take up to 200 bytes and values up to 64 kB. `sideporch.now()` is the time in seconds, for timestamps like the one above. The data lives in the database, so [backups](@/docs/community/server/backups.md) include it; test runs only change a copy.

## Secrets

API tokens don't belong in scripts, where anyone who can read the script, its history or an exported file sees them. Store them under **Automations → Secrets** instead, and read them with `sideporch.secret`:

```lua
sideporch.command("status", { description = "Is the website up?" }, function(cmd)
  local token = sideporch.secret("STATUS_TOKEN")
  if not token then
    sideporch.respond(cmd, "Add a secret named STATUS_TOKEN first.")
    return
  end
  local response = sideporch.http.get("https://status.example.com/api/summary", {
    headers = { authorization = "Bearer " .. token },
  })
  sideporch.respond(cmd, response.ok and "All good." or ("Status page says " .. response.status))
end)
```

Secrets are encrypted in the database with a key kept outside it: `secret.key` in the data directory, or the `SIDEPORCH_SECRET_KEY` environment variable. Keep that key with your backups; without it, stored secrets can't be read. Environment variables named `SIDEPORCH_SECRET_<NAME>` work as secrets too, and win over stored ones, which suits servers configured from outside.

The Secrets page only ever shows names, never values. Values that appear in a script's output are replaced in run logs and test runs. Every automation can read every secret.

## Calling other services

`sideporch.http` makes requests to other services and reads their answers:

```lua
sideporch.command("fact", { description = "A random cat fact" }, function(cmd)
  local ok, response = pcall(sideporch.http.get, "https://catfact.ninja/fact")
  if not ok or not response.ok then
    sideporch.respond(cmd, "The cats are asleep. Try again later.")
    return
  end
  sideporch.post(cmd.channel, ":cat: " .. response.json.fact)
end)
```

- `sideporch.http.get(url, options)` and `sideporch.http.post(url, body, options)`; a table body is sent as JSON. `sideporch.http.request({ method = "PATCH", url = …, json = … })` sends anything else.
- `options` takes `headers` and a `timeout` in seconds: 10 by default, at most 30.
- The response has `status`, `ok` (true for 2xx), `headers` with lower-case names, the `body` as text, and `json` when the body is JSON.
- A server that can't be reached raises an error, which stops the handler. Wrap calls in `pcall`, as above, to handle that yourself.
- Redirects aren't followed, and each handler call may make 10 requests.

By default, requests to private, loopback and link-local addresses are refused, so a script can't reach the server's own network or the machine it runs on. To call something on your LAN, such as Home Assistant, turn on **Let automations reach private networks** under **Automations → Settings**.

## JSON

`sideporch.json.encode(value)` turns a Lua value into JSON, and `sideporch.json.decode(text)` reads JSON into Lua values; invalid JSON raises an error. Lua has a single kind of table for lists and objects, so an empty table encodes as `{}`; make it with `sideporch.json.array()` to get `[]`. JSON's `null` decodes to `sideporch.json.null`.

## Libraries

When several automations need the same code, such as a client for an API, put it in a library: **New library** on the Automations page. A library is a Lua module; it returns a table of functions:

```lua
-- library: greetings
local greetings = {}

local openers = { "Welcome", "Good to see you", "Make yourself at home" }

function greetings.hello(name)
  return openers[math.random(#openers)] .. ", " .. name .. "! :wave:"
end

return greetings
```

Automations load it with `require` and the library's name:

```lua
local greetings = require("greetings")

sideporch.on("member_joined", function(event)
  sideporch.post("general", greetings.hello(event.user))
end)
```

A library loads once per automation, the first time it's required, and can use the whole `sideporch` API, acting for the automation that loaded it. Saving a library restarts the automations that use it. Test-running a library lists what it exports.

Exporting an automation brings the libraries it requires along; see [Sharing](@/docs/integrations/automations/sharing.md).
