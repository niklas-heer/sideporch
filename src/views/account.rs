//! Account pages: your password, reset links, and resetting a password.

use maud::{Markup, html};

use super::{Shell, auth_page, copy_row, form_error, panel_page, text_field};
use crate::{
    icons::{self, icon},
    store::User,
};

pub fn account_page(shell: &Shell<'_>, error: Option<&str>, saved: bool) -> Markup {
    panel_page(
        "Account",
        shell,
        &html! { "Your account" },
        &html! {
            (form_error(error))
            @if saved {
                p role="status" class="mb-4 rounded-lg border border-line bg-haint-2 px-3 py-2 text-sm text-floor dark:border-night-line dark:bg-floor-2 dark:text-haint-2" {
                    "Password changed. Your other devices are signed out."
                }
            }
            h2 class="mb-1 text-lg font-bold" { "Change your password" }
            p class="mb-4 text-muted dark:text-haint" { "Changing it signs you out on your other devices." }
            form method="post" action="/settings/account" class="max-w-md" {
                (text_field("Current password", "current", "password", "", "current-password", None))
                (text_field("New password", "password", "password", "", "new-password", Some("At least 8 characters.")))
                button type="submit" class="btn" { (icon(icons::KEY, "h-5 w-5")) "Change password" }
            }
            p class="mt-8 text-sm text-muted dark:text-haint" {
                "Forgot it? Ask an admin for a reset link. "
                a href="/settings/profile" class="underline" { "Edit your profile" }
            }
        },
    )
}

pub fn reset_page(token: &str, person: &User, error: Option<&str>) -> Markup {
    auth_page(
        "Reset password",
        &html! {
            h1 class="mb-2 text-xl font-bold" { "Choose a new password" }
            p class="mb-5 text-muted dark:text-haint" {
                "For " (person.display_name) " (@" (person.username) "). This signs you out everywhere else."
            }
            (form_error(error))
            form method="post" action={ "/reset/" (token) } {
                (text_field("New password", "password", "password", "", "new-password", Some("At least 8 characters.")))
                button type="submit" class="btn mt-2 w-full" { "Save and sign in" }
            }
        },
    )
}

pub fn reset_link_page(shell: &Shell<'_>, person: &User, link: &str) -> Markup {
    panel_page(
        "Reset link",
        shell,
        &html! { "Password reset link" },
        &html! {
            p class="mb-4 max-w-xl text-muted dark:text-haint" {
                "Send this link to " (person.display_name) ". It works once, for 24 hours, and lets them choose a new password. "
                "It is shown only now; create another one if it gets lost."
            }
            (copy_row(link))
            a href={ "/people/" (person.id) } class="btn-quiet mt-6" { (icon(icons::ARROW_LEFT, "h-4 w-4")) "Back to " (person.display_name) }
        },
    )
}
