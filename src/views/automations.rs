//! Automation management for admins.

use maud::{Markup, PreEscaped, html};

use super::{
    ASSET_VERSION, Shell, copy_row, form_error, panel_page, section, timestamp, wide_panel_page,
};
use std::collections::HashMap;

use crate::{
    automations::{
        KIND_LIBRARY, Triggers, api,
        bundle::{Action, Bundle, Plan},
    },
    icons::{self, icon},
    store::{Automation, AutomationRun, AutomationVersion},
};

pub fn list_page(
    shell: &Shell<'_>,
    automations: &[Automation],
    triggers: &HashMap<i64, Triggers>,
    imported: Option<usize>,
) -> Markup {
    let (libraries, scripts): (Vec<&Automation>, Vec<&Automation>) = automations
        .iter()
        .partition(|automation| automation.kind == KIND_LIBRARY);
    panel_page(
        "Automations",
        shell,
        &html! { "Automations" },
        &html! {
            (super::settings::tabs("/automations"))
            p class="mb-5 text-muted dark:text-haint" {
                "Lua scripts that react to messages, reactions, new members and channels, answer slash commands and webhooks, run on schedules, and call APIs. "
                "Libraries hold code that automations share with " code { "require" } "."
            }
            @if let Some(count) = imported {
                p role="status" class="mb-5 rounded-lg border border-line bg-haint-2 px-3 py-2 text-sm text-floor dark:border-night-line dark:bg-floor-2 dark:text-haint-2" {
                    @if count == 0 {
                        "Nothing was imported."
                    } @else {
                        "Imported " (count) @if count == 1 { " script" } @else { " scripts" } ", switched off. "
                        "Open each automation, read what it does, add the secrets it needs, and switch it on."
                    }
                }
            }
            div class="mb-6 flex flex-wrap items-center gap-2" {
                a href="/automations/new" class="btn" { (icon(icons::PLUS, "h-5 w-5")) "New automation" }
                a href="/automations/new?kind=library" class="btn-quiet" { (icon(icons::PLUS, "h-4 w-4")) "New library" }
                span class="ml-auto flex flex-wrap gap-2" {
                    a href="/automations/import" class="btn-quiet" { (icon(icons::UPLOAD_SIMPLE, "h-4 w-4")) "Import" }
                    @if !automations.is_empty() {
                        button type="submit" form="export-form" class="btn-quiet" title="Export the ticked automations, or all of them" {
                            (icon(icons::DOWNLOAD_SIMPLE, "h-4 w-4")) "Export"
                        }
                    }
                }
            }
            // The ticks below belong to this form, so Export sends them.
            form id="export-form" method="get" action="/automations/export" {}
            @if scripts.is_empty() {
                p class="text-muted dark:text-haint" { "No automations yet." }
            } @else {
                p class="mb-2 text-sm text-muted dark:text-haint" {
                    "Tick automations to export just those, with the libraries they need; otherwise Export takes everything. "
                    a href="https://sideporch.app/docs/integrations/automations/sharing/" class="underline" { "About sharing" }
                }
                ul class="space-y-2" {
                    @for automation in &scripts {
                        li class="flex items-center gap-2" {
                            input type="checkbox" name="id" value=(automation.id) form="export-form"
                                aria-label={ "Export " (automation.name) } class="h-4 w-4 shrink-0 accent-floor";
                            a href={ "/automations/" (automation.id) }
                                class="flex min-w-0 flex-1 items-start gap-3 rounded-xl border border-line px-4 py-3 hover:border-floor-3 dark:border-night-line" {
                                (icon(icons::LIGHTNING, "mt-0.5 h-5 w-5 shrink-0 text-floor-3 dark:text-haint"))
                                span class="min-w-0 flex-1" {
                                    span class="block truncate font-semibold" { (automation.name) }
                                    @if let Some(triggers) = triggers.get(&automation.id) {
                                        (trigger_summary(triggers))
                                    }
                                    span class="block text-xs text-muted dark:text-haint" { "Changed " (timestamp(automation.updated_at)) }
                                }
                                (status_badge(automation.enabled, automation.last_error.is_some()))
                            }
                        }
                    }
                }
            }
            h2 class="mb-3 mt-10 text-lg font-bold" { "Libraries" }
            @if libraries.is_empty() {
                p class="text-sm text-muted dark:text-haint" { "No libraries yet. Put code that several automations need, such as a client for an API, in a library." }
            } @else {
                ul class="space-y-2" {
                    @for library in &libraries {
                        li class="flex items-center gap-2" {
                            input type="checkbox" name="id" value=(library.id) form="export-form"
                                aria-label={ "Export " (library.name) } class="h-4 w-4 shrink-0 accent-floor";
                            a href={ "/automations/" (library.id) }
                                class="flex min-w-0 flex-1 items-center gap-3 rounded-xl border border-line px-4 py-3 hover:border-floor-3 dark:border-night-line" {
                                (icon(icons::BOOKS, "h-5 w-5 shrink-0 text-floor-3 dark:text-haint"))
                                span class="min-w-0 flex-1" {
                                    code class="block truncate font-mono font-semibold" { "require(\"" (library.name) "\")" }
                                    span class="block text-xs text-muted dark:text-haint" { "Changed " (timestamp(library.updated_at)) }
                                }
                            }
                        }
                    }
                }
            }
        },
    )
}

