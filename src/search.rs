//! Search over messages with `SQLite` FTS5, plus people and channels.
//!
//! Words match as prefixes; results are ranked by BM25, weighted towards
//! recent messages, or sorted newest first. Filters narrow the messages
//! without words too. When nothing matches, a misspelled word is corrected
//! from the index's own vocabulary.

use axum::{
    Json,
    extract::{Query as QueryString, State},
};
use maud::Markup;
use rusqlite::{Connection, types::Value};
use serde::Deserialize;

use crate::{
    AppState,
    auth::CurrentUser,
    error::AppResult,
    routes::shell_data,
    store::{self, Message},
    views,
};

pub mod query;

use query::{Day, Has, Is, Query};

pub const PAGE: usize = 30;
/// Relevance halves for a message this old.
const RECENCY_HALF_MS: f64 = 90.0 * 24.0 * 60.0 * 60.0 * 1000.0;

#[derive(Deserialize, Default)]
pub struct SearchParams {
    #[serde(default)]
    q: String,
    /// `newest`, or relevance.
    #[serde(default)]
    sort: String,
    #[serde(default)]
    page: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    Relevance,
    Newest,
}

pub struct Hit {
    pub message: Message,
    pub channel: String,
    pub is_direct: bool,
    /// Marks matches with U+0001 and U+0002.
    pub snippet: String,
}

/// A person or channel whose name matches the words.
pub struct Named {
    pub href: String,
    pub label: String,
    pub detail: String,
    pub person: bool,
}

pub struct Results {
    pub hits: Vec<Hit>,
    /// Whether there are more after this page.
    pub more: bool,
    pub sort: Sort,
    pub page: usize,
    pub people: Vec<Named>,
    pub channels: Vec<Named>,
    /// Filters that couldn't be read, or named nobody.
    pub notices: Vec<String>,
    /// When the words matched nothing: the corrected search, shown instead.
    pub corrected: Option<String>,
}

/// Filters resolved against the database.
struct Resolved {
    authors: Vec<i64>,
    channels: Vec<i64>,
    /// A filter named nobody or nothing, so nothing can match.
    impossible: bool,
}

fn find_person(conn: &Connection, user_id: i64, name: &str) -> Option<i64> {
    if name == "me" {
        return Some(user_id);
    }
    conn.query_row(
        "SELECT id FROM users WHERE username = ?1 OR display_name = ?1 COLLATE NOCASE
         ORDER BY username != ?1 LIMIT 1",
        [name],
        |row| row.get(0),
    )
    .ok()
}

/// A public channel `user_id` can see, or their conversation with
/// `@username`.
fn find_channel(conn: &Connection, user_id: i64, name: &str) -> Option<i64> {
    name.strip_prefix('@').map_or_else(
        || {
            conn.query_row(
                "SELECT c.id FROM channels c WHERE c.kind = 'public' AND c.name = ?2
                   AND (c.private = 0 OR EXISTS (SELECT 1 FROM channel_members m WHERE m.channel_id = c.id AND m.user_id = ?1))",
                rusqlite::params![user_id, name],
                |row| row.get(0),
            )
            .ok()
        },
        |username| {
            conn.query_row(
                "SELECT c.id FROM channels c
                 JOIN channel_members a ON a.channel_id = c.id AND a.user_id = ?1
                 JOIN channel_members b ON b.channel_id = c.id
                 JOIN users u ON u.id = b.user_id AND u.username = ?2
                 WHERE c.kind = 'dm' LIMIT 1",
                rusqlite::params![user_id, username],
                |row| row.get(0),
            )
            .ok()
        },
    )
}

