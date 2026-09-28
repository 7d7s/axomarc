//! # keel-event
//! Events are business facts, declared explicitly (11-SPEC §39/§42/§40). No global
//! listener magic: every listener declares event, handler, delivery mode, retry,
//! idempotency and ordering. Publication only happens post-commit via the outbox
//! (D-53): outbox rows exist only after COMMIT, and the pump runs out-of-band.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use keel_foundation::AppError;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Boxed future used across the async ports (no external dependency).
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryMode {
    AtLeastOnce,
    AtMostOnce,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ordering {
    /// Strict per-stream ordering (consumes sequence).
    Strict,
    BestEffort,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListenerSpec {
    pub event: String,
    pub handler: String,
    pub delivery: DeliveryMode,
    pub ordering: Ordering,
    pub retry_max: u32,
    pub idempotent: bool,
}

impl ListenerSpec {
    pub fn validate(&self) -> Result<(), AppError> {
        if self.event.is_empty() || self.handler.is_empty() {
            return Err(AppError::validation("listener requires event and handler"));
        }
        if self.ordering == Ordering::Strict && !self.idempotent {
            // strict ordering with redelivery requires idempotent handlers
            return Err(AppError::validation(
                "strict-ordering listener must be idempotent (at-least-once redelivery)",
            ));
        }
        Ok(())
    }
}

/// Committed event awaiting dispatch (outbox row shape).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboxEvent {
    pub id: Uuid,
    pub event: String,
    pub topic: String,
    pub tenant_id: Option<keel_foundation::TenantId>,
    pub payload: serde_json::Value,
    pub published: bool,
    #[serde(with = "time::serde::iso8601")]
    pub created_at: time::OffsetDateTime,
}

// ------------------------------------------------------------------ handlers

/// A listener body. Handlers must be idempotent whenever delivery is
/// at-least-once (redelivery after failure is the normal recovery path).
pub trait ListenerHandler: Send + Sync {
    fn handle(&self, event: &OutboxEvent) -> BoxFuture<'_, Result<(), AppError>>;
}

/// Registered listener = validated spec + live handler (item 24 dispatch).
pub struct RegisteredListener {
    pub spec: ListenerSpec,
    handler: Arc<dyn ListenerHandler>,
}

impl RegisteredListener {
    pub fn dispatch(&self, event: &OutboxEvent) -> BoxFuture<'_, Result<(), AppError>> {
        self.handler.handle(event)
    }
}

/// Compile-time-visible catalog of listeners (feeds event:list, item 29).
#[derive(Default)]
pub struct ListenerRegistry {
    listeners: Vec<RegisteredListener>,
}

impl ListenerRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &mut self,
        spec: ListenerSpec,
        handler: Arc<dyn ListenerHandler>,
    ) -> Result<(), AppError> {
        spec.validate()?;
        if self
            .listeners
            .iter()
            .any(|l| l.spec.event == spec.event && l.spec.handler == spec.handler)
        {
            return Err(AppError::validation(format!(
                "listener registered twice: {} -> {}",
                spec.event, spec.handler
            )));
        }
        self.listeners.push(RegisteredListener { spec, handler });
        Ok(())
    }

    pub fn for_event(&self, event: &str) -> Vec<&RegisteredListener> {
        self.listeners
            .iter()
            .filter(|l| l.spec.event == event)
            .collect()
    }

    pub fn specs(&self) -> Vec<ListenerSpec> {
        self.listeners.iter().map(|l| l.spec.clone()).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.listeners.is_empty()
    }

    pub fn len(&self) -> usize {
        self.listeners.len()
    }
}

// ------------------------------------------------------------------- outbox

/// Outbox access port. Implemented by keel-postgres (outbox table) and by
/// InMemoryOutbox for degraded boot/tests.
pub trait OutboxStore: Send + Sync {
    /// Unpublished rows, oldest first, up to batch.
    fn claim_unpublished(&self, batch: usize) -> BoxFuture<'_, Result<Vec<OutboxEvent>, AppError>>;
    /// Mark events published (called only after all listeners succeeded).
    fn mark_published(&self, ids: &[Uuid]) -> BoxFuture<'_, Result<(), AppError>>;
}

/// Newtype wrapper so trait objects can live in the lifecycle
/// ServiceRegistry (TypeId-keyed; get::<T>() requires a sized type).
pub struct OutboxStoreService(pub Arc<dyn OutboxStore>);

