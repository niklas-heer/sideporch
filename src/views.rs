//! HTML pages, rendered on the server with maud. Styling uses utility
//! classes that build.rs compiles with encre-css.

use axum::http::StatusCode;
use maud::{DOCTYPE, Markup, PreEscaped, html};

use crate::{
    auth::CurrentUser,
    icons::{self, icon},
    markup,
    store::{Author, Channel, ChannelKind, Invite, Message, Sidebar, SidebarItem, User, Webhook},
    webhook::Attachment,
};

pub const ASSET_VERSION: &str = env!("SIDEPORCH_ASSET_VERSION");

/// Consecutive messages from one author within this window share a header.
const GROUP_WINDOW_MS: i64 = 5 * 60 * 1000;

fn document(title: &str, body_class: &str, content: &Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover";
                meta name="theme-color" content="#24403C";
                title { (title) " · Sideporch" }
                link rel="icon" href="/assets/logo.svg" type="image/svg+xml";
                link rel="apple-touch-icon" href="/assets/logo.svg";
                link rel="manifest" href="/manifest.webmanifest";
                link rel="stylesheet" href={ "/assets/app.css?v=" (ASSET_VERSION) };
                script src={ "/assets/app.js?v=" (ASSET_VERSION) } defer {}
            }
            body class=(body_class) { (content) }
        }
    }
}

// Signed-out pages

fn auth_page(title: &str, content: &Markup) -> Markup {
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
    document(title, "bg-floor text-ink antialiased", &page)
}

fn form_error(error: Option<&str>) -> Markup {
    html! {
        @if let Some(error) = error {
            p role="alert" class="mb-4 rounded-lg border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-800 dark:border-red-900 dark:bg-red-950 dark:text-red-200" {
                (error)
            }
        }
    }
}

