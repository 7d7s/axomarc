//! # kelson-scheduling
//! Recurring execution contracts (11-SPEC §29): schedules declare WHEN; the jobs
//! layer decides HOW (durable job enqueue). Next-run math is deterministic —
//! UTC in the kernel, with optional per-item wall clocks (Wave 6 P4: a
//! zero-dep cron parser, bundled timezones — the IANA offset table moved to
//! `kelson-foundation` in Wave 8 P2 and is re-exported here — overlap/one-server
//! guards and hooks).

pub mod cron;

pub use cron::{daily, daily_at, every_minutes, hourly, monthly, weekly, CronExpr};
pub use kelson_foundation::timezone_offset_secs;

use kelson_foundation::{AppError, TenantId};
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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScheduleItem {
    pub name: String,
    /// Target operation id (module-scoped), e.g. `billing.close-day`.
    pub operation: String,
    pub spec: ScheduleSpec,
    #[serde(default)]
    pub payload: serde_json::Value,
    #[serde(default)]
    pub enabled: bool,
    /// Wave 6 P4: a 5-field cron expression; when set it takes precedence
    /// over `spec` for next-run math (invalid → registration error).
    #[serde(default)]
    pub cron: Option<String>,
    /// Wave 6 P4: IANA zone from the bundled static table (unknown →
    /// registration error). Wall-clock specs (cron/daily/weekly) run in it;
    /// `Interval` is absolute seconds and ignores it.
    #[serde(default)]
    pub timezone: Option<String>,
    /// Wave 6 P4: hold a lock keyed to this item across a firing window so a
    /// still-running pass skips instead of double-firing (Laravel
    /// `withoutOverlapping` parity).
    #[serde(default)]
    pub without_overlapping: bool,
    /// Wave 6 P4: serialize this item's firing across nodes — acquire, run,
    /// release within one tick (Laravel `onOneServer` parity).
    #[serde(default)]
    pub on_one_server: bool,
    /// Wave 6 P4: the overlap lock TTL in seconds = assumed max runtime
    /// (default [`DEFAULT_OVERLAP_TTL_SECS`]).
    #[serde(default)]
    pub lock_ttl_secs: Option<u64>,
    /// Tenant scope (target-arch §3.1): `Some(t)` = a tenant schedule — it
    /// appears only in that tenant's `due_for`/`items_for` view, and its
    /// guard locks + last-fired state are tenant-qualified (see
    /// [`scoped_name`]), so two tenants may register the same schedule name
    /// without sharing windows. `None` = platform/infrastructure schedule
    /// with the pre-Phase-2 keys and visibility, byte-for-byte.
    #[serde(default)]
    pub tenant: Option<TenantId>,
}

/// Scope-qualified schedule identity (target-arch §3.1): platform items
/// (`tenant: None`) keep the bare name — lock keys and persisted state stay
/// byte-identical to pre-Phase-2 — while tenant items are qualified
/// `t:{tenant}:{name}` so same-named schedules in different tenants never
/// share a guard window or a last-fired stamp.
pub fn scoped_name(item: &ScheduleItem) -> String {
    match item.tenant {
        Some(t) => format!("t:{t}:{}", item.name),
        None => item.name.clone(),
    }
}

impl Default for ScheduleSpec {
    fn default() -> Self {
        ScheduleSpec::Interval { secs: 60 }
    }
}

impl ScheduleItem {
    /// Registration gate: spec ranges, the bundled timezone table, and the
    /// cron parse plus a never-fires probe (an impossible date like
    /// `0 0 31 2 *` refuses `register` instead of silently never running).
    pub fn validate(&self) -> Result<(), AppError> {
        self.spec.validate()?;
        let offset_secs = self.offset_secs()?;
        if let Some(expr) = &self.cron {
            let parsed = CronExpr::parse(expr)?;
            if parsed
                .next_after(OffsetDateTime::now_utc(), offset_secs)
                .is_none()
            {
                return Err(AppError::validation(format!(
                    "cron expression never fires: `{expr}`"
                )));
            }
        }
        Ok(())
    }

    /// Next run strictly after `from`. `cron` takes precedence over `spec`;
    /// `Interval` is absolute seconds (timezone ignored); daily/weekly wall
    /// clocks — like cron — run in the item's zone when one is set. UTC
    /// items keep the original spec math byte-for-byte.
    pub fn next_run(&self, from: OffsetDateTime) -> Result<OffsetDateTime, AppError> {
        self.spec.validate()?;
        let offset_secs = self.offset_secs()?;
        if let Some(expr) = &self.cron {
            let parsed = CronExpr::parse(expr)?;
            return parsed
                .next_after(from, offset_secs)
                .ok_or_else(|| {
                    AppError::validation(format!("cron expression never fires: `{expr}`"))
                });
        }
        if offset_secs == 0 {
            return self.spec.next_run(from);
        }
        match &self.spec {
            ScheduleSpec::Interval { .. } => self.spec.next_run(from),
            // Daily / Weekly: the same wall clock, in the item's zone.
            _ => {
                let expr = wall_clock_cron(&self.spec)?;
                expr.next_after(from, offset_secs).ok_or_else(|| {
                    AppError::validation("schedule never fires in this timezone")
                })
            }
        }
    }

    fn offset_secs(&self) -> Result<i32, AppError> {
        match &self.timezone {
            Some(name) => timezone_offset_secs(name).ok_or_else(|| {
                AppError::validation(format!(
                    "unknown timezone `{name}` (bundled static table; see docs/19 P4)"
                ))
            }),
            None => Ok(0),
        }
    }
}

