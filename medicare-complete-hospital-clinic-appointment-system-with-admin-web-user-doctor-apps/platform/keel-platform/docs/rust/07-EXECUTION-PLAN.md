# 07 — Execution Plan: Medicare Execution Kernel

> **Version:** 0.9 draft-for-lock · **Date:** 2026-08-19
> Estimates in dev-days (dd), sized for a team of 2–4 backend devs working in parallel where noted.
> Task ID format: `<Milestone>-<NN>`. Every milestone has an **exit gate** — no milestone closes until the gate is green in CI.

---

## GA Calendar — 10 Weeks (LOCKED 2026-08-19 · D-15)

The M0–M8 catalog below remains the task source; this calendar assigns those tasks to weeks and supersedes baseline sequencing. GA date is fixed.

**Capacity math:** effective scope ≈ 115 dd after the cuts below; 10 weeks × 4 full-time backend devs = 200 dd (≈1.7× absorption for migration rehearsals and hardening). With fewer than 4 devs the date moves — raise it immediately; gates are never silently trimmed.

| Week | Focus (catalog task IDs) | End-of-week gate |
|---|---|---|
| W1 | M0-01…07 · M1-01/02 · MSG91 sandbox + DLT registration kickoff (D-16) · CI/CD pipeline on GH Actions + GHCR (D-25) | Pipeline green; `/healthz`; sandbox OTP sent |
| W2 | M1-03…07 (server-side OTP per D-13) · M2-01/02 · DPDP consent wording draft → legal (D-24) | AUTH contract tests green incl. plane separation |
| W3 | M2-03…08 | **Isolation matrix v1: 0 leaks** |
| W4 | M3-01…09 | Router + delta engine contract tests; canary + delta-cursor demos |
| W5 | M4-01…05 | Scoped clinic/doctor/slot demo; slot race tests green |
| W6 | M4-06…09 · M5-01/02 | Vertical slice 1 demo; booking idempotency tests green |
| W7 | M5-03…09 · M7-02 pulled in (templates + FCM/SMS adapters) | Live calling demo end-to-end over WS |
| W8 | M6-01…06 · book external pen test vendor (D-23) | Balanced-ledger property tests; Razorpay **and** Stripe sandbox capture |
| W9 | M6-07…09 · M7-07/08 + `deploy_mode` config (both forms) · M7-09 import dry-run · tenant export endpoint JSON+files+PDF (D-22) | Money-path demo; installer first-run in both modes; migration validation report |
| W10 | M8-01/02/06/07 · M7-01/03/04 (approvals, outbox hardening, webhooks-out) · external pen test window (D-23) · GA checklist | **GA gate signed** |

**In GA scope (kept):** all P0/P1 · clinic structure, doctors, slots · patients + files · appointments/queue/live-calling · prescriptions · invoices + double-entry ledger · Razorpay + Stripe adapters (D-14) · stock + dispense-against-Rx · single-step procurement (request→approve→PO→receipt) · notifications core (SMS/email/FCM) · platform lifecycle + break-glass · installer with standalone+saas modes (D-11) · legacy tenant import (D-12) · isolation matrix + IDOR fuzzer · load test at NFR targets · national-ID field encryption.

**Fast-follow (post-GA, W11–14):** configurable multi-step approval chains · reconciliation automation beyond daily flagging · reports beyond the top 5 · WhatsApp channel · webhooks-out dashboard UX · recurring/group appointments (flagged) · custom domains for SaaS tenants (D-27) · observability stack selection (D-26).

**Launch critical path:** OTP provider credentials (W1) → isolation matrix (W3) → money path (W8) → **migration import rehearsals ×2 on production snapshot (W9)** → GA (W10).

---
## 0. Team Roles & Operating Rules

| Role | Accountability |
|---|---|
| Kernel lead | kernel-* crates, isolation matrix, architecture decisions |
| Module dev ×1–2 | mod-* crates against the module contract |
| Platform dev | platform crate, installer, break-glass, ops tooling |
| QA/shared | matrix suite, load harness, migration validation |

Rules:
- Branch-per-task, PRs reference task IDs; CI = fmt + clippy (deny warnings) + unit + integration + matrix (post-M2) + dep-budget report.
- No direct sqlx runtime queries without compile-time checked macros; no `unwrap` in request paths.
- Every task ships its tests; "done" = gate evidence, not review approval alone.

---

## M0 — Foundation (Week 1) · Gate: pipeline green + `/healthz` live

> **Status 2026-08-19:** scaffold landed in `medicare-kernel/` — `cargo check`/`cargo test` green (12 unit tests). M0-01/02/03/05 in place; M0-04 tracing-redaction, M0-06 metrics, M0-07 xtask template remain as Week-1 tasks.

