+++
title = "AI and MCP"
description = "Let an AI model write automations in the editor, or connect your own agent through MCP."
weight = 7
aliases = ["/docs/integrations/ai-and-mcp/"]
+++

Sideporch can have an AI model write [automations](@/docs/integrations/automations/_index.md) for you, in two ways.

## In the editor

An admin connects a model under **Automations → AI provider**: Anthropic's API, or any OpenAI-compatible one (OpenAI, OpenRouter, Ollama, …). The key is stored encrypted in the database; local models usually need none.

Then **Ask AI** in the editor writes or changes the script from a description, using your libraries. Sideporch lints and test-loads the answer, sends problems back to the model, and shows you the result to review. Nothing is saved until you choose to.

## From your own agent

Sideporch is an [MCP](https://modelcontextprotocol.io) server at `/mcp`. Create a token under **Automations → MCP**, then connect your agent. For example, with Claude Code:

```sh
claude mcp add --transport http sideporch https://chat.example.com/mcp \
  --header "Authorization: Bearer sp_…"
```

Agents get what a developer needs: the API reference, the public channels and their newest messages (to see what an automation will react to), lint, format, test runs and live runs, run logs, versions and restore, saved data, write-only secrets, settings, and schedule previews. Scripts are also available as MCP resources.

Agents can [share automations](@/docs/integrations/automations/sharing.md) too: `export_automations` returns the same file **Export** downloads, `preview_import` checks a file against the server without changing anything, and `import_automations` imports it, switched off.

Like automations themselves, agents see public channels only. Automations an agent creates start switched off, and every change is kept in the history under the token's name, so you can see what it did and undo it.
