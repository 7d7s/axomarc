# 10 — Structure & Engineering Guidelines: Surpassing Laravel, Deep Definitions

> **Version:** 0.96 · **Date:** 2026-08-19 · **Scope:** norms for folders, files, code, functions, API contracts, logging, and process. Every rule is CI-checkable or review-checkable; guidelines without enforcement erode, so each carries its enforcement mechanism.
> Deep questions raised by this doc: §9 (Q-28…Q-35).

---

## 1. Pattern Verdict — what we take from DDD, MVVM, hexagonal, and why it beats Laravel

We are not "doing DDD" or "doing MVC". We take the strongest piece of each pattern and assign it to the layer it actually fits:

| Pattern | Verdict | Where used | Where rejected |
|---|---|---|---|
| Laravel-style (service providers, facades, fat controllers) | **Rejected as structure** | — | Runtime magic, implicit bindings, convention-enforced bounds (drift); our `Module` trait replaces service providers with compile-time contracts |
| MVC | **Partially adopted** | axum handler = controller role; serde DTOs = view contract | No "model" logic in controllers — handlers stay ~15 lines |
| MVVC (server-side presentation) | **Adopted** — see ADR 0021 | The three layers are framework-provided: the **Model** (operations, models, read models), the **ViewModel** (a typed, declared presentation state) and the **View** (a template bound to a ViewModel). The binder is new template syntax (`@vm` + schema-checked paths, `@command`); the `ViewContext` is assembled once by the framework | No client-side data binding and no client-side state machine — the server stays the source of truth; a ViewModel produces data, never markup |
| DDD — strategic | **Fully adopted** | Bounded context = module crate; context map = delta streams + declared ports; ubiquitous language = shared glossary per module in doc headers | — |
| DDD — tactical | **Tiered adoption (Q-28)** | Tier A aggregates get entities, invariants, state machines (02 §8), property tests | Tier B masters stay thin CRUD — full DDD there is ceremony with zero payoff |
| Hexagonal / Ports & Adapters | **Adopted at module edges** | `domain` never imports sqlx/reqwest; repos + provider adapters implement ports | Purity beyond the edge (e.g., domain-layer DI abstractions) — no |
| Vertical slice | **Adopted inside modules** | A feature = one named handler + one service fn + one repo fn, collocated by file-role | One-file-per-slice sprawl — rejected; layer files keep diff-noise low |
| CQRS-lite | **Adopted** | Writes go command → workflow → delta; reads use direct queries + projections (reports) and may bypass rich domain | Full event-sourced CQRS — deferred (deltas already give us 80%) |

### 1.1 Aggregate tiers (normative)

| Tier | Rule | Modules/aggregates |
|---|---|---|
| **A — rich domain** | State machine required; invariants encoded in constructors/methods; property tests; every mutation via workflow kernel | appointments, queue tokens, invoices + ledger, payments/refunds, stock movements, prescriptions, grants/delegations, tenants, workflows |
| **B — thin CRUD** | Validated create/update + delta emission; no state machine unless a lifecycle emerges; read endpoints may skip service layer | cities, clinics profiles, departments, specializations, services, patients (demographics), family members, medicines master, templates, settings |
| **Kernel** | Never mutated by modules except through kernel APIs | sessions, grants, org_nodes, deltas, outbox, feature_flags |

### 1.2 Why this surpasses Laravel (mechanism by mechanism)

| Laravel reality | Kernel replacement | Concrete gain |
|---|---|---|
| Module bounds by folder convention | CI blocks cross-module imports; context map in manifest | Boundaries that cannot rot |
| Eloquent runtime SQL | `sqlx::query!` compile-time checked | SQL errors caught at build, not 3am |
| `authorize()` remembered per controller | Capability stage in pipeline, deny-by-default | Forgetting is impossible |
| `tenant_id` WHERE-clause discipline | RLS GUC + 3 isolation layers + matrix suite | Proven, not hoped |
| Events fire-and-forget (queue) | Delta + outbox in the mutation tx | Zero event loss; audit is the same stream |
| Swagger hand-maintained | RouteSpec → generated OpenAPI + CI contract diff | Docs cannot drift |
| Migrations mixed per package | Ordered stream + expand/contract rule | Deterministic upgrades |
| Policy sprawl for domain rules | Tier-A state machines with exhaustive transitions | Illegal states unrepresentable |

---

