//! Date helpers (Wave 8 P2, docs/19): parse/format over the closed
//! `Y m d H i s` token set and timezone conversions over the bundled offset
//! table ([`tz`](crate::tz)).
//!
//! The token grammar is THE grammar the `date_format` validation rule
//! enforces — `keel-validation` delegates its spec check and value match
//! here, so "same tokens as the `date_format` rule" is structural, not a
//! convention. Every ASCII letter outside the closed set is a fail-closed
//! error (identical message on both the rule and the helper path); any
//! non-letter is a literal separator.

use time::{OffsetDateTime, PrimitiveDateTime};

/// The closed format-token set (Laravel `date_format` parity).
pub const DATE_FORMAT_TOKENS: &str = "YmdHis";

/// Boot validation for the format spec: closed PHP-style token set
/// (`Y m d H i s`); every other ASCII letter is a fail-closed error and
/// any non-letter is a literal separator. `keel-validation`'s
/// `date_format:<fmt>` rule parse calls this.
pub fn check_date_format_spec(fmt: &str) -> Result<(), String> {
    if fmt.is_empty() {
        return Err("date_format requires a format: date_format:<fmt>".into());
    }
    for c in fmt.chars() {
        if c.is_ascii_alphabetic() && !DATE_FORMAT_TOKENS.contains(c) {
            return Err(format!(
                "unsupported date_format token {c:?} (closed set: Y m d H i s)"
            ));
        }
    }
    Ok(())
}

/// Components captured by [`parse_date_with`] — all optional because a
/// format may legitimately omit them (`H:i` has no date, `m-d` has no year).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateParts {
    pub year: Option<i64>,
    pub month: Option<i64>,
    pub day: Option<i64>,
    pub hour: Option<i64>,
    pub minute: Option<i64>,
    pub second: Option<i64>,
}

impl DateParts {
    /// The captured wall clock as an absolute instant, assuming UTC for
    /// components the format did not tie to a zone (convert with
    /// [`in_tz`] when a zone applies). Needs year + month + day; a
    /// dateless format (`H:i`) has no instant and returns `None`.
    pub fn to_instant(&self) -> Option<OffsetDateTime> {
        let (y, m, d) = (self.year?, self.month?, self.day?);
        let month = u8::try_from(m).ok().and_then(|m| time::Month::try_from(m).ok())?;
        let date = time::Date::from_calendar_date(
            i32::try_from(y).ok()?,
            month,
            u8::try_from(d).ok()?,
        )
        .ok()?;
        let at = time::Time::from_hms(
            u8::try_from(self.hour.unwrap_or(0)).ok()?,
            u8::try_from(self.minute.unwrap_or(0)).ok()?,
            u8::try_from(self.second.unwrap_or(0)).ok()?,
        )
        .ok()?;
        Some(PrimitiveDateTime::new(date, at).assume_utc())
    }
}

/// Match a value against the closed `Y m d H i s` format: fixed digit counts
/// per token, exact literal consumption (no trailing characters), then
/// calendar/time range checks on whatever components the format captured.
/// Returns `None` on any mismatch (the `date_format` rule's boolean answer
/// is `parse_date_with(..).is_some()`).
pub fn parse_date_with(s: &str, fmt: &str) -> Option<DateParts> {
    let mut chars = s.chars().peekable();
    let mut parts = DateParts {
        year: None,
        month: None,
        day: None,
        hour: None,
        minute: None,
        second: None,
    };
    for spec in fmt.chars() {
        let taken = match spec {
            'Y' => take_digits(&mut chars, 4),
            'm' => take_digits(&mut chars, 2),
            'd' => take_digits(&mut chars, 2),
            'H' => take_digits(&mut chars, 2),
            'i' => take_digits(&mut chars, 2),
            's' => take_digits(&mut chars, 2),
            literal => chars.next().filter(|&c| c == literal).map(|_| 0),
        };
        let taken = taken?;
        match spec {
            'Y' => parts.year = Some(taken),
            'm' => parts.month = Some(taken),
            'd' => parts.day = Some(taken),
            'H' => parts.hour = Some(taken),
            'i' => parts.minute = Some(taken),
            's' => parts.second = Some(taken),
            _ => {}
        }
    }
    if chars.next().is_some() {
        return None; // trailing characters
    }
    if parts.hour.is_some_and(|h| h > 23)
        || parts.minute.is_some_and(|m| m > 59)
        || parts.second.is_some_and(|s| s > 59)
    {
        return None;
    }
    match (parts.month, parts.day) {
        (Some(mo), Some(d)) => {
            if !(1..=12).contains(&mo) {
                return None;
            }
            match parts.year {
                Some(y) => date_parts_valid(y, mo, d),
                // No year in the format: validate against a non-leap probe
                // year (2001), so `m-d` can never claim 02-29 or 02-30 —
                // parity: unchanged `date_format` rule behavior.
                None => (1..=days_in_month(2001, mo)).contains(&d),
            }
            .then_some(parts)
        }
        (Some(mo), None) => (1..=12).contains(&mo).then_some(parts),
        (None, Some(d)) => (1..=31).contains(&d).then_some(parts),
        (None, None) => Some(parts),
    }
}

