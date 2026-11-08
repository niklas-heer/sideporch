+++
title = "Single sign-on with OpenID Connect"
description = "Let people sign in with your team's identity provider, such as Keycloak, Authentik, Microsoft Entra or Google, and how accounts are linked to it."
weight = 5
+++

Sideporch can sign people in with one OpenID Connect provider, the account system your team already uses. An admin sets it up once; then each person links their account to their provider sign-in, and signs in with a button on the sign-in page. New in 0.7.0.

## Set up the provider

In your identity provider, create an OpenID Connect client for a web application:

- **Redirect URI**: your Sideporch address followed by `/login/oidc/callback`, such as `https://chat.example.com/login/oidc/callback`. The address must be the one people open, because the provider sends them back there.
- **Client authentication**: a client secret (Sideporch sends it with HTTP Basic).
- **Scopes**: `openid`, `profile` and `email` (Sideporch asks for these).
- **PKCE**: Sideporch always sends a code challenge with the S256 method. If the provider lists its supported methods, S256 must be among them.

Keep the client id and the secret. The secret is sealed with the server's secret key and is never shown again.

## Turn it on

Go to **Admin → Sign-in**, and fill in **Single sign-on**:

- **Issuer**: the provider's address exactly as its discovery document names it, such as `https://login.example.com/realms/team`. Sideporch reads `/.well-known/openid-configuration` under it and refuses a document that names another issuer. It must use `https`.
- **Client id** and **client secret** from the provider.
- **Button label**: what the sign-in page shows, such as "Sign in with Example SSO".
- **Only this email domain** (optional): only people whose verified email address is in this domain can sign in with the provider.
- **Create an account on a first sign-in**: off by default. When on, a first sign-in makes a new account, but only while [sign-up is open](@/docs/community/people/sign-up-and-trust.md) and the server has no rules to agree to, because the sign-up form is where people agree to them. Otherwise a first sign-in is refused, and the person has to ask to join or be invited first.

A new account's username comes from the provider's `preferred_username` (an address keeps the part before the `@`), with `-2`, `-3` and so on added if it's taken. Its display name comes from `name`.

## Link an account

Each person links their account once, under **Sign-in and security → Single sign-on → Link this account**. Sideporch then sends them to the provider, and brings them back linked to their account. After that, the button on the sign-in page signs them in.

Accounts are linked by the provider's identity (its issuer and the subject it gives each person), never by email address. An email claim doesn't prove that someone owns an account here, so a provider account with the same address as a local account signs in to nothing, and links nothing, until its owner links it from that account.

Unlinking, under the same section, works only when the account has another way in: a password, a passkey or a confirmed email address.

## What still applies

- **Second steps**: signing in with the provider is a first step, like a password. An authenticator app or passkey is still asked for after it, and an admin's policy that requires a passkey still applies: once someone has one, the provider alone doesn't sign them in.
- **Accounts under recovery**: while an account is being recovered from the server (see [sign-in and security](@/docs/community/people/sign-in-security.md)), the provider doesn't sign it in. Recovery also removes its provider links, as it removes passkeys.
- **Deactivated accounts** and accounts from other servers are refused.
- **Email bans** apply to the address the provider gives.
- **Cloudflare Access**: when the server requires Access, the provider's redirect still has to come through it, like every other page.
- **Admin resets**: **Reset passkeys and app** on someone's profile also removes their provider links, for when they lost their devices.

## What the tokens must pass

Sideporch accepts an ID token only when all of these hold; otherwise the sign-in is refused:

- It's signed with RS256 or ES256, by a key the provider publishes at its `jwks_uri` under the `kid` the token names. `none` and HMAC algorithms are refused.
- Its issuer is exactly the configured issuer, and its audience contains the client id (with `azp` when there are several audiences).
- It isn't expired, and its `iat` is not in the future, with a minute of clock skew allowed.
- Its nonce matches the sign-in that started it.

The state that ties the redirect to the browser that started it lasts ten minutes and is used once.

## When a sign-in fails

The visitor sees a short generic message. The reason is in the server's log, in a line such as `could not finish an OpenID Connect sign-in` with a `reason` field. The log never contains tokens or the client secret. Common reasons are a redirect URI that doesn't match the one registered with the provider, a wrong client secret, an issuer that doesn't match the discovery document, and clocks that disagree by more than a minute.

To turn it off, use **Turn off sign-in with this provider** on the same admin page. People keep their links, which work again if the provider is set up again with the same issuer.
