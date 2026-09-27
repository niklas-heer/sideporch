+++
schema_version = 1
id = "01M3HRKXMTMQM93PHABB0VW2BC"
title = "Store all data in one embedded SQLite database"
date = "2026-09-27"
status = "accepted"
tags = ["storage", "architecture"]
supersedes = []
superseded_by = []
depends_on = ["01M3G6RCQKT16B893YJKV7YY7K"]
related_to = []
+++
## Decision

Sideporch keeps all of its data in one SQLite database file, `sideporch.db`, inside the data directory (`--data`, default `sideporch-data`). SQLite is compiled into the binary through `rusqlite` with its `bundled` feature, so no system library or server is needed.

- The database runs in WAL mode with foreign keys on.
- The schema is a list of append-only migrations in `src/db.rs`, tracked with `PRAGMA user_version`. A released migration is never edited; changes are new entries.
- `rusqlite` is synchronous. All queries go through `Db::call`, which runs them on Tokio's blocking pool behind one connection.

## Context

Niklas first suggested JSON files as the main database so an instance is easy to copy and replicate. On 2026-09-27 he chose SQLite when offered it against an append-only JSONL event log.

- **JSON files** risk corruption if the process stops mid-write, and they have no indexes or search.
- **A JSONL event log with state rebuilt in memory** is human-readable and easy to copy. But search, unread tracking, and paging would all have to be built by hand, and startup time grows with history.
- **SQLite** is still one file to back up or move. It is crash-safe, has indexes, offers full-text search (FTS5) for later, and can be replicated continuously with Litestream. Every comparable single-binary chat server (Chatto, Campfire, tensorchat) uses it.

`sqlx` was also considered. Its compile-time checked queries need a database or prepared metadata at build time, and its async pool adds little for a single-file database with one writer. `rusqlite` is smaller and gives direct control over pragmas and transactions.

## Consequences

- Backing up or moving an instance means copying the data directory. With WAL, a live copy should use `sqlite3 .backup` or Litestream; copying while stopped is always safe.
- One process owns the database. Running several Sideporch processes against one file is out of scope, consistent with the single-binary decision.
- Heavy write bursts are serialized through one connection. That is fine for small groups; measure before adding a connection pool or read replicas.
- Search can later use SQLite FTS5 without a new dependency.
