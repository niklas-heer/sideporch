//! How much a community talks, where, and who talks most. Only public
//! channels count, never private channels or direct messages. Rankings
//! list people from this server who haven't left them; bots, automations
//! and people from other servers count in the totals but aren't ranked.
//!
//! A [`Summary`] is the same for everyone who picks the same period in the
//! same time zone. On a big server it takes a while to count, so it's read
//! on the database's read-only connection and kept in a [`Cache`] for a
//! time that grows with how long it took.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use jiff::{Timestamp, ToSpan, civil::Date, tz::TimeZone};
use rusqlite::{Connection, OptionalExtension, params};

use crate::error::{AppError, AppResult};

/// Messages that count: in public channels, not deleted.
const COUNTED: &str = "c.kind = 'public' AND c.private = 0 AND m.deleted_at IS NULL";
/// Authors who may be ranked, joined as `u`.
const RANKED: &str = "m.webhook_id IS NULL AND m.automation_id IS NULL AND u.instance_id IS NULL \
     AND u.deactivated_at IS NULL AND u.hide_from_rankings = 0";
/// How many entries a ranking shows.
const TOP: i64 = 10;
const TOP_SHOWN: usize = 10;
/// Messages are counted per quarter hour, then placed in the reader's
/// days; that suits every time zone in use, including half-hour offsets.
const QUARTER_HOUR_MS: i64 = 15 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Period {
    Week,
    #[default]
    Month,
    Year,
    All,
}

impl Period {
    pub const ALL: [Self; 4] = [Self::Week, Self::Month, Self::Year, Self::All];

    pub const fn key(self) -> &'static str {
        match self {
            Self::Week => "7d",
            Self::Month => "30d",
            Self::Year => "12m",
            Self::All => "all",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Week => "7 days",
            Self::Month => "30 days",
            Self::Year => "12 months",
            Self::All => "All time",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|period| period.key() == key)
    }
}

/// How long one bar of the chart is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Day,
    Month,
    Year,
}

impl Step {
    /// The first day of the bar `date` falls in.
    fn start(self, date: Date) -> Date {
        match self {
            Self::Day => date,
            Self::Month => date.first_of_month(),
            Self::Year => date.first_of_year(),
        }
    }

    fn next(self, date: Date) -> Option<Date> {
        let span = match self {
            Self::Day => 1.day(),
            Self::Month => 1.month(),
            Self::Year => 1.year(),
        };
        date.checked_add(span).ok()
    }
}

/// One bar of the chart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bar {
    pub start: Date,
    pub count: i64,
}

/// A ranked person.
#[derive(Debug, Clone)]
pub struct Person {
    pub id: i64,
    pub name: String,
    pub avatar: Option<i64>,
    pub count: i64,
}

/// Where the reader stands.
#[derive(Debug, Clone, Default)]
pub struct Standing {
    pub messages: i64,
    /// Their place among ranked people who wrote; `None` when they wrote
    /// nothing or left the rankings.
    pub rank: Option<i64>,
    /// How many people are ranked.
    pub of: i64,
    pub hidden: bool,
}

/// Everything the page shows except the reader's own standing.
#[derive(Debug, Clone)]
pub struct Summary {
    pub period: Period,
    pub step: Step,
    /// When the period starts, in milliseconds.
    since: i64,
    pub messages: i64,
    pub people: i64,
    pub reactions: i64,
    pub files: i64,
    pub bars: Vec<Bar>,
    pub writers: Vec<Person>,
    pub appreciated: Vec<Person>,
    /// Busiest channels: id, name and messages.
    pub channels: Vec<(i64, String, i64)>,
    /// Most used reactions: emoji name and count.
    pub emoji: Vec<(String, i64)>,
    /// Messages of every ranked person, most first.
    ranked: Vec<(i64, i64)>,
}

#[derive(Debug, Clone)]
pub struct Report {
    pub summary: Arc<Summary>,
    pub you: Standing,
}

/// The longest a summary is kept.
const KEEP_AT_MOST: Duration = Duration::from_mins(5);
/// A summary is kept this many times as long as it took to count, so small
/// servers always see fresh numbers and big ones aren't counted over and
/// over.
const KEEP_FACTOR: u32 = 60;
/// Summaries kept at once, across periods and time zones.
const KEEP_ENTRIES: usize = 64;

