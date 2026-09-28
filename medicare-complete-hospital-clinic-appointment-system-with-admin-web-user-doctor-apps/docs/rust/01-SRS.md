# 01 — SRS: Medicare Execution Kernel (Rust)

> **Version:** 0.9 draft-for-lock · **Date:** 2026-08-19 · **Parent:** `00-PRD.md`
> Every requirement has a stable ID, a pillar tag (P0–P4), and a verification method.
> Verification keys: **U** unit · **I** integration · **P** property test · **M** isolation matrix · **L** load · **A** manual/audit

---

## 1. Conventions

- IDs: `<AREA>-<NN>`; pillar shown in brackets.
- MUST = gate-blocking. SHOULD = tracked, may slip one milestone. MAY = optional.
- All IDs are immutable once published; deletions are marked `[WITHDRAWN]`, never reused.

---

## 2. Functional Requirements

### 2.1 Kernel (P0)

| ID | Requirement | Ver. |
|---|---|---|
| KER-01 | Every inbound request MUST produce a `RequestContext` (request_id, identity, tenant, plane, api_version, idempotency_key, trace fields) before any handler runs. | U/I |
| KER-02 | Handlers MUST NOT read auth/tenant state from process globals; only via `RequestContext` + typed extractors. Enforced by review checklist + compile-time extractor design. | U |
| KER-03 | Any state-changing operation MUST execute inside a `Tx` wrapper that sets tenant GUC, runs the mutation, writes delta(s), and emits outbox rows atomically. | I |
| KER-04 | Workflow kernel: every aggregate with a lifecycle MUST declare a state machine (states, allowed transitions, side-effect hooks); illegal transitions rejected with `409 workflow.invalid_transition`. | U/P |
| KER-05 | Command handlers MUST accept an optional `Idempotency-Key`; replays within 24h return the original result without re-execution. | I |
| KER-06 | Optimistic concurrency: aggregates with contention risk (slots, stock, balances) MUST carry a `version` integer; stale writes rejected `409 version_conflict`. | I |
| KER-07 | Kernel exposes `Clock`, `IdGen`, `Config` as injectable traits; no module may call wall-clock/uuid/ENV directly in business logic. | U |
| KER-08 | Kernel hot path (authn → capability, snapshot cached) p50 < 2 ms, p99 < 8 ms. | L |

### 2.2 Identity & Sessions (P0)

| ID | Requirement | Ver. |
|---|---|---|
| AUTH-01 | Phone OTP sign-in: server-side issue + verify via SMS provider adapter (OtpProvider trait; MSG91 default, Twilio alternative); creates/binds identity; returns kernel session JWT. | I |
| AUTH-02 | Email/password sign-in with argon2id; constant-time verification; lockout after 10 failures/15 min per account+IP. | U/I |
| AUTH-03 | Session JWTs: ES256 or EdDSA, 15 min access + rotating refresh (30d), revocation via `sessions` table + broadcast invalidation. | I |
| AUTH-04 | Users can have multiple identities across tenants via memberships; one identity, many grants. | I |
| AUTH-05 | Platform-plane identities use a separate signing key and separate route namespace; tenant routes MUST reject platform tokens unless a live break-glass grant exists. | M |
| AUTH-06 | Password/OTP secret material never logged, never in deltas; PHI redaction filter in tracing layer. | U/A |
| AUTH-07 | OTP abuse control: per-number + per-IP rate limits, daily per-number cap, attempt cap with expiry, uniform anti-enumeration responses; OTP codes stored hashed, never logged or deltaed. | U/I |
| AUTH-08 | Support mode (D-21): read-only, PHI-redacted, watermarked platform session; TTL ≤ 2h with auto-expiry; writes structurally impossible; every read audited with support actor; tenant admin notified on start/end. | M/I |

### 2.3 Org Hierarchy — Scope Lattice (P1)

| ID | Requirement | Ver. |
|---|---|---|
| ORG-01 | Hierarchy node kinds MUST include: legal_entity, region, business_unit, location (clinic), department, warehouse, cost_center; each node belongs to exactly one tenant and one tree. | U |
| ORG-02 | Every node stores materialized `path` (`/<root>/<…>/<self>`) and depth; moves recompute subtree paths transactionally. | I/P |
| ORG-03 | Every tenant aggregate (patient, appointment, invoice, stock movement…) MUST be owned by exactly one hierarchy node (`owner_node`) at creation; ownership is the anchor for scoping. | M |
| ORG-04 | Grants bind subject (user|role) → action class → node path prefix → conditions (ttl, modules). Effective scope = union of granted prefixes, computed by capability engine from a cached snapshot. | U/M |
| ORG-05 | Delegation: a grant can be temporarily delegated downward (never upward) with reason + expiry. | I |
| ORG-06 | Org changes emit deltas; capability snapshot cache MUST invalidate within 2s of a grant/org mutation. | I |

