# docs/rust — Medicare Execution Kernel (Rust track)

> The Rust backend track for Medicare. Supersedes the Laravel API track (`docs/00..24` remain the **domain reference**: modules, flows, V2 stubs).
> Status: **0.96 — vendors & controls locked 2026-08-19 (D-11…D-27)**: both deploy forms · legacy import at launch · server-side OTP on **MSG91** (D-16) · FCM push (D-17) · SMTP mail (D-18) · MinIO self-host (D-19) · usage-cap SaaS pricing (D-20) · support mode (D-21) · export JSON+files+PDF (D-22) · external pen test (D-23) · legal track from W2 (D-24) · GH Actions CI/CD (D-25) · observability deferred (D-26) · subdomains at GA (D-27) · self-managed Postgres (D-28) · Razorpay+Stripe confirmed (D-29) · **10-week GA (D-15)**. All questions resolved (D-01…D-43; D-30–D-37 provisional, veto-able) — see `08-DECISIONS.md`.

| Doc | Contents |
|---|---|
| [00-PRD.md](00-PRD.md) | Product definition, pillars P0–P4, personas, scenarios, success metrics |
| [01-SRS.md](01-SRS.md) | Granular functional (KER/AUTH/ORG/POL/RT/DLS/…/MIG) + non-functional requirements, all verifiable |
| [02-SYSTEM-DESIGN.md](02-SYSTEM-DESIGN.md) | Modular monolith, workspace layout, kernel pipeline, scope lattice, smart router, delta engine |
| [03-DATA-MODEL.md](03-DATA-MODEL.md) | PostgreSQL schema: kernel DDL, module tables, RLS, indexing, migrations policy |
| [04-API-CONTRACTS.md](04-API-CONTRACTS.md) | REST conventions, endpoint inventory per module, sync/delta + webhook + realtime contracts |
| [05-SECURITY-ISOLATION.md](05-SECURITY-ISOLATION.md) | Two security planes, hierarchy + delta-level isolation, field policy, isolation matrix, compliance |
| [06-REALTIME-AND-SYNC.md](06-REALTIME-AND-SYNC.md) | WS hub, LISTEN/NOTIFY fan-out, nudge-not-payload push, offline sync protocol, scale path |
| [07-EXECUTION-PLAN.md](07-EXECUTION-PLAN.md) | M0–M8 milestones, task-level breakdown with estimates + acceptance criteria, risks, DoD |
| [08-DECISIONS.md](08-DECISIONS.md) | Locked decisions, assumptions, **open questions Q-01…Q-12 for stakeholders**, ADR log |
| [09-RESEARCH-FINDINGS.md](09-RESEARCH-FINDINGS.md) | Research pass: build-needs inventory, Laravel→Rust structure mapping, MNC-grade patterns, API standard, module system v2, tenant/platform separation v2, live dependency survey, **questions Q-15…Q-26** |
| [10-STRUCTURE-AND-GUIDELINES.md](10-STRUCTURE-AND-GUIDELINES.md) | Engineering norms: pattern verdict (Laravel/DDD/MVVM/hexagonal), module anatomy folders→files→functions with canonical code shapes, contract lifecycle, logging/PHI standard, process rules, **questions Q-28…Q-35** |
| [11-FOUNDATION-MASTER-SPEC.md](11-FOUNDATION-MASTER-SPEC.md) | Frozen foundation master spec: topology, dep rules, gates, D-44…D-53 |

**One-paragraph summary:** a Rust modular monolith acting as an ERP execution kernel — every request flows identity → scope lattice → capability → transaction → delta; isolation is enforced at tenant (RLS), hierarchy (grants), field (PHI classes) and stream (delta filter) levels; modules plug in through a compile-time contract and switch on/off per tenant via the smart router; one delta engine powers audit, offline sync, webhooks and reports without a broker.
## Keel foundation status (2026-08-20) — supersedes the M0 scaffold below

The authoritative platform foundation now lives at `platform/` (own Cargo workspace, 38 crates) — frozen spec: `11-FOUNDATION-MASTER-SPEC.md`, decisions D-44…D-53. `cargo check --workspace` and `cargo test --workspace --no-fail-fast` are green (~80 tests).

- **Framework:** keel-foundation (typed ids, envelope, AppError) · identity/auth · tenant + RLS · capability scope lattice · execution engine (units, operations, delta stones) · command bus · delta engine · policy engine · notify hub · scheduling · config · cache (moka) · storage (MinIO/S3) · db (sqlx, Tx) · api host (axum) · worker.
- **Toolchain:** `keel-console` — `doctor` / `check` (architecture rules) / `make:module` / `graph` / `modules` / `routes` / `capabilities` · keel-analyzer · keel-codegen · xtask dep-rules · `validation/architecture` CI-gate tests · `examples/minimal` golden-path proof (1 delta stone, replay true, stranger AUTH_DENIED).
- **Verified live:** `keel-api` boots degraded without DB (D-52) — `/healthz` 200 `{ok,degraded:true}`, `/readyz` 503 `KER_BOOT`, `/meta/boot` lists 7 services, `POST /api/v1/demo/echo` returns the standard envelope.
- **Governance:** 13 root governance docs + `platform/docs/` + ADRs · `.github/workflows/keel.yml` · `database/roles.sql`.
- `medicare-kernel/` is superseded by `platform/` (D-46) — kept compilable, will become `applications/reference-healthcare` later; the M0 section below is historical.

## M0 scaffold status (2026-08-19) (historical)

The Rust workspace now exists at `medicare-kernel/` (commands in its own `README.md`). `cargo check --workspace` and `cargo test --workspace` are green — 12 unit tests passing (capability 5, workflow 4, appointment 3).

- **Real, end-to-end:** kernel-core (typed ids, Money/Secret/Phi, error codes, Delta, Module contract) · kernel-db (pool, `Tx` with tenant GUC + KER-03 delta-required commit) · kernel-delta · kernel-auth (argon2 / JWT / OTP skeleton + Msg91 stub) · kernel-capability (scope-lattice decide + cache) · kernel-workflow (state machine) · kernel-notify (hub) · kernel-runtime (envelope, Services, smart-router assembly) · `modules/appointment` Tier-A reference slice (request validation → domain invariants → `FOR UPDATE` repo → same-tx delta) · migrations 0001/0002 · CI workflow · docker-compose (pg16 + minio) · typed config · justfile.
- **Scaffolded, returns 501 until its 07-EXECUTION-PLAN task:** org · clinic · doctor · patient · rx · billing · inventory · notifycfg · reports · platform — each a uniform compiling skeleton of the 10-GUIDELINES module anatomy.
- **Known residual warnings:** dead_code on some `apps/api` config fields; they are consumed when M2/M3 slices land.
- **External input still pending:** MSG91 DLT entity details (see 08-DECISIONS open questions) are required before real OTP sending.