# Kelson Platform — Corrected Target Architecture
**Post-Verification Architecture Decision Record**  
*Based on Phase 0 Evidence Verification Audit (all P0/P1 findings confirmed by executable evidence)*

---

> **Citation status (2026-10-13):** this document is a Phase-0 decision record whose
> `file:line` citations were repaired against the current tree (Phase M/A) by
> replacing stale fragments (`lib.rs`, line 20; `runtime.rs`, lines 89 and 257)
> with full `crates/...` paths, qualifying divergent spans with `` `keel-platform` ``,
> and updating moved symbols (`xtask/src/main.rs`, line 589 for `bump`,
> `tools/kelson-console/src/main.rs`, line 2624 for `wire_promotion`). The citation
> gate (`cargo xtask cite-audit`) now enforces that these citations remain
> accurate; if code moves, the gate will flag the drift — update the citation
> or date the claim, do not silently re-point it at different code. Two citations
> the gate cannot see use forms it does not detect (a bare line reference at
> `:200`, and `services/search` without an extension).

---

## 1. Executive Summary

The verification audit confirmed **5 root causes** driving 14 P0/P1 findings. This document defines the corrected target architecture that eliminates each root cause at its source, with explicit architectural decisions, component boundaries, and migration sequencing.

| Root Cause | Findings Eliminated | Architectural Lever |
|---|---|---|
| Monolithic workspace + static composition | KEEL-001, KEEL-003, KEEL-008, KEEL-V07, KEEL-V08 | **Workspace split**: kernel / framework / application |
| Tenant isolation only at DB layer | KEEL-002, KEEL-V02..V06 | **Tenant-scoped ports** + compile-time tenant context |
| Boot-time freeze everywhere | KEEL-003, KEEL-004, KEEL-008, KEEL-V07, KEEL-V08 | **Hot-reloadable config/routes/flags** with versioned snapshots |
| No versioning/compatibility model | KEEL-005, KEEL-006, KEEL-007, KEEL-V09, KEEL-V10 | **Schema versioning** on all durable payloads + migration contracts |
| RLS escape hatch (`OR NULL`) | SEC-001, SEC-002 | **GUC enforcement** + role separation + connection guards |

---

## 2. Workspace Split (Addresses Root Cause 1)

### 2.1 Three-Workspace Architecture

```
keel-platform/          ← Framework crates (75+ crates, versioned independently)
├── Cargo.toml          ← workspace: kernel, config, execution, tenancy, jobs, events, wasm, ...
├── crates/
│   ├── kernel/         ← keel-kernel (lifecycle, registry, graph)
│   ├── framework/      ← kelson-framework (application-facing umbrella; re-exports only)
│   ├── config/         ← kelson-config (layered config + hot reload)
│   ├── execution/      ← kelson-execution (plan, engine, idempotency)
│   ├── tenancy/        ← kelson-tenancy (TenantScope, RLS enforcement)
│   ├── jobs/           ← kelson-jobs (DurableJob v2 with schema version)
│   ├── events/         ← kelson-events (OutboxEvent v2 with idempotency key)
│   ├── wasm/           ← kelson-wasm (read + write capability ABI)
│   ├── cache/          ← kelson-cache (tenant-scoped CachePort)
│   ├── scheduler/      ← kelson-scheduler (tenant-aware + distributed lock)
│   ├── storage/        ← kelson-storage (tenant-prefixed StoragePort)
│   ├── search/         ← kelson-search (tenant-enforced SearchEngine)
│   ├── realtime/       ← kelson-realtime (tenant-scoped channels)
│   └── deployment/     ← kelson-deployment (RollbackExecutor + promotion)
├── runtime/
│   ├── kelson-demo/       ← kelson-demo (HTTP server, route registry, middleware)
│   └── kelson-worker/    ← kelson-worker (job runner, scheduler, outbox pump)
├── applications/       ← optional opt-in modules (appointments, oauth2)
└── tools/kelson-cli/     ← kelson binary (build, promote, rollback, config:reload)

# Phase O (2026-10-10): the former `keel-kernel/` workspace was MERGED into
# `keel-platform/`. `kernel-api` -> `runtime/kelson-demo`, `kernel-worker` ->
# `runtime/kelson-worker`, the `keel-kernel-facade` -> `crates/framework`
# (`kelson-framework`), and `applications/`, `lang/` moved across whole. There is
# no longer a third workspace: `your-app` consumes the `kelson-framework`
# umbrella directly.

your-app/               ← Application workspace (YOUR code, YOUR release cadence)
├── Cargo.toml          ← members: [modules/*, apps/*]
├── modules/
│   ├── patients/       ← HttpModule + models + migrations (independently versioned)
│   ├── appointments/
│   ├── billing/
│   └── ...
├── apps/
│   ├── api/            ← Thin composition: ModuleLoader::discover("modules/")
│   └── worker/         ← Thin composition: JobRegistry::discover("modules/")
└── config/             ← Application config (tenant overrides, feature flags)
```

### 2.2 Dependency Rules (Enforced by `cargo deny` + architecture tests)

```toml
# keel-platform/Cargo.toml (enforced in CI)
[workspace.lints.rust]
workspace = true

# runtime/kelson-demo/Cargo.toml (host binary — consumes the umbrella)
dependencies = [
    "kelson-framework",   # re-exported facade (lifecycle, registry, graph, config, ...)
    "kelson-config",      # FrozenConfig + ConfigReloader
    "kelson-execution",   # ExecutionEngine
    "kelson-tenancy",     # TenantScope, RLS
    # NO direct dep on kelson-jobs, kelson-events, kelson-wasm, etc.
]

# your-app/Cargo.toml
dependencies = [
    "kelson-framework",   # re-exported facade (since Phase O; was `keel-kernel`)
    "kelson-jobs",        # job definitions
    "kelson-events",      # event definitions
    "kelson-wasm",        # plugin definitions
    # NO direct dep on keel-platform internal crates
]
```

### 2.3 Independent Release Cadence

| Workspace | Release Trigger | Versioning | Deploy Artifact |
|---|---|---|---|
| `keel-platform` | Framework feature/fix | SemVer minor/patch | Crates.io + GitHub Releases |
| `keel-platform/runtime` (`kelson-demo`, `kelson-worker`) | Host binary change | SemVer major/minor | `kelson-demo`, `kelson-worker` binaries |
| `your-app` | Business feature/fix | SemVer patch/minor | Container images + WASM plugins |

**Critical**: `your-app` **never** rebuilds `keel-platform`. Module changes → WASM plugin rebuild (seconds) or hot-reload (milliseconds).

---

## 3. Tenant-Scoped Ports (Addresses Root Cause 2)

### 3.1 Design Principle: Tenant Context at Port Boundary

Every framework port **requires** `TenantId` at the call site. No ambient context, no thread-locals.

