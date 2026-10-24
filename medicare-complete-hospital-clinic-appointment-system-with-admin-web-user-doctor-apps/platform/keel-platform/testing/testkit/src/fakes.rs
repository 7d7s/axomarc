//! Port fakes: in-memory doubles for the async/storage/notify surfaces plus
//! an audit recorder. Each fake also carries its own assert helpers.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use kelson_caching::CachePort;
use kelson_event::{InMemoryOutbox, OutboxEvent};
use kelson_execution::{AuditOutcome, AuditRecord};
use kelson_foundation::{AppError, Clock, FixedClock, PrincipalId, TenantId};
use kelson_jobs::{DurableJob, InMemoryJobStore, JobStore, JobStoreAdmin};
use kelson_notifications::{
    Channel, DispatchRecord, DispatchTap, MailPort, Mailable, NotifyMessage, ResolvedMailAttachment,
};
use kelson_search::{
    InMemorySearchEngine, Index, SearchDocument, SearchEngine, SearchQuery, SearchResult,
};
use kelson_storage::{ObjectRef, StoragePort};
use time::OffsetDateTime;
use uuid::Uuid;

// ----------------------------------------------------------------- queue

/// Fake durable queue backed by the degraded-boot store (same claim/fail
/// semantics as production, no persistence).
#[derive(Debug, Default)]
pub struct QueueFake {
    store: InMemoryJobStore,
}

impl QueueFake {
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

    /// Wave 7 P1 (19-PARITY `assert_pushed(job_type, |j| ...)`): the first
    /// pushed job of `job_type` satisfying `matches`, panicking with the
    /// visible queue state. `job_type` is the job's name
    /// (`DurableJob::name`).
    pub fn assert_pushed(
        &self,
        job_type: &str,
        matches: impl Fn(&DurableJob) -> bool,
    ) -> DurableJob {
        self.jobs_named(job_type)
            .into_iter()
            .find(matches)
            .unwrap_or_else(|| {
                panic!(
                    "expected pushed job {job_type:?} matching the predicate; have: {:?}",
                    self.job_names()
                )
            })
    }

    /// Wave 7 P1: assert no job of `job_type` was pushed.
    pub fn assert_not_pushed(&self, job_type: &str) {
        assert_eq!(
            self.count(job_type),
            0,
            "expected no pushed job {job_type:?}; have: {:?}",
            self.job_names()
        );
    }

    /// Wave 7 P1: assert the exact number of pushed jobs of `job_type`.
    pub fn assert_pushed_count(&self, job_type: &str, expected: usize) {
        assert_eq!(
            self.count(job_type),
            expected,
            "pushed count mismatch for {job_type:?}; have: {:?}",
            self.job_names()
        );
    }

    fn job_names(&self) -> Vec<String> {
        self.jobs().iter().map(|j| j.name.clone()).collect()
    }
}

/// Legacy alias — the Wave 7 spec name is [`QueueFake`].
pub type FakeQueue = QueueFake;

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

/// Wave 7 P1 (19-PARITY `EventFake`): outbox recorder with assertions.
/// Share [`EventFake::outbox`] with the publishers under test — they append
/// into the same buffer the asserts read.
#[derive(Debug, Clone, Default)]
pub struct EventFake {
    outbox: Arc<InMemoryOutbox>,
}

impl EventFake {
    pub fn new() -> Self {
        Self::default()
    }

    /// Shared outbox for wiring into the publishers/handlers under test.
    pub fn outbox(&self) -> Arc<InMemoryOutbox> {
        Arc::clone(&self.outbox)
    }

    /// Record an emission directly (tests that seed without the publisher).
    pub fn dispatch(&self, event: OutboxEvent) {
        self.outbox.push(event);
    }

    /// Every recorded emission, in order.
    pub fn events(&self) -> Vec<OutboxEvent> {
        self.outbox.events()
    }

