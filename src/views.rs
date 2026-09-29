//! HTML pages, rendered on the server with maud. Styling uses utility
//! classes that build.rs compiles with encre-css.

use axum::http::StatusCode;
use maud::{DOCTYPE, Markup, PreEscaped, html};

use crate::{
    auth::CurrentUser,
    icons::{self, icon},
    markup::{self, Context},
    store::{
        Author, Channel, ChannelKind, FileRef, Gif, Invite, Message, Sidebar, SidebarItem, User,
    },
    themes::Choice,
    webhook::Attachment,
};

pub mod account;
pub mod admin;
pub mod appearance;
pub mod automations;
pub mod channels;
pub mod community;
pub use channels::{ChannelSettings, channel_settings_page};
pub mod emoji;
pub mod gifs;
pub mod later;
pub mod messages;
pub mod profile;
pub mod search;
pub mod settings;

/// How to render messages: this Sideporch's custom emoji and usernames, and
/// who is looking. Live updates are rendered once for everyone, so they
/// have no viewer; the browser marks the viewer's own reactions.
pub struct Render<'a> {
    pub ctx: &'a Context,
    pub viewer: Option<i64>,
    /// Messages the viewer saved for later.
    pub saved: Option<&'a std::collections::HashSet<i64>>,
}

impl<'a> Render<'a> {
    pub const fn shared(ctx: &'a Context) -> Self {
        Self {
            ctx,
            viewer: None,
            saved: None,
        }
    }

    pub const fn for_user(ctx: &'a Context, viewer: i64) -> Self {
        Self {
            ctx,
            viewer: Some(viewer),
            saved: None,
        }
    }

    #[must_use]
    pub const fn with_saved(mut self, saved: &'a std::collections::HashSet<i64>) -> Self {
        self.saved = Some(saved);
        self
    }

    fn is_saved(&self, id: i64) -> bool {
        self.saved.is_some_and(|saved| saved.contains(&id))
    }

    /// Slack-style text, as webhooks send it.
    fn markup(&self, text: &str) -> PreEscaped<String> {
        PreEscaped(markup::render(text, self.ctx))
    }

    /// A message body, in the format its sender wrote.
    fn body(&self, message: &Message) -> PreEscaped<String> {
        if message.slack_format {
            self.markup(&message.body)
        } else {
            PreEscaped(crate::markdown::render(&message.body, self.ctx))
        }
    }
}

pub const ASSET_VERSION: &str = env!("SIDEPORCH_ASSET_VERSION");

/// Consecutive messages from one author within this window share a header.
const GROUP_WINDOW_MS: i64 = 5 * 60 * 1000;

fn document(title: &str, body_class: &str, choice: &Choice, content: &Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" data-assets=(ASSET_VERSION) data-theme=(choice.theme.id)
            data-appearance=(choice.appearance.key()) class=[choice.dark().then_some("dark")] {
            head {
                // Before anything else, so the page never flashes in the wrong mode.
                script src={ "/assets/theme.js?v=" (ASSET_VERSION) } {}
                meta charset="utf-8";
                // Resizing for the on-screen keyboard keeps the composer visible.
                meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover, interactive-widget=resizes-content";
                meta name="theme-color" content="#24403C";
                meta name="mobile-web-app-capable" content="yes";
                meta name="apple-mobile-web-app-capable" content="yes";
                meta name="apple-mobile-web-app-title" content="Sideporch";
                meta name="apple-mobile-web-app-status-bar-style" content="default";
                title { (title) " · Sideporch" }
                link rel="icon" href="/assets/logo.svg" type="image/svg+xml";
                link rel="icon" href="/assets/icons/icon-192.png" type="image/png" sizes="192x192";
                link rel="apple-touch-icon" href="/assets/icons/apple-touch-icon.png";
                link rel="manifest" href="/manifest.webmanifest";
                link rel="stylesheet" href={ "/assets/themes.css?v=" (crate::themes::VERSION.as_str()) };
                link rel="stylesheet" href={ "/assets/app.css?v=" (ASSET_VERSION) };
                script src={ "/assets/app.js?v=" (ASSET_VERSION) } defer {}
            }
            body class=(body_class) { (content) }
        }
    }
}

// Signed-out pages

pub fn auth_page(title: &str, content: &Markup) -> Markup {
    let page = html! {
        main class="mx-auto flex min-h-screen max-w-md flex-col justify-center px-5 py-10" {
            a href="/" class="mb-8 flex items-center gap-3 self-center text-haint-2" {
                img src="/assets/logo.svg" alt="" class="h-11 w-11";
                span class="text-2xl font-bold tracking-tight" { "Sideporch" }
            }
            div class="rounded-2xl bg-white p-7 text-ink shadow-xl dark:bg-night-2 dark:text-haint-2" {
                (content)
            }
        }
    };
    document(
        title,
        "bg-floor text-ink antialiased",
        &Choice::default(),
        &page,
    )
}

pub fn form_error(error: Option<&str>) -> Markup {
    html! {
        @if let Some(error) = error {
            p role="alert" class="mb-4 rounded-lg border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-800 dark:border-red-900 dark:bg-red-950 dark:text-red-200" {
                (error)
            }
        }
    }
}

pub fn text_field(
    label: &str,
    name: &str,
    kind: &str,
    value: &str,
    autocomplete: &str,
    hint: Option<&str>,
) -> Markup {
    html! {
        div class="mb-4" {
            label for=(name) class="field-label" { (label) }
            input id=(name) name=(name) type=(kind) value=(value) autocomplete=(autocomplete) required class="field";
            @if let Some(hint) = hint {
                p class="mt-1 text-sm text-muted dark:text-haint" { (hint) }
            }
        }
    }
}

pub fn login_page(
    error: Option<&str>,
    username: &str,
    next: Option<&str>,
    registration: crate::community::Registration,
) -> Markup {
    auth_page(
        "Sign in",
        &html! {
            h1 class="mb-5 text-xl font-bold" { "Sign in" }
            (form_error(error))
            form method="post" action="/login" {
                @if let Some(next) = next {
                    input type="hidden" name="next" value=(next);
                }
                (text_field("Username", "username", "text", username, "username", None))
                (text_field("Password", "password", "password", "", "current-password", None))
                button type="submit" class="btn mt-2 w-full" { "Sign in" }
            }
            p class="mt-5 text-sm text-muted dark:text-haint" {
                @match registration {
                    crate::community::Registration::Invite => { "New here? Ask someone on this porch for an invite link." }
                    crate::community::Registration::Open => { "New here? " a href="/signup" class="font-semibold underline underline-offset-2" { "Create an account" } }
                    crate::community::Registration::Approval => { "New here? " a href="/signup" class="font-semibold underline underline-offset-2" { "Ask to join" } }
                }
            }
        },
    )
}

#[derive(Default)]
pub struct AccountForm {
    pub display_name: String,
    pub username: String,
}

pub fn account_fields(form: &AccountForm) -> Markup {
    html! {
        (text_field("Your name", "display_name", "text", &form.display_name, "name", Some("Shown next to your messages.")))
        (text_field("Username", "username", "text", &form.username, "username", Some("Used to sign in. Letters, numbers, dots, dashes and underscores.")))
        (text_field("Password", "password", "password", "", "new-password", Some("At least 8 characters.")))
    }
}

/// `open`: anyone who reaches the page can claim it, so say so.
pub fn setup_page(action: &str, open: bool, error: Option<&str>, form: &AccountForm) -> Markup {
    auth_page(
        "Set up",
        &html! {
            h1 class="mb-2 text-xl font-bold" { "Welcome to your Sideporch" }
            p class="mb-5 text-muted dark:text-haint" {
                "Create the admin account. It can invite everyone else, manage channels, and write automations."
            }
            @if open {
                p class="mb-5 rounded-lg border border-line bg-screen px-3 py-2 text-sm text-muted dark:border-night-line dark:bg-night dark:text-haint" {
                    "The first person to open this page becomes the admin, so finish setup now. To require a one-time link instead, start Sideporch with "
                    code { "--require-setup-link" } "."
                }
            }
            (form_error(error))
            form method="post" action=(action) {
                (account_fields(form))
                button type="submit" class="btn mt-2 w-full" { "Create account" }
            }
        },
    )
}

pub fn join_page(token: &str, error: Option<&str>, form: &AccountForm) -> Markup {
    auth_page(
        "Join",
        &html! {
            h1 class="mb-2 text-xl font-bold" { "Join this Sideporch" }
            p class="mb-5 text-muted dark:text-haint" { "Pick a name and a password to create your account." }
            (form_error(error))
            form method="post" action={ "/join/" (token) } {
                (account_fields(form))
                button type="submit" class="btn mt-2 w-full" { "Join" }
            }
        },
    )
}

pub fn error_page(status: StatusCode, message: &str) -> Markup {
    auth_page(
        status.canonical_reason().unwrap_or("Error"),
        &html! {
            h1 class="mb-2 text-xl font-bold" { (status.canonical_reason().unwrap_or("Error")) }
            p class="mb-5 text-muted dark:text-haint" { (message) }
            a href="/" class="btn-quiet" { "Go to your channels" }
        },
    )
}

// Signed-in shell

/// What every signed-in page needs to draw the sidebar.
pub struct Shell<'a> {
    pub user: &'a CurrentUser,
    pub sidebar: &'a Sidebar,
    pub current: Option<i64>,
}