/// One line per kind of trigger: events, schedules, commands, webhook.
fn trigger_summary(triggers: &Triggers) -> Markup {
    html! {
        span class="mt-1 flex flex-wrap gap-1" {
            @for event in &triggers.events {
                span class="rounded bg-screen px-1.5 font-mono text-xs dark:bg-night-2" { "on " (event.description) }
            }
            @for schedule in &triggers.schedules {
                span class="rounded bg-screen px-1.5 font-mono text-xs dark:bg-night-2" {
                    (schedule.description)
                    @if let Some(next) = schedule.next_run { ", next " (datetime(next)) }
                }
            }
            @for command in &triggers.commands {
                span class="rounded bg-screen px-1.5 font-mono text-xs dark:bg-night-2" { "/" (command.name) }
            }
            @if triggers.webhook {
                span class="rounded bg-screen px-1.5 font-mono text-xs dark:bg-night-2" { "webhook" }
            }
        }
    }
}

fn status_badge(enabled: bool, failed: bool) -> Markup {
    html! {
        @if failed {
            span class="rounded bg-red-100 px-2 text-xs font-semibold text-red-800 dark:bg-red-950 dark:text-red-200" { "Error" }
        } @else if enabled {
            span class="rounded bg-haint-2 px-2 text-xs font-semibold text-floor dark:bg-floor-2 dark:text-haint-2" { "On" }
        } @else {
            span class="rounded bg-screen px-2 text-xs font-semibold text-muted dark:bg-night-2 dark:text-haint" { "Off" }
        }
    }
}

#[derive(Default)]
pub struct Editor<'a> {
    pub id: Option<i64>,
    pub name: &'a str,
    pub source: &'a str,
    pub enabled: bool,
    pub last_error: Option<&'a str>,
    pub form_error: Option<&'a str>,
    pub hook_url: Option<&'a str>,
    pub runs: &'a [AutomationRun],
    pub versions: &'a [AutomationVersion],
    /// Whether an AI provider is set up, so the editor can offer to write scripts.
    pub ai_ready: bool,
    /// A library holds shared code instead of running itself.
    pub library: bool,
    /// What the running script listens to.
    pub triggers: Option<&'a Triggers>,
}

