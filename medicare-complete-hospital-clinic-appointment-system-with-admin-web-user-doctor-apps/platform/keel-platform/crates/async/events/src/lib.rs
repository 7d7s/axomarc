//! # keel-event
//! Events are business facts, declared explicitly (11-SPEC §39/§42/§40). No global
//! listener magic: every listener declares event, handler, delivery mode, retry,
//! idempotency and ordering. Publication only happens post-commit via the outbox
//! (D-53): outbox rows exist only after COMMIT, and the pump runs out-of-band.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use keel_foundation::{AppError, TenantId};
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
    /// Tenant scope (target-arch §3.1): `Some(t)` = this listener serves
    /// ONLY tenant `t`'s events (plus nothing else); `None` = a global,
    /// tenant-agnostic listener that receives every tenant's events and is
    /// the only kind that ever sees platform events (tenant `None`).
    #[serde(default)]
    pub tenant: Option<TenantId>,
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

/// §5.2 event idempotency key (KEEL-006, KEEL-V10): SHA-256 hex over
/// `event + tenant + correlation_id + payload_hash` in canonical JSON —
/// deterministic across versions and processes, so a producer that
/// re-appends the same logical event (replay, retry, dual-write) carries
/// the same key and the pump skips the duplicate dispatch.
///
/// Empty on pre-migration rows (`serde(default)`): the pump treats the
/// empty key as legacy and performs NO dedup — back-compat, zero behavior
/// change for rows written before the column existed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IdempotencyKey(pub String);

impl IdempotencyKey {
    /// True for a pre-migration / legacy row: never deduped.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Display for IdempotencyKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Compute the §5.2 key at APPEND time (the row shape has no separate
/// correlation column — the formula reads `correlation_id` from the event
/// body when the producer embeds one). Distinct-but-identical occurrences
/// stay distinct exactly when the producer stamps a correlation id (or
/// varies the payload); everything else collapses to one key BY DESIGN —
/// that collapse is the KEEL-006 dedup.
pub fn idempotency_key(
    event: &str,
    tenant_id: Option<TenantId>,
    payload: &serde_json::Value,
) -> IdempotencyKey {
    let correlation_id = payload
        .get("correlation_id")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let payload_hash = keel_registry::config_hash(payload);
    IdempotencyKey(keel_registry::config_hash(&serde_json::json!({
        "event": event,
        "tenant": tenant_id,
        "correlation_id": correlation_id,
        "payload_hash": payload_hash,
    })))
}

/// Committed event awaiting dispatch (outbox row shape).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboxEvent {
    pub id: Uuid,
    pub event: String,
    pub topic: String,
    pub tenant_id: Option<keel_foundation::TenantId>,
    pub payload: serde_json::Value,
    /// §5.2 (KEEL-006/KEEL-V10): computed at append time by
    /// [`idempotency_key`]; empty for pre-migration rows (no dedup).
    #[serde(default)]
    pub idempotency_key: IdempotencyKey,
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
        // Identity is scope-qualified: the same event → handler pair may be
        // registered once per tenant (plus once as a global listener).
        if self.listeners.iter().any(|l| {
            l.spec.event == spec.event && l.spec.handler == spec.handler && l.spec.tenant == spec.tenant
        }) {
            return Err(AppError::validation(format!(
                "listener registered twice: {} -> {}",
                spec.event, spec.handler
            )));
        }
        self.listeners.push(RegisteredListener { spec, handler });
        Ok(())
    }

    /// §3.1: listeners for ONE tenant's event — the tenant's own listeners
    /// plus the global (tenant-agnostic) ones. A tenant-scoped listener
    /// never matches another tenant; platform events never route through
    /// this view (see [`for_event_global`](Self::for_event_global)).
    pub fn for_event(&self, tenant: TenantId, event: &str) -> Vec<&RegisteredListener> {
        self.listeners
            .iter()
            .filter(|l| {
                l.spec.event == event
                    && match l.spec.tenant {
                        None => true,
                        Some(t) => t == tenant,
                    }
            })
            .collect()
    }

    /// §3.1: listeners for a PLATFORM event (no tenant) — global
    /// (tenant-agnostic) listeners only; a tenant-scoped listener never
    /// observes platform events.
    pub fn for_event_global(&self, event: &str) -> Vec<&RegisteredListener> {
        self.listeners
            .iter()
            .filter(|l| l.spec.event == event && l.spec.tenant.is_none())
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

/// §5.2 completion gate: `check` returns `true` when this key already
/// completed → the pump skips dispatch and marks the row published;
/// `mark_done` records completion only after EVERY listener succeeded.
/// The pump only ever calls these with NON-empty keys — an empty key is a
/// pre-migration row and is dispatched without dedup (back-compat).
pub trait IdempotencyGate: Send + Sync {
    fn check(&self, key: &IdempotencyKey) -> BoxFuture<'_, Result<bool, AppError>>;
    fn mark_done(&self, key: &IdempotencyKey) -> BoxFuture<'_, Result<(), AppError>>;
}

/// Default gate: completed keys in a process-local set (degraded boot +
/// tests; a durable deployment implements [`IdempotencyGate`] over the
/// platform's `idempotency_keys` table and injects it with
/// [`OutboxPump::with_gate`]).
#[derive(Default)]
pub struct InMemoryIdempotencyGate {
    done: Mutex<std::collections::HashSet<String>>,
}

impl InMemoryIdempotencyGate {
    pub fn new() -> Self {
        Self::default()
    }
}

impl IdempotencyGate for InMemoryIdempotencyGate {
    fn check(&self, key: &IdempotencyKey) -> BoxFuture<'_, Result<bool, AppError>> {
        let seen = self.done.lock().unwrap().contains(&key.0);
        Box::pin(async move { Ok(seen) })
    }

    fn mark_done(&self, key: &IdempotencyKey) -> BoxFuture<'_, Result<(), AppError>> {
        self.done.lock().unwrap().insert(key.0.clone());
        Box::pin(async move { Ok(()) })
    }
}

