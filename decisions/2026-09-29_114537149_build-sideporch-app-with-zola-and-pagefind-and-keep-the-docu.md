+++
schema_version = 1
id = "01M3PFR3VXT8N94T3QY94WYDXN"
title = "Build sideporch.app with Zola and Pagefind and keep the documentation there"
date = "2026-09-29"
status = "accepted"
tags = ["website", "documentation", "tooling"]
supersedes = []
superseded_by = []
depends_on = []
related_to = ["01M3HRKXN442S40GZ9AGYGQHH1", "01M3J7NEGN00V51QXJAHX99QWB"]
+++
## Decision

sideporch.app is a [Zola](https://www.getzola.org) site in `website/`, with our own templates in Sideporch's palette and typeface, and [Pagefind](https://pagefind.app) for search. It holds the landing page and the documentation. The documentation lives only there (`website/content/docs/`); the README pitches Sideporch and links into it.

- **Versions**: Zola and Pagefind are pinned in `mise.toml`, with their download checksums and attestation provenance in `mise.lock`. Linux x64 uses Zola's static musl build.
- **Builds**: locally through mise (`mise run site`, `site-build`, `site-check`). Vercel and the Dagger `site` check run `website/ci-build.sh`, which downloads the two tools from the URLs in `mise.lock`, verifies their checksums, and builds. Preview deployments get their own base URL.
- **Generated reference**: the automation API page is rendered from `src/automations/api.rs::reference()`; a test fails when the committed page differs.
- **Screenshots**: one copy in `website/static/img/`, shared by the site, the docs and the README, retaken with `mise run screenshots`.

## Context

On 2026-09-29 Niklas asked to use the website for the documentation, "set up a nice system for that", make every screenshot zoomable and easy to leave, and bring information and screenshots up to date. The documentation was a 350-line README plus `docs/capacity.md`; the site was one hand-written HTML page with no build step.

He chose Zola with Pagefind over the alternatives:

- **Starlight (Astro)** has the most polished documentation experience out of the box, but brings npm and a JavaScript dependency tree to maintain, which Sideporch otherwise avoids.
- **mdBook** is Rust and minimal, but looks like a separate generic book next to the landing page.
- **Hand-written HTML** has no navigation, search or shared layout, and doesn't scale to dozens of pages.

He also chose to make the site the single source of the documentation and slim the README, rather than keep two copies in step.

Observed while building it on 2026-09-29:

- Zola 0.23 replaced shortcodes and macros with Tera 2 components, and reads `zola.toml`.
- Zola's glibc Linux build needs glibc 2.35; Vercel builds on Amazon Linux 2023 with glibc 2.34, so Linux x64 uses the musl build.
- mise verifies Zola's GitHub attestations through the GitHub API and refuses to skip them once they are recorded in `mise.lock`. Unauthenticated, that API is rate-limited per IP, which Vercel's builders share, so the hosted builds download the locked files themselves.

## Consequences

- The templates, styles and the one script (`static/site.js`: search, the docs drawer, the screenshot viewer) are ours to maintain; there is no theme to update.
- Changes people will notice update the matching docs page in the same commit; `AGENTS.md` says so.
- The site deploys from `main`, so it describes `main`. Anything newer than the latest release should say which version it arrives in.
- Upgrading Zola or Pagefind means changing `mise.toml` and relocking all four platforms with a GitHub token.
- The website builds in CI, so a broken internal link fails the build.
