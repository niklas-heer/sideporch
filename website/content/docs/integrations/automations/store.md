+++
title = "Automation store"
description = "Browse automations other people publish, and install one with a click after a preview."
weight = 7
+++

The automation store is a catalogue of automation files. Under **Automations → Browse the store**, an admin sees what it lists and installs one with a click. Nothing is installed without a preview, and an installed automation starts switched off, as with any [import](@/docs/integrations/automations/sharing.md).

Sideporch doesn't review what's listed. Each entry shows its name, description, author, license and homepage, and the script is there in the preview. Read it before you switch it on.

## What the store checks

Each entry names a file, its SHA-256 checksum and, when its publisher signed it, a minisign signature. Before a file is previewed, Sideporch:

- downloads it through the same guarded client as `sideporch.http`, so addresses on private networks are refused unless [Automation settings](@/docs/integrations/automations/data-and-services.md) allow them, and downloads stop at 2 MB,
- checks its SHA-256 against the entry, and refuses the file if it differs,
- checks the signature against Sideporch's release key, when the entry has one, and says in the preview whether the file is **Signed by Sideporch** or **Not signed**.

A checksum or signature that doesn't match is a hard refusal. An unsigned file can still be installed, but only after you confirm that you want an unsigned automation.

## The store's address

The store is read from `https://sideporch.app/automations/index.json` unless you change it under **Settings → Automations → Automation store**. Leave the field empty for the default. The index is kept for ten minutes; if it can't be reached, the page says so and nothing is installed.

## Publishing an index

The index is one JSON file:

```json
{
  "format": "sideporch-catalogue",
  "version": 1,
  "entries": [
    {
      "name": "Welcome new members",
      "description": "Posts a welcome in #general when someone joins.",
      "author": "Ann",
      "license": "MIT",
      "homepage": "https://example.com/welcome",
      "version": "1.0.0",
      "file": "https://example.com/welcome.sideporch.json",
      "sha256": "…the file's SHA-256 in hex…",
      "signature": "https://example.com/welcome.sideporch.json.minisig"
    }
  ]
}
```

- `name`, `version`, `file` and `sha256` are required. `description`, `author`, `license`, `homepage` and `signature` are optional.
- `file` is a `.sideporch.json` file from [sharing](@/docs/integrations/automations/sharing.md#the-file-format), and `sha256` is its checksum. Compute it with `shasum -a 256 welcome.sideporch.json`.
- `signature`, when given, is the minisign signature of the file.

Sideporch reads an index only in the format versions it understands. The examples Sideporch publishes are at `https://sideporch.app/automations/`.