pub fn editor_page(shell: &Shell<'_>, editor: &Editor<'_>) -> Markup {
    let action = editor.id.map_or_else(
        || "/automations".to_owned(),
        |id| format!("/automations/{id}"),
    );
    let completions = api::completions().to_string().replace("</", "<\\/");
    wide_panel_page(
        "Automation",
        shell,
        &html! {
            @if editor.id.is_some() { (editor.name) }
            @else if editor.library { "New library" }
            @else { "New automation" }
        },
        &html! {
            (form_error(editor.form_error))
            @if let Some(error) = editor.last_error {
                div role="alert" class="mb-4 rounded-lg border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-800 dark:border-red-900 dark:bg-red-950 dark:text-red-200" {
                    p class="font-semibold" { "The script stopped with an error" }
                    pre class="mt-1 whitespace-pre-wrap font-mono text-xs" { (error) }
                }
            }
            div class="flex flex-col gap-8 xl:flex-row" {
                div class="min-w-0 flex-1" {
                    (script_form(editor, &action))
                }
                aside class="w-full space-y-6 xl:w-96 xl:shrink-0" {
                    @if let Some(triggers) = editor.triggers {
                        section aria-labelledby="listens-heading" class="rounded-xl border border-line p-4 dark:border-night-line" {
                            h2 id="listens-heading" class="font-bold" { "Listens to" }
                            @if triggers.is_empty() {
                                p class="mt-1 text-sm text-muted dark:text-haint" { "Nothing yet: the script registers no events, schedules, commands or webhook." }
                            } @else {
                                (trigger_summary(triggers))
                            }
                        }
                    }
                    (test_panel(editor.library))
                    (ai_panel(editor.ai_ready, shell.user.is_admin))
                }
            }
            @if let (Some(id), Some(url), false) = (editor.id, editor.hook_url, editor.library) {
                (webhook_section(id, url))
            }
            @if editor.id.is_some() {
                div class="max-w-3xl" {
                    (runs_section(editor.runs))
                    (history_section(editor.id, editor.versions))
                }
            }
            div class="max-w-3xl" {
                (reference_section())
            }
            script type="application/json" id="lua-api" { (PreEscaped(completions)) }
            script src={ "/assets/editor.js?v=" (ASSET_VERSION) } defer {}
        },
    )
}

fn script_form(editor: &Editor<'_>, action: &str) -> Markup {
    html! {
        form id="automation-form" method="post" action=(action) data-automation-id=[editor.id]
            data-kind=(if editor.library { KIND_LIBRARY } else { "automation" }) {
            @if editor.library && editor.id.is_none() {
                input type="hidden" name="kind" value=(KIND_LIBRARY);
            }
            div class="mb-4" {
                label for="automation-name" class="field-label" { "Name" }
                @if editor.library {
                    input id="automation-name" name="name" value=(editor.name) required maxlength="40"
                        pattern="[a-z][a-z0-9_]*" class="field font-mono" placeholder="github_api";
                    p class="mt-1 text-sm text-muted dark:text-haint" {
                        "Automations load it with " code { "require(\"" (if editor.name.is_empty() { "name" } else { editor.name }) "\")" }
                        ". Use lowercase letters, digits and underscores."
                    }
                } @else {
                    input id="automation-name" name="name" value=(editor.name) required maxlength="80" class="field"
                        placeholder="Porch butler";
                    p class="mt-1 text-sm text-muted dark:text-haint" { "Messages and reactions from the automation appear under this name." }
                }
            }
            div class="mb-2 flex flex-wrap items-center gap-2" {
                label for="automation-source" class="field-label mb-0 mr-auto" { "Lua script" }
                div data-editor-tools hidden class="flex flex-wrap items-center gap-2" {
                    span data-editor-status role="status" class="text-sm text-muted dark:text-haint" {}
                    button type="button" data-action="format" class="btn-quiet text-sm" title="Format (Shift+Alt+F)" {
                        (icon(icons::MAGIC_WAND, "h-4 w-4")) "Format"
                    }
                    button type="button" data-action="test" class="btn-quiet text-sm" title="Run a test (Ctrl+Enter)" {
                        (icon(icons::PLAY, "h-4 w-4")) "Test"
                    }
                    button type="button" data-action="ai" class="btn-quiet text-sm" title="Let AI write or change the script" {
                        (icon(icons::SPARKLE, "h-4 w-4")) "Ask AI"
                    }
                }
            }
            div data-editor class="code-editor" {
                textarea id="automation-source" name="source" rows="22" spellcheck="false" maxlength="100000"
                    autocapitalize="off" autocomplete="off" data-lua-editor
                    class="field font-mono text-sm leading-relaxed" { (editor.source) }
            }
            p data-editor-hint hidden class="mt-1 text-xs text-muted dark:text-haint" {
                "Tab indents; press Esc, then Tab, to leave the editor. Ctrl+S saves, Ctrl+Enter tests, Ctrl+Space completes, Ctrl+/ comments."
            }
            ul data-problems hidden class="mt-2 space-y-1 text-sm" {}
            @if editor.library {
                p class="mb-5 mt-4 text-sm text-muted dark:text-haint" {
                    "Saving a library restarts the automations that use it."
                }
            } @else {
                label class="mb-5 mt-4 flex items-center gap-2" {
                    input type="checkbox" name="enabled" value="on" checked[editor.enabled] class="h-4 w-4 accent-floor";
                    span { "Run this automation" }
                }
            }
            div class="flex flex-wrap gap-2" {
                button type="submit" class="btn" { @if editor.library { "Save library" } @else { "Save automation" } }
                a href="/automations" class="btn-quiet" { "Back to automations" }
                @if let Some(id) = editor.id {
                    a href={ "/automations/export?id=" (id) } class="btn-quiet" download
                        title={ "Download a file to import on another server" @if !editor.library { ", with the libraries it needs" } } {
                        (icon(icons::DOWNLOAD_SIMPLE, "h-4 w-4")) "Export"
                    }
                }
            }
        }
        @if let Some(id) = editor.id {
            form method="post" action={ "/automations/" (id) "/delete" } class="mb-10 mt-8" {
                button type="submit" class="btn-quiet text-sm" { (icon(icons::TRASH, "h-4 w-4")) "Delete automation" }
            }
        }
    }
}