fn sidebar_link(item: &SidebarItem, current: Option<i64>, svg: &str) -> Markup {
    let active = current == Some(item.channel_id);
    let state = if active {
        "bg-floor-3 text-white"
    } else {
        "text-haint-2 hover:bg-floor-2 data-[unread]:font-bold data-[unread]:text-white data-[muted]:opacity-60"
    };
    let svg = match item.look {
        crate::store::SidebarLook::Private => icons::LOCK_SIMPLE,
        crate::store::SidebarLook::Announcement => icons::MEGAPHONE_SIMPLE,
        crate::store::SidebarLook::Channel | crate::store::SidebarLook::Direct => svg,
    };
    html! {
        li {
            a href={ "/c/" (item.channel_id) } data-channel-link=(item.channel_id) data-unread[item.unread && !active]
                data-muted[item.muted] title=[item.muted.then_some("Muted")]
                aria-current=[active.then_some("page")]
                class={ "group flex items-center gap-2 rounded-lg px-3 py-1.5 " (state) } {
                (icon(svg, "h-4 w-4 shrink-0 opacity-70"))
                span class="truncate" { (item.label) }
                span class="ml-auto hidden h-2 w-2 shrink-0 rounded-full bg-lamp group-data-[unread]:block" {}
            }
        }
    }
}

/// A link to one of the personal pages at the top of the sidebar. app.js
/// marks the open one.
fn nav_link(href: &str, svg: &str, label: &str, badge: bool) -> Markup {
    html! {
        li {
            a href=(href) data-nav-link data-unread[badge]
                class="group flex items-center gap-2 rounded-lg px-3 py-1.5 text-haint-2 hover:bg-floor-2 aria-[current=page]:bg-floor-3 aria-[current=page]:text-white data-[unread]:font-bold data-[unread]:text-white" {
                (icon(svg, "h-4 w-4 shrink-0 opacity-70"))
                span class="truncate" { (label) }
                span class="ml-auto hidden h-2 w-2 shrink-0 rounded-full bg-lamp group-data-[unread]:block" {}
            }
        }
    }
}

fn sidebar(shell: &Shell<'_>, full_width: bool) -> Markup {
    let width = if full_width {
        "flex w-full md:w-72"
    } else {
        "hidden w-72 md:flex"
    };
    html! {
        nav aria-label="Channels" class={ "h-full shrink-0 flex-col bg-floor text-haint-2 " (width) } {
            a href="/" class="flex items-center gap-2.5 px-5 pb-3 pt-5" {
                img src="/assets/logo.svg" alt="" class="h-8 w-8";
                span class="text-lg font-bold tracking-tight text-white" { "Sideporch" }
            }
            form method="get" action="/search" role="search" class="relative px-3 pb-2" data-quick-search {
                label class="flex items-center gap-2 rounded-lg bg-floor-2 px-3 py-1.5 text-haint focus-within:bg-floor-3" {
                    (icon(icons::MAGNIFYING_GLASS, "h-4 w-4 shrink-0"))
                    input type="search" name="q" placeholder="Search" aria-label="Search messages, people and channels"
                        autocomplete="off" role="combobox" aria-expanded="false" aria-controls="quick-results" aria-autocomplete="list"
                        class="min-w-0 flex-1 bg-transparent text-sm text-white outline-hidden placeholder:text-haint";
                }
                // app.js fills this with matches while typing.
                ul id="quick-results" role="listbox" hidden
                    class="absolute inset-x-3 top-full z-40 mt-1 max-h-96 overflow-y-auto rounded-xl border border-line bg-white p-1 text-ink shadow-2xl dark:border-night-line dark:bg-night-2 dark:text-haint-2" {}
            }
            div class="flex-1 overflow-y-auto px-3 pb-4" {
                ul class="mb-4 space-y-0.5" data-sidebar-nav {
                    (nav_link("/activity", icons::AT, "Activity", shell.sidebar.activity))
                    (nav_link("/saved", icons::BOOKMARK_SIMPLE, "Saved", false))
                    (nav_link("/scheduled", icons::CLOCK, "Scheduled", false))
                    (nav_link("/people", icons::USERS, "People", false))
                }
                div class="mb-1 mt-2 flex items-center justify-between px-3 text-sm text-haint" {
                    h2 class="font-semibold" { a href="/channels/browse" class="hover:text-white hover:underline" title="Browse all channels" { "Channels" } }
                    span class="flex items-center" {
                        a href="/channels/browse" class="rounded-md p-1 hover:bg-floor-2 hover:text-white" aria-label="Browse channels" title="Browse channels" {
                            (icon(icons::COMPASS, "h-4 w-4"))
                        }
                        @if shell.user.may(crate::community::Permission::CreateChannels) || shell.user.may(crate::community::Permission::CreatePrivateChannels) {
                            a href="/channels/new" class="rounded-md p-1 hover:bg-floor-2 hover:text-white" aria-label="Create a channel" title="Create a channel" {
                                (icon(icons::PLUS, "h-4 w-4"))
                            }
                        }
                    }
                }
                ul class="space-y-0.5" {
                    @for item in &shell.sidebar.channels { (sidebar_link(item, shell.current, icons::HASH)) }
                }
                div class="mb-1 mt-6 flex items-center justify-between px-3 text-sm text-haint" {
                    h2 class="font-semibold" { "Direct messages" }
                    a href="/people" class="rounded-md p-1 hover:bg-floor-2 hover:text-white" aria-label="Message someone" title="Message someone" {
                        (icon(icons::PLUS, "h-4 w-4"))
                    }
                }
                ul class="space-y-0.5" {
                    @for item in &shell.sidebar.direct { (sidebar_link(item, shell.current, icons::CHAT_CIRCLE_TEXT)) }
                    @if shell.sidebar.direct.is_empty() {
                        li class="px-3 py-1.5 text-sm text-haint" { "Nobody yet. Say hello from People." }
                    }
                }
            }
            // app.js fills and shows this: how to install Sideporch or turn
            // on notifications, depending on the device.
            div data-install hidden class="mx-3 mb-3 rounded-xl bg-floor-2 p-3 text-sm text-haint-2" {
                p data-install-text {}
                div class="mt-2 flex items-center gap-2" {
                    button type="button" data-install-action class="rounded-lg bg-lamp px-3 py-1 text-sm font-semibold text-floor" {}
                    button type="button" data-install-dismiss class="rounded-lg px-2 py-1 text-sm text-haint hover:text-white" { "Not now" }
                }
            }
            (account_menu(shell))
        }
    }
}

/// One entry in the account menu.
fn menu_link(href: &str, svg: &str, label: &str) -> Markup {
    html! {
        a href=(href) class="flex items-center gap-2.5 rounded-lg px-3 py-1.5 hover:bg-floor-2 hover:text-white" {
            (icon(svg, "h-4 w-4 shrink-0 opacity-80")) (label)
        }
    }
}

/// The signed-in person at the bottom of the sidebar, opening everything
/// about their account. A native disclosure, so it works without scripts.
fn account_menu(shell: &Shell<'_>) -> Markup {
    let user = shell.user;
    let author = Author::User {
        id: user.id,
        display_name: user.display_name.clone(),
        avatar: user.avatar,
        status_emoji: String::new(),
    };
    html! {
        details data-account-menu class="relative border-t border-floor-2 px-3 py-2" {
            summary class="flex cursor-pointer list-none items-center gap-2.5 rounded-lg px-2 py-1.5 hover:bg-floor-2" {
                div class="scale-90" { (avatar(&author, &Context::default())) }
                span class="min-w-0 flex-1" {
                    span class="block truncate font-semibold text-white" { (user.display_name) }
                    span class="block truncate text-xs text-haint" { "Account and settings" }
                }
                (icon(icons::CARET_UP_DOWN, "h-4 w-4 shrink-0 opacity-70"))
            }
            nav aria-label="Account" class="absolute inset-x-3 bottom-full mb-1 space-y-0.5 rounded-xl border border-floor-3 bg-floor p-1.5 text-sm shadow-2xl" {
                (menu_link(&format!("/people/{}", user.id), icons::USER_CIRCLE, "Your profile"))
                (menu_link("/settings/profile", icons::PENCIL_SIMPLE, "Edit profile"))
                (menu_link("/settings/appearance", icons::PALETTE, "Appearance"))
                (menu_link("/settings/account", icons::KEY, "Password"))
                (menu_link("/emoji", icons::SMILEY, "Custom emoji"))
                (menu_link("/gifs/library", icons::GIF, "GIF library"))
                button type="button" data-push-toggle hidden aria-pressed="false"
                    class="flex w-full items-center gap-2.5 rounded-lg px-3 py-1.5 text-left hover:bg-floor-2 hover:text-white" {
                    (icon(icons::BELL, "h-4 w-4 shrink-0 opacity-80"))
                    span class="flex-1" { "Notifications" }
                    span data-push-state class="text-xs text-haint" { "Off" }
                }
                @if user.is_admin || user.may(crate::community::Permission::Moderate) {
                    div class="my-1 border-t border-floor-2" {}
                    (menu_link("/moderation", icons::SHIELD_CHECK, "Moderation"))
                }
                @if user.is_admin {
                    (menu_link("/automations", icons::LIGHTNING, "Automations"))
                    (menu_link("/admin/system", icons::GAUGE, "Admin"))
                }
                div class="my-1 border-t border-floor-2" {}
                form method="post" action="/logout" {
                    button type="submit" class="flex w-full items-center gap-2.5 rounded-lg px-3 py-1.5 text-left hover:bg-floor-2 hover:text-white" {
                        (icon(icons::SIGN_OUT, "h-4 w-4 shrink-0 opacity-80")) "Sign out"
                    }
                }
            }
        }
    }
}

