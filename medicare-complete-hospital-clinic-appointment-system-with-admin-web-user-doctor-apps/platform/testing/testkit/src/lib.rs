//! # keel-testkit — testkit v1 (item 28, L58)
//!
//! The shared testing surface for Keel modules: an HTTP harness (TestApp),
//! port fakes (queue/mail/storage), deterministic time, factory primitives,
//! and assert helpers for the observable side effects of an execution:
//! deltas, audit records, outbox events and query budgets (assert_queries!).
//!
//! Layer rule (STAGE-0-SPEC §3): the Testing class may depend on platform
//! layers but never on runtime binaries — TestApp takes a router built from
//! primitives; nothing here imports keel-api or keel-worker.

mod app;
mod factories;
mod fakes;
mod queries;
mod time;

pub use app::{TestApp, TestRequest, TestResponse};
pub use factories::{sample_event, sample_job, EventFactory, Factory, JobFactory};
pub use fakes::{
    assert_delta, assert_event, AuditRecorder, CacheFake, EventFake, FakeMail, FakeQueue,
    FakeSearch, FakeStorage, MailFake, MailMessage, NotificationFake, QueueFake, StorageFake,
};
pub use keel_search::{
    HybridQuery, Index, SearchDocument, SearchEngine, SearchQuery, SearchResult, VectorQuery,
};
pub use time::{advance, freeze_time, travel_to};

// Convenience re-exports so tests get one import root.
pub use keel_caching::CachePort;
pub use keel_event::{InMemoryOutbox, OutboxEvent};
pub use keel_execution::{AuditRecord, InMemoryBackend, QueryTap};
pub use keel_http_client::{HttpClientPort, HttpFake, HttpRequest, HttpResponse};
pub use keel_jobs::{DurableJob, InMemoryJobStore, JobStatus};
pub use keel_notifications::{
    Channel, DispatchRecord, MailPort, Mailable, NotificationContext, NotificationDef, NotifyHub,
    NotifyMessage,
};
pub use keel_storage::{ObjectRef, StoragePort};
