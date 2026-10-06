# 09 — Research Findings: What We Need to Build It (and How to Surpass the Laravel Track)

> **Version:** 0.95 · **Date:** 2026-08-19 · **Method:** audit of `docs/00..24` domain reference + Rust-track docs `00..08` + live crates.io survey (2026-08-19) + pattern comparison against reference systems (Stripe, Shopify/Linear, GitLab, Odoo/ERPNext, Supabase).
> Companion: answers here land in `08-DECISIONS.md`; schedule impact lands in `07-EXECUTION-PLAN.md`.

---

## 1. Top Findings (TL;DR)

1. **The architecture is sound; the gaps are around it.** Kernel design covers ~90% of structural risk. The remaining 10% is: support-access model, SaaS metering/billing, client SDK pipeline, localization, per-tenant noisy-neighbor controls, data portability. All fixable inside the 10-week plan except localization scope.
2. **Surpassing Laravel is mostly free** once the kernel exists: compile-time SQL, snapshot authorization, delta streams, and RLS replace an entire category of Laravel middleware/policy/event plumbing with fewer moving parts. The one real loss — CRUD dev speed — is recovered with the module scaffold (`xtask scaffold-module`) + meta-driven admin screens.
3. **Dependency surface is small and healthy.** 20 surveyed crates, all actively maintained, all MIT/Apache/BSD-compatible. Budget of <40 direct crates stays realistic (current design ~24).
4. **Server-side OTP (D-13) creates a W1 vendor dependency** (SMS provider + DLT registration in India takes days). This is the single most time-sensitive external item.
5. **Both deploy forms (D-11) require a control-plane/data-plane split** — designed in §7 below: same binary, separate DB schema + separate DB roles + support mode + metering.

---

## 2. Build-Needs Inventory ("what we need to make it")

Status: 🟢 have · 🟡 partial/spec only · 🔴 missing. "Needed by" refs the 10-week calendar (07 §GA Calendar).

### 2.1 People & process

| Need | Detail | Owner | By | Status |
|---|---|---|---|---|
| 4 full-time Rust backend devs | capacity math in 07; fewer ⇒ date moves | Founder | W1 | 🔴 confirm hiring/assignment |
| 1 QA (part-time) | matrix suite, load harness, migration validation | Founder | W3 | 🔴 |
| Frontend integration owner | admin web + 2 Flutter apps will consume contract changes | Founder | W4 | 🔴 named person |
| On-call rotation + escalation policy | min 2 people from W8 | Founder | W8 | 🔴 |
| ADR + contract-change review habit | all decisions land in `08-DECISIONS.md` | Kernel lead | W1 | 🟢 docs exist |

### 2.2 Vendor accounts & credentials

| Need | Detail | By | Status |
|---|---|---|---|
| SMS provider | **D-16: MSG91 decided**; account + API keys outstanding; DLT Principal Entity registration by client entity (confirmed, Q-27b); legal-entity details outstanding | W1 | 🟡 decided |
| Razorpay | sandbox keys W1, live keys W8, webhook endpoints configured | W1 | 🔴 |
| Stripe (D-14) | account + restricted API key, webhook signing secret | W6 | 🔴 |
| FCM service account | **D-17: FCM retained**; service-account key outstanding | W7 | 🟡 decided |
| Email provider | **D-18: SMTP relay decided** (`lettre` adapter); sending domain + DKIM/SPF outstanding | W7 | 🟡 decided |
| Object storage | **D-19: self-hosted MinIO decided** (bundled in standalone compose); saas bucket credentials outstanding | W2 | 🟢 decided |
| Backup bucket | WAL-G archive destination, region-separated | W8 | 🔴 |
| Domains + DNS | SaaS root domain (subdomain strategy, Q-26), per-customer domains for standalone | W9 | 🔴 |

### 2.3 Environments & infrastructure

| Need | Detail | By | Status |
|---|---|---|---|
| Dev environment | Docker Compose (PG 16 + API + MinIO) one-command | W1 | 🟢 specified (M0-02) |
| CI/CD | **D-25: GitHub Actions + GHCR decided**; fmt/clippy/unit/integration/matrix/dep-budget; artifacts (binary + OpenAPI) | W1 | 🟢 decided |
| Staging | 1 VPS mirroring prod (both deploy modes exercised) + sandbox gateway endpoints | W5 | 🔴 |
| Prod topology | standalone: per-customer VPS (minimal 4 vCPU/8GB); saas: 2 API nodes + managed PG + LB | W9 | 🔴 |
| Observability | Q-25: Prometheus + Grafana assumed; log shipping; uptime monitor | W8 | 🔴 |
| PostgreSQL hosting | **D-28: self-managed on VPS decided**; WAL-G + backup bucket provisioning (M0-05/M8-05) | W1 | 🟢 decided |