/// Identifies what a signed-in page shows, for the live-update script.
#[derive(Default)]
struct PageData {
    channel: Option<i64>,
    thread: Option<i64>,
}

const APP_BODY: &str = "bg-white text-ink antialiased dark:bg-night dark:text-haint-2";

fn app_page(title: &str, shell: &Shell<'_>, data: &PageData, main: &Markup) -> Markup {
    let page = html! {
        div id="app" data-me=(shell.user.id) data-admin[shell.user.is_admin] data-moderator[shell.user.may(crate::community::Permission::Moderate)] data-channel=[data.channel] data-thread=[data.thread] class="flex h-dvh overflow-hidden" {
            (sidebar(shell, false))
            (main)
        }
    };
    document(title, APP_BODY, &shell.user.choice, &page)
}

/// The channel list on its own: the mobile home screen.
pub fn home_page(shell: &Shell<'_>) -> Markup {
    let page = html! {
        div id="app" data-me=(shell.user.id) class="flex h-dvh overflow-hidden" {
            (sidebar(shell, true))
            main class="hidden flex-1 items-center justify-center p-10 text-muted md:flex dark:text-haint" {
                p { "Pick a channel to start talking." }
            }
        }
    };
    document("Channels", APP_BODY, &shell.user.choice, &page)
}

fn back_to_channels() -> Markup {
    html! {
        a href="/home" class="-ml-2 rounded-lg p-2 hover:bg-screen md:hidden dark:hover:bg-night-2" aria-label="All channels" {
            (icon(icons::ARROW_LEFT, "h-5 w-5"))
        }
    }
}

fn channel_label(channel: &Channel) -> Markup {
    let svg = match channel.kind {
        ChannelKind::Public if channel.posting.post == crate::store::Policy::Managers => {
            icons::MEGAPHONE_SIMPLE
        }
        ChannelKind::Public => icons::HASH,
        ChannelKind::Private => icons::LOCK_SIMPLE,
        ChannelKind::Direct => icons::CHAT_CIRCLE_TEXT,
    };
    html! {
        span class="flex min-w-0 items-center gap-1.5" {
            (icon(svg, "h-5 w-5 shrink-0 text-muted dark:text-haint"))
            span class="truncate" { (channel.name) }
        }
    }
}

// Channel view

pub struct ChannelView<'a> {
    pub channel: &'a Channel,
    pub messages: &'a [Message],
    pub older: Option<i64>,
    pub thread: Option<(&'a Message, &'a [Message])>,
    pub render: &'a Render<'a>,
    /// Emoji names the reaction picker shows first.
    pub favorites: &'a [String],
    /// Where the GIF picker searches.
    pub gifs: &'a crate::gifs::Settings,
    /// How many messages are pinned in the channel.
    pub pins: i64,
    /// Minutes people can edit their messages for, if limited.
    pub edit_window: Option<i64>,
}

pub fn channel_page(shell: &Shell<'_>, view: &ChannelView<'_>) -> Markup {
    let channel = view.channel;
    let thread_open = view.thread.is_some();
    let data = PageData {
        channel: Some(channel.id),
        thread: view.thread.map(|(root, _)| root.id),
    };
    let composer_label = match channel.kind {
        ChannelKind::Public | ChannelKind::Private => format!("Message #{}", channel.name),
        ChannelKind::Direct => format!("Message {}", channel.name),
    };
    let column = if thread_open {
        "hidden lg:flex"
    } else {
        "flex"
    };
    let main = html! {
        main class={ "min-w-0 flex-1 flex-col " (column) } data-edit-minutes=[view.edit_window] {
            header class="flex h-14 shrink-0 items-center gap-2 border-b border-line px-5 dark:border-night-line" {
                (back_to_channels())
                h1 class="min-w-0 text-lg font-bold" { (channel_label(channel)) }
                @if !channel.topic.is_empty() {
                    p class="hidden min-w-0 truncate border-l border-line pl-3 text-sm text-muted sm:block dark:border-night-line dark:text-haint" {
                        (channel.topic)
                    }
                }
                span class="ml-auto" {}
                @if channel.kind != ChannelKind::Direct {
                    a href={ "/search?q=in%3A%23" (channel.name) "%20" }
                        class="rounded-lg p-2 text-muted hover:bg-screen hover:text-ink dark:text-haint dark:hover:bg-night-2"
                        aria-label="Search this channel" title="Search this channel" {
                        (icon(icons::MAGNIFYING_GLASS, "h-5 w-5"))
                    }
                }
                a href={ "/c/" (channel.id) "/pins" } class="flex items-center gap-1 rounded-lg p-2 text-sm text-muted hover:bg-screen hover:text-ink dark:text-haint dark:hover:bg-night-2"
                    aria-label={ "Pinned messages: " (view.pins) } title="Pinned messages" {
                    (icon(icons::PUSH_PIN, "h-5 w-5"))
                    @if view.pins > 0 { span { (view.pins) } }
                }
                @if channel.kind != ChannelKind::Direct {
                    form method="post" action={ "/c/" (channel.id) "/mute" } {
                        button type="submit" aria-pressed=(if channel.muted { "true" } else { "false" })
                            class="rounded-lg p-2 text-muted hover:bg-screen hover:text-ink aria-pressed:text-amber-700 dark:text-haint dark:hover:bg-night-2 dark:aria-pressed:text-lamp"
                            aria-label=(if channel.muted { "Unmute channel" } else { "Mute channel" })
                            title=(if channel.muted { "Muted: no unread marks or @channel notifications. Click to unmute." } else { "Mute: no unread marks or @channel notifications" }) {
                            (icon(if channel.muted { icons::BELL_SLASH } else { icons::BELL }, "h-5 w-5"))
                        }
                    }
                    a href={ "/c/" (channel.id) "/settings" } class="rounded-lg p-2 text-muted hover:bg-screen hover:text-ink dark:text-haint dark:hover:bg-night-2"
                        aria-label="Channel settings" title="Channel settings and webhooks" {
                        (icon(icons::GEAR_SIX, "h-5 w-5"))
                    }
                }
            }
            div id="scroller" class="flex-1 overflow-y-auto" {
                @if let Some(before) = view.older {
                    div class="flex justify-center pt-4" {
                        a href={ "/c/" (channel.id) "?before=" (before) } class="btn-quiet text-sm" { "Show earlier messages" }
                    }
                }
                @if view.messages.is_empty() {
                    (empty_channel(channel))
                }
                ol id="messages" class="py-3" {
                    (message_list(view.messages, view.render))
                }
            }
            @if channel.left {
                form method="post" action={ "/c/" (channel.id) "/join" } class="mx-4 flex items-center gap-3 rounded-xl bg-screen px-4 py-2 text-sm dark:bg-night-2" {
                    span class="flex-1" { "You left #" (channel.name) ". Join it again to see it in your sidebar, or just write." }
                    button type="submit" class="btn px-3 py-1 text-sm" { "Join" }
                }
            }
            @if channel.may_write(false) {
                (composer(&format!("/c/{}/messages", channel.id), None, &composer_label, shell.user.grants))
            } @else {
                (read_only_notice(channel, false))
            }
        }
    };
    let thread = view.thread.map(|(root, replies)| {
        thread_panel(channel, root, replies, view.render, shell.user.grants)
    });
    app_page(
        &channel.name,
        shell,
        &data,
        &html! {
            (main)
            @if let Some(thread) = thread { (thread) }
            (emoji_picker(view.render.ctx, view.favorites))
            (gif_picker(view.gifs))
        },
    )
}

fn empty_channel(channel: &Channel) -> Markup {
    html! {
        div class="px-5 pt-10" {
            h2 class="text-xl font-bold" {
                @match channel.kind {
                    ChannelKind::Public => { "This is the start of #" (channel.name) "." }
                    ChannelKind::Private => { "This is the start of the private channel #" (channel.name) "." }
                    ChannelKind::Direct => { "This is the start of your conversation with " (channel.name) "." }
                }
            }
            @if channel.kind == ChannelKind::Private {
                p class="mt-1 text-muted dark:text-haint" {
                    "Only its members see it. "
                    a href={ "/c/" (channel.id) "/settings" } class="underline underline-offset-2" { "Add people" }
                    " in the channel settings."
                }
            }
            @if channel.kind == ChannelKind::Public && !channel.may_write(false) {
                p class="mt-1 text-muted dark:text-haint" { "Announcements from the channel's managers will appear here." }
            }
            @if channel.kind == ChannelKind::Public && channel.may_write(false) {
                p class="mt-1 text-muted dark:text-haint" {
                    "Write the first message below, or "
                    a href={ "/c/" (channel.id) "/settings" } class="underline underline-offset-2" { "connect a monitor or bot" }
                    " to post here."
                }
            }
        }
    }
}

