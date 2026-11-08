+++
schema_version = 1
id = "01M6WGE8E3W5FR0VQD568Z1XSQ"
title = "Let new accounts start with a passkey instead of a password"
date = "2026-11-08"
status = "accepted"
tags = ["security", "accounts", "onboarding"]
supersedes = []
superseded_by = []
depends_on = []
related_to = ["01M3P2TSPAVERYFTHH7KA1RMS5", "01M6WEV37Q7ZMSRQTZ3CXBM378"]
+++
## Decision

New accounts can sign in with a passkey from the start, instead of a password. The setup page, invite links and open sign-up show **Create account with a passkey** above the password field whenever the browser can make passkeys and Sideporch is reached by a name, not an IP address; the password stays as the quieter choice, and the only one without JavaScript.

- `POST /webauthn/create/options` starts a ceremony bound to the form (`/setup`, `/setup/<token>`, `/join/<token>`, `/signup`), the name and the username. It only works while that form can make an account, checks a setup code when setup needs one (counting wrong codes like `/setup`), and is limited to 20 per address in 15 minutes. It generates the account's `WebAuthn` user handle.
- app.js makes the passkey and sends the form with the answer instead of a password. The form's own handler keeps every check it already makes (setup code, invite, proof of work, rules), then verifies the passkey for that form, name and username and creates the account, its handle and its passkey in one transaction. A cancelled or failed attempt leaves no account and no used invite.
- Passkey accounts store an empty password hash, which `auth::verify_password` never accepts, as with imported accounts; no schema change.
- A new passkey counts as both sign-in steps, like signing in with one, so the account is signed in right away and meets every policy.
- Sign-ups that wait for approval keep using a password, because the waiting row stores a password hash until someone approves it.
- Lockouts: an account without a password adds one under Account without an old one; its only passkey can't be removed until it has another way in; and `sideporch reset-link USERNAME` prints a 24-hour reset link from the server, for an admin with nobody else to ask. A reset link still asks for the second step afterwards, so `--reset-sign-in` also removes the passkeys and authenticator app when those are what was lost; the council review found the lockout without it.

## Context

On 2026-11-08 Niklas asked to "provide passkeys by default" to make setups easy. A council review (Claude Opus 5.5 and GPT-6.1 Sol) preferred creating the account with a passkey over creating a password account and adding a passkey right after, because inventing a password is the friction to remove. Both pointed out that a passkey-only account needs a way back in, which is why the reset command and the guards above came with it.

GPT-6.1 Sol suggested asking for a fresh assertion before trusting a new credential, since registration with attestation `none` doesn't prove the device holds the private key. Not done: the person is creating their own new account, so a key they don't hold only locks themselves out, and that is true of every `WebAuthn` registration without attestation. Credential ids are unique, so a registration can't take over someone else's passkey.

## Consequences

- Visitors to the demo, which starts over every day, may keep passkeys in their password manager for accounts that no longer exist; signing in with one fails with a message, and they can sign up again.
- Moving a server to another domain name breaks every passkey made for the old one; people without a password then need a reset link. The documentation says to pick the final name before inviting people.
- Setting a password on a passkey-only account doesn't ask for the passkey again, like removing a passkey today; a stolen signed-in session could add one. Revisit with re-authentication for sensitive changes.
