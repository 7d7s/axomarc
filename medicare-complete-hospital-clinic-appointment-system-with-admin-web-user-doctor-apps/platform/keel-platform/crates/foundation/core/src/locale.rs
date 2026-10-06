//! Locale, timezone + deadline value types.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// IANA timezone for an execution (display + business-day semantics).
/// Validated name only; all kernel timestamps remain UTC instants (Clock).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TimeZone(String);

impl TimeZone {
    /// UTC — the safe default.
    pub fn utc() -> Self {
        Self("UTC".to_string())
    }

    /// Parse and validate an IANA-style name (e.g. `Asia/Kolkata`, `UTC`).
    pub fn parse(raw: impl Into<String>) -> Result<Self, crate::AppError> {
        let raw = raw.into();
        let ok = raw == "UTC"
            || (raw.split('/').count() >= 2
                && raw.split('/').all(|seg| {
                    !seg.is_empty()
                        && seg
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '+')
                }));
        if ok {
            Ok(Self(raw))
        } else {
            Err(crate::AppError::validation(format!(
                "invalid timezone {raw:?}: expected IANA name like Area/Location"
            )))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for TimeZone {
    fn default() -> Self {
        Self::utc()
    }
}

impl std::fmt::Display for TimeZone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Locale {
    /// BCP-47-ish tag, e.g. "en-IN".
    pub lang: String,
    /// IANA timezone, e.g. "Asia/Kolkata".
    pub timezone: String,
}

impl Default for Locale {
    fn default() -> Self {
        Self {
            lang: "en-IN".into(),
            timezone: "Asia/Kolkata".into(),
        }
    }
}

/// Execution deadline (absolute). The engine aborts work past the deadline
/// (`KER_DEADLINE`), never silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Deadline {
    #[serde(with = "time::serde::iso8601")]
    pub at: OffsetDateTime,
}

impl Deadline {
    pub fn from_timeout(now: OffsetDateTime, secs: u64) -> Self {
        Self {
            at: now + time::Duration::seconds(secs as i64),
        }
    }
    pub fn expired(&self, now: OffsetDateTime) -> bool {
        now >= self.at
    }
}