fn resolve(conn: &Connection, user_id: i64, query: &Query, notices: &mut Vec<String>) -> Resolved {
    let mut resolved = Resolved {
        authors: Vec::new(),
        channels: Vec::new(),
        impossible: false,
    };
    for name in &query.from {
        if let Some(id) = find_person(conn, user_id, name) {
            resolved.authors.push(id);
        } else {
            notices.push(format!("Nobody here is called “{name}”."));
            resolved.impossible = true;
        }
    }
    for name in &query.channels {
        if let Some(id) = find_channel(conn, user_id, name) {
            resolved.channels.push(id);
        } else {
            notices.push(format!("There's no channel “{name}” you can see."));
            resolved.impossible = true;
        }
    }
    resolved
}

/// The first and last millisecond of a day or month in `zone`.
fn bounds(day: Day, zone: &jiff::tz::TimeZone) -> Option<(i64, i64)> {
    let start = if day.whole_month {
        day.date.first_of_month()
    } else {
        day.date
    };
    let end = if day.whole_month {
        day.date.last_of_month().tomorrow().ok()?
    } else {
        day.date.tomorrow().ok()?
    };
    let ms = |date: jiff::civil::Date| {
        date.to_zoned(zone.clone())
            .ok()
            .map(|zoned| zoned.timestamp().as_millisecond())
    };
    Some((ms(start)?, ms(end)?))
}

/// A `WHERE` clause and its parameters. `?1` is the searcher.
struct Sql {
    params: Vec<Value>,
    conditions: Vec<String>,
}

impl Sql {
    /// Adds a parameter and returns its placeholder.
    fn bind(&mut self, value: Value) -> String {
        self.params.push(value);
        format!("?{}", self.params.len())
    }

    fn and(&mut self, condition: impl Into<String>) {
        self.conditions.push(condition.into());
    }
}

/// Adds the conditions for everything but the words.
fn filters(sql: &mut Sql, query: &Query, resolved: &Resolved, zone: &jiff::tz::TimeZone) {
    if let Some(excluded) = query.fts_excluded() {
        let excluded = sql.bind(Value::Text(excluded));
        sql.and(format!(
            "m.id NOT IN (SELECT rowid FROM messages_fts WHERE messages_fts MATCH {excluded})"
        ));
    }
    let list =
        |ids: &[i64]| Value::Text(serde_json::to_string(ids).unwrap_or_else(|_| "[]".to_owned()));
    if !resolved.authors.is_empty() {
        let authors = sql.bind(list(&resolved.authors));
        sql.and(format!(
            "m.user_id IN (SELECT value FROM json_each({authors}))"
        ));
    }
    if !resolved.channels.is_empty() {
        let channels = sql.bind(list(&resolved.channels));
        sql.and(format!(
            "m.channel_id IN (SELECT value FROM json_each({channels}))"
        ));
    }
    for has in &query.has {
        sql.and(match has {
            Has::File => "EXISTS (SELECT 1 FROM message_files mf WHERE mf.message_id = m.id)",
            Has::Image => "(m.gif IS NOT NULL OR EXISTS (SELECT 1 FROM message_files mf JOIN files fi ON fi.id = mf.file_id
                            WHERE mf.message_id = m.id AND fi.mime LIKE 'image/%'))",
            Has::Link => "(m.body LIKE '%://%' OR m.preview IS NOT NULL)",
            Has::Poll => "m.poll IS NOT NULL",
            Has::Gif => "m.gif IS NOT NULL",
            Has::Reaction => "EXISTS (SELECT 1 FROM reactions r WHERE r.message_id = m.id)",
        });
    }
    for is in &query.is {
        sql.and(match is {
            Is::Pinned => "m.pinned_at IS NOT NULL",
            Is::Saved => "EXISTS (SELECT 1 FROM saved_messages s WHERE s.message_id = m.id AND s.user_id = ?1)",
            Is::Thread => "(m.parent_id IS NOT NULL OR EXISTS (SELECT 1 FROM messages r WHERE r.parent_id = m.id))",
        });
    }
    if query.mentions_me {
        sql.and("EXISTS (SELECT 1 FROM activity a WHERE a.message_id = m.id AND a.user_id = ?1 AND a.reason = 'mention')");
    }
    if let Some((start, _)) = query.before.and_then(|day| bounds(day, zone)) {
        let start = sql.bind(Value::Integer(start));
        sql.and(format!("m.created_at < {start}"));
    }
    if let Some((_, end)) = query.after.and_then(|day| bounds(day, zone)) {
        let end = sql.bind(Value::Integer(end));
        sql.and(format!("m.created_at >= {end}"));
    }
    if let Some((start, end)) = query.on.and_then(|day| bounds(day, zone)) {
        let start = sql.bind(Value::Integer(start));
        let end = sql.bind(Value::Integer(end));
        sql.and(format!("m.created_at >= {start} AND m.created_at < {end}"));
    }
}

