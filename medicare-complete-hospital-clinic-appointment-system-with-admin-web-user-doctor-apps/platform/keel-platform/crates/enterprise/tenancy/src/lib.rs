//! # keel-tenancy
//! Enterprise tenancy service (blueprint L76, S3). Tenants are the isolation
//! boundary of the tenant plane: every tenant-plane execution carries a
//! resolved `TenantContext` (keel-context) and the postgres adapter anchors
//! rows with `SET LOCAL keel.tenant_id` + RLS. This crate adds the control
//! plane ABOVE that: tenant records, resolution, plan/status enforcement and
//! the S3 TENANT service lifetime.
//!
//! **Service lifetimes added in S3** (STAGE-0-SPEC §6, ServiceLifetime):
//! - REQUEST: one per HTTP request (sessions + request-scoped services; see
//!   keel-session / keel-http).
//! - TENANT: one per tenant per process — `TenantScope` below. Long-lived
//!   tenant-keyed services (per-tenant config cache, rate buckets) register
//!   here instead of in the process-wide Application registry.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, RwLock};

use keel_context::TenantContext;
use keel_foundation::{AppError, TenantId, TenantRevision};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// §3.2 compile-time tenant enforcement: port extension traits whose
/// methods require a `&TenantContext`.
pub mod scoped;

pub use scoped::{
    TenantScopedCache, TenantScopedEvents, TenantScopedJobs, TenantScopedScheduler,
    TenantScopedStorage,
};

/// Tenant lifecycle status. Only `Active` and `Trial` may run executions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TenantStatus {
    Trial,
    Active,
    Suspended,
    Archived,
}

impl TenantStatus {
    pub fn may_execute(&self) -> bool {
        matches!(self, TenantStatus::Active | TenantStatus::Trial)
    }
}

/// Control-plane tenant record (persistence: `tenants` table, keel-postgres).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantRecord {
    pub id: TenantId,
    /// URL-safe unique handle, `[a-z][a-z0-9-]*`.
    pub slug: String,
    pub name: String,
    /// Plan key (e.g. `standard`, `enterprise`); interpreted via `PlanPolicy`.
    pub plan: String,
    pub status: TenantStatus,
    /// Deployment region hint (`in`, `eu`, ...); flags may scope by region.
    pub region: String,
    /// Explicit capability grants independent of plan (feature-flag dimension).
    pub capabilities: BTreeSet<String>,
    #[serde(with = "time::serde::iso8601")]
    pub created_at: OffsetDateTime,
    /// Bumped on every control-plane mutation; caches key on it.
    pub revision: TenantRevision,
}

/// Tenant storage port. keel-postgres implements it on the `tenants` table;
/// the in-memory registry serves degraded boot and tests.
pub trait TenantStore: Send + Sync {
    fn by_id(&self, id: TenantId) -> Result<Option<TenantRecord>, AppError>;
    fn by_slug(&self, slug: &str) -> Result<Option<TenantRecord>, AppError>;
    /// Insert or update; implementations bump `revision` on update.
    fn upsert(&self, record: TenantRecord) -> Result<TenantRecord, AppError>;
    fn list(&self, limit: usize) -> Result<Vec<TenantRecord>, AppError>;
}

/// Degraded-boot / test registry (same semantics as the postgres adapter).
#[derive(Debug, Default)]
pub struct InMemoryTenantStore {
    inner: RwLock<BTreeMap<TenantId, TenantRecord>>,
}

impl InMemoryTenantStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl TenantStore for InMemoryTenantStore {
    fn by_id(&self, id: TenantId) -> Result<Option<TenantRecord>, AppError> {
        Ok(self.inner.read().unwrap().get(&id).cloned())
    }

    fn by_slug(&self, slug: &str) -> Result<Option<TenantRecord>, AppError> {
        Ok(self
            .inner
            .read()
            .unwrap()
            .values()
            .find(|t| t.slug == slug)
            .cloned())
    }

