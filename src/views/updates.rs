//! Admin → Updates, and the reminder admins see in the sidebar.

use maud::{Markup, html};

use super::{Shell, admin::tabs, panel_page};
use crate::updates::{AutoInstall, Method, Notice, Settings, Status, Urgency, Version};

pub struct UpdatesView<'a> {
    pub status: &'a Status,
    pub settings: Settings,
    pub method: Method,
    pub program: Option<&'a std::path::Path>,
    /// Why Sideporch can't replace itself here, if it can't.
    pub cannot_install: Option<&'a str>,
    /// Checking is allowed by the environment.
    pub allowed: bool,
    /// Sideporch restarts into a new version by itself.
    pub restarts: bool,
}

const fn method_label(method: Method) -> &'static str {
    match method {
        Method::Archive => "installed from a release archive",
        Method::Homebrew => "installed with Homebrew",
        Method::Nix => "installed with Nix",
        Method::Container => "running in a container",
    }
}

pub fn updates_page(shell: &Shell<'_>, view: &UpdatesView<'_>) -> Markup {
    let current = Version::current();
    panel_page(
        "Updates",
        shell,
        &html! { "Updates" },
        &html! {
            (tabs("/admin/updates"))
            (super::section("This server", "", &html! {
                p {
                    "Sideporch " strong { (current) } ", " (method_label(view.method))
                    @if let Some(program) = view.program { " at " code class="break-all" { (program.display()) } }
                    "."
                }
            }))
            (releases(view))
            (settings(view))
            (super::section("How updates are checked", "", &html! {
                p class="max-w-xl" {
                    "Releases publish their checksums signed with Sideporch's release key, which is built into this program. Sideporch installs a release only when the signature and the download's checksum both match, and keeps the program it replaces next to it as "
                    code { "sideporch.previous" } "."
                }
            }))
        },
    )
}

fn releases(view: &UpdatesView<'_>) -> Markup {
    let status = view.status;
    let current = Version::current();
    let latest = status.newer.first();
    html! {
        (super::section("New releases", "", &html! {
            @if let Some(version) = status.installed {
                p class="mb-3 rounded-lg bg-haint-2 px-3 py-2 text-floor" {
                    "Sideporch " strong { (version) } " is installed. "
                    @if view.restarts { "Sideporch restarts into it by itself; reload this page in a moment. If it still says " (current) ", restart Sideporch." }
                    @else { "Restart Sideporch to use it." }
                }
            }
            @if let Some(version) = status.installing {
                p class="mb-3" { "Installing Sideporch " (version) "…" }
            }
            @if let (Some(error), None) = (&status.install_error, status.installed) {
                p class="mb-3 text-red-700 dark:text-red-300" { "The update wasn't installed: " (error) }
            }
            @if !view.allowed {
                p class="mb-3 text-muted dark:text-haint" {
                    "Checking is turned off on this server (" code { "SIDEPORCH_UPDATE_CHECK=false" } "), so Sideporch doesn't contact GitHub."
                }
            } @else if let Some(error) = &status.error {
                p class="mb-3 text-red-700 dark:text-red-300" { "The last check didn't work: " (error) }
            } @else if status.checked_at.is_none() {
                p class="mb-3 text-muted dark:text-haint" { "Sideporch hasn't checked yet." }
            } @else if latest.is_none() {
                p class="mb-3" { "This is the newest release." }
            }
            @if !status.newer.is_empty() {
                ul class="mb-4" {
                    @for release in &status.newer {
                        li class="flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-line py-2 last:border-b-0 dark:border-night-line" {
                            strong { "Sideporch " (release.version) }
                            @if release.security {
                                span class="rounded bg-red-100 px-1.5 text-xs font-bold text-red-800 dark:bg-red-950 dark:text-red-200" { "Security" }
                            }
                            span class="text-sm text-muted dark:text-haint" { (super::timestamp_date(release.published_at)) }
                            a href=(release.url) class="ml-auto text-sm underline" { "What changed" }
                        }
                    }
                }
            }
            div class="flex flex-wrap items-center gap-3" {
                @if let (Some(release), None, None) = (latest, view.cannot_install, status.installed) {
                    form method="post" action="/admin/updates/install" {
                        button type="submit" class="btn" { "Update now to " (release.version) }
                    }
                }
                @if view.allowed {
                    form method="post" action="/admin/updates/check" {
                        button type="submit" class="btn-quiet" { "Check now" }
                    }
                }
                @if let Some(at) = status.checked_at {
                    span class="text-sm text-muted dark:text-haint" { "Checked " (super::timestamp_date(at)) }
                }
            }
            @if let (Some(reason), Some(_)) = (view.cannot_install, latest) {
                p class="mt-3 max-w-xl" { (reason) " " (view.method.advice()) }
            }
        }))
    }
}

