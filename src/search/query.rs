//! Reads what someone typed into the search box: words, "phrases",
//! `-excluded` words, `OR`, and filters such as `from:ada` or `has:file`.

/// A word or phrase to look for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Term {
    pub text: String,
    /// Written in quotes: the words must appear together, in order.
    pub phrase: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Has {
    File,
    Image,
    Link,
    Poll,
    Gif,
    Reaction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Is {
    Pinned,
    Saved,
    Thread,
}

/// A day, or its whole month.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Day {
    pub date: jiff::civil::Date,
    pub whole_month: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Query {
    /// Every group must match; a group matches when any of its terms does.
    pub groups: Vec<Vec<Term>>,
    pub excluded: Vec<Term>,
    /// Usernames or names, or `me`.
    pub from: Vec<String>,
    /// Channel names, or `@username` for a conversation.
    pub channels: Vec<String>,
    pub has: Vec<Has>,
    pub is: Vec<Is>,
    pub mentions_me: bool,
    pub before: Option<Day>,
    pub after: Option<Day>,
    pub on: Option<Day>,
    /// Filters that couldn't be read, to explain.
    pub problems: Vec<String>,
}

impl Query {
    pub const fn has_words(&self) -> bool {
        !self.groups.is_empty()
    }

    pub const fn has_filters(&self) -> bool {
        !(self.from.is_empty()
            && self.channels.is_empty()
            && self.has.is_empty()
            && self.is.is_empty()
            && !self.mentions_me
            && self.before.is_none()
            && self.after.is_none()
            && self.on.is_none()
            && self.excluded.is_empty())
    }

    pub const fn is_empty(&self) -> bool {
        !self.has_words() && !self.has_filters()
    }

    /// The FTS5 expression for the words: each word matches as a prefix,
    /// phrases match whole, and FTS5 syntax in the input has no effect.
    pub fn fts(&self) -> Option<String> {
        let groups: Vec<String> = self
            .groups
            .iter()
            .map(|group| {
                let terms: Vec<String> = group.iter().map(fts_term).collect();
                if terms.len() == 1 {
                    terms.join("")
                } else {
                    format!("({})", terms.join(" OR "))
                }
            })
            .collect();
        (!groups.is_empty()).then(|| groups.join(" "))
    }

    /// The FTS5 expression matching any excluded word.
    pub fn fts_excluded(&self) -> Option<String> {
        let terms: Vec<String> = self.excluded.iter().map(fts_term).collect();
        (!terms.is_empty()).then(|| terms.join(" OR "))
    }

    /// The single words, for spelling suggestions.
    pub fn words(&self) -> impl Iterator<Item = &str> {
        self.groups
            .iter()
            .flatten()
            .filter(|term| !term.phrase)
            .map(|term| term.text.as_str())
    }

    /// The query with `from` replaced by `to` in its words.
    pub fn replacing(&self, from: &str, to: &str) -> Self {
        let mut query = self.clone();
        for term in query.groups.iter_mut().flatten() {
            if !term.phrase && term.text == from {
                to.clone_into(&mut term.text);
            }
        }
        query
    }

    /// Writes the words back out, for "search instead for" links.
    pub fn words_text(&self) -> String {
        self.groups
            .iter()
            .map(|group| {
                group
                    .iter()
                    .map(|term| {
                        if term.phrase {
                            format!("\"{}\"", term.text)
                        } else {
                            term.text.clone()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" OR ")
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn fts_term(term: &Term) -> String {
    let quoted = term.text.replace('"', "\"\"");
    if term.phrase {
        format!("\"{quoted}\"")
    } else {
        format!("\"{quoted}\"*")
    }
}

/// A piece of the input: text, whether it was quoted, whether it had a `-`.
struct Token {
    text: String,
    quoted: bool,
    negated: bool,
}

fn tokens(input: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut chars = input.chars().peekable();
    while let Some(&next) = chars.peek() {
        if next.is_whitespace() {
            chars.next();
            continue;
        }
        let negated = next == '-';
        if negated {
            chars.next();
        }
        let mut text = String::new();
        let mut quoted = false;
        // A word, which may hold a quoted part: `in:"team chat"` or `"a b"`.
        while let Some(&c) = chars.peek() {
            if c.is_whitespace() {
                break;
            }
            chars.next();
            if matches!(c, '"' | '“' | '”') {
                quoted = true;
                for inner in chars.by_ref() {
                    if matches!(inner, '"' | '“' | '”') {
                        break;
                    }
                    text.push(inner);
                }
            } else {
                text.push(c);
            }
        }
        if !text.trim().is_empty() {
            tokens.push(Token {
                text: text.trim().to_owned(),
                quoted,
                negated,
            });
        }
    }
    tokens
}

/// Reads a day: `2026-09-28`, a month `2026-09`, `today` or `yesterday`.
pub fn parse_day(value: &str, today: jiff::civil::Date) -> Option<Day> {
    match value {
        "today" => Some(Day {
            date: today,
            whole_month: false,
        }),
        "yesterday" => Some(Day {
            date: today.yesterday().ok()?,
            whole_month: false,
        }),
        _ => {
            if let Ok(date) = value.parse::<jiff::civil::Date>() {
                return Some(Day {
                    date,
                    whole_month: false,
                });
            }
            let date = format!("{value}-01").parse::<jiff::civil::Date>().ok()?;
            Some(Day {
                date,
                whole_month: true,
            })
        }
    }
}

/// Reads a search. `today` is the searcher's date, for `on:today`.
pub fn parse(input: &str, today: jiff::civil::Date) -> Query {
    let mut query = Query::default();
    let mut join_next = false;
    for token in tokens(input).into_iter().take(24) {
        if !token.quoted && !token.negated && token.text == "OR" {
            join_next = !query.groups.is_empty();
            continue;
        }
        if !token.negated
            && let Some((key, value)) = token.text.split_once(':')
            && !value.is_empty()
            && filter(&mut query, &key.to_lowercase(), value, today)
        {
            join_next = false;
            continue;
        }
        let term = Term {
            text: token.text.chars().take(100).collect(),
            phrase: token.quoted && token.text.contains(char::is_whitespace),
        };
        if token.negated {
            query.excluded.push(term);
        } else if join_next && let Some(group) = query.groups.last_mut() {
            group.push(term);
        } else {
            query.groups.push(vec![term]);
        }
        join_next = false;
    }
    query
}

/// Applies a `key:value` filter, or returns false if `key` isn't one.
fn filter(query: &mut Query, key: &str, value: &str, today: jiff::civil::Date) -> bool {
    let lower = value.to_lowercase();
    match key {
        "from" => query
            .from
            .push(lower.trim_start_matches('@').to_owned()),
        "in" => query.channels.push(lower.trim_start_matches('#').to_owned()),
        "has" => match lower.as_str() {
            "file" | "files" | "attachment" => query.has.push(Has::File),
            "image" | "images" | "picture" | "photo" => query.has.push(Has::Image),
            "link" | "links" | "url" => query.has.push(Has::Link),
            "poll" | "polls" => query.has.push(Has::Poll),
            "gif" | "gifs" => query.has.push(Has::Gif),
            "reaction" | "reactions" | "emoji" => query.has.push(Has::Reaction),
            _ => query.problems.push(format!(
                "has:{value} isn't a filter; try has:file, has:image, has:link, has:poll, has:gif or has:reaction."
            )),
        },
        "is" => match lower.as_str() {
            "pinned" => query.is.push(Is::Pinned),
            "saved" => query.is.push(Is::Saved),
            "thread" | "reply" => query.is.push(Is::Thread),
            _ => query.problems.push(format!(
                "is:{value} isn't a filter; try is:pinned, is:saved or is:thread."
            )),
        },
        "mentions" if lower == "me" || lower == "@me" => query.mentions_me = true,
        "before" | "after" | "on" | "during" => match parse_day(&lower, today) {
            Some(day) => match key {
                "before" => query.before = Some(day),
                "after" => query.after = Some(day),
                _ => query.on = Some(day),
            },
            None => query.problems.push(format!(
                "{key}:{value} isn't a date; write it like {key}:2026-09-28, {key}:2026-09 or {key}:yesterday."
            )),
        },
        _ => return false,
    }
    true
}

/// The Damerau–Levenshtein distance between two words, or `None` once it
/// exceeds `limit`.
pub fn distance(a: &str, b: &str, limit: usize) -> Option<usize> {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.len().abs_diff(b.len()) > limit {
        return None;
    }
    let width = b.len().saturating_add(1);
    let mut rows: Vec<Vec<usize>> = Vec::with_capacity(a.len().saturating_add(1));
    rows.push((0..width).collect());
    for (i, ca) in a.iter().enumerate() {
        let mut row = Vec::with_capacity(width);
        row.push(i.saturating_add(1));
        let mut best = i.saturating_add(1);
        for (j, cb) in b.iter().enumerate() {
            let above = rows
                .last()
                .and_then(|r| r.get(j.saturating_add(1)))
                .copied()
                .unwrap_or(usize::MAX);
            let diagonal = rows
                .last()
                .and_then(|r| r.get(j))
                .copied()
                .unwrap_or(usize::MAX);
            let left = row.last().copied().unwrap_or(usize::MAX);
            let cost = usize::from(ca != cb);
            let mut value = above
                .saturating_add(1)
                .min(left.saturating_add(1))
                .min(diagonal.saturating_add(cost));
            // A swap of two neighbouring letters costs one.
            if i > 0
                && j > 0
                && a.get(i.saturating_sub(1)) == Some(cb)
                && b.get(j.saturating_sub(1)) == Some(ca)
                && let Some(swapped) = rows
                    .len()
                    .checked_sub(2)
                    .and_then(|index| rows.get(index))
                    .and_then(|r| r.get(j.saturating_sub(1)))
            {
                value = value.min(swapped.saturating_add(1));
            }
            best = best.min(value);
            row.push(value);
        }
        if best > limit {
            return None;
        }
        rows.push(row);
    }
    rows.last()
        .and_then(|row| row.last())
        .copied()
        .filter(|value| *value <= limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn today() -> jiff::civil::Date {
        jiff::civil::date(2026, 9, 29)
    }

    fn words(query: &Query) -> Vec<Vec<&str>> {
        query
            .groups
            .iter()
            .map(|group| group.iter().map(|term| term.text.as_str()).collect())
            .collect()
    }

    #[test]
    fn words_match_as_prefixes_and_operators_have_no_effect() {
        let query = parse("tomato stak", today());
        assert_eq!(query.fts().as_deref(), Some(r#""tomato"* "stak"*"#));
        let query = parse("NOT x* NEAR", today());
        assert_eq!(query.fts().as_deref(), Some(r#""NOT"* "x*"* "NEAR"*"#));
        assert_eq!(parse("  ", today()).fts(), None);
    }

    #[test]
    fn phrases_or_and_exclusions() {
        let query = parse(r#""release notes" pizza OR tacos -anchovies"#, today());
        assert_eq!(
            words(&query),
            vec![vec!["release notes"], vec!["pizza", "tacos"]]
        );
        assert_eq!(
            query.fts().as_deref(),
            Some(r#""release notes" ("pizza"* OR "tacos"*)"#)
        );
        assert_eq!(query.fts_excluded().as_deref(), Some(r#""anchovies"*"#));
        // A leading OR is just a word.
        assert_eq!(words(&parse("OR pizza", today())), vec![vec!["pizza"]]);
    }

    #[test]
    fn filters() {
        let query = parse(
            r#"deploy from:@Ada in:#ops in:"team chat" has:link is:pinned mentions:me after:2026-09-01 on:2026-09"#,
            today(),
        );
        assert_eq!(words(&query), vec![vec!["deploy"]]);
        assert_eq!(query.from, vec!["ada"]);
        assert_eq!(query.channels, vec!["ops", "team chat"]);
        assert_eq!(query.has, vec![Has::Link]);
        assert_eq!(query.is, vec![Is::Pinned]);
        assert!(query.mentions_me);
        assert_eq!(
            query.after.map(|day| day.date),
            Some(jiff::civil::date(2026, 9, 1))
        );
        assert!(query.on.is_some_and(|day| day.whole_month));
        assert!(query.problems.is_empty());
        let query = parse("has:banana before:someday", today());
        assert_eq!(query.problems.len(), 2);
        // Unknown keys stay words, such as times or URLs.
        assert_eq!(
            words(&parse("at 10:30", today())),
            vec![vec!["at"], vec!["10:30"]]
        );
        assert_eq!(
            parse("on:yesterday", today()).on.map(|day| day.date),
            Some(jiff::civil::date(2026, 9, 28))
        );
    }

    #[test]
    fn replacing_a_misspelled_word() {
        let query = parse("tomatoe soup", today());
        let fixed = query.replacing("tomatoe", "tomato");
        assert_eq!(fixed.words_text(), "tomato soup");
    }

    #[test]
    fn distances() {
        assert_eq!(distance("recipe", "recipie", 2), Some(1));
        assert_eq!(distance("teh", "the", 1), Some(1));
        assert_eq!(distance("kitten", "sitting", 2), None);
        assert_eq!(distance("kitten", "sitting", 3), Some(3));
        assert_eq!(distance("", "ab", 2), Some(2));
    }
}
