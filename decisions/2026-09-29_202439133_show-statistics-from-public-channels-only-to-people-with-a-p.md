+++
schema_version = 1
id = "01M3QDEFYXSK20K1DDDWK37F2C"
title = "Show statistics from public channels only, to people with a permission"
date = "2026-09-29"
status = "accepted"
tags = ["community", "privacy"]
supersedes = []
superseded_by = []
depends_on = []
related_to = ["01M3P1TDCJAJ2ZB99F0QWPE0CR"]
+++
## Decision

Sideporch has a statistics page that counts public channels only, ranks people, and is shown to people with the **See statistics** permission.

- Totals (messages, people who wrote, reactions, files), messages per day, month or year in the reader's time zone, the ten people with the most messages and the most reactions received, the busiest channels, the most used reactions, and the reader's own place. Periods: 7 days, 30 days, 12 months, all time.
- Only messages in public channels count. Private channels and direct messages never count, not even in totals. Deleted messages don't count.
- Bots, webhooks and automations, and people from other servers, count in totals but aren't ranked. Reactions to your own messages don't count as received.
- **See statistics** is a normal permission, level 1 by default. Admins can raise the level, or leave it to roles so only chosen people see statistics.
- Anyone can leave the rankings from their profile. They still count in totals.
- Messages are counted per quarter hour in SQL and placed into the reader's days in Rust, so days follow each person's time zone, including half-hour offsets.

## Context

On 2026-09-29 Niklas asked for statistics and a ranking of who writes most, available to "normal verified users" by default, with admins deciding who may see them. Level 1 is where people who joined with an invite start, and where newcomers arrive after a day of taking part, which matches that description.

- **Counting private channels and DMs in totals** would give truer numbers, but shows how much people talk privately, which members of a private channel may not expect to be shared.
- **A separate admin switch** for statistics would duplicate what permissions already do; the permission covers "everyone", "trusted people only", and "only this role".
- **No opt-out** would be simpler, but rankings put names on a public board. In an open community some people won't want that.
- **Caching the report** isn't done yet: the page isn't hot, and the queries use a new index on `messages (created_at)`.

## Consequences

- Adding a kind of message or channel means deciding whether it counts here.
- Statistics over all time scan every counted message; if large servers find the page slow, cache the shared part per period and time zone.
- The ranking is a leaderboard. Communities that dislike that can leave the permission to a role nobody has.