/// One pump pass: what was fetched, dispatched, failed.
#[derive(Debug, Clone, Default)]
pub struct PumpReport {
    pub fetched: usize,
    pub dispatched: usize,
    pub failed: usize,
    /// §5.2: rows whose idempotency key already completed — skipped before
    /// dispatch (and marked published) instead of redelivered.
    pub skipped_duplicate: usize,
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
    gate: Arc<dyn IdempotencyGate>,
    batch_size: usize,
}

impl OutboxPump {
    pub fn new(store: Arc<dyn OutboxStore>, listeners: Arc<ListenerRegistry>) -> Self {
        Self {
            store,
            listeners,
            gate: Arc::new(InMemoryIdempotencyGate::new()),
            batch_size: 50,
        }
    }

    /// §5.2: swap the completion gate (e.g. a durable gate over the
    /// `idempotency_keys` table); defaults to [`InMemoryIdempotencyGate`].
    pub fn with_gate(mut self, gate: Arc<dyn IdempotencyGate>) -> Self {
        self.gate = gate;
        self
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
            // §5.2 (KEEL-006): dedup BEFORE dispatch. Empty key = a
            // pre-migration row: legacy rows carry no key, are never
            // deduped, and never reach the gate (back-compat).
            let gated = !event.idempotency_key.is_empty();
            if gated && self.gate.check(&event.idempotency_key).await? {
                report.skipped_duplicate += 1;
                tracing::debug!(
                    event = %event.event,
                    key = %event.idempotency_key,
                    "duplicate idempotency key: already completed, skipping dispatch"
                );
                done.push(event.id);
                continue;
            }
            // Target-arch §3.1 dispatch: a tenant event reaches that
            // tenant's listeners plus the global ones; a platform event
            // reaches ONLY global listeners.
            let listeners = match event.tenant_id {
                Some(tenant) => self.listeners.for_event(tenant, &event.event),
                None => self.listeners.for_event_global(&event.event),
            };
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
                // Completion is recorded only after EVERY listener
                // succeeded: a failed event stays unmarked, so redelivery
                // dispatches it again (at-least-once preserved).
                if gated {
                    self.gate.mark_done(&event.idempotency_key).await?;
                }
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
        event_for(name, None, json!({"k": "v"}))
    }

