//! Factory primitives (L20): explicit, composable fixtures. Defaults always
//! build; every override is visible at the call site.

use keel_foundation::{CorrelationId, ExecutionId};
use keel_event::OutboxEvent;
use keel_jobs::{DurableJob, JobStatus};
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