fn thread_panel(
    channel: &Channel,
    root: &Message,
    replies: &[Message],
    render: &Render<'_>,
    grants: crate::community::Grants,
) -> Markup {
    html! {
        aside aria-label="Thread" class="flex min-w-0 flex-1 flex-col border-line lg:border-l lg:w-96 lg:flex-none xl:w-[28rem] dark:border-night-line" {
            header class="flex h-14 shrink-0 items-center gap-2 border-b border-line px-5 dark:border-night-line" {
                h2 class="text-lg font-bold" { "Thread" }
                span class="truncate text-sm text-muted dark:text-haint" {
                    @if channel.kind != ChannelKind::Direct { "#" } (channel.name)
                }
                a href={ "/c/" (channel.id) } class="ml-auto rounded-lg p-2 text-muted hover:bg-screen hover:text-ink dark:text-haint dark:hover:bg-night-2" aria-label="Close thread" {
                    (icon(icons::X, "h-5 w-5"))
                }
            }
            div id="thread-scroller" class="flex-1 overflow-y-auto py-3" {
                ol { (message_item(root, false, false, render)) }
                div class="my-2 flex items-center gap-3 px-5 text-sm text-muted dark:text-haint" {
                    span id="thread-count" { (reply_label(root.reply_count)) }
                    span class="h-px flex-1 bg-line dark:bg-night-line" {}
                }
                ol id="replies" { (message_list(replies, render)) }
            }
            @if channel.may_write(true) {
                (composer(&format!("/c/{}/messages", channel.id), Some(root.id), "Reply", grants))
            } @else {
                (read_only_notice(channel, true))
            }
        }
    }
}

/// Takes the composer's place where the reader may not write.
fn read_only_notice(channel: &Channel, reply: bool) -> Markup {
    let can_reply = channel.may_write(true);
    let can_react = channel.may_react();
    html! {
        div class="mx-4 mb-4 mt-2 flex items-center gap-3 rounded-xl border border-line bg-screen px-4 py-3 text-sm text-muted dark:border-night-line dark:bg-night-2 dark:text-haint" {
            (icon(icons::MEGAPHONE_SIMPLE, "h-5 w-5 shrink-0"))
            p {
                @if reply { "Only managers reply in this channel's threads." }
                @else { "Only managers start posts in #" (channel.name) "." }
                @if !reply && can_reply { " Reply in a thread to join in" }
                @if can_react { @if !reply && can_reply { ", or react." } @else { " You can react." } }
                @else if !reply && can_reply { "." }
            }
        }
    }
}

fn reply_label(count: i64) -> String {
    if count == 1 {
        "1 reply".to_owned()
    } else {
        format!("{count} replies")
    }
}

fn composer(
    action: &str,
    parent: Option<i64>,
    label: &str,
    grants: crate::community::Grants,
) -> Markup {
    let may_upload = grants.has(crate::community::Permission::UploadFiles);
    let may_poll = grants.has(crate::community::Permission::CreatePolls);
    let poll_id = if parent.is_some() {
        "poll-form-thread"
    } else {
        "poll-form"
    };
    html! {
        @if may_poll { (poll_form(poll_id, action.trim_end_matches("/messages"), parent)) }
        form data-composer data-uploads[may_upload] method="post" action=(action) enctype="multipart/form-data" class="shrink-0 px-4 pb-4 pt-2" {
            @if let Some(parent) = parent {
                input type="hidden" name="parent_id" value=(parent);
            }
            ul data-file-list class="mb-1.5 hidden flex-wrap gap-1.5 px-1 text-sm" {}
            div class="flex items-end gap-1 rounded-xl border border-line bg-white py-1.5 pl-1.5 pr-1.5 focus-within:border-floor-3 dark:border-night-line dark:bg-night-2" {
                @if may_upload {
                    label class="cursor-pointer rounded-lg p-2 text-muted hover:bg-screen hover:text-ink focus-within:bg-screen dark:text-haint dark:hover:bg-night" title="Attach files or images (you can also paste or drop them)" {
                        input type="file" name="files" multiple class="sr-only" aria-label="Attach files";
                        (icon(icons::PAPERCLIP, "h-5 w-5"))
                    }
                }
                button type="button" data-gif-button hidden title="Send a GIF" aria-label="Send a GIF"
                    class="rounded-lg p-2 text-muted hover:bg-screen hover:text-ink dark:text-haint dark:hover:bg-night" {
                    (icon(icons::GIF, "h-5 w-5"))
                }
                @if may_poll {
                    button type="button" popovertarget=(poll_id) title="Create a poll" aria-label="Create a poll"
                        class="rounded-lg p-2 text-muted hover:bg-screen hover:text-ink dark:text-haint dark:hover:bg-night" {
                        (icon(icons::CHART_BAR_HORIZONTAL, "h-5 w-5"))
                    }
                }
                textarea name="body" rows="1" maxlength="10000" aria-label=(label) placeholder=(label)
                    class="max-h-48 min-h-6 flex-1 resize-none bg-transparent px-1 py-1.5 leading-6 outline-hidden placeholder:text-muted dark:placeholder:text-haint" {}
                button type="button" data-schedule-button hidden title="Send later" aria-label="Send later"
                    class="rounded-lg p-2 text-muted hover:bg-screen hover:text-ink dark:text-haint dark:hover:bg-night" {
                    (icon(icons::CLOCK, "h-5 w-5"))
                }
                button type="submit" class="btn px-3" aria-label="Send" title="Send (Enter)" {
                    (icon(icons::PAPER_PLANE_RIGHT, "h-5 w-5"))
                }
            }
            p class="mt-1 hidden px-1 text-xs text-red-700 dark:text-red-300" data-composer-error role="alert" {}
            p class="mt-0.5 h-4 truncate px-1 text-xs text-muted dark:text-haint" data-typing aria-live="polite" {}
        }
    }
}

/// Asks for a poll's question, options and kind. A popover, so it opens
/// without scripts; app.js sends it in the background.
fn poll_form(id: &str, channel_path: &str, parent: Option<i64>) -> Markup {
    let kinds = [
        ("single", "Pick one", "Everyone votes for one option."),
        (
            "multiple",
            "Pick several",
            "Good for finding a day that works for everyone.",
        ),
        (
            "ranked",
            "Rank them",
            "Everyone ranks the options. The one most people can live with wins, by instant runoff.",
        ),
    ];
    html! {
        div id=(id) popover class="dialog-popover w-[min(28rem,92vw)] rounded-2xl border border-line bg-white p-5 text-ink shadow-2xl dark:border-night-line dark:bg-night-2 dark:text-haint-2" {
            form method="post" action={ (channel_path) "/polls" } data-poll-form class="space-y-3" {
                h2 class="text-lg font-bold" { "Create a poll" }
                @if let Some(parent) = parent {
                    input type="hidden" name="parent_id" value=(parent);
                }
                label class="block" {
                    span class="mb-1 block text-sm font-semibold" { "Question" }
                    input type="text" name="question" required maxlength="300" placeholder="Where do we eat on Friday?" class="field";
                }
                label class="block" {
                    span class="mb-1 block text-sm font-semibold" { "Options, one per line" }
                    textarea name="options" required rows="5" placeholder="Pizza\nTacos\nRamen" class="field" {}
                }
                fieldset class="space-y-1.5" {
                    legend class="mb-1 text-sm font-semibold" { "How people vote" }
                    @for (index, (value, name, hint)) in kinds.iter().enumerate() {
                        label class="flex cursor-pointer items-start gap-2 rounded-lg border border-line p-2 has-[:checked]:border-floor-3 dark:border-night-line" {
                            input type="radio" name="kind" value=(value) checked[index == 0] class="mt-1";
                            span {
                                span class="block text-sm font-semibold" { (name) }
                                span class="block text-xs text-muted dark:text-haint" { (hint) }
                            }
                        }
                    }
                }
                p class="hidden text-sm text-red-700 dark:text-red-300" data-poll-error role="alert" {}
                div class="flex justify-end gap-2" {
                    button type="button" popovertarget=(id) popovertargetaction="hide" class="btn-quiet" { "Cancel" }
                    button type="submit" class="btn" { "Post poll" }
                }
            }
        }
    }
}

fn message_list(messages: &[Message], render: &Render<'_>) -> Markup {
    let grouped = std::iter::once(false).chain(messages.windows(2).map(|pair| match pair {
        [previous, message] => {
            author_key(&previous.author) == author_key(&message.author)
                && message.created_at.saturating_sub(previous.created_at) < GROUP_WINDOW_MS
        }
        _ => false,
    }));
    html! {
        @for (message, compact) in messages.iter().zip(grouped) {
            (message_item(message, compact, message.parent_id.is_none(), render))
        }
    }
}

/// Identifies an author for grouping consecutive messages.
pub fn author_key(author: &Author) -> String {
    match author {
        Author::User { id, .. } => format!("u:{id}"),
        Author::Bot { name, .. } => format!("b:{name}"),
        Author::Removed => "removed".to_owned(),
    }
}