## 2. Project Structure — Laravel-Familiar Folder Design (D-43, approved 2026-08-19)

The team moves from Laravel, so the workspace mirrors Laravel's structure: everything sits where a Laravel developer's muscle memory expects it — while kernel boundaries enforce what Laravel could only suggest by convention.

### 2.1 Workspace ↔ Laravel mapping

| Laravel | medicare-kernel (Rust) | Role |
|---|---|---|
| `bootstrap/app.php` | `apps/api/src/main.rs` + kernel-runtime | app bootstrap / composition root |
| `routes/api.php` | `modules/<name>/src/routes.rs` | route registration, compiled by SmartRouter |
| `app/Http/Controllers` | `modules/<name>/src/http/controllers.rs` | thin handlers (~15 lines) |
| `app/Http/Requests` (form requests) | `modules/<name>/src/http/requests.rs` | wire DTOs + validation (`*Req`) |
| `app/Http/Resources` | `modules/<name>/src/http/resources.rs` | response DTOs (`*Resp`) |
| `app/Services` | `modules/<name>/src/services/` | use-case orchestration |
| `app/Models` (Eloquent) | `modules/<name>/src/models/` | domain aggregates — **active domain objects, not ORM rows** |
| (Eloquent query layer) | `modules/<name>/src/repositories/` | the Eloquent replacement: all SQL, row ↔ model mapping |
| `app/Events` + listeners | `modules/<name>/src/events/` | delta builders + EventDef + listeners |
| `app/Policies` | `modules/<name>/src/policies.rs` | CapabilityDef list (kernel evaluates grants) |
| Laravel state machines (ad-hoc) | `modules/<name>/src/workflows/` | Tier-A state machines (kernel-workflow) |
| `app/Console` / artisan | `xtask/` | codegen, scaffolds, contract checks |
| `database/migrations` | `modules/<name>/migrations/` → merged ordered stream | migrations |
| `database/seeders` | `database/seeders/` | demo tenant + matrix fixtures |
| `config/*.php` | `config/*.toml` → typed `Config` | boot-time validated config |
| `storage/app` | MinIO (D-19) | files |
| `vendor/` (Composer) | crates.io workspace deps (< 40 budget) | third-party code |
| first-party packages | `modules/*` crates | plug-n-play modules |
| `tests/Unit`, `tests/Feature` | `#[cfg(test)]` + `modules/<name>/tests/` + `tests/` | unit / contract / matrix |

### 2.2 Workspace tree (normative)

```
medicare-kernel/
├─ apps/api/                    # = bootstrap/app.php — the only binary
├─ config/                      # environments.toml, secrets via env
├─ database/seeders/            # demo tenant, matrix fixtures
├─ migrations/                  # merged ordered migration stream (xtask-built)
├─ kernel/                      # "the framework": kernel-core, kernel-runtime, kernel-db,
│                               #   kernel-delta, kernel-auth, kernel-capability,
│                               #   kernel-workflow, kernel-notify
├─ modules/                     # = first-party packages (one crate per module)
│  └─ appointment/
│     ├─ Cargo.toml
│     ├─ src/
│     │  ├─ lib.rs              # = service provider: impl Module + manifest; ONLY public surface
│     │  ├─ module.rs           # ModuleManifest const
│     │  ├─ routes.rs           # = routes/api.php slice
│     │  ├─ http/
│     │  │  ├─ controllers.rs   # thin handlers
│     │  │  ├─ requests.rs      # *Req + validation (form-request equivalent)
│     │  │  └─ resources.rs     # *Resp (API-resource equivalent)
│     │  ├─ services/           # use-case fns (commands/, queries/ split above ~500 lines)
│     │  ├─ models/             # aggregates + invariants + DomainError
│     │  ├─ repositories/       # ALL SQL (sqlx::query!), row structs
│     │  ├─ events/             # delta payload builders, stream consts
│     │  ├─ workflows/          # Tier A only: state machine defs
│     │  └─ policies.rs         # action classes this module guards
│     ├─ migrations/            # contributed SQL
│     └─ tests/
│        ├─ contract.rs         # API contract + isolation cells
│        └─ domain_props.rs     # property tests (Tier A)
├─ xtask/                       # = artisan: scaffold-module, openapi emit, migration merge
├─ tests/                       # matrix + e2e (workspace level)
└─ docs/rust/                   # this spec set
```

### 2.3 Rules (enforcement)