    fn upsert(&self, mut record: TenantRecord) -> Result<TenantRecord, AppError> {
        validate_slug(&record.slug)?;
        let mut map = self.inner.write().unwrap();
        if map
            .values()
            .any(|t| t.slug == record.slug && t.id != record.id)
        {
            return Err(AppError::conflict(format!(
                "tenant slug taken: {}",
                record.slug
            )));
        }
        match map.get(&record.id) {
            Some(prev) => record.revision = prev.revision.next(),
            None => record.revision = TenantRevision(1),
        }
        map.insert(record.id, record.clone());
        Ok(record)
    }

    fn list(&self, limit: usize) -> Result<Vec<TenantRecord>, AppError> {
        Ok(self
            .inner
            .read()
            .unwrap()
            .values()
            .take(limit)
            .cloned()
            .collect())
    }
}

fn validate_slug(slug: &str) -> Result<(), AppError> {
    let ok_first = slug.chars().next().is_some_and(|c| c.is_ascii_lowercase());
    let ok_rest = slug.len() <= 63
        && slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if ok_first && ok_rest {
        Ok(())
    } else {
        Err(AppError::validation(format!(
            "tenant slug {slug:?} must match [a-z][a-z0-9-]* (max 63)"
        )))
    }
}

/// How the caller identified the tenant (transport-agnostic input).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TenantHint {
    Id(TenantId),
    Slug(String),
    /// Host header form, e.g. `acme.app.example.com` -> slug `acme`.
    Host(String),
}

impl TenantHint {
    pub fn from_host(host: &str) -> Option<Self> {
        let first = host.split('.').next()?.trim().to_lowercase();
        if first.is_empty() || first == "www" {
            return None;
        }
        Some(Self::Slug(first))
    }
}

/// Resolution + enforcement facade (L76). Stateless; holds an `Arc<dyn TenantStore>`.
pub struct TenantService {
    store: Arc<dyn TenantStore>,
}

impl TenantService {
    pub fn new(store: Arc<dyn TenantStore>) -> Self {
        Self { store }
    }

    pub fn store(&self) -> &Arc<dyn TenantStore> {
        &self.store
    }

    /// Resolve a hint to a record. Unknown tenant = `KER_NOT_FOUND`.
    pub fn resolve(&self, hint: &TenantHint) -> Result<TenantRecord, AppError> {
        let found = match hint {
            TenantHint::Id(id) => self.store.by_id(*id)?,
            TenantHint::Slug(slug) => self.store.by_slug(slug)?,
            TenantHint::Host(host) => {
                let slug = TenantHint::from_host(host)
                    .ok_or_else(|| AppError::not_found("tenant for host"))?;
                match slug {
                    TenantHint::Slug(slug) => self.store.by_slug(&slug)?,
                    _ => None,
                }
            }
        };
        found.ok_or_else(|| AppError::not_found("tenant"))
    }

    /// Resolve AND enforce status: suspended/archived tenants cannot execute.
    pub fn authorize(&self, hint: &TenantHint) -> Result<TenantRecord, AppError> {
        let record = self.resolve(hint)?;
        if !record.status.may_execute() {
            return Err(AppError::denied(format!(
                "tenant {} is {:?}; executions blocked",
                record.slug, record.status
            )));
        }
        Ok(record)
    }

    /// Project a record into the execution-context carrier (keel-context).
    pub fn context(&self, record: &TenantRecord) -> TenantContext {
        TenantContext {
            id: record.id,
            slug: record.slug.clone(),
            plan: record.plan.clone(),
        }
    }
}

// ------------------------------------------------------------------ plans

/// Numeric limits attached to plan keys. Unknown plan = explicit error at
/// enforcement time, never an open-ended default.
#[derive(Debug, Clone, Default)]
pub struct PlanPolicy {
    plans: BTreeMap<String, PlanLimits>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanLimits {
    pub max_users: u64,
    pub max_jobs_per_day: u64,
    pub max_storage_bytes: u64,
}

impl PlanPolicy {
    pub fn new() -> Self {
        Self::default()
    }

