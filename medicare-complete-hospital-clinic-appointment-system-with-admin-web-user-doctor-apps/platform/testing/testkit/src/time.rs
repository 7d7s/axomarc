//! Deterministic time helpers around the platform FixedClock.

use std::sync::Arc;

use keel_foundation::FixedClock;
use time::{Duration, OffsetDateTime};

/// Freeze time at the given instant; hand the shared clock to the code
/// under test.
pub fn freeze_time(at: OffsetDateTime) -> Arc<FixedClock> {
    Arc::new(FixedClock::new(at))
}

/// Move the frozen clock to an exact instant.
pub fn travel_to(clock: &FixedClock, at: OffsetDateTime) {
    clock.set(at);
}

/// Advance the frozen clock by a duration.
pub fn advance(clock: &FixedClock, by: Duration) {
    clock.advance(by);
}

/// Wave 7 P3 (docs/19 L340): `travel(seconds)` — advance the frozen clock
/// by whole seconds (the unit session TTLs / schedule windows are quoted in).
pub fn travel(clock: &FixedClock, seconds: i64) {
    clock.advance(Duration::seconds(seconds));
}

#[cfg(test)]
mod tests {
    use super::*;
    use keel_foundation::Clock;

    #[test]
    fn freeze_travel_advance() {
        let t0 = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        let clock = freeze_time(t0);
        assert_eq!(clock.now(), t0);

        travel_to(&clock, t0 + Duration::hours(2));
        assert_eq!(clock.now(), t0 + Duration::hours(2));

        advance(&clock, Duration::minutes(15));
        assert_eq!(clock.now(), t0 + Duration::seconds(7200 + 900));

        travel(&clock, 60);
        assert_eq!(clock.now(), t0 + Duration::seconds(7200 + 900 + 60));
    }
}
