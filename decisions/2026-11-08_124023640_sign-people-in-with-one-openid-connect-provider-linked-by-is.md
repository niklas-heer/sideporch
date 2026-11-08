+++
schema_version = 1
id = "01M6XJS5ARDBFV4WSHHHTZS446"
title = "Sign people in with one OpenID Connect provider, linked by issuer and subject"
date = "2026-11-08"
status = "proposed"
tags = ["security", "sign-in"]
supersedes = []
superseded_by = []
depends_on = []
related_to = []
+++
## Decision

Sideporch can sign people in through **one** OpenID Connect provider that an admin configures on **Admin → Sign-in**, next to the sign-in policy. The provider's sign-in is a first step, like a password: a passkey, authenticator app or the policy still applies after it.

- **Configuration** is stored in the settings table. The client secret is sealed with the vault (`src/secrets.rs`), is never rendered back, and an empty secret field keeps it. The issuer must be `https`; `http` is allowed only for loopback addresses, for tests.
- **Flow** (`src/oidc.rs`, `src/routes/oidc.rs`): authorization code with PKCE S256, a random `state` and a random `nonce`. Discovery comes from `{issuer}/.well-known/openid-configuration`, and its `issuer` must equal the configured one exactly. The login is recorded server-side with a ten-minute life, bound to the browser by a cookie only that browser has, and used once. A callback that doesn't match the cookie is refused without using up the login another browser started.
- **ID tokens** are checked with ring, not a JWT crate: only RS256 and ES256 are read, and `none` and HMAC algorithms are refused before anything else. The key comes from the provider's own key set, chosen by a `kid` the token must name, and an unknown `kid` makes Sideporch ask again at most once a minute. The claims must match: `iss` (exactly), `aud` (with `azp` when there are several audiences), `exp` and `iat` with 60 seconds of skew, and `nonce` in constant time.
- **Accounts** are bound by `(issuer, subject)` in `oidc_identities`. Email never links: an address the provider gives is not proof that someone owns a local account. A signed-in person links from **Sign-in and security**, with the same session recheck as adding a passkey or an Access address.
- **First sign-in** creates an account only when the admin turned that on and sign-up is open. The username comes from `preferred_username`, with `-2`, `-3`… while taken.
- **Refusals**: other-server accounts (`instance_id`), deactivated accounts, accounts under recovery, banned email addresses, and accounts that require a passkey (until they have one) are refused. Visitors see one generic message; the reason goes to the server log, never with a token or the secret.
- **Timeouts and limits**: every request has a ten-second deadline. Discovery and key documents are capped at 256 KB, and ID tokens at 16 KB.

## Context

- **Why not an OIDC or JWT crate**: the project's existing signature checks (Cloudflare Access, passkeys) use ring directly, and a JWT crate would add a second verifier with its own defaults. The algorithm allowlist and claim checks are short and tested.
- **Why not link by email**: the brief's security rule. Linking by an email claim lets whoever controls a provider account with a matching address take over that local account.
- **Why one provider**: each provider needs its own admin-side trust decision (its issuer, its secret, its domain rule). Several providers would multiply the linking and recovery rules without a current need.
- **Why the internal-address guard does not apply**: the issuer is operator-configured, and self-hosted providers often sit on private networks. The guard is for addresses users or automations supply.
- **Why the redirect URI comes from the request's address** (as invite links do) unless a public address is set: the provider only sends codes to registered URIs, and the browser cookie and state stop a forged callback.

## Consequences

- Operators must register `<address>/login/oidc/callback` with the provider and use an `https` issuer.
- Providers that sign with other algorithms (EdDSA, RS512, PS256) or publish no `kid` are refused. A provider that lists its PKCE methods without S256 is refused too.
- Changing the issuer leaves existing links for the old issuer in place but unused; they work again if the old issuer is restored.
- Unlinking is refused when it would leave no way in (no password, passkey or confirmed email).
- Recovery and admin resets (**Reset passkeys and app**) remove an account's provider links with its passkeys.
- Adding another provider later means a schema and settings change, not a rewrite of the flows.

## Verification

- Unit tests in `src/oidc.rs`: issuer validation, the RFC 7636 PKCE example, algorithm and `kid` parsing, claim checks (issuer, audience, `azp`, expiry, future `iat`, nonce, subject, domain with verified email), and username derivation with collision suffixes.
- `tests/oidc.rs` runs a real server against a fake provider on a random port that serves discovery, keys and the token endpoint, checks PKCE, client authentication and single-use codes, and signs tokens with test keys. It covers the happy path, link and sign-in, ES256, second steps still asked, first sign-in refused without creation, email matching a local account not signing in or linking, a sign-in from another browser, a wrong state, a replayed callback, wrong nonce, audience and issuer, expired tokens, `alg: none`, HS256 with the public key, an unknown `kid`, deactivated and other-server accounts, and the client secret never appearing on a page.
- Not covered by the test suite: a live provider (Keycloak, Entra, Google). The fake follows the specification's shapes, not any one provider's quirks.
