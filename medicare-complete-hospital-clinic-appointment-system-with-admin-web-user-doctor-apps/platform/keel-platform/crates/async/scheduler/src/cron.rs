//! Wave 6 P4: zero-dep 5-field cron — `minute hour day-of-month month
//! day-of-week` with `*`, lists (`1,15`), ranges (`1-5`) and steps
//! (`*/5`, `1-30/2`) (docs/19). Numbers only: no `MON` names, no `?`.
//! Anything else refuses `ScheduleRegistry::register` at boot instead of
//! silently never running.
//!
//! Next-occurrence math walks day candidates in a wall clock (`offset`), so
//! the same parse serves UTC items and per-timezone items. Day-of-week is
//! `0..=6` with Sunday = 0 (`7` is accepted and normalized to Sunday).

use kelson_foundation::AppError;
use time::{Date, Duration, Month, OffsetDateTime, Time, UtcOffset, Weekday};

/// A parsed, validated cron expression. Values are stored ascending and
/// deduplicated; the day-of-week set is normalized (`7` → `0`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronExpr {
    minutes: Vec<u8>,
    hours: Vec<u8>,
    days_of_month: Vec<u8>,
    months: Vec<u8>,
    days_of_week: Vec<u8>,
    /// The day-of-month field was literally `*`.
    dom_any: bool,
    /// The day-of-week field was literally `*`.
    dow_any: bool,
}

impl CronExpr {
    /// Parse a 5-field expression. Invalid input names the field and value
    /// (registration error, never a silent fallback).
    pub fn parse(expr: &str) -> Result<Self, AppError> {
        let fields: Vec<&str> = expr.split_whitespace().collect();
        if fields.len() != 5 {
            return Err(invalid(
                expr,
                format!(
                    "expected 5 fields (minute hour day-of-month month day-of-week), found {}",
                    fields.len()
                ),
            ));
        }
        let minutes = parse_field(expr, "minute", fields[0], 0, 59)?;
        let hours = parse_field(expr, "hour", fields[1], 0, 23)?;
        let days_of_month = parse_field(expr, "day-of-month", fields[2], 1, 31)?;
        let months = parse_field(expr, "month", fields[3], 1, 12)?;
        let days_of_week = normalize_dow(parse_field(expr, "day-of-week", fields[4], 0, 7)?);
        Ok(Self {
            minutes,
            hours,
            days_of_month,
            months,
            days_of_week,
            dom_any: fields[2] == "*",
            dow_any: fields[4] == "*",
        })
    }

    /// First occurrence strictly after `from`, searched in the wall clock
    /// at `offset` and returned as a UTC instant. `None` when no date
    /// within the next 1462 days (a full leap cycle) matches — e.g.
    /// `0 0 31 2 *` refuses registration instead of never firing.
    pub fn next_after(&self, from: OffsetDateTime, offset_secs: i32) -> Option<OffsetDateTime> {
        let offset = UtcOffset::from_whole_seconds(offset_secs).ok()?;
        let local = from.to_offset(offset);
        // Smallest minute-aligned instant strictly after `from` in this zone.
        let first = local + Duration::minutes(1);
        let first = first
            - Duration::seconds(i64::from(first.second()))
            - Duration::nanoseconds(i64::from(first.nanosecond()));
        let first_date = first.date();
        let (start_h, start_m) = (first.hour(), first.minute());
        let mut day = first_date;
        for _ in 0..1462 {
            if self.day_matches(day) {
                let (from_h, from_m) = if day == first_date {
                    (start_h, start_m)
                } else {
                    (0, 0)
                };
                for &hour in &self.hours {
                    if hour < from_h {
                        continue;
                    }
                    for &minute in &self.minutes {
                        if hour == from_h && minute < from_m {
                            continue;
                        }
                        let time = Time::from_hms(hour, minute, 0).ok()?;
                        let candidate = day.with_time(time).assume_offset(offset);
                        return Some(candidate.to_offset(UtcOffset::UTC));
                    }
                }
            }
            day = day.checked_add(Duration::days(1))?;
        }
        None
    }

