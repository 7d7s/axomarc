//! Test doubles shared by the §7.1 `rollback` and §7.2 `promote` suites.
//!
//! Compiled only under `cfg(test)` (see the declaration in `lib.rs`).
//!
//! Every double funnels its calls into one shared [`Trace`], so a test can
//! assert not just *that* a dimension ran but the exact **order**. That is
//! what both key safety properties rest on:
//!
//! * §7.1 — the read-only job/event guards run before any mutation;
//! * §7.2 — promotion stages run in order, and a failed gate restores the
//!   checkpoint after (never before) the failure.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use kelson_foundation::AppError;

use crate::ports::{BoxFuture, ConfigPort, EventPort, HealthCheck, JobPort, MigrationPort, ModulePort};
use crate::rollback::{PromotionTarget, RollbackExecutor};

/// Ordered log of everything the doubles were asked to do.
#[derive(Clone, Default)]
pub(crate) struct Trace(Arc<Mutex<Vec<String>>>);

impl Trace {
    pub(crate) fn push(&self, what: &str) {
        self.0.lock().unwrap().push(what.to_string());
    }

    pub(crate) fn calls(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }

    /// Calls under one of `prefixes`, in order — for asserting "nothing in
    /// this family ran" without spelling out every other call.
    pub(crate) fn under(&self, prefix: &str) -> Vec<String> {
        self.calls()
            .into_iter()
            .filter(|c| c.starts_with(prefix))
            .collect()
    }
}

// --- Dimension 1: config -------------------------------------------------

pub(crate) struct FakeConfig {
    pub(crate) trace: Trace,
    pub(crate) live: Mutex<u64>,
}

impl ConfigPort for FakeConfig {
    fn current_version(&self) -> BoxFuture<'_, Result<u64, AppError>> {
        let v = *self.live.lock().unwrap();
        self.trace.push("config.current");
        Box::pin(async move { Ok(v) })
    }

    fn activate(&self, version: u64) -> BoxFuture<'_, Result<u64, AppError>> {
        self.trace.push(&format!("config.activate({version})"));
        *self.live.lock().unwrap() = version;
        Box::pin(async move { Ok(version) })
    }

    fn reload(&self) -> BoxFuture<'_, Result<u64, AppError>> {
        self.trace.push("config.reload");
        let next = {
            let mut live = self.live.lock().unwrap();
            *live += 1;
            *live
        };
        Box::pin(async move { Ok(next) })
    }
}

// --- Dimension 2: migrations ---------------------------------------------

pub(crate) struct FakeMigrations {
    pub(crate) trace: Trace,
    pub(crate) watermark: Mutex<u32>,
    /// Newest pending pair that `apply_pending` can actually reach — lets a
    /// test simulate a release whose migrations did not land in full.
    pub(crate) reachable: Mutex<u32>,
}

impl MigrationPort for FakeMigrations {
    fn watermark(&self) -> BoxFuture<'_, Result<u32, AppError>> {
        let w = *self.watermark.lock().unwrap();
        self.trace.push("db.watermark");
        Box::pin(async move { Ok(w) })
    }

    fn apply_pending(&self) -> BoxFuture<'_, Result<Vec<String>, AppError>> {
        self.trace.push("db.apply_pending");
        let reachable = *self.reachable.lock().unwrap();
        let current = *self.watermark.lock().unwrap();
        *self.watermark.lock().unwrap() = reachable;
        Box::pin(async move {
            Ok((current + 1..=reachable).map(|b| format!("m{b}")).collect())
        })
    }

    fn rollback_to(&self, target: u32) -> BoxFuture<'_, Result<Vec<String>, AppError>> {
        self.trace.push(&format!("db.rollback_to({target})"));
        let current = *self.watermark.lock().unwrap();
        *self.watermark.lock().unwrap() = target.min(current);
        Box::pin(async move {
            if target >= current {
                return Ok(Vec::new());
            }
            Ok((target + 1..=current).rev().map(|b| format!("m{b}")).collect())
        })
    }
}

// --- Dimension 3: module pins --------------------------------------------

pub(crate) struct FakeModules {
    pub(crate) trace: Trace,
    pub(crate) pins: Mutex<BTreeMap<String, String>>,
}

impl ModulePort for FakeModules {
    fn current_pins(&self) -> BoxFuture<'_, Result<BTreeMap<String, String>, AppError>> {
        let p = self.pins.lock().unwrap().clone();
        self.trace.push("modules.current_pins");
        Box::pin(async move { Ok(p) })
    }

    fn pin_versions<'a>(
        &'a self,
        pins: &'a BTreeMap<String, String>,
    ) -> BoxFuture<'a, Result<usize, AppError>> {
        let before = self.pins.lock().unwrap().clone();
        // `filter` hands the closure `&(&String, &String)`, so the bindings
        // are double references — go through `as_str`/deref rather than
        // asking `Borrow<&String>`.
        let changed = pins
            .iter()
            .filter(|(k, v)| before.get(k.as_str()) != Some(*v))
            .count();
        *self.pins.lock().unwrap() = pins.clone();
        self.trace.push(&format!("modules.pin_versions({})", pins.len()));
        Box::pin(async move { Ok(changed) })
    }
}

// --- Dimension 4: job schema compatibility --------------------------------

pub(crate) struct FakeJobs {
    pub(crate) trace: Trace,
    pub(crate) supported: BTreeMap<String, u32>,
    /// Job schema floor the running code enforces. A target below it would
    /// strand in-flight rows stamped at `floor`, so compatibility refuses.
    pub(crate) refuse_below: Option<u32>,
}