    /// Canonical dev defaults (standard/enterprise); apps extend in config.
    pub fn defaults() -> Self {
        let mut policy = Self::new();
        policy.define(
            "standard",
            PlanLimits {
                max_users: 50,
                max_jobs_per_day: 1_000,
                max_storage_bytes: 5 << 30,
            },
        );
        policy.define(
            "enterprise",
            PlanLimits {
                max_users: 10_000,
                max_jobs_per_day: 1_000_000,
                max_storage_bytes: 1_000 << 30,
            },
        );
        policy
    }

    pub fn define(&mut self, plan: &str, limits: PlanLimits) {
        self.plans.insert(plan.to_string(), limits);
    }

    pub fn limits(&self, plan: &str) -> Result<PlanLimits, AppError> {
        self.plans
            .get(plan)
            .copied()
            .ok_or_else(|| AppError::internal(format!("plan policy missing for {plan:?}")))
    }

    /// Quota check: adding one more unit past the limit = `CAP_LIMIT_EXCEEDED`.
    pub fn check(&self, plan: &str, metric: PlanMetric, current: u64) -> Result<(), AppError> {
        let limits = self.limits(plan)?;
        let cap = match metric {
            PlanMetric::Users => limits.max_users,
            PlanMetric::JobsPerDay => limits.max_jobs_per_day,
            PlanMetric::StorageBytes => limits.max_storage_bytes,
        };
        if current.saturating_add(1) > cap {
            return Err(AppError::cap_limit(format!(
                "{metric:?} quota for plan {plan:?} reached ({cap})"
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanMetric {
    Users,
    JobsPerDay,
    StorageBytes,
}

// ------------------------------------------------------- TENANT lifetime

type TenantServiceMap =
    BTreeMap<TenantId, BTreeMap<std::any::TypeId, Arc<dyn std::any::Any + Send + Sync>>>;

/// TENANT-scoped service container (S3 lifetime): one shared instance per
/// tenant per process. Services stored type-erased, keyed by type — same
/// constructor-injection discipline as the Application registry.
#[derive(Default)]
pub struct TenantScope {
    services: RwLock<TenantServiceMap>,
}

impl TenantScope {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fetch or lazily create the tenant-scoped instance of `T`.
    pub fn get_or_init<T: Send + Sync + 'static>(
        &self,
        tenant: TenantId,
        init: impl FnOnce(TenantId) -> T,
    ) -> Arc<T> {
        let key = std::any::TypeId::of::<T>();
        {
            let map = self.services.read().unwrap();
            if let Some(any) = map.get(&tenant).and_then(|m| m.get(&key)) {
                return any
                    .clone()
                    .downcast::<T>()
                    .expect("tenant-scoped service type is stable");
            }
        }
        let mut map = self.services.write().unwrap();
        let entry = map.entry(tenant).or_default();
        entry
            .entry(key)
            .or_insert_with(|| Arc::new(init(tenant)) as Arc<dyn std::any::Any + Send + Sync>)
            .clone()
            .downcast::<T>()
            .expect("tenant-scoped service type is stable")
    }

    /// Drop ALL per-tenant services (tenant offboarding / cache reset).
    pub fn evict(&self, tenant: TenantId) {
        self.services.write().unwrap().remove(&tenant);
    }

    pub fn tenant_count(&self) -> usize {
        self.services.read().unwrap().len()
    }
}

impl std::fmt::Debug for TenantScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TenantScope")
            .field("tenants", &self.tenant_count())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_with_acme() -> InMemoryTenantStore {
        let store = InMemoryTenantStore::new();
        store
            .upsert(TenantRecord {
                id: TenantId::new(),
                slug: "acme".into(),
                name: "Acme Hospitals".into(),
                plan: "standard".into(),
                status: TenantStatus::Active,
                region: "in".into(),
                capabilities: BTreeSet::new(),
                created_at: OffsetDateTime::now_utc(),
                revision: TenantRevision(0),
            })
            .unwrap();
        store
    }

    #[test]
    fn resolve_by_slug_id_and_host() {
        let store = Arc::new(store_with_acme());
        let svc = TenantService::new(store.clone());
        let by_slug = svc.resolve(&TenantHint::Slug("acme".into())).unwrap();
        let by_id = svc.resolve(&TenantHint::Id(by_slug.id)).unwrap();
        let by_host = svc
            .resolve(&TenantHint::Host("acme.app.example.com".into()))
            .unwrap();
        assert_eq!(by_slug.id, by_id.id);
        assert_eq!(by_slug.id, by_host.id);
        assert!(svc.resolve(&TenantHint::Slug("missing".into())).is_err());
        assert!(svc
            .resolve(&TenantHint::Host("www.example.com".into()))
            .is_err());
    }

    #[test]
    fn suspended_tenant_is_denied() {
        let store = Arc::new(store_with_acme());
        let svc = TenantService::new(store.clone());
        let mut rec = svc.resolve(&TenantHint::Slug("acme".into())).unwrap();
        rec.status = TenantStatus::Suspended;
        store.upsert(rec).unwrap();
        let err = svc.authorize(&TenantHint::Slug("acme".into())).unwrap_err();
        assert_eq!(err.code, keel_foundation::codes::DENIED);
        assert!(
            svc.resolve(&TenantHint::Slug("acme".into())).is_ok(),
            "resolve still finds it"
        );
    }

    #[test]
    fn slug_rules_and_duplicates() {
        let store = InMemoryTenantStore::new();
        let bad = TenantRecord {
            id: TenantId::new(),
            slug: "Bad_Slug".into(),
            name: "x".into(),
            plan: "standard".into(),
            status: TenantStatus::Trial,
            region: "in".into(),
            capabilities: BTreeSet::new(),
            created_at: OffsetDateTime::now_utc(),
            revision: TenantRevision(0),
        };
        assert!(store.upsert(bad).is_err());

        let a = TenantRecord {
            id: TenantId::new(),
            slug: "acme".into(),
            name: "Acme".into(),
            plan: "standard".into(),
            status: TenantStatus::Active,
            region: "in".into(),
            capabilities: BTreeSet::new(),
            created_at: OffsetDateTime::now_utc(),
            revision: TenantRevision(0),
        };
        store.upsert(a.clone()).unwrap();
        let mut b = a.clone();
        b.id = TenantId::new();
        assert!(store.upsert(b).is_err(), "slug collision rejected");
        let updated = store.upsert(a.clone()).unwrap();
        assert_eq!(updated.revision, TenantRevision(2), "upsert bumps revision");
    }

    #[test]
    fn plan_policy_enforces_quotas() {
        let policy = PlanPolicy::defaults();
        assert!(policy.check("standard", PlanMetric::Users, 49).is_ok());
        let err = policy.check("standard", PlanMetric::Users, 50).unwrap_err();
        assert_eq!(err.code, keel_foundation::codes::CAP_LIMIT);
        assert!(policy.limits("ghost-plan").is_err());
    }

    #[test]
    fn tenant_scope_isolates_services() {
        let scope = TenantScope::new();
        let t1 = TenantId::new();
        let t2 = TenantId::new();
        let a: Arc<std::sync::Mutex<u64>> = scope.get_or_init(t1, |_| std::sync::Mutex::new(1));
        let b: Arc<std::sync::Mutex<u64>> = scope.get_or_init(t1, |_| std::sync::Mutex::new(99));
        assert!(
            Arc::ptr_eq(&a, &b),
            "same tenant reuses the instance (init not re-run)"
        );
        let c: Arc<std::sync::Mutex<u64>> = scope.get_or_init(t2, |_| std::sync::Mutex::new(7));
        assert!(!Arc::ptr_eq(&a, &c));
        assert_eq!(scope.tenant_count(), 2);
        scope.evict(t1);
        assert_eq!(scope.tenant_count(), 1);
    }

    #[test]
    fn context_projection_matches_record() {
        let store = Arc::new(store_with_acme());
        let svc = TenantService::new(store);
        let rec = svc.resolve(&TenantHint::Slug("acme".into())).unwrap();
        let ctx = svc.context(&rec);
        assert_eq!(ctx.id, rec.id);
        assert_eq!(ctx.slug, "acme");
        assert_eq!(ctx.plan, rec.plan);
    }
}