- **Public surface = `lib.rs` only**; everything else `pub(crate)` unless a kernel port requires it (compiler + CI import check).
- **Layer discipline:** controllers → services → repositories; controllers never call repositories; models never import sqlx/reqwest; services never return wire types (shape rules in §4).
- **File budget:** soft cap 400 lines; split by role (http/services/models/repositories), never by entity name.
- **Every file starts with a doc header:** one line of purpose + doc anchor (CI lint `missing_doc_header`).
- **Migrations live with the module**, merged by xtask into one ordered stream; ordering conflicts fail CI.
- **Tests colocated:** pure logic `#[cfg(test)]` in-file; DB/API in `tests/`.
- Kernel crates keep their own internal layering (they are "the framework", not a module) — Laravel devs: think of `kernel/` as `vendor/laravel/framework` that we own and can change.

---
## 3. Code Guidelines — files, naming, types, errors

### 3.1 Naming (rustfmt can't help you here; review does)

| Thing | Convention | Good | Bad |
|---|---|---|---|
| Module directory | `modules/<aggregate>` | `modules/appointment` | `booking-svc`, `appointment-v2` |
| Handler fn | verb_noun, matches use case | `create_appointment` | `handler`, `post1` |
| Service fn | same verb_noun as handler | `create` | `do_create_appointment` |
| Repo fn | verb_noun, SQL intent visible | `lock_open_slot` | `get_slot_2` |
| Wire DTO | `<Op>Req` / `<Op>Resp` | `CreateAppointmentReq` | `AppointmentDto2` |
| Domain error | exhaustive enum, no strings | `DomainError::SlotTaken` | `anyhow!("slot taken")` |
| Stream consts | SCREAMING, module-prefixed | `STREAM_APPOINTMENTS` | `stream1` |
| Feature flags | kebab-case strings | `recurring-appointments` | `flagRecAppt` |

### 3.2 Type rules

- **Newtypes for identity and money** (compile-time correctness): `AppointmentId(Uuid)`, `SlotId(Uuid)`, `MoneyCents(i64)` — **money is integer minor units + currency code; `f64` for money is a firing-level bug**, enforced by clippy lint `float_cmp` + review.
- **`Secret<T>` / `Phi<T>` wrappers** for sensitive fields: they implement neither `Debug` nor `Display`, so they *cannot* be logged accidentally (compile-time PHI protection — pairs with logging §7).
- Time is `time::OffsetDateTime`, always tz-aware; slot math uses tenant timezone from settings; never store local-naive timestamps.
- Enums over string columns in Rust (DB may store text for legibility; mapping is in repo.rs).

### 3.3 Error rules

- Domain: `thiserror` enum, exhaustive, no `anyhow` in library crates.
- Service: maps domain/repo errors into `ServiceError` with kernel error codes attached (`workflow.invalid_transition`, `conflict.slot_taken`…).
- API boundary: single conversion into `ApiError` → envelope (§6.4). Handler never formats error strings itself.
- Every error variant that reaches clients gets a stable code + doc page (04 §1, G-07).

### 3.4 SQL rules

- All SQL in `repositories/` using `sqlx::query!`/`query_as!` (compile-time checked against dev DB).
- Mutating repo fns take `&mut Tx` and **never** open their own transaction; transaction scope belongs to the kernel `Tx` wrapper.
- `FOR UPDATE` locks are mandatory for contention rows (slots, stock batches) and must have a matching concurrency test.
- No `SELECT *`; column lists explicit (delta state shapes depend on stable columns).

---

## 4. The Four Canonical Function Shapes (normative templates)

### 4.1 Handler (http/controllers.rs) — thin, ~15 lines, no logic

```rust
/// POST /api/v1/appointments — spec: 04-API-CONTRACTS.md §8 (KER-05 idempotent)
#[tracing::instrument(skip(tx), fields(request_id = %ctx.request_id))]
pub async fn create(
    ctx: RequestContext,                       // kernel extractor (authn/tenant/snapshot done)
    tx: TxExtractor,                           // kernel extractor: RLS GUC set, tx open
    Json(body): Json<CreateAppointmentReq>,
) -> Result<Json<Envelope<AppointmentResp>>, ApiError> {
    let input = CreateAppointmentInput::try_from(body)?;      // wire → domain, validation here
    let appt = service::create(&ctx, tx, input).await?;       // one service call
    Ok(Envelope::created(AppointmentResp::from(appt)))        // domain → wire
}
```

