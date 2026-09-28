# 11 — Foundation Master Specification (Stage-0/Stage-1 Freeze)

> Status: **FROZEN for foundation build (2026-08-19)**. This is the Stage-0/Stage-1 master artifact demanded by the
> Principal/Distinguished-level review: it freezes the repository, Cargo topology, dependency rules, public API
> boundaries, core types, execution contracts, graph schema, module manifest schema, lifecycle, and foundation gates
> **before any ERP/healthcare implementation begins**.
>
> Lineage: synthesizes the two attached foundation specs ("successor-class enterprise application platform",
> "ERP Execution Platform Foundation Specification" §1–63) plus decisions D-01…D-43 from `08-DECISIONS.md`.
> Where this document and older docs disagree, **this document wins** for the platform track.
> New decisions introduced here: **D-44…D-53** (appended in `08-DECISIONS.md`).

---

## 0. What we are building (positioning)

Not "Laravel rewritten in Rust". Not "a sophisticated web framework".

> **A successor-class enterprise application execution platform** where the compiler, runtime, tooling, contracts and
> governance cooperate to make production-safe development the default: correct code easier to write, incorrect
> architecture harder to compile, unsafe behavior harder to express, production behavior easier to inspect.

Five products, one repository, deliberately decoupled:

| # | Product | Reusable? | Content |
|---|---|---|---|
| 1 | **Runtime** | yes | Execution kernel: context, plan, engine, transaction, delta/audit/outbox |
| 2 | **Framework** | yes | MVVM/HTTP, buses, workflow, jobs, events, ports, caching, config |
| 3 | **Toolchain** | yes | CLI, generators, graph compiler, architecture analyzer, release |
| 4 | **Reference implementations** | no (proof) | `applications/reference-erp`, later `reference-healthcare` (Medicare) |
| 5 | **Governance system** | no (guardrails) | Dep rules, gates, maturity levels, CI, policies, ADRs |

**Working codename: Keel** (D-44, veto-able). CLI binary: `keel`. Crate prefix `keel-`. Repo folder: `platform/`.

### Trusted computing base (ruthlessly small)

```text
TCB = IDs · AppError · ExecutionContext (immutable) · ExecutionPlan · execution engine
    · authorization decision boundary · transaction boundary · lifecycle · registry validation
```

Everything else sits **outside** the TCB. Growth inside the TCB is a review-gated change.

### Three classes of code (machine-enforced, see §9)

```text
CORE        absolutely trusted (core/*)
PLATFORM    reusable, replaceable (framework/*, security/*, adapters/*)
APPLICATION business/domain-specific (domains/*, applications/*, examples/*)
```

Allowed dependency direction: `APPLICATION → PLATFORM → CORE`. Never reversed. Never skipped upward.

---

## 1. Security/operational planes (typed, machine-checked)

```rust
pub enum SecurityPlane { Edge, Platform, Tenant }
```

| Plane | Owns | Forbidden |
|---|---|---|
| **Edge** | ingress, routing, caching, rate control, read sync | mutating tenant business state |
| **Platform** | identity, tenant provisioning, module/feature registry, release state | automatic tenant-data authority |
| **Tenant** | business resources, domain execution, tenant audit/delta | mutating platform control state |

Hard invariants (become analyzer + test rules P1–P5):

- **P1** Edge cannot mutate tenant business state.
- **P2** Platform operator status grants no implicit tenant authority.
- **P3** Tenant execution cannot mutate platform control state.
- **P4** Cross-plane authority requires explicit elevation.
- **P5** Every elevated platform→tenant operation carries reason + capability + scope + expiry + audit.

---

## 2. The core object: Execution (not Request)

A request is one transport among many. **Execution** is canonical:

```text
REST · CLI · JOB · SCHEDULE · WEBHOOK · WORKFLOW · BATCH IMPORT · INTEGRATION · AI TOOL · INTERNAL
                          └────────────► one execution model ────────────┘
```

One business operation (e.g. `appointment.book`) enters through any front door and receives the same
authorization, transaction, idempotency, delta, audit and events.