    /// Wave 7 P1 (19-PARITY `assert_dispatched(event_name, |e| ...)`): the
    /// first event named `event_name` satisfying `matches`; panics listing
    /// what was emitted.
    pub fn assert_dispatched(
        &self,
        event_name: &str,
        matches: impl Fn(&OutboxEvent) -> bool,
    ) -> OutboxEvent {
        self.outbox
            .events()
            .into_iter()
            .filter(|e| e.event == event_name)
            .find(matches)
            .unwrap_or_else(|| {
                panic!(
                    "expected dispatched event {event_name:?} matching the predicate; have: {:?}",
                    self.names()
                )
            })
    }

    /// Wave 7 P1: assert no event named `event_name` was emitted.
    pub fn assert_not_dispatched(&self, event_name: &str) {
        assert!(
            !self.outbox.events().iter().any(|e| e.event == event_name),
            "expected no dispatch of {event_name:?}; have: {:?}",
            self.names()
        );
    }

    fn names(&self) -> Vec<String> {
        self.outbox
            .events()
            .iter()
            .map(|e| e.event.clone())
            .collect()
    }
}

// ------------------------------------------------------------------ mail

/// One recorded outbound message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailMessage {
    pub to: String,
    pub subject: String,
    pub body: String,
}

/// Fake mailer: records everything, sends nothing. Wave 7 P1 (19-PARITY
/// `MailFake`): implements the notifications [`MailPort`], so a hub wired
/// with it captures rich [`Mailable`]s for [`MailFake::assert_sent`] and
/// mirrors them into the message log so the message asserts cover both
/// mail paths. Cheap to clone — clones share the recording.
#[derive(Debug, Default, Clone)]
pub struct MailFake {
    messages: Arc<Mutex<Vec<MailMessage>>>,
    mailables: Arc<Mutex<Vec<Mailable>>>,
}

impl MailFake {
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

    /// Every recorded rich [`Mailable`] (the `MailPort::send_mailable`
    /// path), in order.
    pub fn mailables(&self) -> Vec<Mailable> {
        self.mailables.lock().unwrap().clone()
    }

    /// Wave 7 P1 (19-PARITY `assert_sent(|m| ...)`): the first recorded
    /// [`Mailable`] satisfying `matches` — match on subject / `to` /
    /// `attachments.len()` inside the closure; panics listing what was
    /// sent.
    pub fn assert_sent(&self, matches: impl Fn(&Mailable) -> bool) -> Mailable {
        self.mailables()
            .into_iter()
            .find(matches)
            .unwrap_or_else(|| {
                panic!(
                    "no sent mailable matched the predicate; subjects: {:?}",
                    self.mailables()
                        .iter()
                        .map(|m| m.subject.clone())
                        .collect::<Vec<_>>()
                )
            })
    }
}

impl MailPort for MailFake {
    fn send(&self, msg: &NotifyMessage, subject: &str) -> Result<(), AppError> {
        // Template path: the transport renders `template` with `vars`;
        // record the raw template key as the body.
        self.send(msg.to.clone(), subject, msg.template.clone());
        Ok(())
    }

    fn send_mailable(
        &self,
        mail: &Mailable,
        _attachments: &[ResolvedMailAttachment],
    ) -> Result<(), AppError> {
        // Transports validate before touching the network — the fake keeps
        // the same fail-closed posture and records nothing on rejection.
        mail.validate()?;
        self.mailables.lock().unwrap().push(mail.clone());
        if let Some(first) = mail.to.first() {
            self.messages.lock().unwrap().push(MailMessage {
                to: first.clone(),
                subject: mail.subject.clone(),
                body: mail
                    .text
                    .clone()
                    .or_else(|| mail.html.clone())
                    .unwrap_or_default(),
            });
        }
        Ok(())
    }
}

/// Legacy alias — the Wave 7 spec name is [`MailFake`].
pub type FakeMail = MailFake;

// --------------------------------------------------------------- storage

/// Fake object storage implementing the platform StoragePort.
#[derive(Debug, Default)]
pub struct StorageFake {
    objects: Mutex<HashMap<String, Vec<u8>>>,
}

impl StorageFake {
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

