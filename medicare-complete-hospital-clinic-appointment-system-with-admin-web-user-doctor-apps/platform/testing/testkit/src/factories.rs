//! Factory primitives (L20): explicit, composable fixtures. Defaults always
//! build; every override is visible at the call site.

use keel_foundation::{AppError, CorrelationId, ExecutionId, TenantId};
use keel_event::OutboxEvent;
use keel_jobs::{DurableJob, JobStatus};
use keel_models::{ModelDef, ModelStore, Record};
use serde_json::{json, Value};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

/// Minimal factory contract: build an Item from explicit defaults.
pub trait Factory: Sized + Default {
    type Item;

    fn build(self) -> Self::Item;

    fn make() -> Self::Item {
        Self::default().build()
    }

    fn make_with(mutate: impl FnOnce(&mut Self)) -> Self::Item {
        let mut factory = Self::default();
        mutate(&mut factory);
        factory.build()
    }

    /// Wave 7 P3 (docs/19 L343): chainable state override — mutate the
    /// factory BEFORE it builds, so every override stays visible at the
    /// call site (`Factory::default().state(|f| …).build()/create()`).
    fn state(mut self, mutate: impl FnOnce(&mut Self)) -> Self {
        mutate(&mut self);
        self
    }
}

// ------------------------------------------- Wave 7 P3: model factories

/// Wave 7 P3 (docs/19 L343): a factory that PERSISTS through the
/// ModelStore. `build()` produces the item; `create()` maps it into a
/// `Record`, stamps `id` when the mapping omits it, stamps the test tenant
/// on `tenant_scoped` models, inserts the row and returns the STORED record.
pub trait ModelFactory: Factory {
    /// The model this factory creates rows for.
    fn model(&self) -> ModelDef;

    /// Item -> stored row (include a uuid `id`; `tenant_id` is stamped by
    /// `create` for `tenant_scoped` models).
    fn to_record(item: &Self::Item) -> Record;

    /// Persist one row through the ModelStore; returns the record as the
    /// store kept it.
    fn create(self, store: &dyn ModelStore, tenant: TenantId) -> Result<Record, AppError> {
        let model = self.model();
        let item = self.build();
        let mut record = Self::to_record(&item);
        if !record.contains_key("id") {
            record.insert("id".into(), json!(Uuid::now_v7().to_string()));
        }
        if model.tenant_scoped {
            record.insert("tenant_id".into(), json!(tenant.to_string()));
        }
        store.insert(&model, &record)
    }
}

/// Wave 7 P3 (docs/19 L343): deterministic sequence counter — SEEDED per
/// test (`Sequence::seeded(1)`), so labels/keys reproduce run-to-run
/// instead of leaking a process-wide counter across parallel tests.
#[derive(Debug)]
pub struct Sequence {
    next: std::sync::atomic::AtomicU64,
}

impl Sequence {
    /// Start the counter at `at` (typically 1).
    pub fn seeded(at: u64) -> Self {
        Self {
            next: std::sync::atomic::AtomicU64::new(at),
        }
    }

    /// Next value (increments).
    pub fn next(&self) -> u64 {
        self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }
}

impl Default for Sequence {
    fn default() -> Self {
        Self::seeded(1)
    }
}

// ------------------------------------------------------------------- jobs

/// DurableJob fixture; starts pending, claim-ready.
#[derive(Debug, Clone)]
pub struct JobFactory {
    pub name: String,
    pub payload: Value,
    pub queue: String,
    pub max_attempts: u32,
    pub priority: i32,
    pub available_in: Option<Duration>,
}

impl Default for JobFactory {
    fn default() -> Self {
        Self {
            name: "demo.job".into(),
            payload: Value::Null,
            queue: "default".into(),
            max_attempts: 3,
            priority: 0,
            available_in: None,
        }
    }
}

impl JobFactory {
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    pub fn payload(mut self, payload: Value) -> Self {
        self.payload = payload;
        self
    }

    pub fn queue(mut self, queue: impl Into<String>) -> Self {
        self.queue = queue.into();
        self
    }

    pub fn priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    pub fn max_attempts(mut self, max_attempts: u32) -> Self {
        self.max_attempts = max_attempts;
        self
    }

    /// Delayed job: invisible to claimers for the given duration.
    pub fn available_in(mut self, delay: Duration) -> Self {
        self.available_in = Some(delay);
        self
    }
}

impl Factory for JobFactory {
    type Item = DurableJob;

    fn build(self) -> DurableJob {
        let now = OffsetDateTime::now_utc();
        DurableJob {
            id: Uuid::now_v7(),
            execution_id: ExecutionId::new(),
            name: self.name,
            payload: self.payload,
            queue: self.queue,
            attempt: 0,
            max_attempts: self.max_attempts,
            deadline: now + Duration::minutes(5),
            available_at: now + self.available_in.unwrap_or(Duration::ZERO),
            status: JobStatus::Pending,
            priority: self.priority,
            claimed_by: None,
            claimed_at: None,
            causation_id: None,
            correlation_id: Some(CorrelationId::new()),
            last_error: None,
            enqueued_at: now,
            finished_at: None,
        }
    }
}

