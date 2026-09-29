+++
schema_version = 1
id = "01M3NZYKHNN20Q7BCVEWV7M860"
title = "Upgrade the database automatically on start, keeping a copy first"
date = "2026-09-29"
status = "accepted"
tags = ["operations", "storage"]
supersedes = []
superseded_by = []
depends_on = []
related_to = []
+++
## Decision

Sideporch upgrades itself on start. Migrations in `src/db.rs` are numbered by position and applied in order from the database's `PRAGMA user_version` to the newest, so skipping several releases works like installing each one. Each step is SQL or, where SQL can't express it, Rust code (`Migration::Code`), and runs in its own `BEGIN IMMEDIATE` transaction that re-reads the version first, so an interrupted upgrade resumes and two processes on one database can't apply a step twice.

Before changing a database an older release wrote, Sideporch copies it with `VACUUM INTO` to `upgrade-backups/sideporch-schema<N>-<time>.db` and keeps the newest three. `sideporch restore` accepts such a copy. A `schema_history` table records which release applied each version; a database newer than the binary is refused with the name of the release that wrote it.

## Context

On 2026-09-29 Niklas asked that upgrading be as simple as installing a new binary or container, including jumps over several versions, with everything handled safely. The existing runner already applied numbered SQL migrations in transactions, but it read the version once before the loop, could not run data migrations in Rust, and would open a database from a newer release and fail later on unknown tables.

Alternatives considered:

- A migration crate such as `refinery` or `rusqlite_migration`: they add the same numbered, transactional steps without the pre-upgrade copy or the refusal message, and the existing list would have to be converted.
- A full backup archive before each upgrade: it would copy every uploaded file, which migrations never change (files are stored by content hash), and double the disk use of large instances on every upgrade.
- Downgrade migrations: they would have to be written and tested for every step and could still lose data added by the newer version; restoring the pre-upgrade copy is simpler to reason about.

## Consequences

- Released migrations must never change, only be appended; AGENTS.md says so.
- Each upgrade costs the time and disk space of one database copy. Only three copies are kept.
- Going back to an older release means losing what was written since the upgrade; the README says to restore the copy for that.
- The unit tests upgrade a database from every schema version to the newest, and an end-to-end test upgrades one with data from the second schema.
