# MNC-grade readiness audit — 2026-10-23

Scope: `platform/keel-platform` as the framework behind a **medicare/hospital
appointment product** (admin web + user app + doctor app), judged at the bar an
MNC/enterprise healthcare deployment would apply. Method: six parallel static
audit lanes (runtime/kernel, security/supply-chain, data/ORM/persistence,
observability/ops/deployment, testing/API-governance/docs, healthcare
compliance/PHI), each with file:line evidence. Read-only; the only files
written are in this directory. Rows are **candidates** — NOT merged into
`../kelson-laravel-forensic-2026-10-21/ACTIONABLE-FINDINGS.csv` until each is
individually approved (the MVA house rule).

Severity is the house S-scale (S0 blocker … S3 polish). Cross-references name
existing register rows so nothing double-counts: KFA-nnn = the forensic
register, MVA-nnn = the MVVC-first register (`../mvvc-first-2026-10-23/`),
LF/LDB = the feature atlas / parity ledger.

## 1. Verdict

**Current posture: NOT PRODUCTION READY for a real patient population — but
the distance has changed shape.** The forensic audit's pillars (supply chain,
host boot, lint/clippy gates) are closed and verified in the lockfile this
pass (wasmtime 36.0.19, rustls 0.23.46/0.103.16, webpki-roots 1.0.10). What
now blocks launch is **application-trust-boundary correctness** (tenant
membership at login, RLS fail-open, double-booking guard, request timeouts)
and **healthcare-specific obligations** (PHI encryption-at-rest, read audit,
consent, erasure, breach tooling) — plus a deployment story that today cannot
serve traffic from outside a pod at all.

**53 candidate rows: 8 S0, 20 S1, 18 S2, 7 S3.**

## 2. The register (MNC-001..MNC-053)

### S0 — launch blockers

| ID | Finding | Evidence | Smallest safe fix |
|---|---|---|---|
| MNC-001 | Tenant isolation bypass at login/register: a client-supplied `body.tenant` is trusted and `install_access_snapshot` hardcodes `membership_active: true` — any authenticated principal enters any tenant | `runtime/kelson-demo/src/auth_routes.rs:445,370,170`, `resolve_tenant :128` | gate snapshot install on an active `MembershipStore` row for principal+tenant; refuse 403 otherwise |
| MNC-006 | No server-side TLS anywhere: plain `TcpListener` + `axum::serve`; secure-cookie default off — PHI travels cleartext whenever the "trusted proxy" isn't | `runtime/kelson-demo/src/lib.rs:1213,1238`; `crates/web/http/src/session_mw.rs:78-88,144` | tokio-rustls acceptor or a boot assert: non-loopback bind requires proxy-detected HTTPS (docs/rust/05-SECURITY-ISOLATION.md:161 already promises it) |
| MNC-010 | RLS is fail-open: every tenant policy ends `OR kelson_current_tenant() IS NULL` — a forgotten `SET LOCAL` (or leaked GUC) reveals ALL tenants | `database/migrations/20260819000001_foundation.sql:185-199`; `database/rls/` ships empty | drop the NULL branch; deny on unset GUC; exempt platform roles via grants, not policy |
| MNC-011 | Slot uniqueness three-way inconsistent: `unique:Appointment` probes without a status filter (cancelled rows block forever), the DB guard is `(tenant,patient,slot) WHERE booked` (never stops two patients booking one slot = clinical double-booking), and probe→insert is a race | `applications/appointments/module.toml:115`; `crates/foundation/validation/src/checks.rs:454-468`; `database/migrations/20260822000002_appointments.sql:17-19`; `applications/appointments/src/lib.rs:107,124` | partial unique index `(tenant_id, slot) WHERE deleted_at IS NULL AND status='booked'`; map 23505→CONFLICT as the arbiter |
| MNC-028 | No request wall-clock timeout anywhere in the stack | `crates/web/http/src/middleware.rs:767-801` (Admission::apply has no timeout layer); `runtime/kelson-demo/src/lib.rs:1233-1239` | `TimeoutLayer::new(cfg.http.request_timeout)` inside `Admission::apply` |
| MNC-030 | Postgres bridge: one process-wide thread, `sync_channel(64)`, two `.expect(…)` sites — a panicking job kills it permanently, a hung query blocks `recv()` forever; no statement timeout set on the pool | `crates/data/postgres/src/bridge.rs:56-84`; `crates/data/postgres/src/lib.rs:46-53` | `options=-statement_timeout=…`; `submit` returns `AppError::persistence` instead of `expect` |
| MNC-041 | Worker ignores SIGTERM: awaits only `ctrl_c()`, so Docker/k8s stop kills it mid-job; graceful drain, outbox flush and telemetry shutdown unreachable | `runtime/kelson-worker/src/main.rs:919-924` vs demo's correct `lib.rs:1243-1256` | reuse the demo `shutdown_signal()` (SIGINT+SIGTERM) |
| MNC-039 | The shipped container binds loopback only: `listen = "127.0.0.1:8180"` baked into the image — no Service/Ingress/probe outside the pod can reach it | `config/dev.toml:5`; `Dockerfile:37` (copies exactly that config), `EXPOSE 8180` at 46-51 | ship a prod config overlay (`listen = "0.0.0.0:8180"`); make the image's default config the prod one |