### ExecutionContext (immutable after resolution)

```rust
pub struct ExecutionContext {
    pub execution_id: ExecutionId,
    pub request_id: Option<RequestId>,
    pub correlation_id: CorrelationId,
    pub causation_id: Option<CorrelationId>,

    pub plane: SecurityPlane,
    pub principal: PrincipalContext,     // who holds authority
    pub actor: ActorContext,             // who physically acts (may differ: delegation/support mode)

    pub tenant: TenantContext,
    pub scope: ScopeSnapshot,            // where may I operate (frozen effective scope)
    pub ownership: OwnershipSnapshot,    // resource ownership facts for this execution

    pub authorization: AuthorizationSnapshot,  // revision-pinned capability view
    pub feature: FeatureSnapshot,

    pub profile: ExecutionProfile,
    pub deadline: Deadline,
    pub locale: Locale,
}
```

No mutation after resolution. No ambient state. No `user_id == principal_id == actor_id` shortcuts.

### ExecutionPlan (declared intent → resolved semantics)

```rust
pub struct ExecutionPlan {
    pub operation: OperationId,          // stable namespaced id: "appointment.book"
    pub plane: SecurityPlane,
    pub authorization: AuthorizationPlan,
    pub transaction: TransactionPlan,    // profile-driven; developers never write begin/commit
    pub consistency: ConsistencyPlan,
    pub concurrency: ConcurrencyPlan,    // Optimistic | Pessimistic | Serialized
    pub idempotency: IdempotencyPlan,    // execution-level, not HTTP middleware
    pub delta: DeltaPlan,                // required for mutations (KER-03 lineage)
    pub audit: AuditPlan,
    pub timeout: Deadline,
    pub retry: RetryPolicy,
    pub profile: ExecutionProfileKind,
}
```

### Execution profiles (reusable semantics packages)

```text
READ · COMMAND · FINANCIAL · INVENTORY · WORKFLOW · BATCH · REPORT · SYSTEM · REPLAY
```

Each profile fixes: authorization depth, transaction shape, consistency, locking, retry, timeout, idempotency,
audit, delta. Example — `FINANCIAL`: strong transaction, no blind retry, idempotency required, delta required,
audit required, reversal semantics. Engineering policy is encoded, not re-implemented.

### Fast path vs safe path

```text
FAST: route → trusted identity → valid AccessSnapshot → capability lookup → policy check → SQL
SAFE: snapshot stale / elevation / unresolved ownership / dynamic policy / replay / cross-plane
      → kernel performs full re-resolution (slow path), then continues
```

Speed without weakening correctness: hot path is a lookup against frozen state; correctness path is the same
code with fresh resolution.

---

## 3. Authorization model

**Permission ≠ authorization.** A permission says the operation exists; authorization says this principal may
perform it, on this resource, in this scope, right now.

```text
ALLOW = IdentityValid ∧ MembershipActive ∧ CapabilityGranted ∧ ScopeContainsResource
      ∧ OwnershipCompatible ∧ PolicyAllows ∧ ModuleEnabled ∧ ResourceStateAllows ∧ RevisionCurrent
```

- **RBAC** (class of authority) + **ABAC** (conditions) + **Scope** (where) + **Ownership** (which resource
  relationship) + **State** (valid right now).
- **Identity** = `Principal · Actor · Membership · Credential · Session` as separate types.
- **Scope** is a graph, not one strict tree: Tenant → LegalEntity / BusinessUnit / Region / Location / Branch /
  Department / CostCenter / Warehouse / custom. Effective scope computed once per authorization refresh and
  frozen into the context.
- **Ownership** is separate from scope: an actor may reach BU-4 yet not control the invoice owned by LE-7.
- **Capability** = `resource · operation · scope · conditions` (e.g. `invoice.post` with `legal_entity ∈ {LE1,LE2}`
  and `amount ≤ 500000`).

### Revisions (no TTL-only thinking)

```text
PlatformRevision · TenantRevision · AuthorizationRevision · ModuleRevision · FeatureRevision · SchemaRevision · DeltaSequence
```

