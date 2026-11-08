+++
schema_version = 1
id = "01M6WEV37Q7ZMSRQTZ3CXBM378"
title = "Let operators lock setup with a setup code they choose"
date = "2026-11-08"
status = "accepted"
tags = ["onboarding", "security"]
supersedes = []
superseded_by = []
depends_on = []
related_to = ["01M3M5C0QATQT5YVGTMZA1KDAV"]
+++
## Decision

Besides the first-visitor default and the random setup link, an operator can choose a **setup code** ahead of time: `--setup-code` / `SIDEPORCH_SETUP_CODE`. While no account exists, `/setup` asks for it next to the account fields.

- The code must have at least 16 characters; Sideporch refuses to start with a shorter one and suggests `openssl rand -base64 24`.
- It is only accepted in the form, never as `/setup/<code>`, so it stays out of browser history and proxy logs. It isn't written to `<data>/setup-link`, because the operator already keeps it.
- Wrong codes are counted per client address (10 in 15 minutes) and checked before the password is hashed.
- When both a code and `--require-setup-link` are set, the code wins. The random link keeps working as before, and `/setup` now asks for its token too, instead of answering 404.
- A server that is locked in either way sends visitors to `/setup` instead of showing a 503 page that told them to run a command.

## Context

On 2026-11-08 the live demo on Fly.io had been deployed for five weeks with `SIDEPORCH_REQUIRE_SETUP_LINK=true`, and nobody had fetched the link, which needs `fly machine exec` into a `FROM scratch` image. Niklas asked for setups that are easy, with "a toggle for a more secure setup". On a platform without a shell, a secret set before the first start (a Fly secret, a Docker env var, a Kubernetes secret, kept in a password manager) is easier than reading a file from the server.

A council review (Claude Opus 5.5 and GPT-6.1 Sol) agreed on a POSTed code over a code in the URL, on a minimum length, and on keeping the link out of logs as decided in "Let the first visitor create the admin account".

Not chosen:

- Printing the random link to stdout on first start: container and platform logs are often readable by others, which is why it left the logs.
- A global limit on attempts across all addresses: anyone could use it to keep the owner out, and with 16 or more characters of a generated code, spreading guesses over many addresses doesn't make guessing feasible.

## Consequences

- Hosted deployments, including the demo (`deploy/fly/`), use a setup code kept in 1Password instead of the setup link.
- A weak code of 16 characters, such as a repeated letter, passes the length check; the documentation says to generate one.
- Once the first account exists the code has no effect, so it can stay set; if the data is ever wiped, setup is still locked.