/// Render an instant through the closed token set, reading the components
/// from the instant's OWN offset (pair with [`in_tz`] to render a zone's
/// wall clock). Any ASCII letter outside the set fails with the spec-check
/// message — the same grammar on both sides.
pub fn format_date_with(at: OffsetDateTime, fmt: &str) -> Result<String, String> {
    check_date_format_spec(fmt)?;
    let mut out = String::with_capacity(fmt.len());
    for spec in fmt.chars() {
        match spec {
            'Y' => out.push_str(&format!("{:04}", at.year())),
            'm' => out.push_str(&format!("{:02}", u8::from(at.month()))),
            'd' => out.push_str(&format!("{:02}", at.day())),
            'H' => out.push_str(&format!("{:02}", at.hour())),
            'i' => out.push_str(&format!("{:02}", at.minute())),
            's' => out.push_str(&format!("{:02}", at.second())),
            other => out.push(other),
        }
    }
    Ok(out)
}

/// The current instant rendered in `tz` (the instant with the zone's
/// bundled offset applied), `None` for a zone outside the table — the
/// helpers never convert in a zone they cannot know.
pub fn now_in(tz: &str) -> Option<OffsetDateTime> {
    in_tz(OffsetDateTime::now_utc(), tz)
}

/// Shift an instant to `tz`'s offset (same instant, zone wall clock);
/// `None` for a zone outside the bundled [`tz`](crate::tz) table.
pub fn in_tz(at: OffsetDateTime, tz: &str) -> Option<OffsetDateTime> {
    let offset = crate::tz::timezone_offset_secs(tz)?;
    let offset = time::UtcOffset::from_whole_seconds(offset).ok()?;
    Some(at.to_offset(offset))
}

/// The zone wall clock of an instant as a naive [`PrimitiveDateTime`]
/// (no offset attached) — the shape a `datetime:<tz>` model cast stores
/// the local reading of. `None` for a zone outside the table.
pub fn local_wall(at: OffsetDateTime, tz: &str) -> Option<PrimitiveDateTime> {
    in_tz(at, tz)
        .map(|shifted| PrimitiveDateTime::new(shifted.date(), shifted.time()))
}

/// Consume exactly `n` ASCII digits as a number (whole token, no prefix).
fn take_digits(chars: &mut std::iter::Peekable<std::str::Chars<'_>>, n: usize) -> Option<i64> {
    let mut out = 0i64;
    for _ in 0..n {
        let c = chars.next()?;
        if !c.is_ascii_digit() {
            return None;
        }
        out = out * 10 + i64::from(c.to_digit(10)?);
    }
    Some(out)
}

/// Real calendar validity (leap years included) for a captured Y/M/D.
fn date_parts_valid(y: i64, m: i64, d: i64) -> bool {
    let Ok(month) = time::Month::try_from(m as u8) else {
        return false;
    };
    let (Ok(y), Ok(d)) = (i32::try_from(y), u8::try_from(d)) else {
        return false;
    };
    time::Date::from_calendar_date(y, month, d).is_ok()
}

