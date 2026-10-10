//! # §3.2 compile-time tenant enforcement (target-arch §3.2)
//!
//! Port **extension traits** whose methods REQUIRE a `&TenantContext`
//! (`kelson-context`, produced only by a resolved execution path): a caller
//! without a context cannot invoke tenant-scoped access at all — the
//! compiler demands the argument. No ambient context, no thread-local and
//! no implicit tenant exists anywhere in the platform; the tenant is either
//! an explicit parameter (§3.1 ports) or an explicit context (§3.2
//! extension), never environmental.
//!
//! Blanket impls give EVERY adapter of a scoped port (`TenantCacheShard`,
//! `TenantStorageShard`, `InMemoryJobStore`, `PgJobStore`, …) the
//! context-taking surface for free; the registry-shaped ports (events,
//! scheduler) are structs, so they receive direct impls instead.
//!
//! All five extensions live here — in `kelson-tenancy`, the enterprise crate —
//! because the matrix (`allowed_deps`, tools/kelson-analyzer) permits
//! Enterprise → Services (cache/storage) and Enterprise → Async
//! (jobs/events/scheduler), and no workspace crate depends on
//! kelson-tenancy, so adding the five deps creates no cycle.

use kelson_caching::TenantCachePort;
use kelson_context::TenantContext;
use kelson_event::{ListenerRegistry, RegisteredListener};
use kelson_foundation::AppError;
use kelson_jobs::{DurableJob, JobStatus, TenantJobStore};
use kelson_scheduling::{ScheduleItem, ScheduleRegistry};
use kelson_storage::{ObjectRef, TenantStoragePort};
use time::OffsetDateTime;
use uuid::Uuid;

// ----------------------------------------------------------------- cache

/// §3.2 cache extension: every method demands `&TenantContext`, then
/// delegates to the §3.1 tenant-parameter port with `ctx.id`.
pub trait TenantScopedCache: TenantCachePort {
    fn get_scoped(&self, ctx: &TenantContext, key: &str) -> Option<Vec<u8>> {
        self.get(ctx.id, key)
    }

    fn put_scoped(&self, ctx: &TenantContext, key: &str, value: Vec<u8>, ttl_secs: Option<u64>) {
        self.put(ctx.id, key, value, ttl_secs)
    }

    fn delete_scoped(&self, ctx: &TenantContext, key: &str) {
        self.delete(ctx.id, key)
    }

    fn invalidate_prefix_scoped(&self, ctx: &TenantContext, prefix: &str) {
        self.invalidate_prefix(ctx.id, prefix)
    }

    fn put_if_absent_scoped(
        &self,
        ctx: &TenantContext,
        key: &str,
        value: Vec<u8>,
        ttl_secs: Option<u64>,
    ) -> bool {
        self.put_if_absent(ctx.id, key, value, ttl_secs)
    }

    fn delete_if_equals_scoped(&self, ctx: &TenantContext, key: &str, expected: &[u8]) -> bool {
        self.delete_if_equals(ctx.id, key, expected)
    }
}

impl<T: TenantCachePort + ?Sized> TenantScopedCache for T {}

// --------------------------------------------------------------- storage

/// §3.2 storage extension: context-gated `TenantStoragePort` delegation.
pub trait TenantScopedStorage: TenantStoragePort {
    fn put_scoped(
        &self,
        ctx: &TenantContext,
        bucket: &str,
        key: &str,
        bytes: Vec<u8>,
    ) -> Result<ObjectRef, AppError> {
        self.put(ctx.id, bucket, key, bytes)
    }

    fn get_scoped(&self, ctx: &TenantContext, obj: &ObjectRef) -> Result<Vec<u8>, AppError> {
        self.get(ctx.id, obj)
    }

    fn delete_scoped(&self, ctx: &TenantContext, obj: &ObjectRef) -> Result<(), AppError> {
        self.delete(ctx.id, obj)
    }

