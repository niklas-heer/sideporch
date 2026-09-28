<p align="center"><img src="assets/logo.svg" alt="" width="96" height="96"></p>

# Sideporch

Sideporch is a small, self-hosted team chat: channels, direct messages, and threads for a team, a club, or a family. It ships as one binary you copy to a server and run.

> **Status:** early. It works end to end, but expect rough edges and breaking changes before 1.0.

## Goals

- **One binary.** The server, realtime WebSocket connection, and web interface ship as a single self-contained Rust executable. No external database, message broker, cache, or container runtime.
- **Easy to run and move.** Your data should be easy to back up, copy to another server, and replicate.
- **Built for small groups.** Invite links instead of an email server, and sensible defaults instead of configuration.
- **Works with your existing tools.** Monitors, CI systems, and bots post into channels through webhooks. Services that already speak Slack's incoming-webhook format, such as [Gatus](https://gatus.io/) and Grafana, work unchanged.

## Features

- Public channels anyone can join, **private channels** for the people you add, direct messages, and threads one level deep. Leave channels you don't need, and **mute** noisy ones.
- Messages appear live over a WebSocket, with unread markers in the sidebar.
- GitHub-flavored Markdown: headings, lists and task lists, tables, code blocks, quotes and alerts, links, `@mentions`, `:emoji:` codes, and [Mermaid](https://mermaid.js.org) diagrams in ```` ```mermaid ```` blocks. Webhook posts keep Slack's own formatting.
- **Edit and delete** your messages (↑ in an empty composer edits your last one), **pin** messages to a channel, and **save** messages for later.
- **Activity**: mentions and replies in your threads, in one place. See who is typing, and move around with the keyboard: ⌘K (Ctrl+K) jumps anywhere, Alt+↑/↓ switches channels, ⌘/ lists every shortcut.
- **Reminders and scheduled messages**: `/remind me tomorrow to water the plants`, “Remind me” on any message, and a clock next to Send to send a message later, all in your own time zone.
- **Link previews** with the title, description and image of the first link in a message, fetched safely by the server; admins can turn them off.
- **Search** across every channel and conversation you're part of (SQLite full-text search).
- **Files and images**: attach, paste or drop them into a message; images show inline, other files download. They are stored in the data directory, next to the database.
- **GIFs** from the team's own library, which anyone can add to, or, if an admin chooses, from [GIPHY](https://giphy.com) or [KLIPY](https://klipy.com) with a free API key.
- **Reactions** with every standard emoji, in a searchable picker grouped by category, with your favorites or most used emoji first, plus **custom emoji** anyone can add.
- **Profiles** with a picture, a status, a bio and links.
- **A system page** for admins: CPU and memory with a short history, database and file sizes, free disk space, and activity.
- **Push notifications** for direct messages, thread replies and mentions, sent by Sideporch itself through Web Push. On iPhone and iPad, add Sideporch to the home screen first.
- **Automations**: Lua scripts that react to events, run on cron schedules, answer slash commands and webhooks, call APIs with encrypted secrets, and share code through libraries. They are written in an editor with a linter, formatter, test runs and version history, and AI can write them, from the editor or through MCP.
- **Move from Slack**: import a workspace export with its people, channels, direct messages, threads and reactions.
- Set up in the browser: the first visitor creates the admin account, then invites everyone else. No email needed: admins hand out password reset links, make other people admins, and deactivate accounts.
- Slack-compatible incoming webhooks per channel, tested against the payloads Gatus sends, and **outgoing webhooks** that send people's messages (optionally only those starting with a trigger word) to another service and post its answer.
- **Polls** with `/poll Where do we eat? | Pizza | Tacos`, one vote per person.
- Works on phones, in dark mode, and without JavaScript (pages reload instead of updating live).

## Install

Every release has prebuilt binaries for Linux and macOS, on x86-64 and ARM64. The Linux binaries are fully static: they need no libc or any other library, so they run on any distribution.

```sh
# Script: detects your system, verifies the checksum, installs to /usr/local/bin or ~/.local/bin
curl -fsSL https://raw.githubusercontent.com/niklas-heer/sideporch/main/install.sh | sh

# Homebrew (macOS and Linux)
brew install niklas-heer/tap/sideporch

# Docker: a 5 MB image with nothing but the binary
docker run -d -p 8080:8080 -v sideporch:/data ghcr.io/niklas-heer/sideporch

# Nix
nix run github:niklas-heer/sideporch
```

On NixOS, the flake provides a module: import `sideporch.nixosModules.default` and set `services.sideporch.enable = true;`. You can also build from source with `cargo build --release`.

## Run it

```sh
sideporch --data /var/lib/sideporch --public-url https://chat.example.com
```

Then open Sideporch in your browser. While no account exists, the first visitor creates the admin account; after that, new people need an invite link from **People**. No email is needed.

If others can reach the server before you set it up, start it with `--require-setup-link`. Setup then needs a one-time link that stays out of service and container logs. Get it on the server:

```sh
sideporch setup-link --data /var/lib/sideporch
docker exec <container> /sideporch setup-link
```

| Option | Environment variable | Default | Meaning |
| --- | --- | --- | --- |
| `--listen` | `SIDEPORCH_LISTEN` | `127.0.0.1:8080` | Address and port to listen on. |
| `--data` | `SIDEPORCH_DATA` | `sideporch-data` | Directory for the SQLite database. |
| `--public-url` | `SIDEPORCH_PUBLIC_URL` | derived from requests | The URL people use, for invite and webhook links. An `https://` URL also turns on secure cookies. |
| `--require-setup-link` | `SIDEPORCH_REQUIRE_SETUP_LINK` | off | Require the one-time link from `sideporch setup-link` to create the first account. |

Put Sideporch behind a reverse proxy such as Caddy for HTTPS. The proxy must pass WebSocket upgrades for `/ws`. Browsers only allow push notifications and installing Sideporch as an app over HTTPS.

### Back up and move

Everything lives in the data directory: the SQLite database `sideporch.db`, uploaded files and pictures in `files/` (named by their SHA-256, so they never change once written), and `secret.key`, which decrypts stored secrets.

The simplest way is **Admin → Backups**: download one archive with all of it while Sideporch keeps running, or let Sideporch write backups on a schedule to a directory (ideally another disk or a mounted volume) and keep the newest few. To restore, stop Sideporch and unpack an archive into an empty data directory:

```sh
sideporch restore sideporch-20260928-101500.tar.gz --data /path/to/data
```

You can also stop Sideporch and copy the directory. To back up while it runs, copy the database with `sqlite3 sideporch-data/sideporch.db ".backup backup.db"` and then the rest of the directory, for example with rsync or restic; files are only ever added, so copying them after the database is safe. [Litestream](https://litestream.io) can replicate the database continuously; back up `files/` and `secret.key` alongside it.

Sideporch 0.1 and 0.2 kept uploads inside the database. Newer versions move them to `files/` on the first start and then compact the database.

## Connect Gatus and other tools

In a channel, open the settings (the gear icon) and create a webhook. Copy its URL into the tool. For Gatus:

```yaml
alerting:
  slack:
    webhook-url: "https://chat.example.com/hooks/<token>"
```

Gatus's `mattermost` provider works too. Its `channel` setting posts to another public channel by name, and `username` and `icon_url` set the sender. Any tool that posts Slack-style `text` and `attachments` works the same way.

## Develop

Tool versions and tasks live in `mise.toml`:

```sh
mise run dev           # run locally with data in ./data
mise run check         # formatting, Clippy, unit and end-to-end tests
mise run ci            # the Linux CI pipeline in containers, through Dagger
mise run build-static  # static Linux binaries with zig and musl
mise run image         # build the container image and load it into Docker
```

CI runs through [Dagger](https://dagger.io) (`.dagger/main.dang`); the GitHub workflows only call it. Pushing a `vX.Y.Z` tag that matches `Cargo.toml` builds, tests and publishes a release, its container images, and the checksums that the Homebrew formula and `install.sh` verify.

The web interface is rendered on the server with [maud](https://maud.lambda.xyz). Styles are Tailwind-style utility classes compiled at build time by [encre-css](https://gitlab.com/encre-org/encre-css), a Rust implementation of Tailwind, so there is no Node toolchain. `assets/app.js` is the main page script, `assets/editor.js` powers the automation editor, and `assets/sw.js` shows push notifications.

## Automations

Admins write automations in Lua under the lightning icon in the sidebar. Like Slack workflows, each one says what should happen when something happens, but with a real language:

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

- **Events**: `message`, `message_changed`, `message_deleted`, `reaction_added`, `reaction_removed`, `member_joined` and `channel_created`, with filters for the channel, a text pattern, the emoji, the person, and threads. Automations see public channels only.
- **Buttons**: `sideporch.post` and `sideporch.reply` can put up to five buttons under a message; clicks arrive as `button` events for that automation, which can change its message with `sideporch.update`, for approvals and the like.
- **Schedules**: `sideporch.cron` with standard five-field expressions (`*/15 * * * *`, `@daily`, `mon-fri`) in the instance's time zone or one you name, and `sideporch.every(seconds, …)`.
- **Slash commands**: `/name` in any channel runs the automation that registered it. Answers from `sideporch.respond` are visible only to the person who typed the command; `/help` lists all commands, and the composer suggests them.
- **Outgoing HTTP**: `sideporch.http.get`, `.post` and `.request` call APIs and parse JSON. Requests to private, loopback and link-local addresses are refused unless an admin allows them under *Settings*, so scripts cannot probe the server's network.
- **Secrets**: API tokens live under *Secrets*, encrypted in the database with a key kept outside it (`secret.key` in the data directory, or `SIDEPORCH_SECRET_KEY`). Environment variables named `SIDEPORCH_SECRET_<NAME>` work too. Scripts read them with `sideporch.secret("NAME")`, and their values are hidden in run logs.
- **Libraries**: a library is shared code, for example a client for an API, that automations load with `require("name")`.
- **Data and messages**: `sideporch.get`/`set` keep data between runs, `sideporch.json` reads and writes JSON, and messages are GitHub-flavored Markdown.

Each automation runs on its own thread in a sandbox, with limits on instructions, memory, posts and requests per run, and no file or process access. The editor lists the full API.

The editor is made for writing and debugging automations:

- Syntax highlighting, completions for the `sideporch` API, and auto-indentation.
- A linter ([selene](https://github.com/Kampfkarren/selene)) that knows the sandbox, and a formatter ([StyLua](https://github.com/JohnnyMorganz/StyLua)).
- **Test runs** against a simulated message, reaction, command, webhook request, schedule, new member or new channel. They show what the script prints, posts and answers, without changing anything in Sideporch.
- What each automation **listens to**, with its next scheduled runs, a **run log**, a **history** of every saved version with one-click restore, and its **webhook URL**.

Scripts, libraries, their history and their data all live in the SQLite database, so a backup of it covers automations too. Keep `secret.key` with it.

### Let AI write automations

- **In the editor**: an admin connects a model under *Automations → AI provider*, either Anthropic's API or any OpenAI-compatible one (OpenAI, OpenRouter, Ollama, …). Then *Ask AI* writes or changes the script from a description, using your libraries. Sideporch lints and test-loads the answer, sends problems back to the model, and shows the result for review; nothing is saved until you choose to.
- **From your own agent**: Sideporch is an [MCP](https://modelcontextprotocol.io) server at `/mcp`. Create a token under *Automations → MCP*, then connect, for example with Claude Code:

  ```sh
  claude mcp add --transport http sideporch https://chat.example.com/mcp \
    --header "Authorization: Bearer sp_…"
  ```

  Agents get what a developer needs: the API reference, channels and recent messages, lint, format, dry-run tests and live runs, run logs, versions and restore, saved data, write-only secrets, settings, and cron previews. Scripts are also available as MCP resources. Automations an agent creates start switched off, and every change is kept in the history under the token's name.

## Decisions

Lasting choices are recorded in [decisions/](decisions/) using [vrdx](https://github.com/niklas-heer/vrdx).

## License

[MIT](LICENSE). Diagrams are drawn by the embedded [Mermaid](https://mermaid.js.org) (MIT; see `assets/vendor/README.md`). Icons are from [Phosphor](https://phosphoricons.com) (MIT). The embedded [Atkinson Hyperlegible Next](https://github.com/googlefonts/atkinson-hyperlegible-next) font is under the [SIL Open Font License](assets/fonts/OFL.txt).
