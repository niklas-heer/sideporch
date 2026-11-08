//! Personal notification schedules: a pause, and quiet hours that repeat
//! every week. They decide only whether a person gets Web Push, in
//! `push::deliver`. Unread markers, Activity and live updates ignore them.
//!
//! Quiet hours run on the person's own clock, in the time zone their
//! browser reports (see `later.rs`). A window that crosses midnight belongs
//! to the day it starts on: Friday 22:00 to 07:00 covers Friday night and
//! the early hours of Saturday, and the day list is checked against Friday.

use jiff::{Timestamp, Zoned, civil, tz::TimeZone};
use rusqlite::Connection;

use crate::{
    error::{AppError, AppResult},
    later, store,
};

const HOUR_MS: i64 = 60 * 60 * 1000;

/// Monday to Sunday, with the codes a form sends and the names shown.
pub const WEEKDAYS: [(&str, &str); 7] = [
    ("mon", "Monday"),
    ("tue", "Tuesday"),
    ("wed", "Wednesday"),
    ("thu", "Thursday"),
    ("fri", "Friday"),
    ("sat", "Saturday"),
    ("sun", "Sunday"),
];

/// Every day of the week, as a bitmask.
pub const EVERY_DAY: u8 = 0b111_1111;

/// Quiet hours as stored: minutes after midnight, and a day bitmask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuietHours {
    pub enabled: bool,
    pub start: u16,
    pub end: u16,
    /// Bit 0 is Monday, bit 6 is Sunday.
    pub days: u8,
}

impl Default for QuietHours {
    /// Off, but set to 22:00 to 07:00 every day, so turning it on is one tick.
    fn default() -> Self {
        Self {
            enabled: false,
            start: 22 * 60,
            end: 7 * 60,
            days: EVERY_DAY,
        }
    }
}

/// A person's notification settings.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Schedule {
    /// When the pause ends, in milliseconds, if one is set.
    pub paused_until: Option<i64>,
    pub quiet: QuietHours,
}

impl Schedule {
    /// Whether Web Push must stay quiet for this person at `now_ms`, read
    /// on `zone`, their own time zone.
    pub fn silences(&self, now_ms: i64, zone: &TimeZone) -> bool {
        if self.paused_until.is_some_and(|until| now_ms < until) {
            return true;
        }
        self.quiet.enabled
            && Timestamp::from_millisecond(now_ms)
                .is_ok_and(|now| quiet_at(self.quiet, &now.to_zoned(zone.clone())))
    }
}

/// Whether `now` falls inside the quiet window. Ignores `enabled`.
pub fn quiet_at(quiet: QuietHours, now: &Zoned) -> bool {
    let minute = minute_of_day(now);
    let today = day_index(now.weekday());
    // The weekday a window that started yesterday belongs to.
    let yesterday = today.checked_sub(1).unwrap_or(6);
    let (start, end) = (quiet.start, quiet.end);
    match start.cmp(&end) {
        std::cmp::Ordering::Less => minute >= start && minute < end && day_is_on(quiet.days, today),
        std::cmp::Ordering::Greater => {
            (minute >= start && day_is_on(quiet.days, today))
                || (minute < end && day_is_on(quiet.days, yesterday))
        }
        // Equal ends make no window.
        std::cmp::Ordering::Equal => false,
    }
}

fn minute_of_day(now: &Zoned) -> u16 {
    let hour = u16::try_from(now.hour()).unwrap_or(0);
    let minute = u16::try_from(now.minute()).unwrap_or(0);
    hour.saturating_mul(60).saturating_add(minute)
}

/// Monday is 0, Sunday is 6.
fn day_index(weekday: civil::Weekday) -> u8 {
    u8::try_from(weekday.to_monday_zero_offset()).unwrap_or(0)
}

/// Whether `day` (Monday is 0) is in the bitmask `days`.
pub const fn day_is_on(days: u8, day: u8) -> bool {
    (days >> day) & 1 == 1
}

