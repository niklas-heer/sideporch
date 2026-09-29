+++
title = "Security"
description = "How Sideporch signs people in, keeps sessions, limits abuse, stores secrets and checks updates."
weight = 4
+++

## Signing in

Passwords are hashed with Argon2id. Besides passwords, people sign in with **passkeys** (WebAuthn, checked on the server with pure-Rust cryptography), **authenticator codes** (TOTP, with recovery codes) and **email links**. Admins decide what signing in takes; see [Sign-in and security](@/docs/community/people/sign-in-security.md).

{% <diagram caption="Every way of signing in ends in the same place, so a required second step can't be skipped."> %}
flowchart LR
  P[Password] --> F[First step done]
  K[Passkey] --> F
  E[Email link] --> F
  F --> Q{Policy asks for<br/>a second step?}
  Q -- yes --> S[Passkey or<br/>authenticator code]
  Q -- no --> N[Session starts]
  S --> N
{% </diagram> %}

## Sessions

A session is a random token in a cookie that is `HttpOnly`, `SameSite=Lax`, and `Secure` over HTTPS. The database only keeps its SHA-256 hash, so a copy of the database can't be used to sign in. Requests that change something must come from Sideporch's own pages; others are refused. Pages are served with a strict Content Security Policy: scripts only from Sideporch itself, no inline scripts, no embedding in other sites.

## Limits and bans

- Wrong passwords pause signing in per address and per account; sign-ups are limited per address and across the server; the sign-up form asks the browser for a small proof of work, which keeps scripts from signing up by the thousand without any outside CAPTCHA service.
- Moderators ban accounts, addresses and ranges, email addresses and domains; see [Moderation](@/docs/community/people/moderation.md#bans).
- Behind a reverse proxy, Sideporch only trusts the header named by `--client-ip-header`, so nobody can claim another address.

## Secrets

API tokens for automations and the AI provider are encrypted with AES-256-GCM, with a key kept outside the database (`secret.key`, or a passphrase in an environment variable). A stolen database alone doesn't reveal them.

## Updates

Every release's checksums are signed with the project's release key, whose public half is built into Sideporch. Before replacing itself, Sideporch checks that signature and that the download matches the checksums; the install script does the same when `minisign` is available. See [Update](@/docs/get-started/update.md).

## Connected servers

Servers connect only when an admin on each side agrees. Each server has an Ed25519 key; every request between servers is signed, carries a one-time value so it can't be replayed, and is checked against the key pinned when they connected. A server may only speak for its own people.
