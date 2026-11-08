+++
schema_version = 1
id = "01M6XKDECGZJE93QYCF6KJDW7G"
title = "Thread followers are explicit, and an unfollow outranks past participation"
date = "2026-11-08"
status = "proposed"
tags = []
supersedes = []
superseded_by = []
depends_on = []
related_to = []
+++
## Decision

Thread followers are explicit. A person follows a thread by choosing to, or by writing in it, and can unfollow it. The thread's audience is everyone who wrote in it or follows it, minus the people who explicitly unfollowed it. An explicit unfollow outranks past participation, and replying in a thread follows it again.

- `thread_follows (user_id, root_message_id, following)` holds one row per person and thread, only once they follow or unfollow it. `following = 0` is the unfollow; `following = 1` is a follow without a post.
- A reply by someone who unfollowed flips their row back to following, in the same transaction as the reply.
- Mentions reach people regardless of following.
- `thread_reads (user_id, root_message_id, last_read_id)` holds the newest reply each person has seen. Replies after it from others are unread. Opening a thread, following it, or replying advances it.
- Following is local to this server. Thread roots in shared channels behave like any other message; nothing about following is sent to other servers.

## Context

Until now thread recipients were inferred from the people who had written in the thread (`store::recipients`). Niklas wanted to follow a thread without posting, unfollow it, and see unread followed threads first (`docs/future-ideas.md`).

Alternatives considered:

- Follow state as a flag on each message or as an append-only event log. Rejected: a per-person row is what the audience query and the Threads list need, and an unfollow must survive later posting by the same person.
- Treating an unfollow as a mute that suppresses push only. Rejected: the person would still get the thread in Activity, and the Activity list is the record of what concerns them.
- Not re-following on reply. Rejected for now: the common case is "unfollow, then say thanks", and a person who replies is taking part again. This is the one rule that's easy to reverse if it proves wrong.

Existing threads were marked read for everyone who had taken part in them with a reply, so upgrading doesn't mark every old thread unread.

## Consequences

- Each unfollow and each reply can add a row; rows are removed with their messages and users.
- Replies to a thread notify more people when someone explicitly followed it.
- Push notifications for thread replies follow the same audience, so an unfollow silences push as well as Activity.
- A person's Threads list is limited to 50 threads, unread first, so a long-running account sees the newest ones.