### S1 — must-fix before general availability

| ID | Finding | Evidence | Fix sketch |
|---|---|---|---|
| MNC-002 | `deny.toml` guards the WRONG workspace (`platform/deny.toml`; keel-platform is its own workspace) — "cargo deny gate green" doesn't apply to this tree | `platform/keel-platform/Cargo.toml` + absent `keel-platform/deny.toml`; contradicts REMEDIATION-ROADMAP.md:149 | move/copy deny.toml into keel-platform; re-run the gate |
| MNC-003 | Session-bound CSRF compare not constant-time (`as_bytes() ==`) | `crates/security/sessions/src/lib.rs:356` | `signed::constant_time_eq` |
| MNC-004 | Login throttle spoofable: first X-Forwarded-For hop trusted unconditionally | `crates/web/http/src/request.rs:65-75`; `auth_routes.rs:52-53` | honor XFF only from trusted proxy ranges (already modeled: middleware.rs:509,725) |
| MNC-005 | `/auth/register` unthrottled (CPU-DoS via Argon2 per hit) + email-existence enumeration | `auth_routes.rs:335-351` | same bucket; uniform 202 + async verification mail |
| MNC-007 | Webhook egress SSRF: checks only `https://` prefix; private https targets deliver | `adapters/kelson-webhooks/src/lib.rs:79,15` | resolve-then-reject loopback/RFC1918/link-local before dispatch |
| MNC-019 | PHI stored plaintext: the `encrypted` cast exists but NO model field applies it (patient free-text, email) | `crates/data/models/src/casts.rs:315`; `crates/data/appointments/src/lib.rs:42-85`; `crates/security/identity/src/lib.rs:48-56` | `cast="encrypted"` on PHI fields + bound sealer at boot |
| MNC-020 | No PHI→principal binding: `ListHandler` returns ALL tenant appointments; "patient sees own records" is unenforceable | `crates/data/appointments/src/lib.rs`; `applications/appointments/src/lib.rs:64-70,113-120` | appointment↔principal FK + object-level policy (ownership check is built; needs the link) |
| MNC-021 | No PHI **read**-access audit trail (HIPAA §164.312(b)) — only mutations audited | `crates/execution/engine/src/unit.rs:38-58` | read-access log on model-store reads of PHI models |
| MNC-022 | Consent capture MISSING entirely; OAuth2 explicitly consent-less first-party; mail/SMS not opt-in gated | `crates/data/oauth2/src/lib.rs:20`; notifications lib.rs:288-321 | consent record + gate comms/dispatch on it |
| MNC-023 | No GDPR Art.17 erasure workflow / retention scheduler; prune jobs cover queues only; soft→hard delete never cascades | `crates/async/jobs/src/lib.rs:625-629,1081` | erasure job (rows, outbox, inbox, notifications) + retention schedule |
| MNC-024 | No breach detection/notification tooling (HIPAA §164.404, GDPR Art.33) | `crates/security/tokens/src/reset.rs:26` ("lists belong elsewhere"); SECURITY.md covers vuln-report only | incident-runbook surface + export |
| MNC-025 | `docs/20-SECURITY-COMPLIANCE.md` claims "HIPAA/GDPR Aligned" and documents a PHP/Firebase stack not in this codebase — false-assurance liability | docs/20 vs the Rust tree | rewrite to what IS built + the named gaps above |
| MNC-027 | The product's own app layer (admin web / user app / doctor app) does not exist — only one book-appointment view + API modules | repo root `medicare-kernel/apps` = api only; runtime ships demo views | the MVVC surface (MVA-004..008) is the on-ramp; scope an app wave |
| MNC-029 | Guest wasm defaults: deadline `u64::MAX/2`, `max_concurrency: None` = unlimited; no shipped module declares budgets | `crates/extension/wasm/src/lib.rs:509,76-77,1407-1413` | host-side default deadline/concurrency from config |
| MNC-031 | Postgres branch skips the schema assertion the MySQL branch runs (`let _ = p;`) — boots against an unverified schema | `runtime/kelson-demo/src/lib.rs:301-307` | call `kelson_postgres::assert_current` symmetrically |
| MNC-032 | Fail-open degraded boot: `database.required` defaults false → in-memory boot with a warning (prod would silently lose data) — **same finding as KFA-006, still open** | `crates/application/configuration/src/lib.rs:116-119`; `runtime/kelson-demo/src/lib.rs:341-348` | refuse degraded boot outside dev/test (KFA-006 fix sketch) |
| MNC-033 | Serialised operations lock in-process only (`InMemoryConcurrency` default, no production caller of `with_concurrency`) AND block a worker thread (`Condvar::wait` inside an async fn) | `crates/execution/engine/src/engine.rs:54,73,196-207`; `concurrency.rs:72-79` | pg advisory-lock port + async acquire with deadline |
| MNC-036 | Graceful shutdown waits unbounded; the required-DB failure path `process::exit`s before telemetry flush | `runtime/kelson-demo/src/lib.rs:1238,343-344,1221` | `select!` a drain timer; return Err instead of exit |
| MNC-040 | `kelson-worker` has no container story: Dockerfile builds demo only; no app-wide compose, no k8s manifests | `Dockerfile:24,34`; `docker-compose.test.yml` (DB-only) | worker build stage + compose service + manifests |
| MNC-042 | PHI leaks to logs: worker logs whole job payload (`payload = %job.payload`); redaction is key-name-based, message bodies never scrubbed | `runtime/kelson-worker/src/main.rs:213-219`; `crates/observability/telemetry/src/classify.rs:8-23` | PHI vocabulary + payload-field redaction by default |
| MNC-043 | Ops surfaces unauthenticated: `/metrics`, `/_dashboard`, `/meta/*`, `/__runtime/status` gate on flags only | `runtime/kelson-demo/src/routes.rs:409-448` | admin capability requirement on the ops router |
| MNC-049 | No browser/E2E pipeline: Playwright specs in the starter template never run (no CI job); none for the medicare app; end_to_end_verified ≈ 9% | `templates/starter-template/e2e/*.spec.ts`; `.github/workflows/keel.yml` | CI e2e job + first medicare journey |

