//! Custom emoji management.

use maud::{Markup, html};

use super::{Shell, form_error, panel_page, section};
use crate::{
    icons::{self, icon},
    store::CustomEmoji,
};

pub fn page(shell: &Shell<'_>, emoji: &[CustomEmoji], error: Option<&str>) -> Markup {
    panel_page(
        "Custom emoji",
        shell,
        &html! { "Custom emoji" },
        &html! {
            @if shell.user.may(crate::community::Permission::AddEmoji) {
            (section("Add an emoji", "Upload a small square image. Everyone can then use it as :name: in messages and reactions.", &html! {
                (form_error(error))
                form method="post" action="/emoji" enctype="multipart/form-data" class="flex flex-wrap items-end gap-3" {
                    div class="min-w-40 flex-1" {
                        label for="emoji-name" class="field-label" { "Name" }
                        input id="emoji-name" name="name" required maxlength="32" pattern="[a-z0-9_+\\-]+" placeholder="partyporch" class="field";
                    }
                    div class="min-w-40 flex-1" {
                        label for="emoji-image" class="field-label" { "Image" }
                        input id="emoji-image" name="image" type="file" required accept="image/png,image/jpeg,image/gif,image/webp"
                            class="block w-full text-sm file:mr-3 file:rounded-lg file:border-0 file:bg-haint-2 file:px-3 file:py-2 file:font-semibold file:text-floor";
                    }
                    button type="submit" class="btn" { (icon(icons::PLUS, "h-5 w-5")) "Add emoji" }
                }
            }))
            }
            (section("This porch's emoji", "The person who added an emoji, or an admin, can remove it.", &html! {
                @if emoji.is_empty() {
                    p class="text-muted dark:text-haint" { "No custom emoji yet." }
                } @else {
                    ul class="grid grid-cols-1 gap-2 sm:grid-cols-2" {
                        @for item in emoji {
                            li class="flex items-center gap-3 rounded-xl border border-line px-3 py-2 dark:border-night-line" {
                                img src={ "/files/" (item.file_id) } alt="" class="h-8 w-8 object-contain";
                                div class="min-w-0 flex-1" {
                                    p class="truncate font-semibold" { ":" (item.name) ":" }
                                    @if let Some(creator) = &item.creator {
                                        p class="truncate text-xs text-muted dark:text-haint" { "Added by " (creator) }
                                    }
                                }
                                @if shell.user.is_admin || item.created_by == Some(shell.user.id) {
                                    form method="post" action={ "/emoji/" (item.name) "/delete" } {
                                        button type="submit" class="btn-quiet text-sm" aria-label={ "Remove :" (item.name) ":" } {
                                            (icon(icons::TRASH, "h-4 w-4"))
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }))
        },
    )
}
