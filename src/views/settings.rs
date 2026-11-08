//! Admin settings: the AI provider and MCP API tokens.

use maud::{Markup, html};

use super::{Shell, copy_row, form_error, panel_page, section, timestamp};
use crate::{
    ai::{Protocol, Provider},
    automations::Settings,
    bots::{self, BotRow, TokenRow},
    icons::{self, icon},
    secrets::SecretInfo,
    store::ApiToken,
};

/// Tabs across automations and their settings.
pub fn tabs(current: &str) -> Markup {
    html! {
        nav class="mb-6 flex gap-2" aria-label="Settings" {
            @for (href, label) in [("/automations", "Automations"), ("/settings/automations", "Settings"), ("/settings/secrets", "Secrets"), ("/settings/ai", "AI provider"), ("/settings/mcp", "MCP"), ("/settings/bots", "Bots")] {
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

pub fn secrets_page(
    shell: &Shell<'_>,
    secrets: &[(SecretInfo, Vec<String>)],
    error: Option<&str>,
    saved: Option<&str>,
) -> Markup {
    panel_page(
        "Secrets",
        shell,
        &html! { "Secrets" },
        &html! {
            (tabs("/settings/secrets"))
            p class="mb-5 text-muted dark:text-haint" {
                "API tokens and passwords for automations. Scripts read them with "
                code { "sideporch.secret(\"NAME\")" } "; nobody can read them back here, and they are hidden in run logs. "
                "They are stored encrypted with a key kept outside the database, in "
                code { "secret.key" } " in the data directory or in " code { "SIDEPORCH_SECRET_KEY" } ", so back that key up with the database."
            }
            p class="mb-5 text-sm text-muted dark:text-haint" {
                "Secrets can also come from the environment: " code { "SIDEPORCH_SECRET_GITHUB_TOKEN" }
                " becomes " code { "GITHUB_TOKEN" } " and takes precedence over a stored one."
            }
            (form_error(error))
            @if let Some(name) = saved {
                p role="status" class="mb-4 rounded-lg border border-line bg-haint-2 px-3 py-2 text-sm text-floor dark:border-night-line dark:bg-floor-2 dark:text-haint-2" {
                    "Saved " code { (name) } ". Automations were restarted to use it."
                }
            }
            form method="post" action="/settings/secrets" class="mb-8 max-w-lg space-y-3" {
                div {
                    label for="secret-name" class="field-label" { "Name" }
                    input id="secret-name" name="name" required maxlength="64" pattern="[A-Za-z][A-Za-z0-9_]*"
                        class="field font-mono text-sm" placeholder="GITHUB_TOKEN" autocomplete="off";
                }
                div {
                    label for="secret-value" class="field-label" { "Value" }
                    input id="secret-value" name="value" type="password" required autocomplete="off" class="field font-mono text-sm";
                    p class="mt-1 text-sm text-muted dark:text-haint" { "Saving an existing name replaces its value." }
                }
                button type="submit" class="btn" { (icon(icons::KEY, "h-4 w-4")) "Save secret" }
            }
            @if secrets.is_empty() {
                p class="text-sm text-muted dark:text-haint" { "No secrets yet." }
            } @else {
                ul class="max-w-2xl space-y-2" {
                    @for (secret, users) in secrets {
                        li class="flex items-center gap-3 rounded-lg border border-line px-3 py-2 dark:border-night-line" {
                            (icon(icons::KEY, "h-5 w-5 shrink-0 text-floor-3 dark:text-haint"))
                            span class="min-w-0 flex-1" {
                                span class="block truncate font-mono font-semibold" { (secret.name) }
                                span class="block text-xs text-muted dark:text-haint" {
                                    @if secret.from_environment { "From the environment" @if secret.stored { " (overrides the stored value)" } }
                                    @else if let Some(at) = secret.updated_at {
                                        "Stored " (timestamp(at)) @if let Some(by) = &secret.updated_by { " by " (by) }
                                    }
                                    @if !users.is_empty() { " · used by " (users.join(", ")) }
                                }
                            }
                            @if secret.stored {
                                form method="post" action={ "/settings/secrets/" (secret.name) "/delete" } {
                                    button type="submit" class="btn-quiet text-sm" { "Delete" }
                                }
                            }
                        }
                    }
                }
            }
        },
    )
}

pub fn automation_settings_page(
    shell: &Shell<'_>,
    settings: &Settings,
    error: Option<&str>,
    saved: bool,
) -> Markup {
    panel_page(
        "Automation settings",
        shell,
        &html! { "Automation settings" },
        &html! {
            (tabs("/settings/automations"))
            (form_error(error))
            @if saved {
                p role="status" class="mb-4 rounded-lg border border-line bg-haint-2 px-3 py-2 text-sm text-floor dark:border-night-line dark:bg-floor-2 dark:text-haint-2" {
                    "Saved. Automations were restarted with the new settings."
                }
            }
            form method="post" action="/settings/automations" class="max-w-lg space-y-5" {
                div {
                    label for="timezone" class="field-label" { "Time zone for schedules" }
                    input id="timezone" name="timezone" required class="field font-mono text-sm" value=(settings.timezone)
                        placeholder="Europe/Berlin";
                    p class="mt-1 text-sm text-muted dark:text-haint" {
                        code { "sideporch.cron" } " schedules run in this time zone unless they name another, such as "
                        code { "{ timezone = \"America/New_York\" }" } ". Use an IANA name."
                    }
                }
                label class="flex items-start gap-2" {
                    input type="checkbox" name="allow_private_network" value="on" checked[settings.allow_private_network] class="mt-1 h-4 w-4 accent-floor";
                    span {
                        span class="block font-semibold" { "Let automations reach private networks" }
                        span class="block text-sm text-muted dark:text-haint" {
                            "By default " code { "sideporch.http" } " refuses private, loopback and link-local addresses, so scripts cannot probe the server's network. "
                            "Allow them to call services on your LAN or on this machine, such as Home Assistant."
                        }
                    }
                }
                button type="submit" class="btn" { "Save settings" }
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

/// A token just made, shown once.
pub struct Issued {
    pub bot: String,
    pub token: String,
}

pub fn bots_page(
    shell: &Shell<'_>,
    endpoint: &str,
    bots: &[BotRow],
    channels: &[(i64, String, String)],
    error: Option<&str>,
    issued: Option<&Issued>,
) -> Markup {
    panel_page(
        "Bots",
        shell,
        &html! { "Bots" },
        &html! {
            (tabs("/settings/bots"))
            p class="mb-5 text-muted dark:text-haint" {
                "Bots are named accounts for scripts and other programs. They post as themselves through the chat API, and read and post only in the public channels their tokens list. "
                "Private channels and direct messages stay out of reach, even when a token lists them. Revoking a bot or a token takes effect on its next request."
            }
            @if let Some(issued) = issued {
                div role="status" class="mb-6 rounded-xl border border-floor-3 p-4" {
                    p class="mb-2 font-semibold" { "Token for " (issued.bot) ". Copy it now; Sideporch only shows it once." }
                    (copy_row(&issued.token))
                }
            }
            (form_error(error))
            (section("Chat API", "Send requests to this address with a token as a bearer token. Each request needs one of the token's scopes.", &html! {
                (copy_row(endpoint))
                p class="mt-3 text-sm text-muted dark:text-haint" {
                    "channels:read lists channels, messages:read reads them, and messages:write posts them."
                }
            }))
            (section("New bot", "The handle is the bot's name in the API; the display name shows on its messages.", &html! {
                form method="post" action="/settings/bots" class="max-w-lg space-y-4" {
                    div {
                        label for="bot-handle" class="field-label" { "Handle" }
                        input id="bot-handle" name="name" required maxlength="32" class="field" placeholder="deploy-bot";
                    }
                    div {
                        label for="bot-display-name" class="field-label" { "Display name" }
                        input id="bot-display-name" name="display_name" required maxlength="60" class="field" placeholder="Deploys";
                    }
                    div {
                        label for="bot-icon" class="field-label" { "Icon (optional)" }
                        input id="bot-icon" name="icon" maxlength="500" class="field" placeholder=":rocket: or an image address";
                    }
                    button type="submit" class="btn" { (icon(icons::ROBOT, "h-4 w-4")) "Create bot" }
                }
            }))
            (section("Bots", "Each token acts as its bot, with the scopes and channels it was given.", &html! {
                @if bots.is_empty() {
                    p class="text-sm text-muted dark:text-haint" { "No bots yet." }
                } @else {
                    ul class="space-y-6" {
                        @for bot in bots {
                            (bot_card(bot, channels))
                        }
                    }
                }
            }))
        },
    )
}

/// One bot: its header, its tokens, and a form for a new token.
fn bot_card(bot: &BotRow, channels: &[(i64, String, String)]) -> Markup {
    html! {
        li class="rounded-xl border border-line p-4 dark:border-night-line" {
            div class="flex items-center gap-3" {
                (icon(icons::ROBOT, "h-5 w-5 shrink-0 text-floor-3 dark:text-haint"))
                span class="min-w-0 flex-1" {
                    span class="block truncate font-semibold" {
                        (bot.display_name) " "
                        span class="font-normal text-muted dark:text-haint" { "@" (bot.name) }
                    }
                    span class="block text-xs text-muted dark:text-haint" {
                        "created " (timestamp(bot.created_at))
                        @if bot.revoked_at.is_some() { " · revoked" }
                    }
                }
                @if bot.revoked_at.is_none() {
                    form method="post" action={ "/settings/bots/" (bot.id) "/revoke" } {
                        button type="submit" class="btn-quiet text-sm" { "Revoke bot" }
                    }
                }
            }
            @if !bot.tokens.is_empty() {
                ul class="mt-3 space-y-2" {
                    @for token in &bot.tokens {
                        (token_row(token))
                    }
                }
            }
            @if bot.revoked_at.is_none() {
                (new_token_form(bot.id, channels))
            }
        }
    }
}

/// One token: its name, scopes, channels and use, and a way to revoke it.
fn token_row(token: &TokenRow) -> Markup {
    let channels = token.channels.as_ref().map_or_else(
        || "all public channels".to_owned(),
        |names| {
            names
                .iter()
                .map(|name| format!("#{name}"))
                .collect::<Vec<_>>()
                .join(", ")
        },
    );
    html! {
        li class="flex items-center gap-3 rounded-lg bg-screen px-3 py-2 text-sm dark:bg-night-2" {
            span class="min-w-0 flex-1" {
                span class="block truncate font-semibold" { (token.label) }
                span class="block text-xs text-muted dark:text-haint" {
                    (token.scopes.join(" · ")) " · " (channels)
                }
                span class="block text-xs text-muted dark:text-haint" {
                    @if let Some(expires) = token.expires_at { "expires " (timestamp(expires)) } @else { "never expires" }
                    " · "
                    @if let Some(used) = token.last_used_at { "last used " (timestamp(used)) } @else { "never used" }
                }
            }
            @if token.revoked_at.is_some() {
                span class="text-xs text-muted dark:text-haint" { "revoked" }
            } @else {
                form method="post" action={ "/settings/bots/tokens/" (token.id) "/revoke" } {
                    button type="submit" class="btn-quiet text-sm" { "Revoke" }
                }
            }
        }
    }
}

/// The form for a new token: its name, scopes, channels and expiry.
fn new_token_form(bot_id: i64, channels: &[(i64, String, String)]) -> Markup {
    let name_id = format!("bot-{bot_id}-token-label");
    let expires_id = format!("bot-{bot_id}-expires");
    html! {
        form method="post" action={ "/settings/bots/" (bot_id) "/tokens" } class="mt-4 max-w-lg space-y-4" {
            div {
                label for=(name_id) class="field-label" { "New token name" }
                input id=(name_id) name="label" maxlength="60" class="field" placeholder="Deploy pipeline";
            }
            fieldset class="space-y-1" {
                legend class="field-label" { "Scopes" }
                @for scope in bots::SCOPES {
                    label class="flex items-center gap-2 text-sm" {
                        input type="checkbox" name="scope" value=(scope) checked;
                        span { (scope) }
                    }
                }
            }
            fieldset class="space-y-1" {
                legend class="field-label" { "Channels" }
                label class="flex items-center gap-2 text-sm" {
                    input type="radio" name="channel_scope" value="all" checked;
                    span { "All public channels" }
                }
                label class="flex items-center gap-2 text-sm" {
                    input type="radio" name="channel_scope" value="list";
                    span { "Only the public channels checked below" }
                }
                @for (id, name, _) in channels {
                    label class="ml-6 flex items-center gap-2 text-sm" {
                        input type="checkbox" name="channel" value=(id);
                        span { "#" (name) }
                    }
                }
                @if channels.is_empty() {
                    p class="ml-6 text-sm text-muted dark:text-haint" { "There are no public channels yet." }
                }
            }
            div {
                label for=(expires_id) class="field-label" { "Expires" }
                select id=(expires_id) name="expires" class="field" {
                    option value="never" selected { "Never" }
                    option value="30" { "In 30 days" }
                    option value="90" { "In 90 days" }
                    option value="365" { "In a year" }
                }
            }
            button type="submit" class="btn" { (icon(icons::KEY, "h-4 w-4")) "Create token" }
        }
    }
}
