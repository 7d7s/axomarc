# Keel Stage-0 Foundation Specification (FROZEN — D-61)

> Status: FROZEN 2026-08-20. Supersedes 11-FOUNDATION-MASTER-SPEC.md topology sections (§8, §9); those remain historical.
> Constitution: Keel is a **framework operating system** with four simultaneous identities:
> ① Application framework (Laravel parity) · ② Execution runtime (context/policy/transaction/workflow/delta) · ③ Developer platform (CLI/generators/testkit/diagnostics/graph/AI) · ④ Enterprise control plane (tenant/security/deployment/feature/governance).
> Identity ① gives parity; ②③④ are the successor surface.

## 1. Conceptual model (the final rule)

```
Intent → Execution → ExecutionPlan → Policy → Transaction → StateTransition → Delta
```

HTTP, CLI, JOB, SCHEDULE, EVENT, WEBHOOK, WORKFLOW, BATCH, IMPORT, EXPORT, AI, MCP, INTERNAL, REPLAY are all **front doors** converging into one Execution. The domain never sees the transport.

**Final rule (top of repository):** If a behavior is important enough to be repeated across applications, it must become a platform contract. If it affects security, consistency, observability, or recoverability, it must be enforced by the execution kernel — not left to developer convention.

## 2. Workspace topology (exact)

```
platform/
├── crates/
│   ├── foundation/   core · context · contracts · lifecycle · validation
│   ├── execution/    engine · command · query · transaction
│   ├── application/  registry · configuration
│   ├── web/          http
│   ├── security/     identity · policies · scopes · ownership · elevation (+ S5: integrity)
│   ├── data/         postgres
│   ├── async/        events · jobs · workflows · scheduler
│   ├── services/     cache · storage · notifications
│   ├── enterprise/   (S3: tenancy · audit-stream · delta-stream · feature-flags · deployment)
│   ├── observability/ logging (+ S5 Sentinel: telemetry · tracing · metrics · audit · pipeline · buffer · export)
│   ├── ai/           (S6 stubs: client · agents · tools · mcp)
│   └── testing/      (S2: testkit)
├── adapters/         keel-cache-memory · keel-sms-msg91 · keel-mail-smtp · keel-storage-s3 · keel-push-fcm (+ S2 storage-local, S3 webhooks)
├── runtime/          keel-api · keel-worker
├── applications/     generated/domain apps (medicare reference later — D-46/D-70)
├── templates/        project-template for `keel new`
├── examples/         minimal (golden path)
├── database/         migrations · roles.sql
├── tools/            keel-console · keel-analyzer · keel-codegen
├── tests/            architecture (CI-gate tests)
├── xtask/  docs/  specs/  config/
```

Package names keep the `keel-` prefix (D-63). Crate directories follow blueprint names; package name may differ from directory (e.g. `crates/execution/engine` = package `keel-execution`).

### Crate responsibility table (current 47)

