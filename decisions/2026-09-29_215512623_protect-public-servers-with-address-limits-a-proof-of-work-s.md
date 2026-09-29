+++
schema_version = 1
id = "01M3QJMA3FCC89D3QFQXGKQRPA"
title = "Protect public servers with address limits, a proof-of-work sign-up and bans"
date = "2026-09-29"
status = "accepted"
tags = ["security", "community"]
supersedes = []
superseded_by = []
depends_on = []
related_to = ["01M3P1TDCJAJ2ZB99F0QWPE0CR"]
+++
## Decision

Sideporch protects servers open to the public with what it can do alone, without outside services:

- **Client addresses** come from the connection, or from one header a trusted reverse proxy sets (`--client-ip-header`, such as `X-Forwarded-For` or `Fly-Client-IP`). Without the setting, such headers are ignored.
- **Limits in memory**: 10 wrong passwords per address and 20 per account within 15 minutes pause signing in there; 3 sign-ups an hour per address, next to 30 an hour overall.
- **A proof of work on sign-up**: the form carries a one-time challenge; the browser finds a nonce whose SHA-256 of `challenge:nonce` starts with 18 zero bits (a quarter of a million hashes, well under a second on a desktop). The server checks it and accepts each challenge once, within an hour.
- **Bans** by address or range (IPv4 and IPv6, CIDR), by email address and by email domain, for a day, a week, 30 days or until lifted. Banned addresses get a short refusal for every request except the health check. Banning a person deactivates them and can ban their recent addresses and email and remove all their messages, reactions and votes. Admins can't be banned; nobody can ban a range covering their own address.
- **Recent addresses per account**: noted at most once a day while signed in, kept 30 days, shown only to admins and moderators on the profile, with other accounts that used them.

## Context

On 2026-09-29 Niklas asked whether Sideporch is secure enough to run as a public instance, and for bans including IP bans and something against spam from bots, before hosting a public demo. He agreed that sign-in addresses may be stored for 30 days.

- **A third-party CAPTCHA** (reCAPTCHA, hCaptcha, Turnstile) would stop more bots, but sends every visitor to another company, needs keys, and goes against running on your own. A proof of work stops scripts that don't run JavaScript and makes mass sign-ups cost CPU, with nothing to solve for people.
- **Limits in the database** would survive restarts, but a restart already costs an attacker their progress on nothing, and the counters change on every failed attempt.
- **Trusting `X-Forwarded-For` by default** would make limits and bans useless: anyone could send any address. It must be switched on for a proxy the admin controls.
- **Storing no addresses** would respect privacy most, but moderators couldn't see which addresses a spammer used, or that several accounts share one.

## Consequences

- Signing up needs JavaScript. Invite links don't, since an invite is trust already.
- Shared addresses (offices, schools, mobile carriers' NAT) can be caught by address bans; the profile shows other accounts per address, and the docs warn about it.
- Behind a proxy without `--client-ip-header`, every visitor looks like one address, so limits hit everyone at once. The docs for running behind HTTPS say to set it.
- A determined attacker with many addresses and CPU can still sign up; time-outs, trust levels and bans remain the tools for that.