struct Kept {
    made: Instant,
    keep: Duration,
    summary: Arc<Summary>,
}

/// Recent summaries, by period and time zone.
#[derive(Default)]
pub struct Cache(Mutex<HashMap<(Period, String), Kept>>);

impl Cache {
    pub fn get(&self, period: Period, zone: &str) -> Option<Arc<Summary>> {
        let kept = self.0.lock().ok()?;
        kept.get(&(period, zone.to_owned()))
            .filter(|kept| kept.made.elapsed() < kept.keep)
            .map(|kept| Arc::clone(&kept.summary))
    }

    /// Keeps `summary`, which took `took` to count.
    pub fn put(&self, period: Period, zone: &str, summary: Arc<Summary>, took: Duration) {
        if let Ok(mut kept) = self.0.lock() {
            if kept.len() >= KEEP_ENTRIES {
                kept.retain(|_, kept| kept.made.elapsed() < kept.keep);
                if kept.len() >= KEEP_ENTRIES {
                    kept.clear();
                }
            }
            kept.insert(
                (period, zone.to_owned()),
                Kept {
                    made: Instant::now(),
                    keep: took.saturating_mul(KEEP_FACTOR).min(KEEP_AT_MOST),
                    summary,
                },
            );
        }
    }

    /// Forgets every summary, such as after someone left the rankings.
    pub fn clear(&self) {
        if let Ok(mut kept) = self.0.lock() {
            kept.clear();
        }
    }
}