/// Days in a month, leap-year aware (Gregorian).
fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instant() -> OffsetDateTime {
        // 2024-01-02T03:04:05Z
        time::Date::from_calendar_date(2024, time::Month::January, 2)
            .unwrap()
            .with_hms(3, 4, 5)
            .unwrap()
            .assume_utc()
    }

    #[test]
    fn date_format_spec_is_the_closed_token_set() {
        assert!(check_date_format_spec("Y-m-d H:i:s").is_ok());
        assert!(check_date_format_spec("d/m/Y").is_ok());
        assert_eq!(
            check_date_format_spec(""),
            Err("date_format requires a format: date_format:<fmt>".into())
        );
        let err = check_date_format_spec("Y-m-d x").unwrap_err();
        assert!(err.contains("unsupported date_format token"), "{err}");
        assert!(err.contains("closed set: Y m d H i s"), "{err}");
    }

    #[test]
    fn parse_reads_the_closed_grammar() {
        let parts = parse_date_with("2024-01-02", "Y-m-d").expect("valid");
        assert_eq!(
            parts,
            DateParts {
                year: Some(2024),
                month: Some(1),
                day: Some(2),
                hour: None,
                minute: None,
                second: None,
            }
        );
        // Day-first formats flip the components.
        let eu = parse_date_with("02/01/2024", "d/m/Y").expect("valid");
        assert_eq!((eu.day, eu.month, eu.year), (Some(2), Some(1), Some(2024)));

        // Trailing characters, bad digits, impossible ranges all fail.
        assert!(parse_date_with("2024-01-02x", "Y-m-d").is_none());
        assert!(parse_date_with("2024-1-2", "Y-m-d").is_none());
        assert!(parse_date_with("2024-13-01", "Y-m-d").is_none());
        assert!(parse_date_with("2024-02-30", "Y-m-d").is_none()); // calendar
        assert!(parse_date_with("2023-02-29", "Y-m-d").is_none()); // not a leap year
        assert!(parse_date_with("2024-01-02 25:00:00", "Y-m-d H:i:s").is_none());
    }

    #[test]
    fn parse_keeps_partial_formats_and_refuses_lying_ranges() {
        // No date in the format: valid parse, but no instant exists.
        let time_only = parse_date_with("10:30", "H:i").expect("valid");
        assert_eq!((time_only.hour, time_only.minute), (Some(10), Some(30)));
        assert!(time_only.to_instant().is_none());

        // No year in the format: the probe year (2001) is non-leap, so
        // February tops out at 28 — 02-29 is refused just like 02-30
        // (parity: unchanged `date_format` rule behavior).
        assert!(parse_date_with("02-29", "m-d").is_none());
        assert!(parse_date_with("02-30", "m-d").is_none());
        assert!(parse_date_with("13-01", "m-d").is_none());
        // February with a real leap year accepts 02-29 through the
        // calendar check (the non-leap refusal is asserted above).
        assert!(parse_date_with("2024-02-29", "Y-m-d").is_some());
    }

    #[test]
    fn format_writes_the_closed_tokens_and_round_trips() {
        let at = instant();
        assert_eq!(
            format_date_with(at, "Y-m-d H:i:s").unwrap(),
            "2024-01-02 03:04:05"
        );
        assert_eq!(format_date_with(at, "d/m/Y").unwrap(), "02/01/2024");
        assert_eq!(format_date_with(at, "Y\\m\\d").unwrap(), "2024\\01\\02");

        // Unknown letters fail with the spec message; round trip holds.
        let err = format_date_with(at, "j").unwrap_err();
        assert!(err.contains("closed set: Y m d H i s"), "{err}");
        let fmt = "Y-m-d H:i:s";
        let text = format_date_with(at, fmt).unwrap();
        let back = parse_date_with(&text, fmt).expect("round trip parses");
        assert_eq!(back.to_instant(), Some(at));
    }

    #[test]
    fn timezone_conversions_shift_the_offset_not_the_instant() {
        let at = instant();

        let kolkata = in_tz(at, "Asia/Kolkata").expect("bundled zone");
        assert_eq!(kolkata.offset().whole_seconds(), 19_800);
        // Same instant, different wall clock: 03:04:05Z == 08:34:05+05:30.
        assert_eq!(kolkata, at);
        assert_eq!(
            format_date_with(kolkata, "Y-m-d H:i:s").unwrap(),
            "2024-01-02 08:34:05"
        );
        let wall = local_wall(at, "Asia/Kolkata").expect("wall clock");
        let expected = time::Date::from_calendar_date(2024, time::Month::January, 2)
            .unwrap()
            .with_hms(8, 34, 5)
            .unwrap();
        assert_eq!(wall, expected);

        // Round trip back to UTC reproduces the original.
        let utc = in_tz(kolkata, "UTC").expect("utc is bundled");
        assert_eq!(format_date_with(utc, "Y-m-d H:i:s").unwrap(), "2024-01-02 03:04:05");

        // Zones outside the table never convert.
        assert!(in_tz(at, "Mars/Olympus_Mons").is_none());
        assert!(local_wall(at, "asia/kolkata").is_none()); // case-sensitive
        assert!(now_in("Mars/Olympus_Mons").is_none());
    }

    #[test]
    fn now_in_lands_inside_the_capture_window() {
        let before = OffsetDateTime::now_utc();
        let shift = now_in("Asia/Kolkata").expect("bundled zone");
        let after = OffsetDateTime::now_utc();
        assert_eq!(shift.offset().whole_seconds(), 19_800);
        // The instant (compare at UTC) sits inside the capture window.
        let shift_utc = in_tz(shift, "UTC").expect("utc is bundled");
        assert!(shift_utc >= before && shift_utc <= after + time::Duration::seconds(1));
        assert_eq!(now_in("UTC").expect("utc").offset().whole_seconds(), 0);
    }
}
