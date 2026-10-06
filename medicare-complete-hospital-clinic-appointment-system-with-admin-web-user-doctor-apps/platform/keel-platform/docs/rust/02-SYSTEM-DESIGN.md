# 02 — System Design: Medicare Execution Kernel (Rust)

> **Version:** 0.9 draft-for-lock · **Date:** 2026-08-19 · **Parent:** `00-PRD.md`
> Design goal: a **modular monolith** whose kernel enforces identity → scope → capability → transaction → delta on every request, with business modules plugged in via a stable contract.

---

## 1. Top-Level Shape

```
                        ┌────────────────────────────────────────────┐
  HTTPS ──► nginx/edge  │              medicare-api (1 binary)       │
                        │                                            │
                        │  axum router ──► kernel middleware stack   │
                        │       │                                    │
                        │       ▼                                    │
                        │  SmartRouter ──► Module A │ Module B │ ... │
                        │       │              (all in-process)      │
                        │       ▼                                    │
                        │  Tx wrapper (SQLx + RLS GUC)               │
                        │       │                                    │
                        │       ▼                                    │
                        │  Delta engine ──► outbox ──► NOTIFY fanout │
                        └───────┼────────────────────────┬───────────┘
                                ▼                        ▼
                          PostgreSQL 16            WebSockets / Webhooks
                          (RLS, LISTEN/NOTIFY)     (in-proc hub)
```

One binary, one primary DB, in-process realtime hub. No broker, no ORM, no DI container. Service extraction is a later option because modules never share tables and all cross-module communication goes through deltas/events.

---

## 2. Workspace Layout

> **Updated by D-43 (2026-08-19):** the normative layout is now the Laravel-familiar design in `10-STRUCTURE-AND-GUIDELINES.md` §2 — top-level `modules/`, `kernel/`, `config/`, `database/`, `xtask/`. Crate names below map 1:1 (`crates/mod-appointment` → `modules/appointment`; `crates/kernel-*` → `kernel/kernel-*`).

```
medicare-kernel/
├─ Cargo.toml                  # workspace, deny.toml, rust-toolchain
├─ apps/
│  └─ api/                     # binary: composition root (modules wired here ONLY)
├─ crates/
│  ├─ kernel-core/             # traits + types: Module, Capability, Delta, Workflow,
│  │                           #   RequestContext, Clock/IdGen/Config traits
│  ├─ kernel-runtime/          # axum layers, SmartRouter, middleware stack, error mapping
│  ├─ kernel-db/               # pool, migrations runner, RLS session guard, Tx wrapper
│  ├─ kernel-delta/            # delta writer, cursor store, LISTEN/NOTIFY fanout, outbox
│  ├─ kernel-auth/             # sessions, JWT verify/sign, OTP (Firebase JWKS), argon2
│  ├─ kernel-capability/       # scope lattice eval, grants resolution, snapshot cache
│  ├─ kernel-workflow/         # declarative state machines, transitions, hooks
│  ├─ kernel-notify/           # realtime hub (WS), webhook deliverer, channel adapters
│  ├─ platform/                # platform plane: tenants, plans, break-glass, installer
│  ├─ mod-org/                 # org nodes, memberships, roles, grants, settings
│  ├─ mod-clinic/              # cities, clinics, departments, specializations, services
│  ├─ mod-doctor/              # doctors, schedules, slots
│  ├─ mod-patient/             # patients, family, vitals, files
│  ├─ mod-appointment/         # appointments, queue, check-in, live calling
│  ├─ mod-rx/                  # prescriptions, medicine master
│  ├─ mod-billing/             # invoices, ledger, payments, reconciliations
│  ├─ mod-inventory/           # stock, movements, procurement, PO workflows
│  ├─ mod-notifycfg/           # notification templates, channel config, sends
│  └─ mod-reports/             # projections + report endpoints
└─ xtask/                      # codegen: module scaffold, openapi emit, migration checks
```

**Dependency rule (enforced by CI):** `mod-*` may depend on `kernel-core` only (plus `kernel-workflow` traits). A module can never import a sibling module; cross-module interaction happens through deltas/events and kernel-defined ports. This is what keeps "plug n play" honest.

---

## 3. Kernel Type Sketch (kernel-core)

