//! Sign-in and security: passkeys, authenticator apps, recovery codes and
//! email, the second step of signing in, and the admin's sign-in policy.

use maud::{Markup, PreEscaped, html};

use super::{Shell, admin::tabs, auth_page, form_error, panel_page, section, timestamp_date};
use crate::{
    icons::{self, icon},
    mail,
    security::{Factors, PasskeyInfo, Policy, Requirement},
};

pub struct SecurityView<'a> {
    pub passkeys: &'a [PasskeyInfo],
    pub factors: Factors,
    pub policy: Policy,
    pub email: Option<&'a str>,
    pub mail_ready: bool,
    pub notice: Option<&'a str>,
    /// Recovery codes to show once, right after making them.
    pub codes: Option<&'a [String]>,
}

fn notice(text: &str) -> Markup {
    html! {
        p role="status" class="mb-5 rounded-lg border border-line bg-haint-2 px-3 py-2 text-sm text-floor dark:border-night-line dark:bg-floor-2 dark:text-haint-2" {
            (text)
        }
    }
}

const fn requirement_text(requirement: Requirement) -> &'static str {
    match requirement {
        Requirement::None => "",
        Requirement::AdminsSecondStep | Requirement::EveryoneSecondStep => {
            "This Sideporch asks you to add a passkey or an authenticator app before you continue."
        }
        Requirement::Passkeys => {
            "This Sideporch signs in with passkeys. Add one before you continue; after that, your password alone no longer signs you in."
        }
    }
}

/// New recovery codes, shown once.
fn recovery_box(codes: &[String]) -> Markup {
    html! {
                section class="mb-8 rounded-xl border-2 border-floor-3 p-4" {
                    h2 class="mb-1 font-bold" { "Your recovery codes" }
                    p class="mb-3 text-sm text-muted dark:text-haint" {
                        "Each works once, instead of a code from your app or a passkey, if you lose your phone. "
                        "Keep them somewhere safe, like your password manager. They won't be shown again."
                    }
                    ul class="mb-3 grid grid-cols-2 gap-1 font-mono text-sm sm:grid-cols-5" {
                        @for code in codes { li { (code) } }
                    }
                    button type="button" data-copy=(codes.join("\n")) class="btn-quiet text-sm" {
                        (icon(icons::COPY, "h-4 w-4")) span data-copy-label { "Copy" }
                    }
                }
    }
}