fn webhook_section(id: i64, url: &str) -> Markup {
    html! {
        div id="webhook" class="mt-10 max-w-3xl" {
            (section("Webhook", "Requests to this URL reach the script's sideporch.on_webhook handler; paths below it arrive as request.path. Anyone with the URL can call it, so treat it like a password.", &html! {
                (copy_row(url))
                pre class="mt-3 overflow-x-auto rounded-lg bg-screen p-3 text-xs dark:bg-night-2" {
                    code { "curl -X POST " (url) " -H 'Content-Type: application/json' -d '{\"hello\": \"porch\"}'" }
                }
                form method="post" action={ "/automations/" (id) "/webhook-token" } class="mt-3" {
                    button type="submit" class="btn-quiet text-sm" { (icon(icons::ARROWS_CLOCKWISE, "h-4 w-4")) "Make a new URL" }
                }
            }))
        }
    }
}

fn test_panel(library: bool) -> Markup {
    html! {
        section data-test-panel hidden aria-labelledby="test-heading" class="rounded-xl border border-line p-4 dark:border-night-line" {
            h2 id="test-heading" class="font-bold" { "Test run" }
            p class="mb-3 mt-1 text-sm text-muted dark:text-haint" {
                @if library {
                    "Loads the library as it is in the editor and lists what it exports."
                } @else {
                    "Runs the script as it is in the editor, without saving. Posts and reactions are only described, and saved data stays unchanged."
                }
            }
            form data-test-form class="space-y-3" {
                div hidden[library] {
                    label for="test-kind" class="field-label" { "Simulate" }
                    select id="test-kind" name="kind" class="field" {
                        @if library {
                            option value="load" selected { "Loading only" }
                        } @else {
                            option value="message" { "A new message" }
                            option value="reaction" { "A reaction" }
                            option value="command" { "A slash command" }
                            option value="webhook" { "A webhook request" }
                            option value="timer" { "Schedules firing" }
                            option value="member_joined" { "Someone joining" }
                            option value="channel_created" { "A new channel" }
                            option value="load" { "Loading only" }
                        }
                    }
                }
                div data-for="message reaction" {
                    label for="test-text" class="field-label" { "Message text" }
                    input id="test-text" name="text" class="field font-mono text-sm" value="!ping";
                }
                div data-for="message reaction command" {
                    label for="test-channel" class="field-label" { "Channel" }
                    input id="test-channel" name="channel" class="field" value="general";
                }
                div data-for="command" hidden {
                    label for="test-command" class="field-label" { "Command" }
                    input id="test-command" name="command" class="field font-mono text-sm" value="/help";
                }
                div data-for="reaction" hidden {
                    label for="test-emoji" class="field-label" { "Emoji" }
                    input id="test-emoji" name="emoji" class="field" value="thumbsup";
                    label class="mt-2 flex items-center gap-2 text-sm" {
                        input type="checkbox" name="added" checked class="h-4 w-4 accent-floor";
                        "Added (off: removed)"
                    }
                }
                div data-for="member_joined" hidden {
                    label for="test-user" class="field-label" { "New member" }
                    input id="test-user" name="user" class="field" value="Test Person";
                }
                div data-for="channel_created" hidden {
                    label for="test-new-channel" class="field-label" { "New channel" }
                    input id="test-new-channel" name="new_channel" class="field" value="garden-club";
                }
                div data-for="webhook" hidden {
                    div class="flex gap-2" {
                        div class="w-28" {
                            label for="test-method" class="field-label" { "Method" }
                            select id="test-method" name="method" class="field" {
                                option { "POST" } option { "GET" } option { "PUT" } option { "DELETE" }
                            }
                        }
                        div class="min-w-0 flex-1" {
                            label for="test-path" class="field-label" { "Path" }
                            input id="test-path" name="path" class="field font-mono text-sm" placeholder="/deploy";
                        }
                    }
                    label for="test-body" class="field-label mt-2" { "Body" }
                    textarea id="test-body" name="body" rows="4" class="field font-mono text-xs" spellcheck="false" { "{\"hello\": \"porch\"}" }
                }
                label class="flex items-start gap-2 text-sm" {
                    input type="checkbox" name="http" checked class="mt-0.5 h-4 w-4 accent-floor";
                    span { "Make real HTTP requests " span class="text-muted dark:text-haint" { "(they reach the outside world)" } }
                }
                button type="submit" class="btn w-full" { (icon(icons::PLAY, "h-4 w-4")) "Run test" }
            }
            div data-test-output aria-live="polite" class="mt-4 empty:hidden" {}
        }
    }
}