### 2.4 Data & migration

| Need | Detail | By | Status |
|---|---|---|---|
| Sanitized production MySQL dump | from existing Laravel tenants; required for M7-09 + W9 rehearsals ×2 | W6 | 🔴 request from ops |
| Tenant list + cutover windows | which tenants migrate at launch, order, rollback contacts | W8 | 🔴 |
| Id-mapping report plan | old MySQL ids ↔ new uuids (appointments, patients, invoices) for any external references | W7 | 🟡 design in M7-09 |
| Seed/demo dataset | realistic demo tenant for sales + installer demo | W9 | 🟡 |
| DLT template text copies | OTP + booking confirmation + reminder texts for SMS approval | W1 | 🔴 |

### 2.5 Compliance & client artifacts

| Need | Detail | By | Status |
|---|---|---|---|
| DPDP consent language review | **D-24: legal review starts W2**; stakeholder coordinates counsel; drafted wording is our deliverable | W7 | 🟡 decided |
| Privacy policy + erasure workflow wording | for GA | W9 | 🔴 |
| Pen test | **D-23: external pen test decided**; vendor selection + booking (stakeholder) outstanding | W6 | 🟡 decided |
| OpenAPI artifact + SDK generation pipeline | TypeScript + Dart clients generated per release (see §5) | W6 | 🟡 specified, needs pipeline |
| Mobile offline-sync library contract | doctor app team adopts §6 of doc 06 | W7 | 🟡 |

---

## 3. Surpassing Laravel — Structure Mapping

We keep `docs/00..24` as the **domain** reference but do **not** port Laravel's structure. Concept-by-concept upgrade:

| Concern | Laravel track | Rust kernel | What we surpass |
|---|---|---|---|
| Composition | Service providers auto-discovered | `Module` trait + composition root; **CI blocks cross-module imports** | Module bounds are compile-time, not convention |
| Data access | Eloquent ORM (runtime SQL, N+1 risk, mass assignment) | SQLx compile-time checked SQL, repositories per module | Whole bug classes eliminated (SQL typo/injection, wrong column) |
| AuthZ | Spatie + controller policies (developer must remember `authorize()`) | Capability snapshot in pipeline, deny-by-default (POL-01) | Cannot forget the check; it is the pipeline |
| Multi-tenancy | `tenant_id` column + middleware trust | RLS (DB) + lattice (app) + delta filters (streams) — 3 enforced layers | Defense in depth; matrix-proven |
| Events | Laravel events/queues (lossy on crash) | Deltas + outbox in the same transaction | No event loss; audit/sync/CDC are one mechanism |
| Realtime | Reverb (separate server, channel auth ad-hoc) | In-proc hub + LISTEN/NOTIFY; filter re-derived from grants | Authorization and fanout share one source of truth |
| Migrations | Per-package, runtime ordering | Ordered stream, boot-time runner, two-release contract rule | Deterministic, reversible upgrades |
| Job queues | Horizon + Redis | Outbox worker (no extra infra) | One fewer stateful system to run |
| Plugins | Planned Composer-based (V2) | Plug-n-play modules **at GA** (§6) | Shipped day 1, not roadmap |
| API docs | Swagger by hand (drifts) | RouteSpec registry → generated OpenAPI + CI contract diff | Docs cannot drift |
| Performance | PHP-FPM per-request boot | Persistent async runtime; authZ p50 < 2ms (KER-08) | Targets PHP cannot reach |
| Type safety | Dynamic | Rust compiler + serde contracts | Regressions caught at build |

Honest tradeoffs we compensate for:
- **CRUD speed:** `cargo xtask scaffold-module` generates module crate + migrations + contract tests + RouteSpecs in one command (M0-07).
- **Admin screen velocity:** `/api/v1/meta/modules` exposes panels/flags so the Next.js admin renders available features per tenant instead of hardcoded menus (config-driven UI).
- **Ecosystem breadth:** where Laravel had a package, we prefer a small adapter behind a trait (SMS, mail, storage) over a framework dependency.

---

## 4. MNC-Grade Patterns Adopted (reference benchmarks)

