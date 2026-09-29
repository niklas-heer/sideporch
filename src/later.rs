//! Things that happen later: reminders and scheduled messages.
//!
//! People say when in plain words, such as `in 20 minutes`, `at 3pm`,
//! `tomorrow`, `friday at 16:30` or `2026-10-01 09:00`, read in their own
//! time zone, which their browser reports. A background task delivers what
//! is due every few seconds.

use std::time::Duration;

use jiff::{Span, Timestamp, Zoned, civil, tz::TimeZone};

use crate::{
    AppState,
    error::{AppError, AppResult},
    messages::{self, Draft, Sender},
    now_ms, store,
};

/// How often the background task looks for due work.
const TICK: Duration = Duration::from_secs(5);
/// Where a day starts, for `tomorrow` and weekdays without a time.
const MORNING: i8 = 9;

fn weekday(word: &str) -> Option<civil::Weekday> {
    use civil::Weekday::{Friday, Monday, Saturday, Sunday, Thursday, Tuesday, Wednesday};
    let days = [
        ("monday", Monday),
        ("tuesday", Tuesday),
        ("wednesday", Wednesday),
        ("thursday", Thursday),
        ("friday", Friday),
        ("saturday", Saturday),
        ("sunday", Sunday),
    ];
    days.iter()
        .find(|(name, _)| word.len() >= 3 && name.starts_with(word))
        .map(|(_, day)| *day)
}

/// `15:30`, `3pm`, `9`, `9:15am`.
fn clock(word: &str) -> Option<(i8, i8)> {
    let (digits, offset) = word
        .strip_suffix("pm")
        .map(|rest| (rest, 12))
        .or_else(|| word.strip_suffix("am").map(|rest| (rest, 0)))
        .unwrap_or((word, -1));
    let (hour, minute) = digits.split_once(':').unwrap_or((digits, "0"));
    let mut hour: i8 = hour.parse().ok()?;
    let minute: i8 = minute.parse().ok()?;
    if offset >= 0 {
        if !(1..=12).contains(&hour) {
            return None;
        }
        hour = hour.rem_euclid(12).checked_add(offset)?;
    }
    ((0..24).contains(&hour) && (0..60).contains(&minute)).then_some((hour, minute))
}

fn at(day: &Zoned, (hour, minute): (i8, i8)) -> Option<Zoned> {
    day.with()
        .hour(hour)
        .minute(minute)
        .second(0)
        .subsec_nanosecond(0)
        .build()
        .ok()
}

/// A time of day, today if still ahead, else tomorrow.
fn next_clock(now: &Zoned, time: (i8, i8)) -> Option<Zoned> {
    let today = at(now, time)?;
    if today > *now {
        Some(today)
    } else {
        at(&now.tomorrow().ok()?, time)
    }
}

/// `in 20 minutes`, `in 2h`, `in 3 days`.
fn relative(words: &[&str], now: &Zoned) -> Option<Zoned> {
    let (amount, unit) = match words {
        ["in", amount, unit] => ((*amount).to_owned(), (*unit).to_owned()),
        ["in", compact] => {
            let split = compact.find(|c: char| !c.is_ascii_digit())?;
            (
                compact.get(..split)?.to_owned(),
                compact.get(split..)?.to_owned(),
            )
        }
        _ => return None,
    };
    let amount: i64 = amount.parse().ok()?;
    if !(1..=1000).contains(&amount) {
        return None;
    }
    let span = match unit.trim_end_matches('s') {
        "m" | "min" | "minute" => Span::new().try_minutes(amount).ok()?,
        "h" | "hr" | "hour" => Span::new().try_hours(amount).ok()?,
        "d" | "day" => Span::new().try_days(amount).ok()?,
        "w" | "week" => Span::new().try_weeks(amount).ok()?,
        _ => return None,
    };
    now.checked_add(span).ok()
}

/// A day, optionally followed by `at <time>`.
fn day_and_time(words: &[&str], now: &Zoned, tz: &TimeZone) -> Option<Zoned> {
    let (day_words, time) = match words {
        [day @ .., "at", time] => (day, Some(clock(time)?)),
        [day @ .., time] if clock(time).is_some() && !day.is_empty() => (day, clock(time)),
        day => (day, None),
    };
    let time = time.unwrap_or((MORNING, 0));
    let day = match day_words {
        ["today"] => now.clone(),
        ["tomorrow"] => now.tomorrow().ok()?,
        ["on", name] | [name] if weekday(name).is_some() => {
            now.nth_weekday(1, weekday(name)?).ok()?
        }
        [date] => date
            .parse::<civil::Date>()
            .ok()?
            .to_zoned(tz.clone())
            .ok()?,
        _ => return None,
    };
    at(&day, time)
}