fn ai_panel(ready: bool, is_admin: bool) -> Markup {
    html! {
        section data-ai-panel hidden aria-labelledby="ai-heading" class="rounded-xl border border-line p-4 dark:border-night-line" {
            h2 id="ai-heading" class="flex items-center gap-2 font-bold" { (icon(icons::SPARKLE, "h-5 w-5")) "Ask AI" }
            @if ready {
                p class="mb-3 mt-1 text-sm text-muted dark:text-haint" {
                    "Describe what the automation should do, or how to change the script. You can review the result before it replaces anything."
                }
                form data-ai-form class="space-y-3" {
                    label for="ai-prompt" class="sr-only" { "What should the script do?" }
                    textarea id="ai-prompt" name="prompt" rows="4" required maxlength="4000" class="field text-sm"
                        placeholder="When someone reacts with :eyes: to a message in #alerts, answer in the thread that they're looking into it." {}
                    button type="submit" class="btn w-full" { (icon(icons::SPARKLE, "h-4 w-4")) "Write the script" }
                }
                div data-ai-output aria-live="polite" class="mt-4 empty:hidden" {}
            } @else {
                p class="mt-1 text-sm text-muted dark:text-haint" {
                    "No AI provider is connected yet. "
                    @if is_admin {
                        a href="/settings/ai" class="font-semibold underline" { "Connect one" }
                        " (OpenAI, Anthropic, Ollama, or any OpenAI-compatible API) to let it write scripts."
                    }
                }
            }
        }
    }
}