/// Reads `HH:MM` as minutes after midnight. Both parts take two digits.
pub fn parse_clock(text: &str) -> Option<u16> {
    let (hour, minute) = text.split_once(':')?;
    let digits = |part: &str| part.len() == 2 && part.bytes().all(|byte| byte.is_ascii_digit());
    if !digits(hour) || !digits(minute) {
        return None;
    }
    let hour: u16 = hour.parse().ok()?;
    let minute: u16 = minute.parse().ok()?;
    (hour < 24 && minute < 60).then_some(hour.saturating_mul(60).saturating_add(minute))
}

/// `HH:MM` for minutes after midnight, as a time input takes it.
pub fn format_clock(minute: u16) -> String {
    let hour = minute.checked_div(60).unwrap_or(0);
    let minute = minute.checked_rem(60).unwrap_or(0);
    format!("{hour:02}:{minute:02}")
}

/// Reads the quiet-hours form. Times are required; days are required when
/// the window is on. The error is shown to the person as it is.
pub fn quiet_from_form(
    enabled: bool,
    start: &str,
    end: &str,
    days: &[String],
) -> Result<QuietHours, &'static str> {
    let start = parse_clock(start).ok_or("Pick a start time.")?;
    let end = parse_clock(end).ok_or("Pick an end time.")?;
    if start == end {
        return Err("Quiet hours need different start and end times.");
    }
    let mut mask: u8 = 0;
    for code in days {
        let (day, _) = (0_u8..)
            .zip(WEEKDAYS)
            .find(|(_, (known, _))| *known == code.as_str())
            .ok_or("That day isn't a day of the week.")?;
        mask |= 1 << day;
    }
    if enabled && mask == 0 {
        return Err("Choose at least one day for quiet hours.");
    }
    Ok(QuietHours {
        enabled,
        start,
        end,
        days: mask,
    })
}

/// The moment a pause the person picked ends: `1h`, `8h`, or 08:00
/// tomorrow in their time zone. `resume` returns `None`, which ends it now.
pub fn pause_until(choice: &str, now_ms: i64, zone: &TimeZone) -> AppResult<Option<i64>> {
    match choice {
        "1h" => Ok(Some(now_ms.saturating_add(HOUR_MS))),
        "8h" => Ok(Some(now_ms.saturating_add(8 * HOUR_MS))),
        "tomorrow" => {
            let now = Timestamp::from_millisecond(now_ms)
                .map_err(AppError::internal)?
                .to_zoned(zone.clone());
            let morning = now
                .date()
                .tomorrow()
                .map_err(AppError::internal)?
                .at(8, 0, 0, 0)
                .to_zoned(zone.clone())
                .map_err(AppError::internal)?;
            Ok(Some(morning.timestamp().as_millisecond()))
        }
        "resume" => Ok(None),
        _ => Err(AppError::bad_request("Pick a pause, or resume.")),
    }
}

/// When a pause ends, as the person reads it: `14:30` on the same day,
/// otherwise `Tue 10 Nov, 08:00`.
pub fn describe_until(until_ms: i64, now: &Zoned) -> String {
    let Ok(until) = Timestamp::from_millisecond(until_ms) else {
        return "later".to_owned();
    };
    let until = until.to_zoned(now.time_zone().clone());
    if until.date() == now.date() {
        until.strftime("%H:%M").to_string()
    } else {
        later::describe(&until)
    }
}

/// "Paused until 14:30", while a pause is running.
pub fn paused_label(conn: &Connection, user_id: i64, now_ms: i64) -> AppResult<Option<String>> {
    let schedule = store::notification_schedule(conn, user_id)?;
    let Some(until) = schedule.paused_until.filter(|until| *until > now_ms) else {
        return Ok(None);
    };
    let now = later::now_for(conn, user_id)?;
    Ok(Some(format!(
        "Paused until {}",
        describe_until(until, &now)
    )))
}

