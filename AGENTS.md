# Working on Sideporch

Sideporch is a self-hosted team chat shipped as a single Rust binary. See [README.md](README.md) for goals and usage.

## Layout

- `src/routes.rs`: HTTP handlers, with more in `src/routes/`: `message.rs` (edit, delete, pin, save, vote, buttons, Activity), `channels.rs` (directory, join, leave, mute, private members, outgoing webhooks), `account.rs` (passwords, reset links, deactivation), `later.rs` (reminders and scheduled messages), `backups.rs` (backups and Slack import). `src/store.rs`: SQL queries. `src/db.rs`: connection and migrations.
- `src/views.rs`: maud templates. `src/markup.rs`: Slack-style message formatting to safe HTML.
- `src/messages.rs`: the one pipeline every new message takes (store, index, live update, push, automations).
- `src/webhook.rs`: Slack-compatible webhook parsing. `src/realtime.rs`: WebSocket fan-out and presence.
- `src/files.rs`: uploads, downloads, custom emoji; `src/blobs.rs` keeps file contents on disk by SHA-256. `src/search.rs`: FTS5 search with filters, ranking and spelling suggestions (`src/search/query.rs` reads the search box). `src/push.rs`: Web Push.
- `src/emoji.rs`: every standard emoji from gemoji (`assets/vendor/emoji.tsv`). `src/gifs.rs`: GIF settings (local library by default, GIPHY searched by the server, KLIPY searched by the browser) and the GIPHY client. `src/system.rs`: resource sampling for the admin's system page. `src/routes/profile.rs`, `src/routes/admin.rs` and `src/routes/gifs.rs`: profiles, the system page, and the GIF library, search and settings.
- `src/automations.rs`: starts one worker per automation and routes events, webhooks, commands and live runs to them; also dry runs. In `src/automations/`: `worker.rs` (a script's thread), `sandbox.rs` (the Lua state and the `sideporch` API), `events.rs`, `cron.rs`, `http.rs` (outgoing requests with the internal-address guard), `tooling.rs` (lint with selene, format with StyLua), and `api.rs`, the API described once for the linter, editor completions, reference, AI prompt and MCP.
- `src/later.rs`: time phrases in each person's time zone, `/remind`, and the task that delivers reminders and scheduled messages. `src/previews.rs`: link previews through the guarded HTTP client. `src/outgoing.rs`: outgoing webhooks. `src/backup.rs`: backup archives, schedules and `sideporch restore`. `src/import.rs`: Slack export import.
- `src/community.rs`: trust levels, roles, permissions, sign-up modes, reports and time-outs; `src/routes/community.rs` and `src/views/community.rs` are their pages. Check a new capability with `CurrentUser::require(Permission::…)` and add it to `Permission`.
- `src/polls.rs`: poll kinds and the instant-runoff count.
- Sign-in: `src/passkeys.rs` (WebAuthn verification with ring, from the browser's SPKI public key), `src/totp.rs` (authenticator codes and recovery codes), `src/mail.rs` (SMTP through lettre), `src/security.rs` (policy and stored factors); `src/routes/security.rs` and `src/views/security.rs` hold the flows. Every way of signing in ends in `routes::security::after_first_step` or `finish`, so a second step can't be skipped. `tests/security.rs` drives passkeys with a software authenticator and email with a fake SMTP server.
- `src/secrets.rs`: encrypted secrets and their key. `src/markdown.rs`: GitHub-flavored Markdown for messages (`src/markup.rs` stays for Slack-format webhooks).
- `src/ai.rs`: AI providers that write scripts. `src/mcp.rs`: the MCP server. `src/routes/automation.rs` and `src/routes/settings.rs`: their pages and endpoints.
- `assets/`: the page script, the automation editor (`editor.js`), vendored Mermaid (`vendor/`, see its README), service worker, base CSS, logo, and fonts. `build.rs` compiles utility classes with encre-css using `encre-css.toml`.
- `docs/screenshots/`: the README's screenshots. `scripts/screenshots.mjs` seeds a fresh server with a demo team and retakes them (see its header); update them when the interface changes visibly.
- `tools/loadtest/`: a separate crate that simulates people online (live connections, posts, page loads) against a size-limited container; `run.sh` steps through sizes and numbers of people, `report.sh` turns `results/` into the tables in `docs/capacity.md`. Rerun it after changes to the message pipeline, realtime fan-out or rendering, and update the document.
- `tests/`: end-to-end tests against a real server on a random port. `tests/fixtures/gatus/` holds captured Gatus payloads.

## Commands

Use mise: `mise run check` runs formatting, Clippy with the strict lints in `Cargo.toml`, and all tests. `mise run dev` starts a local server. `mise run ci` runs the same checks in containers through Dagger (`.dagger/main.dang`), which also builds the static binaries and the `FROM scratch` image. Keep Rust, zig and cargo-zigbuild versions aligned across `mise.toml`, `rust-toolchain.toml` and the Dagger module.

Build output grows quickly (a debug build with tests is several GB); `CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0` keeps it near 1.5 GB. Clean up when you are done: `mise run clean-debug` removes debug builds and test binaries, `mise run clean` removes all of `target/` and `dist/`, and `mise run clean-ci` empties only this project's cache in the local Dagger engine.

Releases: bump `version` in `Cargo.toml`, then push a matching `vX.Y.Z` tag. The release notes come from the commit subjects since the previous tag (`cliff.toml`; preview with `mise run release-notes -- --unreleased`): `feat`, `perf`, `fix` and `docs` commits are listed for people who run Sideporch, so write their subjects for them, and describe anything they must do before upgrading in a `BREAKING CHANGE:` footer. Linux release binaries must stay fully static; `scripts/package.sh` refuses dynamic ones.

## Conventions

- Use conventional commit messages (`feat`, `fix`, `docs`, `refactor`, `chore`).
- Keep compatibility claims for external tools (such as Gatus or Slack webhooks) backed by tests against the payloads those tools actually send. Regenerate fixtures as described in `tests/fixtures/gatus/README.md`.
- Never edit a released migration in `src/db.rs`; append a new one (`Migration::Sql`, or `Migration::Code` for steps that need Rust, such as rebuilding the search index). People skip versions, so every step must work on data any earlier release wrote.
- Write utility classes as literal strings in `class="…"` so encre-css finds them, and check the generated CSS for utilities you haven't used before. encre-css differs from Tailwind in places (see the frontend decision record).
- Import new icons through `src/icons.rs` only.
- A new `sideporch.*` function goes into `src/automations/api.rs` as well as the sandbox, so the linter, completions, reference, AI prompt and MCP know it. Write transactions use `BEGIN IMMEDIATE`, because each automation worker writes on its own connection.

## Decisions

Record lasting choices (architecture, storage, compatibility, tooling) in [decisions/](decisions/) with [vrdx](https://github.com/niklas-heer/vrdx). Install it with `brew install niklas-heer/tap/vrdx` and run `vrdx guide` for the conventions. Check existing records with `vrdx context "<question>"` before changing an established direction, and run `vrdx validate` before committing.
