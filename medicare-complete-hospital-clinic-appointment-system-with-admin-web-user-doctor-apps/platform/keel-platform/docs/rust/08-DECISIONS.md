# 08 — Decisions, Assumptions & Open Questions

> **Version:** 0.9 draft-for-lock · **Date:** 2026-08-19
> Statuses: **LOCKED** = changing requires a superseding ADR · **ASSUMED** = acting on it until answered · **OPEN** = needs stakeholder input before the noted milestone.
> Answers land here as ADR entries (D-NN), never in chat threads.

---

## 1. Locked Decisions (architecture)

| ID | Decision | Rationale (short) |
|---|---|---|
| D-01 | Rust modular monolith, single binary; microservice extraction only via outbox/delta seams | team size, hot-path perf, healthcare correctness; avoids distributed tax on day 1 |
| D-02 | Minimal dependency policy: < 40 direct non-dev crates; ADR required per new dep (MIT/Apache/BSD only) | audit surface, upgrade cost, PRD §2 |
| D-03 | PostgreSQL 16 with RLS as the tenant hard wall; SQLx compile-time-checked SQL; no ORM | last-blade isolation + correctness |
| D-04 | Delta engine as the single source of audit/sync/CDC/analytics; outbox for side-effects; no broker in V1 | one mechanism, many consumers; simpler ops |
| D-05 | Scope lattice with materialized paths + prefix grants; every aggregate has owner_node | uniform model from single clinic to hospital group |
| D-06 | Two security planes with break-glass as the only platform→tenant PHI door | least privilege by construction |
| D-07 | SmartRouter compiles module RouteSpecs; gating order version→module→plan→flag→canary→capability | plug-n-play enable/disable without deploys |
| D-08 | API: `/api/v1` stable additive; `/platform/v1` separate; errors via stable string codes | client stability across upgrades |
| D-09 | Realtime: in-proc hub + LISTEN/NOTIFY; push = nudge-only, never payload | PHI never through push providers |
| D-10 | Supersedes Laravel/MySQL track for the API; `docs/00..24` remain the domain reference | single source of truth per track |
| D-11 | **Both** deployment forms from GA: self-host standalone + shared SaaS; single binary/schema, `deploy_mode` config | stakeholder answer Q-01 (2026-08-19) |
| D-12 | Legacy Laravel/MySQL tenants are imported at V1 launch; migration on launch critical path with per-tenant validation report + rollback | stakeholder answer Q-03 (2026-08-19) |
| D-13 | Server-side OTP replaces Firebase Phone Auth; `OtpProvider` trait; MSG91 default adapter (India DLT), Twilio next | stakeholder answer Q-05 (2026-08-19) |
| D-14 | Payments: gateway-adapter trait; Razorpay at GA (India-first) + Stripe adapter for international SaaS tenants; further gateways by adapter | stakeholder answer Q-06 (2026-08-19) — see Q-14 to confirm interpretation |
| D-15 | **GA fixed at 10 weeks** from kickoff; weekly calendar + scope cuts in `07-EXECUTION-PLAN.md` §GA Calendar; requires 4 full-time backend devs | stakeholder answer Q-12 (2026-08-19) |
| D-16 | SMS provider = **MSG91** for OTP + transactional SMS; template texts drafted by us; DLT Principal Entity registration by the client's business entity (**confirmed Q-27b**); client legal-entity details outstanding | Q-15 answer (2026-08-19) |
| D-17 | Push = **FCM retained**; dropping Firebase Auth does not affect push infra | Q-16 answer (2026-08-19) |
| D-18 | Email = **SMTP relay** behind adapter (confirms `lettre` budget exception); DKIM/SPF per environment | Q-17 answer (2026-08-19) |
| D-19 | Object storage = **self-hosted MinIO bundled** in standalone compose; S3-compatible bucket in saas mode | Q-18 answer (2026-08-19) |
| D-20 | SaaS pricing = **usage-cap tiers** (quotas on users, bookings/day, SMS sends, storage); metering stream (research §7.4) feeds cap enforcement + billing | Q-19 + Q-27a answers (2026-08-19) |
| D-21 | **Support mode approved**: read-only, PHI-redacted, watermarked platform sessions; TTL ≤ 2h; tenant admin notified; writes structurally impossible | Q-20 answer (2026-08-19) |
| D-22 | Tenant export at GA = **JSON + files archive + PDF bundle** (invoices/Rx); erase = DPDP crypto-shred workflow | Q-21 answer, confirmed 2026-08-19 |
| D-23 | **External penetration test before GA**; vendor selection + booking owned by stakeholder; window W9–W10; internal structured review runs meanwhile | Q-22 answer (2026-08-19) |
| D-24 | **DPDP legal review starts W2** in parallel; stakeholder coordinates counsel; drafted consent/SMS-opt-in/privacy wording is our deliverable | Q-23 answer (2026-08-19) |
| D-25 | CI/CD = **GitHub Actions + GHCR** artifacts; no self-hosted runners initially | Q-24 answer (2026-08-19) |
| D-26 | **Observability stack deferred**; /metrics + JSON logs ship now; stack selection in fast-follow | Q-25 answer (2026-08-19) |
| D-27 | SaaS addressing = **subdomains at GA**; custom domains fast-follow | Q-26 answer (2026-08-19) |
| D-28 | PostgreSQL = **self-managed on the VPS**; WAL-G PITR + separate-region backup bucket; restore drill per NFR-R3 | Q-02 answer (2026-08-19) |
| D-29 | Payment gateway list **confirmed**: Razorpay at GA + Stripe adapter; further gateways by adapter | Q-14 answer (2026-08-19) |
| D-38 | Dependency exceptions resolved (Q-04): **moka approved** for snapshot caches; **utoipa declined** — OpenAPI emitted from RouteSpec registry via xtask (zero added deps) | chat answer 9 (2026-08-19) |
| D-39 | Realtime GA premise: **5k concurrent WS per node stands**, no over-engineering now; revisit with real traffic post-GA (Q-08 resolved) | chat answer 10 (2026-08-19) |
| D-40 | Staff PHI default = **masked (last-4)**; unmasking requires `*.phi.read` grant + audit; default role templates encode this (Q-09 resolved) | chat answer 11 (2026-08-19) |
| D-41 | GA reports fixed (Q-10): appointments by day/doctor · revenue by clinic · doctor utilization · stock + expiring batches · cancellations/no-shows | chat answer 12 (2026-08-19) |
| D-42 | **No external legacy-API parity obligation**; clean `/api/v1`; internal old→new endpoint mapping table ships with the migration tooling (Q-11 resolved) | chat answer 13 (2026-08-19) |
| D-43 | Project layout = **Laravel-familiar folder design**: `modules/<aggregate>/{http,services,models,repositories,events,workflows,policies}` + `kernel/`, `config/`, `database/`, `xtask`; normative spec in `10-STRUCTURE-AND-GUIDELINES.md` §2 | chat answer 14 (2026-08-19) |