/// One message. Live updates send exactly this markup to browsers.
pub fn message_item(
    message: &Message,
    compact: bool,
    thread_link: bool,
    render: &Render<'_>,
) -> Markup {
    let (name, is_bot) = match &message.author {
        Author::User { display_name, .. } => (display_name.as_str(), false),
        Author::Bot { name, .. } => (name.as_str(), true),
        Author::Removed => ("Former member", false),
    };
    let thread_href = format!("/c/{}/t/{}", message.channel_id, message.id);
    let user_id = match &message.author {
        Author::User { id, .. } => Some(*id),
        _ => None,
    };
    html! {
        li id={ "m" (message.id) } data-message-id=(message.id) data-author=(author_key(&message.author))
            data-channel-id=(message.channel_id) data-user=[user_id] data-pinned[message.pinned_by.is_some()]
            data-saved[render.is_saved(message.id)] data-deleted[message.deleted]
            data-created=(message.created_at) data-compact[compact]
            class="group relative flex gap-3 px-5 py-1 hover:bg-screen data-[compact]:py-0.5 data-[pinned]:bg-amber-50 dark:hover:bg-night-2 dark:data-[pinned]:bg-floor/40" {
            div class="w-9 shrink-0 pt-0.5" {
                div class="group-data-[compact]:hidden" { (avatar(&message.author, render.ctx)) }
            }
            div class="min-w-0 flex-1" {
                @if let Some(pinner) = &message.pinned_by {
                    p class="mb-0.5 flex items-center gap-1 text-xs font-semibold text-amber-800 dark:text-lamp" {
                        (icon(icons::PUSH_PIN, "h-3.5 w-3.5")) "Pinned by " (pinner)
                    }
                }
                div class="flex items-baseline gap-2 group-data-[compact]:hidden" {
                    @if let Author::User { id, status_emoji, .. } = &message.author {
                        a href={ "/people/" (id) } class="font-bold hover:underline" { (name) }
                        @if !status_emoji.is_empty() {
                            span class="text-sm" { (PreEscaped(markup::render(status_emoji, render.ctx))) }
                        }
                    } @else {
                        span class="font-bold" { (name) }
                    }
                    @if is_bot {
                        span class="rounded bg-haint-2 px-1.5 text-xs font-semibold text-floor dark:bg-floor-2 dark:text-haint-2" { "Bot" }
                    }
                    (timestamp(message.created_at))
                }
                @if message.deleted {
                    p class="italic text-muted dark:text-haint" { "This message was deleted." }
                } @else if let Some(poll) = &message.poll {
                    (poll_card(message, poll, render))
                } @else if !message.body.is_empty() {
                    div class="rich" data-body { (render.body(message)) }
                }
                @if let Some(edited) = message.edited_at {
                    span class="text-xs text-muted dark:text-haint" title={ "Edited " (jiff::Timestamp::from_millisecond(edited).unwrap_or_default().strftime("%Y-%m-%d %H:%M UTC")) } { "(edited)" }
                }
                @if let Some(gif) = &message.gif {
                    (gif_card(gif))
                }
                @if let Some(preview) = &message.preview {
                    (preview_card(preview))
                }
                @if !message.buttons.is_empty() && !message.deleted {
                    (button_row(message))
                }
                @for attachment in &message.attachments {
                    (attachment_card(attachment, render))
                }
                @if !message.files.is_empty() {
                    div class="mt-1.5 flex flex-wrap items-start gap-2" {
                        @for file in &message.files { (file_card(file)) }
                    }
                }
                (reactions_bar(message, render))
                @if thread_link {
                    a href=(thread_href) data-reply-count=(message.id)
                        class={ "mt-1 items-center gap-1.5 text-sm font-semibold text-floor-3 hover:underline dark:text-haint "
                            (if message.reply_count > 0 { "inline-flex" } else { "hidden" }) } {
                        (icon(icons::ARROW_BEND_UP_LEFT, "h-4 w-4"))
                        span { (reply_label(message.reply_count)) }
                    }
                }
            }
            div class="absolute -top-3 right-5 hidden overflow-hidden rounded-lg border border-line bg-white text-muted shadow-sm group-hover:flex group-focus-within:flex dark:border-night-line dark:bg-night-2 dark:text-haint" {
                a href={ "/c/" (message.channel_id) "/m/" (message.id) "/react" } data-react=(message.id)
                    aria-label="Add reaction" title="Add reaction" class="p-1.5 hover:bg-screen hover:text-ink dark:hover:bg-night" {
                    (icon(icons::SMILEY, "h-4 w-4"))
                }
                @if thread_link {
                    a href=(thread_href) aria-label="Reply in thread" title="Reply in thread" class="p-1.5 hover:bg-screen hover:text-ink dark:hover:bg-night" {
                        (icon(icons::ARROW_BEND_UP_LEFT, "h-4 w-4"))
                    }
                }
                a href={ "/c/" (message.channel_id) "/m/" (message.id) "/actions" } data-actions=(message.id)
                    aria-label="More actions" title="More actions" class="p-1.5 hover:bg-screen hover:text-ink dark:hover:bg-night" {
                    (icon(icons::DOTS_THREE, "h-4 w-4"))
                }
            }
        }
    }
}

/// A GIF, from the library or shown from its service's URL with the
/// attribution the service asks for.
fn gif_card(gif: &Gif) -> Markup {
    let credit = match gif.provider.as_str() {
        "giphy" => Some("via GIPHY"),
        "klipy" => Some("via KLIPY"),
        _ => None,
    };
    html! {
        figure class="mt-1.5 inline-block max-w-full" {
            img src=(gif.url) alt=(gif.title) title=(gif.title) width=(gif.width) height=(gif.height)
                loading="lazy" referrerpolicy="no-referrer"
                class="block h-auto max-h-64 w-auto max-w-full rounded-lg bg-screen dark:bg-night-2";
            @if let Some(credit) = credit {
                figcaption class="mt-0.5 text-xs text-muted dark:text-haint" { (credit) }
            }
        }
    }
}

/// A poll, drawn for its kind. Everything the viewer did is marked by
/// app.js from data attributes too, since live updates are rendered once
/// for everyone.
fn poll_card(message: &Message, poll: &crate::store::Poll, render: &Render<'_>) -> Markup {
    let author = match &message.author {
        Author::User { id, .. } => Some(*id),
        _ => None,
    };
    let can_close = !poll.closed && render.viewer.is_some_and(|viewer| Some(viewer) == author);
    html! {
        div data-poll=(poll.kind.key()) class="mt-1 max-w-md rounded-xl border border-line p-3 dark:border-night-line" {
            div class="rich mb-2 font-bold" data-body { (render.body(message)) }
            @match poll.kind {
                crate::polls::Kind::Ranked => (ranked_poll(message, poll, render)),
                _ => (choice_poll(message, poll, render)),
            }
            @if !poll.closed {
                form method="post" action={ "/c/" (message.channel_id) "/m/" (message.id) "/close-poll" } data-background
                    data-poll-close=[author] hidden[!can_close] class="mt-1" {
                    button type="submit" class="text-xs font-semibold text-muted underline underline-offset-2 hover:text-ink dark:text-haint" { "End the poll" }
                }
            }
        }
    }
}

/// A poll where people pick one option, or several: an option per button
/// with a bar for its share. Buttons carry their voters, so app.js marks
/// the viewer's own votes in live updates.
fn choice_poll(message: &Message, poll: &crate::store::Poll, render: &Render<'_>) -> Markup {
    let people = poll.voters();
    let total: usize = poll.options.iter().map(|option| option.voters.len()).sum();
    let action = format!("/c/{}/m/{}/vote", message.channel_id, message.id);
    let several = poll.kind == crate::polls::Kind::Multiple;
    html! {
        @if several && !poll.closed {
            p class="-mt-1 mb-2 text-xs text-muted dark:text-haint" { "Pick every option that works for you." }
        }
        @for (index, option) in poll.options.iter().enumerate() {
            @let share = option.voters.len().saturating_mul(100).checked_div(if several { people } else { total }).unwrap_or(0);
            @let mine = render.viewer.is_some_and(|viewer| option.voters.contains(&viewer));
            @let bar = html! {
                span class="absolute inset-y-0 left-0 bg-haint-2 dark:bg-floor-2" style={ "width: " (share) "%" } {}
                span class="relative flex items-center justify-between gap-2 px-3 py-1.5" {
                    span class="flex items-center gap-2" {
                        @if several {
                            span aria-hidden="true" class="poll-check" {}
                        }
                        (option.label)
                    }
                    span class="text-sm font-semibold" { (option.voters.len()) }
                }
            };
            @if poll.closed {
                div data-users=(joined(&option.voters)) title=(option.names.join(", "))
                    class="relative mb-1.5 block w-full overflow-hidden rounded-lg border border-line text-left aria-pressed:border-floor-3 dark:border-night-line" { (bar) }
            } @else {
                form method="post" action=(action) data-background class="mb-1.5" {
                    input type="hidden" name="option" value=(index);
                    button type="submit" data-users=(joined(&option.voters))
                        aria-pressed=(if mine { "true" } else { "false" }) title=(option.names.join(", "))
                        class="relative block w-full overflow-hidden rounded-lg border border-line text-left hover:border-floor-3 aria-pressed:border-floor-3 dark:border-night-line" { (bar) }
                }
            }
        }
        p class="mt-2 text-xs text-muted dark:text-haint" {
            @if several {
                (people) (if people == 1 { " person voted" } else { " people voted" })
            } @else {
                (total) (if total == 1 { " vote" } else { " votes" })
            }
            @if poll.closed { ". Voting has ended." }
            @else if several { ". Pick an option again to take it back." }
            @else { ". Pick again to take your vote back." }
        }
    }
}