/// The wall-clock cron equivalent of a non-interval spec (used only for
/// zoned items; UTC items keep [`ScheduleSpec::next_run`]).
fn wall_clock_cron(spec: &ScheduleSpec) -> Result<CronExpr, AppError> {
    match spec {
        ScheduleSpec::Daily { hour, minute } => {
            CronExpr::parse(&format!("{minute} {hour} * * *"))
        }
        ScheduleSpec::Weekly { day, hour, minute } => CronExpr::parse(&format!(
            "{minute} {hour} * * {}",
            weekday_to_sunday_zero(*day)
        )),
        ScheduleSpec::Interval { .. } => {
            Err(AppError::internal("interval schedules have no wall clock"))
        }
    }
}

fn weekday_to_sunday_zero(day: Weekday) -> u8 {
    match day {
        Weekday::Sunday => 0,
        Weekday::Monday => 1,
        Weekday::Tuesday => 2,
        Weekday::Wednesday => 3,
        Weekday::Thursday => 4,
        Weekday::Friday => 5,
        Weekday::Saturday => 6,
    }
}

// ------------------------------------------------------------------ hooks ---

/// A `before`/`after`/`on_success` hook: returning `Err` records a failure
/// (`before` additionally skips the enqueue — the window is not fired).
pub type ScheduleHook = Arc<dyn Fn(&ScheduleItem) -> Result<(), AppError> + Send + Sync>;
/// An `on_failure` hook: observes the recorded failure (infallible by
/// contract — a broken observer must not mask the original error).
pub type ScheduleFailureHook = Arc<dyn Fn(&ScheduleItem, &AppError) + Send + Sync>;

/// Per-item callbacks (docs/19 P4), registered alongside the item:
/// `before` gates the run, `after` observes every attempt, `on_success` /
/// `on_failure` split on the enqueue outcome. Hook errors are recorded in
/// the scheduler state doc like any other failure.
#[derive(Clone, Default)]
pub struct ScheduleHooks {
    pub before: Vec<ScheduleHook>,
    pub after: Vec<ScheduleHook>,
    pub on_success: Vec<ScheduleHook>,
    pub on_failure: Vec<ScheduleFailureHook>,
}

impl std::fmt::Debug for ScheduleHooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScheduleHooks")
            .field("before", &self.before.len())
            .field("after", &self.after.len())
            .field("on_success", &self.on_success.len())
            .field("on_failure", &self.on_failure.len())
            .finish()
    }
}

// ------------------------------------------------------------------ locks ---

/// Guard port for the scheduler's windows (docs/19 P4). The runner default
/// is the in-process [`MemoryScheduleLock`]; the worker wires a cache-backed
/// impl over `CacheService::lock` (the Redis adapter when `REDIS_URL`
/// connects, memory otherwise) so `on_one_server` spans nodes exactly when
/// Redis is configured and silently degrades to a local lock when it is not
/// (D-52 single-server posture).
pub trait ScheduleLock: Send + Sync {
    /// Try to hold `key` for `ttl_secs`; `false` = held elsewhere (skip).
    fn try_acquire(&self, key: &str, ttl_secs: u64) -> bool;
    /// Free `key` after a bounded window (`on_one_server`, and a failed
    /// overlap run); overlap locks otherwise expire by TTL alone.
    fn release(&self, key: &str);
}

/// In-process TTL lock — the runner default and the local-only degradation.
/// Entries are purged lazily on acquisition, so an expired hold frees the
/// window without a background task.
#[derive(Default)]
pub struct MemoryScheduleLock {
    held: Mutex<HashMap<String, (std::time::Instant, u64)>>,
}

impl MemoryScheduleLock {
    pub fn new() -> Self {
        Self::default()
    }
}

impl ScheduleLock for MemoryScheduleLock {
    fn try_acquire(&self, key: &str, ttl_secs: u64) -> bool {
        let ttl = ttl_secs.max(1);
        let now = std::time::Instant::now();
        let mut held = self.held.lock().unwrap();
        held.retain(|_, (acquired, entry_ttl)| {
            now.duration_since(*acquired).as_secs() < (*entry_ttl).max(1)
        });
        if held.contains_key(key) {
            false
        } else {
            held.insert(key.to_string(), (now, ttl));
            true
        }
    }

    fn release(&self, key: &str) {
        self.held.lock().unwrap().remove(key);
    }
}

/// In-memory registry with deterministic due-scan (persistence = postgres adapter,
/// later). `due` is a pure function of (items, last-fired, now) — testable, replayable.
#[derive(Debug, Default)]
pub struct ScheduleRegistry {
    items: Vec<ScheduleItem>,
    /// Wave 6 P4: hooks by item name (`register_hooks` after the item).
    hooks: HashMap<String, ScheduleHooks>,
}