`AccessSnapshot` pins revisions. Any authorization-affecting mutation bumps the revision; snapshots pinned to an
older revision are **stale** and force the safe path. This supersedes arbitrary cache TTLs.

### Elevation (support mode, D-21 lineage)

```text
Platform operator → elevation request (reason + target tenant + capability + scope + approval)
→ short-lived lease → audited execution → automatic expiry. Read-only default.
```

Never `platform_admin == tenant_all_access`. Leases are rows: `elevation_leases` (P5).

---

## 4. State transitions: Delta · Audit · Event · Outbox

Four distinct concepts, one atomic transaction:

| Concept | Meaning | Mutability |
|---|---|---|
| **DeltaStone** | state transition record | immutable, append-only, replayable |
| **Audit** | accountability/security record (who/what/when/where/authority/result) | append-only |
| **Event** | business fact (`InvoicePosted`) | emitted post-commit only |
| **Outbox** | delivery guarantee | committed atomically, dispatched after commit |

```text
DeltaStone = delta_id · stream_id · sequence · resource_type/id/version · execution_id · principal_id
           · tenant_id · scope_snapshot · ownership_snapshot · before · command · after
           · causation_id · correlation_id · timestamp
```

### Delta streams (the sync/export spine)

```text
PlatformDelta (module/deployment/feature/tenant-lifecycle changes)
ScopeDelta    (membership/role/scope/delegation/authorization changes)
DataDelta     (business mutations)
```

Streams are monotonic sequences (`tenant`, `module`, `resource`, `scope`, `edge` partitions). Cursor protocol:
`GET /sync/{stream}?after=N`. This yields offline sync, edge replication, incremental export, analytics and replay
**without Kafka as a prerequisite** (06-REALTIME-AND-SYNC remains the detail doc).

Rules: "update the audit record" is unexpressable. Events never precede commit (transactional outbox, D series).

---

## 5. Persistence fabric

- **SQLx first-class. No ORM, no hidden query-builder magic** (D-48). Compile-time-checked `query!` macros once CI
  provisions the DB; typed runtime queries until then. Explicit indexes, transactions, locking, query plans.
- **Three data-access modes** (D-49) — not "repositories everywhere":

```text
AggregateRepository  invariant-heavy writes
ReadProjection       fast operational reads
ReportingQuery       large analytical reads
```

- **Tenant anchor**: every tenant-owned table carries `tenant_id` unless documented proof says platform-only or
  globally immutable. **RLS is the final containment barrier**; application authorization is the business decision.
  Context flow: `ExecutionContext → SET LOCAL keel.tenant_id → RLS` (GUC renamed from `medicare.*`, D-47).
- **Database roles** (created per environment, documented in `database/roles.sql`):
  `migration_owner · platform_runtime · tenant_runtime · reporting_reader · maintenance`.
  The runtime role never owns tables (RLS owner-bypass protection).
- **Transaction engine**: developers declare a transaction profile; the engine performs
  `BEGIN → security context → locks → version checks → domain transition → persist → DeltaStone → Audit → Outbox → COMMIT`,
  and only after commit may the outbox dispatcher publish events.
- **KER-03 preserved**: a mutating transaction that emits no delta cannot commit.
- **Concurrency**: three explicit strategies — `OPTIMISTIC` (version column CAS), `PESSIMISTIC` (`FOR UPDATE`),
  `SERIALIZED` (sequenced). No one-size locking.
- **Deterministic lock ordering**: multi-row locking sorts resource ids and acquires in order (deadlock prevention;
  real concurrency harness required in `validation/concurrency/`).
- **Idempotency is execution-level**: `(tenant, principal, operation, idempotency_key, request_hash) → stored result`.
  Same key+payload → replayed result. Same key, different hash → conflict. Works for REST/CLI/jobs/webhooks/imports/AI.

---

## 6. MVVM and transport (presentation adapter only)

```text
HTTP → Controller → RequestViewModel → Command/Query → Execution Engine → Result
     → ResponseViewModel → Resource → HTTP        (CLI/Worker/AI adapters reuse the same ExecutionPlan)
```

