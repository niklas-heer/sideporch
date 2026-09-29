# Typing, statistics, sharing automations, and better automation docs

Agreed in conversation on 2026-09-29.

## Intent

- People should notice when someone is writing to them.
- A community should see how it is doing: how much is said, where, and by whom. Admins decide who may see this; by default everyone with ordinary access (trust level 1) does.
- Admins should be able to share automations between servers: export one or several, import them elsewhere. The format should not close off a future automation store or plugin system hosted on sideporch.app.
- The automation documentation should teach: a nested structure, a first walkthrough, a thorough tour of the editor with a screenshot, and real examples.
- Screenshots carry the version they show, and the site and README always show the newest set.

## 1. Typing indicators (polish; the feature exists)

Typing already shows as "Ada is typing…" under the message box of the open channel or thread. Changes:

- Animated dots before the label, so it catches the eye.
- Direct conversations: the other people see a typing mark on the conversation in the sidebar, even when it isn't open. Only DMs, because their audience is a handful of people; channels stay limited to the people looking at them.
- Emptying the message box sends a stop, so the indicator clears at once instead of lingering for five seconds.
- Not carried to other servers: the federation outbox delivers in order and retries, which is wrong for a signal that is stale after seconds.

## 2. Statistics

- New permission `ViewStatistics` ("See statistics"), default trust level 1. Admins always see statistics. Admin → Permissions changes it like any other, including Roles only.
- `/statistics`, linked from the sidebar for people who may see it. A period switch: 7 days, 30 days, 12 months, all time.
- Contents: totals (messages, people who wrote, reactions, files); messages per day as a server-drawn bar chart; top people by messages and by reactions received; busiest channels; most used emoji; your own numbers and place.
- Counts public channels only, never private channels or direct messages. Automations, webhooks and people from other servers are not ranked. Deleted messages don't count.
- People may hide themselves from rankings (a setting on their profile settings page). They still count in totals.

## 3. Sharing automations

- A bundle is JSON: `{ "format": "sideporch-automations", "version": 1, "items": [ … ] }`. Each item: `kind` (`automation` or `library`), `name`, `source`, `secrets` (names found in `sideporch.secret("…")`), `requires` (libraries found in `require("…")`). Optional descriptive fields for a future store (`description`, `author`, `license`, `homepage`) are accepted and kept out of the way.
- Never exported: secret values, stored data, run logs, webhook tokens, history.
- Export from the automation list (pick several, or all) and from the editor. Exporting an automation brings the libraries it requires.
- Import: upload or paste a bundle, then a preview lists every item as new or clashing with an existing name (skip, replace, or import as a copy), with the secrets still missing and the linter's findings. Imported automations start switched off.
- `docs/future-ideas.md` records the automation store / plugin idea. A decision record describes the format and why it is versioned.

## 4. Documentation

- The docs sidebar, breadcrumbs and previous/next links understand nested sections.
- Automations become a section of its own under Integrations: overview, your first automation, the editor, what automations react to, data/secrets/HTTP/libraries, examples, sharing, limits and safety, and the generated API reference. Old URLs keep working through aliases.
- "Run a community" is grouped into nested sections too.
- A test lints every Lua example in the docs, so they stay correct.
- New docs page for statistics; chatting mentions the typing changes.

## 5. Screenshots

- Screenshots live in `website/static/img/<version>/`. `mise run screenshots` writes into the directory for the version in `Cargo.toml` and removes older directories (git keeps them).
- The site reads the version from one place in `zola.toml`; the shot component builds paths from it and captions show "Sideporch X.Y.Z". The screenshot script rewrites the README's image paths and that setting.
- New shots: statistics, the import preview, and the editor tour.

## Order

Typing → statistics → sharing → docs → screenshots, README and landing page. One commit per piece, each with its tests and docs.
