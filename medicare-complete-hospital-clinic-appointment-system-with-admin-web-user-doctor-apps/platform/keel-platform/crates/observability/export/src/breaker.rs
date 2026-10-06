//! Exporter circuit breaker (Sentinel S5 §15/§16): CLOSED → OPEN on failure
//! threshold, cooldown, HALF_OPEN probe, back to CLOSED on success. An
//! unavailable telemetry backend must never cause a retry storm.

use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakerState {
    Closed,
    Open,
    HalfOpen,
}

#[derive(Debug, Clone)]
pub struct BreakerConfig {
    pub failure_threshold: u32,
    pub cooldown: Duration,
}

impl Default for BreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 3,
            cooldown: Duration::from_secs(15),
        }
    }
}

#[derive(Debug)]
struct Inner {
    state: BreakerState,
    consecutive_failures: u32,
    opened_at: Option<Instant>,
}

pub struct CircuitBreaker {
    cfg: BreakerConfig,
    inner: Mutex<Inner>,
    now: fn() -> Instant, // test seam
}

impl CircuitBreaker {
    pub fn new(cfg: BreakerConfig) -> Self {
        Self::with_clock(cfg, Instant::now)
    }

    fn with_clock(cfg: BreakerConfig, now: fn() -> Instant) -> Self {
        Self {
            cfg,
            inner: Mutex::new(Inner {
                state: BreakerState::Closed,
                consecutive_failures: 0,
                opened_at: None,
            }),
            now,
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().expect("breaker poisoned")
    }

    /// Current state, promoting Open → HalfOpen once cooldown has elapsed.
    pub fn state(&self) -> BreakerState {
        let mut inner = self.lock();
        if inner.state == BreakerState::Open {
            let elapsed = inner
                .opened_at
                .map(|t| (self.now)().duration_since(t))
                .unwrap_or(Duration::MAX);
            if elapsed >= self.cfg.cooldown {
                inner.state = BreakerState::HalfOpen;
            }
        }
        inner.state
    }

    /// Should an export attempt be allowed right now?
    pub fn allow(&self) -> bool {
        !matches!(self.state(), BreakerState::Open)
    }

    pub fn success(&self) {
        let mut inner = self.lock();
        inner.consecutive_failures = 0;
        inner.state = BreakerState::Closed;
        inner.opened_at = None;
    }

    pub fn failure(&self) {
        let mut inner = self.lock();
        inner.consecutive_failures += 1;
        let trip = inner.state == BreakerState::HalfOpen
            || inner.consecutive_failures >= self.cfg.failure_threshold;
        if trip {
            inner.state = BreakerState::Open;
            inner.opened_at = Some((self.now)());
        }
    }

    /// Test helper: build a breaker with a fake clock source.
    #[cfg(test)]
    pub fn for_test(cfg: BreakerConfig, now: fn() -> Instant) -> Self {
        Self::with_clock(cfg, now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_until_threshold_then_open() {
        let b = CircuitBreaker::new(BreakerConfig {
            failure_threshold: 2,
            cooldown: Duration::from_secs(1000),
        });
        assert_eq!(b.state(), BreakerState::Closed);
        b.failure();
        assert_eq!(b.state(), BreakerState::Closed);
        b.failure();
        assert_eq!(b.state(), BreakerState::Open);
        assert!(!b.allow());
    }

    #[test]
    fn half_open_after_cooldown_and_resets_on_success() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NOW_MS: AtomicU64 = AtomicU64::new(0);
        let b = CircuitBreaker::for_test(
            BreakerConfig {
                failure_threshold: 1,
                cooldown: Duration::from_millis(100),
            },
            || Instant::now() + Duration::from_millis(NOW_MS.load(Ordering::SeqCst)),
        );
        b.failure();
        assert_eq!(b.state(), BreakerState::Open);
        NOW_MS.store(150, Ordering::SeqCst);
        assert_eq!(b.state(), BreakerState::HalfOpen);
        assert!(b.allow());
        b.success();
        assert_eq!(b.state(), BreakerState::Closed);
    }
}