impl ScheduleRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, item: ScheduleItem) -> Result<(), AppError> {
        item.validate()?;
        // Name uniqueness is SCOPE-qualified: a platform name and a tenant
        // name (or the same name under two tenants) are distinct schedules.
        if self
            .items
            .iter()
            .any(|i| i.name == item.name && i.tenant == item.tenant)
        {
            return Err(AppError::validation(format!(
                "schedule registered twice: {}",
                item.name
            )));
        }
        // ...but RUNTIME state — the overlap lock and the last-fired slot —
        // is keyed by `scoped_name`, so uniqueness has to hold on THAT key
        // too. Without this a platform schedule literally named
        // `t:{tenant}:{name}` passes the check above while aliasing that
        // tenant's same-named schedule: two unrelated schedules sharing one
        // lock and one last-fired timestamp, so one silently suppresses the
        // other. (Phase 5 pentest; the two key spaces must agree.)
        if let Some(clash) = self
            .items
            .iter()
            .find(|i| scoped_name(i) == scoped_name(&item))
        {
            return Err(AppError::validation(format!(
                "schedule name `{}` collides with `{}` on runtime key `{}`",
                item.name,
                clash.name,
                scoped_name(&item)
            )));
        }
        self.items.push(item);
        Ok(())
    }

    /// Attach Wave 6 P4 hooks to an already-registered item (unknown name
    /// or a second registration refuses — hooks are boot-time wiring).
    pub fn register_hooks(&mut self, name: &str, hooks: ScheduleHooks) -> Result<(), AppError> {
        if !self.items.iter().any(|i| i.name == name) {
            return Err(AppError::validation(format!(
                "hooks for unregistered schedule: {name}"
            )));
        }
        if self.hooks.contains_key(name) {
            return Err(AppError::validation(format!(
                "hooks registered twice: {name}"
            )));
        }
        self.hooks.insert(name.to_string(), hooks);
        Ok(())
    }

    pub fn hooks_for(&self, name: &str) -> Option<&ScheduleHooks> {
        self.hooks.get(name)
    }

    pub fn names(&self) -> Vec<&str> {
        self.items.iter().map(|i| i.name.as_str()).collect()
    }

    /// Wave 8 ADR 0009: read access for the ops dashboard (name +
    /// `next_run(now)` per registered item). Mutation stays boot-time
    /// (`register`); nothing here writes.
    pub fn items(&self) -> &[ScheduleItem] {
        &self.items
    }

    /// Items whose next run (strictly after `last_fired`) is <= `now`.
    /// The platform-wide view (the worker's tick): every enabled item,
    /// tenant and platform alike.
    pub fn due<'a>(
        &'a self,
        last_fired: &[(String, OffsetDateTime)],
        now: OffsetDateTime,
    ) -> Result<Vec<&'a ScheduleItem>, AppError> {
        self.due_inner(last_fired, now, |_| true)
    }

    /// §3.1 tenant view: due-scan for ONE tenant. Platform items
    /// (`tenant: None`) are infrastructure schedules and never appear here;
    /// `last_fired` entries match on the scope-qualified name (see
    /// [`scoped_name`]), so same-named schedules in other tenants cannot
    /// suppress this tenant's firing.
    pub fn due_for<'a>(
        &'a self,
        tenant: TenantId,
        last_fired: &[(String, OffsetDateTime)],
        now: OffsetDateTime,
    ) -> Result<Vec<&'a ScheduleItem>, AppError> {
        self.due_inner(last_fired, now, |item| item.tenant == Some(tenant))
    }

    /// Shared due-scan: enabled items passing `keep`, matched against
    /// last-fired by scope-qualified name.
    fn due_inner<'a, F>(
        &'a self,
        last_fired: &[(String, OffsetDateTime)],
        now: OffsetDateTime,
        keep: F,
    ) -> Result<Vec<&'a ScheduleItem>, AppError>
    where
        F: Fn(&ScheduleItem) -> bool,
    {
        let mut out = Vec::new();
        for item in &self.items {
            if !item.enabled || !keep(item) {
                continue;
            }
            let key = scoped_name(item);
            let base = last_fired
                .iter()
                .find(|(n, _)| n == &key)
                .map(|(_, t)| *t)
                .unwrap_or(now - Duration::days(365 * 10));
            if item.next_run(base)? <= now {
                out.push(item);
            }
        }
        Ok(out)
    }

    /// Every schedule registered for ONE tenant (ops views). Platform items
    /// are excluded — they belong to the platform-wide [`names`](Self::names).
    pub fn items_for(&self, tenant: TenantId) -> Vec<&ScheduleItem> {
        self.items
            .iter()
            .filter(|i| i.tenant == Some(tenant))
            .collect()
    }
}

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Wave 6 P4: default `without_overlapping` TTL — the assumed max runtime
/// of one firing window (per-item override: `lock_ttl_secs`).
pub const DEFAULT_OVERLAP_TTL_SECS: u64 = 3600;
/// Wave 6 P4: `on_one_server` lock TTL — a safety valve only (the lock is
/// released right after the run attempt; it bounds a crashed holder).
pub const ONE_SERVER_TTL_SECS: u64 = 300;
/// Failures kept in the state doc (oldest evicted past the cap).
const MAX_STATE_FAILURES: usize = 100;

/// Scheduler runner (L35, S3): turns the pure `due` scan into worker ticks.
/// The enqueue closure keeps this crate free of a jobs dependency — the
/// worker wires it to `JobRegistry::build` + `JobStore::enqueue`, so a
/// firing schedule is a durable execution like any other front door.
/// Wave 6 P4 adds guard locks, hooks and failure recording: a failing item
/// is recorded and skipped, never aborting the pass.
pub struct SchedulerRunner {
    registry: Arc<ScheduleRegistry>,
    /// (schedule name, last fired instant); persisted optionally via JSON.
    last_fired: Mutex<Vec<(String, OffsetDateTime)>>,
    /// Wave 6 P4: recorded run/hook failures (persisted in the state doc).
    failures: Mutex<Vec<FailureEntry>>,
    /// Wave 6 P4: guard port — in-process default; the worker swaps in a
    /// cache-backed lock (Redis when configured).
    lock: Arc<dyn ScheduleLock>,
}

impl SchedulerRunner {
    pub fn new(registry: Arc<ScheduleRegistry>) -> Self {
        Self {
            registry,
            last_fired: Mutex::new(Vec::new()),
            failures: Mutex::new(Vec::new()),
            lock: Arc::new(MemoryScheduleLock::new()),
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
            ..Self::new(Arc::new(ScheduleRegistry::new()))
        }
    }

    /// Wave 6 P4: seed the full persisted snapshot (last-fired + failures)
    /// so a restart keeps previously recorded failures.
    pub fn with_state_doc(registry: Arc<ScheduleRegistry>, doc: SchedulerStateDoc) -> Self {
        let SchedulerStateDoc { last_fired, failures } = doc;
        let state = last_fired
            .into_iter()
            .map(|entry| (entry.name, entry.at))
            .collect();
        Self {
            registry,
            last_fired: Mutex::new(state),
            failures: Mutex::new(failures),
            ..Self::new(Arc::new(ScheduleRegistry::new()))
        }
    }