/// Pending job fixture with a name.
pub fn sample_job(name: &str) -> DurableJob {
    JobFactory::default()
        .name(name)
        .payload(json!({ "sample": true }))
        .build()
}

// ----------------------------------------------------------------- events

/// OutboxEvent fixture; starts unpublished.
#[derive(Debug, Clone)]
pub struct EventFactory {
    pub event: String,
    pub topic: String,
    pub payload: Value,
}

impl Default for EventFactory {
    fn default() -> Self {
        Self {
            event: "demo.happened".into(),
            topic: "test".into(),
            payload: Value::Null,
        }
    }
}

impl EventFactory {
    pub fn event(mut self, event: impl Into<String>) -> Self {
        self.event = event.into();
        self
    }

    pub fn topic(mut self, topic: impl Into<String>) -> Self {
        self.topic = topic.into();
        self
    }

    pub fn payload(mut self, payload: Value) -> Self {
        self.payload = payload;
        self
    }
}

impl Factory for EventFactory {
    type Item = OutboxEvent;

    fn build(self) -> OutboxEvent {
        OutboxEvent {
            id: Uuid::now_v7(),
            event: self.event,
            topic: self.topic,
            tenant_id: None,
            payload: self.payload,
            published: false,
            created_at: OffsetDateTime::now_utc(),
        }
    }
}

/// Unpublished event fixture with a name.
pub fn sample_event(name: &str) -> OutboxEvent {
    EventFactory::default()
        .event(name)
        .payload(json!({ "sample": true }))
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use keel_models::{InMemoryModelStore, ListOpts};

    struct Patient {
        id: Uuid,
        email: String,
    }

    #[derive(Debug, Default)]
    struct PatientFactory {
        id: Option<Uuid>,
        email: Option<String>,
        seq: Option<u64>,
    }

    impl Factory for PatientFactory {
        type Item = Patient;

        fn build(self) -> Patient {
            Patient {
                id: self.id.unwrap_or_else(Uuid::now_v7),
                email: self
                    .email
                    .unwrap_or_else(|| format!("patient-{}@clinic.test", self.seq.unwrap_or(0))),
            }
        }
    }

    impl ModelFactory for PatientFactory {
        fn model(&self) -> ModelDef {
            ModelDef::new("Patient", "patients").tenant_scoped()
        }

        fn to_record(item: &Self::Item) -> Record {
            let mut record = Record::new();
            record.insert("id".into(), json!(item.id.to_string()));
            record.insert("email".into(), json!(item.email));
            record
        }
    }

    /// Wave 7 P3 (docs/19 L343): `state(|f| …)` overrides are chainable and
    /// visible at the call site, before the build.
    #[test]
    fn state_overrides_apply_before_build() {
        let job = JobFactory::default()
            .state(|f| f.name = "orders.reconcile".into())
            .state(|f| f.max_attempts = 7)
            .build();
        assert_eq!(job.name, "orders.reconcile");
        assert_eq!(job.max_attempts, 7);

        let untouched = JobFactory::default().build();
        assert_eq!(untouched.name, "demo.job", "state is per-instance");
    }

    /// Wave 7 P3 (docs/19 L343): `create()` persists through the
    /// ModelStore, stamps the tenant on a `tenant_scoped` model, and
    /// returns the STORED record (a live row the store can list back).
    #[test]
    fn create_persists_through_the_model_store_and_returns_the_row() {
        let store = InMemoryModelStore::new();
        let tenant = TenantId::new();
        let created = PatientFactory::default()
            .state(|f| f.email = Some("ada@clinic.test".into()))
            .create(&store, tenant)
            .expect("row persists");

        assert_eq!(created["email"], json!("ada@clinic.test"));
        assert_eq!(created["tenant_id"], json!(tenant.to_string()));
        assert!(created["id"].as_str().unwrap().parse::<Uuid>().is_ok());

        // The row is LIVE under the same tenant scope the probes use.
        let page = store
            .list(
                &ModelDef::new("Patient", "patients").tenant_scoped(),
                Some(tenant),
                &[],
                &ListOpts::default(),
            )
            .expect("store reads");
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0]["email"], json!("ada@clinic.test"));
    }

    /// Wave 7 P3 (docs/19 L343): sequences are counter-SEEDED per test —
    /// a fresh `Sequence::seeded(1)` reproduces the same values run-to-run
    /// (no process-wide leak across parallel tests).
    #[test]
    fn sequences_are_counter_seeded_per_test() {
        let seq = Sequence::seeded(1);
        let store = InMemoryModelStore::new();
        let tenant = TenantId::new();

        let mut emails = Vec::new();
        for _ in 0..2 {
            let n = seq.next();
            let row = PatientFactory::default()
                .state(|f| f.seq = Some(n))
                .create(&store, tenant)
                .expect("row persists");
            emails.push(row["email"].as_str().unwrap().to_string());
        }
        assert_eq!(emails, ["patient-1@clinic.test", "patient-2@clinic.test"]);

        // A second test seeding the same counter starts from the seed again.
        let seq_again = Sequence::seeded(1);
        assert_eq!(seq_again.next(), 1);
        assert_eq!(seq_again.next(), 2);
    }
}