pub fn settings_page(shell: &Shell<'_>, view: &SecurityView<'_>) -> Markup {
    let factors = view.factors;
    panel_page(
        "Sign-in and security",
        shell,
        &html! { "Sign-in and security" },
        &html! {
            @if shell.user.must_secure {
                p role="alert" class="mb-5 rounded-lg border border-amber-300 bg-amber-50 px-3 py-2 text-sm text-amber-900 dark:border-amber-800 dark:bg-amber-950 dark:text-amber-100" {
                    (requirement_text(view.policy.require))
                }
            }
            @if let Some(text) = view.notice { (notice(text)) }
            @if let Some(codes) = view.codes { (recovery_box(codes)) }
            (section("Passkeys", "Sign in with your fingerprint, face or device PIN instead of a password. Passkeys can't be phished or guessed, and they sync between your devices.", &html! {
                @if !view.passkeys.is_empty() {
                    ul class="mb-4 space-y-2" {
                        @for passkey in view.passkeys {
                            li class="flex items-center gap-3 rounded-lg border border-line px-3 py-2 dark:border-night-line" {
                                (icon(icons::FINGERPRINT, "h-5 w-5 shrink-0 text-muted dark:text-haint"))
                                div class="min-w-0 flex-1" {
                                    p class="truncate font-semibold" { (passkey.name) }
                                    p class="text-xs text-muted dark:text-haint" {
                                        "Added " (timestamp_date(passkey.created_at))
                                        @if let Some(used) = passkey.last_used_at { ", last used " (timestamp_date(used)) }
                                    }
                                }
                                form method="post" action={ "/settings/security/passkeys/" (passkey.id) "/delete" } {
                                    button type="submit" class="btn-quiet p-1.5" aria-label={ "Remove " (passkey.name) } title="Remove" {
                                        (icon(icons::TRASH, "h-4 w-4"))
                                    }
                                }
                            }
                        }
                    }
                }
                button type="button" data-passkey-add hidden class="btn" { (icon(icons::FINGERPRINT, "h-5 w-5")) "Add a passkey" }
                p data-passkey-unsupported class="text-sm text-muted dark:text-haint" {
                    "This browser can't create passkeys here. Passkeys need a current browser, and Sideporch opened over HTTPS (or on localhost)."
                }
                p data-passkey-error role="alert" class="mt-2 hidden text-sm text-red-700 dark:text-red-300" {}
            }))
            (section("Authenticator app", "Asks for a six-digit code from an app like 1Password, Google Authenticator or Aegis after your password.", &html! {
                @if factors.totp {
                    div class="flex flex-wrap items-center gap-3" {
                        p class="flex items-center gap-2 font-semibold" { (icon(icons::SHIELD_CHECK, "h-5 w-5 text-floor-3 dark:text-haint")) "On" }
                        form method="post" action="/settings/security/totp/delete" {
                            button type="submit" class="btn-quiet text-sm" { "Turn off" }
                        }
                    }
                } @else {
                    a href="/settings/security/totp" class="btn" { "Set up an authenticator app" }
                }
            }))
            @if factors.second_step() {
                (section("Recovery codes", "For when you can't use your passkeys or app. Making new ones replaces the old.", &html! {
                    @if factors.recovery_codes == 0 {
                        p class="mb-3 text-sm" { "You have none. Make some, so you can still get in if you lose your devices." }
                    } @else {
                        p class="mb-3 text-sm" { (factors.recovery_codes) (if factors.recovery_codes == 1 { " code left." } else { " codes left." }) }
                    }
                    form method="post" action="/settings/security/recovery" {
                        button type="submit" class=(if factors.recovery_codes == 0 { "btn" } else { "btn-quiet" }) {
                            @if factors.recovery_codes == 0 { "Make recovery codes" } @else { "Make new recovery codes" }
                        }
                    }
                }))
            }
            (section("Email", "For sign-in links and resetting your password. Nobody else sees it.", &html! {
                @if let Some(email) = view.email {
                    div class="mb-3 flex flex-wrap items-center gap-3" {
                        p class="font-semibold" { (email) }
                        form method="post" action="/settings/security/email/remove" {
                            button type="submit" class="btn-quiet text-sm" { "Remove" }
                        }
                    }
                }
                @if view.mail_ready {
                    form method="post" action="/settings/security/email" class="flex flex-wrap items-end gap-2" {
                        label class="min-w-60 flex-1" {
                            span class="field-label" { @if view.email.is_some() { "Change it to" } @else { "Your email address" } }
                            input type="email" name="email" required autocomplete="email" class="field";
                        }
                        button type="submit" class="btn" { "Send confirmation" }
                    }
                } @else {
                    p class="text-sm text-muted dark:text-haint" { "This Sideporch doesn't send email yet; an admin can set it up." }
                }
            }))
            (section("Password", "", &html! {
                a href="/settings/account" class="btn-quiet" { (icon(icons::KEY, "h-4 w-4")) "Change your password" }
            }))
        },
    )
}