| Pattern | Reference system | Our adaptation | Doc |
|---|---|---|---|
| Idempotency keys + stable error codes | Stripe API | `Idempotency-Key`, 24h replay, namespaced codes | 04 §1 |
| Signed webhooks + retry ladder + replay | Standard Webhooks (Stripe/Svix) | HMAC `t=…,v1=…`, 1m/5m/30m/2h/12h, replay endpoint | 06 §5 |
| Client sync engine (bootstrap + cursors) | Linear / CouchDB changes feed | `/sync/bootstrap` + `/sync/deltas` per stream, version-apply | 04 §12, 06 §8 |
| Break-glass access | GitLab / cloud providers | `platform_grants`: scope, ttl≤4h, reason, auto-expire | 05 §2.3 |
| Module manifests + dependency rules | Odoo / ERPNext / VSCode extensions | Module trait + manifest + kernel semver negotiation | §6 below |
| Movement-only inventory + FEFO | ERPNext stock module | Stock quantities change only via movements | 03 §3.7 |
| Double-entry ledger, immutable post | Ledgers in finance systems | Balanced-per-tx enforcement; corrections via reversals | 03 §3.6 |
| Canary cohorts | Netflix / Shopify | Stable hash cohorts, config-flip rollback | 02 §6 |
| Row-level multi-tenancy with GUC | Supabase/Neon guidance | `SET LOCAL medicare.tenant_id` + DISCARD on release | 03 §2 |
| Errors as typed documents | RFC 9457 (problem+json) | Stable code + details + request_id; machine-readable | 04 §1 |
| Rate limit response headers | IETF RateLimit draft | `RateLimit-Limit/Remaining/Reset` | 04 §1 |
| Per-tenant noisy-neighbor control | SaaS platform standard | Per-tenant rate buckets + quotas + circuit (control-plane, §7) | §7 below |

---

## 5. API Design Standard (hardens `04-API-CONTRACTS.md`)

Decisions added by this research pass:

1. **Profile header:** responses set `Content-Type: application/json`; clients may request `Accept: application/vnd.medicare.v1+json` for version pinning (optional; path version remains primary).
2. **Action endpoints** use `POST /{resource}/{id}/transition` with `{event}` — deliberate deviation from pure REST because it maps 1:1 to the workflow kernel, makes state machines auditable, and lets the server return `allowed_events` for dynamic UIs.
3. **Filtering grammar (fixed):** `?status=a,b&date=YYYY-MM-DD&from=&to=&node=<org_node_id>&q=`; **sorting:** `?sort=field:asc|desc` with a safe-list per route; free-form SQL-ish filtering is not allowed.
4. **Cursors are opaque** (base64 of `{stream,seq}` or keyset) — clients must not parse them; this lets us change internal pagination.
5. **Error contract:** every error code gets a documentation URL `https://docs.medicare.dev/errors/{code}` shipped in `details.doc_url`; clients can rely on the code, humans on the URL.
6. **Backward-compat machine check:** xtask diffs OpenAPI artifacts per PR; breaking removals/renames fail CI on `/api/v1` (additive-only enforcement per RT-04).
7. **SDK generation (new pipeline):** OpenAPI → `openapi-generator` → TypeScript client (admin/patient web) + Dart client (mobile), published per release; client repos run contract smoke tests against staging. This is how "plug n play" reaches the frontend too.
8. **Rate limiting:** token bucket per session + stricter anon budgets; burst headers per draft above; `429` includes `Retry-After`.
9. **Long-running ops:** none at GA (everything < 250ms except presigned uploads). If one appears later, pattern is fixed: `POST` returns `202 {operation_id}` + `GET /operations/{id}`.

---

## 6. Plug-n-Play Module System v2 (reusable by design)

Beyond the `Module` trait (02 §3), this pass fixes the reusability contract:

**Manifest (compiled-in, declared per module):**

```ron
ModuleManifest(
  id: "appointment", version: "1.0.0",
  kernel: ">=1.0, <2.0",                    // semver negotiation at boot
  streams: ["appointments", "queue"],
  capabilities: ["appointment.read", "appointment.write", "appointment.transition", "queue.call"],
  events: ["appointment.transitioned"],
  settings_schema: "settings.schema.json",   // tenant-editable settings, validated, versioned
  ui_hints: Panels(["queue-board", "calendar", "cancel-queue"]), // consumed by admin web
  depends_on_streams: [],                    // read-side deps only (no crate deps!)
)
```

**Rules (CI-enforced):**
- Modules depend only on `kernel-core`/`kernel-workflow` traits — never sibling crates (already D-07; now with manifest declaration of stream-level reads).
- Cross-module reads happen via **published read ports** (kernel-defined traits) or delta projections — never via another module's tables (FK ban in 03 is the DB-level version of this).
- Semantics of enable/disable/upgrade are config writes (feature registry), not deploys; disabling a module must never corrupt other modules' streams.
- Reuse tiers: (a) **kernel reuse** — every module gets authZ, tenancy, deltas, workflows, idempotency for free; (b) **client reuse** — meta/modules + generated SDKs mean web/mobile add a module's UI in hours; (c) **deployment reuse** — same binary powers standalone and saas; module availability differs per tenant plan.
- Deprecation policy: a route can be marked `deprecated` in its RouteSpec; router emits a `Sunset` header; removal only on next major.

