//! Clock port: deterministic time for tests; system time in production.

use time::OffsetDateTime;

pub trait Clock: Send + Sync {
    fn now(&self) -> OffsetDateTime;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }
}

/// Test clock: fixed until you say otherwise.
#[derive(Debug, Clone)]
pub struct FixedClock {
    now: std::sync::Arc<std::sync::Mutex<OffsetDateTime>>,
}

impl FixedClock {
    pub fn new(at: OffsetDateTime) -> Self {
        Self {
            now: std::sync::Arc::new(std::sync::Mutex::new(at)),
        }
    }
    pub fn set(&self, at: OffsetDateTime) {
        *self.now.lock().unwrap() = at;
    }
    pub fn advance(&self, d: time::Duration) {
        let mut g = self.now.lock().unwrap();
        *g += d;
    }
}

impl Clock for FixedClock {
    fn now(&self) -> OffsetDateTime {
        *self.now.lock().unwrap()
    }
}