- **Controller**: transport only. **ViewModel**: transport contract + boundary validation. **Domain**: invariants.
  **Resource**: representation. MVVM never leaks below the application layer.
- **Domain code never knows HTTP**: no 404/403/500 in domain. One canonical error protocol:

```text
AppError = code · category · severity · retryability · public_message · internal_message · safe_metadata · correlation
```

Transport maps `AppError → application/problem+json` at the edge (envelope kept for success payloads).

---

## 7. Module model and graph compilation

A developer builds a **Module**, not controllers/services/models:

```text
Module = module.toml + domain + application(commands/queries/handlers/jobs/workflows)
       + transport(http/cli) + infrastructure(postgres/integrations/projections/caches)
       + contracts(api/events/commands) + migrations + tests + docs
```

### module.toml (manifest schema, frozen)

```toml
[module]
name = "inventory"                  # unique, kebab-case
version = "0.1.0"
maturity = "L0"                     # L0 Experimental … L4 Certified (see §12)
plane = "tenant"                    # edge | platform | tenant
description = "Stock levels, reservations, adjustments."

[capabilities]                       # operation catalog = permission catalog
"stock.adjust" = { profile = "INVENTORY", delta = "required", audit = "required", idempotency = "required" }
"stock.view"   = { profile = "READ" }

[dependencies]                       # other modules' CONTRACTS only (never their internals)
contracts = ["product"]

[[routes]]
method = "POST"
path = "/api/v1/stock/adjust"
operation = "stock.adjust"
capability = "stock.adjust"
version = "v1"

[[events]]
name = "StockAdjusted"
stream = "data"

[[workflows]]
name = "stocktake"
file = "domain/state-machines/stocktake.toml"

[[migrations]]
path = "migrations/"
```

### PlatformGraph (compiled at build, frozen at boot)

```text
PlatformGraph = Modules · Routes · Commands · Queries · Capabilities · Policies · Events · Jobs · Workflows
              · Migrations · Resources · Dependencies
```

Graph validation (only a valid graph produces a release artifact):

```text
cycles · orphans · duplicates · missing dependencies · missing authorization · missing tenant boundary
· missing delta · missing audit · missing migration · route conflicts · event conflicts · contract conflicts
```

Runtime never re-discovers "what exists": startup loads the validated graph, freezes it, serves lookups.

### Workflow compiler

Workflows are executable metadata (`states · transitions · guards · actions · compensation · permissions · timeout`);
the compiler rejects unreachable states, dead ends, illegal transitions, missing guards/policies/compensation,
cycles without exit.

---

## 8. Repository topology (FROZEN)

