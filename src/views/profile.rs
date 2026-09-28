//! Profile pages and profile settings.

use maud::{Markup, PreEscaped, html};

use super::{Shell, form_error, panel_page, timestamp_date};
use crate::{
    icons::{self, icon},
    markdown,
    markup::{self, Context},
    store::User,
};

fn big_avatar(user: &User) -> Markup {
    html! {
        @if let Some(file_id) = user.avatar_file_id {
            img src={ "/files/" (file_id) } alt="" class="h-24 w-24 rounded-2xl bg-screen object-cover dark:bg-night-2";
        } @else {
            div class="flex h-24 w-24 items-center justify-center rounded-2xl bg-floor-3 text-4xl font-bold text-white" aria-hidden="true" {
                (user.display_name.chars().next().unwrap_or('?').to_uppercase().collect::<String>())
            }
        }
    }
}

fn status(user: &User, ctx: &Context) -> Markup {
    html! {
        @if !user.status_emoji.is_empty() || !user.status_text.is_empty() {
            p class="mt-2 flex items-center gap-2" {
                @if !user.status_emoji.is_empty() {
                    span class="text-xl" { (PreEscaped(markup::render(&user.status_emoji, ctx))) }
                }
                span { (user.status_text) }
            }
        }
    }
}

pub fn profile_page(shell: &Shell<'_>, user: &User, ctx: &Context) -> Markup {
    let own = user.id == shell.user.id;
    panel_page(
        &user.display_name,
        shell,
        &html! { (user.display_name) },
        &html! {
            div class="flex flex-wrap items-start gap-5" {
                (big_avatar(user))
                div class="min-w-0 flex-1" {
                    h2 class="text-2xl font-bold" {
                        (user.display_name)
                        @if user.is_admin {
                            span class="ml-2 align-middle rounded bg-haint-2 px-1.5 text-xs font-semibold text-floor dark:bg-floor-2 dark:text-haint-2" { "Admin" }
                        }
                    }
                    p class="text-muted dark:text-haint" { "@" (user.username) }
                    (status(user, ctx))
                    div class="mt-4 flex flex-wrap gap-2" {
                        a href={ "/dm/" (user.id) } class="btn" {
                            (icon(icons::CHAT_CIRCLE_TEXT, "h-4 w-4"))
                            @if own { "Notes to self" } @else { "Message" }
                        }
                        @if own {
                            a href="/settings/profile" class="btn-quiet" { "Edit profile" }
                            a href="/settings/account" class="btn-quiet" { (icon(icons::KEY, "h-4 w-4")) "Password" }
                        }
                    }
                    @if user.deactivated {
                        p class="mt-3 rounded-lg bg-screen px-3 py-2 text-sm dark:bg-night-2" { "This account is deactivated." }
                    }
                }
            }
            @if shell.user.is_admin && !own {
                (admin_controls(user))
            }
            @if !user.bio.is_empty() {
                div class="rich mt-8 max-w-prose" { (PreEscaped(markdown::render(&user.bio, ctx))) }
            }
            @if !user.links.is_empty() {
                ul class="mt-6 space-y-2" {
                    @for link in &user.links {
                        li class="flex items-center gap-2" {
                            (icon(icons::LINK_SIMPLE, "h-4 w-4 shrink-0 text-muted dark:text-haint"))
                            a href=(link.url) target="_blank" rel="noopener noreferrer nofollow me" class="underline" { (link.label) }
                        }
                    }
                }
            }
            p class="mt-8 text-sm text-muted dark:text-haint" { "Joined " (timestamp_date(user.created_at)) }
        },
    )
}

/// What an admin can do about someone else's account.
fn admin_controls(user: &User) -> Markup {
    let base = format!("/people/{}", user.id);
    html! {
        section class="mt-10 rounded-xl border border-line p-4 dark:border-night-line" {
            h2 class="mb-3 font-bold" { "Admin" }
            div class="flex flex-wrap gap-2" {
                @if user.deactivated {
                    form method="post" action={ (base) "/reactivate" } {
                        button type="submit" class="btn-quiet" { "Reactivate account" }
                    }
                } @else {
                    form method="post" action={ (base) "/reset-link" } {
                        button type="submit" class="btn-quiet" { (icon(icons::KEY, "h-4 w-4")) "Create password reset link" }
                    }
                    form method="post" action={ (base) "/admin" } {
                        input type="hidden" name="admin" value=(if user.is_admin { "false" } else { "true" });
                        button type="submit" class="btn-quiet" {
                            @if user.is_admin { "Remove admin rights" } @else { "Make admin" }
                        }
                    }
                    form method="post" action={ (base) "/deactivate" } {
                        button type="submit" class="btn-quiet text-red-700 dark:text-red-300" { "Deactivate account" }
                    }
                }
            }
            p class="mt-3 text-sm text-muted dark:text-haint" {
                "Deactivating signs them out everywhere and stops their notifications. Their messages stay."
            }
        }
    }
}

