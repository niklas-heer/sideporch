//! Admin settings: the AI provider and MCP API tokens.

use maud::{Markup, html};

use super::{Shell, copy_row, form_error, panel_page, section, timestamp};
use crate::{
    ai::{Protocol, Provider},
    icons::{self, icon},
    store::ApiToken,
};

fn tabs(current: &str) -> Markup {
    html! {
        nav class="mb-6 flex gap-2" aria-label="Settings" {
            @for (href, label) in [("/settings/ai", "AI provider"), ("/settings/mcp", "MCP"), ("/automations", "Automations")] {
                a href=(href) aria-current=[(href == current).then_some("page")]
                    class="rounded-lg px-3 py-1.5 text-sm font-semibold hover:bg-screen aria-[current=page]:bg-haint-2 aria-[current=page]:text-floor dark:hover:bg-night-2 dark:aria-[current=page]:bg-floor-2 dark:aria-[current=page]:text-haint-2" {
                    (label)
                }
            }
        }
    }
}

pub fn ai_page(
    shell: &Shell<'_>,
    provider: Option<&Provider>,
    error: Option<&str>,
    saved: bool,
) -> Markup {
    let protocol = provider.map_or(Protocol::Anthropic, |provider| provider.protocol);
    panel_page(
        "AI provider",
        shell,
        &html! { "AI provider" },
        &html! {
            (tabs("/settings/ai"))
            p class="mb-5 text-muted dark:text-haint" {
                "Connect an AI model so admins can ask it to write and change automation scripts in the editor. "
                "Sideporch sends the request, the automation API reference, the current script and the names of public channels; it never sends messages. "
                "Answers are linted and test-loaded, and nothing is saved until you choose to."
            }
            (form_error(error))
            @if saved {
                p role="status" class="mb-4 rounded-lg border border-line bg-haint-2 px-3 py-2 text-sm text-floor dark:border-night-line dark:bg-floor-2 dark:text-haint-2" {
                    "Saved. Open an automation and choose Ask AI to try it."
                }
            }
            form method="post" action="/settings/ai" class="max-w-lg space-y-4" {
                div {
                    label for="ai-protocol" class="field-label" { "API" }
                    select id="ai-protocol" name="protocol" class="field" {
                        option value="anthropic" selected[protocol == Protocol::Anthropic] { "Anthropic (Claude)" }
                        option value="openai" selected[protocol == Protocol::OpenAi] { "OpenAI-compatible (OpenAI, OpenRouter, Ollama, …)" }
                    }
                }
                div {
                    label for="ai-model" class="field-label" { "Model" }
                    input id="ai-model" name="model" required class="field font-mono text-sm"
                        value=(provider.map_or("claude-opus-5", |provider| provider.model.as_str()));
                    p class="mt-1 text-sm text-muted dark:text-haint" {
                        "Such as " code { "claude-opus-5" } " for Anthropic, or the model name your OpenAI-compatible service uses, such as " code { "qwen3-coder" } " on Ollama."
                    }
                }
                div {
                    label for="ai-base-url" class="field-label" { "API URL (optional)" }
                    input id="ai-base-url" name="base_url" type="url" class="field font-mono text-sm"
                        value=[provider.map(|provider| provider.base_url.as_str())]
                        placeholder="https://api.anthropic.com";
                    p class="mt-1 text-sm text-muted dark:text-haint" {
                        "Leave empty for " code { "https://api.anthropic.com" } " or " code { "https://api.openai.com/v1" } ". For Ollama use " code { "http://localhost:11434/v1" } ", for OpenRouter " code { "https://openrouter.ai/api/v1" } "."
                    }
                }
                div {
                    label for="ai-key" class="field-label" { "API key" }
                    input id="ai-key" name="api_key" type="password" autocomplete="off" class="field font-mono text-sm"
                        placeholder=(provider.and_then(Provider::key_hint).map_or_else(String::new, |hint| format!("Saved key ending in …{hint}; leave empty to keep it")));
                    p class="mt-1 text-sm text-muted dark:text-haint" {
                        "Stored in Sideporch's database, so keep backups of it private. Local models usually need none."
                    }
                }
                button type="submit" class="btn" { "Save provider" }
            }
            @if provider.is_some() {
                form method="post" action="/settings/ai/remove" class="mt-8" {
                    button type="submit" class="btn-quiet text-sm" { (icon(icons::TRASH, "h-4 w-4")) "Disconnect the provider" }
                }
            }
        },
    )
}