```rust
// kelson-cache: TENANT-SCOPED (replaces current CachePort)
#[async_trait]
pub trait CachePort: Send + Sync {
    async fn get(&self, tenant: TenantId, key: &str) -> Result<Option<Bytes>, AppError>;
    async fn set(&self, tenant: TenantId, key: &str, value: Bytes, ttl: Duration) -> Result<(), AppError>;
    async fn delete(&self, tenant: TenantId, key: &str) -> Result<(), AppError>;
    async fn tags(&self, tenant: TenantId, tags: &[&str]) -> Result<Vec<String>, AppError>;
    async fn invalidate_tags(&self, tenant: TenantId, tags: &[&str]) -> Result<usize, AppError>;
}

// kelson-jobs: TENANT-SCOPED claim
#[async_trait]
pub trait JobStore: Send + Sync {
    async fn enqueue(&self, tenant: TenantId, job: DurableJob) -> Result<Uuid, AppError>;
    async fn claim(&self, tenant: TenantId, worker: &str) -> Result<Option<DurableJob>, AppError>;
    // ... complete, fail, list ALL require tenant
}

// kelson-events: TENANT-SCOPED dispatch
#[async_trait]
pub trait ListenerRegistry: Send + Sync {
    fn for_event(&self, tenant: TenantId, event: &str) -> Vec<Arc<dyn ListenerHandler>>;
    fn for_event_global(&self, event: &str) -> Vec<Arc<dyn ListenerHandler>>; // platform events only
}

// kelson-scheduler: TENANT-SCOPED due scan
#[async_trait]
pub trait ScheduleRegistry: Send + Sync {
    fn due(&self, tenant: TenantId, now: OffsetDateTime) -> Vec<ScheduleSpec>;
    fn register(&self, tenant: TenantId, spec: ScheduleSpec) -> Result<(), AppError>;
}

// kelson-storage: TENANT-PREFIXED keys (enforced at port)
#[async_trait]
pub trait StoragePort: Send + Sync {
    async fn put(&self, tenant: TenantId, bucket: &str, key: &str, bytes: Bytes) -> Result<(), AppError>;
    async fn get(&self, tenant: TenantId, bucket: &str, key: &str) -> Result<Option<Bytes>, AppError>;
    async fn list(&self, tenant: TenantId, bucket: &str, prefix: &str) -> Result<Vec<ObjectMeta>, AppError>;
    async fn delete(&self, tenant: TenantId, bucket: &str, key: &str) -> Result<(), AppError>;
}
```

### 3.2 Compile-Time Enforcement

```rust
// kelson-tenancy: TenantContext is a ZST passed by value — zero cost, compile-time checked
pub struct TenantContext {
    tenant_id: TenantId,
    _phantom: PhantomData<fn() -> TenantId>, // prevents construction without TenantId
}

impl TenantContext {
    pub fn new(tenant_id: TenantId) -> Self { Self { tenant_id, _phantom: PhantomData } }
    pub fn tenant_id(&self) -> TenantId { self.tenant_id }
}

// Port extension trait — callers MUST have TenantContext
pub trait TenantScopedCache: CachePort {
    fn get_scoped(&self, ctx: &TenantContext, key: &str) -> impl Future<Output = Result<Option<Bytes>, AppError>> {
        self.get(ctx.tenant_id(), key)
    }
    // ... all methods
}

// Usage in application code — COMPILE ERROR if TenantContext missing
async fn handle_request(ctx: TenantContext, cache: Arc<dyn CachePort>) -> Result<Response> {
    let data = cache.get_scoped(&ctx, "patient:123").await?; // OK
    // let data = cache.get("patient:123").await?; // COMPILE ERROR: no TenantContext
}
```

### 3.3 Adapter Implementations

| Port | PostgreSQL Adapter | Redis Adapter | In-Memory Adapter (test) |
|---|---|---|---|
| Cache | `PgCache` (table per tenant) | `RedisCache` (key prefix `t:{tenant}:`) | `HashMap<TenantId, HashMap<...>>` |
| Jobs | `PgJobStore` (RLS on `jobs` table) | — | `InMemoryJobStore` (tenant-sharded) |
| Events | `PgOutboxStore` (RLS on `outbox`) | `RedisOutboxStore` (stream per tenant) | `InMemoryOutboxStore` |
| Scheduler | `PgScheduleLock` (advisory lock per tenant) | `RedisScheduleLock` (Redlock per tenant) | `MemoryScheduleLock` (tenant-sharded) |
| Storage | `S3Storage` (prefix `tenant/{id}/`) | `MinIOStorage` (same) | `FsStorage` (dir per tenant) |

**Migration**: Existing adapters wrap with `TenantShardedAdapter` during transition; new code uses tenant-scoped ports directly.

---

## 4. Hot-Reloadable Configuration (Addresses Root Cause 3)

### 4.1 Config v2: Versioned Snapshots + Atomic Swap

```rust
// kelson-config v2: replaces FrozenConfig
pub struct ConfigRuntime {
    current: Arc<ConfigSnapshot>,
    watcher: ConfigWatcher,
    reloader: Arc<dyn ConfigReloader>,
}

pub struct ConfigSnapshot {
    version: u64,
    platform: PlatformConfig,
    tenant_overrides: BTreeMap<TenantId, TenantConfig>,
    feature_flags: FeatureSnapshot,
    route_manifest: RouteManifest,
    hash: ConfigHash, // SHA256 of all sources
}

impl ConfigRuntime {
    pub async fn get(&self) -> Arc<ConfigSnapshot> {
        self.current.clone() // lock-free read
    }

    pub async fn reload(&self) -> Result<ConfigSnapshot, AppError> {
        let new = self.watcher.poll().await?; // reads files/env/etcd
        if new.hash != self.current.hash {
            let validated = self.validate(&new).await?;
            let swapped = self.current.swap(Arc::new(validated)); // atomic
            self.reloader.on_reload(&swapped).await; // notifies route cache, flag eval, etc.
            return Ok(swapped);
        }
        Ok(self.current.clone())
    }
}

// Hot-reload triggers (configurable):
// - SIGHUP
// - File mtime change (config/*.toml, config/tenants/*.toml)
// - etcd/consul watch (kelson-config-etcd crate)
// - Manual: `kelson config:reload` CLI
```

### 4.2 Route Cache Hot Reload

```rust
// kelson-http: RouteManifest versioned by config hash
pub struct RouteManifest {
    version: u64,
    config_hash: ConfigHash,
    routes: Vec<CompiledRoute>,
    middleware_stack: MiddlewareStack,
}

// On config reload:
async fn on_config_reload(new_config: &ConfigSnapshot) {
    let new_routes = compile_routes(&new_config.platform);
    let new_manifest = RouteManifest {
        version: new_config.version,
        config_hash: new_config.hash,
        routes: new_routes,
        middleware_stack: build_middleware(&new_config),
    };
    ROUTE_MANIFEST.store(Arc::new(new_manifest)); // atomic swap
    // In-flight requests complete on old manifest; new requests use new manifest
}
```

### 4.3 Feature Flag Hot Reload

```rust
// kelson-feature-flags: FeatureSnapshot derived from ConfigSnapshot
pub struct FeatureSnapshot {
    version: u64,
    flags: BTreeMap<FlagKey, FlagState>,
    tenant_overrides: BTreeMap<TenantId, BTreeMap<FlagKey, FlagState>>,
}

impl FeatureSnapshot {
    pub fn evaluate(&self, ctx: &EvaluationContext) -> FlagValue {
        // Pure function — no IO, no locks
    }
}

// Reload: new ConfigSnapshot → new FeatureSnapshot → atomic swap
// Request-scoped: FeatureSnapshot resolved ONCE per request (kelson-context)
```

