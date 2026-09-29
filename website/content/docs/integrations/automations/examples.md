+++
title = "Examples"
description = "Complete automations to copy: welcomes, standups, approvals, webhooks from GitHub, uptime checks, kudos, a weekly digest and more."
weight = 5
+++

Each example is a complete automation: paste it into a new automation, change the channel names to yours, test it, and switch it on. Every script on this page is checked against the linter and loaded in the sandbox whenever Sideporch's tests run, so they keep working as Sideporch changes.

## Welcome new members

Greets everyone who joins, points them to the right places, and tells a moderator channel.

```lua
local CHANNELS = { "general", "introductions", "help" }

sideporch.on("member_joined", function(event)
  local links = {}
  for _, name in ipairs(CHANNELS) do
    table.insert(links, "#" .. name)
  end
  sideporch.post(
    "general",
    "Welcome to the porch, **" .. event.user .. "**! :wave:\n\n"
      .. "Say hi in #introductions, ask anything in #help, and have a look around: "
      .. table.concat(links, ", ")
  )
  sideporch.post("moderators", "New member: @" .. event.username)
end)
```

**Try it** with the test run **Someone joining**.

## A daily standup question

Asks the same question every weekday morning, and collects the answers in its thread.

```lua
sideporch.cron("0 9 * * mon-fri", { timezone = "Europe/Berlin" }, function()
  sideporch.post("standup", table.concat({
    "Good morning! :sunrise:",
    "",
    "Answer in this thread:",
    "1. What did you do yesterday?",
    "2. What are you doing today?",
    "3. Anything in your way?",
  }, "\n"))
end)
```

**Try it** with **Schedules firing**. The **Listens to** panel shows the next morning it runs.

## Answers to common questions

Replies to questions the team asks again and again. Add your own pairs of a pattern and an answer.

```lua
local ANSWERS = {
  { pattern = "wifi", answer = "The guest Wi-Fi is **porch-guest**; the password is on the fridge." },
  { pattern = "vpn", answer = "Set up the VPN with the guide in #it-help's pinned messages." },
  { pattern = "holiday", answer = "Book time off in the HR tool and tell your team in #general." },
}

sideporch.on("message", { thread = false }, function(msg)
  local text = msg.text:lower()
  if not text:find("?", 1, true) then
    return -- only questions
  end
  for _, entry in ipairs(ANSWERS) do
    if text:find(entry.pattern) then
      sideporch.reply(msg, entry.answer)
      return
    end
  end
end)
```

**Try it** with **A new message** and `Where do I find the wifi password?`.

## Highlights from reactions

When someone reacts with :star: to a message, the automation copies it to #highlights, once.

```lua
sideporch.on("reaction_added", { emoji = "star" }, function(event)
  local message = event.message
  local key = "posted:" .. message.id
  if message.channel == "highlights" or sideporch.get(key) then
    return
  end
  sideporch.set(key, "yes")
  local quote = message.text:gsub("\n", "\n> ")
  sideporch.post(
    "highlights",
    "> " .. quote .. "\n\n— **" .. message.author .. "** in #" .. message.channel
      .. ", starred by " .. event.user
  )
end)
```

**Try it** with **A reaction**, emoji `star`.

## Kudos

`/kudos @ada for fixing the build` thanks someone in public and counts it; `/kudos top` shows who got the most.

```lua
local function load()
  return sideporch.json.decode(sideporch.get("kudos") or "{}")
end

sideporch.command("kudos", { description = "Thank someone", usage = "@name for what | top" }, function(cmd)
  local counts = load()
  if cmd.args[1] == "top" then
    local ranking = {}
    for name, count in pairs(counts) do
      table.insert(ranking, { name = name, count = count })
    end
    table.sort(ranking, function(a, b)
      return a.count > b.count
    end)
    local lines = { "**Kudos so far**" }
    for place = 1, math.min(5, #ranking) do
      local entry = ranking[place]
      table.insert(lines, place .. ". @" .. entry.name .. ": " .. entry.count)
    end
    sideporch.respond(cmd, table.concat(lines, "\n"))
    return
  end

  local name = cmd.text:match("^@?([%w_%.%-]+)")
  if not name or name == cmd.username then
    sideporch.respond(cmd, "Usage: `/kudos @name for what they did`")
    return
  end
  local reason = cmd.text:match("for (.+)$") or "being great"
  counts[name] = (counts[name] or 0) + 1
  sideporch.set("kudos", sideporch.json.encode(counts))
  sideporch.post(cmd.channel, ":sparkles: " .. cmd.user .. " thanks @" .. name .. " for " .. reason)
end)
```