    /// Replace the guard port (worker: cache/Redis-backed).
    pub fn with_lock(mut self, lock: Arc<dyn ScheduleLock>) -> Self {
        self.lock = lock;
        self
    }

    pub fn registry(&self) -> &Arc<ScheduleRegistry> {
        &self.registry
    }

    /// Current last-fired snapshot (for persistence).
    pub fn state(&self) -> Vec<(String, OffsetDateTime)> {
        self.last_fired.lock().unwrap().clone()
    }

    /// Wave 6 P4: recorded failures (newest last).
    pub fn failures(&self) -> Vec<FailureEntry> {
        self.failures.lock().unwrap().clone()
    }

    /// Wave 6 P4: the full persisted snapshot — last-fired plus failures.
    pub fn state_doc(&self) -> SchedulerStateDoc {
        SchedulerStateDoc {
            last_fired: self
                .state()
                .into_iter()
                .map(|(name, at)| FiredEntry { name, at })
                .collect(),
            failures: self.failures(),
        }
    }

    /// One tick: fire every due schedule through `enqueue`, honoring the
    /// item's guards and hooks. A failing item (enqueue or `before`) is
    /// RECORDED and skipped — it does not abort the pass and does not
    /// advance its last-fired stamp, so the next tick retries it. Returns
    /// the names that actually fired, in registry order.
    pub fn tick(
        &self,
        now: OffsetDateTime,
        mut enqueue: impl FnMut(&ScheduleItem) -> Result<(), AppError>,
    ) -> Result<Vec<String>, AppError> {
        let state = self.last_fired.lock().unwrap().clone();
        let due = self.registry.due(&state, now)?;
        let mut fired: Vec<String> = Vec::new();
        let mut recorded: Vec<FailureEntry> = Vec::new();
        for item in due {
            let one_key = format!("schedule:one-server:{}", scoped_name(item));
            let overlap_key = format!("schedule:overlap:{}", scoped_name(item));
            // Guards: a held lock means "another run/node owns this window"
            // — skip silently, never double-fire.
            if item.on_one_server && !self.lock.try_acquire(&one_key, ONE_SERVER_TTL_SECS) {
                continue;
            }
            let mut overlap_acquired = false;
            if item.without_overlapping {
                let ttl = item
                    .lock_ttl_secs
                    .unwrap_or(DEFAULT_OVERLAP_TTL_SECS)
                    .max(1);
                if !self.lock.try_acquire(&overlap_key, ttl) {
                    if item.on_one_server {
                        self.lock.release(&one_key);
                    }
                    continue; // previous run still inside its max-runtime window
                }
                overlap_acquired = true;
            }
            let hooks = self.registry.hooks_for(&item.name);
            // `before` gates the run: a rejecting hook fails this window.
            let mut outcome: Result<(), AppError> = Ok(());
            if let Some(hooks) = hooks {
                for hook in &hooks.before {
                    if let Err(e) = hook(item) {
                        outcome = Err(e);
                        break;
                    }
                }
            }
            if outcome.is_ok() {
                outcome = enqueue(item);
            }
            let ok = outcome.is_ok();
            match &outcome {
                Ok(()) => {
                    fired.push(scoped_name(item));
                    if let Some(hooks) = hooks {
                        for hook in &hooks.after {
                            if let Err(e) = hook(item) {
                                recorded.push(hook_failure(item, "after", &e, now));
                            }
                        }
                        for hook in &hooks.on_success {
                            if let Err(e) = hook(item) {
                                recorded.push(hook_failure(item, "on_success", &e, now));
                            }
                        }
                    }
                }
                Err(e) => {
                    recorded.push(FailureEntry {
                        name: scoped_name(item),
                        at: now,
                        error: error_text(e),
                    });
                    if let Some(hooks) = hooks {
                        for hook in &hooks.after {
                            if let Err(err) = hook(item) {
                                recorded.push(hook_failure(item, "after", &err, now));
                            }
                        }
                        for hook in &hooks.on_failure {
                            hook(item, e);
                        }
                    }
                }
            }
            if overlap_acquired && !ok {
                // A failed run is not "in flight" — free the window so the
                // next tick can retry instead of waiting out the TTL.
                self.lock.release(&overlap_key);
            }
            if item.on_one_server {
                self.lock.release(&one_key);
            }
        }
        if !fired.is_empty() || !recorded.is_empty() {
            let mut last_fired = self.last_fired.lock().unwrap();
            for name in &fired {
                match last_fired.iter_mut().find(|(n, _)| n == name) {
                    Some(entry) => entry.1 = now,
                    None => last_fired.push((name.clone(), now)),
                }
            }
            if !recorded.is_empty() {
                let mut failures = self.failures.lock().unwrap();
                failures.append(&mut recorded);
                if failures.len() > MAX_STATE_FAILURES {
                    let excess = failures.len() - MAX_STATE_FAILURES;
                    failures.drain(0..excess);
                }
            }
        }
        Ok(fired)
    }
}

fn hook_failure(
    item: &ScheduleItem,
    context: &str,
    err: &AppError,
    at: OffsetDateTime,
) -> FailureEntry {
    FailureEntry {
        name: scoped_name(item),
        at,
        error: format!("{context}: {}", error_text(err)),
    }
}

/// The recorded failure text: internal detail when present (this is an
/// operator-facing local state file, never served through an endpoint),
/// else the public message.
fn error_text(err: &AppError) -> String {
    err.internal
        .clone()
        .unwrap_or_else(|| err.public_message.clone())
}

/// Persisted tick state (storage/framework/schedule-state.json) so a CLI
/// `schedule:run` or a restarting worker does not re-fire immediately.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FiredEntry {
    pub name: String,
    #[serde(with = "time::serde::iso8601")]
    pub at: OffsetDateTime,
}

