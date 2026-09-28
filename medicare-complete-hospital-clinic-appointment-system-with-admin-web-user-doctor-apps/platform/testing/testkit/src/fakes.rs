//! Port fakes: in-memory doubles for the async/storage/notify surfaces plus
//! an audit recorder. Each fake also carries its own assert helpers.

use std::collections::HashMap;
use std::sync::Mutex;

use keel_foundation::{AppError, TenantId};
use keel_event::{InMemoryOutbox, OutboxEvent};
use keel_execution::{AuditOutcome, AuditRecord};
use keel_jobs::{DurableJob, InMemoryJobStore, JobStore, JobStoreAdmin};
use keel_search::{
    InMemorySearchEngine, Index, SearchDocument, SearchEngine, SearchQuery, SearchResult,
};
use keel_storage::{ObjectRef, StoragePort};
use uuid::Uuid;

// ----------------------------------------------------------------- queue

/// Fake durable queue backed by the degraded-boot store (same claim/fail
/// semantics as production, no persistence).
#[derive(Debug, Default)]
pub struct FakeQueue {
    store: InMemoryJobStore,
}

impl FakeQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// Underlying store for wiring into engines/handlers.
    pub fn store(&self) -> &InMemoryJobStore {
        &self.store
    }

    pub fn enqueue(&self, job: DurableJob) -> Uuid {
        self.store.enqueue(job).expect("enqueue succeeds")
    }

    pub fn jobs(&self) -> Vec<DurableJob> {
        self.store.list(None, 1000).expect("list succeeds")
    }

    pub fn jobs_named(&self, name: &str) -> Vec<DurableJob> {
        self.jobs().into_iter().filter(|j| j.name == name).collect()
    }

    pub fn count(&self, name: &str) -> usize {
        self.jobs_named(name).len()
    }

    /// First job with the given name; panics with visible queue state.
    pub fn assert_enqueued(&self, name: &str) -> DurableJob {
        self.jobs_named(name).into_iter().next().unwrap_or_else(|| {
            panic!(
                "expected job {name:?} in queue; have: {:?}",
                self.jobs()
                    .iter()
                    .map(|j| j.name.clone())
                    .collect::<Vec<_>>()
            )
        })
    }

    pub fn assert_empty(&self) {
        assert!(self.jobs().is_empty(), "expected empty queue");
    }
}

// ----------------------------------------------------------------- events

/// First outbox event with the given name; panics listing what exists.
pub fn assert_event(outbox: &InMemoryOutbox, name: &str) -> OutboxEvent {
    outbox.find(name).unwrap_or_else(|| {
        panic!(
            "expected outbox event {name:?}; have: {:?}",
            outbox
                .events()
                .iter()
                .map(|e| e.event.clone())
                .collect::<Vec<_>>()
        )
    })
}

// ------------------------------------------------------------------ mail

/// One recorded outbound message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailMessage {
    pub to: String,
    pub subject: String,
    pub body: String,
}

/// Fake mailer: records everything, sends nothing.
#[derive(Debug, Default)]
pub struct FakeMail {
    messages: Mutex<Vec<MailMessage>>,
}

impl FakeMail {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn send(&self, to: impl Into<String>, subject: impl Into<String>, body: impl Into<String>) {
        self.messages.lock().unwrap().push(MailMessage {
            to: to.into(),
            subject: subject.into(),
            body: body.into(),
        });
    }

    pub fn messages(&self) -> Vec<MailMessage> {
        self.messages.lock().unwrap().clone()
    }

    pub fn count(&self) -> usize {
        self.messages.lock().unwrap().len()
    }

    /// First message to the given address; panics listing sent mail.
    pub fn assert_sent_to(&self, to: &str) -> MailMessage {
        self.messages()
            .into_iter()
            .find(|m| m.to == to)
            .unwrap_or_else(|| panic!("no mail to {to:?}; sent: {:?}", self.messages()))
    }

    pub fn assert_count(&self, expected: usize) {
        assert_eq!(self.count(), expected, "mail count mismatch");
    }
}

// --------------------------------------------------------------- storage

