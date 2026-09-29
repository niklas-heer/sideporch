+++
schema_version = 1
id = "01M3P2TSPAVERYFTHH7KA1RMS5"
title = "Offer passkeys, authenticator codes and email links, and let admins require them"
date = "2026-09-29"
status = "accepted"
tags = ["security", "accounts"]
supersedes = []
superseded_by = []
depends_on = []
related_to = []
+++
## Decision

Sideporch offers passkeys, authenticator-app codes (TOTP) with recovery codes, and email links, and lets admins require them:

- **Passkeys** are verified by Sideporch itself (`src/passkeys.rs`), following WebAuthn Level 2 for attestation `none`. The browser's `getPublicKey()` supplies the key as SPKI, so there is no CBOR parsing; signatures (ES256, EdDSA, RS256) are checked with ring, which rustls already brings in. User verification is required, discoverable credentials are requested, and counters that go backwards are refused. Challenges live in memory for five minutes and work once. Browsers only allow passkeys on domain names, so Sideporch explains when it is opened by IP address.
- **TOTP** follows RFC 6238 (SHA-1, 6 digits, 30 s, ±1 step), with ring's HMAC. Secrets are sealed with the secret key; a code's step can't be used twice. Setting up a second step makes ten recovery codes, stored as SHA-256 hashes.
- **Email** goes through an admin-configured SMTP server with lettre (rustls). It confirms addresses, sends single-use sign-in links (15 minutes, behind a button because mail scanners open links) and password resets. Replies never reveal whether an address has an account, and each account gets at most five links an hour.
- Every sign-in path (password, email link, password reset) ends in the same function, which asks for the second step when the account has a passkey or app. A passkey sign-in counts as both steps. Pending second steps allow five attempts.
- The policy is one of: a password is enough, admins need a second step, everyone does, or everyone signs in with a passkey (a password only works until someone has one). People who don't meet it are redirected to Sign-in and security until they do; admins must meet a policy before choosing it, so they can't lock themselves out. Admins can reset someone's factors after a lost device.

## Context

On 2026-09-29 Niklas asked for passkeys, for admins to change the authentication method or require multi-factor authentication, to require passkeys for everyone, and maybe magic links, to run a public instance safely.

Alternatives:

- `webauthn-rs`, the established crate, needs OpenSSL, which would break the static musl builds and the `FROM scratch` image. Newer pure-Rust crates (`passkey-rp`, `passkeep`) are months old with little use. The verification Sideporch needs is small and is covered by unit tests and an end-to-end test with a software authenticator, and it was checked in Chromium with a virtual authenticator.
- `totp-rs` would add a crate for about 30 lines on top of ring.
- A hand-written SMTP client would avoid lettre, but mail servers vary in authentication and TLS details that lettre already handles.

## Consequences

- Passkey verification is Sideporch's to maintain. Attestation is not checked (`none`), so Sideporch doesn't know which device made a passkey; that isn't needed for sign-in.
- Email is optional; without SMTP, sign-in links and self-service password resets are hidden and admins hand out reset links as before.
- Removing a passkey or turning off the app doesn't ask for the password again; a stolen signed-in session could weaken the account. Revisit with re-authentication for sensitive changes.
- API tokens for MCP stay separate credentials and aren't affected by the sign-in policy.