| Path | Package | Responsibility |
|---|---|---|
| crates/foundation/core | keel-foundation | ids, error taxonomy+codes, envelope types, Money/Secret/Phi, Clock, Locale, Delta stone types, OperationId, Deadline |
| crates/foundation/context | keel-context | ExecutionContext (immutable after build), builders, snapshots |
| crates/foundation/contracts | keel-contracts | cross-layer port contracts shared by all layers |
| crates/foundation/lifecycle | keel-lifecycle | Provider trait, ApplicationHost, service registry |
| crates/foundation/validation | keel-validation | boundary validation primitives + Validator (S2: collect-all) |
| crates/execution/engine | keel-execution | ExecutionUnit, OperationHandler, ExecutionPlan, engine pipeline, idempotency+concurrency ports |
| crates/execution/command | keel-command | CommandBus dispatch front door |
| crates/execution/query | keel-query | read-side query contract |
| crates/execution/transaction | keel-transaction | tx contracts: TxOptions/isolation, TransactionPort/TxHandle, advisory + FOR-UPDATE lock helpers (SQL in keel-postgres) |
| crates/application/registry | keel-registry | module manifests, PlatformGraph compile, capability catalog |
| crates/application/configuration | keel-config | typed layered config (defaults←file←env), env profiles |
| crates/web/http | keel-http | Envelope/ErrorBody wire, AppError→status, Request/ResponseViewModel, (S2: Page<T>) |
| crates/security/identity | keel-identity | principal/authN: argon2, JWT, OTP skeleton, Secret |
| crates/security/policies | keel-policy | policy engine decide |
| crates/security/scopes | keel-scope | scope lattice |
| crates/security/ownership | keel-ownership | ownership snapshots |
| crates/security/elevation | keel-elevation | platform-plane elevation flow |
| crates/data/postgres | keel-postgres | pool, embedded migrations, Tx engine, delta/audit/outbox/idempotency writers |
| crates/async/events | keel-event | event/listener specs, outbox row contract |
| crates/async/jobs | keel-jobs | DurableJob, JobStore port, retry semantics |
| crates/async/workflows | keel-workflow | state machines |
| crates/async/scheduler | keel-scheduling | schedule definitions/catalog |
| crates/services/cache | keel-caching | CachePort + TTL entries |
| crates/services/storage | keel-storage | StoragePort |
| crates/services/notifications | keel-notifications | NotifyHub, MailPort/SmsPort/PushPort |
| crates/observability/logging | keel-observability | S5 Sentinel runtime host: ObservabilityRuntime, policy engine, incident control plane, ObservabilityProvider; keeps the S1 API (init_logging, LogConfig, redact_str, chain records) |
| crates/observability/telemetry | keel-telemetry | S5: event bus, EventEnvelope + schemas, typed event ids, classification, plugin traits |
| crates/observability/tracing | keel-tracing | S5: span model + causality, W3C traceparent helpers, adaptive sampler |
| crates/observability/metrics | keel-metrics | S5: counters/gauges/histograms, bounded label cardinality, Prometheus text renderer |
| crates/observability/audit | keel-audit | S5: audit event schema, retention types, tamper/equivalence report models |
| crates/observability/pipeline | keel-pipeline | S5: processing DAG (normalize, classify, enrich, redact, fingerprint, sample, dedupe, route), policy grammar |
| crates/observability/buffer | keel-buffer | S5: bounded buffers, durability classes, backpressure policy, disk WAL |
| crates/observability/export | keel-export | S5: Exporter trait + console/file/OTLP-JSON/HTTP/syslog, circuit breakers, retry budget, health snapshots |
| crates/security/integrity | keel-integrity | S5: audit hash-chain primitives (sha2), HMAC batch signing |
| adapters/* | 5 adapter crates | sandbox/external integrations |
| runtime/keel-api | keel-api | axum host, degraded boot (D-52) |
| runtime/keel-worker | keel-worker | worker binary (S2: real loop) |
| tools/* | cli/analyzer/codegen | `keel` CLI, G07 dep-matrix analyzer, module scaffolder |
| tests/architecture | architecture-tests | CI-gate architecture assertions |

## 3. Dependency direction matrix (enforced by keel check)

Layers (may depend only on same-or-lower):

```
L0 foundation          (core, context, contracts, validation, lifecycle)
L1 security            (identity, scopes, ownership, policies, elevation, integrity)   → L0
L2 execution           (engine, command, query)                              → L0..L1
L3 application         (configuration, registry)                             → L0..L2
L4 web/services/async  (http, cache, storage, notifications, events, jobs, workflows, scheduler) → L0..L3
L5 observability       (logging; S5: telemetry, tracing, metrics, audit, pipeline, buffer, export) → L0 (does NOT import runtime)
L6 data                (postgres implements L2/L4 ports)                     → L0..L4
L7 runtime             (keel-api, keel-worker)                               → all crates/, no adapters rule exception: runtime wires adapters
L8 adapters            (implement service ports; never imported by crates/*) → L0..L4
L9 applications/       (domain modules)                                      → platform crates; platform NEVER imports applications
LA tools               (cli, analyzer, codegen, xtask)                       → anything except runtime binaries
```

Forbidden: adapter→adapter, crates/*→adapters/*, platform→applications, domain→transport (modules use ports), security→web.

## 4. Public/private boundaries & API governance (D-71)

- Every public item is either **Public-API** (versioned, API-ID tracked from S4) or **Internal** (`#[doc(hidden)]` or crate-private). New public surface requires an API-ID entry once the registry lands; until then the crate's `lib.rs` module listing is the contract.
- `cargo-semver-checks` gates releases from S4 (external CI tool, not a Rust dependency).
- Breaking changes only in a new major; deprecation window of one minor via `DEPRECATION.md`.

## 5. Core traits (frozen signatures as of Stage-0)

```rust
// foundation/lifecycle
pub trait Provider: Send + Sync {
    fn name(&self) -> &'static str;
    fn dependencies(&self) -> Vec<&'static str> { vec![] }       // S1: topo-sort + cycle rejection
    fn register(&self, registry: &mut ServiceRegistry) -> Result<(), AppError>;
    fn boot(&self, registry: &ServiceRegistry) -> BoxFuture<'_, Result<(), AppError>>;
}

// foundation/core
pub trait Clock: Send + Sync { fn now(&self) -> OffsetDateTime; }

// execution/engine (post borrow-checker redesign)
pub type HandlerFn<'a> = Box<dyn FnOnce(&mut dyn ExecutionUnit)
    -> BoxFuture<'static, Result<Value, AppError>> + Send + 'a>;
pub trait OperationHandler: Send + Sync {
    fn handle(&self, ctx: Arc<ExecutionContext>, input: Value, unit: &mut dyn ExecutionUnit)
        -> BoxFuture<'static, Result<Value, AppError>>;
}

// async/jobs
pub trait JobStore: Send + Sync {
    fn enqueue(&self, job: DurableJob) -> Result<Uuid, AppError>;
    fn claim(&self, worker: &str) -> Result<Option<DurableJob>, AppError>;
    fn complete(&self, id: Uuid) -> Result<(), AppError>;
    fn fail(&self, id: Uuid, error: &str) -> Result<JobStatus, AppError>;
}

// services/notifications
pub trait MailPort: Send + Sync { fn send(&self, msg: &NotifyMessage, subject: &str) -> Result<(), AppError>; }

// foundation/context (immutable after build — S1 enforces no post-build mutation)
pub struct ExecutionContext {
    pub execution_id: ExecutionId, pub request_id: Option<RequestId>,
    pub correlation_id: CorrelationId, pub causation_id: Option<CorrelationId>,
    pub plane: SecurityPlane, pub principal: PrincipalContext, pub actor: ActorContext,
    pub tenant: Option<TenantContext>, pub scope: Option<ScopeSnapshot>,
    pub ownership: OwnershipSnapshot, pub authorization: Option<AuthorizationSnapshot>,
    pub feature: Option<FeatureSnapshot>, pub profile: ExecutionProfile,
    pub deadline: Deadline, pub locale: Locale,
    // S1 adds: trace_id: TraceId, timezone: TimeZone
}
```

## 6. Provider lifecycle & service lifetimes (S1 target)

register → dependency topo-sort (cycle = boot refusal) → boot in order → READY. Service lifetimes understood by the container: **PROCESS** (pools, caches) · **APPLICATION** (config, registry) · **EXECUTION** (ExecutionContext) · **TRANSACTION** (Tx handle) · (S3: REQUEST/TENANT scopes). Constructor injection everywhere; no reflection magic.

## 7. Registry & platform graph

Modules declare `module.toml` (id, version, maturity, plane, dependencies, capabilities, routes, events, permissions). `PlatformGraph::compile` rejects: duplicate routes/capabilities/events, missing dependencies, plane violations. This feeds `keel graph|modules|routes|capabilities` and gate G05.

## 8. Execution state machine

```
ACCEPTED → PLANNED → AUTHORIZED → RUNNING → COMMITTED → DELIVERED
              │           │           │
              └→ DENIED   └→ REJECTED └→ FAILED → (RETRY | DEAD)
```
Plan compilation (S1) produces: AuthorizationPlan, TransactionPlan, ConcurrencyPlan, ConsistencyPlan, IdempotencyPlan, RetryPolicy, AuditPlan, DeltaPlan, timeout. The engine consumes plans; it never re-derives framework semantics at execution time.

## 9. Configuration system

Sources layered: `defaults ← config file (config/<env>.toml) ← .env (S2) ← environment variables`. Parsed → validated (secrets asserted; prod refuses silent defaults) → normalized → **frozen at boot** (immutable `Arc<PlatformConfig>` thereafter). Profiles (D: Environment enum): development, test, staging, production (+ S1: benchmark, migration, worker postures).

## 10. Error taxonomy (frozen)

Categories: Validation, Authentication, Authorization, NotFound, Conflict, Workflow, Domain, Persistence, External, ResourceExhausted, Internal. Each carries code (KER_* / module prefix), public_message, severity, retryability, meta (field errors etc.). Wire shape = Envelope{ok:false,error:{code,message,retryable,meta}}; internals never leak.

## 11. Feature flags (S3 service, contract now)

Dimensions: global · environment · tenant · region · percentage · capability · actor. Rollout ladder: disabled → internal → 1% → 10% → 50% → 100% with rollback. Snapshot lives in context (`FeatureSnapshot`), resolved once per execution.

## 12. Build profiles

cargo profiles: `dev` (debug=1), `test`, `release` (LTO later). Runtime postures selected by config env + binary (api vs worker vs migration). `keel build` wraps cargo with the matrix documented in RELEASE.md.

## 13. CI gates

G01 format (fmt) · G02 clippy -D warnings · G03 unit+integration tests · G04 workspace compile matrix · G05 platform graph compile (no duplicate routes/caps/events) · G06 module manifest validity · G07 dependency matrix (keel check, xtask rules incl. §3 layer directions) · G08 migrations ordered & present · G09 golden-path example passes · G10 degraded-boot smoke · G11 architecture tests (domain ⊥ transport; platform ⊄ applications; tenant cannot mutate platform) · G12 secrets hygiene (no key material in repo) · G13 docs links resolve · G14 API surface delta reported (S4: enforced via semver-checks) · G15 benchmark baseline (S4) · G16 diagnostic chain persistence (S2) · G17 **capability completion**: a capability is done only when all 13 facets exist — contract, runtime, API, configuration, CLI, generator, testkit, observability, security, documentation, extension points, performance budget, compatibility policy. · G18 **telemetry resilience**: failure-injection + tenant-isolation suite (WAL corruption recovery, exporter fault injection, backpressure, shutdown drain, cross-tenant leak hammer) green in CI.

## 14. Stage roadmap

S0 freeze+restructure (this doc) → S1 core hardening (trace_id/timezone, plan compiler, tx contract split, logging, provider graph) → S2 parity surface (validator, .env, migrations CLI, seeders, tokens, job runtime, worker loop, outbox pump, middleware pipeline, pagination, storage-local, testkit v1, operational CLI, generators + keel new) → S3 enterprise control plane → S4 developer platform → S5 Sentinel (telemetry runtime) → S6 AI/MCP → S7 public ecosystem. Full L00–L80 matrix: docs/rust/13-FRAMEWORK-BLUEPRINT.md.

## 15. Frozen decisions recorded here

D-61 restructure-now · D-62 dependency policy v2 (justify table; cap 25) · D-63 naming Keel/keel-* · D-64 consolidation granularity · D-65 delta kernel in engine · D-66 sessions opt-in S3 · D-67 AI stubs S6 (renumbered by D-73) · D-68 metrics deferred stubs · D-69 compile-time modules first · D-70 medicare isolation · D-71 API-ID + semver gate · D-72 G17 capability completion · D-73 Sentinel stage insertion (rescopes D-68 metrics deferral into S5; AI/MCP → S6, public ecosystem → S7) · D-74 crate split: TelemetryContext carrier in keel-foundation + audit hash chain in keel-integrity (security class; data layer cannot depend on observability) · D-75 OTLP over reqwest (OTLP/HTTP + JSON encoding), no opentelemetry* deps (budget 23/25 unbreached) · D-76 incident control plane = file plane under storage/framework/telemetry/control/ (source of truth) + keel-api admin HTTP endpoint writing the same file.