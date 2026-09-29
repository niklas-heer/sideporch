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

/// Someone's roles and trust level, and what an admin can change.
pub struct Standing {
    pub roles: Vec<String>,
    pub role_ids: Vec<i64>,
    pub all_roles: Vec<crate::community::Role>,
    pub progress: crate::community::Progress,
    pub timed_out_until: Option<i64>,
    /// For moderators: the addresses they used lately.
    pub addresses: Vec<crate::access::Address>,
    /// For moderators: whether they have an email address.
    pub has_email: bool,
}

pub fn profile_page(shell: &Shell<'_>, user: &User, ctx: &Context, standing: &Standing) -> Markup {
    let own = user.id == shell.user.id;
    let moderator = shell.user.may(crate::community::Permission::Moderate);
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
                    @if let Some(server) = &user.server {
                        p class="mt-2 flex items-center gap-2 text-sm" data-server=(server) {
                            (icon(icons::GLOBE, "h-4 w-4 text-muted dark:text-haint"))
                            "On " strong { (server) } ", a connected server. Their own server's admins look after their account."
                        }
                    }
                    @if !standing.roles.is_empty() {
                        p class="mt-2 flex flex-wrap gap-1.5" {
                            @for role in &standing.roles {
                                span class="rounded-full border border-line px-2 py-0.5 text-xs font-semibold dark:border-night-line" { (role) }
                            }
                        }
                    }
                    @if own || moderator {
                        p class="mt-2 text-sm text-muted dark:text-haint" title="Trust grows as people stay and take part." {
                            "Trust level " (standing.progress.level) ": " (crate::community::level_name(standing.progress.level))
                            @if standing.progress.locked { " (set by an admin)" }
                        }
                    }
                    @if let Some(until) = standing.timed_out_until {
                        p class="mt-2 rounded-lg bg-screen px-3 py-2 text-sm dark:bg-night-2" { "Timed out until " (super::timestamp(until)) "." }
                    }
                    (status(user, ctx))
                    div class="mt-4 flex flex-wrap gap-2" {
                        a href={ "/dm/" (user.id) } class="btn" {
                            (icon(icons::CHAT_CIRCLE_TEXT, "h-4 w-4"))
                            @if own { "Notes to self" } @else { "Message" }
                        }
                        @if own {
                            a href="/settings/profile" class="btn-quiet" { "Edit profile" }
                            a href="/settings/security" class="btn-quiet" { (icon(icons::KEY, "h-4 w-4")) "Sign-in and security" }
                        }
                    }
                    @if user.deactivated {
                        p class="mt-3 rounded-lg bg-screen px-3 py-2 text-sm dark:bg-night-2" { "This account is deactivated." }
                    }
                }
            }
            @if user.server.is_some() {
            } @else if shell.user.is_admin && !own {
                (admin_controls(user, standing))
            } @else if moderator && !own && !user.is_admin {
                (timeout_controls(user, standing))
            }
            @if moderator && !own && !user.is_admin && user.server.is_none() {
                (ban_controls(user, standing))
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

/// Banning someone, and the addresses they used, for moderators.
fn ban_controls(user: &User, standing: &Standing) -> Markup {
    html! {
        section id="ban" class="mt-6 space-y-4 rounded-xl border border-red-200 p-4 dark:border-red-900" {
            h2 class="font-bold" { "Ban" }
            @if standing.addresses.is_empty() {
                p class="text-sm text-muted dark:text-haint" { "No addresses noted in the last 30 days." }
            } @else {
                div {
                    p class="field-label" { "Addresses in the last 30 days" }
                    ul class="space-y-1 text-sm" data-addresses {
                        @for seen in &standing.addresses {
                            li {
                                code class="font-mono" { (seen.ip) }
                                span class="text-muted dark:text-haint" { ", first " (super::timestamp(seen.first_seen)) ", last " (super::timestamp(seen.last_seen)) }
                                @if !seen.shared_with.is_empty() {
                                    span class="text-muted dark:text-haint" { ", also used by " }
                                    @for (index, (id, name)) in seen.shared_with.iter().enumerate() {
                                        @if index > 0 { ", " }
                                        a href={ "/people/" (id) } class="underline underline-offset-2" { (name) }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            @if user.deactivated {
                p class="text-sm text-muted dark:text-haint" { "This account is deactivated. Ban addresses and emails from Moderation." }
            } @else {
                form method="post" action={ "/people/" (user.id) "/ban" } class="space-y-3" {
                    div class="flex flex-wrap items-end gap-2" {
                        label class="min-w-0 flex-1" {
                            span class="field-label" { "Reason" }
                            input name="reason" maxlength="200" class="field" placeholder="Spam";
                        }
                        label {
                            span class="field-label" { "For" }
                            select name="duration" class="field py-1.5" {
                                @for (duration, label) in crate::routes::BAN_DURATIONS {
                                    option value=(duration) selected[duration == 0] { (label) }
                                }
                            }
                        }
                    }
                    label class="flex items-center gap-2 text-sm" {
                        input type="checkbox" name="addresses" value="on" checked[!standing.addresses.is_empty()] disabled[standing.addresses.is_empty()] class="h-4 w-4 accent-floor";
                        "Also ban the addresses above"
                    }
                    @if standing.has_email {
                        label class="flex items-center gap-2 text-sm" {
                            input type="checkbox" name="email" value="on" checked class="h-4 w-4 accent-floor";
                            "Also ban their email address"
                        }
                    }
                    label class="flex items-center gap-2 text-sm" {
                        input type="checkbox" name="remove" value="on" class="h-4 w-4 accent-floor";
                        "Remove all their messages, reactions and votes"
                    }
                    button type="submit" class="btn bg-red-700 hover:bg-red-800" { "Ban " (user.display_name) }
                    p class="text-sm text-muted dark:text-haint" {
                        "Banning deactivates the account and signs it out everywhere. Banned addresses can't use this server at all, so check that nobody else shares them."
                    }
                }
            }
        }
    }
}

/// Pausing someone's posting, for moderators.
fn timeout_controls(user: &User, standing: &Standing) -> Markup {
    html! {
        form method="post" action={ "/people/" (user.id) "/timeout" } class="flex flex-wrap items-end gap-2" {
            label {
                span class="field-label" { "Time out" }
                select name="duration" class="field py-1.5" {
                    @if standing.timed_out_until.is_some() { option value="0" { "End the time-out" } }
                    @for (duration, label) in crate::community::TIMEOUTS {
                        option value=(duration) { "For " (label) }
                    }
                }
            }
            button type="submit" class="btn-quiet" { "Apply" }
            p class="w-full text-sm text-muted dark:text-haint" { "Timed-out people can read but not post, react or vote." }
        }
    }
}

/// What an admin can do about someone else's account.
fn admin_controls(user: &User, standing: &Standing) -> Markup {
    let base = format!("/people/{}", user.id);
    html! {
        section class="mt-10 space-y-6 rounded-xl border border-line p-4 dark:border-night-line" {
            h2 class="font-bold" { "Admin" }
            @if !user.is_admin {
                form method="post" action={ (base) "/trust" } class="flex flex-wrap items-end gap-2" {
                    label {
                        span class="field-label" { "Trust level" }
                        select name="level" class="field py-1.5" {
                            @for (level, name, _) in crate::community::LEVELS {
                                option value=(level) selected[level == standing.progress.level] { (level) ": " (name) }
                            }
                        }
                    }
                    label class="mb-2 flex items-center gap-2 text-sm" {
                        input type="checkbox" name="locked" value="on" checked[standing.progress.locked];
                        "Keep it there"
                    }
                    button type="submit" class="btn-quiet" { "Set level" }
                    p class="w-full text-sm text-muted dark:text-haint" {
                        (standing.progress.days) " days here, visited on " (standing.progress.visits) " days, "
                        (standing.progress.messages) " messages."
                    }
                }
                @if !standing.all_roles.is_empty() {
                    form method="post" action={ (base) "/roles" } {
                        fieldset {
                            legend class="field-label" { "Roles" }
                            div class="flex flex-wrap gap-3" {
                                @for role in &standing.all_roles {
                                    label class="flex items-center gap-2 text-sm" {
                                        input type="checkbox" name={ "role_" (role.id) } value="on" checked[standing.role_ids.contains(&role.id)];
                                        (role.name)
                                    }
                                }
                            }
                        }
                        button type="submit" class="btn-quiet mt-2" { "Save roles" }
                    }
                } @else {
                    p class="text-sm text-muted dark:text-haint" {
                        "Create roles, such as moderators, under " a href="/admin/permissions#roles" class="underline underline-offset-2" { "Permissions" } "."
                    }
                }
                (timeout_controls(user, standing))
            }
            div class="flex flex-wrap gap-2" {
                @if user.deactivated {
                    form method="post" action={ (base) "/reactivate" } {
                        button type="submit" class="btn-quiet" { "Reactivate account" }
                    }
                } @else {
                    form method="post" action={ (base) "/reset-link" } {
                        button type="submit" class="btn-quiet" { (icon(icons::KEY, "h-4 w-4")) "Create password reset link" }
                    }
                    form method="post" action={ (base) "/reset-security" } {
                        button type="submit" class="btn-quiet" title="Removes their passkeys, authenticator app and recovery codes, for when they lost their devices" { "Reset passkeys and app" }
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

/// What the profile form shows.
pub struct Edit<'a> {
    pub user: &'a User,
    pub ctx: &'a Context,
    /// Emoji they react with most, suggested as favorites.
    pub most_used: &'a [String],
    pub hidden_from_rankings: bool,
}

pub fn edit_page(shell: &Shell<'_>, edit: &Edit<'_>, error: Option<&str>, saved: bool) -> Markup {
    let Edit {
        user,
        ctx,
        most_used,
        hidden_from_rankings,
    } = *edit;
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
                fieldset id="rankings" {
                    legend class="field-label" { "Statistics" }
                    label class="flex items-start gap-2 text-sm" {
                        input type="checkbox" name="hide_from_rankings" value="on" checked[hidden_from_rankings] class="mt-0.5 h-4 w-4 accent-floor";
                        span {
                            "Leave me out of the rankings"
                            span class="block text-muted dark:text-haint" { "Your messages still count in the totals on the Statistics page, but your name isn't listed." }
                        }
                    }
                }
                button type="submit" class="btn" { "Save profile" }
            }
        },
    )
}