fn joined(ids: &[i64]) -> String {
    ids.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

/// A ranked poll: a rank per option, the instant-runoff result, and how the
/// count went round by round.
fn ranked_poll(message: &Message, poll: &crate::store::Poll, render: &Render<'_>) -> Markup {
    let outcome = poll.outcome();
    let last = outcome.rounds.last();
    let valid: usize = last.map_or(0, |round| round.counts.iter().flatten().sum());
    let label = |option: usize| {
        poll.options
            .get(option)
            .map_or("?", |option| option.label.as_str())
    };
    let mine = render.viewer.and_then(|viewer| poll.ballot_of(viewer));
    let ballots: serde_json::Map<String, serde_json::Value> = poll
        .ballots
        .iter()
        .map(|ballot| {
            (
                ballot.user_id.to_string(),
                serde_json::json!(ballot.ranking),
            )
        })
        .collect();
    let out_in: Vec<Option<usize>> = (0..poll.options.len())
        .map(|option| {
            outcome
                .rounds
                .iter()
                .position(|round| round.eliminated.contains(&option))
                .map(|round| round.saturating_add(1))
        })
        .collect();
    let count = poll.options.len();
    let action = format!("/c/{}/m/{}/rank", message.channel_id, message.id);
    html! {
        @if !poll.closed {
            p class="-mt-1 mb-2 text-xs text-muted dark:text-haint" {
                "Rank the options, 1 for your favorite. Rank as few or as many as you like."
            }
        }
        form method="post" action=(action) data-rank-form data-ballots=(serde_json::Value::Object(ballots).to_string()) {
            @for (index, option) in poll.options.iter().enumerate() {
                @let votes = last.and_then(|round| round.counts.get(index).copied().flatten());
                @let share = votes.unwrap_or(0).saturating_mul(100).checked_div(valid).unwrap_or(0);
                @let winner = outcome.winners.contains(&index);
                @let rank = mine.and_then(|ballot| ballot.ranking.iter().position(|&choice| choice == index)).map(|position| position.saturating_add(1));
                div class="relative mb-1.5 flex items-center gap-2 overflow-hidden rounded-lg border border-line py-1 pl-1 pr-3 dark:border-night-line" {
                    span class="absolute inset-y-0 left-0 bg-haint-2 dark:bg-floor-2" style={ "width: " (share) "%" } {}
                    select name={ "r" (index) } aria-label={ "Your rank for " (option.label) } disabled[poll.closed]
                        class="relative rounded-md border border-line bg-white py-0.5 pl-1.5 pr-6 text-sm dark:border-night-line dark:bg-night-2" {
                        option value="" { "–" }
                        @for place in 1..=count {
                            option value=(place) selected[rank == Some(place)] { (place) }
                        }
                    }
                    span class="relative flex-1" { (option.label) }
                    span class="relative text-sm font-semibold" {
                        @if winner && outcome.winners.len() == 1 {
                            span class="rounded bg-lamp px-1.5 py-0.5 text-xs text-ink" { (if poll.closed { "Won" } else { "Leads" }) }
                            " "
                        }
                        @if let Some(votes) = votes {
                            (votes)
                        } @else if let Some(round) = out_in.get(index).copied().flatten() {
                            span class="text-xs font-normal text-muted dark:text-haint" { "out in round " (round) }
                        }
                    }
                }
            }
            @if !poll.closed {
                button type="submit" data-rank-save class="btn-quiet mt-1 px-3 py-1 text-sm" { "Save my ranking" }
            }
        }
        p class="mt-2 text-sm" data-poll-result {
            @match outcome.winners.as_slice() {
                [] => { "No rankings yet." }
                [winner] => {
                    strong { (label(*winner)) }
                    (if poll.closed { " won with " } else { " leads with " })
                    (last.and_then(|round| round.counts.get(*winner).copied().flatten()).unwrap_or(0))
                    " of " (valid) (if valid == 1 { " vote" } else { " votes" })
                    @if outcome.rounds.len() > 1 { " after " (outcome.rounds.len()) " rounds" }
                    "."
                }
                tied => {
                    @for (position, option) in tied.iter().enumerate() {
                        @if position > 0 { @if position.saturating_add(1) == tied.len() { " and " } @else { ", " } }
                        strong { (label(*option)) }
                    }
                    (if poll.closed { " were tied." } else { " are tied." })
                }
            }
        }
        @if outcome.rounds.len() > 1 { (count_details(&outcome, &label)) }
        p class="mt-2 text-xs text-muted dark:text-haint" title=(poll.ballots.iter().map(|ballot| ballot.name.as_str()).collect::<Vec<_>>().join(", ")) {
            (poll.ballots.len()) (if poll.ballots.len() == 1 { " person ranked" } else { " people ranked" })
            @if poll.closed { ". Voting has ended." } @else { ". Change your ranking any time; the winner needs more than half of the votes, counting each person's highest choice still in the running." }
        }
    }
}

/// How an instant-runoff count went, round by round.
fn count_details<'a>(outcome: &crate::polls::Outcome, label: &dyn Fn(usize) -> &'a str) -> Markup {
    html! {
            details class="mt-1 text-sm" {
                summary class="cursor-pointer text-xs font-semibold text-muted dark:text-haint" { "How the votes moved" }
                ol class="mt-1 space-y-1 text-xs" {
                    @for (number, round) in outcome.rounds.iter().enumerate() {
                        li {
                            span class="font-semibold" { "Round " (number.saturating_add(1)) ": " }
                            @for (position, (option, votes)) in round.counts.iter().enumerate().filter_map(|(option, votes)| votes.map(|votes| (option, votes))).enumerate() {
                                @if position > 0 { " · " }
                                (label(option)) " " (votes)
                            }
                            @if round.exhausted > 0 {
                                " (" (round.exhausted) (if round.exhausted == 1 { " ranking had" } else { " rankings had" }) " no options left)"
                            }
                            @if !round.eliminated.is_empty() {
                                ". "
                                @for (position, option) in round.eliminated.iter().enumerate() {
                                    @if position > 0 { " and " }
                                    (label(*option))
                                }
                                (if round.eliminated.len() == 1 { " is out; its votes go to each voter's next choice." } else { " are out; their votes go to each voter's next choice." })
                            }
                        }
                    }
                }
            }
    }
}

/// Buttons an automation put under its message.
fn button_row(message: &Message) -> Markup {
    let action = format!("/c/{}/m/{}/buttons", message.channel_id, message.id);
    html! {
        div class="mt-1.5 flex flex-wrap gap-2" {
            @for (index, button) in message.buttons.iter().enumerate() {
                form method="post" action=(action) data-background {
                    input type="hidden" name="index" value=(index);
                    button type="submit" class=(match button.style.as_str() {
                        "primary" => "btn px-3 py-1 text-sm",
                        "danger" => "btn-quiet px-3 py-1 text-sm text-red-700 dark:text-red-300",
                        _ => "btn-quiet px-3 py-1 text-sm",
                    }) { (button.label) }
                }
            }
        }
    }
}

/// What a message's first link shows. The image loads from the linked
/// site, without a referrer.
fn preview_card(preview: &crate::store::LinkPreview) -> Markup {
    html! {
        div data-preview class="mt-1.5 flex max-w-xl gap-3 rounded-r-lg border-l-4 border-haint bg-screen px-4 py-2.5 dark:border-floor-3 dark:bg-night-2" {
            div class="min-w-0 flex-1" {
                @if let Some(site) = &preview.site {
                    p class="text-xs font-semibold text-muted dark:text-haint" { (site) }
                }
                a href=(preview.url) target="_blank" rel="noopener noreferrer nofollow" class="font-bold text-floor-3 hover:underline dark:text-haint-2" {
                    (preview.title)
                }
                @if let Some(description) = &preview.description {
                    p class="mt-0.5 line-clamp-3 text-sm" { (description) }
                }
            }
            @if let Some(image) = &preview.image {
                img src=(image) alt="" loading="lazy" referrerpolicy="no-referrer"
                    class="h-20 w-20 shrink-0 rounded-md bg-white object-cover dark:bg-night";
            }
        }
    }
}

pub fn timestamp(created_at: i64) -> Markup {
    let when = jiff::Timestamp::from_millisecond(created_at).unwrap_or_default();
    html! {
        time datetime=(when.to_string()) class="text-xs text-muted dark:text-haint" {
            (when.strftime("%H:%M UTC").to_string())
        }
    }
}

/// A person as a message author, for their avatar.
pub fn user_author(user: &User) -> Author {
    Author::User {
        id: user.id,
        display_name: user.display_name.clone(),
        avatar: user.avatar_file_id,
        status_emoji: user.status_emoji.clone(),
    }
}

const AVATAR_TONES: [&str; 6] = [
    "bg-floor-3 text-white",
    "bg-haint text-floor",
    "bg-lamp text-floor",
    "bg-rose-200 text-rose-900",
    "bg-sky-200 text-sky-900",
    "bg-stone-300 text-stone-800",
];

fn avatar(author: &Author, ctx: &Context) -> Markup {
    let base = "flex h-9 w-9 items-center justify-center overflow-hidden rounded-lg font-bold";
    match author {
        Author::User {
            avatar: Some(file_id),
            ..
        } => html! {
            img src={ "/files/" (file_id) } alt="" loading="lazy" class={ (base) " bg-screen object-cover dark:bg-night-2" };
        },
        Author::User {
            id, display_name, ..
        } => {
            let tone = usize::try_from(id.rem_euclid(6))
                .ok()
                .and_then(|index| AVATAR_TONES.get(index))
                .copied()
                .unwrap_or("bg-floor-3 text-white");
            let initial: String = display_name
                .chars()
                .next()
                .unwrap_or('?')
                .to_uppercase()
                .collect();
            html! { div class={ (base) " " (tone) } aria-hidden="true" { (initial) } }
        }
        Author::Bot {
            icon: Some(url), ..
        } if url.starts_with("http") => html! {
            img src=(url) alt="" loading="lazy" referrerpolicy="no-referrer" class={ (base) " bg-screen object-cover dark:bg-night-2" };
        },
        Author::Bot {
            icon: Some(emoji), ..
        } if emoji.starts_with(':') => html! {
            div class={ (base) " bg-screen text-xl dark:bg-night-2" } aria-hidden="true" { (PreEscaped(markup::render(emoji, ctx))) }
        },
        Author::Bot { .. } | Author::Removed => html! {
            div class={ (base) " bg-screen text-floor-3 dark:bg-night-2 dark:text-haint" } aria-hidden="true" {
                (icon(icons::ROBOT, "h-5 w-5"))
            }
        },
    }
}