    /// Month gate plus the day gate. When BOTH day fields are restricted
    /// (neither is `*`), classic Vixie semantics apply: the expression fires
    /// when EITHER matches (`0 0 1 * 1` = the 1st OR every Monday).
    fn day_matches(&self, day: Date) -> bool {
        if !self.months.contains(&month_number(day.month())) {
            return false;
        }
        if self.dom_any && self.dow_any {
            return true;
        }
        if self.dom_any {
            return self.days_of_week.contains(&weekday_number(day.weekday()));
        }
        if self.dow_any {
            return self.days_of_month.contains(&day.day());
        }
        self.days_of_month.contains(&day.day())
            || self.days_of_week.contains(&weekday_number(day.weekday()))
    }
}

fn invalid(expr: &str, detail: String) -> AppError {
    AppError::validation(format!("invalid cron expression `{expr}`: {detail}"))
}

/// One field: comma-separated items, each `*`, `n`, `lo-hi`, optionally
/// followed by `/step` (`*/5`, `1-30/2`). Values are range-checked against
/// the field's bounds before expansion.
fn parse_field(expr: &str, label: &str, spec: &str, min: u8, max: u8) -> Result<Vec<u8>, AppError> {
    if spec.is_empty() {
        return Err(invalid(expr, format!("{label} field is empty")));
    }
    let mut values: Vec<u8> = Vec::new();
    for raw in spec.split(',') {
        let part = raw.trim();
        if part.is_empty() {
            return Err(invalid(expr, format!("{label} has an empty list item")));
        }
        let (range_part, step) = match part.split_once('/') {
            Some((range_part, step_part)) => {
                let step: u16 = step_part.trim().parse().map_err(|_| {
                    invalid(
                        expr,
                        format!("{label} step `{}` is not a number", step_part.trim()),
                    )
                })?;
                if step == 0 {
                    return Err(invalid(expr, format!("{label} step must be >= 1")));
                }
                (range_part.trim(), step)
            }
            None => (part, 1u16),
        };
        let (lo, hi) = match range_part {
            "*" => (min, max),
            single if !single.contains('-') => {
                let v = parse_value(expr, label, single)?;
                (v, v)
            }
            range => {
                let (a, b) = range.split_once('-').expect("contains '-'");
                let lo = parse_value(expr, label, a)?;
                let hi = parse_value(expr, label, b)?;
                if lo > hi {
                    return Err(invalid(expr, format!("{label} range `{range}` descends")));
                }
                (lo, hi)
            }
        };
        if lo < min || hi > max {
            return Err(invalid(expr, format!("{label} value outside {min}-{max}")));
        }
        let mut v = u16::from(lo);
        while v <= u16::from(hi) {
            values.push(v as u8);
            v += step;
        }
    }
    values.sort_unstable();
    values.dedup();
    Ok(values)
}

fn parse_value(expr: &str, label: &str, raw: &str) -> Result<u8, AppError> {
    raw.trim().parse().map_err(|_| {
        invalid(
            expr,
            format!("{label} value `{}` is not a number", raw.trim()),
        )
    })
}

fn normalize_dow(mut values: Vec<u8>) -> Vec<u8> {
    for v in &mut values {
        if *v == 7 {
            *v = 0;
        }
    }
    values.sort_unstable();
    values.dedup();
    values
}

fn month_number(month: Month) -> u8 {
    match month {
        Month::January => 1,
        Month::February => 2,
        Month::March => 3,
        Month::April => 4,
        Month::May => 5,
        Month::June => 6,
        Month::July => 7,
        Month::August => 8,
        Month::September => 9,
        Month::October => 10,
        Month::November => 11,
        Month::December => 12,
    }
}

fn weekday_number(day: Weekday) -> u8 {
    match day {
        Weekday::Sunday => 0,
        Weekday::Monday => 1,
        Weekday::Tuesday => 2,
        Weekday::Wednesday => 3,
        Weekday::Thursday => 4,
        Weekday::Friday => 5,
        Weekday::Saturday => 6,
    }
}

// -------------------------------------------------------------- builders ---

/// `0 * * * *` — every hour on the hour.
pub fn hourly() -> String {
    "0 * * * *".to_string()
}

/// `0 0 * * *` — daily at midnight.
pub fn daily() -> String {
    "0 0 * * *".to_string()
}