### Provisional adoptions — recommendations adopted 2026-08-19 (veto-able)

Chat items 1–8 were unanswered; the recommended column is provisionally locked to keep W1 unblocked. Veto any row and it reopens.

| ID | Decision (provisional) | From |
|---|---|---|
| D-30 | Tiered DDD: rich domain only for Tier A (appointments, invoices/ledger, stock, prescriptions, grants); thin CRUD for masters | chat 1 |
| D-31 | Polyrepo: backend / admin-web / mobile apps / published SDK clients | chat 2 |
| D-32 | Admin web = Next.js server components + generated TS SDK + TanStack Query | chat 3 |
| D-33 | Flutter state = Riverpod + drift-backed sync store | chat 4 |
| D-34 | Clippy = default lints + `-D warnings` + curated lint set (float-money ban, unwrap-in-request-path ban, doc-header lint) | chat 5 |
| D-35 | Self-hosted logs at GA = JSON files + logrotate; Loki/Promtail optional compose profile | chat 6 |
| D-36 | Error docs = static site bundled with release artifacts | chat 7 |
| D-37 | Release rhythm = weekly train (Tuesdays) from W6 + hotfix flow | chat 8 |

---

## 2. Assumed Decisions (valid until contradicted)

| ID | Assumption | Used by | Contradiction cost |
|---|---|---|---|
| A-01 | ~~Firebase Phone Auth continues~~ **[SUPERSEDED by D-13 — server-side OTP, 2026-08-19]** | M1-04 | — |
| A-02 | ~~Razorpay only assumption~~ **[SUPERSEDED by D-14 — Razorpay + Stripe adapters, 2026-08-19]** | M6-03 | — |
| A-03 | Deployment target: single VPS Docker/compose first; managed PG acceptable later | M0/M8 | low |
| A-04 | Admin web stays Next.js; mobile stays Flutter; both consume `/api/v1` unchanged | contracts | low |
| A-05 | JWT ES256 (P-256) keys; if HSM/KMS required at GA, swap to Ed25519 via KMS signing | M1-02 | low |
| A-06 | Team of 2–4 backend devs for estimation (**GA calendar assumes 4**, D-15) | execution plan | reschedule only |
| A-07 | No GraphQL, no WASM plugins, no Kafka in V1 (per PRD out-of-scope) | design | n/a (explicit) |
| A-08 | India compliance baseline: DPDP Act 2023; HIPAA mapping as export feature, not gate | 05 §8 | medium if regulated markets demanded sooner |