/// Wave 6 P4: one recorded run/hook failure (scheduler state doc).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FailureEntry {
    pub name: String,
    #[serde(with = "time::serde::iso8601")]
    pub at: OffsetDateTime,
    /// The captured failure text — internal detail when present (the state
    /// file is operator-only), else the public message (hook failures
    /// carry the hook context as a prefix).
    pub error: String,
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct SchedulerStateDoc {
    pub last_fired: Vec<FiredEntry>,
    /// Wave 6 P4: recorded failures, capped (oldest evicted). `serde(default)`
    /// keeps pre-P4 state files loading unchanged.
    #[serde(default)]
    pub failures: Vec<FailureEntry>,
}

pub fn state_path(root: &std::path::Path) -> std::path::PathBuf {
    root.join("storage/framework/schedule-state.json")
}

/// Wave 6 P4: read the whole state doc (last-fired plus failures).
pub fn load_state_doc(root: &std::path::Path) -> Result<SchedulerStateDoc, AppError> {
    match std::fs::read_to_string(state_path(root)) {
        Ok(raw) => serde_json::from_str(&raw)
            .map_err(|e| AppError::internal(format!("schedule state corrupt: {e}"))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(SchedulerStateDoc::default()),
        Err(e) => Err(AppError::internal(format!(
            "schedule state unreadable: {e}"
        ))),
    }
}

pub fn load_state(root: &std::path::Path) -> Result<Vec<(String, OffsetDateTime)>, AppError> {
    Ok(load_state_doc(root)?
        .last_fired
        .into_iter()
        .map(|entry| (entry.name, entry.at))
        .collect())
}

/// Wave 6 P4: persist the full doc (last-fired + failures).
pub fn save_state_doc(root: &std::path::Path, doc: &SchedulerStateDoc) -> Result<(), AppError> {
    let path = state_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| AppError::internal(format!("schedule state dir: {e}")))?;
    }
    let raw = serde_json::to_string_pretty(doc).map_err(|e| AppError::internal(e.to_string()))?;
    std::fs::write(&path, raw).map_err(|e| AppError::internal(format!("schedule state write: {e}")))
}

