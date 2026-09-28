+++
schema_version = 1
id = "01M3KXCGGA18NB73RYRCP1RJFE"
title = "Lint, format and test-run automation scripts on the server"
date = "2026-09-28"
status = "accepted"
tags = ["automations", "tooling"]
supersedes = []
superseded_by = []
depends_on = []
related_to = ["01M3J6PYXDJY1DJVFKX0DN39AP"]
+++
## Decision

The automation editor lints, formats and test-runs scripts on the server, and edits them with a small hand-written editor in the browser.

- **Linting** uses [selene](https://github.com/Kampfkarren/selene) (`selene-lib` 0.31) with a standard library generated from `src/automations/api.rs`: Lua 5.4's sandboxed libraries plus the `sideporch` table. Using a removed global such as `os` or `io` explains that the sandbox lacks it. Syntax errors come from full_moon, the parser selene and StyLua share.
- **Formatting** uses [StyLua](https://github.com/JohnnyMorganz/StyLua) as a library, with two-space indentation and output verification.
- **Test runs** load the editor's current script in a fresh sandbox, fire a simulated message, reaction, webhook request or timer, and report printed output, the posts and reactions it would make, errors with line numbers, and cost. Actions are only described, and `sideporch.set` writes to a copy of the saved data.
- **The editor** (`assets/editor.js`, no dependencies) layers a highlighted copy under the textarea and adds line numbers, lint underlines, completions for the `sideporch` API, auto-indentation, and keyboard shortcuts. Without JavaScript the plain textarea and form still work.
- `src/automations/api.rs` describes the API once; the linter's standard library, completions, the in-page reference, the AI prompt and the MCP reference are generated from it.

## Context

On 2026-09-28 Niklas asked for "a cool editor" in which people can work with and run scripts, plus "a linter, formatter, and so on", looking at the tool from a developer's and a user's perspective.

- **CodeMirror or Monaco** would give a richer editor but need a JavaScript build step, which the frontend decision rules out, and add hundreds of kilobytes. The hand-written layer covers what short scripts need.
- **Linting in the browser** (a Lua parser compiled to WebAssembly) would avoid a request per pause but add a wasm build. Server-side linting reuses mature Rust crates and keeps one implementation for the editor, the AI loop and MCP.
- **Only `load()` for syntax checks** would catch syntax errors but not undefined globals, misspelled API calls or unused variables, which are the common mistakes in short scripts.

## Consequences

- selene and StyLua add compile time and a few megabytes to the binary. StyLua's crate declares a `cdylib` target and pulls in clap 3 and env_logger as unconditional dependencies.
- selene's `multiple_statements` lint is off, because layout is the formatter's job.
- The instruction count in test runs is measured by the budget hook, so it moves in steps of 1,000.