### 2.4 Capability & Policy Engine (P0)

| ID | Requirement | Ver. |
|---|---|---|
| POL-01 | Deny-by-default: missing/failed capability check MUST return `403 capability.denied` and log decision with reason. | M |
| POL-02 | Capability decision inputs: identity, role grants, resource owner path, action, module flag, plane. Output: allow/deny + matched grant id (for audit). | U |
| POL-03 | No handler may query a tenant table without the policy layer having resolved the target scope (compile-time extractors + review gate). | A |
| POL-04 | Segregation-of-duties rules declarable per tenant (e.g., invoice creator ≠ approver); violations rejected with `403 sod.violation`. | I |

### 2.5 Smart Router & Module Registry (P0/P4)

| ID | Requirement | Ver. |
|---|---|---|
| RT-01 | Modules register route specs (pattern, action, capability, module, min api_version) at startup; router compiles a single match tree. Unknown/unregistered routes → `404`. | U |
| RT-02 | Request dispatch MUST resolve: api major version → module enabled for tenant → plan tier gate → canary cohort → feature flag → handler version. | I |
| RT-03 | Canary: tenant cohort assignment stable (hash), configurable per route family; rollback = flip cohort weight, no deploy. | I |
| RT-04 | API versioning: `/api/v1` stable; additive only; breaking changes require `/api/v2` with side-by-side handlers selected by router. | A |
| RT-05 | Module registry exposes discovery endpoint (`GET /platform/v1/modules`, tenant-scoped `GET /api/v1/meta/modules`) listing enabled modules + versions. | I |

### 2.6 Delta Engine (P0)

| ID | Requirement | Ver. |
|---|---|---|
| DLS-01 | Every state mutation MUST append ≥1 delta row in the same transaction: seq (per partition), tenant_id, scope_node path, aggregate, aggregate_id, action, changed fields, actor, capability grant id, causation id, occurred_at. | I/P |
| DLS-02 | Delta sequences MUST be monotonic per (tenant_id, partition); consumers MUST be able to resume from any cursor without full refetch. | P |
| DLS-03 | Consumer filters: tenant + scope path prefix + aggregate set + cursor. A consumer MUST NOT ever receive deltas outside its tenant or outside its granted scope prefixes (delta-level isolation). | M/P |
| DLS-04 | Live fan-out via Postgres LISTEN/NOTIFY to in-process hub; durable reads via cursor polling; at-least-once delivery, idempotent apply contract for consumers. | I |
| DLS-05 | Deltas immutable: no UPDATE/DELETE; corrections are new deltas (`compensating`). Retention policy per stream. | A |
| DLS-06 | `changed` payload MUST exclude never-synchronizable fields (secrets, tokens); PHI fields marked so channel filters can strip for non-clinical consumers. | U/M |
| DLS-07 | Audit reconstruction: any aggregate's full history reproducible by replaying deltas; endpoint `GET /api/v1/audit/replay?aggregate=&id=`. | I |

### 2.7 Platform Plane (P1)

| ID | Requirement | Ver. |
|---|---|---|
| PLT-01 | Tenant lifecycle: create → provision (seed org skeleton, admin user, default roles) → active → suspended → archived; suspension forces session revocation + API `403 tenant.suspended`. | I |
| PLT-02 | Plans map to module availability + limits (users, locations, bookings/day); over-limit writes rejected `429 plan.limit`. | I |
| PLT-03 | Break-glass: platform actor requests scoped tenant access (tenant, scope prefix, actions, ttl ≤ 4h, mandatory reason); requires optional second approver (configurable); auto-expire; every action under grant double-audited. | I/A |
| PLT-04 | Platform telemetry: per-tenant rps, error rate, delta lag, storage size via `/platform/v1/tenants/{id}/health`. | I |

### 2.8 Clinic Structure (P2)

| ID | Requirement | Ver. |
|---|---|---|
| CLN-01 | Cities, clinics (location nodes), departments, specializations, billable services CRUD with active flags; all tenant-scoped, all owner_node-anchored. | I |
| CLN-02 | Public (unauthenticated) directory read-only endpoints: search doctors by city/specialization/clinic with pagination + rate limit. | I/L |

### 2.9 Doctors & Slots (P2)

