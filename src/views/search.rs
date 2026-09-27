//! The search page.

use maud::{Markup, html};

use super::{Shell, panel_page, timestamp};
use crate::{
    icons::{self, icon},
    store::{Author, SearchHit},
};

pub fn page(shell: &Shell<'_>, query: &str, hits: &[SearchHit]) -> Markup {
    panel_page(
        "Search",
        shell,
        &html! { "Search" },
        &html! {
            form method="get" action="/search" role="search" class="mb-6 flex gap-2" {
                input type="search" name="q" value=(query) autofocus aria-label="Search messages"
                    placeholder="Words from a message, file name or alert" class="field";
                button type="submit" class="btn shrink-0" { (icon(icons::MAGNIFYING_GLASS, "h-5 w-5")) "Search" }
            }
            @if query.is_empty() {
                p class="text-muted dark:text-haint" { "Search every channel and conversation you're part of." }
            } @else if hits.is_empty() {
                p class="text-muted dark:text-haint" { "Nothing matches \u{201c}" (query) "\u{201d}. Try fewer or shorter words." }
            } @else {
                p class="mb-3 text-sm text-muted dark:text-haint" {
                    (hits.len()) (if hits.len() == 1 { " message" } else { " messages" })
                    @if hits.len() >= 50 { ", newest first. Add words to narrow it down." }
                }
                ol class="space-y-2" {
                    @for hit in hits { (result(hit)) }
                }
            }
        },
    )
}

fn result(hit: &SearchHit) -> Markup {
    let message = &hit.message;
    let href = message.parent_id.map_or_else(
        || {
            format!(
                "/c/{}?before={}#m{}",
                message.channel_id,
                message.id.saturating_add(1),
                message.id
            )
        },
        |parent| format!("/c/{}/t/{parent}#m{}", message.channel_id, message.id),
    );
    let author = match &message.author {
        Author::User { display_name, .. } => display_name.as_str(),
        Author::Bot { name, .. } => name.as_str(),
        Author::Removed => "Former member",
    };
    html! {
        li {
            a href=(href) class="block rounded-xl border border-line px-4 py-3 hover:border-floor-3 dark:border-night-line" {
                div class="mb-1 flex items-baseline gap-2 text-sm" {
                    span class="font-semibold text-floor-3 dark:text-haint" {
                        @if hit.is_direct { "Conversation with " (hit.channel) } @else { "#" (hit.channel) }
                    }
                    span class="font-bold" { (author) }
                    (timestamp(message.created_at))
                }
                p class="rich" { (highlighted(&hit.snippet)) }
            }
        }
    }
}

/// Renders an FTS5 snippet, whose matches sit between U+0001 and U+0002.
fn highlighted(snippet: &str) -> Markup {
    let mut parts = snippet.split('\u{1}');
    let first = parts.next().unwrap_or_default();
    html! {
        (first)
        @for part in parts {
            @let (marked, rest) = part.split_once('\u{2}').unwrap_or((part, ""));
            mark class="rounded bg-lamp px-0.5 text-ink" { (marked) }
            (rest)
        }
    }
}