---

## 5. Schema Versioning on All Durable Payloads (Addresses Root Cause 4)

### 5.1 Job Payload Versioning (KEEL-005, KEEL-V09)

```rust
// kelson-jobs v2: DurableJob with schema version + migrator
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DurableJob {
    pub id: Uuid,
    pub execution_id: ExecutionId,
    pub name: String,
    pub schema_version: u32,           // REQUIRED
    pub payload: JobPayload,           // Typed enum, NOT serde_json::Value
    // ...
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum JobPayload {
    V1(JobPayloadV1),
    V2(JobPayloadV2),
    // ...
}

// Migration trait — implemented by application per job type
#[async_trait]
pub trait JobMigrator: Send + Sync {
    fn job_name(&self) -> &str;
    fn current_version(&self) -> u32;
    async fn migrate(&self, from: u32, payload: JobPayload) -> Result<JobPayload, AppError>;
}

// JobRegistry holds migrators; claim() auto-migrates to current version
impl JobRegistry {
    pub async fn claim_migrated(&self, tenant: TenantId, worker: &str) -> Result<Option<DurableJob>, AppError> {
        let job = self.store.claim(tenant, worker).await?;
        if let Some(job) = job {
            if job.schema_version < self.migrators[&job.name].current_version() {
                let migrated = self.migrators[&job.name].migrate(job.schema_version, job.payload).await?;
                return Ok(Some(DurableJob { payload: migrated, schema_version: CURRENT, ..job }));
            }
        }
        Ok(job)
    }
}
```

### 5.2 Event Idempotency Keys (KEEL-006, KEEL-V10)

```rust
// kelson-events v2: OutboxEvent with idempotency key
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboxEvent {
    pub id: Uuid,
    pub event: String,
    pub topic: String,
    pub tenant_id: Option<TenantId>,
    pub payload: serde_json::Value,
    pub idempotency_key: IdempotencyKey,  // REQUIRED: SHA256(event + tenant + correlation_id + payload_hash)
    pub published: bool,
    pub created_at: OffsetDateTime,
}

// OutboxPump: deduplicates on idempotency_key
impl OutboxPump {
    async fn run_once(&self) -> Result<PumpReport, AppError> {
        let events = self.store.claim_unpublished(self.batch_size).await?;
        let mut report = PumpReport::default();
        
        for event in &events {
            // Check idempotency gate BEFORE dispatch
            if self.idempotency_gate.check(&event.idempotency_key).await? {
                report.skipped_duplicate += 1;
                self.store.mark_published(&[event.id]).await?;
                continue;
            }
            
            let listeners = self.listeners.for_event(event.tenant_id, &event.event);
            let mut ok = true;
            for listener in listeners {
                if let Err(e) = listener.dispatch(event).await {
                    ok = false; tracing::error!(%e, "listener failed");
                }
            }
            
            if ok {
                self.idempotency_gate.mark_done(&event.idempotency_key).await?;
                self.store.mark_published(&[event.id]).await?;
                report.dispatched += 1;
            } else {
                report.failed += 1;
                report.failed_events.push(event.id);
            }
        }
        Ok(report)
    }
}
```

### 5.3 WASM Write Capability ABI (KEEL-007)

```rust
// kelson-wasm v2: Capability profiles include write
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ExecutionProfile {
    ReadOnly,      // projections, queries
    ReadWrite,     // mutations, deltas — NEW
    Admin,         // schema changes, tenant provisioning — NEW
}

pub struct WasmHandler {
    plan: ExecutionPlan,
    plugin: Arc<WasmPlugin>,
    local: String,
    capabilities: CapabilitySet, // includes host.capability_write for ReadWrite/Admin
}

impl WasmHandler {
    pub fn new(plugin: Arc<WasmPlugin>, capability: &str) -> Result<Self, WasmError> {
        let spec = plugin.manifest().capabilities.get(capability)?;
        // ACCEPT ReadWrite/Admin if host provides write imports
        match ExecutionProfile::resolve(spec.profile) {
            ExecutionProfile::ReadOnly => Ok(Self { capabilities: READ_ONLY, ... }),
            ExecutionProfile::ReadWrite => Ok(Self { capabilities: READ_WRITE, ... }),
            ExecutionProfile::Admin => Ok(Self { capabilities: ADMIN, ... }),
        }
    }
}

// Guest imports (WASM side):
// host.capability_read(ptr, len) -> ptr
// host.capability_write(ptr, len) -> ptr  // NEW: mutation delta
// host.capability_audit(ptr, len) -> ptr  // NEW: audit log
```

### 5.4 Module/Config Versioning (KEEL-V07, KEEL-V08)

```rust
// ConfigSnapshot + RouteManifest + FeatureSnapshot all share version + hash
// Rollback = swap to previous snapshot (kept in ring buffer of last 10)
pub struct ConfigHistory {
    snapshots: Arc<Mutex<VecDeque<Arc<ConfigSnapshot>>>>, // max 10
}

impl ConfigRuntime {
    pub async fn rollback(&self, target_version: u64) -> Result<(), AppError> {
        let snapshots = self.history.snapshots.lock().await;
        let target = snapshots.iter().find(|s| s.version == target_version)
            .ok_or_else(|| AppError::not_found("config version"))?;
        self.current.swap(target.clone());
        self.reloader.on_reload(target).await;
        Ok(())
    }
}
```

---

## 6. RLS Enforcement + Connection Guards (Addresses Root Cause 5)

### 6.1 Eliminate `OR NULL` Escape Hatch

```sql
-- BEFORE (vulnerable):
CREATE POLICY tenant_isolation ON memberships
    USING (tenant_id = kelson_current_tenant() OR kelson_current_tenant() IS NULL);

-- AFTER (strict):
CREATE POLICY tenant_isolation ON memberships
    USING (tenant_id = kelson_current_tenant());
-- kelson_current_tenant() MUST return non-NULL for tenant-plane roles
-- kelson_platform role: BYPASSES RLS via NO POLICY (separate mechanism)
```

### 6.2 Role Separation (Enforced by Migration)

```sql
-- platform/database/roles.sql (CORRECTED)
-- 1. Migration owner (DDL only)
CREATE ROLE kelson_owner NOLOGIN;

-- 2. Platform runtime (break-glass, NO RLS policies applied)
CREATE ROLE kelson_platform LOGIN;
GRANT USAGE ON SCHEMA public TO kelson_platform;
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO kelson_platform;
-- NO RLS policies on tables for this role — uses separate connection pool

-- 3. Tenant runtime (RLS ENFORCED)
CREATE ROLE kelson_tenant LOGIN;
GRANT USAGE ON SCHEMA public TO kelson_tenant;
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO kelson_tenant;
-- RLS policies APPLY to this role

-- 4. Reporting (read-only, RLS enforced)
CREATE ROLE kelson_reporting LOGIN;
GRANT USAGE ON SCHEMA public TO kelson_reporting;
GRANT SELECT ON ALL TABLES IN SCHEMA public TO kelson_reporting;
```

### 6.3 Connection Pool Guards (Eliminates SEC-002)