/// Persist last-fired state while preserving failures already on disk (the
/// tuple-shaped path; Wave 6 P4 callers prefer [`save_state_doc`], which
/// writes both halves of [`SchedulerRunner::state_doc`]).
pub fn save_state(
    root: &std::path::Path,
    last_fired: &[(String, OffsetDateTime)],
) -> Result<(), AppError> {
    let mut doc = load_state_doc(root).unwrap_or_default();
    doc.last_fired = last_fired
        .iter()
        .cloned()
        .map(|(name, at)| FiredEntry { name, at })
        .collect();
    save_state_doc(root, &doc)
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
            ..Default::default()
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
            ..Default::default()
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
            "kelson-sched-test-{}",
            time::OffsetDateTime::now_utc().unix_timestamp()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let state = vec![("close-day".to_string(), datetime!(2026-08-19 23:00:00 UTC))];
        save_state(&root, &state).unwrap();
        let loaded = load_state(&root).unwrap();
        assert_eq!(loaded, state);
        std::fs::remove_dir_all(&root).ok();
    }

    // ------------------------------------------------- Wave 6 P4 (cron/tz)

    fn item(name: &str, spec: ScheduleSpec) -> ScheduleItem {
        ScheduleItem {
            name: name.into(),
            operation: "billing.close-day".into(),
            spec,
            payload: serde_json::json!({}),
            enabled: true,
            ..Default::default()
        }
    }

    #[test]
    fn registration_rejects_invalid_cron_and_unknown_timezones() {
        let mut reg = ScheduleRegistry::new();

        let mut bad_cron = item("bad-cron", ScheduleSpec::Interval { secs: 60 });
        bad_cron.cron = Some("61 * * * *".into());
        let err = reg.register(bad_cron).unwrap_err();
        assert!(
            err.public_message.contains("invalid cron expression"),
            "{}",
            err.public_message
        );

        let mut never = item("never", ScheduleSpec::Interval { secs: 60 });
        never.cron = Some("0 0 31 2 *".into());
        let err = reg.register(never).unwrap_err();
        assert!(
            err.public_message.contains("never fires"),
            "{}",
            err.public_message
        );

        let mut bad_tz = item("bad-tz", ScheduleSpec::Interval { secs: 60 });
        bad_tz.timezone = Some("Mars/Olympus".into());
        let err = reg.register(bad_tz).unwrap_err();
        assert!(
            err.public_message.contains("unknown timezone"),
            "{}",
            err.public_message
        );

        let mut ok = item("ok", ScheduleSpec::Interval { secs: 60 });
        ok.cron = Some(hourly());
        ok.timezone = Some("Asia/Kolkata".into());
        reg.register(ok).unwrap();
    }

    #[test]
    fn cron_items_take_precedence_over_the_spec() {
        let mut scheduled = item("close-day", ScheduleSpec::Interval { secs: 60 });
        scheduled.cron = Some("30 14 * * *".into());
        let t0 = datetime!(2026-08-19 10:00:00 UTC);
        assert_eq!(
            scheduled.next_run(t0).unwrap(),
            datetime!(2026-08-19 14:30:00 UTC)
        );

        let mut reg = ScheduleRegistry::new();
        reg.register(scheduled).unwrap();
        let runner = SchedulerRunner::new(Arc::new(reg));

        // Never fired → due (today's slot is still ahead).
        let fired = runner.tick(t0, |_| Ok(())).unwrap();
        assert_eq!(fired, vec!["close-day".to_string()]);
        // 60s later the Interval spec WOULD be due; the cron says tomorrow
        // 14:30 — cron wins and the enqueue never runs.
        let fired = runner
            .tick(t0 + Duration::seconds(60), |_| {
                panic!("cron takes precedence over the interval spec")
            })
            .unwrap();
        assert!(fired.is_empty());
    }

    #[test]
    fn zoned_items_run_in_the_item_wall_clock() {
        // Daily 13:00 IST — from 10:00 UTC (15:30 IST): tomorrow's slot.
        let mut daily = item("standup", ScheduleSpec::Daily {
            hour: 13,
            minute: 0,
        });
        daily.timezone = Some("Asia/Kolkata".into());
        assert_eq!(
            daily.next_run(datetime!(2026-09-30 10:00:00 UTC))
                .unwrap(),
            datetime!(2026-10-01 07:30:00 UTC)
        );
        // From 04:00 UTC (09:30 IST): the same day's slot.
        assert_eq!(
            daily.next_run(datetime!(2026-09-30 04:00:00 UTC))
                .unwrap(),
            datetime!(2026-09-30 07:30:00 UTC)
        );

        // A zoned cron resolves to the same instants.
        let mut cron = item("standup-cron", ScheduleSpec::Interval { secs: 60 });
        cron.cron = Some("0 13 * * *".into());
        cron.timezone = Some("Asia/Kolkata".into());
        assert_eq!(
            cron.next_run(datetime!(2026-09-30 04:00:00 UTC))
                .unwrap(),
            datetime!(2026-09-30 07:30:00 UTC)
        );

        // The UTC item keeps the original spec math untouched.
        let utc_daily = item("utc-daily", ScheduleSpec::Daily {
            hour: 13,
            minute: 0,
        });
        assert_eq!(
            utc_daily.next_run(datetime!(2026-09-30 10:00:00 UTC))
                .unwrap(),
            datetime!(2026-09-30 13:00:00 UTC)
        );
    }

    // ------------------------------------------------ Wave 6 P4 (guards/hook)

    /// Scripted [`ScheduleLock`]: consumes canned `try_acquire` answers
    /// (granted once exhausted) and records every call.
    struct RecordingLock {
        answers: Mutex<std::collections::VecDeque<bool>>,
        log: Mutex<Vec<String>>,
    }

    impl RecordingLock {
        fn new(answers: &[bool]) -> Self {
            Self {
                answers: Mutex::new(answers.iter().copied().collect()),
                log: Mutex::new(Vec::new()),
            }
        }

        fn log(&self) -> Vec<String> {
            self.log.lock().unwrap().clone()
        }
    }

    impl ScheduleLock for RecordingLock {
        fn try_acquire(&self, key: &str, _ttl_secs: u64) -> bool {
            self.log.lock().unwrap().push(format!("acquire:{key}"));
            self.answers.lock().unwrap().pop_front().unwrap_or(true)
        }

        fn release(&self, key: &str) {
            self.log.lock().unwrap().push(format!("release:{key}"));
        }
    }

    #[test]
    fn overlap_guard_skips_while_the_window_is_held() {
        let mut scheduled = item("close-day", ScheduleSpec::Interval { secs: 60 });
        scheduled.without_overlapping = true;
        let mut reg = ScheduleRegistry::new();
        reg.register(scheduled).unwrap();

        // Tick 1 acquires; tick 2 is refused (window held); tick 3 grants.
        let lock = Arc::new(RecordingLock::new(&[true, false, true]));
        let runner = SchedulerRunner::new(Arc::new(reg)).with_lock(lock.clone());
        let t0 = datetime!(2026-08-19 10:00:00 UTC);

        let fired = runner.tick(t0, |_| Ok(())).unwrap();
        assert_eq!(fired, vec!["close-day".to_string()]);
        // Held: the due window skips — no enqueue, no state advance.
        let fired = runner
            .tick(t0 + Duration::seconds(60), |_| {
                panic!("enqueue must not run while the overlap lock is held")
            })
            .unwrap();
        assert!(fired.is_empty());
        // Granted again: fires.
        let fired = runner.tick(t0 + Duration::seconds(120), |_| Ok(())).unwrap();
        assert_eq!(fired, vec!["close-day".to_string()]);

        let key = "acquire:schedule:overlap:close-day".to_string();
        assert_eq!(lock.log(), vec![key.clone(), key.clone(), key]);
        // Successes hold the window (TTL = max runtime); no release calls.
    }

    #[test]
    fn one_server_guard_acquires_runs_and_releases() {
        let mut scheduled = item("close-day", ScheduleSpec::Interval { secs: 60 });
        scheduled.on_one_server = true;
        let mut reg = ScheduleRegistry::new();
        reg.register(scheduled).unwrap();

        let lock = Arc::new(RecordingLock::new(&[false, true]));
        let runner = SchedulerRunner::new(Arc::new(reg)).with_lock(lock.clone());
        let t0 = datetime!(2026-08-19 10:00:00 UTC);

        // Refused: another node owns this window — skip without enqueue.
        let fired = runner
            .tick(t0, |_| panic!("another node owns this window"))
            .unwrap();
        assert!(fired.is_empty());

        // Granted: enqueue runs, then the lock releases within the tick.
        let fired = runner.tick(t0 + Duration::seconds(60), |_| Ok(())).unwrap();
        assert_eq!(fired, vec!["close-day".to_string()]);
        assert_eq!(
            lock.log(),
            vec![
                "acquire:schedule:one-server:close-day".to_string(),
                "acquire:schedule:one-server:close-day".to_string(),
                "release:schedule:one-server:close-day".to_string(),
            ]
        );
    }

    #[test]
    fn hooks_gate_and_observe_successful_runs() {
        let mut reg = ScheduleRegistry::new();
        reg.register(item("close-day", ScheduleSpec::Interval { secs: 60 }))
            .unwrap();

        let calls: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let mut hooks = ScheduleHooks::default();
        for label in ["before", "after", "on_success"] {
            let calls = calls.clone();
            let hook: ScheduleHook = Arc::new(move |_| {
                calls.lock().unwrap().push(label.to_string());
                Ok(())
            });
            match label {
                "before" => hooks.before.push(hook),
                "after" => hooks.after.push(hook),
                _ => hooks.on_success.push(hook),
            }
        }
        reg.register_hooks("close-day", hooks).unwrap();

        let runner = SchedulerRunner::new(Arc::new(reg));
        let fired = runner.tick(datetime!(2026-08-19 10:00:00 UTC), |_| Ok(())).unwrap();
        assert_eq!(fired, vec!["close-day".to_string()]);
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            &[
                "before".to_string(),
                "after".to_string(),
                "on_success".to_string()
            ]
        );
        assert!(runner.failures().is_empty());
    }

    #[test]
    fn before_hook_failures_gate_the_run_and_are_recorded() {
        let mut reg = ScheduleRegistry::new();
        reg.register(item("close-day", ScheduleSpec::Interval { secs: 60 }))
            .unwrap();

        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let mut hooks = ScheduleHooks::default();
        hooks.before.push(Arc::new(|_| {
            Err(AppError::validation("gate closed"))
        }));
        let seen2 = seen.clone();
        hooks.on_failure.push(Arc::new(move |i, e| {
            seen2
                .lock()
                .unwrap()
                .push(format!("{}: {}", i.name, e.public_message));
        }));
        reg.register_hooks("close-day", hooks).unwrap();

        let runner = SchedulerRunner::new(Arc::new(reg));
        let fired = runner
            .tick(datetime!(2026-08-19 10:00:00 UTC), |_| {
                panic!("a rejecting before hook must skip the enqueue")
            })
            .unwrap();
        assert!(fired.is_empty());
        let failures = runner.failures();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].name, "close-day");
        assert_eq!(failures[0].error, "gate closed");
        assert_eq!(
            seen.lock().unwrap().as_slice(),
            &["close-day: gate closed".to_string()]
        );

        // Not advanced: the next tick retries (and records again).
        let fired = runner
            .tick(datetime!(2026-08-19 10:01:00 UTC), |_| Ok(()))
            .unwrap();
        assert!(fired.is_empty());
        assert_eq!(runner.failures().len(), 2);
    }

    #[test]
    fn enqueue_failures_are_recorded_and_the_pass_continues() {
        let mut reg = ScheduleRegistry::new();
        reg.register(item("bad", ScheduleSpec::Interval { secs: 60 }))
            .unwrap();
        reg.register(item("good", ScheduleSpec::Interval { secs: 60 }))
            .unwrap();
        let runner = SchedulerRunner::new(Arc::new(reg));
        let t0 = datetime!(2026-08-19 10:00:00 UTC);

        let fired = runner
            .tick(t0, |i| {
                if i.name == "bad" {
                    Err(AppError::internal("store unavailable"))
                } else {
                    Ok(())
                }
            })
            .unwrap();
        // One bad item never aborts the pass: the healthy item fired.
        assert_eq!(fired, vec!["good".to_string()]);
        let failures = runner.failures();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].name, "bad");
        assert_eq!(failures[0].error, "store unavailable");
        // last_fired: only `good` advanced.
        assert_eq!(runner.state().len(), 1);

        // `bad` is retried next tick (still due, state untouched).
        let fired = runner
            .tick(t0 + Duration::seconds(30), |_| Ok(()))
            .unwrap();
        assert_eq!(fired, vec!["bad".to_string()]);
    }

    #[test]
    fn failures_are_capped_and_persist_through_the_state_doc() {
        let mut reg = ScheduleRegistry::new();
        reg.register(item("bad", ScheduleSpec::Interval { secs: 60 }))
            .unwrap();
        let runner = SchedulerRunner::new(Arc::new(reg));
        let now = datetime!(2026-08-19 10:00:00 UTC);
        for _ in 0..(MAX_STATE_FAILURES as u32 + 5) {
            runner
                .tick(now, |_| Err(AppError::internal("down")))
                .unwrap();
        }
        let failures = runner.failures();
        assert_eq!(failures.len(), MAX_STATE_FAILURES);
        assert_eq!(failures[0].error, "down");

        // The state doc round-trips both halves.
        let root = std::env::temp_dir().join(format!(
            "kelson-sched-doc-{}",
            time::OffsetDateTime::now_utc().unix_timestamp_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        save_state_doc(&root, &runner.state_doc()).unwrap();
        let loaded = load_state_doc(&root).unwrap();
        assert!(loaded.last_fired.is_empty()); // no successful fires
        assert_eq!(loaded.failures.len(), MAX_STATE_FAILURES);

        // Pre-P4 files (last_fired only) still load - failures default.
        std::fs::write(state_path(&root), r#"{"last_fired":[]}"#).unwrap();
        let legacy = load_state_doc(&root).unwrap();
        assert!(legacy.failures.is_empty());
        std::fs::remove_dir_all(&root).ok();
    }

    // ------------------------------------------- tenant isolation probes (DoD)

    fn tenant_item(name: &str, tenant: TenantId, secs: u64) -> ScheduleItem {
        ScheduleItem {
            name: name.into(),
            operation: "billing.close-day".into(),
            spec: ScheduleSpec::Interval { secs },
            payload: serde_json::json!({}),
            enabled: true,
            tenant: Some(tenant),
            ..Default::default()
        }
    }

    /// Tenant due-scan: only the calling tenant's items — same-named
    /// schedules in other tenants and platform items never appear, while
    /// the platform-wide view (the worker's tick) still sees everything.
    #[test]
    fn tenant_isolation_scheduler_due_scan_is_tenant_scoped() {
        let a = TenantId::new();
        let b = TenantId::new();
        let mut reg = ScheduleRegistry::new();
        reg.register(tenant_item("close-day", a, 60)).unwrap();
        // Same name under another tenant — allowed (scope-qualified).
        reg.register(tenant_item("close-day", b, 60)).unwrap();
        reg.register(item("platform-daily", ScheduleSpec::Interval { secs: 60 }))
            .unwrap();

        let now = datetime!(2026-08-19 10:00:00 UTC);
        let due_a = reg.due_for(a, &[], now).unwrap();
        assert_eq!(due_a.len(), 1, "tenant A's view is exactly its own item");
        assert_eq!(due_a[0].tenant, Some(a));
        let due_b = reg.due_for(b, &[], now).unwrap();
        assert_eq!(due_b.len(), 1);
        assert_eq!(due_b[0].tenant, Some(b));

        assert_eq!(reg.due(&[], now).unwrap().len(), 3, "platform view sees all");
        assert_eq!(reg.items_for(a).len(), 1);
        assert_eq!(reg.items_for(b).len(), 1);
    }

    /// Same-named tenant schedules get independent guard windows: one
    /// tenant's overlap lock never blocks the other's firing, and the lock
    /// keys are tenant-qualified.
    #[test]
    fn tenant_isolation_scheduler_guard_locks_do_not_collide_across_tenants() {
        let a = TenantId::new();
        let b = TenantId::new();
        let mut item_a = tenant_item("close-day", a, 60);
        item_a.without_overlapping = true;
        let mut item_b = tenant_item("close-day", b, 60);
        item_b.without_overlapping = true;
        let mut reg = ScheduleRegistry::new();
        reg.register(item_a).unwrap();
        reg.register(item_b).unwrap();

        let lock = Arc::new(RecordingLock::new(&[])); // every acquire granted
        let runner = SchedulerRunner::new(Arc::new(reg)).with_lock(lock.clone());
        let fired = runner
            .tick(datetime!(2026-08-19 10:00:00 UTC), |_| Ok(()))
            .unwrap();
        assert_eq!(fired.len(), 2, "both tenants fire in the same tick");
        let log = lock.log();
        assert!(
            log.contains(&format!("acquire:schedule:overlap:t:{a}:close-day")),
            "tenant A's lock key must be qualified: {log:?}"
        );
        assert!(
            log.contains(&format!("acquire:schedule:overlap:t:{b}:close-day")),
            "tenant B's lock key must be qualified: {log:?}"
        );
    }

    /// Last-fired state is scope-qualified: a seed for tenant A's schedule
    /// does not suppress tenant B's same-named schedule.
    #[test]
    fn tenant_isolation_scheduler_last_fired_state_is_per_tenant() {
        let a = TenantId::new();
        let b = TenantId::new();
        let mut reg = ScheduleRegistry::new();
        reg.register(tenant_item("close-day", a, 60)).unwrap();
        reg.register(tenant_item("close-day", b, 60)).unwrap();

        let now = datetime!(2026-08-19 10:00:00 UTC);
        // A fired five seconds ago; B never fired.
        let state = vec![(
            scoped_name(&tenant_item("close-day", a, 60)),
            now - Duration::seconds(5),
        )];
        let runner = SchedulerRunner::with_state(Arc::new(reg), state);
        let fired = runner.tick(now, |_| Ok(())).unwrap();
        assert_eq!(
            fired,
            vec![format!("t:{b}:close-day")],
            "A's state must not suppress B's schedule"
        );
    }

    // -------------------------------------- pentest (Phase 5, §9 box 2)

    /// Penetration test: scope-key collisions.
    ///
    /// Runtime state — overlap locks and last-fired — is keyed by
    /// [`scoped_name`] (`t:{tenant}:{name}` for tenant items, the bare name
    /// for platform items), while registration only deduplicates on
    /// `(name, tenant)`. Those two spaces must agree: if two DIFFERENT
    /// registrations can resolve to one runtime key, one schedule would hold
    /// another's lock or suppress its firing.
    ///
    /// (1) and (2) are the tenant-reachable attacks — a tenant choosing its
    /// own schedule name. (3) is the reverse direction and is only
    /// reachable by whoever authors PLATFORM schedules.
    #[test]
    fn pentest_scheduler_crafted_names_cannot_collide_with_another_scope() {
        let a = TenantId::new();
        let b = TenantId::new();

        // (1) A names its schedule to spell out B's qualified name. The
        //     registry qualifies A's name again, so the runtime keys stay
        //     distinct and A cannot land on (or suppress) B's state.
        let crafted = tenant_item(&format!("t:{b}:close-day"), a, 60);
        assert_ne!(
            scoped_name(&crafted),
            scoped_name(&tenant_item("close-day", b, 60)),
            "a tenant-crafted name must not resolve onto another tenant's key"
        );
        assert_ne!(
            scoped_name(&crafted),
            scoped_name(&tenant_item("close-day", a, 60)),
            "nor onto its own tenant's same-named schedule"
        );

        // (2) A names its schedule exactly like a PLATFORM schedule: the
        //     tenant item is still qualified, so it never aliases the
        //     platform key and both can coexist independently.
        let platform = item("platform-daily", ScheduleSpec::Interval { secs: 60 });
        let mut reg = ScheduleRegistry::new();
        reg.register(platform).unwrap();
        reg.register(tenant_item("platform-daily", a, 60)).unwrap();
        assert_ne!(
            scoped_name(&tenant_item("platform-daily", a, 60)),
            "platform-daily",
            "a tenant item must never resolve to a platform key"
        );
        assert_eq!(reg.items_for(a).len(), 1, "both registrations survive");

        // (3) The reverse direction: a platform item whose bare name IS a
        //     tenant's qualified name would occupy that tenant's runtime key.
        //     Registration must refuse the alias rather than silently share
        //     a lock and last-fired slot with an unrelated schedule.
        reg.register(tenant_item("close-day", a, 60)).unwrap();
        let clash_name = format!("t:{a}:close-day");
        let clash = item(&clash_name, ScheduleSpec::Interval { secs: 60 });
        assert!(
            reg.register(clash).is_err(),
            "a schedule named {clash_name:?} aliases tenant {a}'s `close-day` \
             runtime key and must be refused"
        );
    }
}
