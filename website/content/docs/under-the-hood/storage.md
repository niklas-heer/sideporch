+++
title = "Data and storage"
description = "The database, files on disk, search, upgrades between versions, and backups."
weight = 2
+++

Everything Sideporch keeps is in its data directory:

| Path | What it is |
| --- | --- |
| `sideporch.db` (with `-wal` and `-shm`) | The SQLite database: people, channels, messages, reactions, settings, automations, the search index. |
| `files/` | Uploaded files, pictures and custom emoji, each stored once under its SHA-256. |
| `secret.key` | The key that encrypts stored secrets such as API tokens. |
| `upgrade-backups/` | A copy of the database from before each upgrade. |
| `models/` | Speech models, if you downloaded them. |

## The database

SQLite runs inside Sideporch, in write-ahead-log mode, so readers never wait for writers. Requests go through one connection, which keeps writes simple and fast; long reports, such as statistics, use a second, read-only connection so they don't hold anything up. Each automation worker has its own connection.

[How big a server](@/docs/community/server/server-size.md) shows what that handles: thousands of people online on half a CPU.

## Files

Uploads are written to `files/<first two letters of the hash>/<SHA-256>`, first to a temporary file and then renamed into place, so a crash never leaves half a file under a real name. The same file uploaded twice is stored once. Deleting the last message that uses a file deletes the file's record, and its contents on disk go when Sideporch next starts or a demo resets.

## Search

Search uses SQLite's full-text index (FTS5). Words match as prefixes, results are ranked by relevance weighted toward recent messages, and a misspelled word is corrected from the words your team actually used.

## Upgrades

{% <diagram caption="Starting a newer version on an older database."> %}
flowchart LR
  S[Sideporch starts] --> C{Database older<br/>than this version?}
  C -- no --> R[Run]
  C -- yes --> K[Keep a copy in<br/>upgrade-backups/]
  K --> M[Apply each missing step,<br/>in order]
  M --> R
{% </diagram> %}

The database records which schema version it has and which Sideporch release wrote it. A newer Sideporch applies the steps it's missing, one after another, after keeping a copy of the old database. People can skip versions: every step works on data any earlier release wrote. An older Sideporch refuses to open a database from a newer one, and says which version wrote it.

## Backups

A backup is one `.tar.gz` file with the database, the files and, if you choose, the secret key. The database is copied with `VACUUM INTO`, which gives a consistent snapshot while the server keeps running. See [Backups](@/docs/community/server/backups.md).