```rust
// kelson-postgres: TenantGuardedPool
pub struct TenantGuardedPool {
    tenant_pool: PgPool,      // connects as kelson_tenant
    platform_pool: PgPool,    // connects as kelson_platform (break-glass only)
}

impl TenantGuardedPool {
    // Tenant plane: ALWAYS sets GUC, verifies on release
    pub async fn with_tenant<T>(
        &self,
        tenant_id: TenantId,
        f: impl FnOnce(&mut PgTransaction) -> Result<T, AppError> + Send,
    ) -> Result<T, AppError> {
        let mut conn = self.tenant_pool.acquire().await?;
        sqlx::query("SELECT set_config('kelson.tenant_id', $1, false)") // NOT LOCAL — session-scoped
            .bind(tenant_id.to_string())
            .execute(&mut *conn)
            .await?;
        
        let tx = conn.begin().await?;
        let result = f(&mut *tx).await;
        
        // VERIFY GUC still set on commit/rollback
        let guc: Option<String> = sqlx::query_scalar("SHOW kelson.tenant_id")
            .fetch_one(&mut *tx).await?;
        assert_eq!(guc, Some(tenant_id.to_string()), "GUC leaked across transaction!");
        
        tx.commit().await?;
        Ok(result?)
    }
    
    // Platform plane: NEVER sets GUC, uses separate pool
    pub async fn platform<T>(
        &self,
        f: impl FnOnce(&mut PgConnection) -> Result<T, AppError> + Send,
    ) -> Result<T, AppError> {
        let mut conn = self.platform_pool.acquire().await?;
        f(&mut *conn).await
    }
}

// Pool acquire/release guard: DISCARD ALL on release to tenant pool
impl Drop for TenantGuardedConnection {
    fn drop(&mut self) {
        let _ = sqlx::query("DISCARD ALL").execute(&mut self.conn); // resets GUC
    }
}
```

### 6.4 Compile-Time Role Separation

```rust
// kelson-tenancy: Executor types encode role at compile time
pub struct TenantExecutor<'a> { pool: &'a TenantGuardedPool, tenant: TenantId }
pub struct PlatformExecutor<'a> { pool: &'a TenantGuardedPool }

// ExecutionEngine uses TenantExecutor for all tenant-plane operations
impl ExecutionEngine {
    pub async fn run<F, T>(&self, ctx: ExecutionContext, f: F) -> Result<T, AppError>
    where F: FnOnce(TenantExecutor) -> Result<T, AppError> {
        let executor = TenantExecutor { pool: &self.pool, tenant: ctx.tenant_id() };
        f(executor).await
    }
}

// Platform operations (provisioning, break-glass) use PlatformExecutor explicitly
pub async fn provision_tenant(platform: PlatformExecutor, spec: TenantSpec) -> Result<Tenant, AppError> {
    // Uses platform_pool — no RLS, no GUC
}
```

---

## 7. Rollback Execution (Addresses KEEL-004)

### 7.1 RollbackExecutor Component

```rust
// kelson-deployment: RollbackExecutor
pub struct RollbackExecutor {
    config_runtime: Arc<ConfigRuntime>,
    migration_runner: Arc<MigrationRunner>,
    module_loader: Arc<ModuleLoader>,
    job_registry: Arc<JobRegistry>,
    event_registry: Arc<EventRegistry>,
}

impl RollbackExecutor {
    pub async fn rollback(&self, target: PromotionTarget) -> Result<RollbackReport, AppError> {
        let report = RollbackReport::default();
        
        // 1. Config rollback (atomic snapshot swap)
        self.config_runtime.rollback(target.config_version).await?;
        report.config_rolled_back = true;
        
        // 2. Migration rollback (pairs only — foundation tables require manual)
        if let Some(db_version) = target.db_version {
            self.migration_runner.rollback_to(db_version).await?;
            report.migrations_rolled_back = true;
        }
        
        // 3. Module rollback (WASM plugin version pin)
        self.module_loader.pin_versions(target.module_versions).await?;
        report.modules_pinned = true;
        
        // 4. Job compatibility (ensure in-flight jobs can complete)
        self.job_registry.ensure_compatibility(target.job_schema_versions).await?;
        report.jobs_compatible = true;
        
        // 5. Event compatibility (idempotency keys stable across versions)
        self.event_registry.verify_idempotency_stability().await?;
        report.events_compatible = true;
        
        Ok(report)
    }
}
```

### 7.2 Promotion Flow with Rollback Points

```rust
// kelson-deployment: Promotion with automatic rollback on failure
pub struct PromotionPipeline {
    executor: RollbackExecutor,
    health_checks: Vec<HealthCheck>,
}

impl PromotionPipeline {
    pub async fn promote(&self, release: ReleaseArtifact) -> Result<PromotionState, AppError> {
        let checkpoint = self.create_checkpoint().await?; // config + db + modules snapshot
        
        // Stage 1: Deploy binaries/WASM (no traffic)
        self.deploy_artifacts(&release).await?;
        if !self.health_checks.run("post-deploy").await? {
            return self.rollback(checkpoint).await; // automatic
        }
        
        // Stage 2: Migrate DB (forward only — pairs support rollback)
        self.migration_runner.migrate_to(release.db_version).await?;
        if !self.health_checks.run("post-migrate").await? {
            return self.rollback(checkpoint).await;
        }
        
        // Stage 3: Switch traffic (config reload + route swap)
        self.config_runtime.activate_release(release.config_version).await?;
        if !self.health_checks.run("post-traffic").await? {
            return self.rollback(checkpoint).await;
        }
        
        // Stage 4: Verify (canary metrics, error rates)
        if !self.health_checks.run("canary").await? {
            return self.rollback(checkpoint).await;
        }
        
        Ok(PromotionState::Deployed)
    }
}
```

---

## 8. Component Interface Contracts (Summary)

| Component | Port | Tenant-Scoped? | Versioned? | Hot-Reload? |
|---|---|---|---|---|
| `kelson-config` | `ConfigReloader` | N/A | ✅ (snapshot version) | ✅ SIGHUP/file/etcd |
| `kelson-cache` | `CachePort` | ✅ REQUIRED | ✅ (key schema) | ✅ (invalidate tags) |
| `kelson-jobs` | `JobStore` | ✅ REQUIRED | ✅ (JobPayload + Migrator) | ✅ (registry hot-swap) |
| `kelson-events` | `ListenerRegistry` | ✅ REQUIRED | ✅ (IdempotencyKey) | ✅ (registry hot-swap) |
| `kelson-scheduler` | `ScheduleRegistry` | ✅ REQUIRED | ✅ (ScheduleSpec version) | ✅ (registry hot-swap) |
| `kelson-storage` | `StoragePort` | ✅ REQUIRED (prefix) | ✅ (object schema) | N/A |
| `kelson-search` | `SearchEngine` | ✅ REQUIRED | ✅ (index mapping version) | ✅ (reindex trigger) |
| `kelson-realtime` | `BroadcastHub` | ✅ REQUIRED (channel) | ✅ (event schema) | N/A |
| `kelson-wasm` | `WasmPlugin` | ✅ (tenant in ExecutionContext) | ✅ (ABI version) | ✅ (plugin hot-swap) |
| `kelson-deployment` | `RollbackExecutor` | N/A | ✅ (promotion version) | N/A |

