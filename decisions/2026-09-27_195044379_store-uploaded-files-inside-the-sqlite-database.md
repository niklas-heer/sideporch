+++
schema_version = 1
id = "01M3J6PYWVD139NB5BKDX877CJ"
title = "Store uploaded files inside the SQLite database"
date = "2026-09-27"
status = "superseded"
tags = ["storage", "files"]
supersedes = []
superseded_by = ["01M3M94K5F7DPZBDQEZNQJS71W"]
depends_on = ["01M3HRKXMTMQM93PHABB0VW2BC"]
related_to = []
+++
## Decision

Uploaded files and custom emoji images are stored as BLOBs in the SQLite database, in a `files` table, not as files in the data directory.

- Each file may be up to 25 MB, a message may carry up to 10, and a custom emoji image up to 256 kB.
- The file type shown to browsers comes from the first bytes, not the uploader's claim. Only PNG, JPEG, GIF and WebP are served inline. Everything else is served as `application/octet-stream` with `Content-Disposition: attachment`, plus `nosniff` and a `sandbox` Content-Security-Policy.
- `/files/{id}` checks access: the uploader, anyone for custom emoji, and otherwise only people who can read a channel where the file is attached.

## Context

Niklas asked for file uploads on 2026-09-27. A goal from the start is that an instance is easy to back up, move, and replicate.

- **Files on disk next to the database** is the conventional layout and streams large files efficiently. But a backup then needs two things kept consistent, and Litestream replicates only the database.
- **BLOBs in SQLite** keep everything in one file with transactional consistency. A single `sqlite3 .backup` or Litestream stream covers messages and files together. For the file sizes a small group shares (photos, PDFs), SQLite handles BLOBs well.

## Consequences

- The database grows with uploads, and a file is read into memory to be served. That is fine at 25 MB per file; raising the limit substantially should come with streaming reads (SQLite incremental BLOB I/O) or a revisit of this record.
- There is no file deletion yet. Deleting a message or channel cascades to its links, but orphaned `files` rows would need a cleanup job once deletion exists.
- Search indexes file names, so people can find shared files by name.
