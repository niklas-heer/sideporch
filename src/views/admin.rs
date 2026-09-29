//! The admin area: system resources and where GIFs come from.

use std::fmt::Write as _;

use maud::{Markup, html};

use super::{Shell, form_error, panel_page};
use crate::{
    gifs::{self, Provider},
    icons::{self, icon},
    store::Counts,
    system::{self, Sample, Snapshot},
};

/// Tabs across the admin pages.
pub fn tabs(current: &str) -> Markup {
    html! {
        nav class="mb-6 flex flex-wrap gap-2" aria-label="Admin" {
            @for (href, label) in [("/admin/system", "System"), ("/admin/community", "Community"), ("/admin/permissions", "Permissions"), ("/admin/sign-in", "Sign-in"), ("/moderation", "Moderation"), ("/admin/speech", "Speech"), ("/admin/backups", "Backups"), ("/admin/updates", "Updates"), ("/admin/connections", "Connections"), ("/admin/gifs", "GIFs"), ("/admin/messages", "Messages"), ("/admin/appearance", "Appearance"), ("/admin/import", "Import"), ("/people", "People"), ("/automations", "Automations")] {
                a href=(href) aria-current=[(href == current).then_some("page")]
                    class="rounded-lg px-3 py-1.5 text-sm font-semibold hover:bg-screen aria-[current=page]:bg-haint-2 aria-[current=page]:text-floor dark:hover:bg-night-2 dark:aria-[current=page]:bg-floor-2 dark:aria-[current=page]:text-haint-2" {
                    (label)
                }
            }
        }
    }
}

pub struct SystemView<'a> {
    pub samples: &'a [Sample],
    pub snapshot: &'a Snapshot,
    pub counts: &'a Counts,
    pub started_at: i64,
    pub running_automations: usize,
    pub online: usize,
}

