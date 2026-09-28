//! Cron schedules for `sideporch.cron`.
//!
//! Standard five fields, `minute hour day-of-month month day-of-week`, with
//! `*`, lists (`1,15`), ranges (`1-5`), steps (`*/10`, `8-18/2`), month and
//! weekday names (`jan`, `mon-fri`), `7` as Sunday, and the shortcuts
//! `@hourly`, `@daily`, `@weekly`, `@monthly` and `@yearly`. As in Vixie
//! cron, when both day fields are restricted a day matches either one.
//! Times are wall-clock times in the schedule's time zone.

use jiff::{Timestamp, ToSpan as _, Zoned, civil, tz::TimeZone};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cron {
    expression: String,
    minutes: u64,
    hours: u32,
    days: u32,
    months: u16,
    weekdays: u8,
    days_restricted: bool,
    weekdays_restricted: bool,
}

const MONTHS: &[&str] = &[
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];
const WEEKDAYS: &[&str] = &["sun", "mon", "tue", "wed", "thu", "fri", "sat"];

fn value(text: &str, names: &[&str], offset: u8) -> Result<u8, String> {
    if let Some(index) = names
        .iter()
        .position(|name| name.eq_ignore_ascii_case(text))
    {
        let index = u8::try_from(index).map_err(|_| "bad name".to_owned())?;
        return Ok(index.saturating_add(offset));
    }
    text.parse::<u8>()
        .map_err(|_| format!("`{text}` is not a number"))
}

/// Parses one field into a bit set of allowed values.
fn field(text: &str, min: u8, max: u8, names: &[&str], name_offset: u8) -> Result<u64, String> {
    let mut bits = 0_u64;
    for part in text.split(',') {
        let (range, step) = match part.split_once('/') {
            Some((range, step)) => {
                let step: u8 = step
                    .parse()
                    .map_err(|_| format!("`{step}` is not a step"))?;
                if step == 0 {
                    return Err("a step must be at least 1".to_owned());
                }
                (range, step)
            }
            None => (part, 1),
        };
        let (start, end) = if range == "*" {
            (min, max)
        } else if let Some((start, end)) = range.split_once('-') {
            (
                value(start, names, name_offset)?,
                value(end, names, name_offset)?,
            )
        } else {
            let single = value(range, names, name_offset)?;
            // `5/15` means from 5 to the end, every 15.
            (single, if part.contains('/') { max } else { single })
        };
        if start < min || end > max || start > end {
            return Err(format!("`{part}` is outside {min}-{max}"));
        }
        let mut current = start;
        while current <= end {
            bits |= 1_u64.checked_shl(u32::from(current)).unwrap_or(0);
            match current.checked_add(step) {
                Some(next) => current = next,
                None => break,
            }
        }
    }
    Ok(bits)
}

fn has(bits: u64, value: i8) -> bool {
    u32::try_from(value)
        .ok()
        .and_then(|shift| 1_u64.checked_shl(shift))
        .is_some_and(|bit| bits & bit != 0)
}

impl Cron {
    pub fn parse(expression: &str) -> Result<Self, String> {
        let expanded = match expression.trim() {
            "@hourly" => "0 * * * *",
            "@daily" | "@midnight" => "0 0 * * *",
            "@weekly" => "0 0 * * 0",
            "@monthly" => "0 0 1 * *",
            "@yearly" | "@annually" => "0 0 1 1 *",
            other => other,
        };
        let fields: Vec<&str> = expanded.split_whitespace().collect();
        let [minute, hour, day, month, weekday] = fields.as_slice() else {
            return Err(format!(
                "`{expression}` needs five fields: minute hour day-of-month month day-of-week"
            ));
        };
        let weekdays = field(weekday, 0, 7, WEEKDAYS, 0)?;
        // 7 is another name for Sunday.
        let weekdays = (weekdays | (weekdays >> 7)) & 0x7f;
        Ok(Self {
            expression: expression.trim().to_owned(),
            minutes: field(minute, 0, 59, &[], 0)?,
            hours: u32::try_from(field(hour, 0, 23, &[], 0)?).map_err(|_| "bad hour")?,
            days: u32::try_from(field(day, 1, 31, &[], 0)?).map_err(|_| "bad day")?,
            months: u16::try_from(field(month, 1, 12, MONTHS, 1)?).map_err(|_| "bad month")?,
            weekdays: u8::try_from(weekdays).map_err(|_| "bad weekday")?,
            days_restricted: *day != "*",
            weekdays_restricted: *weekday != "*",
        })
    }

    pub fn expression(&self) -> &str {
        &self.expression
    }

    fn day_matches(&self, date: civil::Date) -> bool {
        let day = has(u64::from(self.days), date.day());
        let weekday = has(
            u64::from(self.weekdays),
            date.weekday().to_sunday_zero_offset(),
        );
        match (self.days_restricted, self.weekdays_restricted) {
            (true, true) => day || weekday,
            (true, false) => day,
            (false, true) => weekday,
            (false, false) => true,
        }
    }

