+++
schema_version = 1
id = "01M6XJH3G0MZ18JBTEK5HP3VNF"
title = "Group direct messages are direct conversations keyed by their members"
date = "2026-11-08"
status = "proposed"
tags = []
supersedes = []
superseded_by = []
depends_on = []
related_to = []
+++
## Decision

Group direct messages are direct conversations (`channels.kind = 'dm'`) with three to nine people. A conversation is identified by its member set: the sorted, deduplicated member ids joined by `:` in `channels.dm_key`. Two-person keys keep their existing form, and notes to self keep `id:id`. Membership is fixed when the conversation is created.

## Context

People wanted to talk to several others privately without creating and naming a private channel. The schema already had everything a group needs: the `kind` check allows only `public` and `dm`, and audience, push, unread, search and privacy all work from `kind = 'dm'` plus `channel_members`. Widening `kind` would have meant rebuilding the table and touching every query that branches on it.

The alternatives were a third kind (a table rebuild and many more branches to keep right), and a private channel with a generated name (what people did before, and the reason this feature exists).

## Consequences

- No migration. Existing two-person keys and the Slack import use the same key, so the same people always find the same conversation, from the People page, `/dm/{id}`, or a group with the same members.
- Adding someone later means a new conversation, because the key is the membership. The interface says so.
- Groups are never shared with other servers. Their members must all be accounts on this server that can sign in.
- Creating a group is limited to nine people, the creator included, on the server.
