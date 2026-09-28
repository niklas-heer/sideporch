//! Every standard emoji, from GitHub's gemoji (see `assets/vendor/`).
//!
//! Gives `:shortcode:` names to the renderer and the catalog the reaction
//! picker loads, grouped by category in Unicode's order.

use std::{collections::HashMap, sync::LazyLock};

use serde_json::{Value, json};

const TABLE: &str = include_str!("../assets/vendor/emoji.tsv");

/// The picker's categories, in order, with short labels.
pub const CATEGORIES: &[(&str, &str)] = &[
    ("Smileys & Emotion", "Smileys"),
    ("People & Body", "People"),
    ("Animals & Nature", "Nature"),
    ("Food & Drink", "Food"),
    ("Travel & Places", "Travel"),
    ("Activities", "Activities"),
    ("Objects", "Objects"),
    ("Symbols", "Symbols"),
    ("Flags", "Flags"),
];

/// Names chat apps use that gemoji spells differently.
const EXTRA_NAMES: &[(&str, &str)] = &[
    ("thinking_face", "🤔"),
    ("hugging_face", "🤗"),
    ("robot_face", "🤖"),
    ("helmet_with_white_cross", "⛑️"),
    ("large_green_circle", "🟢"),
    ("large_yellow_circle", "🟡"),
    ("thumbsup", "👍"),
    ("thumbsdown", "👎"),
];

pub struct Emoji {
    pub glyph: &'static str,
    pub category: &'static str,
    /// The first alias is the main name.
    pub names: Vec<&'static str>,
    pub keywords: String,
}

impl Emoji {
    pub fn name(&self) -> &'static str {
        self.names.first().copied().unwrap_or_default()
    }
}

pub static ALL: LazyLock<Vec<Emoji>> = LazyLock::new(|| {
    TABLE
        .lines()
        .filter_map(|line| {
            let mut columns = line.split('\t');
            let glyph = columns.next()?;
            let category = columns.next()?;
            let names: Vec<&str> = columns
                .next()?
                .split(' ')
                .filter(|name| !name.is_empty())
                .collect();
            let tags = columns.next().unwrap_or_default();
            let description = columns.next().unwrap_or_default();
            // Descriptions and tags are only needed for search, so keep one string.
            (!names.is_empty()).then(|| Emoji {
                glyph,
                category,
                names,
                keywords: format!("{description} {tags}"),
            })
        })
        .collect()
});

static BY_NAME: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    let mut names: HashMap<&str, &str> = ALL
        .iter()
        .flat_map(|emoji| emoji.names.iter().map(|name| (*name, emoji.glyph)))
        .collect();
    for (name, glyph) in EXTRA_NAMES {
        names.entry(name).or_insert(glyph);
    }
    names
});

/// The emoji for a shortcode name, such as `tada`.
pub fn lookup(name: &str) -> Option<&'static str> {
    BY_NAME.get(name).copied()
}

/// The picker's catalog: categories with `[name, glyph, keywords]` entries.
pub static CATALOG: LazyLock<String> = LazyLock::new(|| {
    let categories: Vec<Value> = CATEGORIES
        .iter()
        .map(|(category, label)| {
            let entries: Vec<Value> = ALL
                .iter()
                .filter(|emoji| emoji.category == *category)
                .map(|emoji| {
                    let other_names = emoji.names.get(1..).unwrap_or_default().join(" ");
                    json!([
                        emoji.name(),
                        emoji.glyph,
                        format!("{other_names} {}", emoji.keywords).trim()
                    ])
                })
                .collect();
            json!({ "name": label, "emoji": entries })
        })
        .collect();
    json!({ "categories": categories }).to_string()
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_every_emoji_by_every_name() {
        assert!(ALL.len() > 1_800);
        assert_eq!(lookup("tada"), Some("🎉"));
        assert_eq!(lookup("+1"), Some("👍"));
        assert_eq!(lookup("thumbsup"), Some("👍"));
        assert_eq!(lookup("thinking_face"), Some("🤔"));
        assert_eq!(lookup("flag-de").or_else(|| lookup("de")), Some("🇩🇪"));
        assert_eq!(lookup("nope"), None);
        for (category, _) in CATEGORIES {
            assert!(
                ALL.iter().any(|emoji| emoji.category == *category),
                "{category}"
            );
        }
        assert!(
            CATALOG.starts_with(r#"{"categories":[{"emoji":[["grinning","😀""#)
                || CATALOG.contains(r#""name":"Smileys""#)
        );
    }
}
