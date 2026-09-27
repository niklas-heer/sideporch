# Working on Sideporch

Sideporch is a self-hosted team chat shipped as a single Rust binary. See [README.md](README.md) for goals and open questions. There is no code yet.

## Decisions

Record lasting choices (architecture, storage, compatibility, tooling) in [decisions/](decisions/) with [vrdx](https://github.com/niklas-heer/vrdx). Install it with `brew install niklas-heer/tap/vrdx` and run `vrdx guide` for the conventions. Check existing records with `vrdx context "<question>"` before changing an established direction, and run `vrdx validate` before committing.

## Conventions

- Use conventional commit messages (`feat`, `fix`, `docs`, `refactor`, `chore`).
- Keep compatibility claims for external tools (such as Gatus or Slack webhooks) backed by tests against the payloads those tools actually send.
