//! The search page.

use std::fmt::Write as _;

use maud::{Markup, html};

use super::{Shell, panel_page, timestamp};
use crate::{
    icons::{self, icon},
    search::{Hit, Named, PAGE, Results, Sort},
    store::Author,
};

/// Shortcuts that add a filter to the search.
const SHORTCUTS: &[(&str, &str)] = &[
    ("from:me", "From me"),
    ("mentions:me", "Mentioning me"),
    ("has:file", "Files"),
    ("has:image", "Images"),
    ("has:link", "Links"),
    ("has:poll", "Polls"),
    ("is:pinned", "Pinned"),
    ("is:saved", "Saved"),
    ("after:yesterday", "Today"),
];

/// Everything the search box understands, for the tips.
const TIPS: &[(&str, &str)] = &[
    (
        "tomato soup",
        "messages with words starting like these, in any order",
    ),
    ("\"release notes\"", "these words together, in this order"),
    ("pizza OR tacos", "either word"),
    ("-anchovies", "leave out messages with this word"),
    ("from:ada", "messages from someone (from:me for yours)"),
    (
        "in:#ops, in:@ada",
        "in a channel, or your conversation with someone",
    ),
    (
        "has:file, has:image, has:link, has:poll, has:gif, has:reaction",
        "messages with these",
    ),
    (
        "is:pinned, is:saved, is:thread",
        "pinned, saved for later, or in a thread",
    ),
    ("mentions:me", "messages that mention you"),
    (
        "before:2026-09-01, after:yesterday, on:2026-09",
        "by date, in your time zone",
    ),
];

/// Percent-encodes a query string value.
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

fn href(query: &str, sort: Option<Sort>, page: usize) -> String {
    let mut href = format!("/search?q={}", encode(query));
    match sort {
        Some(Sort::Newest) => href.push_str("&sort=newest"),
        Some(Sort::Relevance) => href.push_str("&sort=relevance"),
        None => {}
    }
    if page > 0 {
        // Writing to a String cannot fail.
        let _ = write!(href, "&page={page}");
    }
    href
}

/// The query with `filter` added, or taken out when it is already there.
fn toggled(query: &str, filter: &str) -> (String, bool) {
    let words: Vec<&str> = query.split_whitespace().collect();
    if words.iter().any(|word| word.eq_ignore_ascii_case(filter)) {
        let rest: Vec<&str> = words
            .into_iter()
            .filter(|word| !word.eq_ignore_ascii_case(filter))
            .collect();
        (rest.join(" "), true)
    } else if query.trim().is_empty() {
        (filter.to_owned(), false)
    } else {
        (format!("{} {filter}", query.trim()), false)
    }
}

pub fn page(shell: &Shell<'_>, query: &str, results: &Results) -> Markup {
    panel_page(
        "Search",
        shell,
        &html! { "Search" },
        &html! {
            form method="get" action="/search" role="search" class="mb-3 flex gap-2" {
                input type="search" name="q" value=(query) autofocus aria-label="Search messages"
                    placeholder="Words, \"a phrase\", from:someone, in:#channel, has:file" class="field";
                select name="sort" aria-label="Order" class="field w-auto shrink-0" {
                    option value="relevance" selected[results.sort == Sort::Relevance] { "Best match" }
                    option value="newest" selected[results.sort == Sort::Newest] { "Newest" }
                }
                button type="submit" class="btn shrink-0" { (icon(icons::MAGNIFYING_GLASS, "h-5 w-5")) span class="hidden sm:inline" { "Search" } }
            }
            div class="mb-5 flex flex-wrap gap-1.5" {
                @for (filter, label) in SHORTCUTS {
                    @let (next, on) = toggled(query, filter);
                    a href=(href(&next, None, 0)) aria-pressed=(if on { "true" } else { "false" })
                        class="rounded-full border border-line px-3 py-0.5 text-sm hover:border-floor-3 aria-pressed:border-floor-3 aria-pressed:bg-haint-2 dark:border-night-line dark:aria-pressed:bg-floor-2" {
                        (label)
                    }
                }
            }
            @for notice in &results.notices {
                p class="mb-3 rounded-lg bg-screen px-3 py-2 text-sm dark:bg-night-2" role="status" { (notice) }
            }
            @if let Some(corrected) = &results.corrected {
                p class="mb-4 text-sm" {
                    "Showing results for "
                    a href=(href(corrected, None, 0)) class="font-bold underline underline-offset-2" { (corrected) }
                    ". Nothing matched \u{201c}" (query) "\u{201d}."
                }
            }
            @if !results.channels.is_empty() || !results.people.is_empty() {
                ul class="mb-5 grid gap-2 sm:grid-cols-2" {
                    @for named in results.channels.iter().chain(&results.people) { (named_card(named)) }
                }
            }
            @if query.is_empty() {
                (tips(true))
            } @else if results.hits.is_empty() {
                @if results.page > 0 {
                    p class="text-muted dark:text-haint" { "No more messages." }
                } @else if results.notices.is_empty() {
                    p class="mb-6 text-muted dark:text-haint" { "No messages match \u{201c}" (query) "\u{201d}. Try fewer or other words, or fewer filters." }
                    (tips(false))
                }
            } @else {
                p class="mb-3 text-sm text-muted dark:text-haint" {
                    @let first = results.page.saturating_mul(PAGE).saturating_add(1);
                    @let last = results.page.saturating_mul(PAGE).saturating_add(results.hits.len());
                    @if results.page == 0 && !results.more {
                        (results.hits.len()) (if results.hits.len() == 1 { " message" } else { " messages" })
                    } @else {
                        "Messages " (first) "–" (last)
                    }
                    (match results.sort { Sort::Relevance => ", best matches first.", Sort::Newest => ", newest first." })
                }
                ol class="space-y-2" {
                    @for hit in &results.hits { (result(hit)) }
                }
                nav aria-label="More results" class="mt-5 flex justify-between gap-2" {
                    @if results.page > 0 {
                        a href=(href(query, Some(results.sort), results.page.saturating_sub(1))) class="btn-quiet" { "Previous" }
                    } @else { span {} }
                    @if results.more {
                        a href=(href(query, Some(results.sort), results.page.saturating_add(1))) class="btn-quiet" { "More results" }
                    }
                }
            }
        },
    )
}