/// The favorites shown first in the reaction picker, or the most used.
fn favorites_preview(names: &[String], ctx: &Context) -> Markup {
    html! {
        span class="flex flex-wrap gap-1 text-xl" {
            @for name in names {
                span title={ ":" (name) ":" } { (PreEscaped(ctx.emoji_html(name).unwrap_or_default())) }
            }
        }
    }
}

pub fn edit_page(
    shell: &Shell<'_>,
    user: &User,
    ctx: &Context,
    most_used: &[String],
    error: Option<&str>,
    saved: bool,
) -> Markup {
    let status_emoji = user.status_emoji.trim_matches(':');
    panel_page(
        "Your profile",
        shell,
        &html! { "Your profile" },
        &html! {
            (form_error(error))
            @if saved {
                p role="status" class="mb-4 rounded-lg border border-line bg-haint-2 px-3 py-2 text-sm text-floor dark:border-night-line dark:bg-floor-2 dark:text-haint-2" {
                    "Saved. " a href={ "/people/" (user.id) } class="underline" { "See your profile" }
                }
            }
            form method="post" action="/settings/profile" enctype="multipart/form-data" class="max-w-xl space-y-6" {
                div class="flex items-center gap-4" {
                    (big_avatar(user))
                    div class="space-y-2" {
                        label for="avatar" class="field-label" { "Profile picture" }
                        input id="avatar" name="avatar" type="file" accept="image/png,image/jpeg,image/gif,image/webp" class="text-sm";
                        p class="text-sm text-muted dark:text-haint" { "PNG, JPEG, GIF or WebP, up to 2 MB. Square pictures look best." }
                        @if user.avatar_file_id.is_some() {
                            label class="flex items-center gap-2 text-sm" {
                                input type="checkbox" name="remove_avatar" value="on" class="h-4 w-4 accent-floor";
                                "Remove the picture"
                            }
                        }
                    }
                }
                div {
                    label for="display_name" class="field-label" { "Display name" }
                    input id="display_name" name="display_name" required maxlength="60" class="field" value=(user.display_name);
                }
                div {
                    span class="field-label" { "Status" }
                    div class="flex gap-2" {
                        label for="status_emoji" class="sr-only" { "Status emoji" }
                        input id="status_emoji" name="status_emoji" maxlength="40" class="field w-40 font-mono text-sm"
                            placeholder="coffee" value=(status_emoji);
                        label for="status_text" class="sr-only" { "Status text" }
                        input id="status_text" name="status_text" maxlength="100" class="field min-w-0 flex-1"
                            placeholder="Out for lunch" value=(user.status_text);
                    }
                    p class="mt-1 text-sm text-muted dark:text-haint" { "The emoji's name, like coffee or palm_tree. It shows next to your name in chat." }
                }
                div {
                    label for="bio" class="field-label" { "Bio" }
                    textarea id="bio" name="bio" rows="4" maxlength="1000" class="field" placeholder="What you do, what you're into." { (user.bio) }
                    p class="mt-1 text-sm text-muted dark:text-haint" { "Markdown works." }
                }
                fieldset {
                    legend class="field-label" { "Links" }
                    div class="space-y-2" {
                        @for index in 0..5 {
                            @let link = user.links.get(index);
                            div class="flex gap-2" {
                                input name="link_label" maxlength="40" class="field w-40" placeholder="Label"
                                    aria-label={ "Label of link " (index + 1) } value=[link.map(|link| link.label.as_str())];
                                input name="link_url" type="url" class="field min-w-0 flex-1" placeholder="https://"
                                    aria-label={ "Address of link " (index + 1) } value=[link.map(|link| link.url.as_str())];
                            }
                        }
                    }
                }
                div {
                    label for="favorite_emoji" class="field-label" { "Favorite emoji" }
                    input id="favorite_emoji" name="favorite_emoji" class="field font-mono text-sm"
                        placeholder="+1 heart tada eyes" value=(user.favorite_emoji.join(" "));
                    p class="mt-1 text-sm text-muted dark:text-haint" {
                        "Up to 12 emoji names, shown first when you react. Left empty, the ones you use most come first"
                        @if !most_used.is_empty() { ":" }
                    }
                    @if user.favorite_emoji.is_empty() && !most_used.is_empty() {
                        div class="mt-2" { (favorites_preview(most_used, ctx)) }
                    } @else if !user.favorite_emoji.is_empty() {
                        div class="mt-2" { (favorites_preview(&user.favorite_emoji, ctx)) }
                    }
                }
                button type="submit" class="btn" { "Save profile" }
            }
        },
    )
}