    /// Wave 7 P1 (19-PARITY `assert_exists(path)`): an object exists at
    /// `path` — the S3-style `bucket/key` the port addresses by; panics
    /// listing the stored keys.
    pub fn assert_exists(&self, path: &str) {
        assert!(
            self.objects.lock().unwrap().contains_key(path),
            "expected object at {path:?}; have: {:?}",
            self.keys()
        );
    }

    /// Wave 7 P1: assert no object at `path` (`bucket/key`).
    pub fn assert_missing(&self, path: &str) {
        let objects = self.objects.lock().unwrap();
        assert!(
            !objects.contains_key(path),
            "expected no object at {path:?}; found it ({} bytes)",
            objects.get(path).map(Vec::len).unwrap_or(0)
        );
    }

    /// Wave 7 P1: assert no key falls under the directory-style `prefix`.
    pub fn assert_directory_empty(&self, prefix: &str) {
        let under: Vec<String> = self
            .objects
            .lock()
            .unwrap()
            .keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect();
        assert!(
            under.is_empty(),
            "expected no objects under {prefix:?}; have: {under:?}"
        );
    }

    fn keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.objects.lock().unwrap().keys().cloned().collect();
        keys.sort();
        keys
    }
}

impl StoragePort for StorageFake {
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

/// Legacy alias — the Wave 7 spec name is [`StorageFake`].
pub type FakeStorage = StorageFake;

// ---------------------------------------------------------- notifications

/// Wave 7 P1 (19-PARITY `NotificationFake`): records what the notification
/// hub actually dispatched — wire with `NotifyHub::with_tap(Arc::new(fake))`
/// (clones share the recording) and assert with
/// [`NotificationFake::assert_sent_to`].
#[derive(Debug, Default, Clone)]
pub struct NotificationFake {
    sent: Arc<Mutex<Vec<DispatchRecord>>>,
}

impl NotificationFake {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every recorded dispatch, in order (`principal` is `None` for
    /// on-demand dispatches).
    pub fn records(&self) -> Vec<DispatchRecord> {
        self.sent.lock().unwrap().clone()
    }

    /// Wave 7 P1 (19-PARITY `assert_sent_to(principal, notification_type,
    /// channel)`): the hub dispatched `notification_type` to `principal` on
    /// `channel`; panics listing what it recorded.
    pub fn assert_sent_to(
        &self,
        principal: PrincipalId,
        notification_type: &str,
        channel: Channel,
    ) {
        let sent = self.records();
        assert!(
            sent.iter().any(|r| {
                r.principal == Some(principal)
                    && r.notification_type == notification_type
                    && r.channel == channel
            }),
            "expected {notification_type:?} to {principal:?} on {channel:?}; recorded: {:?}",
            sent.iter()
                .map(|r| (r.principal, r.notification_type.clone(), r.channel))
                .collect::<Vec<_>>()
        );
    }
}

impl DispatchTap for NotificationFake {
    fn on_dispatch(&self, record: &DispatchRecord) {
        self.sent.lock().unwrap().push(record.clone());
    }
}

// ------------------------------------------------------------------ cache

/// Wave 7 P1 (19-PARITY `CacheFake`): in-memory [`CachePort`] with the
/// production adapter's expiry posture (lazy read-side invalidation; a
/// `ttl = 0` entry is immediately expired) plus test assertions. TTL reads
/// the clock: the injected testkit `FixedClock` (build with
/// [`CacheFake::with_clock`] so `advance`/`travel_to` drives expiry) or
/// wall time otherwise.
#[derive(Debug, Clone, Default)]
pub struct CacheFake {
    entries: Arc<Mutex<HashMap<String, CacheEntry>>>,
    clock: Option<Arc<FixedClock>>,
}

#[derive(Debug, Clone)]
struct CacheEntry {
    value: Vec<u8>,
    ttl_secs: Option<u64>,
    expires_at: Option<OffsetDateTime>,
}

impl CacheEntry {
    fn is_expired(&self, now: OffsetDateTime) -> bool {
        self.expires_at.map(|t| now >= t).unwrap_or(false)
    }
}

impl CacheFake {
    pub fn new() -> Self {
        Self::default()
    }

