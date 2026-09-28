//! Channel pages: settings, members of private channels, and the directory
//! of every channel.

use maud::{Markup, html};

use super::{Context, Shell, avatar, channel_label, copy_row, panel_page, section, user_author};
use crate::{
    icons::{self, icon},
    store::{Channel, ChannelKind, DirectoryEntry, OutgoingWebhook, Policy, User, Webhook},
};

pub struct ChannelSettings<'a> {
    pub channel: &'a Channel,
    pub hooks: &'a [Webhook],
    pub base_url: &'a str,
    /// Members of a private channel.
    pub members: &'a [User],
    /// Everyone, to add to a private channel or make a manager.
    pub everyone: &'a [User],
    /// Who manages the channel, besides admins.
    pub managers: &'a [User],
    pub outgoing: &'a [OutgoingWebhook],
}

pub fn channel_settings_page(shell: &Shell<'_>, settings: &ChannelSettings<'_>) -> Markup {
    let channel = settings.channel;
    panel_page(
        "Channel settings",
        shell,
        &html! { (channel_label(channel)) },
        &html! {
            (section("Topic", "A short line shown at the top of the channel.", &html! {
                form method="post" action={ "/c/" (channel.id) "/topic" } class="flex gap-2" {
                    input name="topic" value=(channel.topic) maxlength="200" aria-label="Topic" class="field" placeholder="What's this channel for?";
                    button type="submit" class="btn shrink-0" { "Save topic" }
                }
            }))
            @if channel.kind == ChannelKind::Private {
                (members_section(shell, settings))
            }
            (permissions_section(settings))
            (webhooks_section(settings))
            (outgoing_section(settings))
            (section("Leave", leave_intro(channel), &html! {
                form method="post" action={ "/c/" (channel.id) "/leave" } {
                    button type="submit" class="btn-quiet" { (icon(icons::SIGN_OUT, "h-4 w-4")) "Leave #" (channel.name) }
                }
            }))
        },
    )
}

const fn leave_intro(channel: &Channel) -> &'static str {
    match channel.kind {
        ChannelKind::Private => "You stop seeing it. A member has to add you again to come back.",
        _ => "It leaves your sidebar. Find it again under Browse channels.",
    }
}

fn policy_select(name: &str, label: &str, current: Policy, enabled: bool) -> Markup {
    html! {
        div {
            label for={ "policy-" (name) } class="field-label" { (label) }
            select id={ "policy-" (name) } name=(name) class="field" disabled[!enabled] {
                option value="everyone" selected[current == Policy::Everyone] { "Everyone" }
                option value="managers" selected[current == Policy::Managers] { "Only managers" }
            }
        }
    }
}

/// Who may post, reply and react, and who manages the channel.
fn permissions_section(settings: &ChannelSettings<'_>) -> Markup {
    let channel = settings.channel;
    let posting = channel.posting;
    let candidates: Vec<&User> = settings
        .everyone
        .iter()
        .filter(|user| {
            !user.deactivated
                && !user.is_admin
                && !settings
                    .managers
                    .iter()
                    .any(|manager| manager.id == user.id)
        })
        .collect();
    section(
        "Permissions",
        "Managers and admins can always post. For an announcement channel, let only managers start posts while everyone replies in threads and reacts.",
        &html! {
            form method="post" action={ "/c/" (channel.id) "/permissions" } class="mb-6 grid gap-3 sm:grid-cols-3" {
                (policy_select("post", "New posts", posting.post, channel.manager))
                (policy_select("reply", "Replies in threads", posting.reply, channel.manager))
                (policy_select("react", "Reactions and votes", posting.react, channel.manager))
                @if channel.manager {
                    div class="sm:col-span-3" { button type="submit" class="btn" { "Save permissions" } }
                }
            }
            h3 class="mb-2 font-semibold" { "Managers" }
            @if settings.managers.is_empty() {
                p class="mb-3 text-sm text-muted dark:text-haint" { "Only admins manage this channel." }
            }
            ul class="mb-4" {
                @for manager in settings.managers {
                    li class="flex items-center gap-3 border-b border-line py-2 last:border-b-0 dark:border-night-line" {
                        (avatar(&user_author(manager), &Context::default()))
                        span class="min-w-0 flex-1 truncate" { (manager.display_name) " " span class="text-muted dark:text-haint" { "@" (manager.username) } }
                        @if channel.manager {
                            form method="post" action={ "/c/" (channel.id) "/managers/" (manager.id) "/remove" } {
                                button type="submit" class="btn-quiet text-sm" { "Remove" }
                            }
                        }
                    }
                }
            }
            @if channel.manager && !candidates.is_empty() {
                form method="post" action={ "/c/" (channel.id) "/managers" } class="flex items-end gap-2" {
                    div class="flex-1" {
                        label for="manager" class="field-label" { "Add a manager" }
                        select id="manager" name="user_id" class="field" {
                            @for user in candidates {
                                option value=(user.id) { (user.display_name) " (@" (user.username) ")" }
                            }
                        }
                    }
                    button type="submit" class="btn shrink-0" { "Add" }
                }
            }
            @if !channel.manager {
                p class="text-sm text-muted dark:text-haint" { "Managers and admins change these." }
            }
        },
    )
}