/// Reads a time phrase in `tz`, relative to `now`. Returns `None` unless
/// the whole phrase is understood and lies ahead.
pub fn parse(phrase: &str, now: &Zoned) -> Option<Zoned> {
    let lower = phrase.trim().to_lowercase().replace(',', " ");
    // `2026-10-01T09:00`, as a browser's datetime-local field sends it.
    if let Ok(local) = lower.replace(' ', "T").parse::<civil::DateTime>() {
        let when = local.to_zoned(now.time_zone().clone()).ok()?;
        return (when > *now).then_some(when);
    }
    let words: Vec<&str> = lower.split_whitespace().collect();
    let when = match words.as_slice() {
        [] => None,
        ["at", time] => next_clock(now, clock(time)?),
        ["in", ..] => relative(&words, now),
        _ => day_and_time(&words, now, now.time_zone()),
    }?;
    (when > *now).then_some(when)
}

/// Splits `text` into a time phrase and the rest, trying the start first
/// and then the end, longest phrases first.
pub fn split_phrase(text: &str, now: &Zoned) -> Option<(Zoned, String)> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let longest = words.len().min(4);
    for take in (1..=longest).rev() {
        let (head, tail) = words.split_at(take);
        if let Some(when) = parse(&head.join(" "), now) {
            return Some((when, tail.join(" ")));
        }
    }
    for take in (1..=longest).rev() {
        let (head, tail) = words.split_at(words.len().saturating_sub(take));
        if let Some(when) = parse(&tail.join(" "), now) {
            return Some((when, head.join(" ")));
        }
    }
    None
}

/// `Tue 30 Sep, 09:00`.
pub fn describe(when: &Zoned) -> String {
    when.strftime("%a %-d %b, %H:%M").to_string()
}

pub fn zone(name: &str) -> TimeZone {
    TimeZone::get(name).unwrap_or(TimeZone::UTC)
}

/// Now, in `user_id`'s time zone.
pub fn now_for(conn: &rusqlite::Connection, user_id: i64) -> AppResult<Zoned> {
    let name = store::user_timezone(conn, user_id)?;
    Ok(Timestamp::now().to_zoned(zone(&name)))
}

/// Answers `/remind`, as a private notice.
pub async fn remind_command(state: &AppState, user_id: i64, text: &str) -> AppResult<String> {
    let text = text.trim();
    if text.is_empty() || text == "help" {
        return Ok(REMIND_HELP.to_owned());
    }
    let text = text.strip_prefix("me ").unwrap_or(text).trim();
    let owned = text.to_owned();
    let found = state
        .db
        .call(move |conn| Ok(split_phrase(&owned, &now_for(conn, user_id)?)))
        .await?;
    let Some((when, what)) = found else {
        return Ok(format!("I couldn't tell when. Try {REMIND_EXAMPLES}."));
    };
    let what = what
        .trim()
        .trim_start_matches("to ")
        .trim_start_matches("about ")
        .trim()
        .to_owned();
    if what.is_empty() {
        return Ok("Remind you about what? Try `/remind me in 1 hour to call Mo`.".to_owned());
    }
    let at = when.timestamp().as_millisecond();
    let now = now_ms();
    let saved = what.clone();
    state
        .db
        .call(move |conn| store::add_reminder(conn, user_id, &saved, None, at, now))
        .await?;
    Ok(format!(
        "Okay, I'll remind you on {}: {what}. See [Scheduled](/scheduled).",
        describe(&when)
    ))
}

const REMIND_EXAMPLES: &str = "`/remind me in 20 minutes to stretch`, `/remind me tomorrow to water the plants` or `/remind me to call Mo friday at 3pm`";

const REMIND_HELP: &str = concat!(
    "**/remind** me *when* to *what*, such as ",
    "`/remind me in 20 minutes to stretch`, `/remind me tomorrow to water the plants` ",
    "or `/remind me to call Mo friday at 3pm`. Times are in your time zone. ",
    "Your reminders are under [Scheduled](/scheduled)."
);

/// Starts the task that sends due reminders and scheduled messages.
pub fn start(state: AppState) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(TICK);
        loop {
            ticker.tick().await;
            if let Err(error) = deliver_due(&state).await {
                tracing::warn!(?error, "could not deliver scheduled work");
            }
        }
    });
}