    /// Wire a deterministic clock (the `freeze_time` handle) so TTL expiry
    /// follows `advance`/`travel_to` instead of the wall clock.
    pub fn with_clock(clock: Arc<FixedClock>) -> Self {
        Self {
            entries: Default::default(),
            clock: Some(clock),
        }
    }

    fn now(&self) -> OffsetDateTime {
        self.clock
            .as_ref()
            .map(|c| c.now())
            .unwrap_or_else(OffsetDateTime::now_utc)
    }

    /// Live value at `key`, dropping the entry when its TTL passed (the
    /// adapter's lazy read-side invalidation).
    fn live(&self, key: &str, now: OffsetDateTime) -> Option<Vec<u8>> {
        let mut entries = self.entries.lock().unwrap();
        let expired = entries.get(key).map(|e| e.is_expired(now)).unwrap_or(false);
        if expired {
            entries.remove(key);
            return None;
        }
        entries.get(key).map(|e| e.value.clone())
    }

    fn insert(&self, key: &str, value: Vec<u8>, ttl_secs: Option<u64>) {
        let expires_at = ttl_secs
            .map(|s| self.now() + time::Duration::seconds(i64::try_from(s).unwrap_or(i64::MAX)));
        self.entries.lock().unwrap().insert(
            key.to_string(),
            CacheEntry {
                value,
                ttl_secs,
                expires_at,
            },
        );
    }

    /// Wave 7 P1 (19-PARITY `assert_has(key)`): a LIVE entry exists at
    /// `key` (an expired entry counts as missing); panics listing keys.
    pub fn assert_has(&self, key: &str) {
        assert!(
            self.live(key, self.now()).is_some(),
            "expected live cache entry at {key:?}; have: {:?}",
            self.keys()
        );
    }

    /// Wave 7 P1: assert no live entry at `key`.
    pub fn assert_missing(&self, key: &str) {
        assert!(
            self.live(key, self.now()).is_none(),
            "expected no live cache entry at {key:?}; it is present"
        );
    }

    /// Wave 7 P1 TTL inspection: the configured TTL (seconds) of a LIVE
    /// entry — `None` when the key is absent or was stored without one.
    pub fn ttl_secs(&self, key: &str) -> Option<u64> {
        let now = self.now();
        let mut entries = self.entries.lock().unwrap();
        let expired = entries.get(key).map(|e| e.is_expired(now)).unwrap_or(false);
        if expired {
            entries.remove(key);
            return None;
        }
        entries.get(key).and_then(|e| e.ttl_secs)
    }

    fn keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.entries.lock().unwrap().keys().cloned().collect();
        keys.sort();
        keys
    }
}

impl CachePort for CacheFake {
    fn get(&self, key: &str) -> Option<Vec<u8>> {
        self.live(key, self.now())
    }

    fn put(&self, key: &str, value: Vec<u8>, ttl_secs: Option<u64>) {
        self.insert(key, value, ttl_secs);
    }

    fn delete(&self, key: &str) {
        self.entries.lock().unwrap().remove(key);
    }

    fn put_if_absent(&self, key: &str, value: Vec<u8>, ttl_secs: Option<u64>) -> bool {
        if self.live(key, self.now()).is_some() {
            return false;
        }
        self.insert(key, value, ttl_secs);
        true
    }

    fn delete_if_equals(&self, key: &str, expected: &[u8]) -> bool {
        match self.live(key, self.now()) {
            Some(current) if current == expected => {
                self.delete(key);
                true
            }
            _ => false,
        }
    }

