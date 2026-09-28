+++
schema_version = 1
id = "01M3M94K5F7DPZBDQEZNQJS71W"
title = "Store file contents on disk by content hash"
date = "2026-09-28"
status = "accepted"
tags = ["storage", "files"]
supersedes = ["01M3J6PYWVD139NB5BKDX877CJ"]
superseded_by = []
depends_on = []
related_to = []
+++
## Decision

File contents live on disk in the data directory; their metadata stays in SQLite.

- Each file's bytes are stored once as `files/<first two hex digits>/<sha256>`. Writes go to a temporary file that is synced and renamed into place, so a crash never leaves a partial file under a real name. Identical uploads share one copy.
- The `files` table keeps the name, type, size, uploader and hash; its old `data` column holds an empty blob for new rows.
- On start, Sideporch moves any contents older versions kept in the database to disk, compacts the database with `VACUUM`, and deletes stored contents no row refers to anymore.
- Everything else from the previous decision holds: size limits, sniffing the type from the first bytes, serving only safe images inline, the sandbox Content-Security-Policy, and the access checks on `/files/{id}`. Profile pictures are readable by everyone signed in, like custom emoji.

## Context

On 2026-09-28 Niklas asked for posting images and GIFs and wondered where to keep them: "On the file system, potentially? Yes, probably."

- **Keeping bytes in SQLite** made backups a single file, but images and videos grow the database quickly, every backup and Litestream replica copies them again, and serving them means loading whole BLOBs through the one connection.
- **Content addressing** makes files immutable once written, so an ordinary file backup taken after the database backup is always consistent, and it deduplicates reposted images for free.
- **An object store (S3)** would add an external service, against the single-binary goal.

## Consequences

- Backups must include `files/` as well as the database; the README says how to copy them while Sideporch runs.
- A missing file on disk shows as a server error for that file only.
- Deleting data (messages, emoji, pictures) frees disk space at the next start, when garbage collection runs.