**Try it** with **A slash command**: `/kudos @linus for the release notes`, then `/kudos top`. Test runs work on a copy of the saved data, so the count starts over each time.

## Deploy approvals with buttons

Your CI calls the automation's webhook before deploying to production. The automation asks in #deploys with **Approve** and **Reject** buttons, and remembers the answer, which CI can poll at `…/status/<id>`.

```lua
sideporch.on_webhook(function(request)
  local id = request.path:match("^/status/(.+)$")
  if id then
    return { json = { decision = sideporch.get("decision:" .. id) or "pending" } }
  end
  local deploy = request.json
  if not deploy or not deploy.id or not deploy.service then
    return { status = 400, body = "Send { id, service, version }" }
  end
  sideporch.post("deploys", "**" .. deploy.service .. "** " .. tostring(deploy.version) .. " is ready for production.", {
    buttons = {
      { label = "Approve", value = "approve:" .. deploy.id, style = "primary" },
      { label = "Reject", value = "reject:" .. deploy.id, style = "danger" },
    },
  })
  return { status = 202, json = { id = deploy.id } }
end)

sideporch.on("button", function(click)
  local decision, id = click.value:match("^(%a+):(.+)$")
  sideporch.set("decision:" .. id, decision)
  local verdict = decision == "approve" and ":white_check_mark: Approved" or ":x: Rejected"
  sideporch.update(click.message, click.message.text .. "\n\n" .. verdict .. " by " .. click.user, { buttons = {} })
end)
```

The CI job sends `POST` with `{"id": "4711", "service": "api", "version": "v2.3"}` to the webhook URL from the editor, then checks `GET …/status/4711` until the decision isn't `pending`.

**Try it** with **A webhook request**: method `POST` and body `{"id": "1", "service": "api", "version": "v2.3"}`.

## GitHub pushes

Posts a summary of each push to a GitHub repository. In the repository's settings, add a webhook with the automation's URL, content type `application/json`, and the *push* event.

```lua
sideporch.on_webhook(function(request)
  if request.headers["x-github-event"] ~= "push" or not request.json then
    return "ignored"
  end
  local push = request.json
  local branch = (push.ref or ""):gsub("^refs/heads/", "")
  local lines = {
    "**" .. push.pusher.name .. "** pushed " .. #push.commits .. " commit(s) to `" .. branch .. "` of "
      .. push.repository.full_name,
  }
  for index, commit in ipairs(push.commits) do
    if index > 5 then
      table.insert(lines, "- …and " .. (#push.commits - 5) .. " more")
      break
    end
    local title = commit.message:match("^[^\n]*")
    table.insert(lines, "- [" .. commit.id:sub(1, 7) .. "](" .. commit.url .. ") " .. title)
  end
  sideporch.post("code", table.concat(lines, "\n"))
  return "ok"
end)
```

The webhook URL is the only thing that lets a request in, so keep it private. **Make a new URL** in the editor if it ever leaks.

## Is the website up?

Checks a website every five minutes and says so when it goes down or comes back, not on every check.

```lua
local URL = "https://www.example.com/"

sideporch.every(300, function()
  local ok, response = pcall(sideporch.http.get, URL, { timeout = 10 })
  local up = ok and response.status < 500
  local was = sideporch.get("up")
  if was == nil then
    sideporch.set("up", up)
    return
  end
  if tostring(up) ~= was then
    sideporch.set("up", up)
    if up then
      sideporch.post("alerts", ":white_check_mark: " .. URL .. " is back.")
    else
      local reason = ok and ("status " .. response.status) or "no answer"
      sideporch.post("alerts", ":rotating_light: " .. URL .. " is down (" .. reason .. ").")
    end
  end
end)
```