---

## 7. Tenant ↔ Platform Separation v2 (control plane / data plane)

Strengthening beyond `05-SECURITY-ISOLATION.md` (new items marked ◆; answers needed where flagged):

1. **Planes & tokens** — separate signing keys, namespaces, pool roles (existing, 05 §2).
2. ◆ **Schema split:** platform tables (`tenants`, `tenant_limits`, `platform_grants`, metering) move to schema `platform`; tenant data stays in `app` schema under RLS. DB roles: `medicare_app` (tenant handlers; **no access to platform schema**) and `medicare_platform` (provisioning/break-glass only). Compile-time: two executor types, no conversions.
3. ◆ **Support mode (read-only impersonation)** — distinct from break-glass: PLATFORM_SUPPORT may enter a tenant with **read-only, PHI-redacted, watermarked** session (actor = support actor + tenant id stamped on every read audit row), max 2h, tenant admin notified. Break-glass remains the only write door. (Q-20 approval)
4. ◆ **Metering stream** — platform delta partition `metering`: bookings created, users active, storage bytes, SMS sent; feeds plan-limit enforcement + SaaS billing (Q-19 model choice).
5. ◆ **Noisy-neighbor controls** — per-tenant rate buckets + daily quota goals + per-tenant circuit breaker in the router; platform dashboards show top-N consumers; suspension enforcement via PLT-01.
6. ◆ **Tenant data portability** — `POST /platform/v1/tenants/{id}/export` → JSON (all aggregates, delta-corrected state) + files archive; `POST .../erase` executes DPDP crypto-shred + soft-delete workflow (Q-21 scope).
7. **Provisioning pipeline** — create tenant → schema seed (org skeleton + default roles/grants) → feature plan applied → welcome credentials → platform delta emitted (existing PLT-01; now enumerated).
8. **Per-tenant observability** — logs/metrics carry tenant_id; platform SRE views per-tenant error/delta-lag (PLT-04); tenant admins get a scoped health page in W10 fast-follow.
9. **Blast-radius rule** — config/cache invalidations are per-tenant keyed; there is no global cache flush path reachable from tenant-plane code.
10. **Standalone collapse** — in `deploy_mode=standalone`, the platform surface shrinks to local ops (no SaaS metering/billing); same isolation machinery runs unchanged (D-11).

---

## 8. Dependency Research — Surveyed 2026-08-19 (crates.io)

| Crate | Latest stable | Role | Budget | Notes |
|---|---|---|---|---|
| tokio | 1.53.1 | async runtime | core | features: rt-multi-thread, macros, signal |
| axum | 0.8.9 | HTTP framework | core | + axum-extra only if websocket helpers needed (Q-04) |
| tower | 0.5.3 | middleware | core | |
| tower-http | 0.7.0 | cors/compression/trace | core | |
| serde / serde_json | 1.0.229 / 1.0.151 | contracts | core | |
| sqlx | 0.9.0 | compile-time SQL | core | postgres + runtime-tokio; verify macro in CI |
| thiserror | 2.0.20 | error types | core | |
| tracing / tracing-subscriber | 0.1.44 / 0.3.23 | observability | core | JSON layer + redaction filter |
| uuid | 1.24.1 | ids | core | v7 (time-ordered) for deltas-friendly PKs |
| time | 0.3.55 | date/time | core | tz-aware slot math |
| jsonwebtoken | 11.0.0 | session JWT | selective | ES256; kid rotation |
| argon2 | 0.5.3 | password + OTP hashing | selective | |
| reqwest | 0.13.4 | provider HTTP clients | selective | rustls-tls only |
| tokio-tungstenite | 0.30.0 | via axum WS | (indirect) | tracked for API compat |
| moka | 0.12.16 | capability snapshot cache | **Q-04 exception** | else hand-rolled TTL map |
| lettre | 0.11.23 | email adapter | **Q-04 exception** | else SMTP-raw adapter |
| utoipa | 5.5.0 | OpenAPI emission | **Q-04 exception** | else xtask-emitted YAML from RouteSpec |
| rand | 0.10.2 | OTP/canary jitter | selective | |