impl JobPort for FakeJobs {
    fn supported_versions(&self) -> BoxFuture<'_, Result<BTreeMap<String, u32>, AppError>> {
        let s = self.supported.clone();
        self.trace.push("jobs.supported_versions");
        Box::pin(async move { Ok(s) })
    }

    fn ensure_compatibility<'a>(
        &'a self,
        target: &'a BTreeMap<String, u32>,
    ) -> BoxFuture<'a, Result<(), AppError>> {
        self.trace.push("jobs.ensure_compatibility");
        let floor = self.refuse_below;
        Box::pin(async move {
            if let Some(floor) = floor {
                if target.values().any(|v| *v < floor) {
                    return Err(AppError::validation(
                        "in-flight jobs are stamped newer than the target schema",
                    ));
                }
            }
            Ok(())
        })
    }
}

// --- Dimension 5: event idempotency stability -----------------------------

pub(crate) struct FakeEvents {
    pub(crate) trace: Trace,
    pub(crate) stable: bool,
}

impl EventPort for FakeEvents {
    fn verify_idempotency_stability(&self) -> BoxFuture<'_, Result<(), AppError>> {
        self.trace.push("events.verify_idempotency_stability");
        let stable = self.stable;
        Box::pin(async move {
            if stable {
                Ok(())
            } else {
                Err(AppError::validation(
                    "idempotency key formula changed under existing rows",
                ))
            }
        })
    }
}

// --- Promotion health probes ----------------------------------------------

/// What a probe answers. `Error` models a probe that cannot run at all —
/// which the pipeline must treat as unhealthy, never as "unknown, proceed".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    Healthy,
    Unhealthy,
    Error,
}

pub(crate) struct FakeHealth {
    pub(crate) name: &'static str,
    pub(crate) outcome: Outcome,
    pub(crate) trace: Trace,
}

impl HealthCheck for FakeHealth {
    fn name(&self) -> &str {
        self.name
    }

    fn run(&self) -> BoxFuture<'_, Result<bool, AppError>> {
        self.trace.push(&format!("health.{}", self.name));
        let outcome = self.outcome;
        Box::pin(async move {
            match outcome {
                Outcome::Healthy => Ok(true),
                Outcome::Unhealthy => Ok(false),
                Outcome::Error => Err(AppError::internal("probe unreachable")),
            }
        })
    }
}

// --- Harness --------------------------------------------------------------

/// All five doubles in a fixed, known starting state:
///
/// | dimension | starting value |
/// |---|---|
/// | config revision | `7` |
/// | migration watermark | `5` (reachable up to `6`) |
/// | module pins | `echo -> v1` |
/// | job schemas | `mail.send -> 2` |
/// | idempotency keys | stable |
pub(crate) struct Harness {
    pub(crate) trace: Trace,
    pub(crate) config: Arc<FakeConfig>,
    pub(crate) migrations: Arc<FakeMigrations>,
    pub(crate) modules: Arc<FakeModules>,
    pub(crate) jobs: Arc<FakeJobs>,
    pub(crate) events: Arc<FakeEvents>,
}

impl Harness {
    pub(crate) fn new() -> Self {
        Self::build(None, true)
    }

    /// `refuse_below` — job schema floor the running code enforces.
    /// `stable` — whether idempotency keys verify as stable.
    ///
    /// Both are constructor parameters rather than post-construction
    /// mutation because the doubles sit behind `Arc`.
    pub(crate) fn build(refuse_below: Option<u32>, stable: bool) -> Self {
        let trace = Trace::default();
        let mut pins = BTreeMap::new();
        pins.insert("echo".into(), "v1".into());
        Self {
            config: Arc::new(FakeConfig {
                trace: trace.clone(),
                live: Mutex::new(7),
            }),
            migrations: Arc::new(FakeMigrations {
                trace: trace.clone(),
                watermark: Mutex::new(5),
                reachable: Mutex::new(6),
            }),
            modules: Arc::new(FakeModules {
                trace: trace.clone(),
                pins: Mutex::new(pins),
            }),
            jobs: Arc::new(FakeJobs {
                trace: trace.clone(),
                supported: BTreeMap::from([("mail.send".to_string(), 2u32)]),
                refuse_below,
            }),
            events: Arc::new(FakeEvents {
                trace: trace.clone(),
                stable,
            }),
            trace,
        }
    }

    pub(crate) fn executor(&self) -> RollbackExecutor {
        RollbackExecutor::new(
            self.config.clone(),
            self.migrations.clone(),
            self.modules.clone(),
            self.jobs.clone(),
            self.events.clone(),
        )
    }

    /// A target shape distinct from the harness's live state — used both as
    /// "the release we are promoting to" (§7.2) and "the state we are
    /// rolling back to" (§7.1), since both are a `PromotionTarget`.
    pub(crate) fn target() -> PromotionTarget {
        PromotionTarget {
            config_version: 3,
            db_version: Some(4),
            module_versions: BTreeMap::from([("echo".to_string(), "v2".to_string())]),
            job_schema_versions: BTreeMap::from([("mail.send".to_string(), 1u32)]),
        }
    }
}

/// The four promotion stage probes, all healthy, sharing `trace`.
pub(crate) fn healthy_probes(trace: &Trace) -> Vec<Arc<dyn HealthCheck>> {
    ["post-deploy", "post-migrate", "post-traffic", "canary"]
        .into_iter()
        .map(|name| {
            Arc::new(FakeHealth {
                name,
                outcome: Outcome::Healthy,
                trace: trace.clone(),
            }) as Arc<dyn HealthCheck>
        })
        .collect()
}