| ID | Requirement | Ver. |
|---|---|---|
| DOC-01 | Doctor = user with clinical profile (specialization, fee, experience, education, socials, signature); can hold memberships at multiple locations. | I |
| DOC-02 | Schedules: per weekday working hours + exceptions (leave); slot generation derives bookable slots (slot duration per doctor/service). | U/I |
| DOC-03 | Slot availability query MUST account for existing appointments, buffers, and concurrent holds; hold TTL 10 min releases automatically. | I/P |
| DOC-04 | Doctor reviews: verified-visit only; rating aggregates async. | I |

### 2.10 Patients & Family (P2)

| ID | Requirement | Ver. |
|---|---|---|
| PAT-01 | Patient records auto-MRN per tenant; demographics owner_node = registering clinic; consent flags recorded. | I |
| PAT-02 | Family profiles: one patient owner can manage N family members with scoped delegation to booking. | I |
| PAT-03 | Vitals (BP, sugar, SpO2, temp, weight) time-series with append-only writes; history endpoint supports graph ranges. | I |
| PAT-04 | Patient files: uploads to object storage, presigned URLs (5-min TTL), metadata in DB, access via capability check; PHI-marked. | I/M |

### 2.11 Appointments & Queue (P2)

| ID | Requirement | Ver. |
|---|---|---|
| APT-01 | State machine: SCHEDULED → CHECKED_IN → IN_CONSULTATION → COMPLETED; plus CANCEL_REQUESTED → CANCELLED/DECLINED, NO_SHOW; transitions emit deltas + notifications. | U/I |
| APT-02 | Booking MUST be idempotent per (idempotency-key) and reject double-booked slots atomically (slot row lock or hold). | I/P |
| APT-03 | Check-in (manual/QR) issues queue token numbered per doctor/day; queue reorders allowed by staff with audit. | I |
| APT-04 | Live calling: staff triggers call event → realtime broadcast to patient app + waiting-room screen channel. | I |
| APT-05 | Cancellation window policy per clinic (e.g., free until 2h before); refunds follow original payment method. | I |
| APT-06 | Recurring + group appointments behind feature flag (P4 enablement). | I |

### 2.12 Prescriptions (P2)

| ID | Requirement | Ver. |
|---|---|---|
| RX-01 | Prescription state machine: DRAFT → ISSUED → (DISPENSED | REVOKED); only ISSUED drives pharmacy issue. | U |
| RX-02 | Medicine master with generics, strength, form; Rx items reference master + free-text instructions. | I |
| RX-03 | PDF render of Rx with doctor signature (server-side template); link expires; access via capability. | I |

### 2.13 Billing, Payments, Ledger (P2)

| ID | Requirement | Ver. |
|---|---|---|
| BIL-01 | Invoice aggregates services + taxes; states DRAFT → FINAL → POSTED → (VOID_REVERSED); POSTED immutable; corrections only via reversal entries. | I/P |
| BIL-02 | Double-entry ledger: every money movement writes balanced entries (debit=credit enforced transactionally); period close freezes periods. | P |
| PAY-01 | Payment intents + gateway adapter interface (Razorpay first); webhook signatures verified; idempotent capture; refund path with maker-checker above threshold. | I |
| PAY-02 | Reconciliation job matches gateway statements to ledger daily; mismatches flagged to finance queue. | I/A |

### 2.14 Inventory & Procurement (P2)

| ID | Requirement | Ver. |
|---|---|---|
| INV-01 | Stock levels maintained ONLY via stock movements (receipt, issue, adjustment, transfer); quantity never updated directly. | P |
| INV-02 | Issue against ISSUED prescription atomically reserves stock; insufficient stock → workflow exception, not silent negative. | I |
| INV-03 | Procurement: request → approvals (configurable chain) → PO → goods receipt → stock; all steps workflow-kernel states with deltas. | I |
| INV-04 | Batch/expiry tracking with FEFO issuing policy. | I |

### 2.15 Notifications (P2)

| ID | Requirement | Ver. |
|---|---|---|
| NTF-01 | Template-driven notifications with variables; channels: push (FCM), SMS, email, in-app; provider adapters behind one trait. | I |
| NTF-02 | Sends recorded with status; delivery webhooks update status; failures retry with backoff, dead-letter after N. | I |
| NTF-03 | Per-tenant channel config + per-user consent honored; quiet hours supported. | I |

### 2.16 Reports & Sync (P4)

| ID | Requirement | Ver. |
|---|---|---|
| RPT-01 | Standard reports (appointments, revenue, doctor utilization, stock) served from read projections built off delta streams; no hot-path OLAP on primary tables. | I |
| SYNC-01 | Delta cursor APIs per aggregate family with `after`, `limit`, `streams[]`; response includes next cursor + has_more. | I/P |
| SYNC-02 | Webhooks: tenant-registered endpoints receive signed (HMAC) delta batches with retry/backoff + manual replay. | I |