Direct non-dev count today: **~24 without exceptions** → 27 with all three exceptions approved. Budget <40 holds with headroom. Toolchain (dev-only, not counted): `sqlx-cli`, `cargo-deny`, `cargo-audit`, `just`, `cargo-generate` (module template), Docker/pg_ctl for tests (testcontainers-rust if accepted as dev-dep).

---

## 9. Gap Register (what the current docs miss) → fixes

| # | Gap | Recommended fix | Lands in | When |
|---|---|---|---|---|
| G-01 | Support-mode (read-only impersonation) undefined | §7 item 3; add sessions.plane value `support`, redaction profile | 05, 01 (AUTH-08) | W3 |
| G-02 | SaaS metering/billing undefined | §7 item 4; metering partition + plan-limit hooks exist already | 03, 04 | W9 |
| G-03 | Client SDK pipeline undefined | §5 item 7; CI job post-release | 04, 07 (M8-09 new) | W6 |
| G-04 | Localization strategy absent | i18n via `Accept-Language` + per-tenant locale; app-side strings client-owned; SMS/notification templates localized in `notification_templates.locale` (already in schema) | 04 | W7 (notification templates) |
| G-05 | Noisy-neighbor controls missing | §7 item 5; router bucket middleware | 02, 01 (NFR) | W4 |
| G-06 | Tenant export/erase undefined | §7 item 6 | 04 (§15), 05 §8 | W9 |
| G-07 | Error contract vs RFC 9457 not explicit | §5 item 5; keep envelope, add `doc_url` | 04 §1 | W2 (with error layer) |
| G-08 | OpenAPI emission tool undecided (utoipa vs xtask YAML) | Q-04 decision; default: xtask-emitted from RouteSpec (zero deps) | 04, 08 | W1 |
| G-09 | ~~PG hosting undecided~~ **resolved — D-28 self-managed**; WAL-G runbook = M8-05 | — | ✓ |
| G-10 | Load-test harness unspecified | k6 scripts + scenarios (booking mix, socket load) | 07 M8-01 detail | W8 |
| G-11 | Push provider confirmation (FCM without Firebase Auth) | Q-16 | 08 | W2 |

---

## 10. Structured Questionnaire — Batch 2 (Q-15…Q-26)

Answer format: one line per question ("Q-15: MSG91, we handle DLT" is perfect). Urgent items are marked ⚠.

**Vendors & integrations**
| # | Question | Recommendation |
|---|---|---|
| Q-15 ⚠ | Final SMS provider for OTP + transactional SMS, and who performs DLT registration (us or your team)? | MSG91 (India DLT-native, cost); start W1 |
| Q-16 | Keep FCM for push notifications even though Firebase Auth is dropped? | Yes — push infra stays FCM; minimal change |
| Q-17 | Email provider: AWS SES, Resend, or plain SMTP relay? | SES for SaaS; SMTP fallback for standalone |
| Q-18 | Self-host object storage: bundle MinIO in the standalone compose, or require operator-provided S3? | Bundle MinIO default |

**Platform & business**
| # | Question | Recommendation |
|---|---|---|
| Q-19 | SaaS pricing model for plan limits: per seat, per location, or per usage (bookings/SMS)? | Per location + usage caps to start |
| Q-20 | Approve read-only, PHI-redacted "support mode" alongside break-glass? | Yes (G-01) |
| Q-21 | Tenant export/erase at GA: JSON + files archive acceptable? Must include invoices/Rx as PDFs? | JSON + files archive; PDF bundle fast-follow |

**Compliance & security**
| # | Question | Recommendation |
|---|---|---|
| Q-22 | External penetration test before GA (budget/vendor), or internal structured review only? | External if budget allows, schedule W9 |
| Q-23 | Legal review needed for DPDP consent wording/screens before W7? Who owns it? | Start in W2 parallel track |

**Delivery & ops**
| # | Question | Recommendation |
|---|---|---|
| Q-24 | CI/CD: GitHub Actions + GHCR artifacts acceptable? Self-hosted runners needed? | GH Actions + GHCR |
| Q-25 | Observability stack: Prometheus + Grafana bundle for both deploy forms? | Yes; hosted alternative for saas later |
| Q-26 | SaaS tenant addressing at GA: subdomains only, or custom domains too? | Subdomains at GA; custom domains fast-follow |

**All questions resolved (2026-08-19):** Q-02 → D-28 · Q-04 → D-38 (moka yes, utoipa no) · Q-08 → D-39 · Q-09 → D-40 · Q-10 → D-41 · Q-11 → D-42 · Q-13 → D-16 · Q-14 → D-29. Structure guidelines + Laravel-familiar layout: `10-STRUCTURE-AND-GUIDELINES.md` (D-43).