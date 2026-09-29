+++
schema_version = 1
id = "01M3P1TDCJAJ2ZB99F0QWPE0CR"
title = "Decide what people may do with trust levels and roles, and let communities open sign-up"
date = "2026-09-29"
status = "accepted"
tags = ["community", "security"]
supersedes = []
superseded_by = []
depends_on = []
related_to = []
+++
## Decision

Sideporch decides what people other than admins may do with **trust levels** and **roles**:

- Five levels, named like Discourse's: 0 New, 1 Basic, 2 Member, 3 Regular, 4 Leader. People who sign up on their own start at 0, invited or approved people at 1. Levels 1 to 3 are earned automatically from days since joining, days visited and messages sent (defaults 1/1/3, 7/3/20, 30/15/100, tunable by admins); promotion happens after posting and on the first visit of a day. Levels never drop on their own; admins can set one and lock it. Level 4 is only given by admins.
- Ten permissions (upload files, post links, `@channel`, start direct messages, create public or private channels, create polls, add emoji, invite people, moderate) each ask for a minimum level, or none ("roles only"). Admin-made roles grant permissions regardless of level. Admins may do everything. Defaults: polls for everyone, most others from level 1, inviting and moderating for roles only.
- Existing accounts migrated to level 2 (admins 4), so nothing changes for teams that were already here; invited people start at 1, where the defaults allow what they could do before.
- Sign-up is `invite` (default), `approval` (requests wait in a `signups` table and become accounts only when a moderator approves) or `open`. Sign-ups are capped at 30 per hour server-wide and a hidden field turns away simple bots. Admins may add rules people must accept.
- Moderators (the `moderate` permission) handle reports, delete messages, approve sign-ups and time people out. Level-0 members may send at most 6 messages a minute by default.

Permissions are loaded with the session into a bitset on `CurrentUser`, so checks don't query the database.

## Context

On 2026-09-29 Niklas asked for a public demo instance: a role system so admins can turn off uploads or grant them to specific roles, trust levels with good defaults "like Discord" (he meant Discourse's model) to prevent abuse, and public registration.

Alternatives: roles alone (Discord's model) put every newcomer's abilities in one role and need manual promotion; trust levels alone can't express "the design team may upload". Combining both keeps defaults automatic and exceptions explicit. Keeping pending sign-ups out of `users` means nobody unapproved appears in people lists, mention suggestions or automations.

## Consequences

- Every permission-sensitive route must call `CurrentUser::require` or check `may`; the templates hide what people can't use.
- Links are refused, not stripped, for people without the permission; the error explains why.
- Rate limits and sign-up caps are counted from the database, not per IP, since Sideporch often runs behind proxies without a trusted client address.
- Email verification and CAPTCHAs are not part of this; approval mode is the answer for communities that need more.