/// A line chart of `values` (0 to `max`) as inline SVG.
fn sparkline(values: &[f64], max: f64, label: &str) -> Markup {
    const WIDTH: f64 = 240.0;
    const HEIGHT: f64 = 48.0;
    let mut points = String::new();
    let steps = values.len().saturating_sub(1).max(1);
    let step = WIDTH / f64::from(u32::try_from(steps).unwrap_or(u32::MAX));
    for (index, value) in values.iter().enumerate() {
        let x = step * f64::from(u32::try_from(index).unwrap_or(u32::MAX));
        let share = if max > 0.0 {
            (value / max).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let y = (1.0 - share).mul_add(HEIGHT - 4.0, 2.0);
        // Writing to a String cannot fail.
        let _ = write!(points, "{x:.1},{y:.1} ");
    }
    html! {
        svg viewBox="0 0 240 48" preserveAspectRatio="none" role="img" aria-label=(label)
            class="h-12 w-full text-floor-3 dark:text-haint" {
            line x1="0" y1="47" x2="240" y2="47" stroke="currentColor" stroke-opacity="0.2";
            @if values.len() > 1 {
                polyline points=(points.trim_end()) fill="none" stroke="currentColor" stroke-width="2"
                    stroke-linejoin="round" vector-effect="non-scaling-stroke";
            }
        }
    }
}

fn card(title: &str, value: &str, detail: &str, chart: Option<Markup>) -> Markup {
    html! {
        div class="rounded-xl border border-line p-4 dark:border-night-line" {
            p class="text-sm font-semibold text-muted dark:text-haint" { (title) }
            p class="mt-1 text-2xl font-bold" { (value) }
            @if !detail.is_empty() { p class="text-sm text-muted dark:text-haint" { (detail) } }
            @if let Some(chart) = chart { div class="mt-2" { (chart) } }
        }
    }
}

fn percent(value: f32) -> String {
    format!("{value:.1} %")
}

fn to_f64(value: u64) -> f64 {
    // Precise enough for a chart of byte counts.
    f64::from(u32::try_from(value / 1_024).unwrap_or(u32::MAX))
}

pub fn system_page(shell: &Shell<'_>, view: &SystemView<'_>) -> Markup {
    let latest = view.samples.last();
    let snapshot = view.snapshot;
    let cpu: Vec<f64> = view
        .samples
        .iter()
        .map(|sample| f64::from(sample.process_cpu))
        .collect();
    let machine_cpu: Vec<f64> = view
        .samples
        .iter()
        .map(|sample| f64::from(sample.system_cpu))
        .collect();
    let memory: Vec<f64> = view
        .samples
        .iter()
        .map(|sample| to_f64(sample.process_memory))
        .collect();
    let peak_memory = memory.iter().copied().fold(0.0_f64, f64::max);
    let uptime_secs =
        u64::try_from(crate::now_ms().saturating_sub(view.started_at) / 1_000).unwrap_or(0);
    let counts = view.counts;
    panel_page(
        "System",
        shell,
        &html! { "System" },
        &html! {
            (tabs("/admin/system"))
            div data-refresh="10" id="system" {
                @if let Some(latest) = latest {
                    div class="grid gap-4 sm:grid-cols-2" {
                        (card("Sideporch CPU", &percent(latest.process_cpu),
                            &format!("of {} cores; the machine uses {}", snapshot.cores, percent(latest.system_cpu)),
                            Some(sparkline(&cpu, 100.0_f64.min(cpu.iter().copied().fold(5.0, f64::max) * 1.25), "Sideporch CPU over the last ten minutes"))))
                        (card("Sideporch memory", &system::bytes(latest.process_memory),
                            &format!("machine: {} of {} used", system::bytes(latest.used_memory), system::bytes(latest.total_memory)),
                            Some(sparkline(&memory, peak_memory * 1.25, "Sideporch memory over the last ten minutes"))))
                        (card("Machine CPU", &percent(latest.system_cpu),
                            &format!("load {:.2} / {:.2} / {:.2}", snapshot.load.0, snapshot.load.1, snapshot.load.2),
                            Some(sparkline(&machine_cpu, 100.0, "Machine CPU over the last ten minutes"))))
                        (card("Disk", &snapshot.disk_free.map_or_else(|| "unknown".to_owned(), |free| format!("{} free", system::bytes(free))),
                            &snapshot.disk_total.map_or_else(String::new, |total| format!("of {} on the data directory's disk", system::bytes(total))),
                            None))
                    }
                } @else {
                    p class="text-muted dark:text-haint" { "Collecting the first measurements. They appear within a few seconds." }
                }
                h2 class="mb-3 mt-8 text-lg font-bold" { "Storage" }
                div class="grid gap-4 sm:grid-cols-2" {
                    (card("Database", &system::bytes(snapshot.database_bytes), "sideporch.db with its write-ahead log", None))
                    (card("Files", &system::bytes(snapshot.files_bytes), &format!("{} stored files", snapshot.files_count), None))
                }
                p class="mt-2 font-mono text-xs text-muted dark:text-haint" { (snapshot.data_dir.display().to_string()) }
                h2 class="mb-3 mt-8 text-lg font-bold" { "Activity" }
                dl class="grid grid-cols-2 gap-x-6 gap-y-2 text-sm sm:grid-cols-3" {
                    @for (label, value) in [
                        ("People", counts.users.to_string()),
                        ("Online now", view.online.to_string()),
                        ("Channels", counts.channels.to_string()),
                        ("Messages", counts.messages.to_string()),
                        ("Files", counts.files.to_string()),
                        ("Reactions", counts.reactions.to_string()),
                        ("Automations", format!("{} ({} running)", counts.automations, view.running_automations)),
                        ("Push subscriptions", counts.push_subscriptions.to_string()),
                    ] {
                        div {
                            dt class="text-muted dark:text-haint" { (label) }
                            dd class="font-semibold" { (value) }
                        }
                    }
                }
                h2 class="mb-3 mt-8 text-lg font-bold" { "Server" }
                dl class="grid grid-cols-2 gap-x-6 gap-y-2 text-sm sm:grid-cols-3" {
                    @for (label, value) in [
                        ("Version", env!("CARGO_PKG_VERSION").to_owned()),
                        ("Running for", system::duration(uptime_secs)),
                        ("Machine up for", system::duration(snapshot.machine_uptime_secs)),
                        ("System", snapshot.os.clone()),
                        ("Architecture", std::env::consts::ARCH.to_owned()),
                    ] {
                        div {
                            dt class="text-muted dark:text-haint" { (label) }
                            dd class="font-semibold" { (value) }
                        }
                    }
                }
                p class="mt-6 text-xs text-muted dark:text-haint" { "Updates every 10 seconds. Charts show the last ten minutes." }
            }
        },
    )
}

pub fn gifs_page(
    shell: &Shell<'_>,
    settings: &gifs::Settings,
    error: Option<&str>,
    saved: bool,
) -> Markup {
    let placeholder = |key: Option<&String>| {
        key.map_or_else(String::new, |key| {
            format!(
                "Saved key ending in …{}; leave empty to keep it",
                gifs::key_hint(key)
            )
        })
    };
    let choices = [
        (
            Provider::Local,
            "Your own library",
            "People add GIFs to the team's library and pick from it. Nothing leaves your server.",
        ),
        (
            Provider::Giphy,
            "GIPHY",
            "Search GIPHY through Sideporch, so the key stays on the server. GIFs load from GIPHY's servers, as its terms require.",
        ),
        (
            Provider::Klipy,
            "KLIPY",
            "People's browsers search KLIPY directly, as its terms require, so they can see the key.",
        ),
    ];
    panel_page(
        "GIFs",
        shell,
        &html! { "GIFs" },
        &html! {
            (tabs("/admin/gifs"))
            p class="mb-5 max-w-xl text-muted dark:text-haint" {
                "Choose where the composer's GIF button finds GIFs. The team's "
                a href="/gifs/library" class="underline" { "GIF library" }
                " is the default and works without an account anywhere. (Tenor closed its API in June 2026.)"
            }
            (form_error(error))
            @if saved {
                p role="status" class="mb-4 rounded-lg border border-line bg-haint-2 px-3 py-2 text-sm text-floor dark:border-night-line dark:bg-floor-2 dark:text-haint-2" {
                    "Saved."
                }
            }
            form method="post" action="/admin/gifs" class="max-w-lg space-y-4" {
                fieldset class="space-y-2" {
                    legend class="field-label" { "GIFs come from" }
                    @for (provider, label, help) in choices {
                        label class="flex gap-3 rounded-lg border border-line p-3 dark:border-night-line" {
                            input type="radio" name="provider" value=(provider.key()) checked[settings.provider == provider] class="mt-1";
                            span {
                                span class="block font-semibold" { (label) }
                                span class="block text-sm text-muted dark:text-haint" { (help) }
                            }
                        }
                    }
                }
                div {
                    label for="giphy-key" class="field-label" { "GIPHY API key" }
                    input id="giphy-key" name="giphy_key" type="password" autocomplete="off" class="field font-mono text-sm"
                        placeholder=(placeholder(settings.giphy_key.as_ref()));
                    p class="mt-1 text-sm text-muted dark:text-haint" {
                        "From " a href="https://developers.giphy.com" class="underline" { "developers.giphy.com" } ". Stored encrypted, like secrets."
                    }
                }
                div {
                    label for="klipy-key" class="field-label" { "KLIPY API key" }
                    input id="klipy-key" name="klipy_key" type="password" autocomplete="off" class="field font-mono text-sm"
                        placeholder=(placeholder(settings.klipy_key.as_ref()));
                    p class="mt-1 text-sm text-muted dark:text-haint" {
                        "From " a href="https://partner.klipy.com" class="underline" { "partner.klipy.com" } ". Stored encrypted, but sent to signed-in browsers while KLIPY is chosen."
                    }
                }
                div {
                    label for="gif-rating" class="field-label" { "Content rating" }
                    select id="gif-rating" name="rating" class="field" {
                        @for (value, label) in [("g", "G: suitable for everyone"), ("pg", "PG"), ("pg-13", "PG-13"), ("r", "R: adults only")] {
                            option value=(value) selected[value == settings.rating] { (label) }
                        }
                    }
                    p class="mt-1 text-sm text-muted dark:text-haint" { "For GIPHY and KLIPY searches." }
                }
                button type="submit" class="btn" { (icon(icons::GIF, "h-5 w-5")) "Save" }
            }
        },
    )
}

pub fn messages_page(shell: &Shell<'_>, previews: bool, edit_minutes: i64, saved: bool) -> Markup {
    panel_page(
        "Messages",
        shell,
        &html! { "Messages" },
        &html! {
            (tabs("/admin/messages"))
            @if saved {
                p role="status" class="mb-4 rounded-lg border border-line bg-haint-2 px-3 py-2 text-sm text-floor dark:border-night-line dark:bg-floor-2 dark:text-haint-2" {
                    "Saved."
                }
            }
            form method="post" action="/admin/messages" class="max-w-lg space-y-6" {
                div {
                    label for="edit-minutes" class="field-label" { "People can edit their messages" }
                    select id="edit-minutes" name="edit_minutes" class="field" {
                        @for (minutes, label) in crate::messages::EDIT_WINDOWS {
                            option value=(minutes) selected[*minutes == edit_minutes] {
                                @if *minutes == 0 { "Any time" } @else { "For " (label) " after sending" }
                            }
                        }
                    }
                    p class="mt-1 text-sm text-muted dark:text-haint" {
                        "A limit keeps old conversations from being rewritten; it matters more in bigger groups. Deleting always works."
                    }
                }
                label class="flex gap-3" {
                    input type="checkbox" name="previews" value="on" checked[previews] class="mt-1";
                    span {
                        span class="block font-semibold" { "Show link previews" }
                        span class="block text-sm text-muted dark:text-haint" {
                            "Sideporch fetches the first link in a message and shows its title, description and image. "
                            "Requests never reach private or internal addresses; images load from the linked site. Off: links stay plain and are never fetched."
                        }
                    }
                }
                button type="submit" class="btn" { "Save" }
            }
        },
    )
}

pub struct BackupsView<'a> {
    pub settings: &'a crate::backup::Settings,
    pub status: &'a crate::backup::Status,
    pub stored: &'a [crate::backup::Stored],
    pub dir: &'a std::path::Path,
    pub error: Option<&'a str>,
}