```text
platform/                          ← the Keel platform workspace (own Cargo workspace)
├── core/                          CORE class — the TCB
│   ├── keel-foundation/                 ids, AppError, planes, profiles, revisions, money, clock, DeltaStone types
│   ├── keel-context/              ExecutionContext, snapshots, builder (immutability enforced)
│   ├── keel-execution/            ExecutionPlan, engine trait + implementation, idempotency, concurrency types
│   ├── keel-lifecycle/            ApplicationHost, Provider, phases, service registry (typemap), freeze
│   ├── keel-registry/             ModuleManifest, PlatformGraph, validation (cycles/dupes/missing deps)
│   └── keel-contracts/            public contract traits: Command, Query, Event, Job, Resource, Policy
│
├── framework/                     PLATFORM class — reusable framework layer
│   ├── keel-http/                 MVVM adapter: envelope, problem+json, ViewModels, route wiring, request_id
│   ├── keel-command/              command bus (authz → idempotency → tx → handler pipeline)
│   ├── keel-query/                query bus + projections
│   ├── keel-workflow/             state machines + workflow compiler (graph checks)
│   ├── keel-jobs/                 durable executions (not "queue jobs")
│   ├── keel-event/               event model, listener declarations, outbox contract
│   ├── keel-validation/           boundary validation primitives
│   ├── keel-config/               typed · layered · immutable · secret-aware configuration
│   ├── keel-caching/              named cache ports (identity/authorization/config/reference/query/edge)
│   ├── keel-storage/              storage port
│   ├── keel-notifications/        sms/mail/push ports
│   ├── keel-scheduling/           schedule specs
│   └── keel-observability/        execution/trace ids, diagnostic chain records
│
├── security/                      PLATFORM class — security subsystems
│   ├── keel-identity/             Principal · Actor · Membership · Credential · Session
│   ├── keel-scope/                scope graph + effective-scope computation
│   ├── keel-ownership/            ownership graph + snapshots
│   ├── keel-policy/               capability model, AccessSnapshot, ALLOW decision, revision cache
│   └── keel-elevation/            elevation leases (P4/P5)
│
├── adapters/                      PLATFORM class — replaceable implementations of ports
│   ├── keel-postgres/             pool, tx engine, RLS context, delta/audit/outbox writers, revisions, idempotency store
│   ├── keel-cache-memory/         in-process cache (moka)
│   ├── keel-sms-msg91/            MSG91 (D-16; DLT details pending)
│   ├── keel-mail-smtp/            SMTP (D-18)
│   ├── keel-storage-s3/           MinIO/S3 (D-19)
│   └── keel-push-fcm/             Firebase push (D-17)
│
├── runtime/                       deployable binaries
│   ├── keel-api/                  HTTP runtime (ApplicationHost + axum)
│   └── keel-worker/               durable-job worker
│
├── toolchain/
│   ├── keel-console/                  the `keel` binary (§11)
│   ├── keel-analyzer/             architecture/security rule engine (ARCH/AUTH/SCOPE/DELTA/MOD/CONTRACT/MIG/CONC)
│   ├── keel-codegen/              contract-driven generators (make:*)
│   └── xtask/                     build tasks, dep-rules gate, sbom, release prep
│
├── domains/                       APPLICATION class — vertical packs (empty at foundation; healthcare later)
├── examples/minimal/              smallest possible app proving host + module + execution
├── applications/                  reference-erp (built after gates; reference-healthcare = Medicare port)
├── database/
│   ├── migrations/                foundation DDL (platform + kernel tables, RLS)
│   ├── rls/                       RLS policy sources
│   ├── roles.sql                  env role creation (migration_owner/platform_runtime/tenant_runtime/reporting_reader/maintenance)
│   └── seeds/
├── validation/
│   ├── architecture/              dependency-boundary tests (run in CI)
│   ├── isolation/                 RLS + plane cross-access harness (real DB)
│   ├── concurrency/               lock-order + contention harness
│   ├── performance/               budgets + hot-tenant SLO harness (skeleton)
│   └── chaos/                     deterministic failure injection (skeleton)
├── docs/                          platform docs (getting-started/concepts/architecture/security/operations/cookbook/reference)
├── specs/                         platform specs + ADR/
├── config/                        environment configs (dev.toml, test.toml)
├── SECURITY.md GOVERNANCE.md CONTRIBUTING.md ARCHITECTURE.md COMPATIBILITY.md RELEASE.md SUPPORT.md
├── CODE_OF_CONDUCT.md THREAT-MODEL.md DEPENDENCY-POLICY.md API-STABILITY.md DEPRECATION.md
└── Cargo.toml rust-toolchain.toml justfile docker-compose.yml .gitignore README.md
```

Repo placement: `platform/` lives inside the medicare repo for now; extraction to its own public repository is a
release-track task (G19/G20), tracked in `specs/ADR/`. **Nothing in `platform/` may mention Medicare** (healthcare is
a vertical pack; D-50). The existing `medicare-kernel/` remains compilable but is **superseded** (D-46) — its proven
pieces (delta engine, scope lattice, tx guard, workflow machine) are re-homed into this topology; it will be rebuilt
as `applications/reference-healthcare` on the platform after foundation gates.

---

## 9. Dependency rules (machine-enforced)

Class edges (Cargo dependency direction):

