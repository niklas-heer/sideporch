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

- Channels anyone can join, direct messages, and threads one level deep.
- Messages appear live over a WebSocket, with unread markers in the sidebar.
- Slack-style formatting: `*bold*`, `_italic_`, `` `code` ``, code blocks, `>` quotes, links, `@mentions`, and `:emoji:` codes.
- **Search** across every channel and conversation you're part of (SQLite full-text search).
- **File uploads**: images show inline; other files download. Files live in the same database as everything else.
- **Reactions** with emoji, and **custom emoji** anyone can add, like Slack's.
- **Push notifications** for direct messages, thread replies and mentions, sent by Sideporch itself through Web Push. On iPhone and iPad, add Sideporch to the home screen first.
- **Automations**: admins write small Lua scripts in the browser that answer messages, post on a schedule, and remember data. They run sandboxed, with limits on time, memory and posts.
- A one-time setup link for the first account, then invite links. No email needed.
- Slack-compatible incoming webhooks per channel, tested against the payloads Gatus sends.
- Works on phones, in dark mode, and without JavaScript (pages reload instead of updating live).

## Run it

```sh
cargo build --release
./target/release/sideporch --data /var/lib/sideporch --public-url https://chat.example.com
```

On first start, Sideporch prints a setup link. Open it to create the first account, which is an admin. Then open **People** to create invite links for everyone else.

| Option | Environment variable | Default | Meaning |
| --- | --- | --- | --- |
| `--listen` | `SIDEPORCH_LISTEN` | `127.0.0.1:8080` | Address and port to listen on. |
| `--data` | `SIDEPORCH_DATA` | `sideporch-data` | Directory for the SQLite database. |
| `--public-url` | `SIDEPORCH_PUBLIC_URL` | derived from requests | The URL people use, for invite and webhook links. An `https://` URL also turns on secure cookies. |

Put Sideporch behind a reverse proxy such as Caddy for HTTPS. The proxy must pass WebSocket upgrades for `/ws`.

### Back up and move

Everything, including uploaded files, lives in one SQLite database in the data directory. Stop Sideporch and copy the directory, or copy it live with `sqlite3 sideporch-data/sideporch.db ".backup backup.db"`. [Litestream](https://litestream.io) can replicate it continuously.

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
mise run dev    # run locally with data in ./data
mise run check  # formatting, Clippy, unit and end-to-end tests
```

The web interface is rendered on the server with [maud](https://maud.lambda.xyz). Styles are Tailwind-style utility classes compiled at build time by [encre-css](https://gitlab.com/encre-org/encre-css), a Rust implementation of Tailwind, so there is no Node toolchain. `assets/app.js` is the only page script; `assets/sw.js` shows push notifications.

## Automations

Open the lightning icon in the sidebar (admins only) and create an automation:

```lua
sideporch.on_message(function(msg)
  if msg.text == "!ping" then
    sideporch.reply(msg, "pong")
  end
end)

sideporch.every(24 * 60 * 60, function()
  sideporch.post("general", "Good morning, porch! :sunny:")
end)
```

Scripts see new messages in public channels (`msg.text`, `msg.author`, `msg.username`, `msg.channel`, `msg.id`, `msg.thread_id`, `msg.is_bot`) and can `post`, `reply`, and keep data with `sideporch.get` and `sideporch.set`. They cannot read files or reach the network.

## Decisions

Lasting choices are recorded in [decisions/](decisions/) using [vrdx](https://github.com/niklas-heer/vrdx).

## License

[MIT](LICENSE). Icons are from [Phosphor](https://phosphoricons.com) (MIT). The embedded [Atkinson Hyperlegible Next](https://github.com/googlefonts/atkinson-hyperlegible-next) font is under the [SIL Open Font License](assets/fonts/OFL.txt).