fn backup_schedule(view: &BackupsView<'_>) -> Markup {
    let settings = view.settings;
    html! {
        form method="post" action="/admin/backups" class="max-w-lg space-y-4" {
            div {
                label for="backup-every" class="field-label" { "Back up" }
                select id="backup-every" name="every_hours" class="field" {
                    @for (hours, label) in crate::backup::INTERVALS {
                        option value=(hours) selected[*hours == settings.every_hours] { (label) }
                    }
                }
            }
            div {
                label for="backup-keep" class="field-label" { "Keep the newest" }
                input id="backup-keep" name="keep" type="number" min="1" max="100" value=(settings.keep) class="field";
            }
            div {
                label for="backup-dir" class="field-label" { "Directory" }
                input id="backup-dir" name="dir" value=(settings.dir) class="field font-mono text-sm";
                p class="mt-1 text-sm text-muted dark:text-haint" {
                    "On the server; relative to the data directory. Now: " code { (view.dir.display()) }
                    ". Better on another disk or volume, such as a Docker volume mounted at /backups."
                }
            }
            label class="flex gap-3" {
                input type="checkbox" name="include_key" value="on" checked[settings.include_key] class="mt-1";
                span {
                    span class="block font-semibold" { "Include the secret key" }
                    span class="block text-sm text-muted dark:text-haint" { "Without it, restoring needs the same secret.key or SIDEPORCH_SECRET_KEY, or secrets must be entered again." }
                }
            }
            button type="submit" class="btn" { "Save schedule" }
        }
    }
}

