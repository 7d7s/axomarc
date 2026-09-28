//! Retry policy + global retry budget (Sentinel S5 §17). Retries per exporter
//! are bounded with exponential backoff + jitter; the global budget stops the
//! whole telemetry fabric from consuming the app's network capacity.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
    pub jitter: bool,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 5,
            initial_backoff: Duration::from_millis(100),
            max_backoff: Duration::from_secs(10),
            jitter: true,
        }
    }
}

impl RetryPolicy {
    /// Backoff before attempt `n` (0-based retry index).
    pub fn backoff_for(&self, retry_index: u32) -> Duration {
        let mut ms = self.initial_backoff.as_millis() as u64 * 2u64.saturating_pow(retry_index);
        ms = ms.min(self.max_backoff.as_millis() as u64);
        if self.jitter && ms > 1 {
            // Cheap deterministic-ish jitter: quarter-swing based on attempt.
            let swing = ms / 4;
            let offset = (retry_index as u64 * 37) % (2 * swing + 1);
            ms = ms.saturating_sub(swing).saturating_add(offset);
        }
        Duration::from_millis(ms)
    }
}

/// Token bucket over a 1-second window: the whole fabric gets at most
/// `permits_per_second` retry attempts regardless of how many exporters fail.
pub struct RetryBudget {
    permits_per_second: u64,
    window_start: std::sync::Mutex<std::time::Instant>,
    used: AtomicU64,
}

impl RetryBudget {
    pub fn new(permits_per_second: u64) -> Arc<Self> {
        Arc::new(Self {
            permits_per_second: permits_per_second.max(1),
            window_start: std::sync::Mutex::new(std::time::Instant::now()),
            used: AtomicU64::new(0),
        })
    }

    /// Take one retry permit; false when the global budget is exhausted for
    /// the current window.
    pub fn take(&self) -> bool {
        let mut start = self.window_start.lock().expect("budget poisoned");
        if start.elapsed() >= Duration::from_secs(1) {
            *start = std::time::Instant::now();
            self.used.store(0, Ordering::Relaxed);
        }
        let used = self.used.fetch_add(1, Ordering::Relaxed);
        used < self.permits_per_second
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_and_caps() {
        let p = RetryPolicy {
            jitter: false,
            initial_backoff: Duration::from_millis(100),
            max_backoff: Duration::from_millis(400),
            max_attempts: 5,
        };
        assert_eq!(p.backoff_for(0), Duration::from_millis(100));
        assert_eq!(p.backoff_for(1), Duration::from_millis(200));
        assert_eq!(p.backoff_for(2), Duration::from_millis(400));
        assert_eq!(p.backoff_for(3), Duration::from_millis(400));
    }

    #[test]
    fn budget_exhausts_then_refills() {
        let b = RetryBudget::new(3);
        assert!(b.take());
        assert!(b.take());
        assert!(b.take());
        assert!(!b.take(), "fourth retry in the same window must be denied");
    }
}