fn runs_section(runs: &[AutomationRun]) -> Markup {
    section(
        "Recent runs",
        "Runs that printed, posted, reacted, answered a webhook, or failed. The newest 100 are kept.",
        &html! {
            @if runs.is_empty() {
                p class="text-sm text-muted dark:text-haint" { "Nothing yet." }
            } @else {
                ol class="space-y-2" {
                    @for run in runs {
                        li class="rounded-lg border border-line px-3 py-2 text-sm dark:border-night-line" {
                            div class="flex flex-wrap items-center gap-2" {
                                span class="rounded bg-screen px-2 font-mono text-xs dark:bg-night-2" { (run.trigger) }
                                (datetime(run.started_at))
                                span class="text-xs text-muted dark:text-haint" { (duration(run.duration_us)) }
                                @if run.error.is_some() {
                                    span class="ml-auto rounded bg-red-100 px-2 text-xs font-semibold text-red-800 dark:bg-red-950 dark:text-red-200" { "Error" }
                                }
                            }
                            @if !run.output.is_empty() {
                                pre class="mt-2 overflow-x-auto whitespace-pre-wrap font-mono text-xs" { (run.output) }
                            }
                            @if let Some(error) = &run.error {
                                p class="mt-1 font-mono text-xs text-red-800 dark:text-red-200" { (error) }
                            }
                        }
                    }
                }
            }
        },
    )
}

fn history_section(id: Option<i64>, versions: &[AutomationVersion]) -> Markup {
    section(
        "History",
        "Every saved version of the script, newest first. Restoring one saves it as a new version.",
        &html! {
            ol class="space-y-2" {
                @for (index, version) in versions.iter().enumerate() {
                    li class="rounded-lg border border-line px-3 py-2 text-sm dark:border-night-line" {
                        details {
                            summary class="flex cursor-pointer flex-wrap items-center gap-2" {
                                (datetime(version.saved_at))
                                span class="text-muted dark:text-haint" {
                                    "by " (version.saved_by.as_deref().unwrap_or("a former member"))
                                    " with " (version.saved_with)
                                }
                                @if index == 0 {
                                    span class="ml-auto rounded bg-haint-2 px-2 text-xs font-semibold text-floor dark:bg-floor-2 dark:text-haint-2" { "Current" }
                                }
                            }
                            pre data-lua class="code-preview mt-2 max-h-80" { (version.source) }
                            @if let (Some(id), true) = (id, index > 0) {
                                form method="post" action={ "/automations/" (id) "/versions/" (version.id) "/restore" } class="mt-2" {
                                    button type="submit" class="btn-quiet text-sm" { (icon(icons::ARROW_COUNTER_CLOCKWISE, "h-4 w-4")) "Restore this version" }
                                }
                            }
                        }
                    }
                }
            }
        },
    )
}

fn reference_section() -> Markup {
    section(
        "What scripts can do",
        api::LIMITS,
        &html! {
            dl class="space-y-3 text-sm" {
                @for function in api::FUNCTIONS {
                    div {
                        dt { code class="font-mono font-semibold" { (function.signature()) } }
                        dd class="mt-0.5 text-muted dark:text-haint" { (inline_code(function.doc)) }
                    }
                }
            }
            @for (name, fields) in api::EVENTS {
                h3 class="mb-2 mt-5 font-semibold" { (inline_code(name)) }
                dl class="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-sm" {
                    @for (field, doc) in *fields {
                        dt { code class="font-mono" { (field) } }
                        dd class="text-muted dark:text-haint" { (inline_code(doc)) }
                    }
                }
            }
        },
    )
}

/// Text with `backticked` parts shown as code.
fn inline_code(text: &str) -> Markup {
    html! {
        @for (index, part) in text.split('`').enumerate() {
            @if index % 2 == 1 {
                code class="font-mono text-ink dark:text-haint-2" { (part) }
            } @else {
                (part)
            }
        }
    }
}

