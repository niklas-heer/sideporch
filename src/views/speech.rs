//! The admin's page for speech models, and each person's reading voice.

use maud::{Markup, html};

use super::{Shell, admin::tabs, panel_page, section};
use crate::{
    icons::{self, icon},
    speech::{Settings, Speech, models, of_kind, tts},
};

fn megabytes(bytes: u64) -> String {
    format!("{} MB", bytes.div_ceil(1024 * 1024))
}

fn model_card(speech: &Speech, model: &'static models::Model, chosen: bool) -> Markup {
    let installed = speech.installed(model);
    let progress = speech.progress(model).unwrap_or_default();
    html! {
        li class="rounded-xl border border-line p-4 dark:border-night-line" data-model=(model.key) {
            div class="flex flex-wrap items-baseline justify-between gap-2" {
                p class="font-bold" {
                    (model.name)
                    @if chosen { span class="ml-2 rounded bg-haint-2 px-1.5 text-xs font-semibold text-floor dark:bg-floor-2 dark:text-haint-2" { "In use" } }
                }
                span class="text-sm text-muted dark:text-haint" {
                    (megabytes(model.size())) " download, about " (model.memory_mb) " MB memory while in use"
                }
            }
            p class="mt-1 text-sm" { (model.description) }
            p class="mt-1 text-xs text-muted dark:text-haint" {
                "License: " a href=(model.license_url) class="underline underline-offset-2" rel="noopener noreferrer" target="_blank" { (model.license) }
            }
            div class="mt-3 flex flex-wrap items-center gap-2" {
                @if installed {
                    span class="flex items-center gap-1.5 text-sm font-semibold" { (icon(icons::SHIELD_CHECK, "h-4 w-4 text-floor-3 dark:text-haint")) "Installed" }
                    form method="post" action={ "/admin/speech/models/" (model.key) "/remove" } {
                        button type="submit" class="btn-quiet px-3 py-1 text-sm" { "Remove" }
                    }
                } @else if progress.running {
                    progress class="h-2 w-48" max=(model.size()) value=(progress.received) data-progress {}
                    span class="text-sm text-muted dark:text-haint" data-progress-text {
                        "Downloading, " (megabytes(progress.received)) " of " (megabytes(model.size()))
                    }
                } @else {
                    form method="post" action={ "/admin/speech/models/" (model.key) "/install" } {
                        button type="submit" class="btn px-3 py-1 text-sm" { (icon(icons::DOWNLOAD_SIMPLE, "h-4 w-4")) "Download" }
                    }
                    @if let Some(error) = &progress.error {
                        span class="text-sm text-red-700 dark:text-red-300" role="alert" { "The download failed: " (error) }
                    }
                }
            }
        }
    }
}

pub fn admin_page(shell: &Shell<'_>, speech: &Speech, settings: &Settings) -> Markup {
    let voice_models: Vec<&'static models::Model> = of_kind(models::Kind::Voice).collect();
    let dictation_models: Vec<&'static models::Model> = of_kind(models::Kind::Dictation).collect();
    let downloading = models::MODELS.iter().any(|model| {
        speech
            .progress(model)
            .is_some_and(|progress| progress.running)
    });
    panel_page(
        "Speech",
        shell,
        &html! { "Speech" },
        &html! {
            (tabs("/admin/speech"))
            p class="mb-6 max-w-xl text-sm text-muted dark:text-haint" {
                "Sideporch can read messages aloud and take dictation with models that run on this server's processor. "
                "Nothing leaves the server. Without a voice model, people's own devices read aloud instead."
            }
            div data-speech-admin data-downloading[downloading] {
                (section("Reading aloud", "People choose \u{201c}Read aloud\u{201d} on a message.", &html! {
                    ul class="space-y-3" {
                        @for model in &voice_models {
                            (model_card(speech, model, settings.voice_model.is_some_and(|chosen| chosen.key == model.key)))
                        }
                    }
                }))
                (section("Dictation", "A microphone button in the composer turns speech into text. Pick one model; bigger ones are more accurate and slower.", &html! {
                    ul class="space-y-3" {
                        @for model in &dictation_models {
                            (model_card(speech, model, settings.dictation_model.is_some_and(|chosen| chosen.key == model.key)))
                        }
                    }
                }))
            }
            (section("Choices", "", &html! {
                form method="post" action="/admin/speech" class="max-w-md space-y-4" {
                    label class="block" {
                        span class="field-label" { "Read aloud with" }
                        select name="voice_model" class="field" {
                            option value="" selected[settings.voice_model.is_none()] { "People's own devices" }
                            @for model in voice_models.iter().filter(|model| speech.installed(model)) {
                                option value=(model.key) selected[settings.voice_model.is_some_and(|chosen| chosen.key == model.key)] { (model.name) }
                            }
                        }
                    }
                    label class="block" {
                        span class="field-label" { "Voice everyone hears unless they choose another" }
                        select name="voice" class="field" {
                            @for (key, label) in tts::VOICES {
                                option value=(key) selected[settings.voice == *key] { (label) }
                            }
                        }
                    }
                    label class="block" {
                        span class="field-label" { "Dictate with" }
                        select name="dictation_model" class="field" {
                            option value="" selected[settings.dictation_model.is_none()] { "Off" }
                            @for model in dictation_models.iter().filter(|model| speech.installed(model)) {
                                option value=(model.key) selected[settings.dictation_model.is_some_and(|chosen| chosen.key == model.key)] { (model.name) }
                            }
                        }
                    }
                    button type="submit" class="btn" { "Save" }
                }
                @if settings.voice_model.is_some() {
                    form method="get" action="/admin/speech/try" target="_blank" class="mt-6 flex flex-wrap items-end gap-2" data-try-voice {
                        label class="min-w-60 flex-1" {
                            span class="field-label" { "Hear it" }
                            input name="text" class="field" value="Hello! This is how messages sound when Sideporch reads them aloud.";
                        }
                        button type="submit" class="btn-quiet" { (icon(icons::SPEAKER_HIGH, "h-4 w-4")) "Play" }
                    }
                }
                p class="mt-4 text-sm text-muted dark:text-haint" {
                    "Models take " (megabytes(speech.disk_usage())) " in the data directory's models/ folder. "
                    "They load when first used and leave memory after 15 minutes unused."
                }
            }))
        },
    )
}

/// How someone likes messages read aloud, on their appearance page.
pub fn personal_section(voice: &str, speed: f64) -> Markup {
    let speeds = [
        (0.8, "Slower"),
        (1.0, "Normal"),
        (1.2, "Faster"),
        (1.5, "Much faster"),
    ];
    html! {
        section class="mt-10 max-w-md" {
            h2 class="mb-1 text-lg font-bold" { "Reading aloud" }
            p class="mb-3 text-sm text-muted dark:text-haint" { "How messages sound when you choose \u{201c}Read aloud\u{201d}." }
            form method="post" action="/settings/speech" class="space-y-3" {
                label class="block" {
                    span class="field-label" { "Voice" }
                    select name="voice" class="field" {
                        option value="" selected[voice.is_empty()] { "The one everyone hears" }
                        @for (key, label) in tts::VOICES {
                            option value=(key) selected[voice == *key] { (label) }
                        }
                    }
                }
                label class="block" {
                    span class="field-label" { "Speed" }
                    select name="speed" class="field" {
                        @for (value, label) in speeds {
                            option value=(value) selected[(speed - value).abs() < 0.01] { (label) }
                        }
                    }
                }
                button type="submit" class="btn" { "Save" }
            }
        }
    }
}