fn tips(open: bool) -> Markup {
    html! {
        details open[open] class="rounded-xl border border-line p-4 text-sm dark:border-night-line" {
            summary class="cursor-pointer font-semibold" { "Search tips" }
            p class="mb-2 mt-2 text-muted dark:text-haint" { "Search every channel and conversation you're part of. Combine any of these:" }
            dl class="grid gap-x-4 gap-y-1.5 sm:grid-cols-[auto_1fr]" {
                @for (example, meaning) in TIPS {
                    dt { code class="rounded bg-screen px-1.5 py-0.5 text-xs dark:bg-night" { (example) } }
                    dd class="text-muted dark:text-haint" { (meaning) }
                }
            }
        }
    }
}

fn named_card(named: &Named) -> Markup {
    html! {
        li {
            a href=(named.href) class="flex items-center gap-3 rounded-xl border border-line px-4 py-2.5 hover:border-floor-3 dark:border-night-line" {
                (icon(if named.person { icons::USER_CIRCLE } else { icons::HASH }, "h-5 w-5 shrink-0 text-muted dark:text-haint"))
                span class="min-w-0" {
                    span class="block truncate font-semibold" { (named.label) }
                    @if !named.detail.is_empty() {
                        span class="block truncate text-xs text-muted dark:text-haint" { (named.detail) }
                    }
                }
            }
        }
    }
}

fn result(hit: &Hit) -> Markup {
    let message = &hit.message;
    let href = crate::routes::message_href(message);
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
                    @if message.parent_id.is_some() {
                        span class="text-xs text-muted dark:text-haint" { "in a thread" }
                    }
                }
                p class="rich" { (highlighted(&hit.snippet)) }
                @if !message.files.is_empty() {
                    p class="mt-1 text-xs text-muted dark:text-haint" {
                        (icon(icons::PAPERCLIP, "mr-1 inline h-3.5 w-3.5"))
                        (message.files.iter().map(|file| file.name.as_str()).collect::<Vec<_>>().join(", "))
                    }
                }
            }
        }
    }
}

/// Drops Markdown's emphasis and code markers, which snippets of message
/// text would otherwise show as they were typed.
fn without_markdown(snippet: &str) -> String {
    ["**", "__", "~~", "`"]
        .iter()
        .fold(snippet.to_owned(), |text, marker| text.replace(marker, ""))
}

/// Renders an FTS5 snippet, whose matches sit between U+0001 and U+0002.
fn highlighted(snippet: &str) -> Markup {
    let snippet = without_markdown(snippet);
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

/// A snippet without its match marks, for plain-text suggestions.
pub fn plain_snippet(snippet: &str) -> String {
    without_markdown(snippet).replace(['\u{1}', '\u{2}'], "")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippets_hide_markdown_markers() {
        assert_eq!(
            plain_snippet("The **v2.3 \u{1}release\u{2}** notes, `make` and ~~old~~"),
            "The v2.3 release notes, make and old"
        );
        let html = highlighted("**\u{1}bold\u{2}**").into_string();
        assert!(html.contains("<mark") && !html.contains("**"), "{html}");
    }

    #[test]
    fn shortcuts_add_and_remove_filters() {
        assert_eq!(
            toggled("soup", "has:file"),
            ("soup has:file".to_owned(), false)
        );
        assert_eq!(
            toggled("soup has:file", "has:file"),
            ("soup".to_owned(), true)
        );
        assert_eq!(toggled("", "is:pinned"), ("is:pinned".to_owned(), false));
        assert_eq!(
            href("a b&c", Some(Sort::Newest), 2),
            "/search?q=a%20b%26c&sort=newest&page=2"
        );
    }
}
