//! Choosing a theme: for yourself, or as the team's default.

use maud::{Markup, html};

use super::{Shell, form_error, panel_page};
use crate::themes::{Appearance, Palette, THEMES, Theme};

/// A small drawing of the app in a palette: sidebar, messages, accent.
fn preview(palette: &Palette, dark: bool) -> Markup {
    let (background, text, soft) = if dark {
        (palette.night, palette.haint_2, palette.night_line)
    } else {
        (palette.white, palette.ink, palette.line)
    };
    html! {
        span class="flex h-14 flex-1 overflow-hidden rounded-md border" style={ "border-color:" (soft) } aria-hidden="true" {
            span class="flex w-1/3 flex-col gap-1 p-1.5" style={ "background:" (palette.floor) } {
                span class="h-1.5 w-3/4 rounded-full" style={ "background:" (palette.haint_2) } {}
                span class="h-1.5 w-full rounded-full" style={ "background:" (palette.floor_3) } {}
                span class="h-1.5 w-2/3 rounded-full" style={ "background:" (palette.haint) } {}
            }
            span class="relative flex flex-1 flex-col gap-1 p-1.5" style={ "background:" (background) } {
                span class="h-1.5 w-2/3 rounded-full" style={ "background:" (text) } {}
                span class="h-1.5 w-full rounded-full" style={ "background:" (soft) } {}
                span class="h-1.5 w-1/2 rounded-full" style={ "background:" (soft) } {}
                span class="absolute right-1.5 top-1.5 h-2 w-2 rounded-full" style={ "background:" (palette.lamp) } {}
            }
        }
    }
}

fn theme_card(theme: &Theme, chosen: bool) -> Markup {
    html! {
        label data-theme-card class="block cursor-pointer rounded-xl border-2 border-transparent p-2 hover:bg-screen dark:hover:bg-night-2" {
            input type="radio" name="theme" value=(theme.id) checked[chosen] class="sr-only"
                data-light=(theme.light.is_some()) data-dark=(theme.dark.is_some());
            span class="flex gap-1.5" {
                @if let Some(light) = &theme.light { (preview(light, false)) }
                @if let Some(dark) = &theme.dark { (preview(dark, true)) }
            }
            span class="mt-1.5 block text-sm font-semibold" {
                (theme.name)
                @match (theme.light.is_some(), theme.dark.is_some()) {
                    (true, false) => { span class="font-normal text-muted dark:text-haint" { " · light only" } }
                    (false, true) => { span class="font-normal text-muted dark:text-haint" { " · dark only" } }
                    _ => {}
                }
            }
        }
    }
}

/// What someone is choosing for: themselves, where empty means the team's
/// default, or the team.
pub struct Picker<'a> {
    pub action: &'a str,
    /// The chosen theme, or empty for the default.
    pub theme: &'a str,
    pub appearance: &'a str,
    /// For people: the team's default, offered as its own choice.
    pub default: Option<(&'a str, Appearance)>,
}

fn picker(picker: &Picker<'_>) -> Markup {
    let modes = [
        ("system", "Match my system"),
        ("light", "Light"),
        ("dark", "Dark"),
    ];
    html! {
        form method="post" action=(picker.action) data-theme-picker class="space-y-6" {
            fieldset {
                legend class="field-label mb-2" { "Theme" }
                @if let Some((default, _)) = picker.default {
                    label class="mb-3 flex items-center gap-2" {
                        input type="radio" name="theme" value="" checked[picker.theme.is_empty()];
                        "The team's default ("
                        (crate::themes::find(default).map_or("Sideporch", |theme| theme.name))
                        ")"
                    }
                }
                div class="grid grid-cols-2 gap-2 sm:grid-cols-3" {
                    @for theme in THEMES {
                        (theme_card(theme, theme.id == picker.theme))
                    }
                }
            }
            fieldset {
                legend class="field-label mb-2" { "Light or dark" }
                div class="flex flex-wrap gap-4" {
                    @if let Some((_, mode)) = picker.default {
                        label class="flex items-center gap-2" {
                            input type="radio" name="appearance" value="" checked[picker.appearance.is_empty()];
                            "The team's default (" (match mode { Appearance::System => "match the system", Appearance::Light => "light", Appearance::Dark => "dark" }) ")"
                        }
                    }
                    @for (value, label) in modes {
                        label class="flex items-center gap-2" {
                            input type="radio" name="appearance" value=(value) checked[picker.appearance == value];
                            (label)
                        }
                    }
                }
                p class="mt-1 text-sm text-muted dark:text-haint" { "Themes that only come in light or dark stay that way." }
            }
            button type="submit" class="btn" { "Save" }
        }
    }
}

pub fn appearance_page(
    shell: &Shell<'_>,
    picker_state: &Picker<'_>,
    error: Option<&str>,
    saved: bool,
) -> Markup {
    panel_page(
        "Appearance",
        shell,
        &html! { "Appearance" },
        &html! {
            (form_error(error))
            @if saved {
                p role="status" class="mb-4 rounded-lg border border-line bg-haint-2 px-3 py-2 text-sm text-floor dark:border-night-line dark:bg-floor-2 dark:text-haint-2" {
                    "Saved."
                }
            }
            p class="mb-5 max-w-xl text-muted dark:text-haint" { "Only you see your theme. Click one to try it." }
            (picker(picker_state))
        },
    )
}

/// The admin's page for everyone's default.
pub fn default_page(
    shell: &Shell<'_>,
    tabs: &Markup,
    picker_state: &Picker<'_>,
    saved: bool,
) -> Markup {
    panel_page(
        "Appearance",
        shell,
        &html! { "Appearance" },
        &html! {
            (tabs)
            @if saved {
                p role="status" class="mb-4 rounded-lg border border-line bg-haint-2 px-3 py-2 text-sm text-floor dark:border-night-line dark:bg-floor-2 dark:text-haint-2" {
                    "Saved."
                }
            }
            p class="mb-5 max-w-xl text-muted dark:text-haint" {
                "The theme everyone starts with. People can pick their own under Appearance in their account menu."
            }
            (picker(picker_state))
        },
    )
}
