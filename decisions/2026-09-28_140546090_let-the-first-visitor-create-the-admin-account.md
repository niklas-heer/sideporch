+++
schema_version = 1
id = "01M3M5C0QATQT5YVGTMZA1KDAV"
title = "Let the first visitor create the admin account"
date = "2026-09-28"
status = "accepted"
tags = ["onboarding", "security"]
supersedes = []
superseded_by = []
depends_on = []
related_to = []
+++
## Decision

A fresh Sideporch is set up in the browser. While no account exists, every visit leads to `/setup`, and the first person to submit it becomes the admin. The page says so. After that, `/setup` redirects to the sign-in page, and new people need an invite link.

`--require-setup-link` (`SIDEPORCH_REQUIRE_SETUP_LINK=true`) restores a one-time secret link at `/setup/<token>`. The link is saved to a file only the server's user can read, printed by `sideporch setup-link`, kept out of non-interactive logs, and deleted once used. In this mode `/setup` does not exist.

## Context

On 2026-09-28 Niklas found the first start awkward, "especially on Docker, because we don't ship any shell", and suggested that "the first person to visit the website is assumed to be the admin", as other tools do.

- Uptime Kuma and Home Assistant use first-visitor onboarding. Gitea and Grafana instead show an install page or ship default credentials.
- The risk is a race: whoever reaches a new, publicly reachable instance first can claim it. Most people start Sideporch and open it right away, often on a private network or behind a proxy they are still configuring. The lock covers the rest.
- A day earlier the setup link had been moved out of logs because service logs are often readable by others. That still applies in locked mode. In open mode the logged URL holds no secret.

## Consequences

- The Homebrew formula test and the release smoke test check for a `/setup` link, not `/setup/<token>`.
- Deployments exposed to the internet before setup should use `--require-setup-link`; the README and the setup page point to it.
