//! Managed exporter: wraps an `Exporter` with its circuit breaker, bounded
//! retries, and the global retry budget; records health/throughput stats and
//! renders health snapshots (written by the runtime under
//! `storage/framework/telemetry/exporters/` for `keel observe exporters`).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use time::OffsetDateTime;

use keel_telemetry::{Event, ExportError, ExportOk, Exporter};
use serde::{Deserialize, Serialize};

use crate::breaker::{BreakerConfig, BreakerState, CircuitBreaker};
use crate::retry::{RetryBudget, RetryPolicy};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExporterHealth {
    pub name: String,
    pub state: String,
    pub attempts: u64,
    pub failures: u64,
    pub successes: u64,
    pub exported_events: u64,
    pub exported_bytes: u64,
    pub last_error: Option<String>,
    #[serde(with = "time::serde::iso8601::option")]
    pub last_success_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::iso8601")]
    pub updated_at: OffsetDateTime,
}

pub struct ManagedExporter {
    name: String,
    inner: Arc<dyn Exporter>,
    breaker: CircuitBreaker,
    retry: RetryPolicy,
    budget: Arc<RetryBudget>,
    attempts: AtomicU64,
    failures: AtomicU64,
    successes: AtomicU64,
    exported_events: AtomicU64,
    exported_bytes: AtomicU64,
    last_error: std::sync::Mutex<Option<String>>,
    last_success_at: std::sync::Mutex<Option<OffsetDateTime>>,
    critical: bool,
}

