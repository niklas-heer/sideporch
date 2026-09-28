+++
schema_version = 1
id = "01M3MQJHKFQK275WH2W6CEF9MK"
title = "Back up to one tar.gz archive made from a VACUUM INTO snapshot"
date = "2026-09-28"
status = "accepted"
tags = ["operations", "storage"]
supersedes = []
superseded_by = []
depends_on = []
related_to = []
+++
## Decision

Admin → Backups writes one `.tar.gz` with `sideporch.db` (copied with `VACUUM INTO`), every stored file under `files/`, and optionally `secret.key`. Admins download it, or let Sideporch write backups on a schedule to a directory on the server and keep the newest few. `sideporch restore <archive>` unpacks one into an empty data directory and accepts only those three kinds of entries.

## Context

On 2026-09-28 Niklas asked for backups from inside Sideporch. The README promises data that is easy to back up and move, and Docker images have no shell for `sqlite3 .backup` or rsync.

- `VACUUM INTO` gives a consistent snapshot while the server runs, and holds the database only while copying it; files are packed afterwards without blocking anyone.
- tar and gzip (the `tar` and `flate2` crates) are readable everywhere without Sideporch.

## Consequences

- Including the key by default makes backups self-contained, but whoever holds one can read stored secrets. The page says so, and the key can be left out.
- A file deleted between the snapshot and packing is skipped; the next start collects files that no record uses.