pub fn mcp_page(
    shell: &Shell<'_>,
    endpoint: &str,
    tokens: &[ApiToken],
    new_token: Option<&str>,
) -> Markup {
    let token_for_examples = new_token.unwrap_or("YOUR_TOKEN");
    panel_page(
        "MCP",
        shell,
        &html! { "MCP for AI agents" },
        &html! {
            (tabs("/settings/mcp"))
            p class="mb-5 text-muted dark:text-haint" {
                "AI agents such as Claude Code, Claude Desktop, Cursor or Codex can connect to Sideporch over the Model Context Protocol. "
                "They can then read the automation API, lint, format and test scripts, and create or change automations for you. "
                "New automations they write start switched off, and every change lands in the automation's history."
            }
            @if let Some(token) = new_token {
                div role="status" class="mb-6 rounded-xl border border-floor-3 p-4" {
                    p class="mb-2 font-semibold" { "Your new token. Copy it now; Sideporch only shows it once." }
                    (copy_row(token))
                }
            }
            (section("Connect an agent", "The endpoint uses Streamable HTTP. Send a token as a bearer token.", &html! {
                (copy_row(endpoint))
                p class="mb-1 mt-4 text-sm font-semibold" { "Claude Code" }
                pre class="overflow-x-auto rounded-lg bg-screen p-3 text-xs dark:bg-night-2" {
                    code { "claude mcp add --transport http sideporch " (endpoint) " \\\n  --header \"Authorization: Bearer " (token_for_examples) "\"" }
                }
                p class="mb-1 mt-4 text-sm font-semibold" { "Other clients (JSON configuration)" }
                pre class="overflow-x-auto rounded-lg bg-screen p-3 text-xs dark:bg-night-2" {
                    code { "{\n  \"mcpServers\": {\n    \"sideporch\": {\n      \"type\": \"http\",\n      \"url\": \"" (endpoint) "\",\n      \"headers\": { \"Authorization\": \"Bearer " (token_for_examples) "\" }\n    }\n  }\n}" }
                }
            }))
            (section("API tokens", "Each token acts as the admin who created it and stops working if that person is no longer an admin.", &html! {
                form method="post" action="/settings/mcp/tokens" class="mb-5 flex max-w-lg gap-2" {
                    label for="token-name" class="sr-only" { "Token name" }
                    input id="token-name" name="name" maxlength="60" class="field" placeholder="Claude Code on my laptop";
                    button type="submit" class="btn shrink-0" { (icon(icons::KEY, "h-4 w-4")) "Create token" }
                }
                @if tokens.is_empty() {
                    p class="text-sm text-muted dark:text-haint" { "No tokens yet." }
                } @else {
                    ul class="space-y-2" {
                        @for token in tokens {
                            li class="flex items-center gap-3 rounded-lg border border-line px-3 py-2 dark:border-night-line" {
                                (icon(icons::KEY, "h-5 w-5 shrink-0 text-floor-3 dark:text-haint"))
                                span class="min-w-0 flex-1" {
                                    span class="block truncate font-semibold" { (token.name) }
                                    span class="block text-xs text-muted dark:text-haint" {
                                        (token.owner) " · created " (timestamp(token.created_at))
                                        @if let Some(used) = token.last_used_at { " · last used " (timestamp(used)) } @else { " · never used" }
                                    }
                                }
                                form method="post" action={ "/settings/mcp/tokens/" (token.id) "/delete" } {
                                    button type="submit" class="btn-quiet text-sm" { "Revoke" }
                                }
                            }
                        }
                    }
                }
            }))
        },
    )
}