impl ManagedExporter {
    pub fn new(
        name: impl Into<String>,
        inner: Arc<dyn Exporter>,
        breaker_cfg: BreakerConfig,
        retry: RetryPolicy,
        budget: Arc<RetryBudget>,
        critical: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            name: name.into(),
            inner,
            breaker: CircuitBreaker::new(breaker_cfg),
            retry,
            budget,
            attempts: AtomicU64::new(0),
            failures: AtomicU64::new(0),
            successes: AtomicU64::new(0),
            exported_events: AtomicU64::new(0),
            exported_bytes: AtomicU64::new(0),
            last_error: std::sync::Mutex::new(None),
            last_success_at: std::sync::Mutex::new(None),
            critical,
        })
    }

    /// Critical exporters survive resource shedding; non-critical ones are
    /// the first to be paused under severe pressure (S5 §14).
    pub fn is_critical(&self) -> bool {
        self.critical
    }

    pub fn breaker_state(&self) -> BreakerState {
        self.breaker.state()
    }

    /// Export with bounded retries. Never panics; failures surface as
    /// `ExportError` for the runtime's WAL/fallback handling.
    pub fn export(&self, batch: &[Event]) -> Result<ExportOk, ExportError> {
        if !self.breaker.allow() {
            return Err(ExportError::Unavailable(format!(
                "breaker open for {}",
                self.name
            )));
        }
        let mut last_err: Option<ExportError> = None;
        for attempt in 0..self.retry.max_attempts.max(1) {
            if attempt > 0 && !self.budget.take() {
                break; // global retry budget exhausted; stop the storm
            }
            self.attempts.fetch_add(1, Ordering::Relaxed);
            match self.inner.export(batch) {
                Ok(ok) => {
                    self.breaker.success();
                    self.successes.fetch_add(1, Ordering::Relaxed);
                    self.exported_events
                        .fetch_add(ok.events as u64, Ordering::Relaxed);
                    self.exported_bytes
                        .fetch_add(ok.bytes as u64, Ordering::Relaxed);
                    *self.last_success_at.lock().expect("lock") = Some(OffsetDateTime::now_utc());
                    *self.last_error.lock().expect("lock") = None;
                    return Ok(ok);
                }
                Err(ExportError::Rejected(m)) => {
                    // Definitive rejection: do not retry, do not trip breaker.
                    self.breaker.success();
                    *self.last_error.lock().expect("lock") = Some(m.clone());
                    return Err(ExportError::Rejected(m));
                }
                Err(err) => {
                    self.breaker.failure();
                    self.failures.fetch_add(1, Ordering::Relaxed);
                    *self.last_error.lock().expect("lock") = Some(err.to_string());
                    last_err = Some(err);
                    if attempt + 1 < self.retry.max_attempts {
                        std::thread::sleep(self.retry.backoff_for(attempt));
                        if !self.breaker.allow() {
                            break;
                        }
                    }
                }
            }
        }
        Err(last_err.unwrap_or_else(|| ExportError::Unavailable(self.name.clone())))
    }

    pub fn health(&self) -> ExporterHealth {
        ExporterHealth {
            name: self.name.clone(),
            state: format!("{:?}", self.breaker.state()),
            attempts: self.attempts.load(Ordering::Relaxed),
            failures: self.failures.load(Ordering::Relaxed),
            successes: self.successes.load(Ordering::Relaxed),
            exported_events: self.exported_events.load(Ordering::Relaxed),
            exported_bytes: self.exported_bytes.load(Ordering::Relaxed),
            last_error: self.last_error.lock().expect("lock").clone(),
            last_success_at: *self.last_success_at.lock().expect("lock"),
            updated_at: OffsetDateTime::now_utc(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use keel_foundation::TelemetryContext;
    use keel_telemetry::{
        EventEnvelope, Fields, Level, LogPayload, ServiceIdentity, TelemetryPayload, SCHEMA_LOG,
        SCHEMA_LOG_VERSION,
    };
    use std::sync::atomic::AtomicU32;
    use std::time::Duration;

    struct Flaky {
        fails: AtomicU32,
        export_calls: AtomicU32,
    }

    impl Exporter for Flaky {
        fn name(&self) -> &str {
            "flaky"
        }
        fn export(&self, _batch: &[Event]) -> Result<ExportOk, ExportError> {
            self.export_calls.fetch_add(1, Ordering::SeqCst);
            if self.fails.fetch_sub(1, Ordering::SeqCst) > 0 {
                Err(ExportError::Transport("flake".into()))
            } else {
                Ok(ExportOk {
                    bytes: 10,
                    events: 1,
                })
            }
        }
    }

    fn event() -> Event {
        EventEnvelope::new(
            SCHEMA_LOG,
            SCHEMA_LOG_VERSION,
            ServiceIdentity::new("t", "dev"),
            TelemetryContext::root(),
            TelemetryPayload::Log(LogPayload {
                level: Level::Info,
                message: "m".into(),
                target: None,
                fields: Fields::new(),
                extra: Fields::new(),
            }),
        )
    }

    #[test]
    fn retries_until_success_within_policy() {
        let flaky = Arc::new(Flaky {
            fails: AtomicU32::new(2),
            export_calls: AtomicU32::new(0),
        });
        let managed = ManagedExporter::new(
            "flaky",
            flaky.clone(),
            BreakerConfig {
                failure_threshold: 5,
                cooldown: Duration::from_secs(5),
            },
            RetryPolicy {
                max_attempts: 5,
                initial_backoff: Duration::from_millis(1),
                max_backoff: Duration::from_millis(2),
                jitter: false,
            },
            RetryBudget::new(100),
            true,
        );
        let ok = managed.export(&[event()]).unwrap();
        assert_eq!(ok.events, 1);
        assert_eq!(flaky.export_calls.load(Ordering::SeqCst), 3);
        assert_eq!(managed.health().failures, 2);
    }

    #[test]
    fn budget_limits_retry_storm() {
        let flaky = Arc::new(Flaky {
            fails: AtomicU32::new(u32::MAX),
            export_calls: AtomicU32::new(0),
        });
        let budget = RetryBudget::new(2);
        let managed = ManagedExporter::new(
            "flaky",
            flaky.clone(),
            BreakerConfig {
                failure_threshold: 100,
                cooldown: Duration::from_secs(1),
            },
            RetryPolicy {
                max_attempts: 10,
                initial_backoff: Duration::from_millis(1),
                max_backoff: Duration::from_millis(2),
                jitter: false,
            },
            budget,
            true,
        );
        managed.export(&[event()]).unwrap_err();
        // attempt 1 + at most 2 budgeted retries
        assert!(flaky.export_calls.load(Ordering::SeqCst) <= 3);
    }
}