/// Whether Web Push must stay quiet for `user_id` right now.
pub fn silenced(conn: &Connection, user_id: i64, now_ms: i64) -> AppResult<bool> {
    let schedule = store::notification_schedule(conn, user_id)?;
    let zone = later::zone(&store::user_timezone(conn, user_id)?);
    Ok(schedule.silences(now_ms, &zone))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Berlin is UTC+1 in November.
    fn berlin(day: i8, hour: i8, minute: i8) -> Zoned {
        civil::date(2026, 11, day)
            .at(hour, minute, 0, 0)
            .in_tz("Europe/Berlin")
            .unwrap()
    }

    fn window(start: &str, end: &str, days: &[&str]) -> QuietHours {
        let days: Vec<String> = days.iter().map(|day| (*day).to_owned()).collect();
        quiet_from_form(true, start, end, &days).unwrap()
    }

    #[test]
    fn a_window_inside_the_day_covers_its_minutes_only() {
        let quiet = window("13:00", "14:00", &["sun", "mon"]);
        // Sunday 8 Nov 2026 is the first of the listed days.
        assert!(quiet_at(quiet, &berlin(8, 13, 0)));
        assert!(quiet_at(quiet, &berlin(8, 13, 59)));
        assert!(!quiet_at(quiet, &berlin(8, 14, 0)));
        assert!(!quiet_at(quiet, &berlin(8, 12, 59)));
        // Tuesday is not listed.
        assert!(!quiet_at(quiet, &berlin(10, 13, 30)));
    }

    #[test]
    fn a_window_across_midnight_covers_both_sides() {
        let quiet = window("22:00", "07:00", &["fri"]);
        // Friday 13 Nov 2026 at 23:30 starts the window; it's inside.
        assert!(quiet_at(quiet, &berlin(13, 23, 30)));
        // Saturday 06:59 belongs to Friday's window.
        assert!(quiet_at(quiet, &berlin(14, 6, 59)));
        // Saturday 07:00 and onward is outside.
        assert!(!quiet_at(quiet, &berlin(14, 7, 0)));
        // Saturday night starts no window, since Saturday isn't listed.
        assert!(!quiet_at(quiet, &berlin(14, 23, 30)));
        // Early Saturday morning belongs to a Friday window, so Monday's
        // early hours belong to Sunday's, which isn't listed.
        assert!(!quiet_at(quiet, &berlin(16, 3, 0)));
        // Friday 21:59 is before the window opens.
        assert!(!quiet_at(quiet, &berlin(13, 21, 59)));
    }

    #[test]
    fn equal_ends_make_no_window() {
        let quiet = QuietHours {
            enabled: true,
            start: 600,
            end: 600,
            days: EVERY_DAY,
        };
        assert!(!quiet_at(quiet, &berlin(8, 10, 0)));
    }

    #[test]
    fn a_pause_silences_until_it_ends() {
        let zone = TimeZone::get("Europe/Berlin").unwrap();
        let now = berlin(8, 14, 0).timestamp().as_millisecond();
        let schedule = Schedule {
            paused_until: Some(now + HOUR_MS),
            quiet: QuietHours::default(),
        };
        assert!(schedule.silences(now, &zone));
        assert!(schedule.silences(now + HOUR_MS - 1, &zone));
        assert!(!schedule.silences(now + HOUR_MS, &zone));
    }

    #[test]
    fn quiet_hours_need_to_be_on() {
        let zone = TimeZone::get("Europe/Berlin").unwrap();
        let at = berlin(8, 2, 0).timestamp().as_millisecond();
        let mut schedule = Schedule {
            paused_until: None,
            quiet: QuietHours {
                enabled: false,
                ..QuietHours::default()
            },
        };
        assert!(!schedule.silences(at, &zone));
        schedule.quiet.enabled = true;
        assert!(schedule.silences(at, &zone));
    }

    #[test]
    fn times_are_read_in_the_persons_zone() {
        // 22:30 in Berlin is 21:30 UTC. Quiet from 22:00 Berlin time is on
        // then, and off for a UTC reader at that same instant.
        let quiet = window("22:00", "07:00", &["sun"]);
        let instant = berlin(8, 22, 30).timestamp().as_millisecond();
        let schedule = Schedule {
            paused_until: None,
            quiet: QuietHours {
                enabled: true,
                ..quiet
            },
        };
        assert!(schedule.silences(instant, &TimeZone::get("Europe/Berlin").unwrap()));
        assert!(!schedule.silences(instant, &TimeZone::UTC));
    }

    #[test]
    fn clock_times_parse_strictly() {
        assert_eq!(parse_clock("22:00"), Some(1320));
        assert_eq!(parse_clock("07:05"), Some(425));
        assert_eq!(parse_clock("00:00"), Some(0));
        assert_eq!(parse_clock("23:59"), Some(1439));
        assert_eq!(parse_clock("24:00"), None);
        assert_eq!(parse_clock("12:60"), None);
        assert_eq!(parse_clock("7:05"), None);
        assert_eq!(parse_clock("07:5"), None);
        assert_eq!(parse_clock("+7:05"), None);
        assert_eq!(parse_clock("0705"), None);
        assert_eq!(parse_clock(""), None);
        assert_eq!(format_clock(425), "07:05");
    }

    #[test]
    fn invalid_quiet_hours_are_refused() {
        let every = vec!["mon".to_owned()];
        assert_eq!(
            quiet_from_form(true, "25:00", "07:00", &every),
            Err("Pick a start time.")
        );
        assert_eq!(
            quiet_from_form(true, "22:00", "", &every),
            Err("Pick an end time.")
        );
        assert_eq!(
            quiet_from_form(true, "08:00", "08:00", &every),
            Err("Quiet hours need different start and end times.")
        );
        assert_eq!(
            quiet_from_form(true, "22:00", "07:00", &["funday".to_owned()]),
            Err("That day isn't a day of the week.")
        );
        assert_eq!(
            quiet_from_form(true, "22:00", "07:00", &[]),
            Err("Choose at least one day for quiet hours.")
        );
        // Off, with no days, is allowed: it just keeps the times.
        assert_eq!(
            quiet_from_form(false, "22:00", "07:00", &[]).unwrap().days,
            0
        );
    }

    #[test]
    fn the_form_builds_a_day_bitmask_from_monday() {
        let days = ["mon", "wed", "sun"].map(str::to_owned);
        let quiet = quiet_from_form(true, "22:00", "07:00", &days).unwrap();
        assert_eq!(quiet.days, 0b100_0101);
        assert!(day_is_on(quiet.days, 6));
        assert!(!day_is_on(quiet.days, 1));
    }

    #[test]
    fn pauses_end_in_an_hour_eight_hours_or_tomorrow_morning() {
        let zone = TimeZone::get("Europe/Berlin").unwrap();
        let now = berlin(8, 14, 30).timestamp().as_millisecond();
        let until = |choice| pause_until(choice, now, &zone).unwrap().unwrap();
        assert_eq!(until("1h"), now + HOUR_MS);
        assert_eq!(until("8h"), now + 8 * HOUR_MS);
        assert_eq!(
            describe_until(until("tomorrow"), &berlin(8, 14, 30)),
            "Mon 9 Nov, 08:00"
        );
        assert_eq!(pause_until("resume", now, &zone).unwrap(), None);
        assert!(pause_until("forever", now, &zone).is_err());
    }

    #[test]
    fn a_pause_ending_today_shows_only_the_time() {
        let now = berlin(8, 12, 0);
        let today = berlin(8, 14, 30).timestamp().as_millisecond();
        assert_eq!(describe_until(today, &now), "14:30");
    }
}
