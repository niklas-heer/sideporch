//! Automation management for admins.

use maud::{Markup, html};

use super::{Shell, form_error, panel_page, section, timestamp};
use crate::{
    icons::{self, icon},
    store::Automation,
};

pub fn list_page(shell: &Shell<'_>, automations: &[Automation]) -> Markup {
    panel_page(
        "Automations",
        shell,
        &html! { "Automations" },
        &html! {
            p class="mb-5 text-muted dark:text-haint" {
                "Small Lua scripts that answer messages and post on a schedule. They run inside Sideporch, sandboxed, with no access to files or the network."
            }
            a href="/automations/new" class="btn mb-6" { (icon(icons::PLUS, "h-5 w-5")) "New automation" }
            @if automations.is_empty() {
                p class="text-muted dark:text-haint" { "No automations yet." }
            } @else {
                ul class="space-y-2" {
                    @for automation in automations {
                        li {
                            a href={ "/automations/" (automation.id) }
                                class="flex items-center gap-3 rounded-xl border border-line px-4 py-3 hover:border-floor-3 dark:border-night-line" {
                                (icon(icons::LIGHTNING, "h-5 w-5 shrink-0 text-floor-3 dark:text-haint"))
                                span class="min-w-0 flex-1" {
                                    span class="block truncate font-semibold" { (automation.name) }
                                    span class="block text-xs text-muted dark:text-haint" { "Changed " (timestamp(automation.updated_at)) }
                                }
                                @if automation.last_error.is_some() {
                                    span class="rounded bg-red-100 px-2 text-xs font-semibold text-red-800 dark:bg-red-950 dark:text-red-200" { "Error" }
                                } @else if automation.enabled {
                                    span class="rounded bg-haint-2 px-2 text-xs font-semibold text-floor dark:bg-floor-2 dark:text-haint-2" { "On" }
                                } @else {
                                    span class="rounded bg-screen px-2 text-xs font-semibold text-muted dark:bg-night-2 dark:text-haint" { "Off" }
                                }
                            }
                        }
                    }
                }
            }
        },
    )
}

pub struct Editor<'a> {
    pub id: Option<i64>,
    pub name: &'a str,
    pub source: &'a str,
    pub enabled: bool,
    pub last_error: Option<&'a str>,
    pub form_error: Option<&'a str>,
}

pub fn editor_page(shell: &Shell<'_>, editor: &Editor<'_>) -> Markup {
    let action = editor.id.map_or_else(
        || "/automations".to_owned(),
        |id| format!("/automations/{id}"),
    );
    panel_page(
        "Automation",
        shell,
        &html! { @if editor.id.is_some() { (editor.name) } @else { "New automation" } },
        &html! {
            (form_error(editor.form_error))
            @if let Some(error) = editor.last_error {
                div role="alert" class="mb-4 rounded-lg border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-800 dark:border-red-900 dark:bg-red-950 dark:text-red-200" {
                    p class="font-semibold" { "The script stopped with an error" }
                    pre class="mt-1 whitespace-pre-wrap font-mono text-xs" { (error) }
                }
            }
            form method="post" action=(action) {
                div class="mb-4" {
                    label for="automation-name" class="field-label" { "Name" }
                    input id="automation-name" name="name" value=(editor.name) required maxlength="80" class="field"
                        placeholder="Porch butler";
                    p class="mt-1 text-sm text-muted dark:text-haint" { "Messages the automation posts appear under this name." }
                }
                div class="mb-4" {
                    label for="automation-source" class="field-label" { "Lua script" }
                    textarea id="automation-source" name="source" rows="18" spellcheck="false" maxlength="100000"
                        class="field font-mono text-sm leading-relaxed" { (editor.source) }
                }
                label class="mb-5 flex items-center gap-2" {
                    input type="checkbox" name="enabled" value="on" checked[editor.enabled] class="h-4 w-4 accent-floor";
                    span { "Run this automation" }
                }
                div class="flex flex-wrap gap-2" {
                    button type="submit" class="btn" { "Save automation" }
                    a href="/automations" class="btn-quiet" { "Back to automations" }
                }
            }
            @if let Some(id) = editor.id {
                form method="post" action={ "/automations/" (id) "/delete" } class="mb-10 mt-8" {
                    button type="submit" class="btn-quiet text-sm" { (icon(icons::TRASH, "h-4 w-4")) "Delete automation" }
                }
            }
            (section("What scripts can do", "Scripts get Lua's string, table, math, utf8 and coroutine libraries plus these functions:", &html! {
                pre class="overflow-x-auto rounded-lg bg-screen p-3 text-sm dark:bg-night-2" {
                    code { (REFERENCE) }
                }
            }))
        },
    )
}

const REFERENCE: &str = "sideporch.on_message(function(msg) ... end)
  -- msg.text, msg.author, msg.username, msg.channel,
  -- msg.id, msg.thread_id, msg.is_bot
sideporch.every(seconds, function() ... end)   -- at least 10 seconds
sideporch.post(\"general\", \"text\", { thread = id })
sideporch.reply(msg, \"text\")                  -- answers in the thread
sideporch.get(\"key\")  sideporch.set(\"key\", value)
sideporch.now()                                 -- Unix time in seconds
print(...)                                      -- writes to the server log";
