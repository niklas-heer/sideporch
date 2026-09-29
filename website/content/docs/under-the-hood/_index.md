+++
title = "Under the hood"
description = "How Sideporch is built: one program, one database, and what happens inside when someone sends a message."
weight = 5
sort_by = "weight"
template = "docs-section.html"
+++

Sideporch is one program written in [Rust](https://www.rust-lang.org). It serves the web pages, keeps live connections to browsers, runs automations and background work, and keeps everything in one data directory. There is no database server, message broker, cache or separate frontend to run next to it.

{% <diagram caption="What runs where. Everything inside the box is one process."> %}
flowchart LR
  subgraph People
    B[Browsers and phones]
  end
  subgraph Sideporch["sideporch (one process)"]
    H[HTTP server<br/>pages and forms]
    W[Live connections<br/>WebSocket hub]
    P[Message pipeline]
    A[Automation workers<br/>one thread each]
    T[Background tasks<br/>reminders, backups,<br/>updates, cleanups]
  end
  subgraph Data["Data directory"]
    D[(sideporch.db<br/>SQLite)]
    F[files/<br/>by SHA-256]
    K[secret.key]
  end
  B -- HTTPS --> H
  B <-- WebSocket --> W
  H --> P
  P --> D
  P --> W
  P --> A
  A --> D
  T --> D
  H --> F
  A -. secrets .-> K
  P -- Web Push --> X[Push services]
  P -- signed requests --> O[Other Sideporch servers]
{% </diagram> %}

## The parts

- **Pages are made on the server.** Every page is HTML rendered by [maud](https://maud.lambda.xyz), styled with utility classes compiled into one stylesheet when Sideporch is built. Pages work without JavaScript; one small script adds live updates, sending without reloading, the emoji picker and other comforts.
- **Live updates** travel over one WebSocket per open tab. A message is rendered once and sent to the tabs that show its channel; the others only hear that the channel has something new.
- **One database**: [SQLite](https://sqlite.org) in write-ahead-log mode holds people, channels, messages, settings and the search index. File contents live next to it on disk.
- **Automations** run Lua 5.4 in a sandbox, each on its own thread with its own database connection, so one slow script never holds up chat.
- **Background tasks** deliver reminders and scheduled messages, write scheduled backups, check for updates, deliver to connected servers, clean up old data, and reset demo servers.

The program is built as a single static file for Linux (x86-64 and ARM), and for macOS; the container image contains nothing else.

## Read on

- [How a message travels](@/docs/under-the-hood/messages.md): from the send button to everyone's screen, notifications and automations.
- [Data and storage](@/docs/under-the-hood/storage.md): the database, files, search, upgrades and backups.
- [Automations inside](@/docs/under-the-hood/automations.md): workers, the sandbox and its limits.
- [Security](@/docs/under-the-hood/security.md): signing in, sessions, limits, secrets and signed updates.

The decisions behind all of this, with the alternatives considered, are recorded in the repository's [`decisions/`](https://github.com/niklas-heer/sideporch/tree/main/decisions) directory.
