//! Pages about messages: one message's actions, a channel's pins, and what
//! someone saved for later.

use maud::{Markup, html};

use super::{Render, Shell, channel_label, message_item, panel_page};
use crate::{
    icons::{self, icon},
    store::{ActivityItem, Author, Channel, ChannelKind, Located, Message},
};

/// Where a list entry came from, linking to it in place.
fn located_heading(item: &Located) -> Markup {
    let href = crate::routes::message_href(&item.message);
    html! {
        a href=(href) class="mb-1 flex items-center gap-1.5 px-5 text-sm font-semibold text-muted hover:underline dark:text-haint" {
            @if item.is_direct {
                (icon(icons::CHAT_CIRCLE_TEXT, "h-4 w-4")) (item.channel)
            } @else {
                (icon(icons::HASH, "h-4 w-4")) (item.channel)
            }
            @if item.message.parent_id.is_some() { span class="font-normal" { " · in a thread" } }
        }
    }
}

pub fn actions_page(shell: &Shell<'_>, message: &Message, render: &Render<'_>) -> Markup {
    let base = format!("/c/{}/m/{}", message.channel_id, message.id);
    let own = matches!(message.author, Author::User { id, .. } if Some(id) == render.viewer);
    let can_delete =
        !message.deleted && (own || shell.user.may(crate::community::Permission::Moderate));
    panel_page(
        "Message",
        shell,
        &html! { "Message" },
        &html! {
            ol class="-mx-5 mb-6 rounded-xl border border-line py-2 dark:border-night-line" {
                (message_item(message, false, false, render))
            }
            @if own && !message.deleted {
                form method="post" action={ (base) "/edit" } class="mb-6" {
                    label for="edit-body" class="field-label" { "Edit" }
                    textarea id="edit-body" name="body" rows="4" maxlength="10000" required class="field" { (message.body) }
                    button type="submit" class="btn mt-2" { (icon(icons::PENCIL_SIMPLE, "h-5 w-5")) "Save changes" }
                }
            }
            div class="flex flex-wrap gap-2" {
                @if !message.deleted {
                    form method="post" action={ (base) "/pin" } {
                        button type="submit" class="btn-quiet" {
                            (icon(icons::PUSH_PIN, "h-4 w-4"))
                            @if message.pinned_by.is_some() { "Unpin" } @else { "Pin to channel" }
                        }
                    }
                    form method="post" action={ (base) "/save" } {
                        button type="submit" class="btn-quiet" {
                            (icon(icons::BOOKMARK_SIMPLE, "h-4 w-4"))
                            @if render.is_saved(message.id) { "Remove from saved" } @else { "Save for later" }
                        }
                    }
                }
                @if message.preview.is_some() && (own || shell.user.is_admin) {
                    form method="post" action={ (base) "/preview/remove" } {
                        button type="submit" class="btn-quiet" { (icon(icons::X, "h-4 w-4")) "Remove preview" }
                    }
                }
                @if can_delete {
                    form method="post" action={ (base) "/delete" } {
                        button type="submit" class="btn-quiet text-red-700 dark:text-red-300" {
                            (icon(icons::TRASH, "h-4 w-4")) "Delete"
                        }
                    }
                }
            }
            @if !own && !message.deleted && matches!(message.author, Author::User { .. }) {
                details class="mt-6" {
                    summary class="cursor-pointer text-sm font-semibold" { "Report this message" }
                    div class="mt-3" { (super::community::report_form(message)) }
                }
            }
            a href=(crate::routes::message_href(message)) class="btn-quiet mt-6" { (icon(icons::ARROW_LEFT, "h-4 w-4")) "Back to the conversation" }
        },
    )
}