pub fn backups_page(shell: &Shell<'_>, view: &BackupsView<'_>) -> Markup {
    let status = view.status;
    panel_page(
        "Backups",
        shell,
        &html! { "Backups" },
        &html! {
            (tabs("/admin/backups"))
            (form_error(view.error))
            (super::section("Download a backup", "One archive with the database, every stored file and, if you want, the secret key. The server keeps running while it is made.", &html! {
                form method="get" action="/admin/backups/download" class="flex flex-wrap items-center gap-4" {
                    label class="flex items-center gap-2" {
                        input type="checkbox" name="key" value="on" checked;
                        "Include the secret key"
                    }
                    button type="submit" class="btn" { (icon(icons::DOWNLOAD_SIMPLE, "h-5 w-5")) "Download backup" }
                }
            }))
            (super::section("Scheduled backups", "Sideporch writes backups to a directory on the server and keeps the newest few.", &backup_schedule(view)))
            (super::section("Stored backups", "", &html! {
                @if let Some(at) = status.at {
                    p class="mb-3 text-sm" {
                        "Last run " (super::timestamp_date(at)) ": "
                        @if let Some(error) = &status.error { span class="text-red-700 dark:text-red-300" { (error) } }
                        @else { (status.file.as_deref().unwrap_or("done")) }
                    }
                }
                form method="post" action="/admin/backups/run" class="mb-4" {
                    button type="submit" class="btn-quiet" { "Back up now" }
                }
                @if view.stored.is_empty() {
                    p class="text-muted dark:text-haint" { "None yet." }
                } @else {
                    ul {
                        @for stored in view.stored {
                            li class="flex items-center gap-3 border-b border-line py-2 last:border-b-0 dark:border-night-line" {
                                a href={ "/admin/backups/files/" (stored.name) } class="min-w-0 flex-1 truncate font-mono text-sm underline" { (stored.name) }
                                span class="text-sm text-muted dark:text-haint" { (system::bytes(stored.bytes)) }
                            }
                        }
                    }
                }
            }))
            (super::section("Restoring", "", &html! {
                p class="max-w-xl" {
                    "Stop Sideporch, then unpack a backup into an empty data directory with "
                    code { "sideporch restore sideporch-….tar.gz --data /path/to/data" }
                    ", and start it again. With Docker, run the same command in a one-off container that mounts the data volume."
                }
            }))
        },
    )
}