### 4.2 Service (services/) — orchestration, no SQL, no HTTP types

```rust
#[tracing::instrument(skip(tx), fields(slot_id = %input.slot_id))]
pub async fn create(ctx: &RequestContext, mut tx: Tx, input: CreateAppointmentInput)
    -> Result<Appointment, ServiceError>
{
    let slot = repo::lock_open_slot(&mut tx, input.slot_id).await?;     // infra
    let appt = Appointment::book(ctx.subject(), &slot, &input)?;        // domain decides
    repo::insert_appointment(&mut tx, &appt).await?;                    // infra
    repo::book_slot(&mut tx, slot.id, appt.id).await?;                  // infra
    tx.emit(events::created(&appt, ctx))?;                              // kernel: delta in same tx
    Ok(appt)
}
```

### 4.3 Domain (models/) — pure, no IO, fully unit-tested

```rust
impl Appointment {
    pub fn book(subject: Subject, slot: &Slot, input: &CreateAppointmentInput)
        -> Result<Self, DomainError>
    {
        ensure!(slot.state == SlotState::Open, DomainError::SlotTaken);
        ensure!(slot.starts_at >= Clock::now() + lead_time(), DomainError::LeadTime);
        Ok(Self {
            id: AppointmentId::new(), status: Status::Scheduled, version: 1,
            owner_node: slot.node_id, patient_ref: input.patient_ref,
            starts_at: slot.starts_at, fee_snapshot: input.fee_snapshot.clone(),
        })
    }
}
```

### 4.4 Repo (repositories/) — SQL only, row ↔ model mapping

```rust
pub async fn lock_open_slot(tx: &mut Tx, id: SlotId) -> Result<Slot, RepoError> {
    let row = sqlx::query_as!(SlotRow,
        r"SELECT id, doctor_user_id, node_id, starts_at, state, hold_until
          FROM slots WHERE id = $1 AND tenant_id = current_setting('medicare.tenant_id')::uuid
          FOR UPDATE", id.0)
        .fetch_optional(tx.as_mut()).await?;
    slot_from_row(row.ok_or(RepoError::NotFound)?).map_err(Into::into)
}
```

Shape rules: handler DOES NOT call repo; service DOES NOT return wire types; domain DOES NOT import sqlx; repo DOES NOT make decisions (a repo returning "not found" is a fact, "not allowed" is never a repo concern).
---

## 5. Testing Guidelines (normative, per layer)

| Layer | What | Where | Tools | Pass bar |
|---|---|---|---|---|
| Domain pure | Invariants, state machines, fee math | `#[cfg(test)]` in domain.rs + `tests/domain_props.rs` (Tier A) | proptest for properties | 100% transition coverage incl. illegal paths |
| Repo | Query correctness under RLS | kernel harness + fixture tenant | sqlx test db | Row-level isolation cells green |
| Service | Orchestration + delta emission | `tests/contract.rs` | kernel harness | Every op emits ≥1 delta (KER-03) |
| API contract | Envelope, codes, idempotency, version gates | `tests/contract.rs` | harness + recorded fixtures | Matches OpenAPI artifact exactly |
| Isolation matrix | subjects × ops × resources (05 §7) | tests/matrix/ | harness, real PG | 0 leaks — CI gate from M2 |
| Load | NFR-P targets | k6 scripts | k6 + staging | p99 tables in 07 M8-01 |

Rules:
- A PR without tests for changed behavior is blocked (CI coverage diff check; kernel crates ≥ 85%).
- Concurrency tests are mandatory for every `FOR UPDATE` (slot booking, stock issue) — spawn N tasks, assert exactly one winner (APT-02 pattern).
- Fixtures: synthetic tenants generated by harness (`tenant_with_org(depth)`), never production-shaped fake data sitting in repo files.

---

## 6. API Design & Contract Lifecycle (deep)

### 6.1 Contract pipeline (normative flow, CI-enforced)

```
1. PROPOSE   OpenAPI fragment diff in PR (before handler code)
2. REVIEW    checklist: capability set, idempotency, delta stream, version impact, rate class
3. IMPLEMENT handler/service/repo per §4 shapes
4. VERIFY    contract tests compare responses against the OpenAPI fragment (xtask)
5. PUBLISH   OpenAPI artifact + generated TS/Dart SDKs tagged with release (09 §5.7)
6. LOG       changelog entry auto-generated from RouteSpec diff (added/deprecated)
```