/// `0 <hour> * * *` from `"HH:MM"` (same clock rules as the `Daily` spec).
pub fn daily_at(hhmm: &str) -> Result<String, AppError> {
    let (hour, minute) = hhmm
        .split_once(':')
        .ok_or_else(|| AppError::validation(format!("daily_at expects `HH:MM`, found `{hhmm}`")))?;
    let hour: u8 = hour
        .trim()
        .parse()
        .map_err(|_| AppError::validation(format!("daily_at hour `{hour}` is not a number")))?;
    let minute: u8 = minute
        .trim()
        .parse()
        .map_err(|_| AppError::validation(format!("daily_at minute `{minute}` is not a number")))?;
    if hour > 23 || minute > 59 {
        return Err(AppError::validation(format!(
            "daily_at time `{hhmm}` out of range"
        )));
    }
    Ok(format!("{minute} {hour} * * *"))
}

/// `0 0 * * 0` — weekly on Sunday at midnight.
pub fn weekly() -> String {
    "0 0 * * 0".to_string()
}

/// `0 0 1 * *` — monthly on the 1st at midnight.
pub fn monthly() -> String {
    "0 0 1 * *".to_string()
}

/// `*/<n> * * * *` for `n` in `1..=59` (`60` would be `hourly()`).
pub fn every_minutes(n: u8) -> Result<String, AppError> {
    if !(1..=59).contains(&n) {
        return Err(AppError::validation(format!(
            "every_minutes({n}) out of range: expected 1..=59"
        )));
    }
    Ok(format!("*/{n} * * * *"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn star_fields_parse_with_full_ranges() {
        let e = CronExpr::parse("* * * * *").unwrap();
        assert_eq!(e.minutes, (0..=59).collect::<Vec<u8>>());
        assert_eq!(e.hours, (0..=23).collect::<Vec<u8>>());
        assert_eq!(e.days_of_month, (1..=31).collect::<Vec<u8>>());
        assert_eq!(e.months, (1..=12).collect::<Vec<u8>>());
        assert_eq!(e.days_of_week, (0..=6).collect::<Vec<u8>>());
        assert!(e.dom_any && e.dow_any);
        assert_eq!(
            e.next_after(datetime!(2026-09-30 10:00:00 UTC), 0),
            Some(datetime!(2026-09-30 10:01:00 UTC))
        );
    }

    #[test]
    fn lists_ranges_and_steps_expand() {
        assert_eq!(
            CronExpr::parse("1,15,45 * * * *").unwrap().minutes,
            vec![1, 15, 45]
        );
        assert_eq!(
            CronExpr::parse("0 9-17 * * *").unwrap().hours,
            (9..=17).collect::<Vec<u8>>()
        );
        assert_eq!(
            CronExpr::parse("*/15 * * * *").unwrap().minutes,
            vec![0, 15, 30, 45]
        );
        assert_eq!(
            CronExpr::parse("10-30/5 0 1-31/2 * *").unwrap().minutes,
            vec![10, 15, 20, 25, 30]
        );
        assert_eq!(
            CronExpr::parse("10-30/5 0 1-31/2 * *")
                .unwrap()
                .days_of_month,
            (1..=31).step_by(2).collect::<Vec<u8>>()
        );
    }

    #[test]
    fn day_of_week_seven_is_sunday() {
        assert_eq!(CronExpr::parse("0 0 * * 7").unwrap().days_of_week, vec![0]);
        assert_eq!(CronExpr::parse("0 0 * * 0").unwrap().days_of_week, vec![0]);
        assert_eq!(
            CronExpr::parse("0 0 * * 5-7").unwrap().days_of_week,
            vec![0, 5, 6]
        );
    }

    #[test]
    fn invalid_expressions_are_rejected_with_a_reason() {
        for expr in [
            "",
            "* * * *",
            "* * * * * *",
            "60 * * * *",
            "* 24 * * *",
            "0 0 0 * *",
            "0 0 32 * *",
            "0 0 * 13 *",
            "* * * * 8",
            "*/0 * * * *",
            "5-1 * * * *",
            "a * * * *",
            "1,,2 * * * *",
            "0 0 * * MON",
            "0 0 * * ?",
            "0 0/1.5 * * *",
        ] {
            let err = CronExpr::parse(expr)
                .err()
                .unwrap_or_else(|| panic!("expected rejection of {expr:?}"));
            assert!(
                err.public_message.contains("invalid cron expression"),
                "message for {expr:?}: {}",
                err.public_message
            );
        }
    }

    #[test]
    fn impossible_dates_never_fire() {
        let e = CronExpr::parse("0 0 31 2 *").unwrap();
        assert_eq!(e.next_after(datetime!(2026-09-30 10:00:00 UTC), 0), None);
    }

    #[test]
    fn daily_expression_walks_strictly_after_from() {
        let e = CronExpr::parse("30 14 * * *").unwrap();
        // Before the slot today.
        assert_eq!(
            e.next_after(datetime!(2026-09-30 10:00:00 UTC), 0),
            Some(datetime!(2026-09-30 14:30:00 UTC))
        );
        assert_eq!(
            e.next_after(datetime!(2026-09-30 14:29:30 UTC), 0),
            Some(datetime!(2026-09-30 14:30:00 UTC))
        );
        // On / past the slot: strictly after → tomorrow.
        assert_eq!(
            e.next_after(datetime!(2026-09-30 14:30:00 UTC), 0),
            Some(datetime!(2026-10-01 14:30:00 UTC))
        );
        assert_eq!(
            e.next_after(datetime!(2026-09-30 14:30:30 UTC), 0),
            Some(datetime!(2026-10-01 14:30:00 UTC))
        );
    }

    #[test]
    fn weekly_expression_finds_the_next_sunday_and_monday() {
        // 2026-09-30 is a Wednesday.
        let sunday = CronExpr::parse("0 0 * * 0").unwrap();
        assert_eq!(
            sunday.next_after(datetime!(2026-09-30 10:00:00 UTC), 0),
            Some(datetime!(2026-10-04 00:00:00 UTC))
        );
        let monday = CronExpr::parse("0 0 * * 1").unwrap();
        assert_eq!(
            monday.next_after(datetime!(2026-09-30 10:00:00 UTC), 0),
            Some(datetime!(2026-10-05 00:00:00 UTC))
        );
    }

    #[test]
    fn restricted_dom_and_dow_fire_on_either() {
        // 1st of the month OR any Monday — 2026-10-01 (Thursday) comes first.
        let e = CronExpr::parse("0 0 1 * 1").unwrap();
        assert_eq!(
            e.next_after(datetime!(2026-09-30 10:00:00 UTC), 0),
            Some(datetime!(2026-10-01 00:00:00 UTC))
        );
        // From the 1st, the next Monday (2026-10-05).
        assert_eq!(
            e.next_after(datetime!(2026-10-01 00:00:00 UTC), 0),
            Some(datetime!(2026-10-05 00:00:00 UTC))
        );
    }

    #[test]
    fn monthly_expression_finds_the_first() {
        let e = CronExpr::parse("0 0 1 * *").unwrap();
        assert_eq!(
            e.next_after(datetime!(2026-09-15 10:00:00 UTC), 0),
            Some(datetime!(2026-10-01 00:00:00 UTC))
        );
    }

    #[test]
    fn offset_searches_the_item_wall_clock() {
        let e = CronExpr::parse("0 9 * * *").unwrap();
        // 03:00 UTC = 08:30 IST → 09:00 IST today = 03:30 UTC.
        assert_eq!(
            e.next_after(datetime!(2026-09-30 03:00:00 UTC), 19_800),
            Some(datetime!(2026-09-30 03:30:00 UTC))
        );
        // 04:00 UTC = 09:30 IST → 09:00 IST tomorrow = 03:30 UTC next day.
        assert_eq!(
            e.next_after(datetime!(2026-09-30 04:00:00 UTC), 19_800),
            Some(datetime!(2026-10-01 03:30:00 UTC))
        );
    }

    #[test]
    fn builder_helpers_map_to_cron_strings() {
        assert_eq!(hourly(), "0 * * * *");
        assert_eq!(daily(), "0 0 * * *");
        assert_eq!(daily_at("13:00").unwrap(), "0 13 * * *");
        assert_eq!(weekly(), "0 0 * * 0");
        assert_eq!(monthly(), "0 0 1 * *");
        assert_eq!(every_minutes(15).unwrap(), "*/15 * * * *");
        // Every output is itself a valid expression.
        for expr in [
            hourly(),
            daily(),
            daily_at("13:00").unwrap(),
            weekly(),
            monthly(),
            every_minutes(15).unwrap(),
        ] {
            CronExpr::parse(&expr).unwrap_or_else(|e| panic!("{expr}: {}", e.public_message));
        }
    }

    #[test]
    fn builder_helpers_refuse_bad_input() {
        assert!(daily_at("13").is_err());
        assert!(daily_at("25:00").is_err());
        assert!(daily_at("12:61").is_err());
        assert!(every_minutes(0).is_err());
        assert!(every_minutes(60).is_err());
    }
}
