//! # keel-scheduling
//! Recurring execution contracts (11-SPEC §29): schedules declare WHEN; the jobs
//! layer decides HOW (durable job enqueue). Next-run math is deterministic and
//! UTC-only — no wall-clock ambiguity in the kernel.

use keel_foundation::AppError;
use serde::{Deserialize, Serialize};
use time::{Date, Duration, OffsetDateTime, Time, Weekday};

/// Declarative schedule. All times UTC.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScheduleSpec {
    /// Every N seconds (min 10 to avoid hot loops).
    Interval { secs: u64 },
    /// Daily at HH:MM.
    Daily { hour: u8, minute: u8 },
    /// Weekly on a day at HH:MM.
    Weekly { day: Weekday, hour: u8, minute: u8 },
}

impl ScheduleSpec {
    pub fn validate(&self) -> Result<(), AppError> {
        match self {
            ScheduleSpec::Interval { secs } if *secs < 10 => {
                Err(AppError::validation("interval schedules must be >= 10s"))
            }
            ScheduleSpec::Daily { hour, minute } | ScheduleSpec::Weekly { hour, minute, .. }
                if *hour > 23 || *minute > 59 =>
            {
                Err(AppError::validation("schedule time out of range"))
            }
            _ => Ok(()),
        }
    }

    /// Deterministic next occurrence strictly after `from` (UTC).
    pub fn next_run(&self, from: OffsetDateTime) -> Result<OffsetDateTime, AppError> {
        self.validate()?;
        let utc = from.to_offset(time::UtcOffset::UTC);
        match self {
            ScheduleSpec::Interval { secs } => Ok(utc + Duration::seconds(*secs as i64)),
            ScheduleSpec::Daily { hour, minute } => {
                let at = Time::from_hms(*hour, *minute, 0)
                    .map_err(|_| AppError::validation("bad time"))?;
                let today = Date::from_calendar_date(utc.year(), utc.month(), utc.day())
                    .unwrap()
                    .with_time(at)
                    .assume_utc();
                Ok(if today > utc {
                    today
                } else {
                    today + Duration::days(1)
                })
            }
            ScheduleSpec::Weekly { day, hour, minute } => {
                let at = Time::from_hms(*hour, *minute, 0)
                    .map_err(|_| AppError::validation("bad time"))?;
                let mut cand = utc.date().with_time(at).assume_utc();
                for _ in 0..8 {
                    if cand > utc && cand.weekday() == *day {
                        return Ok(cand);
                    }
                    cand += Duration::days(1);
                }
                Err(AppError::internal("weekly schedule: unreachable"))
            }
        }
    }
}
/// One registered recurring schedule. Firing becomes a durable job enqueue —
/// never an inline mutation (same engine, same context as any front door).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleItem {
    pub name: String,
    /// Target operation id (module-scoped), e.g. `billing.close-day`.
    pub operation: String,
    pub spec: ScheduleSpec,
    #[serde(default)]
    pub payload: serde_json::Value,
    #[serde(default)]
    pub enabled: bool,
}

/// In-memory registry with deterministic due-scan (persistence = postgres adapter,
/// later). `due` is a pure function of (items, last-fired, now) — testable, replayable.
#[derive(Debug, Default)]
pub struct ScheduleRegistry {
    items: Vec<ScheduleItem>,
}

impl ScheduleRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, item: ScheduleItem) -> Result<(), AppError> {
        item.spec.validate()?;
        if self.items.iter().any(|i| i.name == item.name) {
            return Err(AppError::validation(format!(
                "schedule registered twice: {}",
                item.name
            )));
        }
        self.items.push(item);
        Ok(())
    }

    pub fn names(&self) -> Vec<&str> {
        self.items.iter().map(|i| i.name.as_str()).collect()
    }

    /// Items whose next run (strictly after `last_fired`) is <= `now`.
    pub fn due<'a>(
        &'a self,
        last_fired: &[(String, OffsetDateTime)],
        now: OffsetDateTime,
    ) -> Result<Vec<&'a ScheduleItem>, AppError> {
        let mut out = Vec::new();
        for item in &self.items {
            if !item.enabled {
                continue;
            }
            let base = last_fired
                .iter()
                .find(|(n, _)| n == &item.name)
                .map(|(_, t)| *t)
                .unwrap_or(now - Duration::days(365 * 10));
            if item.spec.next_run(base)? <= now {
                out.push(item);
            }
        }
        Ok(out)
    }
}

