+++
schema_version = 1
id = "01M3HRKXN442S40GZ9AGYGQHH1"
title = "Render the web interface on the server with Rust-native Tailwind classes"
date = "2026-09-27"
status = "accepted"
tags = ["frontend", "architecture", "tooling"]
supersedes = []
superseded_by = []
depends_on = ["01M3G6RCQKT16B893YJKV7YY7K"]
related_to = []
+++
## Decision

Sideporch's web interface is rendered on the server, with one small hand-written script for live updates. The pieces are:

- **HTML**: [maud](https://maud.lambda.xyz) templates in `src/views.rs`, which are checked at compile time and escape by default.
- **Styling**: Tailwind-style utility classes generated at build time by [encre-css](https://gitlab.com/encre-org/encre-css), a Tailwind-compatible generator written in Rust. `build.rs` scans `src/` and `assets/*.js`, reads the theme and shortcuts from `encre-css.toml`, and embeds the result. There is no Node toolchain.
- **Script**: `assets/app.js` (about 200 lines, no framework). It handles the WebSocket, sending without a reload, drafts, local times, and copy buttons. Every page also works without it: forms post and redirect.
- **Live updates**: the server renders each new message to HTML once and sends that markup over the WebSocket, so there is a single rendering path.

## Context

The open question was a Rust/WebAssembly framework such as Leptos versus server-rendered HTML. On 2026-09-27 Niklas chose server-rendered pages with a little JavaScript, and asked for Tailwind "but Rust-native".

- **Leptos** shares Rust types between server and client, but its WebAssembly bundle is a heavy first load for a chat app that should open instantly on phones. It also adds a wasm build step.
- **The Tailwind CLI** needs Node or its standalone binary as a build dependency.
- Among Rust options (checked on crates.io, 2026-09-27), encre-css was the only maintained one: version 0.21.1 was released on 2026-09-09. `railwind` and `tailwind-css` had had no release since 2023.

## Consequences

- Class names must appear as literal strings in `src/` or `assets/*.js`; classes built by string concatenation at runtime are not generated. Use maud's `class="…"` attribute syntax, not the `.class` shorthand, which the scanner does not see.
- encre-css follows Tailwind closely but not exactly. In 0.21.1 `outline-none` is `outline-hidden`, there is no `size-*`, and `divide-y` puts its border on the wrong side (so the views use explicit borders). Check the generated CSS when using an unfamiliar utility.
- Changing styles or the script needs a rebuild, as the single-binary decision already says for assets.
- If the interface ever needs rich client-side state (offline sync, complex editors), revisit this record rather than growing app.js into a framework.
