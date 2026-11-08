+++
schema_version = 1
id = "01M6XKMJDHG1F1M037A5JDZ7JQ"
title = "Let admins install automations from a static store with checksums and optional signatures"
date = "2026-11-08"
status = "proposed"
tags = []
supersedes = []
superseded_by = []
depends_on = []
related_to = []
+++
## Decision

Sideporch's automation store is a static JSON index of automation files. Admins browse it under Automations → Browse the store and install an entry after a preview, switched off.

- **The index** (`sideporch-catalogue`, version 1) lists entries with name, description, author, license, homepage, version, the file's address, its SHA-256 and an optional signature address. It is read through the guarded HTTP client, cached for ten minutes, and defaults to `https://sideporch.app/automations/index.json`; admins can point it elsewhere in Automation settings.
- **Trust**: a file must match the SHA-256 its entry lists, or it is refused. A signature, when given, is a minisign signature checked against the release key built into `src/updates/signature.rs`, and its trusted comment must be `sideporch-automation <sha256>`, so a release's signature can't be passed off for an automation. A signed file previews as "Signed by Sideporch". An unsigned file installs only after the admin confirms it.
- **Files from the web** (a pasted address, or a store entry) are fetched through the guarded client, so internal addresses are refused unless an admin allows them. Files from disk keep their existing preview.
- **Updates**: an automation installed from a file records its origin (address, version, SHA-256). When the store lists a newer version at the same address, the Automations page offers it, and the update goes through the same preview with Replace chosen, which keeps history and leaves the automation switched off.
- **Settings**: a bundle item may declare `settings` (string, number, boolean or channel, with label, default and required). Values are stored per automation and set on its page; `sideporch.setting(name)` reads them. A channel value must name a public channel, and a switched-on automation needs its required settings. The bundle format version stays at 1, since readers ignore the new field.

Sideporch doesn't review what's listed, and reviews or verified authors are out of scope. Plugins that change Sideporch itself are out of scope too.

## Context

On 2026-11-08 Niklas asked for a store on sideporch.app, built on the existing sharing format (decision "Share automations as versioned JSON files that import switched off") and with the release signing key rather than a new one.

- **A registry with accounts and reviews** would need a server and moderation, which the one-binary, self-hosted goal avoids for now. A static index on the website needs nothing beyond the site.
- **A separate signing key for automations** would be cleaner for rotation, but the brief asked for the key built into `signature.rs`. Signing automations with the release key means the release workflow's key also signs example automations; a separate key is the next step if that worries the maintainer.
- **Matching updates by name** would break when someone renames an automation; matching by the file's address keeps the origin stable across versions.

## Consequences

- Publishing an entry means hosting a `.sideporch.json` file and an index, with a checksum. A signature is optional and only the maintainer can make one.
- An unsigned automation from the store needs a confirmation, and the preview says so.
- The store's trust hinges on the release key; rotating that key also rotates how automations are signed.
- Declared settings are only created by importing a file. An automation written on the server has none, so its settings page stays empty.