```rust
pub struct RequestContext {
    pub request_id: RequestId,
    pub subject: Subject,            // identity + plane + memberships
    pub tenant: TenantCtx,           // tenant_id + plan + feature snapshot
    pub api_version: ApiVersion,
    pub idempotency_key: Option<IdempotencyKey>,
    pub trace: TraceCtx,
}

pub trait Module {
    fn manifest(&self) -> ModuleManifest;           // id, version, stream names
    fn migrations(&self) -> &[Migration];
    fn routes(&self) -> Vec<RouteSpec>;             // consumed by SmartRouter
    fn workflows(&self) -> Vec<WorkflowDef>;
    fn streams(&self) -> Vec<StreamDef>;            // delta partitions this module owns
    fn capabilities(&self) -> Vec<CapabilityDef>;   // actions this module guards
    fn events(&self) -> Vec<EventDef>;              // cross-module intents
}

#[async_trait]
pub trait Handler {
    async fn call(&self, ctx: &RequestContext, tx: &Tx, req: Decoded) -> Outcome;
}
```

Everything a business developer touches is behind these traits. The kernel owns the pipeline; modules own domain behavior.

---

## 4. Request Pipeline (per request)

```
1. Edge            : TLS, basic rate limit, request_id
2. Decode          : method/path → candidate RouteSpec set (SmartRouter)
3. AuthN           : kernel-auth resolves Subject (JWT / OTP session / platform token)
4. Plane gate      : platform tokens may not enter tenant routes (except break-glass)
5. Tenant resolve  : from Subject membership or explicit X-Tenant (validated)
6. Feature gate    : module enabled for tenant plan? → else 403 module.disabled
7. Version gate    : route.min_api_version ≤ ctx.api_version
8. Canary          : cohort hash → handler variant (stable | canary)
9. Capability      : kernel-capability resolves action against resource scope
10. Idempotency    : replay check for mutating ops
11. Tx             : open tx, SET LOCAL medicare.tenant_id, run Handler
12. Deltas+outbox  : written in same tx
13. Commit         : NOTIFY payload published after commit
14. Respond        : envelope + meta (etag/cursor), metrics + audit trace
```

Every stage is a `tower::Layer`, individually unit-tested. Stages 3–9 form the **hot path**; they operate on an in-memory snapshot cache, not per-request DB queries.

---

## 5. Scope Lattice & Capability Engine

### 5.1 Hierarchy

```
tenant
 └─ legal_entity (1..n)
     ├─ region (0..n)
     ├─ business_unit (0..n)
     └─ location (1..n)      ← "clinic"
         ├─ department (0..n)
         ├─ warehouse (0..n)
         └─ cost_center (0..n)
```

- Physical table `org_nodes(id, tenant_id, kind, parent_id, name, path, meta)` where `path` is materialized (`LE1/REG2/LOC5/DEP3`).
- Every aggregate row carries `owner_node` (the hierarchy node that owns it). Ownership is set at creation and is the anchor for all scope decisions.
- Moves/renames rewrite subtree paths in one transaction; a `path` index (GIN trigram + ltree-style prefix) serves ancestor/descendant queries.

### 5.2 Grants

```
grant(id, subject_type, subject_id, action_class, node_path_prefix,
      module, conditions jsonb, valid_from, valid_until, granted_by)
```

- `action_class` examples: `appointment.read`, `appointment.write`, `invoice.approve`, `stock.adjust`, `patient.phi.read`.
- Effective scope for a Subject = **union of all node_path_prefixes** across direct grants + role grants + active delegations.
- Capability check = "resource.owner_node.path **starts with** one of my granted prefixes **AND** action class allowed **AND** module enabled".

### 5.3 Snapshot Cache

- Per `(subject_id)` capability snapshot: resolved prefixes + actions + version stamp.
- Built lazily, cached in-process (moka-style map, hand-rolled or `moka` if accepted — see Q-04), invalidated by kernel-delta subscription on grant/membership/org changes.
- This is why the hot path stays sub-2 ms: authorization is a prefix compare against cached data.

---

## 6. Smart Router

The router is the "plug n play switchboard":

- At startup, the composition root collects `RouteSpec`s from every wired module and compiles a match tree keyed by `(api_version, path, method)`.
- Each `RouteSpec` carries **gates**: `required_capability`, `module_id`, `min_plan_tier`, `flag_name`, `canary_weight`, `handler_variant`.
- Dispatch evaluates gates in order (cheapest first): version → module flag → plan tier → feature flag → canary cohort → capability.
- **Canary:** `cohort = xxhash64(tenant_id) % 100`; route config `canary: {pct: 5, variant: "new_pricing"}`. Changing pct is a config write, not a deploy. Per-tenant overrides supported via feature flags.
- **API versioning:** `/api/v1`, `/api/v2` are distinct route trees sharing handlers where possible; router rejects unknown major with a machine-readable error listing supported majors.
- **Module enable/disable:** disabling a module for a tenant flips its routes to `403 module.disabled` without removing code. Registrar also powers `GET /api/v1/meta/modules` for clients to adapt UI.

---

## 7. Delta Engine (isolation backbone)

### 7.1 Record shape

