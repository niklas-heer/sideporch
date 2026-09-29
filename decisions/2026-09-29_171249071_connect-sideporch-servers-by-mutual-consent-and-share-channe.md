+++
schema_version = 1
id = "01M3Q2F7NFB4C12N6N5YMNEB53"
title = "Connect Sideporch servers by mutual consent and share channels over signed requests"
date = "2026-09-29"
status = "accepted"
tags = ["federation", "security", "architecture"]
supersedes = []
superseded_by = []
depends_on = ["01M3M77KFGMZ3KFYY4E67SKAM2"]
related_to = ["01M3MQJHK4WYKNEZMRJHTR4FNC"]
+++
## Decision

Sideporch servers connect to each other the way companies do in Slack Connect: both admins agree, then they share channels and their people write to each other directly. The servers talk over signed HTTP requests of their own design, not ActivityPub or Matrix.

- **Connecting**: an admin enters the other server's address. Sideporch reads its `/.well-known/sideporch` (address, name, Ed25519 public key), then sends a connect request signed with its own key. The other admin sees the request, the note and the key's fingerprint, and accepts or declines. Each side pins the other's key when it first sees it. Only servers with a public URL (`--public-url`) can connect, and only to public addresses.
- **Signing**: every request between servers carries `sideporch-server`, `sideporch-date`, `sideporch-nonce` and `sideporch-signature` headers. The signature covers a versioned string with the method, path, both servers, the date, the nonce and the body's SHA-256. Requests more than 5 minutes off, with a nonce seen before, or from servers that aren't connected are refused. The server key is made on first start and stored sealed with the secrets key (`src/secrets.rs`).
- **Shared channels**: the server that shares a channel is its host; every guest keeps a full copy. The host passes each event on to every other guest; a guest only tells the host, and only about its own people. A server may speak only for its own people, never for the receiving server's. Messages are known everywhere by the uid `handle#id` from the server they were written on, so edits, deletions, reactions and replies find them. When a guest takes a channel, the host sends the latest 50 conversations.
- **People from other servers** become accounts that can't sign in: username `name@handle`, linked to the server. They can't get reset links, admin rights or sign-in methods, and People lists only local people. Mentions are rewritten per server, so `@bea` on her own server is `@bea@b.example` elsewhere.
- **Private channels** share their membership through join and leave events. Each server adds and removes its own people, and the host passes that on; a message from someone the host doesn't count as a member is refused.
- **Direct messages** start from People → Find someone, which searches the other server's directory, and create a direct conversation on both sides. Each admin decides per server whether people there may start them.
- **Delivery**: events wait in an outbox table, per server and in order, and are sent in batches of 50. Failures back off up to an hour and are retried until delivered; Admin → Connections shows the backlog. Disconnecting ends every share and drops what's waiting.
- **Not shared**: polls (shared channels refuse new ones), moderation and accounts. Each server moderates its own people.

## Context

On 2026-09-29 Niklas asked for "something like Slack Connect": two admins agree, one sends a request by URL, people chat, make groups and write directly across servers, all done safely with a key exchange.

- ActivityPub and Matrix were considered. Both are open networks: any server can talk to any other, with discovery, relays and moderation problems that don't fit a small team server. Their data models (actors and inboxes, or replicated room DAGs) are much larger than one channel's messages, and neither matches Slack Connect's model, where two organisations agree to connect. Implementing either would add a large surface for little benefit, since Sideporch servers only talk to each other.
- HTTP message signatures (RFC 9421) were considered for the signing. The scheme here is simpler: a fixed string over fixed headers, with no negotiation. Ed25519 through `ring`, already a dependency, keeps the keys small and the fingerprints easy to read aloud.
- A hub-and-spoke model per channel (the host relays) keeps ordering and authority simple: one server decides membership and every copy converges on its order. Letting every guest talk to every other guest would need them all to be connected to each other.
- Remote people as local accounts reuse everything (message rendering, mentions, notifications, search) without a second kind of author. The costs are guards wherever an account could be taken over, which `tests/federation.rs` covers.

## Consequences

- The protocol is Sideporch's own. Changes to event shapes must stay readable by older servers: add optional fields, and don't change the signed string without a new version prefix (`sideporch-request-v1`).
- Anything that gives an account a way in (reset links, sign-in methods, admin rights) must refuse accounts from other servers.
- A connected server can post as its own people in shared channels and fetch files attached in them; admins should only connect to servers they trust, which is why connecting takes both admins and a fingerprint check.
- Polls, and features built later, stay local until the protocol carries them; shared channels must refuse what can't be shared, rather than let copies drift.
- The server key is sealed with the secrets key, so a backup that leaves out the secrets key loses the server's identity; connected servers would have to connect again.
