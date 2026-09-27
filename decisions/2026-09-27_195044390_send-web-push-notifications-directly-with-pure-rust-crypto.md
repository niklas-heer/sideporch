+++
schema_version = 1
id = "01M3J6PYX6SHYR5CPG4FMNEG69"
title = "Send Web Push notifications directly with pure-Rust crypto"
date = "2026-09-27"
status = "accepted"
tags = ["notifications", "packaging"]
supersedes = []
superseded_by = []
depends_on = ["01M3G6RCQKT16B893YJKV7YY7K"]
related_to = []
+++
## Decision

Sideporch sends Web Push notifications itself, with no third-party notification service.

- Browsers subscribe through a service worker (`/sw.js`). The server signs requests with its own VAPID key, generated on first start and kept in the `settings` table.
- Payloads are encrypted with [`web-push-native`](https://crates.io/crates/web-push-native) (pure Rust, RustCrypto) and sent with hyper and rustls using the ring provider and bundled Mozilla root certificates (`webpki-roots`).
- Notifications go to the other members of a direct conversation, to people in a thread, and to anyone `@mentioned` (`@channel` and `@here` reach everyone in a public channel). People with a visible Sideporch tab open are skipped.
- Only `https://` push endpoints are accepted. Subscriptions a push service reports as gone (404 or 410) are deleted.

## Context

Niklas asked for push notifications on 2026-09-27, in the same request that asked for a static binary with no libc dependency.

- **`web-push`**, the most-used crate, encrypts through OpenSSL by default. That complicates a static musl build.
- **reqwest 0.13** verifies certificates with the platform verifier, which reads the operating system's certificate store. A `FROM scratch` container has none.
- **A hosted notification service** (Firebase, OneSignal) would add an account and a third party to a self-hosted chat.

`web-push-native` 0.5.0 (updated 2026-07-26) and its `jwt-simple` dependency use pure-Rust cryptography. hyper-rustls with `webpki-roots` needs nothing from the host.

## Consequences

- iPhones and iPads deliver web push only to sites added to the home screen (iOS 16.4 and later). Sideporch ships a web app manifest for that.
- The bundled root certificates change only when Sideporch is rebuilt, so releases should keep dependencies current.
- The VAPID key lives in the database; restoring a backup keeps existing browser subscriptions working.
- Accepting only HTTPS endpoints stops members from pointing the server at plain-HTTP internal services. Tests use a hidden `allow_insecure_push` option to talk to a local fake push service.
