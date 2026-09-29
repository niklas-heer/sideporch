//! Signing up, and the admin and moderator pages for the community.

use maud::{Markup, PreEscaped, html};

use super::{
    AccountForm, Render, Shell, account_fields, admin::tabs, auth_page, form_error, message_item,
    panel_page, section, timestamp,
};
use crate::{
    community::{Joining, LEVELS, Permission, Registration, Requirement, Role, Signup},
    icons::{self, icon},
    markup::Context,
};

pub fn signup_page(
    joining: &Joining,
    ctx: &Context,
    error: Option<&str>,
    form: &AccountForm,
    note: &str,
) -> Markup {
    let approval = joining.registration == Registration::Approval;
    auth_page(
        "Sign up",
        &html! {
            h1 class="mb-2 text-xl font-bold" { "Join this Sideporch" }
            p class="mb-5 text-muted dark:text-haint" {
                @if approval { "Ask to join. Someone here looks at each request and lets you in." }
                @else { "Pick a name and a password to create your account." }
            }
            (form_error(error))
            form method="post" action="/signup" {
                (account_fields(form))
                @if approval {
                    div class="mb-4" {
                        label for="note" class="field-label" { "Why would you like to join?" }
                        textarea id="note" name="note" rows="3" maxlength="500" class="field" { (note) }
                        p class="mt-1 text-sm text-muted dark:text-haint" { "Optional. Helps them know who you are." }
                    }
                }
                // People never see this field; bots fill it in.
                div class="hidden" aria-hidden="true" {
                    label for="website" { "Website" }
                    input id="website" name="website" type="text" tabindex="-1" autocomplete="off";
                }
                @if !joining.rules.is_empty() {
                    div class="mb-4 rounded-lg border border-line p-3 text-sm dark:border-night-line" {
                        p class="mb-1 font-semibold" { "The rules here" }
                        div class="rich max-h-48 overflow-y-auto" { (PreEscaped(crate::markdown::render(&joining.rules, ctx))) }
                        label class="mt-2 flex gap-2 font-semibold" {
                            input type="checkbox" name="rules" value="agreed" required class="mt-1";
                            "I agree to these rules"
                        }
                    }
                }
                button type="submit" class="btn mt-2 w-full" { @if approval { "Ask to join" } @else { "Create account" } }
            }
            p class="mt-5 text-sm text-muted dark:text-haint" {
                "Have an account? " a href="/login" class="underline underline-offset-2" { "Sign in" }
            }
        },
    )
}

pub fn waiting_page(display_name: &str) -> Markup {
    auth_page(
        "Thanks",
        &html! {
            h1 class="mb-2 text-xl font-bold" { "Thanks, " (display_name) }
            p class="mb-5 text-muted dark:text-haint" {
                "Your request to join is in. Once someone here lets you in, sign in with the username and password you chose."
            }
            a href="/login" class="btn-quiet" { "Go to sign in" }
        },
    )
}

fn saved_note(saved: bool) -> Markup {
    html! {
        @if saved {
            p role="status" class="mb-4 rounded-lg border border-line bg-haint-2 px-3 py-2 text-sm text-floor dark:border-night-line dark:bg-floor-2 dark:text-haint-2" {
                "Saved."
            }
        }
    }
}

fn number_field(name: &str, value: u32, label: &str) -> Markup {
    html! {
        label class="block" {
            span class="mb-1 block text-xs text-muted dark:text-haint" { (label) }
            input type="number" name=(name) value=(value) min="0" max="1000000" class="field py-1.5";
        }
    }
}