| ID | Task | Crate | Est | Acceptance |
|---|---|---|---|---|
| M0-01 | Workspace scaffold (`apps/api`, `crates/kernel-core`, `xtask`), toolchain pinned, CI skeleton | repo | 1 | `cargo build` + CI run green |
| M0-02 | Docker Compose dev env: Postgres 16 + API + `just dev` / `cargo run` one-command | repo | 0.5 | Fresh clone → healthy API in < 10 min |
| M0-03 | Typed `Config` loader + boot validation + secret asserts | kernel-runtime | 1 | Boot fails fast with precise message on missing secret |
| M0-04 | RequestContext, error envelope, request_id layer, tracing JSON + redaction filter | kernel-runtime | 1.5 | request_id in every log line; secrets never in logs (unit) |
| M0-05 | kernel-db: pool, migration runner, `_migrations`, health checks | kernel-db | 1 | `/healthz`, `/readyz` correct under DB down |
| M0-06 | Metrics skeleton (request count/latency, build info) | kernel-runtime | 0.5 | `/metrics` scrape works |
| M0-07 | xtask: `scaffold-module` template wired to CI check | xtask | 1 | `cargo xtask scaffold-module demo` compiles + registers |

---

## M1 — Identity & Sessions (Weeks 2–3) · Gate: AuthN contract tests pass

| ID | Task | Crate | Est | Acceptance |
|---|---|---|---|---|
| M1-01 | identities/users/sessions schema + migrations | kernel-auth | 1 | Migration idempotent up/down verified |
| M1-02 | Session JWT sign/verify (ES256/EdDSA), kid rotation support | kernel-auth | 1.5 | Rotation test: old kid still verifies until expiry |
| M1-03 | Email/password argon2id flow + lockout policy | kernel-auth | 1.5 | AUTH-02 unit suite green |
| M1-04 | Firebase ID token verification via JWKS caching | kernel-auth | 1.5 | Verify + clock-skew + revoked tests |
| M1-05 | Refresh rotation + revoke + broadcast invalidation hook | kernel-auth | 1 | AUTH-03 integration green |
| M1-06 | Auth endpoints (§2 of contracts) + socket-token endpoint | apps/api | 1 | Contract tests vs `04-API-CONTRACTS.md` |
| M1-07 | Plane separation of token issuers | kernel-auth | 0.5 | Platform token rejected on tenant routes (matrix seed cell) |

---

## M2 — Org Lattice + Capability + RLS (Weeks 3–5) · Gate: isolation matrix 0 leaks

| ID | Task | Crate | Est | Acceptance |
|---|---|---|---|---|
| M2-01 | org_nodes schema, path maintenance, move recompute tx | mod-org | 2 | ORG-02 property test (random trees, moves) |
| M2-02 | roles/grants schema + CRUD endpoints | mod-org | 2 | Grant CRUD contract tests |
| M2-03 | Capability engine: snapshot build, prefix eval, cache + invalidation via internal delta subscription | kernel-capability | 3 | Snapshot staleness < 2s test (ORG-06) |
| M2-04 | kernel-db RLS: GUC guard, `DISCARD ALL` on release, leak probe | kernel-db | 1.5 | Leak probe test green in CI |
| M2-05 | Policy layer integration into pipeline (deny-by-default, 403 codes) | kernel-runtime | 1.5 | POL-01..03 tests |
| M2-06 | Isolation matrix v1: subjects×ops×resources harness + seed data | tests/ | 3 | Matrix runs in CI; 0 unexpected verdicts |
| M2-07 | Delegation (downward-only) + revocation | mod-org | 1.5 | Delegation tests incl. upward attempt denial |
| M2-08 | memberships + “select active tenant” UX endpoints | mod-org | 1 | Multi-membership flow test |

---

## M3 — Smart Router + Module Registry + Delta Engine (Weeks 5–7) · Gate: canary demo + delta cursor demo