fn datetime(at: i64) -> Markup {
    let when = jiff::Timestamp::from_millisecond(at).unwrap_or_default();
    html! {
        time datetime=(when.to_string()) data-format="datetime" class="text-xs text-muted dark:text-haint" {
            (when.strftime("%Y-%m-%d %H:%M UTC").to_string())
        }
    }
}

fn duration(micros: i64) -> String {
    if micros < 1_000 {
        format!("{micros} µs")
    } else {
        let millis = micros.checked_div(1_000).unwrap_or_default();
        let tenths = micros
            .checked_rem(1_000)
            .and_then(|rest| rest.checked_div(100))
            .unwrap_or_default();
        format!("{millis}.{tenths} ms")
    }
}

/// Where an import starts: a file to upload, or its text pasted.
pub fn import_page(shell: &Shell<'_>, error: Option<&str>) -> Markup {
    panel_page(
        "Import automations",
        shell,
        &html! { "Import automations" },
        &html! {
            (super::settings::tabs("/automations"))
            (form_error(error))
            p class="mb-5 text-muted dark:text-haint" {
                "Bring in automations and libraries exported from another Sideporch server. "
                "You'll see what the file holds before anything is added, and imported automations start switched off."
            }
            form method="post" action="/automations/import" enctype="multipart/form-data" class="max-w-xl space-y-5" {
                div {
                    label for="import-file" class="field-label" { "File" }
                    input id="import-file" name="file" type="file" accept=".json,application/json" class="text-sm";
                    p class="mt-1 text-sm text-muted dark:text-haint" { "A " code { ".sideporch.json" } " file from Export." }
                }
                div {
                    label for="import-text" class="field-label" { "Or paste it" }
                    textarea id="import-text" name="bundle" rows="8" spellcheck="false" class="field font-mono text-xs"
                        placeholder="{ \"format\": \"sideporch-automations\", … }" {}
                }
                div class="flex flex-wrap gap-2" {
                    button type="submit" class="btn" { (icon(icons::UPLOAD_SIMPLE, "h-5 w-5")) "Preview" }
                    a href="/automations" class="btn-quiet" { "Back to automations" }
                }
            }
        },
    )
}

fn plan_badge(plan: &Plan) -> Markup {
    let (text, tone) = match (plan.existing, plan.unchanged) {
        (None, _) => (
            "New",
            "bg-haint-2 text-floor dark:bg-floor-2 dark:text-haint-2",
        ),
        (Some(_), true) => (
            "Already here, unchanged",
            "bg-screen text-muted dark:bg-night-2 dark:text-haint",
        ),
        (Some(_), false) => (
            if plan.item.is_library() {
                "This server already has a library with this name"
            } else {
                "This server already has an automation with this name"
            },
            "bg-amber-100 text-amber-900 dark:bg-amber-950 dark:text-amber-200",
        ),
    };
    html! { span class={ "rounded px-2 text-xs font-semibold " (tone) } { (text) } }
}

const fn action_label(action: Action, library: bool) -> &'static str {
    match action {
        Action::Import => "Import, switched off",
        Action::Replace if library => "Replace the library here",
        Action::Replace => "Replace the one here, switched off",
        Action::Copy => "Import as a copy, switched off",
        Action::Skip => "Skip",
    }
}

