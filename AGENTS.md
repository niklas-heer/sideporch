# Working on Sideporch

Sideporch is a self-hosted team chat shipped as a single Rust binary. See [README.md](README.md) for goals and usage.

## Layout

- `src/routes.rs`: HTTP handlers. `src/store.rs`: SQL queries. `src/db.rs`: connection and migrations.
- `src/views.rs`: maud templates. `src/markup.rs`: Slack-style message formatting to safe HTML.
- `src/messages.rs`: the one pipeline every new message takes (store, index, live update, push, automations).
- `src/webhook.rs`: Slack-compatible webhook parsing. `src/realtime.rs`: WebSocket fan-out and presence.
- `src/files.rs`: uploads, downloads, custom emoji. `src/search.rs`: FTS5 search. `src/push.rs`: Web Push.
- `src/automations.rs`: the automation thread. `src/automations/sandbox.rs`: the Lua sandbox and `sideporch` API. `src/automations/api.rs`: the API described once, feeding the linter, editor completions, the in-page reference, the AI prompt and MCP. `src/automations/tooling.rs`: linting (selene) and formatting (StyLua).
- `src/ai.rs`: AI providers that write scripts. `src/mcp.rs`: the MCP server. `src/routes/automation.rs` and `src/routes/settings.rs`: their pages and endpoints.
- `assets/`: the page script, the automation editor (`editor.js`), service worker, base CSS, logo, and fonts. `build.rs` compiles utility classes with encre-css using `encre-css.toml`.
- `tests/`: end-to-end tests against a real server on a random port. `tests/fixtures/gatus/` holds captured Gatus payloads.

## Commands

Use mise: `mise run check` runs formatting, Clippy with the strict lints in `Cargo.toml`, and all tests. `mise run dev` starts a local server. `mise run ci` runs the same checks in containers through Dagger (`.dagger/main.dang`), which also builds the static binaries and the `FROM scratch` image. Keep Rust, zig and cargo-zigbuild versions aligned across `mise.toml`, `rust-toolchain.toml` and the Dagger module.

Build output grows quickly (a debug build with tests is about 2 GB). Clean up when you are done: `mise run clean-debug` removes debug builds and test binaries, `mise run clean` removes all of `target/` and `dist/`, and `mise run clean-ci` empties only this project's cache in the local Dagger engine.

Releases: bump `version` in `Cargo.toml`, then push a matching `vX.Y.Z` tag. Linux release binaries must stay fully static; `scripts/package.sh` refuses dynamic ones.

## Conventions

- Use conventional commit messages (`feat`, `fix`, `docs`, `refactor`, `chore`).
- Keep compatibility claims for external tools (such as Gatus or Slack webhooks) backed by tests against the payloads those tools actually send. Regenerate fixtures as described in `tests/fixtures/gatus/README.md`.
- Never edit a released migration in `src/db.rs`; append a new one.
- Write utility classes as literal strings in `class="…"` so encre-css finds them, and check the generated CSS for utilities you haven't used before. encre-css differs from Tailwind in places (see the frontend decision record).
- Import new icons through `src/icons.rs` only.
- A new `sideporch.*` function goes into `src/automations/api.rs` as well as the sandbox, so the linter, completions, reference, AI prompt and MCP know it. Automation writes use `BEGIN IMMEDIATE` transactions, because the automation thread writes on its own connection.

## Decisions

Record lasting choices (architecture, storage, compatibility, tooling) in [decisions/](decisions/) with [vrdx](https://github.com/niklas-heer/vrdx). Install it with `brew install niklas-heer/tap/vrdx` and run `vrdx guide` for the conventions. Check existing records with `vrdx context "<question>"` before changing an established direction, and run `vrdx validate` before committing.