```text
core/*        → core/*, std-only externals (uuid, time, serde, thiserror)
security/*    → core/*; never framework/*, adapters/*, domains/*, applications/*
framework/*   → core/*, security/*; never adapters/*, domains/*, applications/*
adapters/*    → core/*, framework/* (ports); never security/* except via contracts; never domains/*
toolchain/*   → core/* (graph/manifest types); never framework runtime internals
runtime/*     → may wire everything (composition root only — no business logic)
domains/*     → core/*, framework/*, security/*, adapters/* (via ports only)
applications/*→ domains/* + everything above
examples/*    → smallest slice possible
```

Forbidden-edge violations produce analyzer diagnostics (e.g. `MOD-017 module imports another module's private
adapter`) and fail CI. Domain modules depend on **other modules' contracts only**, never internals.

External dependency budget: **≤ 20 direct deps** at foundation (DEPENDENCY-POLICY.md governs additions: license,
maintenance, MSRV, feature surface, security history).

---

## 10. Public API boundaries and packaging

Public surface is intentionally small (spec §50): `ApplicationHost · Module · Provider · ExecutionContext ·
Command · Query · Workflow · Policy · Resource · RepositoryPort · Event · Job` plus the error/envelope types.
Everything else stays crate-private until proven.

Publishing order (not "40 crates at once"):

```text
wave 1: keel-foundation · keel-app (= lifecycle+execution façade) · keel-http · keel-security (façade) · keel-postgres · keel-console
wave 2: keel-cache · keel-storage · keel-search · keel-notifications
```

A crate goes public only when: API stable · docs complete · tests complete · compatibility defined · security
reviewed · dependency surface acceptable (API-STABILITY.md, DEPRECATION.md, COMPATIBILITY.md).

---

## 11. Lifecycle (ApplicationHost)

```text
phases: Foundation → Kernel → Platform → Modules → Ready(freeze) → Serve
```

- `Provider` = named lifecycle unit declaring phase + `init(registry)` + `boot(host)`.
- `ServiceRegistry` = typed service map keyed by `TypeId`; lookups are checked, failures are boot errors with
  precise messages.
- At `Ready` the PlatformGraph and service registry **freeze**; mutation after freeze is a panic-level bug.
- Boot is fail-fast (missing secret/DB/config → precise error) but the dev runtime supports degraded boot
  (no DB ⇒ `/readyz` 503, `/healthz` 200).

---

## 12. Maturity levels (modules and the platform)

```text
L0 Experimental · L1 Functional · L2 Production · L3 Enterprise · L4 Certified Platform
```

Enterprise (L3) requires certified: security · isolation · concurrency · migration · observability · contract ·
performance · replay · documentation. `module.toml` carries the claimed level; gates enforce it.

---

## 13. Foundation gates (G00–G20)

No business-domain implementation may bypass these gates.

| Gate | Scope | Acceptance |
|---|---|---|
| G00 | Public governance artifacts | all 12 root docs present, reviewed |
| G01 | Toolchain + repository | pinned toolchain, CI green, justfile one-command dev |
| G02 | Core type system | ids/AppError/planes/profiles/revisions + unit tests; clippy clean |
| G03 | ApplicationHost | providers boot in phase order; freeze enforced |
| G04 | Provider lifecycle | failing provider aborts boot with precise message |
| G05 | ExecutionContext | immutability by construction; builder covers all fields |
| G06 | ExecutionPlan | profiles resolve deterministically; plan validation tests |
| G07 | Policy/scope/ownership | ALLOW conjunction unit tests; revision staleness tests |
| G08 | Execution engine | end-to-end in-memory execution (example app) incl. idempotency replay |
| G09 | Persistence/transaction | tx engine + delta/audit/outbox in one commit (real DB test) |
| G10 | MVVM/HTTP | controller→viewmodel→command→envelope/problem+json tests |
| G11 | Module compiler | module.toml parse + manifest validation errors enumerated |
| G12 | Graph validator | cycles/dupes/missing-authz/missing-delta detected in test graph |
| G13 | Delta/audit/outbox | KER-03 enforced; streams monotonic; post-commit emission |
| G14 | Workflow/jobs | workflow compiler rejects bad graphs; durable job lifecycle test |
| G15 | CLI/generators | `keel doctor`, `keel make:module` produce compiling module |
| G16 | Diagnostics/replay | execution chain inspectable by id; replay modes typed (DRY_RUN safe) |
| G17 | Architecture verifier | dep-rule violations fail CI (analyzer run) |
| G18 | Performance harness | budgets table + first micro-bench (route lookup, authz decide) |
| G19 | Public API freeze | wave-1 crates' public surface listed + semver policy |
| G20 | Reference ERP | org→product→inventory→PO→approval→invoice slice on the platform |