    /// The first matching minute strictly after `after`, in `zone`. Times
    /// skipped by a daylight saving change are skipped here too.
    pub fn next_after(&self, after: Timestamp, zone: &TimeZone) -> Option<Timestamp> {
        let start = after.to_zoned(zone.clone());
        let mut date = start.date();
        // Looking four years ahead covers every valid schedule, even 29 Feb.
        for _ in 0..1_500 {
            if has(u64::from(self.months), date.month()) && self.day_matches(date) {
                for hour in 0..24_i8 {
                    if !has(u64::from(self.hours), hour) {
                        continue;
                    }
                    for minute in 0..60_i8 {
                        if !has(self.minutes, minute) {
                            continue;
                        }
                        let Ok(time) = civil::Time::new(hour, minute, 0, 0) else {
                            continue;
                        };
                        let Ok(zoned) = date.to_datetime(time).to_zoned(zone.clone()) else {
                            continue;
                        };
                        // Skip wall-clock times that do not exist today.
                        if zoned.datetime() != date.to_datetime(time) {
                            continue;
                        }
                        if zoned.timestamp() > start.timestamp() {
                            return Some(zoned.timestamp());
                        }
                    }
                }
            }
            date = date.checked_add(1.day()).ok()?;
        }
        None
    }

    /// The next `count` run times after `after`.
    pub fn upcoming(&self, after: Timestamp, zone: &TimeZone, count: usize) -> Vec<Zoned> {
        let mut runs = Vec::with_capacity(count);
        let mut cursor = after;
        while runs.len() < count {
            let Some(next) = self.next_after(cursor, zone) else {
                break;
            };
            runs.push(next.to_zoned(zone.clone()));
            cursor = next;
        }
        runs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> Timestamp {
        text.parse().unwrap()
    }

    fn next(expression: &str, after: &str, zone: &str) -> String {
        let zone = TimeZone::get(zone).unwrap();
        Cron::parse(expression)
            .unwrap()
            .next_after(at(after), &zone)
            .unwrap()
            .to_zoned(zone)
            .strftime("%Y-%m-%d %H:%M %a")
            .to_string()
    }

    #[test]
    fn finds_the_next_run() {
        assert_eq!(
            next("*/15 * * * *", "2026-09-28T10:07:00Z", "UTC"),
            "2026-09-28 10:15 Mon"
        );
        assert_eq!(
            next("0 9 * * mon-fri", "2026-09-26T12:00:00Z", "UTC"),
            "2026-09-28 09:00 Mon"
        );
        assert_eq!(
            next("@monthly", "2026-09-28T00:00:00Z", "UTC"),
            "2026-10-01 00:00 Thu"
        );
        assert_eq!(
            next("30 8 29 2 *", "2026-03-01T00:00:00Z", "UTC"),
            "2028-02-29 08:30 Tue"
        );
        // 7 means Sunday too.
        assert_eq!(
            next("0 12 * * 7", "2026-09-28T00:00:00Z", "UTC"),
            "2026-10-04 12:00 Sun"
        );
    }

    #[test]
    fn restricted_days_match_either_field() {
        // The 1st of the month or any Friday.
        assert_eq!(
            next("0 0 1 * fri", "2026-09-28T00:00:00Z", "UTC"),
            "2026-10-01 00:00 Thu"
        );
        assert_eq!(
            next("0 0 1 * fri", "2026-10-01T01:00:00Z", "UTC"),
            "2026-10-02 00:00 Fri"
        );
    }

    #[test]
    fn uses_the_time_zone() {
        // 09:00 in Berlin is 07:00 UTC in summer time.
        let zone = TimeZone::get("Europe/Berlin").unwrap();
        let run = Cron::parse("0 9 * * *")
            .unwrap()
            .next_after(at("2026-09-28T08:00:00Z"), &zone)
            .unwrap();
        assert_eq!(run, at("2026-09-29T07:00:00Z"));
        // 02:30 does not exist on the night clocks go forward.
        assert_eq!(
            next("30 2 * * *", "2026-03-28T12:00:00Z", "Europe/Berlin"),
            "2026-03-30 02:30 Mon"
        );
    }

    #[test]
    fn rejects_bad_expressions() {
        for bad in [
            "* * * *",
            "60 * * * *",
            "* * 0 * *",
            "*/0 * * * *",
            "5-1 * * * *",
            "* * * foo *",
        ] {
            assert!(Cron::parse(bad).is_err(), "{bad}");
        }
        assert_eq!(
            Cron::parse("1 2 3").unwrap_err(),
            "`1 2 3` needs five fields: minute hour day-of-month month day-of-week"
        );
    }

    #[test]
    fn lists_upcoming_runs() {
        let runs = Cron::parse("0 */6 * * *").unwrap().upcoming(
            at("2026-09-28T01:00:00Z"),
            &TimeZone::UTC,
            3,
        );
        let hours: Vec<i8> = runs.iter().map(Zoned::hour).collect();
        assert_eq!(hours, [6, 12, 18]);
    }
}