fn attachment_card(attachment: &Attachment, render: &Render<'_>) -> Markup {
    let color = attachment.color.as_deref().unwrap_or("#B9C7C4");
    html! {
        @if let Some(pretext) = &attachment.pretext {
            div class="rich mt-1" { (render.markup(pretext)) }
        }
        div class="mt-1.5 max-w-2xl rounded-r-lg border-l-4 bg-screen px-4 py-2.5 dark:bg-night-2" style={ "border-left-color: " (color) } {
            @if let Some(author) = &attachment.author_name {
                p class="text-sm font-semibold text-muted dark:text-haint" { (author) }
            }
            @if let Some(title) = &attachment.title {
                p class="font-bold" {
                    @if let Some(link) = &attachment.title_link {
                        a href=(link) target="_blank" rel="noopener noreferrer nofollow" class="underline underline-offset-2" {
                            (render.markup(title))
                        }
                    } @else {
                        (render.markup(title))
                    }
                }
            }
            @if let Some(text) = &attachment.text {
                div class="rich" { (render.markup(text)) }
            }
            @if !attachment.fields.is_empty() {
                dl class="mt-2 grid grid-cols-2 gap-x-6 gap-y-2" {
                    @for field in &attachment.fields {
                        div class=(if field.short { "col-span-1" } else { "col-span-2" }) {
                            dt class="text-sm font-bold" { (render.markup(&field.title)) }
                            dd class="rich" { (render.markup(&field.value)) }
                        }
                    }
                }
            }
            @if let Some(footer) = &attachment.footer {
                p class="mt-2 text-xs text-muted dark:text-haint" { (render.markup(footer)) }
            }
        }
    }
}

fn file_card(file: &FileRef) -> Markup {
    let href = format!("/files/{}", file.id);
    html! {
        @if file.mime.starts_with("image/") {
            a href=(href) target="_blank" class="block overflow-hidden rounded-lg border border-line dark:border-night-line" {
                img src=(href) alt=(file.name) loading="lazy" class="max-h-72 max-w-full object-contain sm:max-w-sm";
            }
        } @else {
            a href=(href) download=(file.name)
                class="flex max-w-xs items-center gap-3 rounded-lg border border-line px-3 py-2 hover:bg-screen dark:border-night-line dark:hover:bg-night-2" {
                (icon(icons::FILE, "h-6 w-6 shrink-0 text-floor-3 dark:text-haint"))
                span class="min-w-0" {
                    span class="block truncate font-semibold" { (file.name) }
                    span class="block text-xs text-muted dark:text-haint" { (human_size(file.size)) }
                }
                (icon(icons::DOWNLOAD_SIMPLE, "ml-auto h-4 w-4 shrink-0 text-muted dark:text-haint"))
            }
        }
    }
}

fn human_size(bytes: i64) -> String {
    const KB: i64 = 1024;
    const MB: i64 = KB * KB;
    if bytes >= MB {
        let tenths = bytes.saturating_mul(10) / MB;
        format!("{}.{} MB", tenths / 10, tenths % 10)
    } else if bytes >= KB {
        format!("{} kB", bytes / KB)
    } else {
        format!("{bytes} bytes")
    }
}

/// A message's reactions. Each chip is a form, so reacting works without
/// JavaScript; app.js submits it in the background instead.
pub fn reactions_bar(message: &Message, render: &Render<'_>) -> Markup {
    let action = format!("/c/{}/m/{}/reactions", message.channel_id, message.id);
    html! {
        div id={ "reactions-" (message.id) } class="flex flex-wrap gap-1 empty:hidden [&:not(:empty)]:mt-1" {
            @for reaction in &message.reactions {
                @let mine = render.viewer.is_some_and(|viewer| reaction.user_ids.contains(&viewer));
                @let users = reaction.user_ids.iter().map(ToString::to_string).collect::<Vec<_>>().join(",");
                form method="post" action=(action) data-reaction-form {
                    button type="submit" name="emoji" value=(reaction.emoji) data-users=(users)
                        aria-pressed=(if mine { "true" } else { "false" })
                        title={ (reaction.names.iter().chain(&reaction.bots).cloned().collect::<Vec<_>>().join(", ")) " reacted with :" (reaction.emoji) ":" }
                        class="flex items-center gap-1 rounded-full border border-line bg-white px-2 py-0.5 text-sm hover:border-floor-3 aria-pressed:border-floor-3 aria-pressed:bg-haint-2 dark:border-night-line dark:bg-night-2 dark:aria-pressed:bg-floor-2" {
                        (PreEscaped(render.ctx.emoji_html(&reaction.emoji).unwrap_or_default()))
                        span class="font-semibold" { (reaction.count()) }
                    }
                }
            }
        }
    }
}

/// Names with their rendered emoji, skipping unknown names.
fn emoji_buttons<'a>(ctx: &Context, names: impl Iterator<Item = &'a str>) -> Vec<(String, String)> {
    names
        .filter_map(|name| ctx.emoji_html(name).map(|html| (name.to_owned(), html)))
        .collect()
}

fn picker_section(title: &str, choices: &[(String, String)], submit: bool) -> Markup {
    html! {
        @if !choices.is_empty() {
            section data-picker-section {
                h3 class="sticky top-0 z-[1] bg-white px-1 py-1 text-xs font-semibold text-muted dark:bg-night-2 dark:text-haint" { (title) }
                div class="grid grid-cols-8 gap-0.5" {
                    @for (name, html) in choices {
                        button type=(if submit { "submit" } else { "button" }) name=[submit.then_some("emoji")] value=[submit.then_some(name.as_str())]
                            data-emoji=(name) data-keywords=(name) title={ ":" (name) ":" } aria-label=(name)
                            class="flex h-8 items-center justify-center rounded-md text-xl hover:bg-screen dark:hover:bg-night" {
                            (PreEscaped(html))
                        }
                    }
                }
            }
        }
    }
}

/// One shared picker per page, opened next to a message by app.js. Your
/// emoji and custom emoji are rendered here; the full catalog loads when
/// the picker first opens.
fn emoji_picker(ctx: &Context, favorites: &[String]) -> Markup {
    let yours = emoji_buttons(ctx, favorites.iter().map(String::as_str));
    let custom = emoji_buttons(ctx, ctx.custom_emoji.keys().map(String::as_str));
    html! {
        div id="emoji-picker" popover
            class="m-0 flex max-h-[26rem] w-80 flex-col rounded-xl border border-line bg-white shadow-xl dark:border-night-line dark:bg-night-2 dark:text-haint-2" {
            div class="border-b border-line p-2 dark:border-night-line" {
                label for="emoji-search" class="sr-only" { "Search emoji" }
                input id="emoji-search" type="search" data-emoji-search placeholder="Search emoji" autocomplete="off"
                    class="field py-1.5 text-sm";
                nav data-emoji-tabs class="mt-1.5 flex justify-between text-lg" aria-label="Emoji categories" {}
            }
            div data-emoji-scroll class="min-h-0 flex-1 overflow-y-auto px-2 pb-2" {
                (picker_section("Your emoji", &yours, false))
                (picker_section("Custom", &custom, false))
                div data-emoji-catalog {}
                p data-emoji-empty hidden class="px-1 py-6 text-center text-sm text-muted dark:text-haint" { "No emoji found." }
            }
            div class="flex justify-between border-t border-line px-3 py-1.5 text-xs dark:border-night-line" {
                a href="/settings/profile" class="text-muted hover:underline dark:text-haint" { "Choose your favorites" }
                a href="/emoji" class="text-muted hover:underline dark:text-haint" { "Add custom emoji" }
            }
        }
    }
}

/// The GIF search popover, opened from a composer by app.js. KLIPY is
/// searched from the browser, so its key and filter ride along.
fn gif_picker(settings: &crate::gifs::Settings) -> Markup {
    let klipy = settings.provider == crate::gifs::Provider::Klipy;
    let placeholder = match settings.provider {
        crate::gifs::Provider::Local => "Search the GIF library",
        crate::gifs::Provider::Giphy => "Search GIPHY",
        crate::gifs::Provider::Klipy => "Search KLIPY",
    };
    html! {
        div id="gif-picker" popover data-provider=(settings.provider.key())
            data-klipy-key=[klipy.then(|| settings.api_key()).flatten()]
            data-klipy-filter=[klipy.then(|| settings.klipy_filter())]
            class="m-0 flex max-h-[28rem] w-96 max-w-[95vw] flex-col rounded-xl border border-line bg-white shadow-xl dark:border-night-line dark:bg-night-2 dark:text-haint-2" {
            div class="border-b border-line p-2 dark:border-night-line" {
                label for="gif-search" class="sr-only" { "Search GIFs" }
                input id="gif-search" type="search" data-gif-search placeholder=(placeholder) autocomplete="off" class="field py-1.5 text-sm";
            }
            div data-gif-results class="grid min-h-0 flex-1 grid-cols-2 gap-1 overflow-y-auto p-2" {}
            p data-gif-status class="px-3 py-2 text-sm text-muted empty:hidden dark:text-haint" {}
            div class="flex items-center justify-between gap-2 border-t border-line px-3 py-1.5 text-xs text-muted dark:border-night-line dark:text-haint" {
                a href="/gifs/library" class="underline" {
                    @if settings.provider == crate::gifs::Provider::Local { "Add GIFs" } @else { "GIF library" }
                }
                span class="font-semibold" { (settings.provider.attribution()) }
            }
        }
    }
}