/// Fake object storage implementing the platform StoragePort.
#[derive(Debug, Default)]
pub struct FakeStorage {
    objects: Mutex<HashMap<String, Vec<u8>>>,
}

impl FakeStorage {
    pub fn new() -> Self {
        Self::default()
    }

    fn map_key(bucket: &str, key: &str) -> String {
        format!("{bucket}/{key}")
    }

    pub fn contains(&self, bucket: &str, key: &str) -> bool {
        self.objects
            .lock()
            .unwrap()
            .contains_key(&Self::map_key(bucket, key))
    }

    pub fn object_count(&self) -> usize {
        self.objects.lock().unwrap().len()
    }
}

impl StoragePort for FakeStorage {
    fn put(&self, bucket: &str, key: &str, bytes: Vec<u8>) -> Result<ObjectRef, AppError> {
        self.objects
            .lock()
            .unwrap()
            .insert(Self::map_key(bucket, key), bytes);
        Ok(ObjectRef {
            bucket: bucket.to_string(),
            key: key.to_string(),
        })
    }

    fn get(&self, obj: &ObjectRef) -> Result<Vec<u8>, AppError> {
        self.objects
            .lock()
            .unwrap()
            .get(&Self::map_key(&obj.bucket, &obj.key))
            .cloned()
            .ok_or_else(|| AppError::not_found(format!("{}/{}", obj.bucket, obj.key)))
    }

    fn delete(&self, obj: &ObjectRef) -> Result<(), AppError> {
        self.objects
            .lock()
            .unwrap()
            .remove(&Self::map_key(&obj.bucket, &obj.key))
            .map(|_| ())
            .ok_or_else(|| AppError::not_found(format!("{}/{}", obj.bucket, obj.key)))
    }

    fn presigned_url(&self, obj: &ObjectRef, ttl_secs: u64) -> Result<String, AppError> {
        Ok(format!("fake://{}/{}?ttl={ttl_secs}", obj.bucket, obj.key))
    }
}

// ----------------------------------------------------------------- audit

/// In-memory audit sink: handlers (or test wiring) record here; assertions
/// read back. Pairs with InMemoryBackend for delta assertions.
#[derive(Debug, Default)]
pub struct AuditRecorder {
    records: Mutex<Vec<AuditRecord>>,
}

impl AuditRecorder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&self, record: AuditRecord) {
        self.records.lock().unwrap().push(record);
    }

    pub fn record_success(&self, action: impl Into<String>, resource: impl Into<String>) {
        self.record(AuditRecord {
            action: action.into(),
            resource: resource.into(),
            authority: "testkit".into(),
            outcome: AuditOutcome::Success,
            detail: serde_json::Value::Null,
        });
    }

    pub fn records(&self) -> Vec<AuditRecord> {
        self.records.lock().unwrap().clone()
    }

    /// First record with the given action; panics listing recorded actions.
    pub fn assert_audit(&self, action: &str) -> AuditRecord {
        self.records()
            .into_iter()
            .find(|r| r.action == action)
            .unwrap_or_else(|| {
                panic!(
                    "expected audit action {action:?}; have: {:?}",
                    self.records()
                        .iter()
                        .map(|r| r.action.clone())
                        .collect::<Vec<_>>()
                )
            })
    }
}

/// Delta assertion against the in-memory execution backend: all stones for a
/// resource type.
pub fn assert_delta(
    backend: &keel_execution::InMemoryBackend,
    resource_type: &str,
) -> Vec<keel_foundation::DeltaStone> {
    let stones: Vec<keel_foundation::DeltaStone> = backend
        .stones()
        .into_iter()
        .filter(|s| s.resource_type == resource_type)
        .collect();
    assert!(
        !stones.is_empty(),
        "expected deltas for {resource_type:?}; have: {:?}",
        backend
            .stones()
            .iter()
            .map(|s| s.resource_type.clone())
            .collect::<Vec<_>>()
    );
    stones
}

// ---------------------------------------------------------------- search

/// Fake search engine backed by the in-memory reference implementation
/// (item 40): same keyword/vector/hybrid semantics as the production port,
/// no external service, plus test asserts.
#[derive(Debug, Default)]
pub struct FakeSearch {
    engine: InMemorySearchEngine,
}