/// Runs a message search. Returns one more hit than a page when there are
/// more.
fn messages(
    conn: &Connection,
    user_id: i64,
    query: &Query,
    resolved: &Resolved,
    sort: Sort,
    offset: usize,
    now: i64,
) -> AppResult<Vec<Hit>> {
    let zone = crate::later::zone(&store::user_timezone(conn, user_id)?);
    let fts = query.fts();
    let mut sql = Sql {
        params: vec![Value::Integer(user_id)],
        conditions: vec![
            "m.deleted_at IS NULL".to_owned(),
            "((c.kind = 'public' AND c.private = 0) OR EXISTS (
                SELECT 1 FROM channel_members cm WHERE cm.channel_id = c.id AND cm.user_id = ?1))"
                .to_owned(),
        ],
    };
    if let Some(fts) = &fts {
        let fts = sql.bind(Value::Text(fts.clone()));
        sql.and(format!("messages_fts MATCH {fts}"));
    }
    filters(&mut sql, query, resolved, &zone);
    let (join, snippet, order) = if fts.is_some() {
        let order = match sort {
            Sort::Newest => "m.id DESC".to_owned(),
            Sort::Relevance => {
                let now = sql.bind(Value::Integer(now));
                format!(
                    "bm25(messages_fts) / (1.0 + MAX(0, {now} - m.created_at) / {RECENCY_HALF_MS:.1}), m.id DESC"
                )
            }
        };
        (
            "JOIN messages_fts ON messages_fts.rowid = m.id",
            "snippet(messages_fts, 0, char(1), char(2), '…', 16)",
            order,
        )
    } else {
        ("", "substr(m.body, 1, 300)", "m.id DESC".to_owned())
    };
    let limit = sql.bind(Value::Integer(
        i64::try_from(PAGE.saturating_add(1)).unwrap_or(31),
    ));
    let skip = sql.bind(Value::Integer(i64::try_from(offset).unwrap_or(0)));
    let statement = format!(
        "{} JOIN channels c ON c.id = m.channel_id {join}
         WHERE {} ORDER BY {order} LIMIT {limit} OFFSET {skip}",
        store::MESSAGE_SELECT.replace(
            "FROM messages m LEFT JOIN",
            &format!(
                ", {snippet}, c.kind,
                 COALESCE(c.name, (SELECT u2.display_name FROM channel_members o JOIN users u2 ON u2.id = o.user_id
                                   WHERE o.channel_id = c.id AND o.user_id != ?1), 'yourself')
                 FROM messages m LEFT JOIN"
            ),
        ),
        sql.conditions.join(" AND "),
    );
    let mut statement = conn.prepare(&statement)?;
    let hits = statement.query_map(rusqlite::params_from_iter(sql.params), |row| {
        let kind: String = row.get(store::MESSAGE_COLUMNS.saturating_add(1))?;
        Ok(Hit {
            message: store::message_from_row(row)?,
            snippet: row
                .get::<_, Option<String>>(store::MESSAGE_COLUMNS)?
                .unwrap_or_default(),
            is_direct: kind == "dm",
            channel: row.get(store::MESSAGE_COLUMNS.saturating_add(2))?,
        })
    })?;
    let mut hits: Vec<Hit> = hits.collect::<Result<_, _>>()?;
    let mut loaded: Vec<Message> = hits.iter().map(|hit| hit.message.clone()).collect();
    store::hydrate(conn, &mut loaded)?;
    for (hit, message) in hits.iter_mut().zip(loaded) {
        hit.message = message;
    }
    Ok(hits)
}