/// The reaction picker as a page, for browsers without JavaScript.
pub fn react_page(
    shell: &Shell<'_>,
    channel_id: i64,
    message_id: i64,
    ctx: &Context,
    favorites: &[String],
) -> Markup {
    let action = format!("/c/{channel_id}/m/{message_id}/reactions");
    let yours = emoji_buttons(ctx, favorites.iter().map(String::as_str));
    let custom = emoji_buttons(ctx, ctx.custom_emoji.keys().map(String::as_str));
    panel_page(
        "Add reaction",
        shell,
        &html! { "Add a reaction" },
        &html! {
            form method="post" action=(action) class="max-w-md space-y-4" {
                (picker_section("Your emoji", &yours, true))
                (picker_section("Custom", &custom, true))
                @for (category, label) in crate::emoji::CATEGORIES {
                    @let choices: Vec<(String, String)> = crate::emoji::ALL.iter()
                        .filter(|emoji| emoji.category == *category)
                        .map(|emoji| (emoji.name().to_owned(), emoji.glyph.to_owned()))
                        .collect();
                    (picker_section(label, &choices, true))
                }
            }
            a href={ "/c/" (channel_id) } class="btn-quiet mt-6" { "Back to the channel" }
        },
    )
}

// Management pages

pub fn panel_page(title: &str, shell: &Shell<'_>, heading: &Markup, content: &Markup) -> Markup {
    let main = html! {
        main class="flex min-w-0 flex-1 flex-col" {
            header class="flex h-14 shrink-0 items-center gap-2 border-b border-line px-5 dark:border-night-line" {
                (back_to_channels())
                h1 class="min-w-0 text-lg font-bold" { (heading) }
            }
            div class="flex-1 overflow-y-auto" {
                div class="max-w-2xl px-5 py-6" { (content) }
            }
        }
    };
    app_page(title, shell, &PageData::default(), &main)
}

/// A command's answer, shown only to the person who ran it.
pub fn ephemeral_notice(text: &str) -> Markup {
    html! {
        li class="ephemeral mx-3 my-2 rounded-lg border border-dashed border-line bg-screen px-4 py-2 dark:border-night-line dark:bg-night-2" data-ephemeral {
            p class="mb-1 text-xs font-semibold text-muted dark:text-haint" { "Only visible to you" }
            div class="rich" { (PreEscaped(crate::markdown::render(text, &Context::default()))) }
        }
    }
}

/// A panel page with room for side-by-side tools, such as the script editor.
pub fn wide_panel_page(
    title: &str,
    shell: &Shell<'_>,
    heading: &Markup,
    content: &Markup,
) -> Markup {
    let main = html! {
        main class="flex min-w-0 flex-1 flex-col" {
            header class="flex h-14 shrink-0 items-center gap-2 border-b border-line px-5 dark:border-night-line" {
                (back_to_channels())
                h1 class="min-w-0 text-lg font-bold" { (heading) }
            }
            div class="flex-1 overflow-y-auto" {
                div class="max-w-7xl px-5 py-6" { (content) }
            }
        }
    };
    app_page(title, shell, &PageData::default(), &main)
}

pub fn new_channel_page(
    shell: &Shell<'_>,
    error: Option<&str>,
    name: &str,
    private: bool,
) -> Markup {
    panel_page(
        "New channel",
        shell,
        &html! { "Create a channel" },
        &html! {
            (form_error(error))
            form method="post" action="/channels" class="max-w-md" {
                (text_field("Name", "name", "text", name, "off", Some("Lowercase letters, numbers, dashes and underscores, like garden-club.")))
                @if shell.user.may(crate::community::Permission::CreatePrivateChannels) {
                label class="mb-5 flex gap-3" {
                    input type="checkbox" name="private" value="on" checked[private || !shell.user.may(crate::community::Permission::CreateChannels)] class="mt-1";
                    span {
                        span class="block font-semibold" { "Private" }
                        span class="block text-sm text-muted dark:text-haint" { "Only people you add can find and read it. Automations can't see it." }
                    }
                }
                }
                button type="submit" class="btn" { "Create channel" }
            }
        },
    )
}

pub fn copy_row(value: &str) -> Markup {
    html! {
        div class="flex items-center gap-2" {
            code class="min-w-0 flex-1 truncate rounded-lg bg-screen px-3 py-2 text-sm dark:bg-night-2" { (value) }
            button type="button" data-copy=(value) class="btn-quiet shrink-0 text-sm" {
                (icon(icons::COPY, "h-4 w-4")) span data-copy-label { "Copy" }
            }
        }
    }
}

pub fn section(title: &str, intro: &str, content: &Markup) -> Markup {
    html! {
        section class="mb-10" {
            h2 class="text-lg font-bold" { (title) }
            p class="mb-4 mt-1 text-muted dark:text-haint" { (intro) }
            (content)
        }
    }
}

pub fn people_page(
    shell: &Shell<'_>,
    users: &[User],
    invites: &[Invite],
    base_url: &str,
) -> Markup {
    panel_page(
        "People",
        shell,
        &html! { "People" },
        &html! {
            (section("Everyone here", "Start a direct conversation with anyone on this porch.", &html! {
                ul {
                    @for user in users.iter().filter(|user| !user.deactivated) {
                        li class="flex items-center gap-3 border-b border-line py-3 last:border-b-0 dark:border-night-line" {
                            (avatar(&user_author(user), &Context::default()))
                            div class="min-w-0 flex-1" {
                                p class="truncate font-semibold" {
                                    (user.display_name)
                                    @if user.is_admin {
                                        span class="ml-2 rounded bg-haint-2 px-1.5 text-xs font-semibold text-floor dark:bg-floor-2 dark:text-haint-2" { "Admin" }
                                    }
                                }
                                p class="truncate text-sm text-muted dark:text-haint" { "@" (user.username) }
                            }
                            a href={ "/people/" (user.id) } class="btn-quiet text-sm" { "Profile" }
                            a href={ "/dm/" (user.id) } class="btn-quiet text-sm" {
                                (icon(icons::CHAT_CIRCLE_TEXT, "h-4 w-4"))
                                @if user.id == shell.user.id { "Notes to self" } @else { "Message" }
                            }
                        }
                    }
                }
            }))
            @if shell.user.is_admin && users.iter().any(|user| user.deactivated) {
                (section("Deactivated", "They can't sign in. Their messages stay. Reactivate them from their profile.", &html! {
                    ul {
                        @for user in users.iter().filter(|user| user.deactivated) {
                            li class="flex items-center gap-3 border-b border-line py-3 opacity-70 last:border-b-0 dark:border-night-line" {
                                (avatar(&user_author(user), &Context::default()))
                                div class="min-w-0 flex-1" {
                                    p class="truncate font-semibold" { (user.display_name) }
                                    p class="truncate text-sm text-muted dark:text-haint" { "@" (user.username) }
                                }
                                a href={ "/people/" (user.id) } class="btn-quiet text-sm" { "Profile" }
                            }
                        }
                    }
                }))
            }
            @if shell.user.may(crate::community::Permission::InvitePeople) {
                (section("Invite links", "Anyone with an active link can create an account. Links expire after 7 days.", &html! {
                    @if !invites.is_empty() {
                        ul class="mb-5 space-y-4" {
                            @for invite in invites {
                                li class="rounded-xl border border-line p-4 dark:border-night-line" {
                                    div class="mb-2 flex items-center gap-2 text-sm text-muted dark:text-haint" {
                                        (icon(icons::LINK_SIMPLE, "h-4 w-4"))
                                        span {
                                            "Created by " (invite.created_by) ", used " (invite.uses)
                                            (if invite.uses == 1 { " time" } else { " times" })
                                            ", expires "
                                            (timestamp_date(invite.expires_at))
                                        }
                                        form method="post" action={ "/invites/" (invite.token) "/revoke" } class="ml-auto" {
                                            button type="submit" class="btn-quiet text-sm" { "Revoke" }
                                        }
                                    }
                                    (copy_row(&format!("{base_url}/join/{}", invite.token)))
                                }
                            }
                        }
                    }
                    form method="post" action="/invites" {
                        button type="submit" class="btn" { (icon(icons::LINK_SIMPLE, "h-5 w-5")) "Create invite link" }
                    }
                }))
            }
        },
    )
}

fn timestamp_date(ms: i64) -> Markup {
    let when = jiff::Timestamp::from_millisecond(ms).unwrap_or_default();
    html! {
        time datetime=(when.to_string()) data-format="date" { (when.strftime("%Y-%m-%d").to_string()) }
    }
}