fn text_field(
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

pub fn login_page(error: Option<&str>, username: &str) -> Markup {
    auth_page(
        "Sign in",
        &html! {
            h1 class="mb-5 text-xl font-bold" { "Sign in" }
            (form_error(error))
            form method="post" action="/login" {
                (text_field("Username", "username", "text", username, "username", None))
                (text_field("Password", "password", "password", "", "current-password", None))
                button type="submit" class="btn mt-2 w-full" { "Sign in" }
            }
            p class="mt-5 text-sm text-muted dark:text-haint" {
                "New here? Ask someone on this porch for an invite link."
            }
        },
    )
}

#[derive(Default)]
pub struct AccountForm {
    pub display_name: String,
    pub username: String,
}

fn account_fields(form: &AccountForm) -> Markup {
    html! {
        (text_field("Your name", "display_name", "text", &form.display_name, "name", Some("Shown next to your messages.")))
        (text_field("Username", "username", "text", &form.username, "username", Some("Used to sign in. Letters, numbers, dots, dashes and underscores.")))
        (text_field("Password", "password", "password", "", "new-password", Some("At least 8 characters.")))
    }
}

pub fn setup_page(token: &str, error: Option<&str>, form: &AccountForm) -> Markup {
    auth_page(
        "Set up",
        &html! {
            h1 class="mb-2 text-xl font-bold" { "Set up your Sideporch" }
            p class="mb-5 text-muted dark:text-haint" {
                "Create the first account. It can invite everyone else and manage channels."
            }
            (form_error(error))
            form method="post" action={ "/setup/" (token) } {
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
        "text-haint-2 hover:bg-floor-2 data-[unread]:font-bold data-[unread]:text-white"
    };
    html! {
        li {
            a href={ "/c/" (item.channel_id) } data-channel-link=(item.channel_id) data-unread[item.unread && !active]
                aria-current=[active.then_some("page")]
                class={ "group flex items-center gap-2 rounded-lg px-3 py-1.5 " (state) } {
                (icon(svg, "h-4 w-4 shrink-0 opacity-70"))
                span class="truncate" { (item.label) }
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
            a href="/" class="flex items-center gap-2.5 px-5 pb-4 pt-5" {
                img src="/assets/logo.svg" alt="" class="h-8 w-8";
                span class="text-lg font-bold tracking-tight text-white" { "Sideporch" }
            }
            div class="flex-1 overflow-y-auto px-3 pb-4" {
                div class="mb-1 mt-2 flex items-center justify-between px-3 text-sm text-haint" {
                    h2 class="font-semibold" { "Channels" }
                    a href="/channels/new" class="rounded-md p-1 hover:bg-floor-2 hover:text-white" aria-label="Create a channel" title="Create a channel" {
                        (icon(icons::PLUS, "h-4 w-4"))
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
            div class="flex items-center gap-1 border-t border-floor-2 px-3 py-3" {
                a href="/people" class="flex min-w-0 flex-1 items-center gap-2 rounded-lg px-2 py-1.5 hover:bg-floor-2" {
                    (icon(icons::USERS, "h-5 w-5 shrink-0"))
                    span class="truncate" { "People" }
                }
                form method="post" action="/logout" {
                    button type="submit" class="rounded-lg p-2 hover:bg-floor-2 hover:text-white" aria-label="Sign out" title={ "Sign out " (shell.user.display_name) } {
                        (icon(icons::SIGN_OUT, "h-5 w-5"))
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
        div id="app" data-me=(shell.user.id) data-channel=[data.channel] data-thread=[data.thread]
            class="flex h-dvh overflow-hidden" {
            (sidebar(shell, false))
            (main)
        }
    };
    document(title, APP_BODY, &page)
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
    document("Channels", APP_BODY, &page)
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
        ChannelKind::Public => icons::HASH,
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
}

pub fn channel_page(shell: &Shell<'_>, view: &ChannelView<'_>) -> Markup {
    let channel = view.channel;
    let thread_open = view.thread.is_some();
    let data = PageData {
        channel: Some(channel.id),
        thread: view.thread.map(|(root, _)| root.id),
    };
    let composer_label = match channel.kind {
        ChannelKind::Public => format!("Message #{}", channel.name),
        ChannelKind::Direct => format!("Message {}", channel.name),
    };
    let column = if thread_open {
        "hidden lg:flex"
    } else {
        "flex"
    };
    let main = html! {
        main class={ "min-w-0 flex-1 flex-col " (column) } {
            header class="flex h-14 shrink-0 items-center gap-2 border-b border-line px-5 dark:border-night-line" {
                (back_to_channels())
                h1 class="min-w-0 text-lg font-bold" { (channel_label(channel)) }
                @if !channel.topic.is_empty() {
                    p class="hidden min-w-0 truncate border-l border-line pl-3 text-sm text-muted sm:block dark:border-night-line dark:text-haint" {
                        (channel.topic)
                    }
                }
                @if channel.kind == ChannelKind::Public {
                    a href={ "/c/" (channel.id) "/settings" } class="ml-auto rounded-lg p-2 text-muted hover:bg-screen hover:text-ink dark:text-haint dark:hover:bg-night-2"
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
                    (message_list(view.messages))
                }
            }
            (composer(&format!("/c/{}/messages", channel.id), None, &composer_label))
        }
    };
    let thread = view
        .thread
        .map(|(root, replies)| thread_panel(channel, root, replies));
    app_page(
        &channel.name,
        shell,
        &data,
        &html! { (main) @if let Some(thread) = thread { (thread) } },
    )
}

fn empty_channel(channel: &Channel) -> Markup {
    html! {
        div class="px-5 pt-10" {
            h2 class="text-xl font-bold" {
                @match channel.kind {
                    ChannelKind::Public => { "This is the start of #" (channel.name) "." }
                    ChannelKind::Direct => { "This is the start of your conversation with " (channel.name) "." }
                }
            }
            @if channel.kind == ChannelKind::Public {
                p class="mt-1 text-muted dark:text-haint" {
                    "Write the first message below, or "
                    a href={ "/c/" (channel.id) "/settings" } class="underline underline-offset-2" { "connect a monitor or bot" }
                    " to post here."
                }
            }
        }
    }
}

fn thread_panel(channel: &Channel, root: &Message, replies: &[Message]) -> Markup {
    html! {
        aside aria-label="Thread" class="flex min-w-0 flex-1 flex-col border-line lg:border-l lg:w-96 lg:flex-none xl:w-[28rem] dark:border-night-line" {
            header class="flex h-14 shrink-0 items-center gap-2 border-b border-line px-5 dark:border-night-line" {
                h2 class="text-lg font-bold" { "Thread" }
                span class="truncate text-sm text-muted dark:text-haint" {
                    @if channel.kind == ChannelKind::Public { "#" } (channel.name)
                }
                a href={ "/c/" (channel.id) } class="ml-auto rounded-lg p-2 text-muted hover:bg-screen hover:text-ink dark:text-haint dark:hover:bg-night-2" aria-label="Close thread" {
                    (icon(icons::X, "h-5 w-5"))
                }
            }
            div id="thread-scroller" class="flex-1 overflow-y-auto py-3" {
                ol { (message_item(root, false, false)) }
                div class="my-2 flex items-center gap-3 px-5 text-sm text-muted dark:text-haint" {
                    span id="thread-count" { (reply_label(root.reply_count)) }
                    span class="h-px flex-1 bg-line dark:bg-night-line" {}
                }
                ol id="replies" { (message_list(replies)) }
            }
            (composer(&format!("/c/{}/messages", channel.id), Some(root.id), "Reply"))
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

fn composer(action: &str, parent: Option<i64>, label: &str) -> Markup {
    html! {
        form data-composer method="post" action=(action) class="shrink-0 px-4 pb-4 pt-2" {
            @if let Some(parent) = parent {
                input type="hidden" name="parent_id" value=(parent);
            }
            div class="flex items-end gap-2 rounded-xl border border-line bg-white py-1.5 pl-3 pr-1.5 focus-within:border-floor-3 dark:border-night-line dark:bg-night-2" {
                textarea name="body" rows="1" required maxlength="10000" aria-label=(label) placeholder=(label)
                    class="max-h-48 min-h-6 flex-1 resize-none bg-transparent py-1.5 leading-6 outline-hidden placeholder:text-muted" {}
                button type="submit" class="btn px-3" aria-label="Send" title="Send (Enter)" {
                    (icon(icons::PAPER_PLANE_RIGHT, "h-5 w-5"))
                }
            }
            p class="mt-1 hidden px-1 text-xs text-red-700 dark:text-red-300" data-composer-error role="alert" {}
        }
    }
}

fn message_list(messages: &[Message]) -> Markup {
    let grouped = std::iter::once(false).chain(messages.windows(2).map(|pair| match pair {
        [previous, message] => {
            author_key(&previous.author) == author_key(&message.author)
                && message.created_at.saturating_sub(previous.created_at) < GROUP_WINDOW_MS
        }
        _ => false,
    }));
    html! {
        @for (message, compact) in messages.iter().zip(grouped) {
            (message_item(message, compact, message.parent_id.is_none()))
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
pub fn message_item(message: &Message, compact: bool, thread_link: bool) -> Markup {
    let (name, is_bot) = match &message.author {
        Author::User { display_name, .. } => (display_name.as_str(), false),
        Author::Bot { name, .. } => (name.as_str(), true),
        Author::Removed => ("Former member", false),
    };
    let thread_href = format!("/c/{}/t/{}", message.channel_id, message.id);
    html! {
        li id={ "m" (message.id) } data-message-id=(message.id) data-author=(author_key(&message.author))
            data-created=(message.created_at) data-compact[compact]
            class="group relative flex gap-3 px-5 py-1 hover:bg-screen data-[compact]:py-0.5 dark:hover:bg-night-2" {
            div class="w-9 shrink-0 pt-0.5" {
                div class="group-data-[compact]:hidden" { (avatar(&message.author)) }
            }
            div class="min-w-0 flex-1" {
                div class="flex items-baseline gap-2 group-data-[compact]:hidden" {
                    span class="font-bold" { (name) }
                    @if is_bot {
                        span class="rounded bg-haint-2 px-1.5 text-xs font-semibold text-floor dark:bg-floor-2 dark:text-haint-2" { "Bot" }
                    }
                    (timestamp(message.created_at))
                }
                @if !message.body.is_empty() {
                    div class="rich" { (PreEscaped(markup::render(&message.body))) }
                }
                @for attachment in &message.attachments {
                    (attachment_card(attachment))
                }
                @if thread_link {
                    a href=(thread_href) data-reply-count=(message.id)
                        class={ "mt-1 items-center gap-1.5 text-sm font-semibold text-floor-3 hover:underline dark:text-haint "
                            (if message.reply_count > 0 { "inline-flex" } else { "hidden" }) } {
                        (icon(icons::ARROW_BEND_UP_LEFT, "h-4 w-4"))
                        span { (reply_label(message.reply_count)) }
                    }
                }
            }
            @if thread_link {
                a href=(thread_href) aria-label="Reply in thread" title="Reply in thread"
                    class="absolute -top-3 right-5 hidden rounded-lg border border-line bg-white p-1.5 text-muted shadow-sm hover:text-ink group-hover:block focus:block dark:border-night-line dark:bg-night-2 dark:text-haint" {
                    (icon(icons::ARROW_BEND_UP_LEFT, "h-4 w-4"))
                }
            }
        }
    }
}

fn timestamp(created_at: i64) -> Markup {
    let when = jiff::Timestamp::from_millisecond(created_at).unwrap_or_default();
    html! {
        time datetime=(when.to_string()) class="text-xs text-muted dark:text-haint" {
            (when.strftime("%H:%M UTC").to_string())
        }
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

fn avatar(author: &Author) -> Markup {
    let base = "flex h-9 w-9 items-center justify-center overflow-hidden rounded-lg font-bold";
    match author {
        Author::User { id, display_name } => {
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
            div class={ (base) " bg-screen text-xl dark:bg-night-2" } aria-hidden="true" { (PreEscaped(markup::render(emoji))) }
        },
        Author::Bot { .. } | Author::Removed => html! {
            div class={ (base) " bg-screen text-floor-3 dark:bg-night-2 dark:text-haint" } aria-hidden="true" {
                (icon(icons::ROBOT, "h-5 w-5"))
            }
        },
    }
}

fn attachment_card(attachment: &Attachment) -> Markup {
    let color = attachment.color.as_deref().unwrap_or("#B9C7C4");
    html! {
        @if let Some(pretext) = &attachment.pretext {
            div class="rich mt-1" { (PreEscaped(markup::render(pretext))) }
        }
        div class="mt-1.5 max-w-2xl rounded-r-lg border-l-4 bg-screen px-4 py-2.5 dark:bg-night-2" style={ "border-left-color: " (color) } {
            @if let Some(author) = &attachment.author_name {
                p class="text-sm font-semibold text-muted dark:text-haint" { (author) }
            }
            @if let Some(title) = &attachment.title {
                p class="font-bold" {
                    @if let Some(link) = &attachment.title_link {
                        a href=(link) target="_blank" rel="noopener noreferrer nofollow" class="underline underline-offset-2" {
                            (PreEscaped(markup::render(title)))
                        }
                    } @else {
                        (PreEscaped(markup::render(title)))
                    }
                }
            }
            @if let Some(text) = &attachment.text {
                div class="rich" { (PreEscaped(markup::render(text))) }
            }
            @if !attachment.fields.is_empty() {
                dl class="mt-2 grid grid-cols-2 gap-x-6 gap-y-2" {
                    @for field in &attachment.fields {
                        div class=(if field.short { "col-span-1" } else { "col-span-2" }) {
                            dt class="text-sm font-bold" { (PreEscaped(markup::render(&field.title))) }
                            dd class="rich" { (PreEscaped(markup::render(&field.value))) }
                        }
                    }
                }
            }
            @if let Some(footer) = &attachment.footer {
                p class="mt-2 text-xs text-muted dark:text-haint" { (PreEscaped(markup::render(footer))) }
            }
        }
    }
}

// Management pages

fn panel_page(title: &str, shell: &Shell<'_>, heading: &Markup, content: &Markup) -> Markup {
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

pub fn new_channel_page(shell: &Shell<'_>, error: Option<&str>, name: &str) -> Markup {
    panel_page(
        "New channel",
        shell,
        &html! { "Create a channel" },
        &html! {
            (form_error(error))
            form method="post" action="/channels" class="max-w-md" {
                (text_field("Name", "name", "text", name, "off", Some("Lowercase letters, numbers, dashes and underscores, like garden-club.")))
                button type="submit" class="btn" { "Create channel" }
            }
        },
    )
}

fn copy_row(value: &str) -> Markup {
    html! {
        div class="flex items-center gap-2" {
            code class="min-w-0 flex-1 truncate rounded-lg bg-screen px-3 py-2 text-sm dark:bg-night-2" { (value) }
            button type="button" data-copy=(value) class="btn-quiet shrink-0 text-sm" {
                (icon(icons::COPY, "h-4 w-4")) span data-copy-label { "Copy" }
            }
        }
    }
}

fn section(title: &str, intro: &str, content: &Markup) -> Markup {
    html! {
        section class="mb-10" {
            h2 class="text-lg font-bold" { (title) }
            p class="mb-4 mt-1 text-muted dark:text-haint" { (intro) }
            (content)
        }
    }
}

pub fn channel_settings_page(
    shell: &Shell<'_>,
    channel: &Channel,
    hooks: &[Webhook],
    base_url: &str,
) -> Markup {
    let example = hooks.first().map_or_else(
        || format!("{base_url}/hooks/…"),
        |hook| format!("{base_url}/hooks/{}", hook.token),
    );
    panel_page(
        "Channel settings",
        shell,
        &html! { (channel_label(channel)) },
        &html! {
            (section("Topic", "A short line shown at the top of the channel.", &html! {
                form method="post" action={ "/c/" (channel.id) "/topic" } class="flex gap-2" {
                    input name="topic" value=(channel.topic) maxlength="200" aria-label="Topic" class="field" placeholder="What's this channel for?";
                    button type="submit" class="btn shrink-0" { "Save topic" }
                }
            }))
            (section("Webhooks", "Monitors, CI systems and scripts can post here through a webhook URL. It accepts Slack's incoming-webhook format, so tools like Gatus and Grafana work unchanged.", &html! {
                @if !hooks.is_empty() {
                    ul class="mb-5 space-y-4" {
                        @for hook in hooks {
                            li class="rounded-xl border border-line p-4 dark:border-night-line" {
                                div class="mb-2 flex items-center gap-2" {
                                    (icon(icons::WEBHOOKS_LOGO, "h-5 w-5 text-floor-3 dark:text-haint"))
                                    span class="font-semibold" { (hook.name) }
                                    form method="post" action={ "/c/" (channel.id) "/webhooks/" (hook.id) "/delete" } class="ml-auto" {
                                        button type="submit" class="btn-quiet text-sm" aria-label={ "Delete webhook " (hook.name) } {
                                            (icon(icons::TRASH, "h-4 w-4")) "Delete"
                                        }
                                    }
                                }
                                (copy_row(&format!("{base_url}/hooks/{}", hook.token)))
                            }
                        }
                    }
                }
                form method="post" action={ "/c/" (channel.id) "/webhooks" } class="flex items-end gap-2" {
                    div class="flex-1" {
                        label for="webhook-name" class="field-label" { "Name" }
                        input id="webhook-name" name="name" required maxlength="80" placeholder="Gatus, Grafana, CI…" class="field";
                    }
                    button type="submit" class="btn shrink-0" { "Create webhook" }
                }
                details class="mt-5 rounded-xl bg-screen p-4 dark:bg-night-2" {
                    summary class="cursor-pointer font-semibold" { "Connect Gatus" }
                    p class="mb-2 mt-2 text-sm" { "Add the webhook URL to Gatus's Slack alert provider:" }
                    pre class="overflow-x-auto rounded-lg bg-white p-3 text-sm dark:bg-night" {
                        code { "alerting:\n  slack:\n    webhook-url: \"" (example) "\"" }
                    }
                }
            }))
        },
    )
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
                    @for user in users {
                        li class="flex items-center gap-3 border-b border-line py-3 last:border-b-0 dark:border-night-line" {
                            (avatar(&Author::User { id: user.id, display_name: user.display_name.clone() }))
                            div class="min-w-0 flex-1" {
                                p class="truncate font-semibold" {
                                    (user.display_name)
                                    @if user.is_admin {
                                        span class="ml-2 rounded bg-haint-2 px-1.5 text-xs font-semibold text-floor dark:bg-floor-2 dark:text-haint-2" { "Admin" }
                                    }
                                }
                                p class="truncate text-sm text-muted dark:text-haint" { "@" (user.username) }
                            }
                            a href={ "/dm/" (user.id) } class="btn-quiet text-sm" {
                                (icon(icons::CHAT_CIRCLE_TEXT, "h-4 w-4"))
                                @if user.id == shell.user.id { "Notes to self" } @else { "Message" }
                            }
                        }
                    }
                }
            }))
            @if shell.user.is_admin {
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
