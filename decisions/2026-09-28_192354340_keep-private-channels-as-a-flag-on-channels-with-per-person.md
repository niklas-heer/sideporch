+++
schema_version = 1
id = "01M3MQJHK4WYKNEZMRJHTR4FNC"
title = "Keep private channels as a flag on channels, with per-person preferences"
date = "2026-09-28"
status = "accepted"
tags = ["channels", "privacy"]
supersedes = []
superseded_by = []
depends_on = []
related_to = []
+++
## Decision

Private channels are rows in `channels` with `private = 1`; their members are in `channel_members`, like direct conversations. One SQL condition decides who can read a channel, and every reader-facing query uses it: pages, files, search, live updates, notifications and activity. Leaving a public channel and muting a channel are per-person rows in `channel_prefs`, not memberships.

Automations, MCP, incoming-webhook channel overrides and `channel_created` events see public channels only.

## Context

On 2026-09-28 Niklas asked for private channels, leaving and muting. The `kind` column has a CHECK constraint that SQLite can only change by rebuilding the table, and public channels had no membership at all: everyone can read them.

## Consequences

- Public channels stay open to everyone without a membership row per person; "left" only hides one from the sidebar, and writing there brings it back.
- A private channel's last member can't leave, since nobody, not even an admin, could reach it afterwards.
- Muted and left channels only notify people mentioned by name.