use std::sync::{Arc, Mutex};

/// Scheduler runner (L35, S3): turns the pure `due` scan into worker ticks.
/// The enqueue closure keeps this crate free of a jobs dependency — the
/// worker wires it to `JobRegistry::build` + `JobStore::enqueue`, so a
/// firing schedule is a durable execution like any other front door.
pub struct SchedulerRunner {
    registry: Arc<ScheduleRegistry>,
    /// (schedule name, last fired instant); persisted optionally via JSON.
    last_fired: Mutex<Vec<(String, OffsetDateTime)>>,
}

impl SchedulerRunner {
    pub fn new(registry: Arc<ScheduleRegistry>) -> Self {
        Self {
            registry,
            last_fired: Mutex::new(Vec::new()),
        }
    }

    /// Seed last-fired state (e.g. loaded from disk at boot).
    pub fn with_state(
        registry: Arc<ScheduleRegistry>,
        state: Vec<(String, OffsetDateTime)>,
    ) -> Self {
        Self {
            registry,
            last_fired: Mutex::new(state),
        }
    }

    pub fn registry(&self) -> &Arc<ScheduleRegistry> {
        &self.registry
    }

    /// Current last-fired snapshot (for persistence).
    pub fn state(&self) -> Vec<(String, OffsetDateTime)> {
        self.last_fired.lock().unwrap().clone()
    }

    /// One tick: fire every due schedule through `enqueue`, record
    /// last-fired. Returns the names fired, in registry order.
    pub fn tick(
        &self,
        now: OffsetDateTime,
        enqueue: impl FnMut(&ScheduleItem) -> Result<(), AppError>,
    ) -> Result<Vec<String>, AppError> {
        let state = self.last_fired.lock().unwrap().clone();
        let due = self.registry.due(&state, now)?;
        let mut fired = Vec::new();
        let mut enqueue = enqueue;
        let mut last_fired = self.last_fired.lock().unwrap();
        for item in due {
            enqueue(item)?;
            fired.push(item.name.clone());
            if let Some(entry) = last_fired.iter_mut().find(|(n, _)| n == &item.name) {
                entry.1 = now;
            } else {
                last_fired.push((item.name.clone(), now));
            }
        }
        Ok(fired)
    }
}

/// Persisted tick state (storage/framework/schedule-state.json) so a CLI
/// `schedule:run` or a restarting worker does not re-fire immediately.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FiredEntry {
    pub name: String,
    #[serde(with = "time::serde::iso8601")]
    pub at: OffsetDateTime,
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct SchedulerStateDoc {
    pub last_fired: Vec<FiredEntry>,
}

pub fn state_path(root: &std::path::Path) -> std::path::PathBuf {
    root.join("storage/framework/schedule-state.json")
}