---

## 3. Open Questions for Stakeholders (please answer; each maps to a gate)

| # | Question | Why it matters | Needed by |
|---|---|---|---|
| Q-01 | **Target ops shape:** single-VPS self-host per customer, or one shared multi-tenant deployment (SaaS), or both? Installer flow (INS-01) and plan/limits design change with the answer. | tenancy provisioning, pricing/limits, support model | M0 |
| Q-02 | **PostgreSQL hosting:** self-managed on the VPS vs managed (Supabase/RDS/Neon)? Affects PITR, extensions, WAL-G runbooks. | M0-05, M8-05 | M0 |
| Q-03 | **Existing tenants on the original Medicare (Laravel/MySQL) build:** must V1 import live customer data at launch, or only new tenants? If import — how many tenants, rough data volume, acceptable cutover window? | MIG-01 scope; could move from M7 to M3 if launch-critical | M2 |
| Q-04 | **Dependency budget exceptions:** OK to accept `moka` (in-mem cache), `ulid/uuid-v7`, `axum-extra`, `lettre` (email) as reviewed exceptions under the <40 budget? Any banned vendors? | cache design, email channel | M2 |
| Q-05 | **OTP identity provider:** keep Firebase Auth (current product), or move server-side OTP (e.g., MSG91/Twilio Verify) to cut client SDK coupling? | M1-04; app teams | M1 start |
| Q-06 | **Payment gateways at GA:** Razorpay only, or also Stripe (international tenants)? Refund/maker-checker threshold amount? | M6-03/05 | M6 start |
| Q-07 | **SMS/WhatsApp providers** for OTP fallback + notifications (MSG91? Twilio? Gupshup?) and budgets per message class. | NTF channels, cost | M7 start |
| Q-08 | **Realtime scale expectation:** max concurrent sockets at GA (per node and total)? Waiting-room TV screens deployment constraints (browser kiosk?) | hub sizing, NFR-P3 | M5 start |
| Q-09 | **PHI field policy for staff:** can reception see full patient phone/address by default, or masked (last-4) with `phi.read` escalation? Drives default role templates. | default grants, matrix fixtures | M2 |
| Q-10 | **Reports:** which 5 reports are GA-blocking, and are read-projection latencies (minutes) acceptable vs live query? | mod-reports scope | M7 start |
| Q-11 | **Legacy parity pressure:** any endpoints/behaviors from the original Medicare API that third parties (existing integrations) depend on and must be mimicked in `/api/v1`? | contracts freeze | M3 |
| Q-12 | **Timeline commitment:** is 21 weeks @2 devs / ~13 weeks @4 devs acceptable, or is there a fixed GA date we should plan slices around (M5 = usable product without billing)? | roadmap | M0 |
| Q-13 | **SMS provider pick (urgent):** confirm MSG91 as default for OTP + transactional SMS; DLT template registration takes days — must start W1. | D-13 implementation | W1 |
| Q-14 | Confirm D-14 interpretation: Razorpay at GA + Stripe adapter for international tenants (or name the exact gateway list)? | contracts freeze | W6 |


