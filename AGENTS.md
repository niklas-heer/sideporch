# Working on Sideporch

Sideporch is a self-hosted team chat shipped as a single Rust binary. See [README.md](README.md) for goals and usage.

## Layout

- `src/routes.rs`: HTTP handlers. `src/store.rs`: SQL queries. `src/db.rs`: connection and migrations.
- `src/views.rs`: maud templates. `src/markup.rs`: Slack-style message formatting to safe HTML.
- `src/webhook.rs`: Slack-compatible webhook parsing. `src/realtime.rs`: WebSocket fan-out.
- `assets/`: the one script, base CSS, logo, and fonts. `build.rs` compiles utility classes with encre-css using `encre-css.toml`.
- `tests/`: end-to-end tests against a real server on a random port. `tests/fixtures/gatus/` holds captured Gatus payloads.

## Commands

Use mise: `mise run check` runs formatting, Clippy with the strict lints in `Cargo.toml`, and all tests. `mise run dev` starts a local server.

## Conventions

- Use conventional commit messages (`feat`, `fix`, `docs`, `refactor`, `chore`).
- Keep compatibility claims for external tools (such as Gatus or Slack webhooks) backed by tests against the payloads those tools actually send. Regenerate fixtures as described in `tests/fixtures/gatus/README.md`.
- Never edit a released migration in `src/db.rs`; append a new one.
- Write utility classes as literal strings in `class="…"` so encre-css finds them, and check the generated CSS for utilities you haven't used before. encre-css differs from Tailwind in places (see the frontend decision record).
- Import new icons through `src/icons.rs` only.

## Decisions

Record lasting choices (architecture, storage, compatibility, tooling) in [decisions/](decisions/) with [vrdx](https://github.com/niklas-heer/vrdx). Install it with `brew install niklas-heer/tap/vrdx` and run `vrdx guide` for the conventions. Check existing records with `vrdx context "<question>"` before changing an established direction, and run `vrdx validate` before committing.