pub fn pins_page(
    shell: &Shell<'_>,
    channel: &Channel,
    messages: &[Message],
    render: &Render<'_>,
) -> Markup {
    panel_page(
        "Pinned",
        shell,
        &html! { "Pinned in " (channel_label(channel)) },
        &html! {
            @if messages.is_empty() {
                p class="text-muted dark:text-haint" {
                    "Nothing is pinned yet. Pin a message from its "
                    (icon(icons::DOTS_THREE, "inline h-4 w-4")) " menu to keep it here for everyone"
                    @if channel.kind != ChannelKind::Direct { " in #" (channel.name) } "."
                }
            } @else {
                ol class="-mx-5 space-y-3" {
                    @for message in messages {
                        li class="list-none" {
                            a href=(crate::routes::message_href(message)) class="mb-1 block px-5 text-sm text-muted hover:underline dark:text-haint" {
                                @if message.parent_id.is_some() { "In a thread · " } "Show in the conversation"
                            }
                            ol { (message_item(message, false, false, render)) }
                        }
                    }
                }
            }
            a href={ "/c/" (channel.id) } class="btn-quiet mt-6" { (icon(icons::ARROW_LEFT, "h-4 w-4")) "Back to the channel" }
        },
    )
}

pub fn saved_page(shell: &Shell<'_>, saved: &[Located], render: &Render<'_>) -> Markup {
    panel_page(
        "Saved",
        shell,
        &html! { "Saved for later" },
        &html! {
            @if saved.is_empty() {
                p class="text-muted dark:text-haint" {
                    "Save messages from their " (icon(icons::DOTS_THREE, "inline h-4 w-4"))
                    " menu to find them here. Only you see what you saved."
                }
            } @else {
                ol class="-mx-5 space-y-4" {
                    @for item in saved {
                        li class="list-none" {
                            (located_heading(item))
                            ol { (message_item(&item.message, false, false, render)) }
                        }
                    }
                }
            }
        },
    )
}

pub fn activity_page(shell: &Shell<'_>, items: &[ActivityItem], render: &Render<'_>) -> Markup {
    panel_page(
        "Activity",
        shell,
        &html! { "Activity" },
        &html! {
            @if items.is_empty() {
                p class="text-muted dark:text-haint" {
                    "When someone mentions you or replies in a thread you're part of, it shows up here."
                }
            } @else {
                ol class="-mx-5 space-y-4" {
                    @for item in items {
                        li class="list-none" data-new[item.new] {
                            p class="mb-0.5 flex items-center gap-2 px-5 text-xs font-semibold uppercase tracking-wide text-muted dark:text-haint" {
                                @if item.mention { (icon(icons::AT, "h-3.5 w-3.5")) "Mentioned you" }
                                @else { (icon(icons::ARROW_BEND_UP_LEFT, "h-3.5 w-3.5")) "Replied in a thread" }
                                @if item.new { span class="rounded bg-lamp px-1.5 text-floor" { "New" } }
                            }
                            (located_heading(&item.located))
                            ol { (message_item(&item.located.message, false, false, render)) }
                        }
                    }
                }
            }
        },
    )
}

/// Sharing from another app: pick a conversation, adjust the text, send.
pub fn share_page(shell: &Shell<'_>, text: &str) -> Markup {
    let sidebar = shell.sidebar;
    panel_page(
        "Share",
        shell,
        &html! { "Share to Sideporch" },
        &html! {
            form method="post" action="/share" class="max-w-xl space-y-4" {
                div {
                    label for="share-channel" class="field-label" { "Send to" }
                    select id="share-channel" name="channel_id" class="field" {
                        @if !sidebar.channels.is_empty() {
                            optgroup label="Channels" {
                                @for item in &sidebar.channels {
                                    option value=(item.channel_id) { "#" (item.label) }
                                }
                            }
                        }
                        @if !sidebar.direct.is_empty() {
                            optgroup label="Direct messages" {
                                @for item in &sidebar.direct {
                                    option value=(item.channel_id) { (item.label) }
                                }
                            }
                        }
                    }
                }
                div {
                    label for="share-body" class="field-label" { "Message" }
                    textarea id="share-body" name="body" rows="6" maxlength="10000" required class="field" { (text) }
                }
                button type="submit" class="btn" { (icon(icons::PAPER_PLANE_RIGHT, "h-5 w-5")) "Send" }
            }
        },
    )
}
