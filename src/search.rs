//! Full-text search over messages with `SQLite` FTS5.

use axum::extract::{Query, State};
use maud::Markup;
use serde::Deserialize;

use crate::{AppState, auth::CurrentUser, error::AppResult, routes::shell_data, store, views};

const RESULT_LIMIT: u32 = 50;

#[derive(Deserialize)]
pub struct SearchQuery {
    #[serde(default)]
    q: String,
}

/// Turns what someone typed into an FTS5 query: every word must match,
/// as a prefix, and FTS5 operators in the input have no effect.
pub fn fts_query(input: &str) -> Option<String> {
    let terms: Vec<String> = input
        .split_whitespace()
        .take(12)
        .map(|term| format!("\"{}\"*", term.replace('"', "\"\"")))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" "))
}

pub async fn search(
    user: CurrentUser,
    State(state): State<AppState>,
    Query(query): Query<SearchQuery>,
) -> AppResult<Markup> {
    let user_id = user.id;
    let text = query.q.trim().chars().take(200).collect::<String>();
    let fts = fts_query(&text);
    let hits = state
        .db
        .call(move |conn| {
            fts.map_or_else(
                || Ok(Vec::new()),
                |fts| store::search(conn, user_id, &fts, RESULT_LIMIT),
            )
        })
        .await?;
    let sidebar = shell_data(&state, user.id).await?;
    let shell = views::Shell {
        user: &user,
        sidebar: &sidebar,
        current: None,
    };
    Ok(views::search::page(&shell, &text, &hits))
}

#[cfg(test)]
mod tests {
    use super::fts_query;

    #[test]
    fn quotes_every_term_as_a_prefix() {
        assert_eq!(fts_query("  "), None);
        assert_eq!(
            fts_query("tomato stak").as_deref(),
            Some(r#""tomato"* "stak"*"#)
        );
        assert_eq!(
            fts_query(r#"NOT "x OR y*"#).as_deref(),
            Some(r#""NOT"* """x"* "OR"* "y*"*"#)
        );
    }
}