    fn presigned_url_scoped(
        &self,
        ctx: &TenantContext,
        obj: &ObjectRef,
        ttl_secs: u64,
    ) -> Result<String, AppError> {
        self.presigned_url(ctx.id, obj, ttl_secs)
    }
}

impl<T: TenantStoragePort + ?Sized> TenantScopedStorage for T {}

// ------------------------------------------------------------------ jobs

/// §3.2 queue extension: context-gated `TenantJobStore` delegation —
/// enqueue/claim/complete/fail all require the resolved context.
pub trait TenantScopedJobs: TenantJobStore {
    fn enqueue_scoped(&self, ctx: &TenantContext, job: DurableJob) -> Result<Uuid, AppError> {
        self.enqueue_for(ctx.id, job)
    }

    fn claim_scoped(
        &self,
        ctx: &TenantContext,
        worker: &str,
    ) -> Result<Option<DurableJob>, AppError> {
        self.claim_for(ctx.id, worker)
    }

    fn complete_scoped(&self, ctx: &TenantContext, id: Uuid) -> Result<(), AppError> {
        self.complete_for(ctx.id, id)
    }

    fn fail_scoped(
        &self,
        ctx: &TenantContext,
        id: Uuid,
        error: &str,
    ) -> Result<JobStatus, AppError> {
        self.fail_for(ctx.id, id, error)
    }
}

impl<T: TenantJobStore + ?Sized> TenantScopedJobs for T {}

// ---------------------------------------------------------------- events

/// §3.2 events extension over the registry struct: listener lookup by
/// resolved context — a tenant's listener view never exists without one.
pub trait TenantScopedEvents {
    fn listeners_scoped<'a>(
        &'a self,
        ctx: &TenantContext,
        event: &str,
    ) -> Vec<&'a RegisteredListener>;
}

impl TenantScopedEvents for ListenerRegistry {
    fn listeners_scoped<'a>(
        &'a self,
        ctx: &TenantContext,
        event: &str,
    ) -> Vec<&'a RegisteredListener> {
        self.for_event(ctx.id, event)
    }
}

// ------------------------------------------------------------- scheduler

/// §3.2 scheduler extension: due-scan and catalog views by resolved
/// context (tenant + global schedules only — platform items stay in the
/// break-glass physical views).
pub trait TenantScopedScheduler {
    fn due_scoped<'a>(
        &'a self,
        ctx: &TenantContext,
        last_fired: &[(String, OffsetDateTime)],
        now: OffsetDateTime,
    ) -> Result<Vec<&'a ScheduleItem>, AppError>;

    fn items_scoped(&self, ctx: &TenantContext) -> Vec<&ScheduleItem>;
}