/// People and channels whose names contain the words.
fn named(conn: &Connection, user_id: i64, text: &str) -> AppResult<(Vec<Named>, Vec<Named>)> {
    let pattern = format!(
        "%{}%",
        text.replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    );
    let mut statement = conn.prepare(
        "SELECT id, display_name, username FROM users
         WHERE deactivated_at IS NULL AND (display_name LIKE ?1 ESCAPE '\\' OR username LIKE ?1 ESCAPE '\\')
         ORDER BY display_name COLLATE NOCASE LIMIT 6",
    )?;
    let people = statement
        .query_map([&pattern], |row| {
            Ok(Named {
                href: format!("/people/{}", row.get::<_, i64>(0)?),
                label: row.get(1)?,
                detail: format!("@{}", row.get::<_, String>(2)?),
                person: true,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut statement = conn.prepare(
        "SELECT c.id, c.name, c.topic, c.private FROM channels c
         WHERE c.kind = 'public' AND c.name LIKE ?1 ESCAPE '\\'
           AND (c.private = 0 OR EXISTS (SELECT 1 FROM channel_members m WHERE m.channel_id = c.id AND m.user_id = ?2))
         ORDER BY c.name LIMIT 6",
    )?;
    let channels = statement
        .query_map(rusqlite::params![&pattern, user_id], |row| {
            Ok(Named {
                href: format!("/c/{}", row.get::<_, i64>(0)?),
                label: row.get(1)?,
                detail: row.get(2)?,
                person: false,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok((people, channels))
}

/// A known word close to `word`, when `word` itself matches nothing:
/// the most frequent one within one edit (two for longer words).
fn correction(conn: &Connection, word: &str) -> AppResult<Option<String>> {
    let word = word.to_lowercase();
    if word.chars().count() < 3 || !word.chars().all(char::is_alphanumeric) {
        return Ok(None);
    }
    let known: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM messages_fts_terms WHERE term >= ?1 AND term < ?1 || char(1114111))",
        [&word],
        |row| row.get(0),
    )?;
    if known {
        return Ok(None);
    }
    let limit = if word.chars().count() <= 5 { 1 } else { 2 };
    let length = i64::try_from(word.chars().count()).unwrap_or(0);
    let mut statement = conn
        .prepare("SELECT term, doc FROM messages_fts_terms WHERE length(term) BETWEEN ?1 AND ?2")?;
    let terms = statement.query_map(
        rusqlite::params![length.saturating_sub(limit), length.saturating_add(limit)],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
    )?;
    let mut best: Option<(usize, i64, String)> = None;
    for term in terms {
        let (term, docs) = term?;
        let Some(distance) = query::distance(&word, &term, usize::try_from(limit).unwrap_or(1))
        else {
            continue;
        };
        let better = best.as_ref().is_none_or(|(best_distance, best_docs, _)| {
            (distance, std::cmp::Reverse(docs)) < (*best_distance, std::cmp::Reverse(*best_docs))
        });
        if better {
            best = Some((distance, docs, term));
        }
    }
    Ok(best.map(|(_, _, term)| term))
}

/// Searches for `text` as `user_id` sees things.
pub fn run(
    conn: &Connection,
    user_id: i64,
    text: &str,
    sort: Option<Sort>,
    page: usize,
    now: i64,
) -> AppResult<Results> {
    let today = crate::later::now_for(conn, user_id)?.date();
    let query = query::parse(text, today);
    let sort = sort.unwrap_or_else(|| {
        if query.has_words() {
            Sort::Relevance
        } else {
            Sort::Newest
        }
    });
    let mut results = Results {
        hits: Vec::new(),
        more: false,
        sort,
        page,
        people: Vec::new(),
        channels: Vec::new(),
        notices: query.problems.clone(),
        corrected: None,
    };
    if query.is_empty() {
        return Ok(results);
    }
    let resolved = resolve(conn, user_id, &query, &mut results.notices);
    if resolved.impossible {
        return Ok(results);
    }
    let offset = page.saturating_mul(PAGE);
    let mut hits = messages(conn, user_id, &query, &resolved, sort, offset, now)?;
    if hits.is_empty() && page == 0 && query.has_words() {
        let mut fixed = query.clone();
        for word in query.words() {
            if let Some(better) = correction(conn, word)? {
                fixed = fixed.replacing(word, &better);
            }
        }
        if fixed != query {
            hits = messages(conn, user_id, &fixed, &resolved, sort, offset, now)?;
            if !hits.is_empty() {
                results.corrected = Some(fixed.words_text());
            }
        }
    }
    results.more = hits.len() > PAGE;
    hits.truncate(PAGE);
    results.hits = hits;
    if page == 0 && query.has_words() && !query.has_filters() {
        let words = query.words_text();
        if words.chars().count() >= 2 && !words.contains('"') {
            let (people, channels) = named(conn, user_id, &words)?;
            results.people = people;
            results.channels = channels;
        }
    }
    Ok(results)
}

fn sort_param(value: &str) -> Option<Sort> {
    match value {
        "newest" => Some(Sort::Newest),
        "relevance" => Some(Sort::Relevance),
        _ => None,
    }
}

pub async fn search(
    user: CurrentUser,
    State(state): State<AppState>,
    QueryString(params): QueryString<SearchParams>,
) -> AppResult<Markup> {
    let user_id = user.id;
    let text: String = params.q.trim().chars().take(300).collect();
    let sort = sort_param(&params.sort);
    let page = params.page.min(100);
    let owned = text.clone();
    let now = crate::now_ms();
    let results = state
        .db
        .call(move |conn| run(conn, user_id, &owned, sort, page, now))
        .await?;
    let sidebar = shell_data(&state, user.id).await?;
    let shell = views::Shell {
        user: &user,
        sidebar: &sidebar,
        current: None,
    };
    Ok(views::search::page(&shell, &text, &results))
}

/// Quick results while typing in the sidebar's search box.
pub async fn suggest(
    user: CurrentUser,
    State(state): State<AppState>,
    QueryString(params): QueryString<SearchParams>,
) -> AppResult<Json<serde_json::Value>> {
    let user_id = user.id;
    let text: String = params.q.trim().chars().take(100).collect();
    if text.chars().count() < 2 {
        return Ok(Json(serde_json::json!({ "items": [] })));
    }
    let now = crate::now_ms();
    let results = state
        .db
        .call(move |conn| run(conn, user_id, &text, None, 0, now))
        .await?;
    let named = results.channels.iter().chain(&results.people).map(|named| {
        serde_json::json!({
            "href": named.href,
            "label": if named.person { named.label.clone() } else { format!("#{}", named.label) },
            "detail": named.detail,
            "kind": if named.person { "person" } else { "channel" },
        })
    });
    let messages = results.hits.iter().take(5).map(|hit| {
        serde_json::json!({
            "href": crate::routes::message_href(&hit.message),
            "label": views::search::plain_snippet(&hit.snippet),
            "detail": if hit.is_direct { hit.channel.clone() } else { format!("#{}", hit.channel) },
            "kind": "message",
        })
    });
    Ok(Json(serde_json::json!({
        "items": named.chain(messages).collect::<Vec<_>>(),
        "corrected": results.corrected,
    })))
}