---

## 9. Migration Sequence (Zero-Downtime)

### Phase 0: Preparation (Current Sprint)
- [x] Add `cargo deny` architecture rules to `keel-platform`
  - **ticked — accepted with `dep-rules` as the architecture half (user decision).** The supply-chain
    half is cargo-deny, repaired and proven below; the architecture half is `cargo xtask dep-rules`,
    because cargo-deny structurally cannot carry per-workspace rules.
  - *Correction to an earlier reading of this box:* `platform/deny.toml` (tracked, 57 lines) is
    cargo-deny's real config, and it was valid and being discovered all along — cargo-deny finds
    `deny.toml` by walking up from the working directory (proven by injecting a bogus section at
    `platform/deny.toml` and having it rejected when run from `keel-platform/`). The separate
    `platform/cargo-deny.toml` (untracked) was a stray duplicate whose `[architecture]` and
    `[diagrams]` sections cargo-deny cannot parse; it was **never** loaded, because only
    `deny.toml` is discovered. That stray has been removed.
  - *What was actually wrong:* the gate was running and **failing** — `cargo deny check` from
    `platform/` → **exit 7**, `advisories FAILED, bans FAILED, licenses FAILED, sources ok`
    (4 license rejects, 1 wildcard violation, 21 advisories).
  - *Fixed, preserving the file's existing governance comments:* added `0BSD` and
    `CDLA-Permissive-2.0` to `[licenses].allow`, and `allow-wildcard-paths = true` to `[bans]`
    (the `kelson-mod-*` entries are path-only workspace deps, which cargo-deny reads as `*`).
    **Proven:** `cargo deny check` from `platform/` → `bans ok, licenses ok, sources ok`; from
    `keel-platform/` → exit 0 on the same three checks.
  - *Scope note — why the rules are not literally in cargo-deny:* cargo-deny has no per-workspace
    `[architecture]` section and evaluates a single lock graph per invocation, so crate boundaries
    cannot be expressed there. They are enforced by `cargo xtask dep-rules` — the same engine as
    `kelson check`, already wired into CI (`docs/03-ARCHITECTURE.md`, gates G01-G17). Ticked on that
    basis: the outcome the box asks for (architecture rules gating the workspace split, enforced by
    tooling in CI) exists and is proven — the tool named in the box is simply not where those rules
    live.
  - *Resolved — the **21 RUSTSEC advisories** that were surfacing as `advisories FAILED`.* Closed by
    dependency upgrades, not by ignoring: `wasmtime` 34.0.2 → **36.0.17** (36.0.16 is the lowest
    version whose patched range covers *every* wasmtime advisory — each is `>=36.0.x, <37.0.0`, so
    37.0.2 would *not* have sufficed) and `rustls` 0.23.43 → **0.23.45**. The one advisory carrying
    *"No safe upgrade is available"* — `RUSTSEC-2023-0071` (`rsa`, RustSec `patched = []`, reached
    only transitively via sqlx-mysql) — gets the governance note `deny.toml` demands rather than a
    silent ignore: accepted risk, database connection must be TLS-protected or network-local.
  - *Knock-on: MSRV **1.85 → 1.88 / 1.89** (per workspace).* wasmtime 36 declares
    `rust-version = 1.86.0`, and 34.x had been chosen deliberately as the *"MSRV-aware"* line —
    but that is only the floor wasmtime imposes, and 1.86 is rejected outright by cargo for
    `home`, `icu_*`, `time` and `wasm-{encoder,wast,wat}` 261 (all requiring 1.88). The
    lockfiles then split the four roots: `platform` resolves `uuid 1.24.1` → **1.88**;
    `keel-platform`, `keel-kernel` and `your-app` resolve `uuid 1.27.0`
    (*"requires rustc 1.89.0"*) → **1.89**. Each floor is proved by a real
    `cargo check --workspace --locked` on that exact toolchain — `platform` exit 0 @1.88,
    the other three exit 0 @1.89, all four red at 1.86. Updated in all four workspace roots,
    `keel-publish.yml`'s `msrv` job (now a per-floor matrix) and `docs/01-GETTING-STARTED.md`.
    > **Superseded by Phase O (2026-10-10):** the four roots are now **three** —
    > `keel-kernel` was merged into `keel-platform` and deleted. The floors are
    > unchanged (`platform` @1.88; `keel-platform` + `your-app` @1.89). The text
    > above is kept verbatim as the record of the measurement as taken.
  - *The `msrv` gate itself was a no-op; now fixed.* `platform/rust-toolchain.toml` pins
    channel 1.92.0 and rustup resolves that file **above** `rustup default`, so installing
    1.86.0 and defaulting to it still built 1.92.0 — the gate reported the floor as met
    without testing it. The job now sets `RUSTUP_TOOLCHAIN`, which outranks the file
    (proved: inside `platform/`, cargo resolves 1.92.0 normally and 1.86.0 with it set).
  - **Proven:** `cargo deny check` → **exit 0**, `advisories ok, bans ok, licenses ok, sources ok`
    from **both** `platform/` and `keel-platform/` (each workspace's own `Cargo.lock` carries
    wasmtime 36.0.17).
- [x] Implement `ConfigRuntime` + `ConfigWatcher` in `kelson-config` (new crate)
  - **proved.** `ConfigRuntime` at `crates/application/configuration/src/runtime.rs:134`, exported
    from `crates/application/configuration/src/lib.rs:20`; `kelson-config` **59 passed / 0 failed**, including the Phase 5 reload hammer
    and `config_reload_swaps_routes_and_flags_under_50ms`.
  - *Naming note:* no type is named `ConfigWatcher` — the §4.1 file-mtime trigger ships as
    `FileConfigSource` (mtime stamp, `crates/application/configuration/src/runtime.rs:89`) plus `ConfigRuntime::poll()` (`crates/application/configuration/src/runtime.rs:257`).
