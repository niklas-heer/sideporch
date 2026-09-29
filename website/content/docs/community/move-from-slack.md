+++
title = "Move from Slack"
description = "Import a Slack workspace export: people, channels, threads and reactions."
weight = 7
+++

Bring your team's Slack history along, so nothing gets lost in the move.

1. In Slack, export your workspace: *Workspace settings → Import/Export Data*.
2. In Sideporch, upload the ZIP under **Admin → Import**. Exports up to 1 GB work.

## What comes over

- **People.** Those who already have an account in Sideporch are matched by username. The others get accounts without a password: send each of them a [reset link](@/docs/community/people-and-invites.md#forgotten-passwords) from their profile.
- **Public and private channels**, and **direct messages**, as far as the export includes them.
- **Messages, threads and reactions.**

Files stay in Slack, because Slack keeps them behind its login; messages name them instead.

Importing the same export again only adds what's new, so you can import once to try it, and again on the day you switch.
