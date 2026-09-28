+++
schema_version = 1
id = "01M3KXCGGJDXNR7HFCZVTA778X"
title = "Let AI write automations through a connected provider and an MCP server"
date = "2026-09-28"
status = "accepted"
tags = ["automations", "ai", "security"]
supersedes = []
superseded_by = []
depends_on = []
related_to = ["01M3J6PYXDJY1DJVFKX0DN39AP"]
+++
## Decision

AI can write automations in two ways, both limited to admins.

- **A connected provider in the editor.** An admin configures either Anthropic's Messages API or any OpenAI-compatible chat completions API (OpenAI, OpenRouter, Ollama and others), with a model and an optional key, under Settings. *Ask AI* sends the request, the generated API reference, the current script and the names of public channels, never messages. Sideporch lints and test-loads the answer and sends problems back up to twice, then shows a draft to review; nothing is saved automatically. Anthropic requests follow Anthropic's documented raw-HTTP shape (`x-api-key`, `anthropic-version`, `max_tokens` 16000, text blocks only, `stop_reason: "refusal"` handled) and opt into `fallbacks: "default"` for models that support it. The key is stored in the database and never shown again.
- **An MCP server** at `/mcp` (Streamable HTTP, stateless JSON responses, protocol versions 2025-03-26 to 2025-11-25). Admins create named API tokens; tokens are stored hashed, act as their creator, and stop working when that person is no longer an admin. Tools cover the API reference, listing and reading automations and channels, linting, formatting, test runs, run logs, saving and deleting. Saving refuses scripts with lint errors, new automations start switched off unless the agent asks otherwise, and every save is recorded in the history as `MCP: <token name>`.

## Context

On 2026-09-28 Niklas asked that "the AI, with its connection" be able to write scripts for someone. Asked whether that meant a provider inside Sideporch or an endpoint for external agents, he chose both.

- **Only OpenAI-compatible APIs** would cover most providers with one code path, but Anthropic's guidance is to call Claude through its own API rather than a compatibility shim, so Anthropic gets a native path.
- **Streaming or tool use** for the provider path would show progress sooner, but a single request with a lint-and-retry loop keeps the server code small and works with local models that lack tool support.
- **Per-user OAuth for MCP** is what the MCP specification recommends for remote servers, but bearer tokens are far simpler for a self-hosted single binary and work with every current client.

## Consequences

- Sideporch makes outbound HTTPS (and HTTP, for local models) requests to an admin-chosen URL. Only admins can set it.
- Database backups contain the AI key; the settings page says so.
- An MCP token grants admin-level control over automations, so it is shown once and can be revoked.