fn members_section(shell: &Shell<'_>, settings: &ChannelSettings<'_>) -> Markup {
    let channel = settings.channel;
    let can_remove = shell.user.is_admin || channel.created_by == Some(shell.user.id);
    let others: Vec<&User> = settings
        .everyone
        .iter()
        .filter(|user| !user.deactivated && !settings.members.iter().any(|m| m.id == user.id))
        .collect();
    section(
        "Members",
        "Only these people see the channel. Any member can add people; whoever created it and admins can remove them.",
        &html! {
            ul class="mb-4" {
                @for member in settings.members {
                    li class="flex items-center gap-3 border-b border-line py-2 last:border-b-0 dark:border-night-line" {
                        (avatar(&user_author(member), &Context::default()))
                        span class="min-w-0 flex-1 truncate" { (member.display_name) " " span class="text-muted dark:text-haint" { "@" (member.username) } }
                        @if can_remove && member.id != shell.user.id {
                            form method="post" action={ "/c/" (channel.id) "/members/" (member.id) "/remove" } {
                                button type="submit" class="btn-quiet text-sm" { "Remove" }
                            }
                        }
                    }
                }
            }
            @if !others.is_empty() {
                form method="post" action={ "/c/" (channel.id) "/members" } class="flex items-end gap-2" {
                    div class="flex-1" {
                        label for="member" class="field-label" { "Add someone" }
                        select id="member" name="user_id" class="field" {
                            @for user in others {
                                option value=(user.id) { (user.display_name) " (@" (user.username) ")" }
                            }
                        }
                    }
                    button type="submit" class="btn shrink-0" { "Add" }
                }
            }
        },
    )
}

fn outgoing_section(settings: &ChannelSettings<'_>) -> Markup {
    let channel = settings.channel;
    section(
        "Outgoing webhooks",
        "Send people's messages here to another service as JSON, like Slack's and Mattermost's outgoing webhooks. With trigger words, only messages that start with one go out. If the service answers with JSON that has \"text\", it is posted here.",
        &html! {
            @if !settings.outgoing.is_empty() {
                ul class="mb-5 space-y-4" {
                    @for hook in settings.outgoing {
                        li class="rounded-xl border border-line p-4 dark:border-night-line" {
                            div class="mb-2 flex items-center gap-2" {
                                (icon(icons::PAPER_PLANE_RIGHT, "h-5 w-5 text-floor-3 dark:text-haint"))
                                span class="font-semibold" { (hook.name) }
                                form method="post" action={ "/c/" (channel.id) "/outgoing/" (hook.id) "/delete" } class="ml-auto" {
                                    button type="submit" class="btn-quiet text-sm" aria-label={ "Delete outgoing webhook " (hook.name) } {
                                        (icon(icons::TRASH, "h-4 w-4")) "Delete"
                                    }
                                }
                            }
                            p class="truncate font-mono text-sm" { (hook.url) }
                            p class="mt-1 text-sm text-muted dark:text-haint" {
                                @if hook.triggers.is_empty() { "Every message" } @else { "Messages starting with " (hook.triggers.join(", ")) }
                                " · token " code { (hook.token) }
                            }
                            @if let Some(at) = hook.last_at {
                                p class="mt-1 text-sm" {
                                    "Last sent " (super::timestamp_date(at)) ": "
                                    @if let Some(error) = &hook.last_error { span class="text-red-700 dark:text-red-300" { (error) } }
                                    @else { "OK" }
                                }
                            }
                        }
                    }
                }
            }
            form method="post" action={ "/c/" (channel.id) "/outgoing" } class="grid gap-3 sm:grid-cols-2" {
                div {
                    label for="outgoing-name" class="field-label" { "Name" }
                    input id="outgoing-name" name="name" required maxlength="80" placeholder="Deploy bot" class="field";
                }
                div {
                    label for="outgoing-triggers" class="field-label" { "Trigger words (optional)" }
                    input id="outgoing-triggers" name="triggers" maxlength="200" placeholder="!deploy, !status" class="field";
                }
                div class="sm:col-span-2" {
                    label for="outgoing-url" class="field-label" { "URL" }
                    input id="outgoing-url" name="url" type="url" required placeholder="https://example.com/hooks/sideporch" class="field font-mono text-sm";
                }
                div { button type="submit" class="btn" { "Add outgoing webhook" } }
            }
        },
    )
}

