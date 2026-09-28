# 00 — PRD: Medicare ERP Execution Kernel (Rust)

> **Codename:** `medicare-kernel` · **Track:** Rust backend (supersedes the Laravel track for the API)
> **Version:** 0.9 draft-for-lock · **Date:** 2026-08-19
> **Status:** Draft for lock · **Companions:** `01-SRS.md`, `02-SYSTEM-DESIGN.md`, `03-DATA-MODEL.md`, `04-API-CONTRACTS.md`, `05-SECURITY-ISOLATION.md`, `06-REALTIME-AND-SYNC.md`, `07-EXECUTION-PLAN.md`, `08-DECISIONS.md`
> **Source direction:** Scope-Lattice + Ownership-Graph + Delta-Isolation + Two Security Planes

---

## 1. Product in One Paragraph

Medicare Kernel is a **multi-tenant healthcare ERP backend written in Rust** — not a CRUD API with `tenant_id` on every query, but an **ERP Execution Kernel**: identity → context → scope lattice → capability → ownership → policy → transaction → resource → delta. It serves the same four clients as the original Medicare spec (admin web, patient web, patient mobile app, doctor mobile app) while providing MNC-grade organization modeling, database-enforced isolation, plug-n-play modules, smart routing, and a delta engine that powers audit, offline sync, CDC, and analytics without a message broker.

---

## 2. Why Rust, Why Minimal Libraries

| Driver | Decision |
|---|---|
| Fast hot path with no middleware hell | Single modular-monolith binary; snapshot-based authorization; no per-request RPC fan-out |
| Memory safety + correctness for healthcare | Rust + compile-time-checked SQL (SQLx) + Postgres RLS as final barrier |
| Minimal dependency surface | tokio, axum, tower, tower-http, serde, serde_json, sqlx, uuid, time, tracing, thiserror (+ selective: jsonwebtoken, reqwest, argon2). No ORM, no DI container, no enterprise framework. **The architecture itself is the framework.** |
| Extraction later, not now | Outbox + module boundaries allow future service extraction without paying the distributed-systems tax on day one |

---

## 3. Product Pillars (P0 → P4)

### P0 — Execution Kernel (must exist before any business module)
RequestContext · Smart Routing · Scope Lattice · Capability Engine · Policy Evaluation · Transaction Wrapper · Delta Engine · Module Contract · Workflow Kernel (state machines)

### P1 — ERP Foundation
Tenant lifecycle · Organization objects (Legal Entity, Business Unit, Region, Location/Clinic, Department, Cost Center, Warehouse) · Users & Memberships · Roles & Grants · Feature Registry

### P2 — Healthcare/ERP Modules (first vertical slice)
Clinic Structure · Doctors & Slots · Patients & Family · Appointments & Queue · Prescriptions · Billing & Payments · Inventory (medicines) · Procurement (purchase orders) · Notifications

### P3 — Enterprise Controls
Workflow approvals · Maker-checker · Delegation · Segregation of duties · Fiscal periods · Idempotency · Concurrency control · Audit reconstruction from deltas

### P4 — Enterprise Platform
Per-tenant feature flags · Canary routing · Reports layer · Webhooks · Background jobs · Delta cursor sync APIs · Data export

---

## 4. Personas

| Plane | Persona | Needs |
|---|---|---|
| Platform | PLATFORM_SUPER_ADMIN | Tenant provisioning, module availability, global policy |
| Platform | PLATFORM_SECURITY | Investigation via audited, expiring tenant access grants |
| Platform | PLATFORM_SUPPORT | Tenant metadata only; never PHI by default |
| Platform | PLATFORM_SRE | Runtime diagnostics, health, delta lag |
| Platform | PLATFORM_RELEASE / PLATFORM_BILLING | Deployments, canary targets, tenant plans |
| Tenant | Tenant Admin | Org structure, users, roles, all tenant modules |
| Tenant | Reception / Clinic Staff | Bookings, check-in, queue tokens, live calling |
| Tenant | Doctor | Today's schedule, consultations, prescriptions, follow-ups |
| Tenant | Patient | Booking, payments, prescriptions, vitals, family profiles |
| Tenant | Finance Manager / Accountant | Invoices, payments, ledger, refunds (maker-checker) |
| Tenant | Pharmacy / Warehouse Manager | Stock levels, receipts, issue against prescription |
| Tenant | CFO | Cross-location financial reports, approval chains |

