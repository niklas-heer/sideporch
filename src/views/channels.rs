//! Channel pages: settings, members of private channels, and the directory
//! of every channel.

use maud::{Markup, html};

use super::{Context, Shell, avatar, channel_label, copy_row, panel_page, section, user_author};
use crate::{
    icons::{self, icon},
    store::{Channel, ChannelKind, DirectoryEntry, User, Webhook},
};

pub struct ChannelSettings<'a> {
    pub channel: &'a Channel,
    pub hooks: &'a [Webhook],
    pub base_url: &'a str,
    /// Members of a private channel.
    pub members: &'a [User],
    /// Everyone, to add to a private channel.
    pub everyone: &'a [User],
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
            (webhooks_section(settings))
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
