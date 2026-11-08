+++
schema_version = 1
id = "01M6XJT38TFCJ7GVA81J99NS62"
title = "Send typing to other servers as unqueued signed notices, not through the outbox"
date = "2026-11-08"
status = "proposed"
tags = []
supersedes = []
superseded_by = []
depends_on = []
related_to = []
+++
## Decision

Typing crosses between connected servers as a separate, best-effort notice, not as an event in the outbox. A notice is sent once, signed with the sender's Ed25519 key like every request between servers, to `POST /federation/typing`, with a two-second timeout. It is never queued, retried or stored, and a failure is logged at debug level only.

- Notices follow message topology: a host passes a guest's notice on to its other guests, a guest tells only its host, and nothing is sent back the way it came.
- The sender throttles per person, channel, thread and server, at most one notice every two seconds. A receiving server admits at most 100 notices per server every 10 seconds, and refuses a repeated nonce. That memory is in memory only, so a restart forgets it.
- A receiver checks the same things as for messages (the channel is shared with that server, and the server may speak for that person), and for a private channel also that the person is a member. It never creates an account for the person. A remote typist is shown under a stable negative id that never matches a local account.
- Refusals are generic and reveal nothing about the channel.

## Context

On 2026-11-08 the typing indicator showed only to people on the same server. Shared channels on other servers couldn't see it, because the outbox delivers every event in order and retries it, and a typing signal goes stale within seconds. Sending it through the outbox would queue a stale "Ada is typing…" for an unreachable server and deliver it long after it stopped meaning anything.

- Putting typing in the outbox with an expiry was considered. It keeps one delivery path, but the outbox has no notion of a signal that dies, and a retry queue would still hold notices nobody wants. A second path is simpler to reason about.
- Dropping typing across servers altogether was considered. It keeps the protocol unchanged, but people in shared channels would keep asking whether a colleague is writing, which is the feature gap that prompted this.

## Consequences

- Typing is lossy by design. A server that is slow or down simply shows no dots; it doesn't back up the sender or its outbox.
- The notice shape is part of the protocol. Like other events, it may only gain optional fields, and it is signed with the same `sideporch-request-v1` string, so older servers ignore the route and carry on without typing.
- Typing adds no table and no migration. Its limits and nonce memory are in memory, so they reset on restart.
- Any future signal that is only useful while fresh (read positions, presence) should take this path, not the outbox.
