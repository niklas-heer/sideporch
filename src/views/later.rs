//! The Scheduled page: messages waiting to be sent and reminders.

use jiff::{Timestamp, tz::TimeZone};
use maud::{Markup, html};

use super::{Shell, panel_page, section};
use crate::{
    icons::{self, icon},
    later,
    store::{Channel, ChannelKind, Reminder, Scheduled},
};

fn when(at: i64, zone: &TimeZone) -> Markup {
    let stamp = Timestamp::from_millisecond(at).unwrap_or_default();
    html! {
        time datetime=(stamp.to_string()) data-format="datetime" class="font-semibold" {
            (later::describe(&stamp.to_zoned(zone.clone())))
        }
    }
}

pub fn page(
    shell: &Shell<'_>,
    reminders: &[Reminder],
    scheduled: &[(Scheduled, Channel)],
    zone: &TimeZone,
) -> Markup {
    panel_page(
        "Scheduled",
        shell,
        &html! { "Scheduled" },
        &html! {
            (section("Messages", "Messages you wrote to send later. Schedule one with the clock next to Send.", &html! {
                @if scheduled.is_empty() {
                    p class="text-muted dark:text-haint" { "Nothing scheduled." }
                }
                ul class="space-y-3" {
                    @for (message, channel) in scheduled {
                        li class="rounded-xl border border-line p-4 dark:border-night-line" {
                            p class="mb-1 flex flex-wrap items-center gap-2 text-sm text-muted dark:text-haint" {
                                (icon(icons::CLOCK, "h-4 w-4")) (when(message.send_at, zone)) " to "
                                a href={ "/c/" (channel.id) } class="underline" {
                                    @if channel.kind != ChannelKind::Direct { "#" } (channel.name)
                                }
                                @if message.parent_id.is_some() { " (in a thread)" }
                            }
                            p class="whitespace-pre-wrap break-words" { (message.body) }
                            div class="mt-3 flex gap-2" {
                                form method="post" action={ "/scheduled/" (message.id) "/send" } {
                                    button type="submit" class="btn px-3 py-1 text-sm" { "Send now" }
                                }
                                form method="post" action={ "/scheduled/" (message.id) "/cancel" } {
                                    button type="submit" class="btn-quiet text-sm" { "Cancel" }
                                }
                            }
                        }
                    }
                }
            }))
            (section("Reminders", "Set one with /remind, such as “/remind me tomorrow to water the plants”, or from a message's menu. Reminders arrive in your notes to self.", &html! {
                @if reminders.is_empty() {
                    p class="text-muted dark:text-haint" { "No reminders." }
                }
                ul class="space-y-3" {
                    @for reminder in reminders {
                        li class="flex items-start gap-3 rounded-xl border border-line p-4 dark:border-night-line" {
                            (icon(icons::ALARM, "mt-0.5 h-5 w-5 shrink-0 text-muted dark:text-haint"))
                            div class="min-w-0 flex-1" {
                                p class="text-sm text-muted dark:text-haint" { (when(reminder.remind_at, zone)) }
                                p class="break-words" { (reminder.text) }
                                @if let Some(link) = &reminder.link {
                                    a href=(link) class="text-sm underline" { "See the message" }
                                }
                            }
                            form method="post" action={ "/reminders/" (reminder.id) "/cancel" } {
                                button type="submit" class="btn-quiet text-sm" { "Cancel" }
                            }
                        }
                    }
                }
            }))
        },
    )
}