/// Degraded-boot/test outbox (D-52).
#[derive(Debug, Default)]
pub struct InMemoryOutbox {
    events: Mutex<Vec<OutboxEvent>>,
}

impl InMemoryOutbox {
    pub fn new() -> Self {
        Self::default()
    }

    /// Transactional publishers append here (post-commit by construction:
    /// the caller only appends after its unit of work succeeds).
    pub fn push(&self, event: OutboxEvent) {
        self.events.lock().unwrap().push(event);
    }

    pub fn len(&self) -> usize {
        self.events.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.lock().unwrap().is_empty()
    }

    /// All events (test assertions, inspection).
    pub fn events(&self) -> Vec<OutboxEvent> {
        self.events.lock().unwrap().clone()
    }

    /// First event with the given name, if any (test assertions).
    pub fn find(&self, name: &str) -> Option<OutboxEvent> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .find(|e| e.event == name)
            .cloned()
    }

    pub fn unpublished(&self) -> usize {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| !e.published)
            .count()
    }
}

impl OutboxStore for InMemoryOutbox {
    fn claim_unpublished(&self, batch: usize) -> BoxFuture<'_, Result<Vec<OutboxEvent>, AppError>> {
        let out: Vec<OutboxEvent> = self
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| !e.published)
            .take(batch)
            .cloned()
            .collect();
        Box::pin(async move { Ok(out) })
    }

    fn mark_published(&self, ids: &[Uuid]) -> BoxFuture<'_, Result<(), AppError>> {
        let ids: Vec<Uuid> = ids.to_vec();
        {
            let mut events = self.events.lock().unwrap();
            for e in events.iter_mut() {
                if ids.contains(&e.id) {
                    e.published = true;
                }
            }
        }
        Box::pin(async move { Ok(()) })
    }
}

// --------------------------------------------------------------------- pump

/// One pump pass: what was fetched, dispatched, failed.
#[derive(Debug, Clone, Default)]
pub struct PumpReport {
    pub fetched: usize,
    pub dispatched: usize,
    pub failed: usize,
    /// Events with at least one failed listener; left unpublished for redelivery.
    pub failed_events: Vec<Uuid>,
}

/// Outbox pump (item 24): claims committed events and dispatches them to the
/// registered listeners. Post-commit only (D-53): this process never touches
/// in-flight transactions — outbox rows only exist after COMMIT. Events whose
/// listeners fail stay unpublished and are redelivered on the next pass
/// (at-least-once).
pub struct OutboxPump {
    store: Arc<dyn OutboxStore>,
    listeners: Arc<ListenerRegistry>,
    batch_size: usize,
}

impl OutboxPump {
    pub fn new(store: Arc<dyn OutboxStore>, listeners: Arc<ListenerRegistry>) -> Self {
        Self {
            store,
            listeners,
            batch_size: 50,
        }
    }

    pub fn with_batch_size(mut self, batch_size: usize) -> Self {
        self.batch_size = batch_size.max(1);
        self
    }

