//! The statistics page.

use maud::{Markup, PreEscaped, html};

use super::{Shell, avatar, panel_page};
use crate::{
    markup::Context,
    statistics::{Bar, Period, Person, Report, Step},
    store::Author,
};

/// `value` with thousands separated, like 12,345.
fn grouped(value: i64) -> String {
    let digits = value.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len().saturating_add(4));
    if value < 0 {
        out.push('-');
    }
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && digits.len().saturating_sub(index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

fn plural(count: i64, one: &str, many: &str) -> String {
    format!("{} {}", grouped(count), if count == 1 { one } else { many })
}

fn bar_label(bar: &Bar, step: Step) -> String {
    let format = match step {
        Step::Day => "%a %-d %b",
        Step::Month => "%b %Y",
        Step::Year => "%Y",
    };
    bar.start.strftime(format).to_string()
}

/// A share of `max` as a whole percentage, for bar widths and heights.
fn share(count: i64, max: i64) -> f64 {
    let as_f64 = |value: i64| f64::from(u32::try_from(value.max(0)).unwrap_or(u32::MAX));
    if max <= 0 {
        0.0
    } else {
        (as_f64(count) / as_f64(max) * 100.0).clamp(0.0, 100.0)
    }
}

/// Messages per bar as an SVG bar chart; each bar names its count on hover.
fn chart(report: &Report) -> Markup {
    const HEIGHT: f64 = 100.0;
    let max = report.bars.iter().map(|bar| bar.count).max().unwrap_or(0);
    let width = report.bars.len().saturating_mul(10).max(10);
    let unit = match report.step {
        Step::Day => "day",
        Step::Month => "month",
        Step::Year => "year",
    };
    let first = report.bars.first().map(|bar| bar_label(bar, report.step));
    let last = report.bars.last().map(|bar| bar_label(bar, report.step));
    html! {
        figure {
            svg viewBox={ "0 0 " (width) " 100" } preserveAspectRatio="none" role="img"
                aria-label={ "Messages per " (unit) ", at most " (grouped(max)) }
                class="h-40 w-full text-floor-3 dark:text-haint" {
                // Closed explicitly: inside SVG, an open `<line>` would swallow the bars.
                line x1="0" y1="99.5" x2=(width) y2="99.5" stroke="currentColor" stroke-opacity="0.25" vector-effect="non-scaling-stroke" {}
                g fill="currentColor" {
                    @for (x, bar) in (0_u32..).step_by(10).zip(&report.bars) {
                        @let height = if bar.count > 0 { (share(bar.count, max) * (HEIGHT - 4.0) / 100.0).max(2.0) } else { 0.0 };
                        rect x=(x.saturating_add(1)) y=(format!("{:.1}", HEIGHT - height)) width="8" height=(format!("{height:.1}")) rx="1.5" {
                            title { (bar_label(bar, report.step)) ": " (plural(bar.count, "message", "messages")) }
                        }
                    }
                }
            }
            figcaption class="mt-1 flex justify-between gap-2 text-xs text-muted dark:text-haint" {
                span { (first.unwrap_or_default()) }
                span { "Busiest " (unit) ": " (plural(max, "message", "messages")) }
                span { (last.unwrap_or_default()) }
            }
        }
    }
}

fn total(label: &str, key: &str, value: i64) -> Markup {
    html! {
        div class="rounded-xl border border-line p-4 dark:border-night-line" {
            p class="text-sm font-semibold text-muted dark:text-haint" { (label) }
            p class="mt-1 text-2xl font-bold" data-stat=(key) { (grouped(value)) }
        }
    }
}

fn section(title: &str, body: &Markup) -> Markup {
    html! {
        section class="rounded-xl border border-line p-4 dark:border-night-line" {
            h2 class="mb-3 font-bold" { (title) }
            (body)
        }
    }
}

fn nobody() -> Markup {
    html! { p class="text-sm text-muted dark:text-haint" { "Nothing yet in this period." } }
}

/// A ranking of people, with a bar for their share of the leader's count.
fn ranking(key: &str, people: &[Person], unit: (&str, &str), ctx: &Context) -> Markup {
    let max = people.first().map_or(0, |person| person.count);
    html! {
        @if people.is_empty() { (nobody()) } @else {
            ol class="space-y-2" data-ranking=(key) {
                @for (place, person) in (1..).zip(people) {
                    li class="flex items-center gap-3" data-person=(person.id) data-count=(person.count) {
                        span class="w-5 shrink-0 text-right text-sm font-semibold text-muted dark:text-haint" { (place) }
                        div class="scale-75" {
                            (avatar(&Author::User {
                                id: person.id,
                                display_name: person.name.clone(),
                                avatar: person.avatar,
                                status_emoji: String::new(),
                            }, ctx))
                        }
                        div class="min-w-0 flex-1" {
                            div class="flex items-baseline justify-between gap-2 text-sm" {
                                a href={ "/people/" (person.id) } class="truncate font-semibold hover:underline" { (person.name) }
                                span class="shrink-0 text-muted dark:text-haint" { (plural(person.count, unit.0, unit.1)) }
                            }
                            div class="mt-1 h-1.5 rounded-full bg-screen dark:bg-night-2" {
                                div class="h-1.5 rounded-full bg-floor-3 dark:bg-haint" style={ "width: " (format!("{:.0}", share(person.count, max))) "%" } {}
                            }
                        }
                    }
                }
            }
        }
    }
}

fn you(report: &Report) -> Markup {
    let you = &report.you;
    html! {
        p class="mb-6 rounded-xl bg-haint-2 px-4 py-3 text-sm text-floor dark:bg-floor-2 dark:text-haint-2" data-you {
            @if you.hidden {
                "You wrote " (plural(you.messages, "message", "messages")) " in this period. "
                "You left the rankings, so you're counted but not listed. "
                a href="/settings/profile#rankings" class="underline" { "Change that" }
            } @else if let Some(rank) = you.rank {
                "You wrote " (plural(you.messages, "message", "messages")) " in this period. "
                strong { "You rank #" (rank) } " of " (plural(you.of, "person", "people")) "."
            } @else {
                "You haven't written in public channels in this period yet."
            }
        }
    }
}

pub fn page(shell: &Shell<'_>, report: &Report, ctx: &Context) -> Markup {
    panel_page(
        "Statistics",
        shell,
        &html! { "Statistics" },
        &html! {
            nav class="mb-4 flex flex-wrap gap-2" aria-label="Period" {
                @for period in Period::ALL {
                    a href={ "/statistics?period=" (period.key()) } aria-current=[(period == report.period).then_some("page")]
                        class="rounded-lg px-3 py-1.5 text-sm font-semibold hover:bg-screen aria-[current=page]:bg-haint-2 aria-[current=page]:text-floor dark:hover:bg-night-2 dark:aria-[current=page]:bg-floor-2 dark:aria-[current=page]:text-haint-2" {
                        (period.label())
                    }
                }
            }
            (you(report))
            div class="mb-6 grid grid-cols-2 gap-4 sm:grid-cols-4" {
                (total("Messages", "messages", report.messages))
                (total("People who wrote", "people", report.people))
                (total("Reactions", "reactions", report.reactions))
                (total("Files", "files", report.files))
            }
            div class="mb-6" {
                (section(match report.step { Step::Day => "Messages per day", Step::Month => "Messages per month", Step::Year => "Messages per year" }, &chart(report)))
            }
            div class="grid gap-4 sm:grid-cols-2" {
                (section("Most messages", &ranking("messages", &report.writers, ("message", "messages"), ctx)))
                (section("Most reactions received", &ranking("reactions", &report.appreciated, ("reaction", "reactions"), ctx)))
                (section("Busiest channels", &html! {
                    @if report.channels.is_empty() { (nobody()) } @else {
                        ol class="space-y-1.5 text-sm" data-ranking="channels" {
                            @for (id, name, count) in &report.channels {
                                li class="flex justify-between gap-2" data-channel=(id) data-count=(count) {
                                    a href={ "/c/" (id) } class="truncate font-semibold hover:underline" { "#" (name) }
                                    span class="shrink-0 text-muted dark:text-haint" { (plural(*count, "message", "messages")) }
                                }
                            }
                        }
                    }
                }))
                (section("Most used reactions", &html! {
                    @if report.emoji.is_empty() { (nobody()) } @else {
                        ol class="flex flex-wrap gap-2" data-ranking="emoji" {
                            @for (name, count) in &report.emoji {
                                li class="flex items-center gap-1.5 rounded-full border border-line px-2.5 py-1 text-sm dark:border-night-line" title={ ":" (name) ":" } data-count=(count) {
                                    span class="text-lg leading-none" { (PreEscaped(ctx.emoji_html(name).unwrap_or_else(|| format!(":{name}:")))) }
                                    span class="text-muted dark:text-haint" { (grouped(*count)) }
                                }
                            }
                        }
                    }
                }))
            }
            p class="mt-6 text-xs text-muted dark:text-haint" {
                "Only public channels count; private channels and direct messages never do. "
                "Bots, automations and people from other servers count in the totals but aren't ranked. "
                "Anyone can leave the rankings from " a href="/settings/profile#rankings" class="underline" { "their profile" } "."
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::grouped;

    #[test]
    fn groups_thousands() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1_000), "1,000");
        assert_eq!(grouped(1_234_567), "1,234,567");
        assert_eq!(grouped(-4_200), "-4,200");
    }
}