pub fn community_page(
    shell: &Shell<'_>,
    joining: &Joining,
    requirements: &[Requirement; 3],
    counts: &[(u8, i64)],
    saved: bool,
) -> Markup {
    let modes = [
        (
            Registration::Invite,
            "With an invite link",
            "People join through links from the People page.",
        ),
        (
            Registration::Approval,
            "Ask to join",
            "Anyone can ask; admins and moderators let them in from Moderation.",
        ),
        (
            Registration::Open,
            "Anyone can sign up",
            "For public communities. New accounts start at level 0, with the limits below.",
        ),
    ];
    panel_page(
        "Community",
        shell,
        &html! { "Community" },
        &html! {
            (tabs("/admin/community"))
            (saved_note(saved))
            form method="post" action="/admin/community" class="space-y-8" {
                fieldset {
                    legend class="mb-2 font-bold" { "How people join" }
                    div class="space-y-2" {
                        @for (mode, label, hint) in modes {
                            label class="flex cursor-pointer gap-3 rounded-lg border border-line p-3 has-[:checked]:border-floor-3 dark:border-night-line" {
                                input type="radio" name="registration" value=(mode.key()) checked[joining.registration == mode] class="mt-1";
                                span {
                                    span class="block font-semibold" { (label) }
                                    span class="block text-sm text-muted dark:text-haint" { (hint) }
                                }
                            }
                        }
                    }
                    label for="rules" class="field-label mt-4" { "Rules people agree to when they sign up" }
                    textarea id="rules" name="rules" rows="4" class="field" placeholder="Be kind. No spam. Keep it about gardening." { (joining.rules) }
                    p class="mt-1 text-sm text-muted dark:text-haint" { "Markdown. Leave empty for none." }
                    p class="mt-2 text-sm text-muted dark:text-haint" {
                        "Sign-ups are limited to " (crate::community::SIGNUPS_PER_HOUR) " an hour, and a hidden field turns away simple bots."
                    }
                }
                fieldset {
                    legend class="mb-1 font-bold" { "Trust levels" }
                    p class="mb-3 text-sm text-muted dark:text-haint" {
                        "People move up on their own as they stay and take part, and never move down on their own. "
                        "Invited people start at level 1; people who sign up on their own at 0. "
                        "What each level may do is set under " a href="/admin/permissions" class="underline underline-offset-2" { "Permissions" } "."
                    }
                    div class="space-y-3" {
                        @for (level, name, hint) in LEVELS {
                            @let count = counts.iter().find(|(number, _)| *number == level).map_or(0, |(_, count)| *count);
                            div class="rounded-lg border border-line p-3 dark:border-night-line" {
                                p class="font-semibold" {
                                    (level) " · " (name)
                                    span class="ml-2 text-sm font-normal text-muted dark:text-haint" {
                                        (count) (if count == 1 { " person" } else { " people" })
                                    }
                                }
                                p class="text-sm text-muted dark:text-haint" { (hint) }
                                @if let Some(requirement) = usize::from(level).checked_sub(1).and_then(|index| requirements.get(index)) {
                                    div class="mt-2 grid grid-cols-3 gap-2" {
                                        (number_field(&format!("days_{level}"), requirement.days, "Days since joining"))
                                        (number_field(&format!("visits_{level}"), requirement.visits, "Days visited"))
                                        (number_field(&format!("messages_{level}"), requirement.messages, "Messages sent"))
                                    }
                                }
                            }
                        }
                    }
                }
                fieldset {
                    legend class="mb-1 font-bold" { "Slow down new members" }
                    div class="max-w-xs" {
                        (number_field("new_member_per_minute", joining.new_member_per_minute, "Messages a level 0 member may send per minute (0 for no limit)"))
                    }
                }
                button type="submit" class="btn" { "Save" }
            }
        },
    )
}

fn level_label(level: Option<u8>) -> String {
    match level {
        None => "Roles only".to_owned(),
        Some(0) => "Everyone".to_owned(),
        Some(4) => "Leaders".to_owned(),
        Some(level) => {
            let name = LEVELS
                .iter()
                .find(|(number, _, _)| *number == level)
                .map_or("", |(_, name, _)| name);
            format!("{name} and up")
        }
    }
}