fn settings(view: &UpdatesView<'_>) -> Markup {
    html! {
        (super::section("Settings", "", &html! {
            form method="post" action="/admin/updates/settings" class="max-w-xl space-y-4" {
                label class="flex items-start gap-2" {
                    input type="checkbox" name="check" value="on" checked[view.settings.check] class="mt-1";
                    span {
                        span class="block font-semibold" { "Check for new releases" }
                        span class="block text-sm text-muted dark:text-haint" { "Every six hours Sideporch asks GitHub which releases exist. Nothing about this server is sent." }
                    }
                }
                fieldset {
                    legend class="font-semibold" { "Install by itself" }
                    @if let Some(reason) = view.cannot_install {
                        p class="text-sm text-muted dark:text-haint" { (reason) " These settings take effect once it can." }
                    }
                    @for (value, label, hint) in [
                        (AutoInstall::Off, "Nothing", "Admins install updates."),
                        (AutoInstall::Security, "Security fixes", "Releases that fix security problems install as soon as Sideporch hears of them."),
                        (AutoInstall::All, "Every release", "Security fixes right away, everything else between 3 and 5 in the morning."),
                    ] {
                        label class="mt-2 flex items-start gap-2" {
                            input type="radio" name="install" value=(value.key()) checked[view.settings.install == value] class="mt-1";
                            span {
                                span class="block" { (label) }
                                span class="block text-sm text-muted dark:text-haint" { (hint) }
                            }
                        }
                    }
                }
                button type="submit" class="btn" { "Save" }
            }
        }))
    }
}

/// The reminder at the bottom of an admin's sidebar.
pub fn notice(notice: &Notice) -> Markup {
    let current = Version::current();
    let (tone, title, text) = match notice.urgency {
        Urgency::Security => (
            "bg-red-900 text-red-50",
            format!("Sideporch {} fixes security problems", notice.latest),
            format!("This server runs {current}. Update soon."),
        ),
        Urgency::Overdue => (
            "bg-lamp text-floor",
            "This Sideporch is out of date".to_owned(),
            format!(
                "It runs {current}; {} is out, and updates have waited for over two months.",
                notice.latest
            ),
        ),
        Urgency::Remind | Urgency::Available => (
            "bg-floor-2 text-haint-2",
            format!("Sideporch {} is out", notice.latest),
            format!("This server runs {current}."),
        ),
    };
    html! {
        div data-update-notice class={ "mx-3 mb-3 rounded-xl p-3 text-sm " (tone) } {
            p class="font-bold" { (title) }
            p class="mt-0.5" { (text) }
            div class="mt-2 flex items-center gap-2" {
                a href="/admin/updates" class="rounded-lg bg-white px-3 py-1 font-semibold text-floor" { "See the update" }
                form method="post" action="/updates/hide" {
                    button type="submit" class="rounded-lg px-2 py-1 font-semibold opacity-80 hover:opacity-100" {
                        @if notice.urgency.hide_for_days() == Some(1) { "Tomorrow" } @else { "Later" }
                    }
                }
            }
        }
    }
}