pub fn load_state(root: &std::path::Path) -> Result<Vec<(String, OffsetDateTime)>, AppError> {
    match std::fs::read_to_string(state_path(root)) {
        Ok(raw) => {
            let doc: SchedulerStateDoc = serde_json::from_str(&raw)
                .map_err(|e| AppError::internal(format!("schedule state corrupt: {e}")))?;
            Ok(doc
                .last_fired
                .into_iter()
                .map(|entry| (entry.name, entry.at))
                .collect())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(AppError::internal(format!(
            "schedule state unreadable: {e}"
        ))),
    }
}

pub fn save_state(
    root: &std::path::Path,
    last_fired: &[(String, OffsetDateTime)],
) -> Result<(), AppError> {
    let path = state_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| AppError::internal(format!("schedule state dir: {e}")))?;
    }
    let doc = SchedulerStateDoc {
        last_fired: last_fired
            .iter()
            .cloned()
            .map(|(name, at)| FiredEntry { name, at })
            .collect(),
    };
    let raw = serde_json::to_string_pretty(&doc).map_err(|e| AppError::internal(e.to_string()))?;
    std::fs::write(&path, raw).map_err(|e| AppError::internal(format!("schedule state write: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn interval_next_run() {
        let spec = ScheduleSpec::Interval { secs: 60 };
        let from = datetime!(2026-08-19 10:00:00 UTC);
        assert_eq!(
            spec.next_run(from).unwrap(),
            datetime!(2026-08-19 10:01:00 UTC)
        );
    }

    #[test]
    fn daily_rolls_to_next_day() {
        let spec = ScheduleSpec::Daily { hour: 9, minute: 0 };
        let from = datetime!(2026-08-19 10:00:00 UTC);
        assert_eq!(
            spec.next_run(from).unwrap(),
            datetime!(2026-08-20 09:00:00 UTC)
        );
        let early = datetime!(2026-08-19 08:00:00 UTC);
        assert_eq!(
            spec.next_run(early).unwrap(),
            datetime!(2026-08-19 09:00:00 UTC)
        );
    }

    #[test]
    fn weekly_finds_matching_weekday() {
        // 2026-08-19 is a Wednesday.
        let spec = ScheduleSpec::Weekly {
            day: Weekday::Monday,
            hour: 6,
            minute: 30,
        };
        let from = datetime!(2026-08-19 10:00:00 UTC);
        let next = spec.next_run(from).unwrap();
        assert_eq!(next.weekday(), Weekday::Monday);
        assert_eq!(next, datetime!(2026-08-24 06:30:00 UTC));
    }

    #[test]
    fn interval_floor_enforced() {
        assert!(ScheduleSpec::Interval { secs: 5 }.validate().is_err());
    }

    #[test]
    fn due_scan_fires_never_fired_items() {
        let mut reg = ScheduleRegistry::new();
        reg.register(ScheduleItem {
            name: "close-day".into(),
            operation: "billing.close-day".into(),
            spec: ScheduleSpec::Daily {
                hour: 23,
                minute: 0,
            },
            payload: serde_json::json!({}),
            enabled: true,
        })
        .unwrap();
        let due = reg.due(&[], datetime!(2026-08-19 10:00:00 UTC)).unwrap();
        assert_eq!(due.len(), 1);
        let just_fired = vec![("close-day".to_string(), datetime!(2026-08-19 23:00:00 UTC))];
        let due2 = reg
            .due(&just_fired, datetime!(2026-08-19 23:30:00 UTC))
            .unwrap();
        assert!(due2.is_empty());
    }

    #[test]
    fn runner_fires_due_items_and_records_state() {
        let mut reg = ScheduleRegistry::new();
        reg.register(ScheduleItem {
            name: "close-day".into(),
            operation: "billing.close-day".into(),
            spec: ScheduleSpec::Interval { secs: 60 },
            payload: serde_json::json!({}),
            enabled: true,
        })
        .unwrap();
        let runner = SchedulerRunner::new(Arc::new(reg));
        let now = datetime!(2026-08-19 10:00:00 UTC);
        let fired_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let calls = fired_calls.clone();
        let fired = runner
            .tick(now, |item| {
                calls.lock().unwrap().push(item.operation.clone());
                Ok(())
            })
            .unwrap();
        assert_eq!(fired, vec!["close-day".to_string()]);
        assert_eq!(
            fired_calls.lock().unwrap().as_slice(),
            &["billing.close-day".to_string()]
        );
        assert_eq!(runner.state().len(), 1);

        // 30s later: interval not due again yet.
        let fired = runner
            .tick(now + Duration::seconds(30), |_| Ok(()))
            .unwrap();
        assert!(fired.is_empty());
        // 61s later: due again.
        let fired = runner
            .tick(now + Duration::seconds(91), |_| Ok(()))
            .unwrap();
        assert_eq!(fired.len(), 1);
    }

    #[test]
    fn state_roundtrips_through_disk() {
        let root = std::env::temp_dir().join(format!(
            "keel-sched-test-{}",
            time::OffsetDateTime::now_utc().unix_timestamp()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let state = vec![("close-day".to_string(), datetime!(2026-08-19 23:00:00 UTC))];
        save_state(&root, &state).unwrap();
        let loaded = load_state(&root).unwrap();
        assert_eq!(loaded, state);
        std::fs::remove_dir_all(&root).ok();
    }
}