---

## 5. Core Scenarios (what the kernel must make easy)

1. **Provision a tenant** — platform operator creates tenant, seeds org skeleton, enables modules (platform delta emitted).
2. **Model a multi-clinic group** — tenant admin creates Legal Entity + 5 Locations (clinics) + Departments + Pharmacy Warehouse; a doctor holds memberships at 2 clinics; a finance manager operates across one Legal Entity.
3. **Book an appointment** — patient selects clinic → doctor → slot; appointment owned by Location; slot engine respects per-day schedules.
4. **Check-in → queue → live call** — staff check-in (or QR); queue token issued; realtime call event broadcast; every step is a state transition with delta.
5. **Consultation → prescription → pharmacy issue** — doctor writes Rx (state machine DRAFT→ISSUED); pharmacy issues stock (inventory delta linked to prescription).
6. **Bill → pay → post** — invoice aggregates services; payment via gateway webhook; posting writes balanced ledger entries; posted invoice immutable; corrections via reversal deltas.
7. **Procure stock** — purchase request → approval chain → PO → goods receipt → stock update (workflow kernel drives every transition).
8. **Cancel with approval** — patient requests cancellation; admin approves; refund window policy evaluated; refund issued via original gateway.
9. **Platform break-glass** — PLATFORM_SECURITY requests 30-minute scoped access to tenant T42 with reason; elevation granted, fully audited, auto-expired.
10. **Offline mobile sync** — doctor app reconnects and calls `GET /api/v1/appointments/deltas?after=10003`; receives ordered deltas; no full-state refetch.

---

## 6. Scope

### In scope (V1)
- Full P0 kernel; P1 foundation; P2 modules listed above; P3 maker-checker/approvals/idempotency/concurrency; P4 feature flags + delta cursor APIs + webhooks.
- Auth: phone OTP (Firebase as identity provider, verified via JWKS) + email/password (argon2) + kernel-issued session JWTs.
- Realtime: WebSocket broadcast (queue/calling) via in-process hub + Postgres LISTEN/NOTIFY fan-out (no broker).
- Deployment: single Rust binary + PostgreSQL 16 + object storage for files; Docker Compose for dev. **Both forms from GA (D-11):** per-customer self-host (deploy_mode=standalone) + shared multi-tenant SaaS (deploy_mode=saas).

### Out of scope (later lanes, schema-agnostic stubs only)
- Video telemedicine, AI scribe/recommendation, labs/pathology, FHIR/ABDM bridges, runtime WASM plugins, GraphQL, multi-region DR, Kafka/NATS.

---

## 7. Success Metrics

| Metric | Target |
|---|---|
| Kernel hot-path overhead (authn→policy, cached snapshot) | p50 < 2 ms, p99 < 8 ms |
| End-to-end booking API | p99 < 150 ms at 500 rps/node |
| Cross-tenant reads/writes in isolation matrix tests | **0** |
| Privilege escalation / IDOR findings | **0** at GA |
| New module scaffold time | < 1 dev-day via template + contract tests |
| Test coverage (kernel crates) | ≥ 85% |
| Delta sync correctness (ordered, per-partition monotonic) | 100% in property tests |
| Dependency count (workspace, non-dev) | < 40 direct crates |

---

## 8. Release Summary

| Milestone | Content | Gate |
|---|---|---|
| M0 | Repo, CI, workspace skeleton, Postgres, health | Pipeline green |
| M1 | Kernel core + identity + sessions | AuthN contract tests pass |
| M2 | Org lattice + capability engine + RLS | Isolation matrix: 0 leaks |
| M3 | Smart router + module registry + delta engine | Canary demo + delta cursor demo |
| M4 | Clinic structure + doctors/slots + patients | Vertical slice demo 1 |
| M5 | Appointments + queue + realtime + prescriptions | Vertical slice demo 2 |
| M6 | Billing + payments + inventory + procurement | Vertical slice demo 3 (money path) |
| M7 | Enterprise controls + notifications + reports | Production gates checklist |
| M8 | Hardening, load test, security review | GA |

**Calendar locked 2026-08-19: GA = 10 weeks from kickoff (D-15)** — weekly schedule, scope cuts, and staffing math in  7-EXECUTION-PLAN.md §GA Calendar.

Details, tasks, and exit criteria: `07-EXECUTION-PLAN.md`. Locked decisions and assumptions: `08-DECISIONS.md`.