### S2 — hardening for the enterprise bar

| ID | Finding | Evidence | Fix sketch |
|---|---|---|---|
| MNC-008 | rsa Marvin (RUSTSEC-2023-0071, no patch) permanently ignored; transitive via sqlx-mysql — **rides KFA-007** | `platform/deny.toml:30`; Cargo.lock:3375 | drop sqlx `mysql` feature or document the compensating TLS-only posture |
| MNC-012 | MySqlReadModelStore orphaned: zero install callers; refresh interpolates def table/query into SQL and DELETE+INSERTs outside a tx (empty windows visible) | `crates/data/models/src/readmodel.rs:230`; `crates/data/mysql/src/readmodel.rs:43-58` | tx-wrap + vetted-def gate + install (MVA-007 is the consumer) |
| MNC-013 | Read-model `register()` "validation" is a tautology over `def.source_models` | `crates/data/models/src/readmodel.rs:217-224` | validate against the model registry |
| MNC-014 | Missing hot-path indexes: sessions(principal_id/expires_at), credentials(principal_id) | `foundation.sql:24-39` | three `CREATE INDEX` migrations |
| MNC-015 | No `ON DELETE` on any FK; soft-delete only on appointments → hard-delete wedges, "live row" probes uneven | grep `database/migrations*`; probes.rs:25 | explicit per-FK actions + uniform `deleted_at` |
| MNC-016 | Pool config minimal: max_connections + acquire_timeout only; no max_lifetime/idle_timeout/min_connections | `postgres/src/lib.rs:46-53`; `mysql/src/lib.rs:93-118` | three more builder calls per driver |
| MNC-017 | Migrations embedded at compile time (`sqlx::migrate!`); MySQL twins use non-idempotent `CREATE INDEX`/ALTER outside tx — partial apply bricks restart | `postgres/src/lib.rs:160`; `migrations-mysql/20260822000002_appointments.sql:19-31` | runtime-shipped files + existence-guarded MySQL DDL |
| MNC-018 | Unbounded reads where a paginator exists: `count` lists all rows; `list()` falls back unbounded without page params | `crates/data/appointments/src/lib.rs:218-226`; `applications/appointments/src/lib.rs:304` | default-clamp page in list |
| MNC-026 | No data-residency controls, no PHI-safe backup/DR tooling; `kelson rollback` restores the checkpoint, not data | deployment/rollback.rs:119-123 | `db:backup`/`db:restore` + region pinning story |
| MNC-034 | Sync WASM dispatch + sync DB bridge run on the async worker (block_in_place / Handle::block_on) | wasm lib.rs:1659-1691; bridge.rs:34-46 | spawn_blocking the whole dispatch |
| MNC-035 | Poisoning unwraps on the hot path (`cell.read().unwrap()` per request; reloader locks) | `runtime/kelson-demo/src/hot_reload.rs:81,120-142`; `graph_routes.rs:369,1819` | `unwrap_or_else(|e| e.into_inner())` |
| MNC-037 | Generation drain/promote unwired: `WasmPlugin::drain/reinstantiate` test-only → rollback = restart (weakens the KFA-009 no-rebuild promise at the module layer) | `runtime/kelson-demo/src/wasm_tests.rs:819+`; lib.rs:3185 | `wasm:promote` reload subscriber |
| MNC-038 | Guest `AppError` crosses the seam with its own `public_message` verbatim — guest text bypasses host redaction | `crates/extension/wasm/src/lib.rs:1722` → `crates/web/http/src/lib.rs:118` | force a fixed public message for guest-origin Internal errors |
| MNC-044 | Alerting is a no-op: policy parses `alert` into a dead boolean; SLOs render-on-scrape only, no burn-rate action | `crates/observability/pipeline/src/policy.rs:465`; `metrics/src/slo.rs` | route Action::Alert to the notification hub |
| MNC-045 | (ops) no backup/restore command surface — merged with MNC-026 | console grep; mysqldump at main.rs:4475 is schema-reference only | see MNC-026 |
| MNC-048 | pg/redis suites gate behind `pg-tests`/`redis-tests` features + unset env → silent skips outside a separate CI service job; whole-workspace llvm-cov broken (LNK1318 disk) | 12 cfg-gated files; `keel.yml:286-323`; EXECUTIVE-VERDICT §2 | always-run service job + coverage ratchet |
| MNC-050 | Zero property/fuzz/nondeterminism tooling (no proptest/loom/insta/trybuild anywhere) | test inventory: none found | proptest on parser/validator seams; fuzz the wasm ABI |
| MNC-051 | API governance incomplete: no `Deprecation`/`Sunset` headers (flag exists in graph only), no changelog policy, semver gate self-skips without a tag | `graph_routes`/`main.rs:1818`; `xtask/src/main.rs:77` | emit both headers from `deprecated`; gate the tag |
| MNC-053 | Wall-clock timing tests still flake-prone (50ms sleeps, hot_reload pair + 6 more) | `runtime/kelson-demo/src/hot_reload.rs:489`; `configuration/src/runtime.rs:396` | injectable clock — **same class as F-013** |