### Answers received (2026-08-19) → locked as D-11…D-15

| # | Stakeholder answer | Locked as |
|---|---|---|
| Q-01 | Both self-host per customer **and** shared SaaS | D-11 |
| Q-03 | Yes — import existing (Laravel/MySQL) tenants at launch | D-12 |
| Q-05 | Move to server-side OTP (drop Firebase Auth) | D-13 |
| Q-06 | Payment gateway confirmed — interpreted as Razorpay + Stripe via adapter trait | D-14 (Q-14 to confirm) |
| Q-12 | 10 weeks to GA | D-15 |

Still open: **none blocking** (D-30–D-37 provisional, veto-able). Resolved: Q-02 → D-28, Q-04 → D-38, Q-07/Q-13 → D-16, Q-08 → D-39, Q-09 → D-40, Q-10 → D-41, Q-11 → D-42, Q-14 → D-29, Q-27 → D-16/D-20/D-22, layout → D-43.

**Batch 2 (Q-15…Q-26)** — research-backed, structured in `09-RESEARCH-FINDINGS.md` §10: vendors (SMS/FCM/email/storage), SaaS billing model, support mode, export/erase, pen test, CI/CD, observability, tenant addressing.

### Batch 2 answers received (2026-08-19) → locked as D-16…D-27

| # | Stakeholder answer | Locked as |
|---|---|---|
| Q-15 | MSG91 | D-16 |
| Q-16 | Firebase (FCM) | D-17 |
| Q-17 | SMTP | D-18 |
| Q-18 | Self-hosted (MinIO bundled) | D-19 |
| Q-19 | SaaS pricing → usage caps (Q-27a) | D-20 |
| Q-20 | ok — support mode approved | D-21 |
| Q-21 | upgraded export confirmed: JSON + files + PDF bundle | D-22 |
| Q-22 | yes — external pen test | D-23 |
| Q-23 | yes — legal review from W2 | D-24 |
| Q-24 | CI/CD confirmed | D-25 |
| Q-25 | defer observability stack | D-26 |
| Q-26 | yes — subdomains at GA | D-27 |

**Confirmed follow-ups (2026-08-19):** Q-27a → usage-cap pricing (D-20 updated); Q-27b → client registers DLT Principal Entity (legal-entity details still to be provided for template texts); Q-21 export bundle confirmed; Q-14 gateways confirmed (D-29); Q-02 → D-28.

---

## 4. ADR Template (for answers landing here)

```
### D-NN <title> (status, date, deciders)
Context: …
Decision: …
Consequences: …
Supersedes: D-xx / A-xx (if any)
```

---

## 5. What We Are Explicitly NOT Deciding Yet

- V2 plugin runtime (WASM/sidecar) — kernel keeps module-crate boundaries so either path stays open.
- Search engine (Meilisearch/Typesense) — V1 uses indexed SQL; revisit if public directory QPS demands it.
- Multi-region/DR — schema and delta design are region-friendly; topology decision deferred (see `runbooks/multi-region-dr.md` for the long-term view).
- GraphQL gateway — deferred to V2 per `docs/14-MODULE-V2-STUBS.md`.
---

## 6. Kelson Foundation Decisions (D-44…D-53) — frozen 2026-08-19

Frozen by `11-FOUNDATION-MASTER-SPEC.md` (§16). The platform workspace lives at `platform/` (own Cargo workspace, codename **Kelson**).

| ID | Decision |
|---|---|
| D-44 | Platform codename **Kelson**; CLI `kelson`; crates `kelson-*`; folder `platform/` (veto-able) |
| D-45 | Repository topology frozen per 11-SPEC §8 (topology refinement wins over 02-SYSTEM-DESIGN §5) |
| D-46 | `medicare-kernel/` superseded by this foundation; kept compilable until reference-healthcare rebuild |
| D-47 | Postgres GUC namespace renamed `medicare.*` → `kelson.*` (RLS anchor `kelson.tenant_id`) |
| D-48 | No ORM. SQLx first-class; convenience is generated around SQL |
| D-49 | Three data-access modes (aggregate / projection / reporting) replace repository-everywhere |
| D-50 | Platform contains zero healthcare knowledge; Medicare becomes a vertical pack + reference app |
| D-51 | Idempotency is execution-level (engine port), not HTTP middleware |
| D-52 | Revisions, not TTLs, govern authorization/cache validity |
| D-53 | Events publish post-commit via outbox only |