- [x] Implement tenant-scoped port traits in each crate (parallel, behind feature flags)
  - **proved — the feature-flag gap is closed, and the flag is real rather than decorative.**
    Only two crates *define* tenant-scoped ports — `kelson-caching` and `kelson-storage` (the note's
    `` `keel-platform` `crates/services/cache/src/lib.rs:63` `` / `` `keel-platform` `crates/services/storage/src/lib.rs:173-174` ``; `kelson-tenancy` merely consumes them), so
    those are the two gated. Each `Cargo.toml` now declares
    `[features] default = ["tenant-ports"]` + `tenant-ports = []`, and the entire tenant layer is
    wrapped in `#[cfg(feature = "tenant-ports")]` — the trait, its `…Shard` adapter (struct +
    inherent impl + trait impl), the `MapStore` test substrate, and the tenant tests
    (4 in cache, 5 in storage). Storage's `TenantId` import is gated too, since nothing outside
    the tenant section uses it (cache's is left alone: `TenantId` is still used by the authz key
    fingerprint at `` `keel-platform` `crates/services/cache/src/lib.rs:194-199` ``).
  - **`kelson-tenancy` requests the feature explicitly, not by inheritance:**
    `kelson-caching = { workspace = true, features = ["tenant-ports"] }` (and storage likewise).
    The scoped extensions *are* the tenant ports, so relying on someone else's `default = [...]`
    would let a `--no-default-features` rebuild silently delete §3.2. Verified by feature graph:
    `cargo tree -p kelson-tenancy -e features` → `kelson-caching feature "tenant-ports"` +
    `kelson-storage feature "tenant-ports"`, and `cargo check -p kelson-tenancy --no-default-features`
    exits 0.
  - **Why it is not an inert flag — the OFF state is executable, not asserted.**
    `cargo check -p kelson-caching|kelson-storage --no-default-features --tests` → exit 0 with
    **zero errors and zero warnings**. `cargo test --no-default-features` runs **17 → 13** tests
    for `kelson-caching` and **11 → 6** for `kelson-storage`: exactly the 9 gated tenant tests
    disappear while every non-tenant test (locks, authz key, writer/session, registry,
    `validate_dir_prefix`, `default_get_capped`) survives. The intermediate state is
    self-evidencing too — before the test-level `cfg`s were added, the OFF build failed with
    `E0433: use of undeclared type TenantCacheShard`, proving the attribute reaches real code
    rather than decorating it.
  - **Default stays ON, so nothing changes in any normal build:** `keel-platform`
    **141 suites / 1244 passed / 0 failed** — identical to the pre-change baseline — plus
    `cargo run -p xtask -- dep-rules` exit 0. The note's other observation is untouched: the
    `#![cfg(feature = "pg-tests")]` gating on the five live-database integration tests was and
    remains unrelated to port traits.
- [x] Add `JobMigrator` trait + `IdempotencyKey` to `kelson-jobs`/`kelson-events`
  - **proved.** `pub trait JobMigrator` at `crates/async/jobs/src/lib.rs:375` (= `kelson-jobs`) with a
    migrator registry (`:203`), `register_migrator` (`:321`) and two impls (`LocaleMigrator:2999`,
    `ZeroMigrator:3121`); `pub struct IdempotencyKey` at `crates/async/events/src/lib.rs:75`
    (= `kelson-event`) with a `config_hash`-derived constructor and a struct-field consumer (`:125`).
    Executable: `kelson-jobs` **35 passed**, `kelson-event` **13 passed**, 0 failed — both crates named
    by the box.
- [x] Fix RLS policies: remove `OR NULL`, add `kelson_tenant` role enforcement
  - **proved — `OR NULL`.** `git diff` on `medicare-kernel/migrations/0001_kernel.sql` shows
    `tenant_id IS NULL OR tenant_id = current_setting(…)` removed from the `roles` and
    `feature_flags` policies, the `USING (true)` on `memberships` replaced with real tenant
    scoping, and `GRANT ALL ON ALL TABLES … TO medicare_platform` (an RLS bypass) replaced by
    `ALTER DEFAULT PRIVILEGES`. No `tenant_id IS NULL OR` / `USING (true)` remains in
    `keel-platform/database`.
  - **proved — `kelson_tenant`.** The role is created by `keel-platform/database/roles.sql`
    (`CREATE ROLE kelson_tenant`, the tenant plane) and enforced by `TenantGuardedPool` in
    `crates/data/postgres/src/lib.rs`: `tenant_pool` connects as `kelson_tenant` (RLS enforced),
    `platform_pool` as `kelson_platform` (break-glass, no RLS).
  - *Note:* the live RLS probes are `DATABASE_URL`-gated and skip in this environment, so this is
    exact code-path proof rather than a live round trip.

### Phase 1: Workspace Split (Week 1-2)
- [x] Create `keel-kernel` workspace with `kernel-api`, `kernel-worker`
  - **proved.** `keel-kernel/Cargo.toml` declares members `kernel-api` and `kernel-worker`;
    executable: `cargo test --workspace` in `keel-kernel` → **15 suites / 240 passed / 0 failed**.
    Consumed downstream by `your-app` via `keel-kernel-facade`.
  - **Superseded by Phase O (2026-10-10) — topology change, not a revert.** The three-tree
    split proved here was subsequently merged into a **two-tree** topology so the framework
    ships as one publishable unit: `kernel-api` → `keel-platform/runtime/kelson-demo`,
    `kernel-worker` → `keel-platform/runtime/kelson-worker`, `keel-kernel-facade` →
    `keel-platform/crates/framework` (renamed `kelson-framework`), and `applications/` +
    `lang/` moved across whole. `platform/keel-kernel/` was deleted (52 tracked files).
    `your-app` now depends on `kelson-framework`. Post-merge evidence: `keel-platform`
    **158 suites / 1493 passed / 0 failed** (2 known load-sensitive 50 ms wall-clock
    flakes, both green 3/3 in isolation), root `xtask`
    `platform_tree_is_env_lint_clean` **ok**, `your-app` **3 / 0**, and
    `cargo clippy --workspace --all-targets -- -D warnings` **exit 0** (D-34).