For monitors that already send alerts, such as Gatus or Uptime Kuma, an [incoming webhook](@/docs/integrations/incoming-webhooks.md) is simpler.

**Try it** with **Schedules firing**, and **Make real HTTP requests** on.

## Weather with a library

A library that talks to [Open-Meteo](https://open-meteo.com), which needs no API key, and a `/weather` command that uses it. Create the library first, named `weather`:

```lua
-- library: weather
local weather = {}

local DESCRIPTIONS = {
  [0] = "clear", [1] = "mostly clear", [2] = "partly cloudy", [3] = "overcast",
  [45] = "foggy", [61] = "light rain", [63] = "rain", [65] = "heavy rain",
  [71] = "light snow", [73] = "snow", [95] = "thunderstorms",
}

local function encode(text)
  return (text:gsub("[^%w%-_%.~]", function(c)
    return string.format("%%%02X", string.byte(c))
  end))
end

-- Today's weather in `city`, or nil and why not.
function weather.today(city)
  local found = sideporch.http.get(
    "https://geocoding-api.open-meteo.com/v1/search?count=1&name=" .. encode(city)
  )
  local place = found.json and found.json.results and found.json.results[1]
  if not place then
    return nil, "I don't know a place called " .. city .. "."
  end
  local forecast = sideporch.http.get(
    "https://api.open-meteo.com/v1/forecast?current=temperature_2m,weather_code&latitude="
      .. place.latitude .. "&longitude=" .. place.longitude
  )
  local now = forecast.json.current
  return {
    place = place.name,
    temperature = now.temperature_2m,
    summary = DESCRIPTIONS[now.weather_code] or "mixed",
  }
end

return weather
```

Then the automation:

```lua
local weather = require("weather")

sideporch.command("weather", { description = "Today's weather", usage = "<city>" }, function(cmd)
  local city = cmd.text ~= "" and cmd.text or "Berlin"
  local ok, today, problem = pcall(weather.today, city)
  if not ok then
    sideporch.respond(cmd, "The weather service didn't answer. Try again later.")
  elseif not today then
    sideporch.respond(cmd, problem)
  else
    sideporch.respond(cmd, "**" .. today.place .. "**: " .. today.summary .. ", " .. today.temperature .. " °C")
  end
end)
```

**Try it** with **A slash command**: `/weather Lisbon`.

## A weekly digest

Counts messages per channel through the week, and posts a summary on Friday afternoon.

```lua
sideporch.on("message", function(msg)
  local counts = sideporch.json.decode(sideporch.get("week") or "{}")
  counts[msg.channel] = (counts[msg.channel] or 0) + 1
  sideporch.set("week", sideporch.json.encode(counts))
end)

sideporch.cron("0 16 * * fri", function()
  local counts = sideporch.json.decode(sideporch.get("week") or "{}")
  local channels = {}
  for name, count in pairs(counts) do
    table.insert(channels, { name = name, count = count })
  end
  if #channels == 0 then
    return
  end
  table.sort(channels, function(a, b)
    return a.count > b.count
  end)
  local lines = { "**This week on the porch** :bar_chart:" }
  for index = 1, math.min(5, #channels) do
    table.insert(lines, "- #" .. channels[index].name .. ": " .. channels[index].count .. " messages")
  end
  table.insert(lines, "\nHave a good weekend!")
  sideporch.post("general", table.concat(lines, "\n"))
  sideporch.set("week", nil)
end)
```

Members who may see [statistics](@/docs/using/statistics.md) find more numbers there, any time.

## More ideas

- Remind a channel of a monthly event: `sideporch.cron("0 10 * * mon", …)` runs every Monday, and the handler posts only when `sideporch.now()` falls in the first seven days of the month.
- Turn `!todo buy milk` messages into a list kept with `sideporch.set`, and show it with `/todo`.
- Post new entries from an RSS or JSON feed every hour, remembering the newest one you posted.
- Ask a question with buttons (`Yes`, `No`, `Maybe`) and update the message with the tally on each click.

To have a model write the first version for you, describe the automation to [Ask AI](@/docs/integrations/automations/ai-and-mcp.md) in the editor.