pub fn totp_page(
    shell: &Shell<'_>,
    uri: &str,
    secret: &str,
    sealed: &str,
    error: Option<&str>,
) -> Markup {
    let qr = qrcode::QrCode::new(uri.as_bytes()).map(|code| {
        code.render::<qrcode::render::svg::Color<'_>>()
            .min_dimensions(200, 200)
            .quiet_zone(true)
            .dark_color(qrcode::render::svg::Color("#1b2a28"))
            .light_color(qrcode::render::svg::Color("#ffffff"))
            .build()
    });
    let spaced: String = secret
        .chars()
        .collect::<Vec<_>>()
        .chunks(4)
        .map(|chunk| chunk.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join(" ");
    panel_page(
        "Authenticator app",
        shell,
        &html! { "Set up an authenticator app" },
        &html! {
            ol class="mb-6 list-decimal space-y-4 pl-5" {
                li {
                    "Scan this code with your authenticator app."
                    @if let Ok(svg) = &qr {
                        div class="mt-3 w-52 overflow-hidden rounded-lg border border-line bg-white p-1 dark:border-night-line" role="img" aria-label="QR code for your authenticator app" {
                            (PreEscaped(svg))
                        }
                    }
                    p class="mt-2 text-sm text-muted dark:text-haint" {
                        "Or enter this key: " code class="select-all rounded bg-screen px-1.5 py-0.5 dark:bg-night-2" { (spaced) }
                    }
                }
                li {
                    "Enter the six-digit code it shows."
                    (form_error(error))
                    form method="post" action="/settings/security/totp" class="mt-3 flex flex-wrap items-end gap-2" {
                        input type="hidden" name="secret" value=(sealed);
                        input type="text" name="code" required inputmode="numeric" autocomplete="one-time-code" pattern="[0-9 ]{6,7}" maxlength="7"
                            aria-label="Six-digit code" placeholder="123 456" class="field w-40 text-lg tracking-widest";
                        button type="submit" class="btn" { "Turn on" }
                    }
                }
            }
            a href="/settings/security" class="btn-quiet" { (icon(icons::ARROW_LEFT, "h-4 w-4")) "Back" }
        },
    )
}

/// The second step of signing in.
pub fn verify_page(factors: Factors, next: Option<&str>, error: Option<&str>) -> Markup {
    auth_page(
        "Confirm it's you",
        &html! {
            h1 class="mb-2 text-xl font-bold" { "Confirm it's you" }
            (form_error(error))
            @if factors.passkeys > 0 {
                button type="button" data-passkey-login data-next=[next] class="btn mb-2 w-full" {
                    (icon(icons::FINGERPRINT, "h-5 w-5")) "Use your passkey"
                }
                p data-passkey-error role="alert" class="mb-2 hidden text-sm text-red-700 dark:text-red-300" {}
            }
            @if factors.totp {
                form method="post" action="/login/verify" class="mt-3" {
                    @if let Some(next) = next { input type="hidden" name="next" value=(next); }
                    label for="code" class="field-label" { "Code from your authenticator app" }
                    input id="code" type="text" name="code" required autofocus inputmode="numeric" autocomplete="one-time-code"
                        pattern="[0-9 ]{6,7}" maxlength="7" placeholder="123 456" class="field text-lg tracking-widest";
                    button type="submit" class="btn mt-3 w-full" { "Continue" }
                }
            }
            details class="mt-5 text-sm" {
                summary class="cursor-pointer text-muted dark:text-haint" { "Use a recovery code" }
                form method="post" action="/login/verify" class="mt-2" {
                    @if let Some(next) = next { input type="hidden" name="next" value=(next); }
                    input type="text" name="recovery" required autocomplete="off" aria-label="Recovery code" placeholder="abcd-efgh" class="field";
                    button type="submit" class="btn-quiet mt-2" { "Continue" }
                }
            }
            p class="mt-5 text-sm text-muted dark:text-haint" { "Lost everything? An admin can reset your sign-in." }
        },
    )
}

pub fn email_link_page(reset: bool, sent: bool) -> Markup {
    let title = if reset {
        "Reset your password"
    } else {
        "Sign in with email"
    };
    auth_page(
        title,
        &html! {
            h1 class="mb-2 text-xl font-bold" { (title) }
            @if sent {
                p class="mb-5 text-muted dark:text-haint" {
                    "If an account here has that address, a link is on its way. It works for 15 minutes."
                }
            } @else {
                p class="mb-5 text-muted dark:text-haint" {
                    @if reset { "Enter the address you confirmed for your account. We'll email a link to choose a new password." }
                    @else { "Enter the address you confirmed for your account. We'll email you a link that signs you in." }
                }
                form method="post" action="/login/email" {
                    input type="hidden" name="purpose" value=(if reset { "reset" } else { "login" });
                    label for="email" class="field-label" { "Email" }
                    input id="email" type="email" name="email" required autofocus autocomplete="email" class="field";
                    button type="submit" class="btn mt-4 w-full" { "Email me a link" }
                }
            }
            a href="/login" class="btn-quiet mt-5" { (icon(icons::ARROW_LEFT, "h-4 w-4")) "Back to sign in" }
        },
    )
}

pub fn use_link_page(token: &str) -> Markup {
    auth_page(
        "Sign in",
        &html! {
            h1 class="mb-2 text-xl font-bold" { "Sign in" }
            p class="mb-5 text-muted dark:text-haint" { "Continue to sign in with the link from your email." }
            form method="post" action={ "/login/link/" (token) } {
                button type="submit" class="btn w-full" { "Sign in" }
            }
        },
    )
}

/// Where Sideporch sends email through.
fn mail_fields(mail: &mail::Settings) -> Markup {
    html! {
                fieldset class="space-y-3" {
                    legend class="mb-1 font-bold" { "Email" }
                    p class="text-sm text-muted dark:text-haint" { "An SMTP server Sideporch sends through: for sign-in links, password resets and confirming addresses." }
                    div class="grid gap-3 sm:grid-cols-[1fr_7rem]" {
                        label { span class="field-label" { "Server" } input name="mail_host" value=(mail.host) placeholder="smtp.example.com" class="field"; }
                        label { span class="field-label" { "Port" } input name="mail_port" type="number" value=(mail.port) min="1" max="65535" class="field"; }
                    }
                    label class="block" {
                        span class="field-label" { "Security" }
                        select name="mail_security" class="field" {
                            @for (security, label) in [(mail::Security::StartTls, "STARTTLS (usually port 587)"), (mail::Security::Tls, "TLS (usually port 465)"), (mail::Security::None, "None, for a relay on this network")] {
                                option value=(security.key()) selected[mail.security == security] { (label) }
                            }
                        }
                    }
                    div class="grid gap-3 sm:grid-cols-2" {
                        label { span class="field-label" { "Username" } input name="mail_username" value=(mail.username) autocomplete="off" class="field"; }
                        label {
                            span class="field-label" { "Password" }
                            input name="mail_password" type="password" autocomplete="new-password" class="field"
                                placeholder=(if mail.password.is_some() { "Stored; type to replace" } else { "" });
                        }
                    }
                    @if mail.password.is_some() {
                        label class="flex items-center gap-2 text-sm" { input type="checkbox" name="mail_password_clear" value="on"; "Remove the stored password" }
                    }
                    label class="block" {
                        span class="field-label" { "Send as" }
                        input name="mail_from" value=(mail.from) placeholder="Sideporch <chat@example.com>" class="field";
                    }
                    p class="text-sm text-muted dark:text-haint" { "The password is encrypted with the secret key, like automation secrets." }
                }
    }
}

pub fn policy_page(
    shell: &Shell<'_>,
    policy: Policy,
    mail: &mail::Settings,
    counts: (i64, i64, i64),
    error: Option<&str>,
    saved: Option<&str>,
) -> Markup {
    let (people, with_passkeys, with_apps) = counts;
    let choices = [
        (
            Requirement::None,
            "A password is enough",
            "Everyone can add a passkey or an authenticator app for a second step.",
        ),
        (
            Requirement::AdminsSecondStep,
            "Admins need a second step",
            "Admins must add a passkey or an authenticator app.",
        ),
        (
            Requirement::EveryoneSecondStep,
            "Everyone needs a second step",
            "Everyone must add a passkey or an authenticator app before they continue.",
        ),
        (
            Requirement::Passkeys,
            "Everyone signs in with a passkey",
            "People without one add one at their next sign-in; after that, passwords no longer sign them in.",
        ),
    ];
    panel_page(
        "Sign-in",
        shell,
        &html! { "Sign-in" },
        &html! {
            (tabs("/admin/sign-in"))
            (form_error(error))
            @if let Some(saved) = saved { (notice(saved)) }
            p class="mb-5 text-sm text-muted dark:text-haint" {
                (with_passkeys) " of " (people) " people have a passkey, " (with_apps) " an authenticator app."
            }
            form method="post" action="/admin/sign-in" class="space-y-8" {
                fieldset {
                    legend class="mb-2 font-bold" { "What signing in takes" }
                    div class="space-y-2" {
                        @for (requirement, label, hint) in choices {
                            label class="flex cursor-pointer gap-3 rounded-lg border border-line p-3 has-[:checked]:border-floor-3 dark:border-night-line" {
                                input type="radio" name="require" value=(requirement.key()) checked[policy.require == requirement] class="mt-1";
                                span {
                                    span class="block font-semibold" { (label) }
                                    span class="block text-sm text-muted dark:text-haint" { (hint) }
                                }
                            }
                        }
                    }
                    label class="mt-4 flex gap-3" {
                        input type="checkbox" name="email_links" value="on" checked[policy.email_links] class="mt-1";
                        span {
                            span class="block font-semibold" { "Sign in with a link by email" }
                            span class="block text-sm text-muted dark:text-haint" {
                                "People with a confirmed address can ask for a link instead of typing their password. "
                                "The link counts as the first step; a passkey or app code is still asked for where set up."
                            }
                        }
                    }
                }
                (mail_fields(mail))
                button type="submit" class="btn" { "Save" }
            }
            @if mail.configured() {
                form method="post" action="/admin/sign-in/test-email" class="mt-6 flex flex-wrap items-end gap-2" {
                    label class="min-w-60 flex-1" {
                        span class="field-label" { "Send a test email to" }
                        input type="email" name="to" required class="field";
                    }
                    button type="submit" class="btn-quiet" { "Send test" }
                }
            }
        },
    )
}