Breaking-change machine check runs on every PR: removals/renames/type-changes on `/api/v1` fail CI; additions pass (RT-04).

### 6.2 Canonical example — booking (request/response/error/delta)

```json
// POST /api/v1/appointments   (Idempotency-Key: 7f3b…)
{
  "slot_id": "0194f2a8-…",
  "patient_ref": { "kind": "self" },
  "service_id": "0194c001-…",
  "reason": "follow-up"
}

// 201 Created
{
  "data": {
    "id": "0194f2b0-…", "status": "SCHEDULED", "starts_at": "2026-08-21T10:30:00+05:30",
    "queue_hint": null, "allowed_events": ["check_in", "cancel_request"],
    "version": 1
  },
  "meta": { "request_id": "req_01H…", "etag": "1" }
}

// 409 on slot race
{ "error": { "code": "conflict.slot_taken", "message": "Slot is no longer available.",
  "details": { "slot_id": "0194f2a8-…", "doc_url": "https://docs.medicare.dev/errors/conflict.slot_taken" },
  "request_id": "req_01H…" } }
```

```json
// delta emitted in the same transaction (sync stream "appointments")
{ "stream": "appointments", "seq": 10923, "aggregate": "appointment",
  "aggregate_id": "0194f2b0-…", "action": "create", "version": 1,
  "state": { "status": "SCHEDULED", "starts_at": "…", "doctor": "…", "clinic": "…" },
  "changed": {}, "owner_path": "LE1|LOC5", "actor_id": "…", "capability_id": "…" }
```

### 6.3 Contract rules beyond doc 04

- Every resource response carries `version` (optimistic concurrency) and `allowed_events` (drives client buttons without client-side state machines).
- State changes are always `POST /{res}/{id}/transition` with `{ "event": "…" }`; server rejects unknown events with `workflow.invalid_transition` + `allowed_events`.
- Validation failures: `validation.failed` + `details.fields: [{path, rule, message}]` — stable paths, clients may bind forms to them.
- Lists always support `limit` (≤200) + opaque cursor; search-style endpoints declare their sort whitelist in OpenAPI.
- Idempotency results are byte-identical replays of the original envelope (including `request_id` of the original, noted in `meta.replay_of`).
- Webhook payloads = delta rows verbatim (no second serialization dialect).

### 6.4 Error design rules

- Code grammar: `stage.subject` (`conflict.slot_taken`, `capability.denied`, `workflow.invalid_transition`); stable forever within a major.
- Handlers throw structured errors; only ONE place (kernel-runtime boundary) converts to envelope — grep for handcrafted error JSON to find violations.
- Internal errors never leak internal type names; 500 body is generic + request_id (trace holds the truth).

---

## 7. Logging & Observability Guidelines (normative)

### 7.1 Three records, three purposes (never mix)

| Record | Purpose | Store | Mutability |
|---|---|---|---|
| **Logs** | runtime diagnostics for humans/on-call | stdout JSON → files/shipper | ephemeral (7–30d) |
| **Deltas** | system of record: sync, audit replay, CDC | Postgres `deltas` | immutable |
| **Metrics** | alerting/dashboards | Prometheus `/metrics` | aggregates only |

Rule: if a fact matters to the business or compliance, it MUST be a delta or audit row — logs are never the source of truth (DLS-05/DLS-07).

### 7.2 Structured log standard

Spans (tracing): `request{request_id, route}` → `stage{...}` → `service{module, use_case}` → `sql{db.op}`.
Fields on every request span:

```
request_id, tenant_id, actor_id, plane, api_version, module,
route, method, status, latency_ms, error_code?, idempotent_replay?
```

Rules:
- `tenant_id`/`actor_id` attached once at pipeline entry; never re-derived ad-hoc.
- DB sessions carry `application_name = medicare:<request_id>` so PG logs correlate 1:1.
- Level policy: ERROR = actionable failure (page), WARN = degraded/recovered, INFO = lifecycle + money events only (tenant created, invoice posted, break-glass opened), DEBUG = flow (default off in prod), TRACE = payloads (never prod).
- Log volume budget: ≤ 10 log lines per request at INFO — enforced by review; noisy loops must aggregate (`processed=500, failed=2` not 502 lines).

### 7.3 PHI & secrets redaction (compile-time first, filter second)