```
delta(partition smallint, seq bigserial,           -- per-partition sequence
      tenant_id, stream, aggregate, aggregate_id,
      action,                                       -- create|update|transition|compensate
      state jsonb, changed jsonb,                   -- full post-state + minimal diff
      owner_node_path, actor_id, capability_grant_id,
      causation_id, correlation_id, occurred_at, version)
```

### 7.2 Guarantees

1. **Same-transaction write** — a mutation without its delta cannot commit (`Tx` wrapper asserts at least one delta for mutating handlers; kernel enforced).
2. **Monotonic per (tenant, stream)** — consumers resume by `seq`; replays are idempotent by `(aggregate_id, version)`.
3. **Scoped emission** — each delta carries `owner_node_path`; a consumer session (realtime channel, webhook, sync cursor) is created with an allowed prefix set computed from the subject's capability snapshot. Fanout filters `owner_node_path LIKE prefix || '%'` before delivery. This is **delta-level isolation**: isolation is enforced on *change streams*, not just on queries.
4. **Outbox** — webhook/push side-effects live in `outbox`, drained by a worker after commit; crash-safe, at-least-once, with dedupe keys.

### 7.3 Streams & partitioning

- Partition key `smallint` maps stream → partition (appointments, invoices, stock, audit…). `seq` is generated per partition via dedicated sequences, so hot streams never block the audit stream.
- Retention per stream (e.g., audit: 7y; sync streams: 90d, then snapshot + tail).

### 7.4 Fanout path

```
commit → NOTIFY kernel_deltas {tenant, stream, seq_range, owner_node_path}
hub receives → matches subscribed channels → per-subject prefix filter → WS frame / webhook enqueue
```

Single-node: in-process hub only. Multi-node without broker: every node `LISTEN`s the same channel; fanout remains correct because filtering is per-subject. (Broker upgrade path documented in `06-REALTIME-AND-SYNC.md`.)

---

## 8. Workflow Kernel

- Declarative `WorkflowDef`: states, transitions `(from, event, to, guard, hooks[])`.
- Guards are capability or domain predicates; hooks run inside the same Tx (e.g., on `appointment.checked_in` → enqueue token assignment, emit event).
- Every transition writes a `transition` delta; the state machine is auditable by design and testable in pure unit tests (no DB).
- Approval chains (procurement, refunds above threshold, cancellation overrides) are workflows over a generic `approval_request` aggregate: maker submits, checker approves/rejects, delegation + SoD rules apply.

---

## 9. Configuration & Secrets

- `config.toml` per env + environment overrides (`MEDICARE_*`), validated at boot into a typed `Config` (fail fast).
- Secrets (DB URL, JWT keys, gateway keys) only via env/secret store; boot refuses to start if a required secret is missing or if a non-localhost bind is attempted without TLS material.
- Per-tenant runtime config lives in DB (feature flags, settings) — never in the binary config.

---

## 10. Error Model

- Envelope: `{ error: { code: "capability.denied", message, details{}, request_id } }`.
- Codes are stable strings namespaced by stage (`auth.*`, `tenant.*`, `module.*`, `workflow.*`, `plan.*`, `validation.*`, `conflict.*`).
- Handlers never leak stack traces to clients; internal errors map to `500 internal` + full trace in logs with request_id.

---

## 11. Observability

- `tracing` span per pipeline stage; request_id propagated to DB (session GUC) so PG logs and app logs correlate.
- Metrics: request count/latency by route+result, capability denials, delta lag per stream, outbox depth, WS connections, cache hit ratio.
- `/healthz` (liveness), `/readyz` (DB + cache warm), `/metrics` (Prometheus).

---

## 12. Deployment Topology — Both Forms from GA (D-11)

```
VPS: nginx (TLS) ──► medicare-api (1 binary, N workers)
                     ├─ PostgreSQL 16 (self-managed on host, D-28)
                     ├─ object storage (files, exports)
                     └─ WAL-G → backup bucket
```

- **`deploy_mode = standalone`** (per-customer self-host): installer activates domain, seeds exactly one tenant + local admin; platform plane collapses to local ops endpoints; plans/limits disabled; upgrades via binary swap; **MinIO bundled for object storage (D-19)**; email via operator SMTP (D-18).
- **`deploy_mode = saas`** (shared deployment): full platform plane, plan/limit enforcement, per-tenant canary cohorts, break-glass, subdomain/path tenant resolution.
- Same binary, same schema, same isolation guarantees; mode is config-only — no code fork between forms.
- Scale step 1: 2 API nodes behind LB (stateless; realtime works via LISTEN/NOTIFY across nodes).
- Scale step 2: read replicas for reports; outbox workers split into their own process (same binary, `--worker` flag).
- Containers optional: the binary + PG is the whole system; Docker Compose provided for dev and for operators who want it.