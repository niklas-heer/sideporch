# Future ideas

Ideas worth keeping in mind, not planned work. Each says what exists today, so a later design starts from there instead of from scratch.

## An automation store, and plugins

A place on sideporch.app where people publish automations and libraries, and where an admin can browse them and install one with a click.

What exists: automations already travel as versioned JSON files (`src/automations/bundle.rs`, decision "Share automations as versioned JSON files that import switched off"). Those files accept optional `description`, `author`, `license` and `homepage` fields, and readers ignore fields they don't know. Importing always goes through a preview, and imported scripts start switched off.

What a store would need:

- **Importing from a URL**: fetch a file through the guarded HTTP client (`src/automations/http.rs`) and hand it to the existing preview.
- **A catalogue**: a static index on sideporch.app (one JSON file listing entries and their file URLs) would do at first; no server needed beyond the website.
- **Trust**: files signed like releases (`src/updates/signature.rs`), so a server knows an entry wasn't changed on the way. Reviews or verified authors, if the store grows.
- **Updates**: remembering where an automation came from (a new column, for example `automations.origin`) so Sideporch can offer a newer version. Replacing already keeps history and leaves it switched off.
- **Settings instead of editing**: many shared automations need a channel name or a threshold. A declared list of settings in the file, shown as a form, would spare people editing Lua. The sandbox could expose them as `sideporch.setting("name")`.

**Plugins** that change Sideporch itself (new pages, new message kinds) are a larger step. They'd need a stable extension API beyond the Lua sandbox. The store could start with automations, which are sandboxed already, and grow from there.

## Typing across connected servers

Typing shows to people on the same server only. Shared channels on other servers don't see it, because the federation outbox delivers every event in order and retries it, which is wrong for a signal that is stale after a few seconds. A best-effort, unqueued notice between servers would fix that.