Foundation status 2026-08-20: `cargo check --workspace` + `cargo test --workspace` green in `platform/` (38 crates; golden path example proves boot → policy → engine → delta → replay → deny).

## 7. Stage-0 framework blueprint (2026-08-20)

| ID | Decision |
|---|---|
| D-61 | Adopt Laravel-Class Successor blueprint as frozen constitution; restructure `platform/` to blueprint topology in one wave before any parity feature (specs/STAGE-0-FOUNDATION-SPEC.md) |
| D-62 | Dependency policy v2: hard cap 20 → justified-acceptance model with cap 25 (license/MSRV/audit/compile-time/unsafe/size table); first justified adds: sha2, lettre, reqwest |
| D-63 | Public framework name **Kelson**; crates keep `kelson-*` package names; directory names follow blueprint |
| D-64 | Consolidation: ids/errors/envelope stay in foundation/core; new crates only on real boundary + second consumer; blueprint dirs may start as modules |
| D-65 | Delta kernel remains in execution engine; enterprise/delta = delivery/projection layer only |
| D-66 | Sessions/CSRF become opt-in web capabilities in S3 (revises D-58 from excluded → opt-in); API-first token auth stays default |
| D-67 | AI/MCP: crate roots + tool-contract stubs only in S5; no AI dependencies until S5 |
| D-68 | Observability: logging+diagnostics now; metrics/profiling stub contracts only (D-26 deferral holds) |
| D-69 | Plugins: compile-time modules first; dynamic adapters later; WASM only on ecosystem demand |
| D-70 | Medicare isolation holds (D-50); medicare-kernel superseded (D-46) unchanged |
| D-71 | API governance: public items get API-IDs; cargo-semver-checks gates releases from S4 (CI tool, not Rust dep) |
| D-72 | Capability completion model = gate G17: capability done only when all 13 facets exist |
| D-77 | Dependency matrix extensions: web -> services (upload edge needs object storage) and runtime -> applications (hosts boot domain modules); analyzer matrix updated to match. (Recorded during S7 edge wiring as D-73; renumbered 2026-08-24 to free D-73…D-76 for Sentinel — see section 8.) |

## 8. Sentinel (S5) decisions (D-73…D-76) — frozen 2026-08-24

Normative spec: `platform/specs/STAGE-SENTINEL-SPEC.md`; long-form rationale in `platform/specs/ADR/0004…0006`. Roadmap renumbered by D-73: S4 developer platform → **S5 Sentinel** → S6 AI/MCP → S7 public ecosystem.

| ID | Decision |
|---|---|
| D-73 | Insert frozen stage **S5 "Sentinel"** (production observability runtime) between S4 and AI/MCP; AI/MCP → S6, public ecosystem → S7. Rescopes D-68 (metrics deferred stubs) into S5 and closes gap-analysis item B-17 (Prometheus-compatible `/metrics`). New CI gate G18 (telemetry resilience). |
| D-74 | Crate split under `crates/observability/` (telemetry, tracing, metrics, audit, pipeline, buffer, export); `kelson-observability` keeps its published name and hosts the runtime. `TelemetryContext` carrier lives in kelson-foundation (no `kelson-context` crate — the name is owned by foundation/context). Audit hash chain lives in new security crate `kelson-integrity` (sha2): the data layer cannot depend on observability, and chain computation stays inside the engine transaction. |
| D-75 | OTLP export = OTLP/HTTP + JSON encoding over reqwest (kelson-export). No `opentelemetry*` dependencies; protobuf encoding deferred. Dependency budget stays 23/25. |
| D-76 | Incident control plane: the file plane under `storage/framework/telemetry/control/` (`incident.json`) is the source of truth; the kelson-api admin endpoint (`POST /admin/telemetry/incident`, capability `telemetry.incident.manage`) writes the same file; every host polls every 2 s. Base config remains frozen at boot (STAGE-0 §10) — only `TelemetryControlState` mutates at runtime, solely via this plane. |