/// Whether `user_id` left the rankings.
pub fn hidden(conn: &Connection, user_id: i64) -> AppResult<bool> {
    Ok(conn
        .query_row(
            "SELECT hide_from_rankings FROM users WHERE id = ?1",
            [user_id],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(false))
}

pub fn set_hidden(conn: &Connection, user_id: i64, hidden: bool) -> AppResult<()> {
    conn.execute(
        "UPDATE users SET hide_from_rankings = ?1 WHERE id = ?2",
        params![hidden, user_id],
    )?;
    Ok(())
}

/// The first day the period covers in `zone`, and how its chart is split.
/// `None` for all time, which starts at the first counted message.
fn window(period: Period, today: Date) -> (Option<Date>, Step) {
    let back = |span: jiff::Span| today.checked_sub(span).ok();
    match period {
        Period::Week => (back(6.days()), Step::Day),
        Period::Month => (back(29.days()), Step::Day),
        Period::Year => (back(11.months()).map(Date::first_of_month), Step::Month),
        Period::All => (None, Step::Month),
    }
}

fn midnight_ms(date: Date, zone: &TimeZone) -> AppResult<i64> {
    Ok(date
        .to_zoned(zone.clone())
        .map_err(AppError::internal)?
        .timestamp()
        .as_millisecond())
}

/// Messages per bar, from `first` to `today`.
fn bars(
    quarters: &[(i64, i64)],
    first: Date,
    today: Date,
    step: Step,
    zone: &TimeZone,
) -> Vec<Bar> {
    let mut bars = Vec::new();
    let mut index = HashMap::new();
    let mut day = step.start(first);
    while day <= today {
        index.insert(day, bars.len());
        bars.push(Bar {
            start: day,
            count: 0,
        });
        match step.next(day) {
            Some(next) => day = next,
            None => break,
        }
    }
    for (quarter, count) in quarters {
        let Ok(at) = Timestamp::from_millisecond(quarter.saturating_mul(QUARTER_HOUR_MS)) else {
            continue;
        };
        let date = step.start(at.to_zoned(zone.clone()).date());
        if let Some(bar) = index.get(&date).and_then(|at| bars.get_mut(*at)) {
            bar.count = bar.count.saturating_add(*count);
        }
    }
    bars
}

/// People and their counts; `limit` -1 for everyone.
fn people(conn: &Connection, sql: &str, since: i64, limit: i64) -> AppResult<Vec<Person>> {
    let mut statement = conn.prepare(sql)?;
    let rows = statement.query_map(params![since, limit], |row| {
        Ok(Person {
            id: row.get(0)?,
            name: row.get(1)?,
            avatar: row.get(2)?,
            count: row.get(3)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Messages per bar since `start`, or since the first counted message.
fn timeline(
    conn: &Connection,
    period: Period,
    zone: &TimeZone,
    today: Date,
    start: Option<Date>,
    since: i64,
) -> AppResult<(Step, Vec<Bar>)> {
    let mut statement = conn.prepare(&format!(
        "SELECT m.created_at / {QUARTER_HOUR_MS}, COUNT(*) FROM messages m
         JOIN channels c ON c.id = m.channel_id
         WHERE {COUNTED} AND m.created_at >= ?1 GROUP BY 1 ORDER BY 1"
    ))?;
    let quarters: Vec<(i64, i64)> = statement
        .query_map([since], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<_, _>>()?;
    let first = start.unwrap_or_else(|| {
        quarters
            .first()
            .and_then(|(quarter, _)| {
                Timestamp::from_millisecond(quarter.saturating_mul(QUARTER_HOUR_MS)).ok()
            })
            .map_or(today, |at| at.to_zoned(zone.clone()).date())
    });
    let (_, mut step) = window(period, today);
    // Years of history read better a bar per year.
    if period == Period::All && today.year().saturating_sub(first.year()) > 3 {
        step = Step::Year;
    }
    Ok((step, bars(&quarters, first, today, step, zone)))
}

fn count(conn: &Connection, sql: &str, since: i64) -> AppResult<i64> {
    Ok(conn.query_row(sql, [since], |row| row.get(0))?)
}

/// Everything but the reader's standing for `period`, with days in `zone`.
pub fn summary(conn: &Connection, period: Period, zone: &TimeZone) -> AppResult<Summary> {
    let today = Timestamp::now().to_zoned(zone.clone()).date();
    let (start, _) = window(period, today);
    let since = match start {
        Some(date) => midnight_ms(date, zone)?,
        None => 0,
    };
    let (step, bars) = timeline(conn, period, zone, today, start, since)?;

    let (messages, people_count): (i64, i64) = conn.query_row(
        &format!(
            "SELECT COUNT(*), COUNT(DISTINCT CASE WHEN m.webhook_id IS NULL AND m.automation_id IS NULL THEN m.user_id END)
             FROM messages m JOIN channels c ON c.id = m.channel_id
             WHERE {COUNTED} AND m.created_at >= ?1"
        ),
        [since],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let reactions = count(
        conn,
        &format!(
            "SELECT COUNT(*) FROM reactions r JOIN messages m ON m.id = r.message_id
             JOIN channels c ON c.id = m.channel_id WHERE {COUNTED} AND r.created_at >= ?1"
        ),
        since,
    )?;
    let files = count(
        conn,
        &format!(
            "SELECT COUNT(*) FROM message_files f JOIN messages m ON m.id = f.message_id
             JOIN channels c ON c.id = m.channel_id WHERE {COUNTED} AND m.created_at >= ?1"
        ),
        since,
    )?;

    // Everyone ranked, once: the top of the list, and where each reader stands.
    let everyone = people(
        conn,
        &format!(
            "SELECT u.id, u.display_name, u.avatar_file_id, COUNT(*) AS n FROM messages m
             JOIN channels c ON c.id = m.channel_id JOIN users u ON u.id = m.user_id
             WHERE {COUNTED} AND {RANKED} AND m.created_at >= ?1
             GROUP BY u.id ORDER BY n DESC, u.display_name COLLATE NOCASE LIMIT ?2"
        ),
        since,
        -1,
    )?;
    let ranked = everyone
        .iter()
        .map(|person| (person.id, person.count))
        .collect();
    let writers = everyone.into_iter().take(TOP_SHOWN).collect();
    // Reactions to your own messages don't count.
    let appreciated = people(
        conn,
        &format!(
            "SELECT u.id, u.display_name, u.avatar_file_id, COUNT(*) AS n FROM reactions r
             JOIN messages m ON m.id = r.message_id
             JOIN channels c ON c.id = m.channel_id JOIN users u ON u.id = m.user_id
             WHERE {COUNTED} AND {RANKED} AND r.created_at >= ?1 AND r.user_id != m.user_id
             GROUP BY u.id ORDER BY n DESC, u.display_name COLLATE NOCASE LIMIT ?2"
        ),
        since,
        TOP,
    )?;

    let mut statement = conn.prepare(&format!(
        "SELECT c.id, c.name, COUNT(*) AS n FROM messages m JOIN channels c ON c.id = m.channel_id
         WHERE {COUNTED} AND m.created_at >= ?1
         GROUP BY c.id ORDER BY n DESC, c.name COLLATE NOCASE LIMIT ?2"
    ))?;
    let channels = statement
        .query_map(params![since, TOP], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?
        .collect::<Result<_, _>>()?;
    let mut statement = conn.prepare(&format!(
        "SELECT r.emoji, COUNT(*) AS n FROM reactions r JOIN messages m ON m.id = r.message_id
         JOIN channels c ON c.id = m.channel_id
         WHERE {COUNTED} AND r.created_at >= ?1
         GROUP BY r.emoji ORDER BY n DESC, r.emoji LIMIT ?2"
    ))?;
    let emoji = statement
        .query_map(params![since, TOP], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<_, _>>()?;

    Ok(Summary {
        period,
        step,
        since,
        messages,
        people: people_count,
        reactions,
        files,
        bars,
        writers,
        appreciated,
        channels,
        emoji,
        ranked,
    })
}

/// Where `viewer` stands in `summary`.
pub fn standing(conn: &Connection, summary: &Summary, viewer: i64) -> AppResult<Standing> {
    let hidden = hidden(conn, viewer)?;
    let of = i64::try_from(summary.ranked.len()).unwrap_or(i64::MAX);
    if let Some((_, messages)) = summary.ranked.iter().find(|(id, _)| *id == viewer) {
        let ahead = summary
            .ranked
            .iter()
            .take_while(|(_, count)| count > messages)
            .count();
        return Ok(Standing {
            messages: *messages,
            rank: Some(i64::try_from(ahead).unwrap_or(i64::MAX).saturating_add(1)),
            of,
            hidden,
        });
    }
    // Not ranked: they left the rankings, or wrote nothing in the period.
    let messages = conn.query_row(
        &format!(
            "SELECT COUNT(*) FROM messages m JOIN channels c ON c.id = m.channel_id
             WHERE {COUNTED} AND m.user_id = ?1 AND m.webhook_id IS NULL AND m.automation_id IS NULL
               AND m.created_at >= ?2"
        ),
        params![viewer, summary.since],
        |row| row.get(0),
    )?;
    Ok(Standing {
        messages,
        rank: None,
        of,
        hidden,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_cover_every_day_and_place_messages_in_the_readers_days() {
        let zone = TimeZone::get("Asia/Kolkata").unwrap_or(TimeZone::UTC);
        let today = jiff::civil::date(2026, 9, 29);
        let (first, step) = window(Period::Week, today);
        let first = first.unwrap_or(today);
        assert_eq!(first, jiff::civil::date(2026, 9, 23));
        // 23:45 UTC on the 27th is already the 28th in India (UTC+5:30).
        let late = jiff::civil::date(2026, 9, 27)
            .at(23, 45, 0, 0)
            .to_zoned(TimeZone::UTC)
            .ok()
            .and_then(|at| at.timestamp().as_millisecond().checked_div(QUARTER_HOUR_MS))
            .unwrap_or_default();
        let bars = bars(&[(late, 3)], first, today, step, &zone);
        assert_eq!(bars.len(), 7);
        let counts: Vec<i64> = bars.iter().map(|bar| bar.count).collect();
        assert_eq!(counts, vec![0, 0, 0, 0, 0, 3, 0]);
    }

    #[test]
    fn a_year_has_twelve_months() {
        let today = jiff::civil::date(2026, 9, 29);
        let (first, step) = window(Period::Year, today);
        let first = first.unwrap_or(today);
        assert_eq!(first, jiff::civil::date(2025, 10, 1));
        assert_eq!(bars(&[], first, today, step, &TimeZone::UTC).len(), 12);
    }
}