impl FakeSearch {
    pub fn new() -> Self {
        Self::default()
    }

    /// Underlying engine, for wiring into handlers under test.
    pub fn engine(&self) -> &InMemorySearchEngine {
        &self.engine
    }

    /// Index a document (panics on invalid docs — test-only convenience).
    pub fn put(&self, index: &str, doc: SearchDocument) {
        self.engine
            .index(&Index(index.to_string()), doc)
            .expect("fake search indexing succeeds");
    }

    pub fn search(&self, index: &str, query: &SearchQuery) -> SearchResult {
        self.engine
            .search(&Index(index.to_string()), query)
            .expect("fake search succeeds")
    }

    pub fn vector_search(&self, index: &str, query: &keel_search::VectorQuery) -> SearchResult {
        self.engine
            .vector_search(&Index(index.to_string()), query)
            .expect("fake vector search succeeds")
    }

    pub fn hybrid_search(&self, index: &str, query: &keel_search::HybridQuery) -> SearchResult {
        self.engine
            .hybrid_search(&Index(index.to_string()), query)
            .expect("fake hybrid search succeeds")
    }

    pub fn count(&self, index: &str, tenant: TenantId) -> usize {
        self.engine
            .count(&Index(index.to_string()), tenant)
            .expect("fake count succeeds")
    }

    /// Assert a document is indexed for the tenant (tenant isolation aware).
    pub fn assert_indexed(&self, index: &str, tenant: TenantId, id: &str) {
        let result = self.search(
            index,
            &SearchQuery {
                tenant,
                limit: 10_000,
                ..Default::default()
            },
        );
        assert!(
            result.hits.iter().any(|hit| hit.id == id),
            "expected document {id:?} indexed in {index:?}; have: {:?}",
            result
                .hits
                .iter()
                .map(|hit| hit.id.clone())
                .collect::<Vec<_>>()
        );
    }

    /// Assert the exact hit-id ordering for a query.
    pub fn assert_hits(&self, index: &str, query: &SearchQuery, ids: &[&str]) {
        let result = self.search(index, query);
        let got: Vec<&str> = result.hits.iter().map(|hit| hit.id.as_str()).collect();
        assert_eq!(got, ids, "search hit ordering mismatch on {index:?}");
    }

    pub fn assert_no_hits(&self, index: &str, query: &SearchQuery) {
        let result = self.search(index, query);
        assert!(
            result.hits.is_empty(),
            "expected no hits on {index:?}; got: {:?}",
            result
                .hits
                .iter()
                .map(|hit| hit.id.clone())
                .collect::<Vec<_>>()
        );
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use keel_foundation::TenantId;

    #[test]
    fn fake_search_isolates_tenants_and_asserts() {
        let fake = FakeSearch::new();
        let ours = TenantId::new();
        let theirs = TenantId::new();
        fake.put(
            "invoices",
            SearchDocument {
                id: "1".into(),
                tenant: ours,
                text: "acme hospital invoice".into(),
                fields: [("status".to_string(), "posted".to_string())]
                    .into_iter()
                    .collect(),
                vector: Vec::new(),
            },
        );
        fake.put(
            "invoices",
            SearchDocument {
                id: "2".into(),
                tenant: theirs,
                text: "acme hospital invoice".into(),
                fields: Default::default(),
                vector: Vec::new(),
            },
        );

        fake.assert_indexed("invoices", ours, "1");
        fake.assert_hits(
            "invoices",
            &SearchQuery {
                tenant: ours,
                text: "acme".into(),
                limit: 10,
                ..Default::default()
            },
            &["1"],
        );
        fake.assert_no_hits(
            "invoices",
            &SearchQuery {
                tenant: ours,
                text: "acme".into(),
                filters: [("status".into(), "draft".into())].into_iter().collect(),
                limit: 10,
                ..Default::default()
            },
        );
        assert_eq!(fake.count("invoices", ours), 1);
        assert_eq!(fake.count("invoices", theirs), 1);
    }
}
