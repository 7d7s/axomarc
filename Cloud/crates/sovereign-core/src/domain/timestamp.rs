// Unix-seconds timestamp. SQLite stores `INTEGER NOT NULL`; chrono is a
// wire/edge concern (only the binary crate should convert). Keeping this
// type pure makes the domain layer cheap to test and impossible to misuse.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

/// A point in time, stored as the number of seconds since the Unix epoch
/// (1970-01-01T00:00:00Z). This matches the schema in
/// `migrations/0001_init.sql` (`INTEGER NOT NULL`).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, sqlx::Type,
)]
#[serde(transparent)]
#[sqlx(transparent)]
pub struct Timestamp(pub i64);

impl Timestamp {
    /// The Unix epoch (1970-01-01T00:00:00Z).
    pub const EPOCH: Self = Self(0);

    /// The current wall-clock time.
    #[inline]
    pub fn now() -> Self {
        Self::from(SystemTime::now())
    }

    /// The current wall-clock time plus `secs` seconds. Used in tests.
    #[inline]
    pub fn now_plus(secs: i64) -> Self {
        Self::from(SystemTime::now()) + Self(secs)
    }

    /// Returns the raw unix-seconds value.
    #[inline]
    pub const fn as_secs(&self) -> i64 {
        self.0
    }
}

impl From<i64> for Timestamp {
    #[inline]
    fn from(v: i64) -> Self {
        Self(v)
    }
}

impl From<Timestamp> for i64 {
    #[inline]
    fn from(t: Timestamp) -> i64 {
        t.0
    }
}

impl From<SystemTime> for Timestamp {
    #[inline]
    fn from(t: SystemTime) -> Self {
        let secs = t
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        Self(secs)
    }
}

impl From<Timestamp> for SystemTime {
    #[inline]
    fn from(t: Timestamp) -> Self {
        UNIX_EPOCH + std::time::Duration::from_secs(t.0.max(0) as u64)
    }
}

impl std::ops::Add for Timestamp {
    type Output = Self;
    #[inline]
    fn add(self, rhs: Self) -> Self {
        Self(self.0 + rhs.0)
    }
}

impl std::ops::Sub for Timestamp {
    type Output = Self;
    #[inline]
    fn sub(self, rhs: Self) -> Self {
        Self(self.0 - rhs.0)
    }
}

impl fmt::Display for Timestamp {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "@{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_is_zero() {
        assert_eq!(Timestamp::EPOCH.as_secs(), 0);
    }

    #[test]
    fn add_and_sub() {
        let t = Timestamp(100);
        assert_eq!((t + Timestamp(50)).as_secs(), 150);
        assert_eq!((t - Timestamp(30)).as_secs(), 70);
    }

    #[test]
    fn system_time_roundtrip() {
        let now = Timestamp::now();
        let back: SystemTime = now.into();
        let again: Timestamp = back.into();
        assert_eq!(now, again);
    }
}