| ID | Task | Crate | Est | Acceptance |
|---|---|---|---|---|
| M3-01 | RouteSpec model + match-tree compiler + gates order | kernel-runtime | 2.5 | RT-01/02 unit tests; unknown route 404 |
| M3-02 | Feature flags table + evaluator (plan tier, flag, cohort hash) | kernel-runtime | 2 | RT-03 canary pct test (deterministic cohorts) |
| M3-03 | Module trait registry + `/meta/modules` + composition root rules | apps/api | 1.5 | RT-05; disabling module flips routes live |
| M3-04 | Delta writer + Tx wrapper enforcement (no mutation without delta) | kernel-delta | 3 | KER-03/DLS-01 integration + violation panic test |
| M3-05 | Per-partition sequences + cursor store + bootstrap endpoint | kernel-delta | 2 | Monotonicity property test (concurrent writers) |
| M3-06 | `/sync/deltas` with server-side StreamFilter SQL builder | kernel-delta | 2 | Delta isolation property tests (L6) |
| M3-07 | LISTEN/NOTIFY bridge + hub skeleton (in-proc) | kernel-notify | 2 | Cross-process fanout e2e (two API procs) |
| M3-08 | Module dependency-rule CI check (modules can't import siblings) | xtask | 1 | Violation fails CI (fixture crate proves it) |
| M3-09 | Canary demo + delta cursor demo scripts | repo | 0.5 | Recorded demo for gate review |

---

## M4 — Vertical Slice 1: Clinic + Doctors + Patients (Weeks 7–9) · Gate: public booking data flow demo

| ID | Task | Crate | Est | Acceptance |
|---|---|---|---|---|
| M4-01 | mod-clinic tables + CRUD + owner anchoring | mod-clinic | 2 | CLN-01 tests; RLS on |
| M4-02 | Public directory endpoint + anon rate limits + response caching | mod-clinic | 1.5 | CLN-02 + load sniff (p99 target) |
| M4-03 | Doctor profiles, memberships, education/review fallbacks | mod-doctor | 2 | DOC-01 tests |
| M4-04 | Schedule + exceptions + slot generation engine (background job per date) | mod-doctor | 3 | DOC-02 unit suite incl. DST/timezone edges |
| M4-05 | Slot availability query + hold TTL + expiry sweeper | mod-doctor | 2 | DOC-03 concurrency test (parallel holds) |
| M4-06 | Patients + MRN generator + family members | mod-patient | 2 | PAT-01/02 tests |
| M4-07 | Vitals append-only + timeline aggregation | mod-patient | 1.5 | PAT-03 tests |
| M4-08 | Files: presigned up/download + capability gate + PHI marking | mod-patient | 2 | PAT-04 + matrix cells for file access |
| M4-09 | Slice demo: clinic→doctor→slot→patient record, all scoped | repo | 0.5 | Gate demo recorded |

---

## M5 — Vertical Slice 2: Appointments + Queue + Realtime + Rx (Weeks 9–12) · Gate: live calling demo end-to-end

| ID | Task | Crate | Est | Acceptance |
|---|---|---|---|---|
| M5-01 | Appointment aggregate + state machine (workflow kernel) | mod-appointment | 2.5 | APT-01 transition tests incl. all illegal paths |
| M5-02 | Booking flow: hold→confirm atomicity + idempotency | mod-appointment | 2 | APT-02 race test (parallel bookings, exactly one wins) |
| M5-03 | Check-in (manual + QR) + queue token numbering | mod-appointment | 1.5 | APT-03 incl. duplicate token race |
| M5-04 | Live calling events + WS channels `queue`/`calling`/`today` | kernel-notify + mod | 2.5 | APT-04 demo; fanout filter tests |
| M5-05 | Cancel requests + window policy + decision endpoints | mod-appointment | 1.5 | APT-05 tests |
| M5-06 | Socket gateway: auth handshake, subscribe validation, heartbeat, resub on invalidation | kernel-notify | 2.5 | 06 §2 lifecycle tests |
| M5-07 | Prescriptions + medicine master + issue/revoke transitions | mod-rx | 2.5 | RX-01/02 tests |
| M5-08 | Rx PDF render (template) + presigned access | mod-rx | 1.5 | RX-03 golden file test |
| M5-09 | Mobile sync bootstrap for slice 2 streams + doctor “today” contract | kernel-delta | 1.5 | PRD scenario 10 demo |

---

## M6 — Vertical Slice 3: Money Path (Weeks 12–15) · Gate: balanced-ledger property test + payment webhook demo

| ID | Task | Crate | Est | Acceptance |
|---|---|---|---|---|
| M6-01 | Invoices + items + finalize/post transitions | mod-billing | 2.5 | BIL-01 tests |
| M6-02 | Ledger + balanced-posting trigger suite + period close | mod-billing | 3 | BIL-02 property: zero unbalanced tx over random ops |
| M6-03 | Gateway adapter trait + Razorpay adapter (orders, verify) | mod-billing | 2.5 | PAY-01 sandbox e2e |
| M6-04 | Webhook receiver: signature verify, idempotent capture, replay-safe | mod-billing | 1.5 | Replay + forgery tests |
| M6-05 | Refunds + maker-checker workflow above threshold | mod-billing | 2 | SoD test: same actor create+approve denied |
| M6-06 | Reconciliation daily job + mismatch queue | mod-billing | 2 | PAY-02 with fixture statements |
| M6-07 | Stock model + movements-only invariant + FEFO issue vs Rx | mod-inventory | 3 | INV-01/02 property tests |
| M6-08 | Procurement workflow: request→approve→PO→receipt | mod-inventory | 2.5 | INV-03 e2e incl. rejection paths |
| M6-09 | Fiscal periods + limits enforcement (`plan.limit`) | kernel + billing | 1.5 | PLT-02 tests |

---

## M7 — Enterprise Controls + Notifications + Reports (Weeks 15–18) · Gate: production gates checklist

| ID | Task | Crate | Est | Acceptance |
|---|---|---|---|---|
| M7-01 | Generic approval workflow + delegation + SoD rule engine | kernel-workflow | 2.5 | POL-04 tests |
| M7-02 | Notification templates + channel adapters (FCM, SMS, email) + prefs/quiet hours | mod-notifycfg | 3 | NTF-01..03 tests |
| M7-03 | Outbox worker hardening: batching, retries, circuit breaker, dead-letter | kernel-notify | 2 | 06 §5 failure tests |
| M7-04 | Webhooks-out: signing, batching, replay endpoint, dashboard data API | kernel-notify | 2 | Signature interop test vectors |
| M7-05 | Report projections (appointments, revenue, utilization, stock) from delta streams | mod-reports | 3 | RPT-01 projection-vs-live drift check |
| M7-06 | Audit replay endpoint + access log (PHI reads) + drift detector job | kernel-delta | 2 | DLS-07 round-trip test |
| M7-07 | Platform plane: tenant lifecycle, limits, break-glass flow + expiry job | platform | 3 | PLT-01..03 tests incl. auto-expire |
| M7-08 | Installer/domain-activation wizard endpoints + bootstrap seed | platform | 2 | INS-01 scripted first run |
| M7-09 | Laravel→Rust migration tooling (CSV/SQL extract → validate → import) + dry-run report | xtask | 3 | MIG-01 on sanitized dataset |

---

## M8 — Hardening & GA (Weeks 18–21) · Gate: GA checklist signed

| ID | Task | Owner | Est | Acceptance |
|---|---|---|---|---|
| M8-01 | Load test: booking 500 rps p99, reads 1k rps, 5k sockets | QA | 3 | NFR-P1..P3 report attached |
| M8-02 | IDOR fuzzer + nightly schedule + alert wiring | QA | 2 | 0 findings over 5 consecutive runs |
| M8-03 | Field-level encryption (national IDs) + crypto-shred erasure workflow | kernel-auth | 2.5 | NFR-S3/S4 tests |
| M8-04 | Key rotation runbook + kid cutover test | platform | 1 | Zero-downtime rotation demo |
| M8-05 | Backup/PITR setup + restore drill runbook | platform | 1 | NFR-R3 drill executed |
| M8-06 | OpenAPI emit + contract diff vs `04-API-CONTRACTS.md` | xtask | 1 | NFR-M3 artifact published |
| M8-07 | Security review (external or structured internal) + fixes | all | 5 | Findings triaged; 0 P0/P1 open |
| M8-08 | Ops docs: deploy, runbooks, on-call, capacity notes | platform | 1.5 | `runbooks/rust-api.md` exists |

**Total ≈ 105 dd** (excl. buffer); with 30% buffer ≈ 135 dd → ~21 calendar weeks with 2 devs fully loaded, ~12–14 weeks with 4.

---

## Risk Register

| # | Risk | Impact | Mitigation |
|---|---|---|---|
| R1 | Scope-lattice design fights real clinic org shapes | rework M2 | M2-06 matrix includes 3 real tenant org fixtures; validate with stakeholders at M2 gate |
| R2 | Delta engine perf under hot streams | NFR miss | Partitioned sequences from day one (M3-05); load sniff each milestone |
| R3 | Slot race conditions under retries | double booking | Row-locked hold model + idempotency keys + property tests (M5-02) |
| R4 | Firebase vendor coupling | lock-in | kernel-auth isolates provider behind trait; swap cost contained |
| R5 | Migration from original Medicare data under-modeled | launch slip | MIG-01 scheduled M7 with dry-run report; treated as product gate |
| R6 | Minimal-dep budget pressure (needs for crypto, cache) | arch drift | ADR required per new dep (08-DECISIONS Q-04); budget report in CI |
| R7 | Small team, long critical path | schedule | Milestone gates demoable early; slices M4–M6 usable even if M7 slips |

---

## Definition of Done (per milestone)

1. All tasks merged with tests; CI green including matrix (post-M2).
2. Gate demo recorded or scripted evidence attached.
3. Docs updated (this set is versioned; changes land in same PR as behavior).
4. No P0/P1 known defects open against the milestone scope.