### S3 — polish

| ID | Finding | Fix sketch |
|---|---|---|
| MNC-009 | No `=`-pins on the four proven-good patched versions (ranges elsewhere) | pin wasmtime/rustls/rustls-webpki/webpki-roots |
| MNC-046 | OTLP exporter implemented but no shipped config enables it; no metrics path | prod telemetry overlay |
| MNC-047 | SBOM/provenance generated as upload artifacts, never attested; no image push/cosign | cosign sign/attest job |
| MNC-052 | Onboarding/licensing/a11y thin: no root README for keel-platform, no LICENSE texts (MIT/Apache declared only), shipped templates zero `aria-*`/`role=`, `<html lang="en">` hardcoded despite the ar/fr catalogs | README + LICENSE files + a11y pass + `@lang` in templates |
| MNC-054* | examples/{minimal,reference} still carry no MVVC usage (F-005 tail) | one bound example |

\* numbered past 053 to stay lane-consistent; merged count is 53 with MNC-045 folded into MNC-026.

## 3. What stayed green under re-examination

- Supply chain pins (wasmtime/rustls/webpki-roots) — verified against the lockfile this pass.
- JWT HS256 pinning, session-fixation rotation on login, `Secret` hygiene, Argon2id defaults, PAT ability ceilings (KFA-010 re-confirmed).
- The error contract (stable KSN_* codes, RFC-7807 problem+json, redaction) — strong.
- Command/CSRF plane design (session-bound token, web-plane-only guard) — sound once MVA-003/004 land; the proof suite passes 4/6 with the two MVVC render cases under active fix.
- Migrations advisory-lock runner, healthz/readyz split, demo shutdown drain, outbox atomicity on MySQL — real engineering.

## 4. Relationship to the existing registers

- KFA-006 (degraded boot) = MNC-032; KFA-007 = MNC-008; KFA-009/013/014/015/018..023 remain as they are — none of this audit closes them, three deepen them (MNC-037 feeds KFA-009's evidence, MNC-048 feeds KFA-018's coverage, MNC-002 makes KFA's deny-gate claim workspace-correct).
- MVA-001..009 (the MVVC batch) are unaffected by this audit except MVA-007, whose read-model half must now also carry MNC-012's tx + SQL-interpolation hardening to count as a consumer.
- F-010 (doctor probe) reconfirmed by the ops lane at `main.rs:875-949`.
- MNC-027 is the product-gap row the whole register points at: the framework's launch value depends on the first real app wave riding MVA-004..008.