impl TenantScopedScheduler for ScheduleRegistry {
    fn due_scoped<'a>(
        &'a self,
        ctx: &TenantContext,
        last_fired: &[(String, OffsetDateTime)],
        now: OffsetDateTime,
    ) -> Result<Vec<&'a ScheduleItem>, AppError> {
        self.due_for(ctx.id, last_fired, now)
    }

    fn items_scoped(&self, ctx: &TenantContext) -> Vec<&ScheduleItem> {
        self.items_for(ctx.id)
    }
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::hash_map::Entry;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use kelson_caching::{CachePort, TenantCacheShard};
    use kelson_event::{DeliveryMode, ListenerHandler, ListenerSpec, Ordering};
    use kelson_foundation::TenantId;
    use kelson_jobs::{build_from_def, InMemoryJobStore, JobDef};
    use kelson_scheduling::ScheduleSpec;

    fn ctx_of(id: TenantId) -> TenantContext {
        TenantContext {
            id,
            slug: "t".into(),
            plan: "std".into(),
        }
    }

    /// Map-backed CachePort fake — this crate can't depend on testkit
    /// (testkit depends on the platform facades, not on kelson-tenancy).
    #[derive(Default)]
    struct FakeCache(Mutex<HashMap<String, Vec<u8>>>);

    impl CachePort for FakeCache {
        fn get(&self, key: &str) -> Option<Vec<u8>> {
            self.0.lock().unwrap().get(key).cloned()
        }

        fn put(&self, key: &str, value: Vec<u8>, _ttl_secs: Option<u64>) {
            self.0.lock().unwrap().insert(key.into(), value);
        }

        fn delete(&self, key: &str) {
            self.0.lock().unwrap().remove(key);
        }

        fn invalidate_prefix(&self, prefix: &str) {
            self.0.lock().unwrap().retain(|k, _| !k.starts_with(prefix));
        }

        fn put_if_absent(&self, key: &str, value: Vec<u8>, _ttl_secs: Option<u64>) -> bool {
            match self.0.lock().unwrap().entry(key.into()) {
                Entry::Vacant(e) => {
                    e.insert(value);
                    true
                }
                Entry::Occupied(_) => false,
            }
        }

        fn delete_if_equals(&self, key: &str, expected: &[u8]) -> bool {
            let mut map = self.0.lock().unwrap();
            if map.get(key).map(|v| v.as_slice()) == Some(expected) {
                map.remove(key);
                true
            } else {
                false
            }
        }
    }

    /// The context is the scoper: identical keys under different contexts
    /// never collide, and one context's mutations can't reach another's.
    #[test]
    fn tenant_isolation_extension_cache_scopes_through_the_context() {
        let shard = TenantCacheShard::new(Arc::new(FakeCache::default()));
        let a = ctx_of(TenantId::new());
        let b = ctx_of(TenantId::new());

        shard.put_scoped(&a, "profile", b"a".to_vec(), None);
        assert_eq!(shard.get_scoped(&a, "profile"), Some(b"a".to_vec()));
        assert_eq!(
            shard.get_scoped(&b, "profile"),
            None,
            "same key, other context: empty"
        );
        shard.delete_scoped(&b, "profile");
        assert_eq!(
            shard.get_scoped(&a, "profile"),
            Some(b"a".to_vec()),
            "B's delete must not reach A's entry"
        );

        shard.put_scoped(&a, "profile.2", b"x".to_vec(), None);
        shard.invalidate_prefix_scoped(&a, "profile");
        assert_eq!(shard.get_scoped(&a, "profile"), None);
        assert_eq!(shard.get_scoped(&a, "profile.2"), None);
    }

    /// Every queue operation demands the context; the cross-context
    /// claim/complete are invisible.
    #[test]
    fn tenant_isolation_extension_jobs_require_the_context() {
        let store = InMemoryJobStore::new();
        let a = ctx_of(TenantId::new());
        let b = ctx_of(TenantId::new());

        let def = JobDef::new("mail.send");
        let job = build_from_def(&def, serde_json::json!({}), OffsetDateTime::now_utc());
        let id = store.enqueue_scoped(&a, job).unwrap();

        assert!(
            store.claim_scoped(&b, "worker").unwrap().is_none(),
            "a claim without the owning context sees nothing"
        );
        let claimed = store
            .claim_scoped(&a, "worker")
            .unwrap()
            .expect("owner context claims its job");
        assert_eq!(claimed.id, id);
        assert_eq!(claimed.tenant_id, Some(a.id));

        let err = store.complete_scoped(&b, id).unwrap_err();
        assert_eq!(err.code, kelson_foundation::codes::NOT_FOUND);
        store.complete_scoped(&a, id).unwrap();
    }

    struct NoopHandler;

    impl ListenerHandler for NoopHandler {
        fn handle(
            &self,
            _event: &kelson_event::OutboxEvent,
        ) -> kelson_event::BoxFuture<'_, Result<(), AppError>> {
            Box::pin(std::future::ready(Ok(())))
        }
    }

    fn spec(handler: &str, tenant: Option<TenantId>) -> ListenerSpec {
        ListenerSpec {
            event: "E".into(),
            handler: handler.into(),
            delivery: DeliveryMode::AtLeastOnce,
            ordering: Ordering::BestEffort,
            retry_max: 3,
            idempotent: true,
            tenant,
        }
    }

    /// Registry views by context: tenant listeners + globals only; the
    /// due-scan is likewise per-context even with same-named schedules.
    #[test]
    fn tenant_isolation_extension_events_and_scheduler_view_by_context() {
        let a = ctx_of(TenantId::new());
        let b = ctx_of(TenantId::new());

        let mut reg = ListenerRegistry::new();
        reg.register(spec("global", None), Arc::new(NoopHandler))
            .unwrap();
        reg.register(spec("a-only", Some(a.id)), Arc::new(NoopHandler))
            .unwrap();
        reg.register(spec("b-only", Some(b.id)), Arc::new(NoopHandler))
            .unwrap();

        let handlers_a: Vec<&str> = reg
            .listeners_scoped(&a, "E")
            .iter()
            .map(|l| l.spec.handler.as_str())
            .collect();
        assert!(handlers_a.contains(&"a-only") && handlers_a.contains(&"global"));
        assert!(!handlers_a.contains(&"b-only"), "B's listener never in A's view");
        let handlers_b: Vec<&str> = reg
            .listeners_scoped(&b, "E")
            .iter()
            .map(|l| l.spec.handler.as_str())
            .collect();
        assert!(handlers_b.contains(&"b-only") && handlers_b.contains(&"global"));
        assert!(!handlers_b.contains(&"a-only"));

        let mut sched = ScheduleRegistry::new();
        let item = |tenant: TenantId| ScheduleItem {
            name: "close-day".into(),
            operation: "billing.close-day".into(),
            spec: ScheduleSpec::Interval { secs: 60 },
            payload: serde_json::json!({}),
            enabled: true,
            tenant: Some(tenant),
            ..Default::default()
        };
        sched.register(item(a.id)).unwrap();
        sched.register(item(b.id)).unwrap(); // same name, other context: allowed

        assert_eq!(sched.items_scoped(&a).len(), 1);
        assert_eq!(sched.items_scoped(&b).len(), 1);
        let now = OffsetDateTime::now_utc();
        let due_a = sched.due_scoped(&a, &[], now).unwrap();
        assert_eq!(due_a.len(), 1);
        assert_eq!(due_a[0].tenant, Some(a.id));
        assert_eq!(sched.due_scoped(&b, &[], now).unwrap().len(), 1);
    }

    // ------------------------------- composed pentest (Phase 5, §9 box 2)

    /// Composed escalation: an artifact forged in ONE subsystem and carried
    /// into the next must never become the scope.
    ///
    /// The per-subsystem probes prove each layer in isolation, but an
    /// attacker does not stop at one layer. This walks a single forged blob
    /// through the realistic chain — planted in the cache, fed into a job
    /// payload, emitted as an event payload, then echoed as a schedule name —
    /// and asserts the same invariant at every hop:
    ///
    /// > the AUTHORITATIVE tenant comes from `&TenantContext`, never from
    /// > the bytes being transported.
    ///
    /// A caller with no context cannot reach any of these APIs at all (the
    /// compiler demands the argument, §3.2), so the only way to try is to
    /// smuggle the tenant inside data — which is what this does.
    #[test]
    fn pentest_composed_escalation_never_carries_tenant_across_subsystems() {
        let a = ctx_of(TenantId::new());
        let b = ctx_of(TenantId::new());
        let forged = br#"{"tenant":"other","role":"admin"}"#;

        // --- hop 1: cache — plant the blob, then read it back as B --------
        let cache = TenantCacheShard::new(Arc::new(FakeCache::default()));
        cache.put_scoped(&a, "session:forge", forged.to_vec(), None);
        assert_eq!(
            cache.get_scoped(&b, "session:forge"),
            None,
            "hop 1: the plant is invisible to the tenant it claims to be"
        );
        let stolen = cache
            .get_scoped(&a, "session:forge")
            .expect("hop 1: A still holds its own plant");

        // --- hop 2: queue — feed the blob in, forge the job's own stamp ---
        let store = InMemoryJobStore::new();
        let def = JobDef::new("mail.send");
        let body = serde_json::json!({
            "blob": String::from_utf8_lossy(&stolen),
            // The attacker also forges the job's first-class tenant field.
            "tenant": b.id,
        });

        let mut pre_stamped = build_from_def(&def, body.clone(), OffsetDateTime::now_utc());
        pre_stamped.tenant_id = Some(b.id);
        let refused = store.enqueue_scoped(&a, pre_stamped).unwrap_err();
        assert_eq!(
            refused.code,
            kelson_foundation::codes::VALIDATION,
            "hop 2: a job pre-stamped for another tenant is refused outright"
        );

        let clean = build_from_def(&def, body, OffsetDateTime::now_utc());
        let id = store.enqueue_scoped(&a, clean).unwrap();
        assert!(
            store.claim_scoped(&b, "worker").unwrap().is_none(),
            "hop 2: B never sees the job A enqueued"
        );
        let claimed = store
            .claim_scoped(&a, "worker")
            .unwrap()
            .expect("hop 2: A claims its own job");
        assert_eq!(
            claimed.tenant_id,
            Some(a.id),
            "hop 2: the context stamped the job, not the payload"
        );
        assert_eq!(
            store.complete_scoped(&b, id).unwrap_err().code,
            kelson_foundation::codes::NOT_FOUND,
            "hop 2: carrying the id across fails closed"
        );

        // --- hop 3: events — the blob rides an event payload --------------
        let mut reg = ListenerRegistry::new();
        let mut listener = ListenerSpec {
            event: "SessionForged".into(),
            handler: "b-only".into(),
            delivery: DeliveryMode::AtLeastOnce,
            ordering: Ordering::BestEffort,
            retry_max: 3,
            idempotent: true,
            tenant: Some(b.id),
        };
        reg.register(listener.clone(), Arc::new(NoopHandler)).unwrap();
        listener.handler = "a-only".to_string();
        listener.tenant = Some(a.id);
        reg.register(listener, Arc::new(NoopHandler)).unwrap();

        // Whatever A published, its view gains nothing of B's, and B's view
        // gains nothing of A's — the registry is keyed by context, not by
        // anything the payload asserts.
        let view_a: Vec<&str> = reg
            .listeners_scoped(&a, "SessionForged")
            .iter()
            .map(|l| l.spec.handler.as_str())
            .collect();
        assert!(
            view_a.contains(&"a-only") && !view_a.contains(&"b-only"),
            "hop 3: A's listener view leaked B's handler: {view_a:?}"
        );
        let view_b: Vec<&str> = reg
            .listeners_scoped(&b, "SessionForged")
            .iter()
            .map(|l| l.spec.handler.as_str())
            .collect();
        assert!(
            view_b.contains(&"b-only") && !view_b.contains(&"a-only"),
            "hop 3: B's listener view leaked A's handler: {view_b:?}"
        );

        // --- hop 4: scheduler — echo B's schedule name from A's context ---
        let mut sched = ScheduleRegistry::new();
        let named = |tenant: kelson_foundation::TenantId, name: &str| ScheduleItem {
            name: name.into(),
            operation: "billing.close-day".into(),
            spec: ScheduleSpec::Interval { secs: 60 },
            payload: serde_json::json!({}),
            enabled: true,
            tenant: Some(tenant),
            ..Default::default()
        };
        // A names its schedule exactly like B's — the name is data too.
        sched.register(named(a.id, "close-day")).unwrap();
        sched.register(named(b.id, "close-day")).unwrap();
        let now = OffsetDateTime::now_utc();
        let due_a = sched.due_scoped(&a, &[], now).unwrap();
        let due_b = sched.due_scoped(&b, &[], now).unwrap();
        assert_eq!(due_a.len(), 1, "hop 4: A sees exactly its own schedule");
        assert_eq!(due_a[0].tenant, Some(a.id));
        assert_eq!(due_b.len(), 1, "hop 4: B sees exactly its own schedule");
        assert_eq!(due_b[0].tenant, Some(b.id));
    }
}