fn webhooks_section(settings: &ChannelSettings<'_>) -> Markup {
    let channel = settings.channel;
    let base_url = settings.base_url;
    let example = settings.hooks.first().map_or_else(
        || format!("{base_url}/hooks/…"),
        |hook| format!("{base_url}/hooks/{}", hook.token),
    );
    section(
        "Webhooks",
        "Monitors, CI systems and scripts can post here through a webhook URL. It accepts Slack's incoming-webhook format, so tools like Gatus and Grafana work unchanged.",
        &html! {
            @if !settings.hooks.is_empty() {
                ul class="mb-5 space-y-4" {
                    @for hook in settings.hooks {
                        li class="rounded-xl border border-line p-4 dark:border-night-line" {
                            div class="mb-2 flex items-center gap-2" {
                                (icon(icons::WEBHOOKS_LOGO, "h-5 w-5 text-floor-3 dark:text-haint"))
                                span class="font-semibold" { (hook.name) }
                                form method="post" action={ "/c/" (channel.id) "/webhooks/" (hook.id) "/delete" } class="ml-auto" {
                                    button type="submit" class="btn-quiet text-sm" aria-label={ "Delete webhook " (hook.name) } {
                                        (icon(icons::TRASH, "h-4 w-4")) "Delete"
                                    }
                                }
                            }
                            (copy_row(&format!("{base_url}/hooks/{}", hook.token)))
                        }
                    }
                }
            }
            form method="post" action={ "/c/" (channel.id) "/webhooks" } class="flex items-end gap-2" {
                div class="flex-1" {
                    label for="webhook-name" class="field-label" { "Name" }
                    input id="webhook-name" name="name" required maxlength="80" placeholder="Gatus, Grafana, CI…" class="field";
                }
                button type="submit" class="btn shrink-0" { "Create webhook" }
            }
            details class="mt-5 rounded-xl bg-screen p-4 dark:bg-night-2" {
                summary class="cursor-pointer font-semibold" { "Connect Gatus" }
                p class="mb-2 mt-2 text-sm" { "Add the webhook URL to Gatus's Slack alert provider:" }
                pre class="overflow-x-auto rounded-lg bg-white p-3 text-sm dark:bg-night" {
                    code { "alerting:\n  slack:\n    webhook-url: \"" (example) "\"" }
                }
            }
        },
    )
}

pub fn directory_page(shell: &Shell<'_>, entries: &[DirectoryEntry]) -> Markup {
    panel_page(
        "Channels",
        shell,
        &html! { "All channels" },
        &html! {
            div class="mb-6 flex items-center gap-3" {
                p class="flex-1 text-muted dark:text-haint" { "Every public channel, and the private ones you're in." }
                a href="/channels/new" class="btn shrink-0" { (icon(icons::PLUS, "h-5 w-5")) "New channel" }
            }
            ul {
                @for entry in entries {
                    li class="flex items-center gap-3 border-b border-line py-3 last:border-b-0 dark:border-night-line" {
                        (icon(if entry.private { icons::LOCK_SIMPLE } else { icons::HASH }, "h-5 w-5 shrink-0 text-muted dark:text-haint"))
                        a href={ "/c/" (entry.id) } class="min-w-0 flex-1" {
                            span class="block truncate font-semibold hover:underline" { (entry.name) }
                            span class="block truncate text-sm text-muted dark:text-haint" {
                                @if entry.topic.is_empty() {
                                    (entry.messages) (if entry.messages == 1 { " message" } else { " messages" })
                                } @else { (entry.topic) }
                            }
                        }
                        @if entry.joined {
                            form method="post" action={ "/c/" (entry.id) "/leave" } {
                                button type="submit" class="btn-quiet text-sm" { "Leave" }
                            }
                        } @else {
                            form method="post" action={ "/c/" (entry.id) "/join" } {
                                button type="submit" class="btn text-sm" { "Join" }
                            }
                        }
                    }
                }
            }
        },
    )
}
