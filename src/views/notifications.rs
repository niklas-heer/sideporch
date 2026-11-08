//! Pausing notifications, and quiet hours, from the person's own settings.

use maud::{Markup, html};

use super::{Shell, form_error, panel_page};
use crate::{
    icons,
    notifications::{QuietHours, WEEKDAYS, day_is_on, format_clock},
};

pub struct Status<'a> {
    pub quiet: QuietHours,
    /// "Paused until 14:30", while a pause runs.
    pub paused: Option<&'a str>,
    /// The time zone quiet hours are read in.
    pub zone: &'a str,
}

pub fn page(shell: &Shell<'_>, status: &Status<'_>, error: Option<&str>, saved: bool) -> Markup {
    panel_page(
        "Notifications",
        shell,
        &html! { "Notifications" },
        &html! {
            (form_error(error))
            @if saved {
                p role="status" class="mb-4 rounded-lg border border-line bg-haint-2 px-3 py-2 text-sm text-floor dark:border-night-line dark:bg-floor-2 dark:text-haint-2" {
                    "Saved."
                }
            }
            p class="mb-6 max-w-xl text-muted dark:text-haint" {
                "Pauses and quiet hours only stop push notifications on your devices. "
                "Mentions still appear in Activity, and unread markers still update."
            }
            section aria-labelledby="pause-heading" class="mb-8 max-w-xl" {
                h2 id="pause-heading" class="text-base font-bold" { "Pause" }
                @if let Some(label) = status.paused {
                    p class="mt-2 flex items-center gap-2 font-semibold" {
                        (icons::icon(icons::PAUSE, "h-4 w-4 shrink-0"))
                        (label)
                    }
                } @else {
                    p class="mt-2 text-muted dark:text-haint" { "Notifications are on." }
                }
                div class="mt-3 flex flex-wrap gap-2" {
                    form method="post" action="/settings/notifications/pause" {
                        button type="submit" name="duration" value="1h" class="btn" { "For 1 hour" }
                    }
                    form method="post" action="/settings/notifications/pause" {
                        button type="submit" name="duration" value="8h" class="btn" { "For 8 hours" }
                    }
                    form method="post" action="/settings/notifications/pause" {
                        button type="submit" name="duration" value="tomorrow" class="btn" { "Until tomorrow morning" }
                    }
                    @if status.paused.is_some() {
                        form method="post" action="/settings/notifications/pause" {
                            button type="submit" name="duration" value="resume" class="btn" { "Resume" }
                        }
                    }
                }
            }
            section aria-labelledby="quiet-heading" class="max-w-xl" {
                h2 id="quiet-heading" class="text-base font-bold" { "Quiet hours" }
                p class="mt-1 text-sm text-muted dark:text-haint" {
                    "They repeat every week, on your own clock in " (status.zone) ". "
                    "A window that crosses midnight counts for the day it starts on."
                }
                form method="post" action="/settings/notifications/quiet-hours" class="mt-4 space-y-5" {
                    label class="flex items-start gap-2" {
                        input type="checkbox" name="enabled" value="on" checked[status.quiet.enabled] class="mt-1 h-4 w-4 accent-floor";
                        span class="font-semibold" { "Turn on quiet hours" }
                    }
                    div class="flex flex-wrap gap-4" {
                        div {
                            label for="quiet-start" class="field-label" { "From" }
                            input id="quiet-start" type="time" name="start" required class="field" value=(format_clock(status.quiet.start));
                        }
                        div {
                            label for="quiet-end" class="field-label" { "Until" }
                            input id="quiet-end" type="time" name="end" required class="field" value=(format_clock(status.quiet.end));
                        }
                    }
                    fieldset {
                        legend class="field-label mb-2" { "On" }
                        div class="flex flex-wrap gap-x-4 gap-y-2" {
                            @for (day, (code, name)) in (0_u8..).zip(WEEKDAYS) {
                                label class="flex items-center gap-2" {
                                    input type="checkbox" name="days" value=(code) checked[day_is_on(status.quiet.days, day)] class="h-4 w-4 accent-floor";
                                    (name)
                                }
                            }
                        }
                    }
                    button type="submit" class="btn" { "Save quiet hours" }
                }
            }
        },
    )
}