pub fn permissions_page(
    shell: &Shell<'_>,
    levels: &[(Permission, Option<u8>)],
    roles: &[Role],
    error: Option<&str>,
    saved: bool,
) -> Markup {
    panel_page(
        "Permissions",
        shell,
        &html! { "Permissions" },
        &html! {
            (tabs("/admin/permissions"))
            (saved_note(saved))
            (form_error(error))
            p class="mb-4 max-w-xl text-sm text-muted dark:text-haint" {
                "Admins may do everything. Anyone else may do something when their trust level reaches the one it asks for "
                "(see " a href="/admin/community" class="underline underline-offset-2" { "Community" } "), "
                "or when one of their roles allows it. Give roles to people on their profile. "
                "\u{201c}Roles only\u{201d} leaves it to admins and the roles you tick."
            }
            form method="post" action="/admin/permissions" {
                div class="overflow-x-auto" {
                    table class="w-full text-sm" {
                        thead {
                            tr class="border-b border-line text-left dark:border-night-line" {
                                th class="py-2 pr-3 font-semibold" { "Permission" }
                                th class="py-2 pr-3 font-semibold" { "Trust level" }
                                @for role in roles {
                                    th class="px-2 py-2 text-center font-semibold" { (role.name) }
                                }
                            }
                        }
                        tbody {
                            @for (permission, level) in levels {
                                tr class="border-b border-line align-top dark:border-night-line" {
                                    td class="py-2 pr-3" {
                                        span class="block font-semibold" { (permission.label()) }
                                        @if !permission.description().is_empty() {
                                            span class="block text-xs text-muted dark:text-haint" { (permission.description()) }
                                        }
                                    }
                                    td class="py-2 pr-3" {
                                        select name={ "level_" (permission.key()) } aria-label={ "Trust level for " (permission.label()) } class="field py-1 text-sm" {
                                            @for choice in [Some(0_u8), Some(1), Some(2), Some(3), Some(4), None] {
                                                option value=(choice.map_or_else(|| "none".to_owned(), |level| level.to_string())) selected[*level == choice] {
                                                    (level_label(choice))
                                                }
                                            }
                                        }
                                    }
                                    @for role in roles {
                                        td class="px-2 py-2 text-center" {
                                            input type="checkbox" name={ "role_" (role.id) "_" (permission.key()) }
                                                aria-label={ (role.name) ": " (permission.label()) }
                                                checked[role.permissions.contains(permission)];
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                button type="submit" class="btn mt-4" { "Save permissions" }
            }
            div id="roles" class="mt-10" {
                (section("Roles", "Group people for what they may do, such as moderators or the design team. Their names show on profiles.", &html! {
                    @if !roles.is_empty() {
                        ul class="mb-4 space-y-2" {
                            @for role in roles {
                                li class="flex items-center gap-3 rounded-lg border border-line px-3 py-2 dark:border-night-line" {
                                    div class="min-w-0 flex-1" {
                                        p class="font-semibold" { (role.name) }
                                        p class="text-sm text-muted dark:text-haint" {
                                            (role.members) (if role.members == 1 { " person" } else { " people" })
                                            @if !role.description.is_empty() { " · " (role.description) }
                                        }
                                    }
                                    form method="post" action={ "/admin/roles/" (role.id) "/delete" } {
                                        button type="submit" class="btn-quiet p-1.5" aria-label={ "Delete the role " (role.name) } title="Delete role" {
                                            (icon(icons::TRASH, "h-4 w-4"))
                                        }
                                    }
                                }
                            }
                        }
                    }
                    form method="post" action="/admin/roles" class="flex flex-wrap items-end gap-2" {
                        label class="min-w-40 flex-1" {
                            span class="field-label" { "Name" }
                            input name="name" required maxlength="40" placeholder="Moderators" class="field";
                        }
                        label class="min-w-40 flex-[2]" {
                            span class="field-label" { "What it's for" }
                            input name="description" maxlength="200" placeholder="Keep an eye on the public channels" class="field";
                        }
                        button type="submit" class="btn" { (icon(icons::PLUS, "h-5 w-5")) "Add role" }
                    }
                }))
            }
        },
    )
}

pub fn moderation_page(
    shell: &Shell<'_>,
    signups: &[Signup],
    reports: &[crate::community::Report],
    timed_out: &[(i64, String, i64)],
    render: &Render<'_>,
) -> Markup {
    panel_page(
        "Moderation",
        shell,
        &html! { "Moderation" },
        &html! {
            @if shell.user.is_admin { (tabs("/moderation")) }
            (section("Asking to join", "People who asked for an account. Letting them in starts them at trust level 1.", &html! {
                @if signups.is_empty() {
                    p class="text-muted dark:text-haint" { "Nobody is waiting." }
                } @else {
                    ul class="space-y-3" {
                        @for signup in signups {
                            li class="rounded-xl border border-line p-4 dark:border-night-line" {
                                p class="font-semibold" { (signup.display_name) " " span class="font-normal text-muted dark:text-haint" { "@" (signup.username) } }
                                p class="text-xs text-muted dark:text-haint" { "Asked " (timestamp(signup.created_at)) }
                                @if !signup.note.is_empty() {
                                    p class="mt-2 whitespace-pre-line text-sm" { (signup.note) }
                                }
                                div class="mt-3 flex gap-2" {
                                    form method="post" action={ "/moderation/signups/" (signup.id) "/approve" } {
                                        button type="submit" class="btn px-3 py-1 text-sm" { "Let them in" }
                                    }
                                    form method="post" action={ "/moderation/signups/" (signup.id) "/decline" } {
                                        button type="submit" class="btn-quiet px-3 py-1 text-sm" { "Decline" }
                                    }
                                }
                            }
                        }
                    }
                }
            }))
            (section("Reported messages", "Messages people reported. Delete a message, or dismiss the report if it's fine.", &html! {
                @if reports.is_empty() {
                    p class="text-muted dark:text-haint" { "Nothing reported." }
                } @else {
                    ul class="space-y-4" {
                        @for report in reports {
                            li class="rounded-xl border border-line dark:border-night-line" {
                                p class="border-b border-line px-4 py-2 text-sm dark:border-night-line" {
                                    span class="font-semibold" { (report.reporter) }
                                    " reported this in " (if report.channel == "a conversation" { "a conversation".to_owned() } else { format!("#{}", report.channel) })
                                    " " (timestamp(report.created_at))
                                    @if !report.reason.is_empty() { ": \u{201c}" (report.reason) "\u{201d}" }
                                }
                                ol { (message_item(&report.message, false, false, render)) }
                                div class="flex gap-2 px-4 pb-3" {
                                    @if !report.message.deleted {
                                        form method="post" action={ "/moderation/reports/" (report.message.id) "/delete" } {
                                            button type="submit" class="btn px-3 py-1 text-sm" { "Delete message" }
                                        }
                                    }
                                    form method="post" action={ "/moderation/reports/" (report.message.id) "/dismiss" } {
                                        button type="submit" class="btn-quiet px-3 py-1 text-sm" { "Dismiss" }
                                    }
                                    @if let crate::store::Author::User { id, .. } = &report.message.author {
                                        a href={ "/people/" (id) } class="btn-quiet px-3 py-1 text-sm" { "Author's profile" }
                                    }
                                }
                            }
                        }
                    }
                }
            }))
            (section("Timed out", "They can read but not post or react until then. End a time-out from their profile.", &html! {
                @if timed_out.is_empty() {
                    p class="text-muted dark:text-haint" { "Nobody is timed out." }
                } @else {
                    ul class="space-y-1" {
                        @for (id, name, until) in timed_out {
                            li { a href={ "/people/" (id) } class="font-semibold underline underline-offset-2" { (name) } " until " (timestamp(*until)) }
                        }
                    }
                }
            }))
        },
    )
}

pub fn report_page(shell: &Shell<'_>, message: &crate::store::Message) -> Markup {
    panel_page(
        "Report",
        shell,
        &html! { "Report a message" },
        &html! {
            (report_form(message))
        },
    )
}

/// Asks why a message should be looked at. Only admins and moderators see
/// reports.
pub fn report_form(message: &crate::store::Message) -> Markup {
    html! {
        form method="post" action={ "/c/" (message.channel_id) "/m/" (message.id) "/report" } class="space-y-3" data-report-form {
            p class="text-sm text-muted dark:text-haint" { "Moderators will look at it. The author isn't told who reported it." }
            label class="block" {
                span class="field-label" { "What's wrong with it?" }
                textarea name="reason" rows="3" maxlength="500" class="field" placeholder="Spam, harassment, something else" {}
            }
            button type="submit" class="btn" { "Report" }
        }
    }
}