    /// Append-time fixture: computes the §5.2 key exactly like the real
    /// append paths do (key = f(event, tenant, payload)).
    fn event_for(name: &str, tenant: Option<TenantId>, payload: serde_json::Value) -> OutboxEvent {
        let key = idempotency_key(name, tenant, &payload);
        OutboxEvent {
            id: Uuid::now_v7(),
            event: name.into(),
            topic: "test".into(),
            tenant_id: tenant,
            payload,
            idempotency_key: key,
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
            tenant: None,
        };
        assert!(bad.validate().is_err());
    }

    #[tokio::test]
    async fn pump_dispatches_and_publishes() {
        let outbox = Arc::new(InMemoryOutbox::new());
        // Content-distinct rows: §5.2 keys are content-based, so two
        // IDENTICAL OrderPlaced bodies would (correctly) dedup — vary the
        // payload the way a producer varying quantity/correlation does.
        outbox.push(event_for("OrderPlaced", None, json!({"seq": 1})));
        outbox.push(event_for("OrderPlaced", None, json!({"seq": 2})));
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
                    tenant: None,
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
                    tenant: None,
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
            tenant: None,
        };
        registry.register(spec(), Recorder::new(0)).unwrap();
        assert!(registry.register(spec(), Recorder::new(0)).is_err());
        assert_eq!(registry.for_event_global("E").len(), 1);
        assert!(registry.for_event_global("Other").is_empty());
        assert_eq!(registry.specs().len(), 1);
    }

    // ------------------------------------------- tenant isolation probes (DoD)

    /// Registry views: a tenant's view = its own listeners + globals; the
    /// platform view = globals only; same event→handler pair may exist once
    /// per tenant (scope-qualified dedup).
    #[test]
    fn tenant_isolation_events_registry_views_are_scoped() {
        let a = TenantId::new();
        let b = TenantId::new();
        let base = ListenerSpec {
            event: "E".into(),
            handler: "same-name".into(),
            delivery: DeliveryMode::AtLeastOnce,
            ordering: Ordering::BestEffort,
            retry_max: 0,
            idempotent: true,
            tenant: None,
        };

        let mut registry = ListenerRegistry::new();
        registry.register(base.clone(), Recorder::new(0)).unwrap(); // global
        let mut for_a = base.clone();
        for_a.tenant = Some(a);
        registry.register(for_a.clone(), Recorder::new(0)).unwrap();
        let mut for_b = base;
        for_b.tenant = Some(b);
        // Identical event + handler under another tenant is legal.
        registry.register(for_b, Recorder::new(0)).unwrap();

        let view_a = registry.for_event(a, "E");
        assert_eq!(view_a.len(), 2, "tenant A sees global + its own listener");
        assert!(view_a.iter().any(|l| l.spec.tenant == Some(a)));
        assert!(
            !view_a.iter().any(|l| l.spec.tenant == Some(b)),
            "tenant B's listener must never appear in A's view"
        );

        let view_b = registry.for_event(b, "E");
        assert_eq!(view_b.len(), 2, "tenant B sees global + its own listener");
        assert!(!view_b.iter().any(|l| l.spec.tenant == Some(a)));

        let platform = registry.for_event_global("E");
        assert_eq!(
            platform.len(),
            1,
            "platform view is globals only — tenant listeners excluded"
        );
        assert!(platform[0].spec.tenant.is_none());
    }

    /// End-to-end pump routing: a tenant event reaches its tenant's
    /// listener + globals; a platform event reaches globals only; another
    /// tenant's listener is never invoked.
    #[tokio::test]
    async fn tenant_isolation_events_pump_routes_only_to_owner_tenant() {
        let a = TenantId::new();
        let b = TenantId::new();
        let outbox = Arc::new(InMemoryOutbox::new());
        // Key computed WITH the tenant (append-time parity): tenant feeds
        // the §5.2 formula, so the tenant row never collides with the
        // platform twin even at identical content.
        outbox.push(event_for("E", Some(a), json!({"k": "v"})));
        outbox.push(event("E")); // platform event (tenant_id = None)

        let global_h = Recorder::new(0);
        let a_h = Recorder::new(0);
        let b_h = Recorder::new(0);
        let mut registry = ListenerRegistry::new();
        let mut spec = ListenerSpec {
            event: "E".into(),
            handler: "global".into(),
            delivery: DeliveryMode::AtLeastOnce,
            ordering: Ordering::BestEffort,
            retry_max: 0,
            idempotent: true,
            tenant: None,
        };
        registry.register(spec.clone(), global_h.clone()).unwrap();
        spec.handler = "for-a".into();
        spec.tenant = Some(a);
        registry.register(spec.clone(), a_h.clone()).unwrap();
        spec.handler = "for-b".into();
        spec.tenant = Some(b);
        registry.register(spec, b_h.clone()).unwrap();

        let pump = OutboxPump::new(outbox.clone(), Arc::new(registry));
        let report = pump.run_once().await.unwrap();
        assert_eq!(report.fetched, 2);
        assert_eq!(report.dispatched, 2, "both events dispatch successfully");

        assert_eq!(
            global_h.calls.load(AtomicOrdering::SeqCst),
            2,
            "the global listener receives tenant AND platform events"
        );
        assert_eq!(
            a_h.calls.load(AtomicOrdering::SeqCst),
            1,
            "tenant A's listener receives A's event"
        );
        assert_eq!(
            b_h.calls.load(AtomicOrdering::SeqCst),
            0,
            "tenant B's listener must never observe A's or platform events"
        );
    }

    // -------------------------------------- event idempotency probes (§5.2 DoD)

    /// §5.2 formula: deterministic, and every input the doc names — event,
    /// tenant, correlation_id (read from the body), payload — separates
    /// keys.
    #[test]
    fn outbox_idempotency_key_is_stable_and_content_sensitive() {
        let t = TenantId::new();
        let payload = json!({"sku": "x1", "correlation_id": "c-1"});
        let base = idempotency_key("order.created", Some(t), &payload);
        assert!(!base.is_empty());
        assert_eq!(
            base,
            idempotency_key("order.created", Some(t), &payload),
            "same inputs => same key (deterministic across processes)"
        );
        assert_ne!(
            base,
            idempotency_key("order.updated", Some(t), &payload),
            "event name separates"
        );
        assert_ne!(
            base,
            idempotency_key("order.created", None, &payload),
            "tenant separates"
        );
        let mut other_corr = payload.clone();
        other_corr["correlation_id"] = json!("c-2");
        assert_ne!(
            base,
            idempotency_key("order.created", Some(t), &other_corr),
            "correlation id separates (distinct occurrences stay distinct)"
        );
        let mut other_body = payload.clone();
        other_body["sku"] = json!("x2");
        assert_ne!(
            base,
            idempotency_key("order.created", Some(t), &other_body),
            "payload separates"
        );
    }

    /// Back-compat: a row written BEFORE the migration (no key member in
    /// its JSON) deserializes to the EMPTY key — legacy rows are never
    /// deduped and never reach the gate.
    #[test]
    fn outbox_event_serde_defaults_empty_key_for_pre_migration_rows() {
        let mut legacy = serde_json::to_value(event("legacy")).unwrap();
        legacy
            .as_object_mut()
            .unwrap()
            .remove("idempotency_key");
        let row: OutboxEvent = serde_json::from_value(legacy).unwrap();
        assert!(row.idempotency_key.is_empty());
        assert!(IdempotencyKey::default().is_empty());
    }

    /// KEEL-006: a re-appended duplicate (same logical content, new row
    /// id) is skipped BEFORE dispatch — the listener runs exactly once and
    /// both rows end published.
    #[tokio::test]
    async fn outbox_pump_skips_reappended_duplicate_by_idempotency_key() {
        let outbox = Arc::new(InMemoryOutbox::new());
        outbox.push(event("OrderPlaced"));
        outbox.push(event("OrderPlaced")); // same content => same key => duplicate
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
                    tenant: None,
                },
                recorder.clone(),
            )
            .unwrap();
        let pump = OutboxPump::new(outbox.clone(), Arc::new(registry));

        let report = pump.run_once().await.unwrap();
        assert_eq!(report.fetched, 2);
        assert_eq!(report.dispatched, 1, "first row dispatches");
        assert_eq!(report.skipped_duplicate, 1, "re-appended duplicate skipped");
        assert_eq!(
            recorder.calls.load(AtomicOrdering::SeqCst),
            1,
            "listener ran once"
        );
        assert_eq!(outbox.unpublished(), 0, "both rows end published");
    }

    /// Tenant scoping in the key: tenant A's "OrderPlaced" + same payload
    /// gets a DIFFERENT key from tenant B's identical "OrderPlaced" —
    /// tenant is part of the formula.
    #[tokio::test]
    async fn outbox_idempotency_key_scopes_by_tenant() {
        let a = TenantId::new();
        let b = TenantId::new();
        let outbox = Arc::new(InMemoryOutbox::new());
        outbox.push(event_for("OrderPlaced", Some(a), json!({"k": "v"})));
        outbox.push(event_for("OrderPlaced", Some(b), json!({"k": "v"}))); // same content, different tenant
        let mut registry = ListenerRegistry::new();
        let recorder = Recorder::new(0);
        registry
            .register(
                ListenerSpec {
                    event: "OrderPlaced".into(),
                    handler: "record".into(),
                    delivery: DeliveryMode::AtLeastOnce,
                    ordering: Ordering::BestEffort,
                    retry_max: 3,
                    idempotent: true,
                    tenant: None, // global listener for both
                },
                recorder.clone(),
            )
            .unwrap();
        let pump = OutboxPump::new(outbox.clone(), Arc::new(registry));

        let report = pump.run_once().await.unwrap();
        assert_eq!(report.fetched, 2);
        assert_eq!(report.dispatched, 2, "tenant-scoped keys are distinct");
        assert_eq!(report.skipped_duplicate, 0);
        assert_eq!(
            recorder.calls.load(AtomicOrdering::SeqCst),
            2,
            "both tenants' events dispatched"
        );
        assert_eq!(outbox.unpublished(), 0);
    }

    // -------------------------------------- pentest (Phase 5, §9 box 2)

    /// Penetration test: cross-tenant idempotency-gate POISONING.
    ///
    /// A completed event marks its key done in the gate. If that key ignored
    /// the tenant, tenant A finishing first would mark B's key too — and B's
    /// later, genuinely new event would be skipped forever: silent
    /// cross-tenant suppression with no error anywhere.
    ///
    /// Deliberately SEQUENTIAL, unlike `outbox_idempotency_key_scopes_by_tenant`
    /// which pushes both rows at once: this exercises the gate's *stored*
    /// state carrying across runs, which is where poisoning would land.
    #[tokio::test]
    async fn pentest_events_completed_key_of_one_tenant_cannot_suppress_another() {
        let a = TenantId::new();
        let b = TenantId::new();
        let outbox = Arc::new(InMemoryOutbox::new());
        let mut registry = ListenerRegistry::new();
        let recorder = Recorder::new(0);
        registry
            .register(
                ListenerSpec {
                    event: "PaymentCaptured".into(),
                    handler: "record".into(),
                    delivery: DeliveryMode::AtLeastOnce,
                    ordering: Ordering::BestEffort,
                    retry_max: 3,
                    idempotent: true,
                    tenant: None, // global listener sees both tenants
                },
                recorder.clone(),
            )
            .unwrap();
        let pump = OutboxPump::new(outbox.clone(), Arc::new(registry));

        // Round 1 — A's event dispatches and marks its key done.
        outbox.push(event_for(
            "PaymentCaptured",
            Some(a),
            json!({"amount": 100}),
        ));
        let first = pump.run_once().await.unwrap();
        assert_eq!(first.dispatched, 1, "A's event dispatches");
        assert_eq!(first.skipped_duplicate, 0);

        // Round 2 — B appends BYTE-IDENTICAL content afterwards. A's
        // completed key must not have poisoned B's.
        outbox.push(event_for(
            "PaymentCaptured",
            Some(b),
            json!({"amount": 100}),
        ));
        let second = pump.run_once().await.unwrap();
        assert_eq!(
            second.skipped_duplicate, 0,
            "A's completed key must not suppress B's identical event"
        );
        assert_eq!(second.dispatched, 1, "B's event still dispatches");
        assert_eq!(
            recorder.calls.load(AtomicOrdering::SeqCst),
            2,
            "the listener observed both tenants"
        );
    }

    /// A failed dispatch does NOT mark the key done — redelivery
    /// retries the same key (at-least-once). When it eventually
    /// succeeds, the key is marked and a subsequent duplicate skips.
    #[tokio::test]
    async fn outbox_failed_dispatch_does_not_mark_key_redelivery_works() {
        let outbox = Arc::new(InMemoryOutbox::new());
        outbox.push(event("StockAdjusted")); // key computed at push
        let mut registry = ListenerRegistry::new();
        let recorder = Recorder::new(1); // first call fails
        registry
            .register(
                ListenerSpec {
                    event: "StockAdjusted".into(),
                    handler: "sync".into(),
                    delivery: DeliveryMode::AtLeastOnce,
                    ordering: Ordering::BestEffort,
                    retry_max: 3,
                    idempotent: true,
                    tenant: None,
                },
                recorder.clone(),
            )
            .unwrap();
        let pump = OutboxPump::new(outbox.clone(), Arc::new(registry));

        let first = pump.run_once().await.unwrap();
        assert_eq!(first.failed, 1);
        assert_eq!(first.skipped_duplicate, 0, "failed key not marked");
        assert_eq!(outbox.unpublished(), 1, "row stays for redelivery");

        let second = pump.run_once().await.unwrap();
        assert_eq!(second.dispatched, 1, "redelivery succeeds");
        assert_eq!(outbox.unpublished(), 0, "published after success");

        // A THIRD pass with the SAME key (re-appended duplicate) skips.
        outbox.push(event("StockAdjusted"));
        let third = pump.run_once().await.unwrap();
        assert_eq!(third.fetched, 1);
        assert_eq!(third.dispatched, 0);
        assert_eq!(third.skipped_duplicate, 1, "now the key is known, duplicate skipped");
        assert_eq!(outbox.unpublished(), 0);
    }

    /// The gate is swappable: InMemory for tests/degraded boot, durable
    /// table-backed for production (injected via with_gate).
    #[test]
    fn outbox_pump_gate_is_swappable() {
        let gate = Arc::new(InMemoryIdempotencyGate::new());
        let pump = OutboxPump::new(
            Arc::new(InMemoryOutbox::new()),
            Arc::new(ListenerRegistry::new()),
        )
        .with_gate(gate);
        let _ = pump;
    }
}