    /// One pass over the outbox.
    pub async fn run_once(&self) -> Result<PumpReport, AppError> {
        let events = self.store.claim_unpublished(self.batch_size).await?;
        let mut report = PumpReport {
            fetched: events.len(),
            ..Default::default()
        };
        let mut done = Vec::new();
        for event in &events {
            let listeners = self.listeners.for_event(&event.event);
            let mut ok = true;
            for listener in listeners {
                if let Err(e) = listener.dispatch(event).await {
                    tracing::warn!(
                        event = %event.event,
                        handler = %listener.spec.handler,
                        listener_error = %e.public_message,
                        "listener failed; event stays unpublished for redelivery"
                    );
                    ok = false;
                }
            }
            if ok {
                done.push(event.id);
                report.dispatched += 1;
            } else {
                report.failed += 1;
                report.failed_events.push(event.id);
            }
        }
        if !done.is_empty() {
            self.store.mark_published(&done).await?;
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicU32, Ordering as AtomicOrdering};
    use time::OffsetDateTime;

    fn event(name: &str) -> OutboxEvent {
        OutboxEvent {
            id: Uuid::now_v7(),
            event: name.into(),
            topic: "test".into(),
            tenant_id: None,
            payload: json!({"k": "v"}),
            published: false,
            created_at: OffsetDateTime::now_utc(),
        }
    }

    struct Recorder {
        calls: AtomicU32,
        fail_next: AtomicU32,
    }

    impl Recorder {
        fn new(fail_next: u32) -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicU32::new(0),
                fail_next: AtomicU32::new(fail_next),
            })
        }
    }

    impl ListenerHandler for Recorder {
        fn handle(&self, _event: &OutboxEvent) -> BoxFuture<'_, Result<(), AppError>> {
            self.calls.fetch_add(1, AtomicOrdering::SeqCst);
            let fails_left = self.fail_next.load(AtomicOrdering::SeqCst);
            if fails_left > 0 {
                self.fail_next.fetch_sub(1, AtomicOrdering::SeqCst);
                return Box::pin(async move { Err(AppError::external("test", "boom")) });
            }
            Box::pin(async move { Ok(()) })
        }
    }

    #[test]
    fn strict_ordering_requires_idempotence() {
        let bad = ListenerSpec {
            event: "StockAdjusted".into(),
            handler: "sync".into(),
            delivery: DeliveryMode::AtLeastOnce,
            ordering: Ordering::Strict,
            retry_max: 3,
            idempotent: false,
        };
        assert!(bad.validate().is_err());
    }

    #[tokio::test]
    async fn pump_dispatches_and_publishes() {
        let outbox = Arc::new(InMemoryOutbox::new());
        outbox.push(event("OrderPlaced"));
        outbox.push(event("OrderPlaced"));
        outbox.push(event("Unrelated"));
        let recorder = Recorder::new(0);
        let mut registry = ListenerRegistry::new();
        registry
            .register(
                ListenerSpec {
                    event: "OrderPlaced".into(),
                    handler: "record".into(),
                    delivery: DeliveryMode::AtLeastOnce,
                    ordering: Ordering::BestEffort,
                    retry_max: 3,
                    idempotent: true,
                },
                recorder.clone(),
            )
            .unwrap();
        let pump = OutboxPump::new(outbox.clone(), Arc::new(registry));

        let report = pump.run_once().await.unwrap();
        assert_eq!(report.fetched, 3);
        assert_eq!(report.dispatched, 3);
        assert_eq!(report.failed, 0);
        assert_eq!(
            recorder.calls.load(AtomicOrdering::SeqCst),
            2,
            "only matching events dispatch"
        );
        assert_eq!(outbox.unpublished(), 0, "all marked published");
    }

    #[tokio::test]
    async fn failed_listener_keeps_event_unpublished_for_redelivery() {
        let outbox = Arc::new(InMemoryOutbox::new());
        outbox.push(event("StockAdjusted"));
        let recorder = Recorder::new(1); // first call fails, redelivery succeeds
        let mut registry = ListenerRegistry::new();
        registry
            .register(
                ListenerSpec {
                    event: "StockAdjusted".into(),
                    handler: "sync".into(),
                    delivery: DeliveryMode::AtLeastOnce,
                    ordering: Ordering::BestEffort,
                    retry_max: 3,
                    idempotent: true,
                },
                recorder.clone(),
            )
            .unwrap();
        let pump = OutboxPump::new(outbox.clone(), Arc::new(registry));

        let first = pump.run_once().await.unwrap();
        assert_eq!(first.failed, 1);
        assert_eq!(outbox.unpublished(), 1, "failed event stays for redelivery");

        let second = pump.run_once().await.unwrap();
        assert_eq!(second.dispatched, 1);
        assert_eq!(outbox.unpublished(), 0);
        assert_eq!(recorder.calls.load(AtomicOrdering::SeqCst), 2);
    }

    #[test]
    fn registry_rejects_duplicates_and_filters() {
        let mut registry = ListenerRegistry::new();
        let spec = || ListenerSpec {
            event: "E".into(),
            handler: "h1".into(),
            delivery: DeliveryMode::AtMostOnce,
            ordering: Ordering::BestEffort,
            retry_max: 0,
            idempotent: false,
        };
        registry.register(spec(), Recorder::new(0)).unwrap();
        assert!(registry.register(spec(), Recorder::new(0)).is_err());
        assert_eq!(registry.for_event("E").len(), 1);
        assert!(registry.for_event("Other").is_empty());
        assert_eq!(registry.specs().len(), 1);
    }
}