/// Sends everything that is due.
pub async fn deliver_due(state: &AppState) -> AppResult<()> {
    let now = now_ms();
    let (reminders, scheduled) = state
        .db
        .call(move |conn| store::take_due(conn, now))
        .await?;
    for message in scheduled {
        let user_id = message.user_id;
        let channel_id = message.channel_id;
        let parent_id = message.parent_id;
        let allowed = state
            .db
            .call(move |conn| {
                Ok(store::channel_for(conn, channel_id, user_id)?
                    .is_some_and(|channel| channel.may_write(parent_id.is_some()))
                    && store::user(conn, user_id)?.is_some_and(|user| !user.deactivated))
            })
            .await?;
        if !allowed {
            continue;
        }
        let draft = Draft {
            channel_id,
            parent_id: message.parent_id,
            sender: Sender::User(user_id),
            body: message.body,
            attachments: Vec::new(),
            files: Vec::new(),
            gif: None,
            poll: None,
            buttons: Vec::new(),
        };
        if let Err(error) = messages::post(state, draft).await {
            tracing::warn!(?error, "could not send a scheduled message");
        }
    }
    for reminder in reminders {
        if let Err(error) = send_reminder(state, &reminder).await {
            tracing::warn!(?error, "could not send a reminder");
        }
    }
    Ok(())
}

/// Posts a reminder in the person's notes to self.
async fn send_reminder(state: &AppState, reminder: &store::Reminder) -> AppResult<()> {
    let user_id = reminder.user_id;
    let now = now_ms();
    let channel_id = state
        .db
        .call(move |conn| {
            if store::user(conn, user_id)?.is_none_or(|user| user.deactivated) {
                return Ok(None);
            }
            store::direct_channel(conn, user_id, user_id, now).map(Some)
        })
        .await?;
    let Some(channel_id) = channel_id else {
        return Ok(());
    };
    let body = reminder.link.as_ref().map_or_else(
        || format!(":alarm_clock: Reminder: {}", reminder.text),
        |link| {
            format!(
                ":alarm_clock: Reminder: {}\n\n[See the message]({link})",
                reminder.text
            )
        },
    );
    messages::post(
        state,
        Draft {
            channel_id,
            parent_id: None,
            sender: Sender::Bot {
                name: "Reminder".to_owned(),
                icon: Some(":alarm_clock:".to_owned()),
            },
            body,
            attachments: Vec::new(),
            files: Vec::new(),
            gif: None,
            poll: None,
            buttons: Vec::new(),
        },
    )
    .await
    .map(drop)
}

/// Validates a time zone name from a browser.
pub fn valid_zone(name: &str) -> AppResult<String> {
    TimeZone::get(name)
        .map(|_| name.to_owned())
        .map_err(|_| AppError::bad_request("Unknown time zone."))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> Zoned {
        // Tuesday 2026-09-29, 14:05 in Berlin.
        civil::date(2026, 9, 29)
            .at(14, 5, 0, 0)
            .in_tz("Europe/Berlin")
            .unwrap()
    }

    fn when(phrase: &str) -> String {
        parse(phrase, &now()).map_or_else(|| "none".to_owned(), |z| describe(&z))
    }

    #[test]
    fn reads_times_in_plain_words() {
        assert_eq!(when("in 20 minutes"), "Tue 29 Sep, 14:25");
        assert_eq!(when("in 2h"), "Tue 29 Sep, 16:05");
        assert_eq!(when("at 3pm"), "Tue 29 Sep, 15:00");
        assert_eq!(when("at 9:30"), "Wed 30 Sep, 09:30");
        assert_eq!(when("tomorrow"), "Wed 30 Sep, 09:00");
        assert_eq!(when("tomorrow at 17:45"), "Wed 30 Sep, 17:45");
        assert_eq!(when("friday"), "Fri 2 Oct, 09:00");
        assert_eq!(when("on tue at 8am"), "Tue 6 Oct, 08:00");
        assert_eq!(when("2026-10-12 18:00"), "Mon 12 Oct, 18:00");
        assert_eq!(when("2026-10-12T18:00"), "Mon 12 Oct, 18:00");
        assert_eq!(when("yesterday"), "none");
        assert_eq!(when("in 0 minutes"), "none");
        assert_eq!(when("2020-01-01"), "none");
    }

    #[test]
    fn finds_the_time_at_either_end() {
        let (at, rest) = split_phrase("in 1 hour to call Mo", &now()).unwrap();
        assert_eq!(
            (describe(&at).as_str(), rest.as_str()),
            ("Tue 29 Sep, 15:05", "to call Mo")
        );
        let (at, rest) = split_phrase("to water the plants friday at 7pm", &now()).unwrap();
        assert_eq!(
            (describe(&at).as_str(), rest.as_str()),
            ("Fri 2 Oct, 19:00", "to water the plants")
        );
        assert!(split_phrase("to do something someday", &now()).is_none());
        let found = split_phrase("to do it someday", &now());
        assert!(
            found.is_none(),
            "{:?}",
            found.map(|(z, r)| (describe(&z), r))
        );
    }
}
