+++
schema_version = 1
id = "01M6XJTFEB7VFHE165Z1ACN7D2"
title = "Give bots their own scoped API tokens, limited to public channels"
date = "2026-11-08"
status = "proposed"
tags = []
supersedes = []
superseded_by = []
depends_on = []
related_to = []
+++
## Decision

Bots are named accounts with their own API tokens, separate from people's accounts and from MCP tokens. Each token has scopes (`channels:read`, `messages:read`, `messages:write`) and a channel list that is either every public channel or the public channels named. Bots reach public channels only, and a channel is looked up again on every request, so one made private later stops working at once. Tokens are stored as SHA-256 hashes, checked with a constant-time comparison, and shown once. Bot posts go through the same `messages::post` pipeline as every other message, and each bot message records its bot's id. Bots are not managers: they post or reply only where the channel lets anyone.

## Context

Sideporch had two ways in for programs: incoming webhooks, which only post, and MCP tokens, which act as an admin with access to everything. Neither gives a script a scoped identity it can read and post with. Alternatives considered:

- **MCP or admin tokens for bots.** Rejected: such a token acts as an admin across every channel, including private ones, and revoking it would break the admin's own agent sessions.
- **Bots as ordinary people's accounts.** Rejected: they would need sign-in methods, would appear in the people list and take part in trust levels, and would need their own password resets.
- **Explicit lists that may include private channels.** Rejected: automations already see public channels only, and a bot should follow the same boundary. A list therefore checks public status at request time instead of trusting the list written at creation.
- **A new rate limiter for tokens.** Rejected in favour of the existing per-key limiter in `src/access.rs`, with one more key kind.

## Consequences

- A bot can never read or write a private channel or a direct message, whatever its token lists. Admins who want a bot in a private channel must use a person's account or an automation.
- Outgoing webhooks still carry only people's messages, because their payload is person-shaped. Bot posts reach live clients, push, search, federation and automations, but not outgoing webhooks. Changing that is a separate decision.
- Revoking a bot revokes all its tokens, and revoking a token takes effect on the next request. Bots are revoked rather than deleted, so their messages keep a bot id that still means something.
- Last use is written at most every five minutes, so a busy bot adds little write load.
- The per-token rate limit is in memory and starts over after a restart, as the other limits do.
- Bots are not tied to the admin who created them: removing that admin's rights does not revoke their bots. Revisit if that's wrong for your team.