1. Sensitive values live in `Secret<T>`/`Phi<T>` newtypes that implement neither Debug nor Display → accidental logging is a compile error.
2. Tracing filter strips known keys (`password*`, `otp*`, `token`, `authorization`, `aadhaar*`) as defense-in-depth.
3. Never-log list (normative): OTP codes, tokens/keys, passwords, webhook secrets, national IDs, diagnosis text, prescription bodies, full addresses, file contents. Log the *reference* (`patient_file_id=…`) instead.
4. Tenant-plane logs MUST NOT contain other tenants' ids even in error paths (error constructors take tenant-scoped context only).

### 7.4 Metrics & alerts baseline

```
medicare_http_request_duration_seconds{route,method,status}   histogram
medicare_capability_denied_total{action,module}               counter   → alert on sustained spike
medicare_delta_lag{stream}                                    gauge     → alert > 5000
medicare_outbox_depth                                         gauge     → alert > 1000
medicare_ws_connections{node}                                 gauge
medicare_isolation_matrix_result{suite}                       gauge 0/1 → page if 0
medicare_otp_send_total{result}                               counter (abuse watch)
```

Self-hosted observability (D-26 deferred): JSON lines to stdout + logrotate bundle in compose; Prometheus scrape endpoint always on; stack choice (Grafana/Loki) in fast-follow — decisions don't block any code here.

---

## 8. Process Guidelines

- **Task-branch PRs:** branch `m4-03-doctor-memberships`, first line `M4-03: …`, body links task row + gate effect.
- **Review gates:** 1 approval normal; kernel-* crates and any RLS/migration change need 2 approvals + matrix green.
- **Migration authoring (expand/contract):** add nullable/with-default → backfill → switch reads → drop old in NEXT release; `CREATE INDEX CONCURRENTLY`; no table rewrite locks during business hours.
- **Versioning:** binary `vX.Y.Z`; API majors additive-only; module manifests semver with kernel range; webhook contracts versioned with the delta schema_version.
- **Config discipline:** every new config key typed in boot validation with a default and doc line; no runtime ENV reads outside boot.
- **Dependency additions:** ADR in 08 + `cargo deny` green + budget check (NFR-S6).
- **Tech debt:** `TODO(D-XX)` tags only with a decision/task id; bare TODOs fail a weekly lint report.

---

## 9. Deep Structured Questions (Q-28…Q-35)

> **Status 2026-08-19:** Q-28…Q-35 recommendations provisionally adopted (D-30…D-37, veto-able); chat answers 9–14 locked (D-38…D-43). §2 rebuilt as the Laravel-familiar layout per D-43.

Answer one line each; recommendations are tuned for the 10-week plan.

**Architecture & structure**
| # | Question | Recommendation |
|---|---|---|
| Q-28 | Confirm **tiered DDD**: rich domain (state machines, property tests) only for Tier A (appointments, money, stock, Rx, grants); thin CRUD for masters? | Yes — full DDD everywhere would burn the schedule for zero clinical gain |
| Q-29 | Client repos: polyrepo (backend / admin-web / patient+doctor mobile / published SDKs) or one monorepo? | Polyrepo + SDK packages from OpenAPI (D-25 CI already assumed) |
| Q-30 | Admin web stack detail: Next.js server components + generated TS SDK + TanStack Query for dynamic panes — approve or adjust? | As stated |
| Q-31 | Flutter state management for both apps: Riverpod vs BLoC? (affects SDK consumption patterns + offline sync store) | Riverpod + drift-backed sync store; but the mobile team's existing skill wins if they vote BLoC |

**Engineering quality**
| # | Question | Recommendation |
|---|---|---|
| Q-32 | Clippy strictness: default lint set with `-D warnings`, or enable extra groups (`clippy::pedantic` subset) from day 1? | Default + `-D warnings` + our 10 curated lints (float money ban, unwrap ban in app paths…) |
| Q-33 | Self-hosted log stack: JSON files + logrotate shipped at GA, Loki/Promtail bundled optionally? | Files at GA; Loki as compose profile (D-26 consistent) |
| Q-34 | Error docs site hosting (`docs.medicare.dev/errors/*`): your domain/infra or a static site bundled with release artifacts? | Static site published with release; custom domain later |
| Q-35 | Release rhythm: weekly release train (Tue) + hotfix flow, first train W6? | Yes |