/// What importing a file would do, with a choice per item.
pub fn import_preview(shell: &Shell<'_>, json: &str, bundle: &Bundle, plans: &[Plan]) -> Markup {
    let missing: std::collections::BTreeSet<&str> = plans
        .iter()
        .flat_map(|plan| plan.missing_secrets.iter().map(String::as_str))
        .collect();
    panel_page(
        "Import automations",
        shell,
        &html! { "Import automations" },
        &html! {
            (super::settings::tabs("/automations"))
            p class="mb-2 text-muted dark:text-haint" {
                "The file holds " (plans.len()) @if plans.len() == 1 { " script" } @else { " scripts" }
                @if !bundle.sideporch.is_empty() { ", exported from Sideporch " (bundle.sideporch) } ". "
                "Read each one before you switch it on: automations can post, react, and call other services."
            }
            @if let Some(description) = &bundle.description {
                p class="mb-2 text-sm" { (description) }
            }
            @if !missing.is_empty() {
                p class="mb-4 rounded-lg border border-amber-200 bg-amber-50 px-3 py-2 text-sm text-amber-900 dark:border-amber-900 dark:bg-amber-950 dark:text-amber-200" {
                    "Add these secrets under " a href="/settings/secrets" class="underline" { "Secrets" } " before switching them on: "
                    @for (index, name) in missing.iter().enumerate() {
                        @if index > 0 { ", " }
                        code { (name) }
                    }
                }
            }
            form method="post" action="/automations/import/confirm" class="space-y-4" {
                textarea name="bundle" hidden { (json) }
                ol class="space-y-3" {
                    @for (index, plan) in plans.iter().enumerate() {
                        li class="rounded-xl border border-line p-4 dark:border-night-line" data-import-item {
                            div class="flex flex-wrap items-center gap-2" {
                                @if plan.item.is_library() {
                                    (icon(icons::BOOKS, "h-5 w-5 shrink-0 text-floor-3 dark:text-haint"))
                                } @else {
                                    (icon(icons::LIGHTNING, "h-5 w-5 shrink-0 text-floor-3 dark:text-haint"))
                                }
                                span class="font-semibold" { (plan.item.name) }
                                @if plan.item.is_library() { span class="text-xs text-muted dark:text-haint" { "library" } }
                                (plan_badge(plan))
                            }
                            @if let Some(description) = &plan.item.description {
                                p class="mt-1 text-sm" { (description) }
                            }
                            div class="mt-2 space-y-1 text-sm text-muted dark:text-haint" {
                                @if !plan.item.secrets.is_empty() {
                                    p {
                                        "Reads secrets: "
                                        @for (index, name) in plan.item.secrets.iter().enumerate() {
                                            @if index > 0 { ", " }
                                            code { (name) }
                                            @if plan.missing_secrets.contains(name) { span class="text-amber-700 dark:text-amber-300" { " (not set here)" } }
                                        }
                                    }
                                }
                                @if !plan.item.requires.is_empty() {
                                    p {
                                        "Loads libraries: "
                                        @for (index, name) in plan.item.requires.iter().enumerate() {
                                            @if index > 0 { ", " }
                                            code { (name) }
                                            @if plan.missing_libraries.contains(name) { span class="text-red-700 dark:text-red-300" { " (missing)" } }
                                        }
                                    }
                                }
                                @if plan.errors > 0 || plan.warnings > 0 {
                                    p {
                                        "The linter found "
                                        @if plan.errors > 0 { span class="text-red-700 dark:text-red-300" { (plan.errors) @if plan.errors == 1 { " error" } @else { " errors" } } }
                                        @if plan.errors > 0 && plan.warnings > 0 { " and " }
                                        @if plan.warnings > 0 { (plan.warnings) @if plan.warnings == 1 { " warning" } @else { " warnings" } }
                                        "; the editor shows where."
                                    }
                                }
                            }
                            details class="mt-2" {
                                summary class="cursor-pointer text-sm font-semibold" { "Read the script" }
                                pre class="mt-2 max-h-80 overflow-auto rounded-lg bg-screen p-3 font-mono text-xs dark:bg-night-2" { code { (plan.item.source) } }
                            }
                            div class="mt-3" {
                                label for={ "action-" (index) } class="field-label" { "Import" }
                                select id={ "action-" (index) } name={ "action_" (index) } class="field max-w-sm" {
                                    @for action in plan.choices() {
                                        option value=(action.key()) { (action_label(action, plan.item.is_library())) }
                                    }
                                }
                            }
                        }
                    }
                }
                div class="flex flex-wrap gap-2" {
                    button type="submit" class="btn" { "Import" }
                    a href="/automations/import" class="btn-quiet" { "Choose another file" }
                }
            }
        },
    )
}