    fn invalidate_prefix(&self, prefix: &str) {
        let mut entries = self.entries.lock().unwrap();
        entries.retain(|k, _| !k.starts_with(prefix));
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
    backend: &kelson_execution::InMemoryBackend,
    resource_type: &str,
) -> Vec<kelson_foundation::DeltaStone> {
    let stones: Vec<kelson_foundation::DeltaStone> = backend
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

    pub fn vector_search(&self, index: &str, query: &kelson_search::VectorQuery) -> SearchResult {
        self.engine
            .vector_search(&Index(index.to_string()), query)
            .expect("fake vector search succeeds")
    }

    pub fn hybrid_search(&self, index: &str, query: &kelson_search::HybridQuery) -> SearchResult {
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
    use kelson_foundation::TenantId;
    use kelson_notifications::{MailAttachment, NotificationContext, NotificationDef, NotifyHub};
    use serde_json::json;
    use time::Duration;

    use crate::{advance, freeze_time, sample_event, sample_job};

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

    // ------------------------------------------------- Wave 7 P1: fakes

    /// A sendable rich mail with one attachment — for direct
    /// `send_mailable` calls (the hub's dispatch path refuses attachments
    /// by design, so [`TestNote`] uses [`plain_mailable`]).
    fn welcome_mailable() -> Mailable {
        Mailable {
            to: vec!["ada@example.com".into()],
            cc: vec![],
            bcc: vec![],
            from: None,
            reply_to: None,
            subject: "Welcome to Kelson".into(),
            text: Some("hello".into()),
            html: None,
            headers: vec![],
            attachments: vec![MailAttachment {
                filename: "guide.pdf".into(),
                content_type: "application/pdf".into(),
                object: ObjectRef {
                    bucket: "files".into(),
                    key: "guide.pdf".into(),
                },
            }],
        }
    }

    /// Attachment-free def mail: the dispatcher fills the recipient from
    /// the delivery target.
    fn plain_mailable() -> Mailable {
        Mailable {
            to: vec![],
            cc: vec![],
            bcc: vec![],
            from: None,
            reply_to: None,
            subject: "Welcome to Kelson".into(),
            text: Some("hello".into()),
            html: None,
            headers: vec![],
            attachments: vec![],
        }
    }

    struct TestNote;

    impl NotificationDef for TestNote {
        fn type_key(&self) -> &str {
            "testkit.note"
        }

        fn to_mail(&self) -> Mailable {
            plain_mailable()
        }

        fn to_sms(&self) -> String {
            "ping".into()
        }

        fn to_push(&self) -> serde_json::Value {
            json!({ "signal": "testkit.note" })
        }

        fn to_database(&self) -> serde_json::Value {
            json!({})
        }
    }

    #[test]
    fn queue_fake_asserts_pushed_jobs() {
        let queue = QueueFake::new();
        queue.enqueue(sample_job("mail.send"));
        queue.enqueue(sample_job("mail.send"));
        queue.enqueue(sample_job("report.build"));

        let pushed = queue.assert_pushed("mail.send", |j| j.payload["sample"] == json!(true));
        assert_eq!(pushed.name, "mail.send");
        queue.assert_pushed_count("mail.send", 2);
        queue.assert_not_pushed("sms.send");
    }

    #[test]
    #[should_panic(expected = "expected pushed job")]
    fn queue_fake_assert_pushed_panics_without_match() {
        let queue = QueueFake::new();
        queue.enqueue(sample_job("mail.send"));
        queue.assert_pushed("mail.send", |_| false);
    }

    #[test]
    #[should_panic(expected = "expected no pushed job")]
    fn queue_fake_assert_not_pushed_panics_when_present() {
        let queue = QueueFake::new();
        queue.enqueue(sample_job("mail.send"));
        queue.assert_not_pushed("mail.send");
    }

    #[test]
    #[should_panic(expected = "pushed count mismatch")]
    fn queue_fake_assert_pushed_count_panics_on_mismatch() {
        let queue = QueueFake::new();
        queue.enqueue(sample_job("mail.send"));
        queue.assert_pushed_count("mail.send", 2);
    }

    #[test]
    fn event_fake_records_and_asserts() {
        let fake = EventFake::new();
        fake.dispatch(sample_event("tenant.created"));
        // Publishers wired to the shared outbox append into the same buffer.
        fake.outbox().push(sample_event("invoice.posted"));

        fake.assert_dispatched("tenant.created", |e| e.topic == "test");
        fake.assert_dispatched("invoice.posted", |_| true);
        fake.assert_not_dispatched("payment.captured");
        assert_eq!(fake.events().len(), 2);
    }

    #[test]
    #[should_panic(expected = "expected dispatched event")]
    fn event_fake_assert_dispatched_panics_when_absent() {
        let fake = EventFake::new();
        fake.dispatch(sample_event("tenant.created"));
        fake.assert_dispatched("tenant.created", |_| false);
    }

    #[test]
    #[should_panic(expected = "expected no dispatch of")]
    fn event_fake_assert_not_dispatched_panics_when_present() {
        let fake = EventFake::new();
        fake.dispatch(sample_event("tenant.created"));
        fake.assert_not_dispatched("tenant.created");
    }

    #[test]
    fn mail_fake_records_mailables_and_messages() {
        let fake = MailFake::new();
        fake.send_mailable(&welcome_mailable(), &[])
            .expect("valid mailable sends");

        let sent = fake.assert_sent(|m| {
            m.subject == "Welcome to Kelson"
                && m.to.len() == 1
                && m.to[0] == "ada@example.com"
                && m.attachments.len() == 1
        });
        assert_eq!(sent.text.as_deref(), Some("hello"));
        assert_eq!(fake.mailables().len(), 1);
        // The mailable is mirrored into the message log.
        fake.assert_count(1);
        fake.assert_sent_to("ada@example.com");

        // The template path records a plain message too (UFCS: the
        // inherent `send` shares the method name).
        let msg = NotifyMessage {
            channel: Channel::Email,
            to: "ops@example.com".into(),
            template: "ops.ping".into(),
            vars: Default::default(),
            tenant_id: None,
            locale: None,
        };
        MailPort::send(&fake, &msg, "Ops ping").expect("template send succeeds");
        fake.assert_count(2);
        assert_eq!(fake.assert_sent_to("ops@example.com").body, "ops.ping");
    }

    #[test]
    fn mail_fake_send_mailable_rejects_invalid_mailable() {
        let fake = MailFake::new();
        let mut invalid = welcome_mailable();
        invalid.to.clear();
        assert!(fake.send_mailable(&invalid, &[]).is_err());
        assert!(fake.mailables().is_empty());
        assert_eq!(fake.count(), 0);
    }

    #[test]
    #[should_panic(expected = "no sent mailable matched")]
    fn mail_fake_assert_sent_panics_without_match() {
        let fake = MailFake::new();
        fake.send_mailable(&welcome_mailable(), &[])
            .expect("valid mailable sends");
        fake.assert_sent(|m| m.subject == "Something else");
    }

    #[test]
    fn storage_fake_path_asserts() {
        let storage = StorageFake::new();
        storage.put("files", "a.txt", vec![1]).unwrap();
        storage.put("files", "b.txt", vec![2]).unwrap();

        storage.assert_exists("files/a.txt");
        storage.assert_directory_empty("reports/");

        storage
            .delete(&ObjectRef {
                bucket: "files".into(),
                key: "b.txt".into(),
            })
            .unwrap();
        storage.assert_missing("files/b.txt");

        storage
            .delete(&ObjectRef {
                bucket: "files".into(),
                key: "a.txt".into(),
            })
            .unwrap();
        storage.assert_directory_empty("files/");
    }

    #[test]
    #[should_panic(expected = "expected object")]
    fn storage_fake_assert_exists_panics_when_absent() {
        StorageFake::new().assert_exists("files/a.txt");
    }

    #[test]
    #[should_panic(expected = "expected no objects under")]
    fn storage_fake_assert_directory_empty_panics_with_contents() {
        let storage = StorageFake::new();
        storage.put("files", "a.txt", vec![1]).unwrap();
        storage.assert_directory_empty("files/");
    }

    #[test]
    fn cache_fake_implements_port_with_asserts() {
        let cache = CacheFake::new();
        cache.put("query:products", b"[]".to_vec(), None);
        assert_eq!(cache.get("query:products"), Some(b"[]".to_vec()));
        cache.assert_has("query:products");
        assert_eq!(
            cache.ttl_secs("query:products"),
            None,
            "stored without a TTL"
        );

        cache.put("authz:user", b"token".to_vec(), Some(60));
        assert_eq!(cache.ttl_secs("authz:user"), Some(60));
        assert!(
            !cache.put_if_absent("authz:user", b"other".to_vec(), None),
            "a live entry blocks put_if_absent"
        );

        assert!(cache.put_if_absent("lock:job", b"a".to_vec(), None));
        assert!(!cache.put_if_absent("lock:job", b"b".to_vec(), None));
        assert!(!cache.delete_if_equals("lock:job", b"b"), "wrong token");
        assert!(cache.delete_if_equals("lock:job", b"a"));

        cache.put("authz:other", b"t".to_vec(), None);
        cache.invalidate_prefix("authz:");
        cache.assert_missing("authz:user");
        cache.assert_missing("authz:other");
        cache.assert_has("query:products");

        cache.delete("query:products");
        cache.assert_missing("query:products");
        assert_eq!(cache.get("query:products"), None);
    }

    #[test]
    fn cache_fake_ttl_expires_under_fixed_clock() {
        let clock = freeze_time(OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap());
        let cache = CacheFake::with_clock(clock.clone());

        cache.put("session", b"v".to_vec(), Some(60));
        cache.assert_has("session");
        assert_eq!(cache.ttl_secs("session"), Some(60));

        // `ttl = 0` is immediately expired (the production posture).
        cache.put("zero", b"v".to_vec(), Some(0));
        cache.assert_missing("zero");
        assert_eq!(cache.get("zero"), None);

        advance(&clock, Duration::seconds(61));
        cache.assert_missing("session");
        assert_eq!(cache.get("session"), None);
        assert_eq!(cache.ttl_secs("session"), None);
    }

    #[test]
    #[should_panic(expected = "expected live cache entry")]
    fn cache_fake_assert_has_panics_when_missing() {
        CacheFake::new().assert_has("absent");
    }

    #[test]
    #[should_panic(expected = "expected no live cache entry")]
    fn cache_fake_assert_missing_panics_when_present() {
        let cache = CacheFake::new();
        cache.put("present", b"v".to_vec(), None);
        cache.assert_missing("present");
    }

    #[tokio::test]
    async fn notification_fake_records_hub_dispatches() {
        let fake = NotificationFake::new();
        let mail = MailFake::new();
        let mut hub = NotifyHub::empty();
        hub.mail = Some(Box::new(mail.clone()));
        let hub = hub.with_tap(Arc::new(fake.clone()));

        let principal = PrincipalId::new();
        let ctx = NotificationContext {
            principal_id: principal,
            tenant_id: None,
            locale: None,
            origin: kelson_notifications::DeliveryOrigin::default(),
        };

        // A failed dispatch (no target for the email channel) records nothing.
        assert!(hub
            .dispatch_notification(&TestNote, Channel::Email, None, &ctx)
            .await
            .is_err());
        assert!(fake.records().is_empty(), "failed dispatch must not record");

        hub.dispatch_notification(&TestNote, Channel::Email, Some("ada@example.com"), &ctx)
            .await
            .expect("dispatch succeeds");
        fake.assert_sent_to(principal, "testkit.note", Channel::Email);
        // The dispatcher filled the recipient and delivered through the
        // wired MailFake.
        let delivered = mail.assert_sent(|m| m.subject == "Welcome to Kelson");
        assert_eq!(delivered.to, vec!["ada@example.com".to_string()]);
        assert_eq!(mail.mailables().len(), 1);
    }

    #[tokio::test]
    #[should_panic(expected = "expected \"testkit.note\"")]
    async fn notification_fake_assert_sent_to_panics_without_dispatch() {
        let fake = NotificationFake::new();
        let hub = NotifyHub::empty().with_tap(Arc::new(fake.clone()));
        let ctx = NotificationContext {
            principal_id: PrincipalId::new(),
            tenant_id: None,
            locale: None,
            origin: kelson_notifications::DeliveryOrigin::default(),
        };
        // Only successful dispatches record; this one has no target.
        assert!(hub
            .dispatch_notification(&TestNote, Channel::Email, None, &ctx)
            .await
            .is_err());
        fake.assert_sent_to(ctx.principal_id, "testkit.note", Channel::Email);
    }
}