/// `1 message`, `3 messages`.
fn count(number: usize, noun: &str) -> String {
    if number == 1 {
        format!("1 {noun}")
    } else {
        format!("{number} {noun}s")
    }
}

fn import_report(report: &crate::import::Report) -> Markup {
    html! {
        div role="status" class="mb-6 rounded-xl border border-line bg-haint-2 p-4 text-floor dark:border-night-line dark:bg-floor-2 dark:text-haint-2" {
            p class="font-semibold" { "Imported." }
            ul class="mt-2 list-disc pl-5 text-sm" {
                li { (count(report.messages, "message")) ", " (count(report.reactions, "reaction")) }
                li { (count(report.channels, "new channel")) ", " (count(report.conversations, "new direct conversation")) }
                li { (count(report.people_created.len(), "new account")) ", " (report.people_matched) " matched to existing accounts by username" }
                @if report.skipped > 0 { li { (count(report.skipped, "message")) " already here, skipped" } }
            }
            @if !report.people_created.is_empty() {
                p class="mt-3 text-sm" {
                    "New accounts have no password yet. Create a reset link for each person from their profile under "
                    a href="/people" class="underline" { "People" } ": "
                    (report.people_created.iter().map(|name| format!("@{name}")).collect::<Vec<_>>().join(", "))
                }
            }
        }
    }
}

pub fn import_page(
    shell: &Shell<'_>,
    report: Option<&crate::import::Report>,
    error: Option<&str>,
) -> Markup {
    panel_page(
        "Import",
        shell,
        &html! { "Import from Slack" },
        &html! {
            (tabs("/admin/import"))
            (form_error(error))
            @if let Some(report) = report { (import_report(report)) }
            p class="mb-3 max-w-xl text-muted dark:text-haint" {
                "Bring in a Slack workspace export: people, public and private channels, direct messages, threads and reactions. "
                "Get the export in Slack under Workspace settings → Import/Export Data. "
                "People are matched to existing accounts by username; channels by name. Importing the same export again only adds what's new."
            }
            p class="mb-5 max-w-xl text-sm text-muted dark:text-haint" {
                "Slack keeps attached files behind its login, so imported messages name their files instead of showing them."
            }
            form method="post" action="/admin/import" enctype="multipart/form-data" class="flex flex-wrap items-end gap-3" {
                div {
                    label for="export" class="field-label" { "Slack export (.zip)" }
                    input id="export" name="export" type="file" accept=".zip,application/zip" required class="field text-sm";
                }
                button type="submit" class="btn" { (icon(icons::UPLOAD_SIMPLE, "h-5 w-5")) "Import" }
            }
        },
    )
}
