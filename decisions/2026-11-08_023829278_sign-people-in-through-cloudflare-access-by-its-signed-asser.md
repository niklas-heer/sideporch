+++
schema_version = 1
id = "01M6WGB1CY3A2G0682WP9AMP4E"
title = "Sign people in through Cloudflare Access by its signed assertion"
date = "2026-11-08"
status = "accepted"
tags = ["security", "sign-in", "onboarding"]
supersedes = []
superseded_by = []
depends_on = []
related_to = ["01M6WEV37Q7ZMSRQTZ3CXBM378"]
+++
## Decision

Sideporch can trust **Cloudflare Access** in front of it. With `SIDEPORCH_CLOUDFLARE_ACCESS_TEAM` (the team domain; `myteam`, `myteam.cloudflareaccess.com` or an `https://` URL) and `SIDEPORCH_CLOUDFLARE_ACCESS_AUD` (the application's audience tag) set, someone Access let in is signed in to the account whose confirmed address matches the one in Access's signed `Cf-Access-Jwt-Assertion` header. Configuration is only by option or environment variable; one without the other refuses to start.

- **Verification** (`src/cloudflare.rs`): only RS256, the key chosen by `kid` from `https://<team>/cdn-cgi/access/certs` (never from a URL in the token), checked with ring. `iss` must be `https://<team>`, `aud` must contain the tag, `exp`/`nbf`/`iat` hold with 60 seconds of clock skew, `type` is `app` when present, and a person needs a non-empty `email`. Tokens with a `common_name` are service tokens and never sign anyone in.
- **Keys** are fetched at start and hourly; an unknown `kid` asks Cloudflare again at most once a minute. If Cloudflare can't be reached, Access sign-in fails closed and passwords, passkeys and everything else keep working.
- **Only where people sign in**: `/login` (GET; every page without a session leads there) and first-run setup read the assertion. Requests with a session never do, and webhooks, federation and MCP tokens never look at it.
- **Access is a first step**, ending in `routes::security::after_first_step`, so a second step still applies. Like a password, it doesn't sign in an account that has a passkey when the policy requires passkeys.
- **Matching** uses `lower(email)` against confirmed addresses of local (`instance_id IS NULL`), active accounts.
- **Unknown addresses** get an explanation on the sign-in page, and the password sign-in then leads to `/settings/cloudflare-access`, which offers to use the address Access vouches for as the account's confirmed address (refused if another active account has it, or the address is banned). New people still join by invite or the normal sign-up; we did not add pre-confirmed addresses to sign-up, which another change was reworking at the time.
- **Setup**: with Access configured, `/setup` (and the one-time link) also need a valid personal assertion, in addition to a setup code if one is set. Without a code, Access is the claim, and the page says so. The address becomes the first admin's confirmed address.
- **Sign out** goes to `/cdn-cgi/access/logout` after ending the session, so Access doesn't sign the person right back in.
- **`SIDEPORCH_CLOUDFLARE_ACCESS_REQUIRED=true`** refuses every page without a valid assertion (people or service tokens), except `/healthz`, assets the browser loads for the app, `/hooks/…`, `/federation/…`, `/.well-known/sideporch` and `/mcp`, which authenticate themselves.

## Context

On 2026-11-08 Niklas asked for "Cloudflare auth". A design council (Claude Opus 5.5 and GPT Sol) recommended trusting Cloudflare Access's signed header now rather than waiting for generic OpenID Connect, which stays a separate agreed future item in `docs/future-ideas.md`: Access already does the identity-provider work, needs no client secret or redirect flow in Sideporch, and gives operators SSO with whatever provider their Cloudflare account uses.

Not chosen:

- Trusting the `Cf-Access-Authenticated-User-Email` header: anyone reaching the origin directly can send it. The JWT is signed.
- Checking the assertion on every request: sessions already authenticate requests, and per-request identity would make Access outages and key rotation break signed-in people.
- An admin page for the settings: the team and audience belong with the deployment's other secrets, and a page would let a compromised admin account point sign-in at another Cloudflare team.
- Creating accounts automatically for unknown addresses: Sideporch's sign-up modes, invites and approval queue decide who joins.

## Consequences

- Operators must add Bypass applications in Cloudflare for `/hooks/`, `/federation/`, `/.well-known/sideporch`, `/mcp` and `/healthz`, or programs can't reach them; the documentation lists them.
- Without a Cloudflare Tunnel or the required mode, the origin may still be reachable directly, where the normal sign-in form applies. `SIDEPORCH_CLIENT_IP_HEADER=CF-Connecting-IP` is only safe when every request comes through Cloudflare.
- In required mode, a Cloudflare outage before the first key fetch closes pages until keys arrive; keys fetched earlier keep working.
- The setup page gained a Cloudflare Access mode, and the first-account code takes the Access address; a future rework of account creation must keep passing it on.
- Linking the Access address to an account (`/settings/cloudflare-access`) only needs a signed-in session, and replaces the account's confirmed email. Someone with a stolen session could link their own Access identity; a second step still applies where the account has one. Revisit together with re-authentication for sensitive changes, as noted for passkeys.
