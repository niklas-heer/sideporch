//! The team's GIF library.

use maud::{Markup, html};

use super::{Shell, form_error, panel_page};
use crate::{
    icons::{self, icon},
    store::LibraryGif,
};

pub fn library_page(shell: &Shell<'_>, gifs: &[LibraryGif], error: Option<&str>) -> Markup {
    panel_page(
        "GIF library",
        shell,
        &html! { "GIF library" },
        &html! {
            p class="mb-5 max-w-xl text-muted dark:text-haint" {
                "GIFs everyone on the team can send from the composer's GIF button. "
                "Give each a title and a few tags, so the picker's search finds it."
            }
            (form_error(error))
            @if shell.user.may(crate::community::Permission::UploadFiles) {
            form method="post" action="/gifs/library" enctype="multipart/form-data"
                class="mb-8 grid max-w-2xl gap-3 sm:grid-cols-[1fr_1fr_auto] sm:items-end" {
                div class="sm:col-span-3" {
                    label for="gif-file" class="field-label" { "GIF or image" }
                    input id="gif-file" name="file" type="file" required accept="image/gif,image/webp,image/png,image/jpeg" class="field text-sm";
                }
                div {
                    label for="gif-title" class="field-label" { "Title" }
                    input id="gif-title" name="title" required maxlength="100" placeholder="Happy dance" class="field";
                }
                div {
                    label for="gif-tags" class="field-label" { "Tags" }
                    input id="gif-tags" name="tags" maxlength="200" placeholder="yay party dance" class="field";
                }
                button type="submit" class="btn" { (icon(icons::GIF, "h-5 w-5")) "Add" }
            }
            }
            @if gifs.is_empty() {
                p class="text-muted dark:text-haint" { "No GIFs yet. Add the first one above." }
            } @else {
                ul class="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-4" {
                    @for gif in gifs {
                        li class="overflow-hidden rounded-lg border border-line dark:border-night-line" {
                            img src={ "/files/" (gif.file_id) } alt=(gif.title) loading="lazy"
                                class="block aspect-square w-full bg-screen object-cover dark:bg-night-2";
                            div class="flex items-start justify-between gap-2 p-2 text-sm" {
                                div class="min-w-0" {
                                    p class="truncate font-semibold" { (gif.title) }
                                    @if !gif.tags.is_empty() {
                                        p class="truncate text-xs text-muted dark:text-haint" { (gif.tags) }
                                    }
                                    p class="text-xs text-muted dark:text-haint" {
                                        @if let Some(adder) = &gif.adder { "by " (adder) " · " }
                                        (gif.uses) @if gif.uses == 1 { " use" } @else { " uses" }
                                    }
                                }
                                @if shell.user.is_admin || gif.added_by == Some(shell.user.id) {
                                    form method="post" action={ "/gifs/library/" (gif.id) "/delete" } {
                                        button type="submit" class="btn-quiet p-1" title="Remove from the library" aria-label={ "Remove " (gif.title) } {
                                            (icon(icons::TRASH, "h-4 w-4"))
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        },
    )
}