### 2.17 Settings, Installer, Migration (P1/P4)

| ID | Requirement | Ver. |
|---|---|---|
| SET-01 | Tenant settings (branding, hours, policies) stored as versioned JSON per scope node; changes are deltas. | I |
| INS-01 | Domain-activation installer: first-run wizard creates platform bootstrap admin, activates license/domain, seeds demo tenant flag. | I/A |
| MIG-01 | Migration tooling MUST import tenants from the original Medicare (Laravel/MySQL) dataset with validation report + rollback plan. | A |

---

## 3. Non-Functional Requirements

### 3.1 Performance & Scale (V1 targets)
| ID | Requirement |
|---|---|
| NFR-P1 | Booking end-to-end p99 < 150 ms at 500 rps per node (4 vCPU/8GB), zero failed states. |
| NFR-P2 | Read endpoints p99 < 80 ms at 1k rps with cached snapshots; cache hit ratio ≥ 95%. |
| NFR-P3 | Realtime fan-out latency < 250 ms p95 for queue events at 5k concurrent sockets per node. |
| NFR-P4 | Delta cursor catch-up ≥ 50k deltas/s per consumer polling. |
| NFR-P5 | Single binary cold start < 3 s to healthy. |

### 3.2 Availability & Reliability
| ID | Requirement |
|---|---|
| NFR-R1 | 99.9% monthly for API; graceful degradation: DB down → 503 with retry-after; realtime down → REST polling fallback. |
| NFR-R2 | Zero data loss on crash: WAL + outbox; no background work without delta/outbox proof. |
| NFR-R3 | Backups: PITR (WAL-G), tested restore runbook quarterly. |

### 3.3 Security & Privacy
| ID | Requirement |
|---|---|
| NFR-S1 | TLS 1.2+ everywhere; HSTS; secrets via env/secret store, never in repo. |
| NFR-S2 | Tenant isolation enforced at DB (RLS) AND app (capability); isolation matrix suite runs in CI per merge. |
| NFR-S3 | PHI at rest encrypted (PG TDE or volume encryption); field-level encryption for national IDs (Aadhaar/SSN) before V1 GA. |
| NFR-S4 | DPDP Act 2023 (India) baseline: consent ledger, right-to-erasure workflow (soft-delete + crypto-shred), breach notification runbook. HIPAA-aligned controls as export feature. |
| NFR-S5 | Audit: all PHI reads logged (actor, capability grant, reason where required); audit stream append-only. |
| NFR-S6 | Dependency policy: < 40 direct non-dev crates; each new dep needs ADR entry + license check (MIT/Apache only). |

### 3.4 Operability & Observability
| ID | Requirement |
|---|---|
| NFR-O1 | Structured JSON logs (tracing), request_id correlation end-to-end; metrics (Prometheus) + `/healthz`, `/readyz`, `/metrics`. |
| NFR-O2 | Feature flags + tenant cohort changes MUST be reversible in < 1 min without deploy. |
| NFR-O3 | One-command dev environment: `docker compose up` (PG + API + seed). |

### 3.5 Maintainability
| ID | Requirement |
|---|---|
| NFR-M1 | New business module scaffolded from template + contract tests in < 1 dev-day; CI enforces module dependency rules (modules depend only on kernel-core contracts, never sibling modules). |
| NFR-M2 | All SQL compile-time checked (SQLx); no raw string SQL outside migrations. |
| NFR-M3 | API contract: OpenAPI 3.1 artifact published per release, generated from route registry. |

---

## 4. External Interfaces

- Clients: Next.js admin web, patient web (Next.js), patient mobile (Flutter), doctor mobile (Flutter). All consumed over REST + WebSocket; no client-specific backends in V1.
- Providers (adapters, swappable): SMS OTP + transactional SMS (MSG91 default, D-13), Razorpay + Stripe (payments, D-14), FCM (push), email provider (Q-07), object storage S3-compatible.
- Later: ABDM/FHIR gateways as separate modules — kernel must not bake assumptions (schema-agnostic stubs only).

---

## 5. Constraints & Assumptions

- PostgreSQL 16+, single primary in V1; read replicas allowed from M7.
- Single Rust binary per environment; horizontal scale via stateless API + Postgres NOTIFY (no broker in V1).
- Team size assumption: 2–4 backend devs (drives modular monolith over microservices).
- Full assumptions + open questions: `08-DECISIONS.md`.