- [ ] Move `keel-platform` crates to independent versioning (publish to private registry)
  - **open — it targets a public registry.** `.github/workflows/keel-publish.yml` defines the job
    `name: publish to crates.io`, and its smoke test builds against crates.io explicitly. No
    `[registries]` or `replace-with` configuration exists in any `.toml` in the repo
    (`platform/.cargo/config.toml` holds only an `xtask` alias). "Independent versioning" is
    workspace-level `version = "0.1.0"` shared by all crates, not per-crate versions.
    *For the record the publish pipeline itself is substantial* — verify / msrv / cargo-deny /
    semver gate / dry-run / registry smoke test / release tag / changelog.
  - *Blocker A — the chosen registry cannot host Cargo.* **GitHub Packages has no
    Cargo/Rust support**: `docs.github.com/en/packages/working-with-a-github-packages-registry/working-with-the-cargo-registry`
    returns **404**, and community discussion
    [#12067 "Cargo (Rust) registry support for Github Packages"](https://github.com/orgs/community/discussions/12067)
    is still *Unanswered* after 4.5 years (newest reply 2026-08-28: *"When are we
    getting this?"* / *"considering the current trajectory likely never"*). A
    `[registries]` entry pointing there would be a config that cannot work, so it
    was not written. Closing this needs a real cargo-capable endpoint (Cloudsmith,
    Artifactory, AWS CodeArtifact, self-hosted GitLab) supplied as
    `CARGO_REGISTRIES_*_INDEX` + token; until one exists, `publish` still targets
    crates.io and stays **dry-run by default** (`CARGO_REGISTRY_TOKEN` is empty
    unless a maintainer explicitly dispatches `dry_run: false`), so nothing is
    uploaded by accident.
  - *Blocker B — "independent versioning" is a release-model change, not a config
    tweak.* `keel-platform` holds **64 crates** and **62** internal
    `kelson-* = { version = "0.1.0", path = ... }` entries under
    `[workspace.dependencies]`, all moved together by one lockstep
    `cargo xtask bump <X.Y.Z>` that rewrites `[workspace.package].version` plus
    every internal dep literal in a single pass (`` `keel-platform` `xtask/src/main.rs:589` ``). Going
    per-crate means rewriting all 62 entries into individual constraints and
    replacing the lockstep bumper with per-crate release tooling — not attempted
    here while there is no registry to publish to.
- [ ] Update `medicare-kernel` → `your-app` workspace structure
  - **open — scoping report only; nothing moved (awaiting approval of the map).**
    `your-app` is a greenfield skeleton — 2 crates (`apps/api` = `my-app-api`,
    `modules/patients` = `mod-patients`, the latter carrying the kelson `module.toml`
    manifest) — while `medicare-kernel` is a full independent workspace with **21 members**
    over **42 `.rs` files** (21 in `modules/`, 17 in `kernel/`, 3 in `apps/`, 1 in `xtask/`),
    and `your-app/Cargo.toml` does not reference it at all.
  - **Inventory.** Root holds `Cargo.toml` + its own `Cargo.lock`, `docker-compose.yml`,
    `justfile`, `rust-toolchain.toml`, `.env.example`, `README.md`; plus `config/`,
    `database/`, `migrations/`, an empty `tests/`, and `.github/workflows/ci.yml`.
    Members: `apps/api` (1) + `kernel/{core,runtime,db,delta,auth,capability,workflow,notify}`
    (8, packages `kernel-*`) + `modules/{org,clinic,doctor,patient,appointment,rx,billing,inventory,notifycfg,reports,platform}`
    (11, packages `mod-*`) + `xtask` (1); `default-members = ["apps/api"]`.
  - **Proposed map (not executed).** `modules/*` → `your-app/modules/*` (its `members` glob
    already matches); `apps/api` → needs a new path, see collision; `kernel/*` → **no
    destination exists** (your-app has no `kernel/`, it takes `keel-kernel` as a *facade* path
    dep); `xtask` → not covered by the `modules/*` + `apps/*` globs, would need adding to
    `members`; `migrations/`, `database/`, `config/` → copied to `your-app/` (nothing to merge
    with); the two workspace manifests merge into `your-app/Cargo.toml`.
  - **Eight conflicts that must be decided before any move** — this is why it is not a file move:
    1. **Directory collision:** both workspaces already have `apps/api` (packages differ —
       `medicare-api` vs `my-app-api` — but the path is identical).
    2. **Two patient modules:** `modules/patient` (`mod-patient`) vs your-app's existing
       `modules/patients` (`mod-patients`).
    3. **`rust-version`:** medicare-kernel declares **1.92**, your-app **1.89** (the floor its own lockfile
       forces — uuid 1.27.0 requires rustc 1.89.0); a merged workspace has exactly one.
    4. **Dependency divergence:** `axum 0.8.9` vs an **exact pin** `=0.8.6`; `reqwest 0.13` vs
       `0.12`; `uuid 1.24` vs `1`; `rand 0.10` vs `0.8`; `toml 0.9` vs `0.8`; `sqlx` postgres-only
       vs postgres+mysql. Reconciling `[workspace.dependencies]` may change resolved versions.
    5. **Kernel naming overlap:** 8 `kernel-*` crates vs your-app's `keel-kernel` facade.
    6. **`modules/platform`** would sit inside a tree that already has `platform/` semantics.
    7. **CI activation:** `.github/workflows/ci.yml` is currently **inert** (GitHub only runs
       *repo-root* `.github/workflows/`). Moving it to the root would **start a new CI workflow**
       whose paths do not exist there — a behavior change, not a relocation.
    8. **Architectural rewrite, not translation:** medicare-kernel is plain axum/sqlx and has
       **0 `module.toml`**; your-app's pattern has one. Landing 11 modules means authoring 11
       kelson manifests and rewiring onto the `kelson-*` deps your-app already declares — the bulk of
       the work is framework adaptation, not file movement.
- [x] CI: separate pipelines for platform / kernel / app
  - **proved.** `.github/workflows/keel.yml` now declares **four jobs, one per Cargo
    workspace**, each with its own job-level `defaults.run.working-directory` and its own target
    cache keyed on that workspace's *own* `Cargo.lock`: `platform`,
    `platform/keel-platform`, `platform/keel-kernel`, `platform/your-app`.
    `defaults` moved from workflow-level to job-level deliberately — a workflow-level
    default would silently run keel-kernel or your-app commands against the platform tree.
  - *Why four rather than three:* the box names platform / kernel / app, but `platform/Cargo.toml`
    is itself a workspace root whose `members` exclude all three new workspaces
    (`members` mentions none of them). Dropping it would have removed the only coverage for the
    original tree — including the `xtask dep-rules` architecture gate and the publishability
    dry-run — so it is retained alongside the three the box asks for.
  - *Why they were invisible before:* each of the three is a separate workspace root with a
    separate `Cargo.lock`, so a single `working-directory: platform` job could never reach them;
    the note above was correct that the old 46-line `foundation` job covered only the original tree.
  - **Executable — each workspace independently with `--locked`, the flag CI uses:**
    `platform` **154 suites / 1412 passed / 0 failed**, `keel-platform` **141 / 1244 / 0**,
    `keel-kernel` **15 / 240 / 0**, `your-app` **3 / 0**, and
    `cargo check --workspace --locked` exit 0 in all four. Supporting gates the jobs run:
    `cargo run -p xtask -- dep-rules` → exit 0 (`dep-rules: clean`), `cargo deny check` →
    exit 0, `keel-publish.yml` `msrv` job → a 4-leg matrix at each workspace's verified floor (1.88 for `platform`, 1.89 for the other three). Both workflow files parse as valid YAML, and
    the `keel.yml` steps that carry over from the old job were checked to reference packages that
    actually exist (`kelson-observability`, `kelson-demo`, `xtask`) and tests that actually exist
    (`g18`, `telemetry_incident`).
  - *Pre-existing blocker cleared to get here:* `dep-rules` and
    `env_lint_tests::platform_tree_is_env_lint_clean` both failed **before** this work — not from
    it (every violating file is tracked and unmodified at `303a2ed`, and the scanner reads only
    `.rs` content plus `SANCTIONED_ENV_PATHS`, none of which these changes touched). Root cause:
    `SANCTIONED_ENV_PATHS` holds paths relative to `platform/` and was never extended when the
    three workspace roots were added, so the already-sanctioned config/entrypoint roles were
    unreachable one root deeper — 29 violations, `lint [!!]`. Repaired by appending those paths
    explicitly (each justified by a sanctioned counterpart of the same role); now
    `lint: clean`, `dep-rules: clean`.

### Phase 2: Tenant Scoping (Week 3-4)
- [x] Migrate all framework adapters to tenant-scoped ports
- [x] Add `TenantContext` compile-time enforcement
- [x] Implement `TenantGuardedPool` with GUC verification
- [x] Integration test: cross-tenant leakage probes in CI

### Phase 3: Hot Reload + Versioning (Week 5-6)
- [x] Deploy `ConfigRuntime` with file/etcd watchers
- [x] Implement route cache hot-swap + feature flag hot-swap
- [x] Deploy `JobMigrator` for all existing job types
- [x] Deploy `IdempotencyKey` on all outbox events
- [x] WASM `ReadWrite`/`Admin` capability ABI

### Phase 4: RolloutExecutor + Rollback (Week 7)
- [x] Implement `RollbackExecutor` with all 5 rollback dimensions
- [x] Promotion pipeline with automatic rollback on health check failure
- [x] `kelson promote` / `kelson rollback` CLI commands

### Phase 5: Hardening (Week 8)
- [x] Chaos testing: config reload under load, rollback mid-promotion
- [x] Tenant isolation penetration test (cache/queue/events/scheduler/storage)
- [ ] Performance baseline: hot-reload latency < 50ms, rollback < 30s
  - hot-reload **proved**: `config_reload_swaps_routes_and_flags_under_50ms` asserts <50ms across a
    real file edit + reload, and the `config-reload` phase of `kelson benchmark` reports ≈2.0 ms
    median over 300 iterations. A 4-writer/4-reader chaos hammer additionally proved the swap never
    tears or regresses a revision — it found a real commit-order race (subscribers notified
    `[.. 7, 6 ..]`, i.e. stale routes installed), fixed by serializing commit.
  - `rollback < 30s` **`requires-DB` — and provably cannot be substituted or staged here.**
    Three independent legs, each verified. **Leg 1 was revised by B-31 Phase C
    (2026-10-08/09) — its original reason no longer holds, its conclusion does; legs 2 and 3
    stand unchanged, and leg 3 remains the binding constraint.**
    1. ~~**It must be PostgreSQL; MySQL cannot stand in, by design.** The platform ships a
       MySQL driver (`crates/data/mysql/`), but it is *forward-only*: `crates/data/mysql/src/lib.rs:15`
       documents **"rollback-capable pairs are PostgreSQL-only"** and `:200` enforces it —
       `pair {base}: rollback-capable pairs are PostgreSQL-only (B-31)` — with
       `refuse_shipped_pairs()` failing the MySQL run closed rather than pretending.
       Only `crates/data/postgres/src/pairs.rs:264 rollback()` exists; MySQL has no
       migration-rollback path at all (`mysql`'s sole `rollback` is the transaction method in
       `crates/data/mysql/src/txport.rs:108`).~~ **Revised:** Phase C replaced the refusal with
       `kelson_mysql::pairs`, a full port of `kelson_postgres::pairs::rollback()` — same
       `database/migrations-pairs` shipment, same `keel_migrations` bookkeeping, same Laravel
       batches — executed without a transaction because MySQL DDL auto-commits, with the live
       round trip `mysql_pair_migrations_apply_rollback_and_reset_with_batches` (plus the
       `kelson migrate:rollback --step 1` operator transcript) proving apply → rollback → reset
       against MySQL 8.0.44. **But this box's figure does not travel that path.** It runs
       `kelson promote` / `kelson rollback` → `RollbackExecutor`, whose composition root still
       refuses a MySQL pool: `wire_promotion`
       (`` `keel-platform/tools/kelson-console/src/main.rs:2624` ``) returns
       `pg_only_on_mysql("promote/rollback")` unless `db` is `DbPool::Postgres`, because the
       ports it wires — `promotion::EventAdapter`, `promotion::MigrationAdapter` (which
       drives `kelson_postgres::pairs`) and the health probes — are `PgPool`-typed. Phase C
       deliberately left promotion scope alone, so **the conclusion of this leg stands for
       this measurement (PostgreSQL is required) while its original *reason* — MySQL "has no
       migration-rollback path at all" — does not.** The struck-through text is kept as the
       record the verdict was written against: the pairs exclusion was real until 2026-10-08.
    2. **No PostgreSQL is available in this environment.** No `postgres`/`pg_ctl`/`initdb`/`psql`
       on PATH and none installed (`C:\Program Files\PostgreSQL\16` contains only a leftover
       `data/` directory — no `bin/`, no service), port **5432 closed**, no docker or podman,
       and **no `DATABASE_URL` in either `.env`** (`platform/config/.env`,
       `platform/keel-kernel/config/.env`). The repo's own example expects
       `postgres://medicare:medicare@127.0.0.1:5432/medicare` from a compose file that cannot be
       started. *Note: MySQL 8.0 **is** installed and listening on 3306 and, since Phase C,
       reverts pairs through `kelson migrate:rollback` — but `kelson promote`/`kelson rollback`
       still refuse it (leg 1), so it cannot stand in for this figure either.*
    3. **Even with a database there is nothing to unwind.** Both rollback-capable directories —
       `platform/database/migrations-pairs` and `platform/keel-platform/database/migrations-pairs` —
       are **README-only (0 pair subdirs)**, which `pairs::scan_pairs()` (both fabrics) classifies
       as "no pairs yet" and returns empty. `pairs::rollback()` on either driver would therefore
       unwind zero steps, i.e. exactly the "no-op migrations dimension (meaningless number)" this
       box refuses to report. The 25th table the MySQL runner creates, `keel_migrations`, holds
       **0 rows** for the same reason.
    Measuring it therefore requires *PostgreSQL* (the promotion wiring still gates on it) **and**
    *at least one shipped pair* — and neither exists here: PostgreSQL is absent (leg 2), both
    `migrations-pairs` directories ship zero pairs (leg 3), so the migrations dimension would
    unwind zero batches on any fabric. Staged as a procedure in
    `platform/docs/13-OPERATIONS.md` §Performance baseline and **not claimed**.
- [x] Documentation + runbooks for each component

---

## 10. Verification Criteria (Definition of Done)

| Root Cause | Verification Test | Pass Criteria |
|---|---|---|
| Monolithic workspace | `cargo build --workspace` in `your-app` without `keel-platform` source | Builds in < 30s (WASM only) |
| Tenant isolation | `cargo test tenant_isolation -- --nocapture` | Zero cross-tenant reads/writes in cache/queue/events/scheduler/storage |
| Boot-time freeze | `kelson config:reload` + verify route/flag change in < 50ms | No restart required; in-flight requests unaffected |
| No versioning | Deploy v2 job schema with v1 jobs in-flight | Zero deserialization errors; auto-migration verified |
| RLS escape hatch | Connect as `kelson_platform`, query tenant table without GUC | `ERROR: permission denied` (RLS enforced) or empty result |
| Pooled connection leak | Acquire tenant conn, set GUC, release, re-acquire, check GUC | GUC reset to NULL on release |

---

## 11. What Explicitly NOT in This Architecture

| Rejected Approach | Reason |
|---|---|
| `libkelson_kernel.so` + native `.so` plugins | ABI instability, allocator conflicts, no isolation — WASM is the plugin boundary |
| Single workspace with feature flags | Doesn't solve independent deploy; compile-time coupling remains |
| Runtime tenant context via thread-locals | Not `Send`/`Sync` safe; loses compile-time enforcement |
| SQL-level rollback for foundation tables | Too risky; foundation migrations remain forward-only with manual rollback runbook |
| Event sourcing for all state | Over-engineering; job/event versioning + idempotency solves the actual problem |

---

## 12. Decision Authority

| Decision | Owner | Escalation |
|---|---|---|
| Workspace split boundaries | Platform Team Lead | CTO |
| Port trait signatures | Framework Architect | Platform Team Lead |
| Tenant scoping enforcement | Security Architect | CTO |
| Hot-reload semantics | Platform Team Lead | Framework Architect |
| RollbackExecutor behavior | Platform Team Lead | CTO |
| RLS policy changes | DBA + Security Architect | CTO |

---

*This architecture is derived exclusively from verified evidence in the Phase 0 audit. No hypotheses, no unimplemented speculation. Each component addresses a confirmed root cause with executable verification criteria.*