**Foundation-first scope (this build): G00–G17 foundations landed as compiling, tested skeleton; G18–G20 follow.**

---

## 14. CLI surface (`keel`)

```text
keel new · dev · build · check · test · doctor · routes · graph [security|routes|modules|dependencies]
keel modules · capabilities · migrate · seed · worker · schedule
keel make:module|resource|command|query|workflow|policy|job|event|integration|migration|test
keel inspect execution|delta|authorization <id> · keel replay execution <id> · keel benchmark · keel release
```

`keel check` runs the analyzer and reports coded diagnostics:

```text
ARCH-001 controller directly accesses persistence      AUTH-011 mutation without declared capability
SCOPE-004 query missing tenant predicate/RLS coverage  DELTA-009 required mutation without delta policy
MOD-017 module imports another module's internals      CONTRACT-023 public route changed without versioning
MIG-004 destructive migration without exception        CONC-007 shared mutable resource without strategy
```

---

## 15. Validation suite, performance, resilience

- **validation/architecture**: dependency-boundary tests (CI-gated now).
- **validation/isolation**: RLS + plane cross-access harness against real Postgres (G09 companion).
- **validation/concurrency**: optimistic/pessimistic/serialized contention + lock-order tests.
- **validation/performance**: per-phase budget table (routing/context/authz/execution/serialization vs SQL);
  p50/p95/p99 + throughput + startup + build time; **hot-tenant SLO**: one hot tenant must not move p99 of
  unaffected tenants beyond budget (1 / 1k / 100k tenant shapes planned).
- **validation/chaos**: deterministic failure matrix (db timeout/disconnect, deadlock, serialization failure,
  outbox unavailable, external timeout/429/duplicate, stale revision, duplicate command, worker crash) — each
  failure must have specified behavior.
- Resilience: every external dependency documents timeout/connect/DNS/500/429/accepted-then-timeout behavior and
  duplicate risk (ports + adapters carry the matrix).

---

## 16. New decisions (D-44…D-53)

| ID | Decision |
|---|---|
| D-44 | Platform codename **Keel**; CLI `keel`; crates `keel-*`; folder `platform/` (veto-able) |
| D-45 | Repository topology frozen per §8 (spec-2 §55 refinement wins over spec-1 §5) |
| D-46 | `medicare-kernel/` superseded by this foundation; kept compilable until reference-healthcare rebuild |
| D-47 | Postgres GUC namespace renamed `medicare.*` → `keel.*` |
| D-48 | No ORM. SQLx first-class; convenience is generated around SQL |
| D-49 | Three data-access modes (aggregate / projection / reporting) replace repository-everywhere |
| D-50 | Platform contains zero healthcare knowledge; Medicare becomes a vertical pack + reference app |
| D-51 | Idempotency is execution-level (not HTTP middleware) |
| D-52 | Revisions, not TTLs, govern authorization/cache validity |
| D-53 | Events publish post-commit via outbox only |

---

## 17. Frozen architectural position

```text
APPLICATION → MVVM/CLI adapters → EXECUTION PLAN → EXECUTION ENGINE
  → (CONTEXT ∧ POLICY ∧ LIFECYCLE) → DOMAIN (WORKFLOW · STATE · COMMAND)
  → RESOURCE FABRIC (POSTGRES · CACHE · STORAGE) → DELTA · AUDIT · OUTBOX
```

**Small at the core, huge at the capability level, extremely strict at the contract boundaries.**