+++
schema_version = 1
id = "01M3QDEFYKN0CNR9JFTD6KTP4V"
title = "Share automations as versioned JSON files that import switched off"
date = "2026-09-29"
status = "accepted"
tags = ["automations", "compatibility"]
supersedes = []
superseded_by = []
depends_on = []
related_to = ["01M3M77KF4V6HX4AC84FV21HM3", "01M3J6PYXDJY1DJVFKX0DN39AP"]
+++
## Decision

Automations travel between servers as one versioned JSON file, exported from Automations and imported after a preview.

- The file is `{ "format": "sideporch-automations", "version": 1, "items": [...] }`, named like `weather.sideporch.json`. Each item has `kind` (`automation` or `library`), `name`, `source`, and the names of the `secrets` it reads and the libraries it `requires`. The file also carries the Sideporch version that wrote it, and optional `description`, `author`, `license` and `homepage` fields for sharing.
- Only source code travels. Secret values, stored data, run logs, webhook URLs and history never leave the server.
- Exporting automations adds every library they need, following libraries that need other libraries. Libraries come first in the file.
- Readers ignore fields they don't know, refuse other formats, and refuse higher versions with a request to update Sideporch. What a script needs is read from its source on import, not taken from the file.
- Importing shows each item as new or as clashing with an existing name. Clashing automations can be skipped, replace the existing one, or be imported as a copy; clashing libraries can only be skipped or replaced, because `require` finds them by name. The preview also lists missing secrets and libraries, and the linter's findings.
- Everything imported starts switched off, including a replaced automation that was on, and is saved as a version with the tool name `import`.

## Context

On 2026-09-29 Niklas asked for a way to share automations: export one or several, import them on other servers. He also named a possible later automation store or plugin system hosted on sideporch.app, and asked that the implementation not close that off (see `docs/future-ideas.md`).

- **Plain `.lua` files** are easy to read but can't hold a library alongside the automation that needs it, or say what secrets it expects, without inventing a comment convention.
- **Including stored data or secrets** would make a move between servers complete, but leaks credentials and private data into files people post publicly. Backups already cover moving a whole server.
- **Importing switched on** would save a click, but imported code can post, react and call other services with this server's secrets. Reading it first is the point of the preview.
- A versioned format with room for descriptive fields lets a store serve the same files later, and lets Sideporch import from a URL without a second format.

## Consequences

- A store, or importing from a URL, can reuse `bundle::parse` and the preview; new fields for it must be optional so older servers still read the files.
- Scripts that build secret or library names at run time aren't listed in the file or the preview. The linter and the run log still show what goes wrong.
- A replaced automation keeps its webhook URL and data, but must be switched on again.
- Changing the meaning of an existing field requires version 2, which older servers refuse with a clear message.
