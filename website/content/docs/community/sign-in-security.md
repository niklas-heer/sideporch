+++
title = "Sign-in and security"
description = "Passkeys, authenticator apps and email sign-in links, and what signing in takes on your server."
weight = 4
+++

Everyone manages how they sign in under **Sign-in and security** in their account menu. Admins decide what signing in must take under **Admin → Sign-in**.

## Ways to sign in

- **Passkeys**: sign in with a fingerprint, face or device PIN, from the sign-in form's autofill or its passkey button. They can't be phished or guessed, and they sync between your devices.
- **Authenticator apps** (1Password, Google Authenticator, Aegis…) ask for a six-digit code after the password. Setting one up gives you ten single-use **recovery codes**, for when you lose your phone.
- **Email**: with an [SMTP server set up](#email), people confirm an address and can sign in with a link, or reset a forgotten password themselves. Links work once, for 15 minutes.

{% <note> %}
Passkeys need Sideporch opened by a name (like `chat.example.com` over HTTPS, or `localhost`), not by an IP address: browsers only use them there.
{% </note> %}

## What signing in takes

Under **Admin → Sign-in**, pick one:

{{<shot name="sign-in" alt="Admin, Sign-in: four choices for what signing in takes, the option to sign in with a link by email, and the SMTP server fields." caption="What signing in takes, sign-in links by email, and the SMTP server." />}}

| Setting | Means |
| --- | --- |
| **A password is enough** (the default) | Everyone can add a passkey or an authenticator app for a second step. |
| **Admins need a second step** | Admins must add a passkey or an authenticator app. |
| **Everyone needs a second step** | Everyone must add a passkey or an authenticator app before they continue. |
| **Everyone signs in with a passkey** | People without one add one at their next sign-in; after that, passwords no longer sign them in. |

People who don't meet the setting yet are walked through adding what's missing at their next sign-in. Every way of signing in, including email links, goes through the same second step, so it can't be skipped.

After a lost phone, an admin resets someone's passkeys and authenticator app from their profile; they set them up again at their next sign-in.

## Email

Sideporch needs no email. To send sign-in links, password resets and address confirmations, give it an SMTP server under **Admin → Sign-in**:

| Field | For example |
| --- | --- |
| Server | `smtp.example.com` |
| Port | `587` |
| Security | STARTTLS (usually port 587), TLS (usually port 465), or none, for a relay on this network |
| Username and password | from your mail provider |
| Send as | `Sideporch <chat@example.com>` |

The password is encrypted with the secret key, like [automation secrets](@/docs/integrations/automations.md#secrets). **Send a test email** checks the settings.

Then turn on **Sign in with a link by email** if people may sign in with a link instead of their password. The link counts as the first step; a passkey or app code is still asked for where set up.

Sideporch doesn't send notifications by email, only these sign-in messages.
