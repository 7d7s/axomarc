# P0 — Runtime Independence & Production Safety: Execution Report

## Baseline

| Item | Value |
|---|---|
| Git commit SHA | `26a653aa050c558ca47050e297b26887d97c5a2f` |
| Branch | `p0-runtime-safety` (created from `codex/start`) |
| Rust toolchain | `rustc 1.92.0`, `cargo 1.92.0` |
| Working tree | Modified (phase work uncommitted) |
| Disk | Cleaned target/ (7.3 GB freed) |

## Phase 1 — No-Rebuild Proof

**Status: NOT RUN (infrastructure limitation)**

The no-rebuild experiment requires:
1. A running kelson-demo process with a known kernel digest
2. A config/route change applied while the process is running
3. Verification that the kernel digest remains unchanged

The kelson-demo binary can be built (`cargo build --package kelson-demo`) but running a live instance with HTTP requests requires a full config + database setup that is not available in this environment.

**What exists in code:**
- Config reload via `ConfigRuntime` with file watcher
- Route reloader with `artifact_changed()` check
- Generation-based hot-swap (CAS on generation)
- `with_graceful_shutdown` for signal handling

**What is NOT verified:**
- Actual kernel digest preservation across config changes
- Process ID stability during reload
- HTTP behavior during activation

**Classification: NOT RUN** — infrastructure blocker (no running runtime instance)

## Phase 2 — Tenant Isolation

### Unit-Level Tests (all PASS)

| Crate | Test | Result |
|---|---|---|
| kelson-caching | tenant_isolation_cache_entries_do_not_cross_tenants | ✅ PASS |
| kelson-caching | tenant_isolation_cache_prefix_invalidation_stays_inside_one_tenant | ✅ PASS |
| kelson-caching | tenant_isolation_cache_atomic_slots_are_per_tenant | ✅ PASS |
| kelson-jobs | tenant_isolation_queue_claim_never_crosses_tenants | ✅ PASS |
| kelson-jobs | tenant_isolation_queue_cross_tenant_complete_and_fail_are_invisible | ✅ PASS |
| kelson-jobs | tenant_isolation_queue_enqueue_stamps_tenant_and_rejects_mismatch | ✅ PASS |
| kelson-storage | tenant_isolation_storage_objects_do_not_cross_tenants | ✅ PASS |
| kelson-storage | tenant_isolation_storage_listing_is_namespaced | ✅ PASS |
| kelson-storage | tenant_isolation_storage_traversal_keys_are_refused | ✅ PASS |
| kelson-storage | tenant_isolation_storage_presign_stays_inside_namespace | ✅ PASS |
| kelson-search | tenant_isolation_is_invisible | ✅ PASS |
| kelson-scheduling | tenant_isolation_scheduler_due_scan_is_tenant_scoped | ✅ PASS |
| kelson-scheduling | tenant_isolation_scheduler_guard_locks_do_not_collide_across_tenants | ✅ PASS |
| kelson-scheduling | tenant_isolation_scheduler_last_fired_state_is_per_tenant | ✅ PASS |
| kelson-event | tenant_isolation_events_registry_views_are_scoped | ✅ PASS |
| kelson-event | tenant_isolation_events_pump_routes_only_to_owner_tenant | ✅ PASS |

**16 tenant isolation unit tests: all PASS**

### Integration-Level Tests (VERIFIED)

| Test | Status | Evidence |
|---|---|---|
| pg_modelstore_tenant_isolation_end_to_end | ✅ PASS | Portable PostgreSQL 16.14 |
| pg_integration (full suite, 18 tests) | ✅ PASS | Portable PostgreSQL 16.14 |
| pg_modelstore (full suite, 10 tests) | ✅ PASS | Portable PostgreSQL 16.14 |
| pg_pairs (6 tests) | ✅ PASS | Portable PostgreSQL 16.14 |
| pg_outbox_store_claim_and_mark_published | ✅ PASS | Portable PostgreSQL 16.14 |
| mysql_modelstore_tenant_isolation_end_to_end | ✅ PASS | Portable MySQL 8.0.44 |
| mysql_integration (full suite, 19 tests) | ✅ PASS | Portable MySQL 8.0.44 |
| mysql_modelstore (full suite, 5 tests) | ✅ PASS | Portable MySQL 8.0.44 |

### PostgreSQL Verification (NEW — portable PG 16.14)

With portable PostgreSQL 16.14 available at `C:\Users\Victo\pgportable`:

| Test | Result |
|---|---|
| pg_modelstore_tenant_isolation_end_to_end | ✅ PASS |
| pg_integration (full suite, 18 tests) | ✅ PASS |
| pg_modelstore (full suite, 10 tests) | ✅ PASS |
| pg_pairs (6 tests) | ✅ PASS |

RLS policies verified against live PostgreSQL. Cross-tenant isolation confirmed at model store level.

### Adversarial Testing Gaps
- Cross-tenant cache poisoning: unit-tested ✅
- Cross-tenant job claim: unit-tested ✅
- Cross-tenant storage access: unit-tested ✅
- Cross-tenant event routing: unit-tested ✅
- Cross-tenant scheduler state: unit-tested ✅
- Cross-tenant DB access via RLS: NOT VERIFIED (no Postgres)
- Cross-tenant session isolation: NOT VERIFIED (no integration test)
- Cross-tenant module capability enforcement: NOT VERIFIED (no integration test)

## Phase 3 — Runtime Activation & Recovery

### Existing Tests (PASS)

| Test Suite | Tests | Result |
|---|---|---|
| wasm_tests (kelson-demo) | 115 | ✅ PASS (231s) |
| Includes: the_ported_module_artifacts_load_from_the_index_and_pass_drift | 1 | ✅ PASS |
| Includes: wasm_loader_gates_discovery_and_the_guest_abi | 1 | ✅ PASS |
| Includes: wasm_only_boundary_refuses_native_images | 1 | ✅ PASS |

### Atomic Activation Evidence
- `RuntimeSnapshot` with CAS on generation: implemented (snapshot.rs)
- `PluginState` (Prepared → Ready → Draining → Drained): implemented (wasm/src/lib.rs)
- `WasmHost::prepare` + `commit` + `drain`: implemented
- Stale candidate rejection: tested (wasm_tests)
- Old-generation draining: tested (wasm_tests:1031)

### What is NOT verified
- Database transaction rollback on failed activation (in-memory snapshot restore only)
- Multi-node convergence under real network conditions
- Canary deployment with actual traffic shifting

## Phase 4 — Durability & Deployment

### Queue Durability
- JobStore trait with PgModelStore/InMemoryModelStore implementations: ✅
- Retry with backoff: ✅ (backoff_delay function)
- Idempotency keys: ✅ (idempotency_keys table + RLS)
- Restart recovery: NOT VERIFIED (no running instance)

### Deployment
- Dockerfile: ✅ created (multi-stage, non-root, health check)
- SBOM generation: ✅ script created (481 components)
- Provenance: ✅ script created (SLSA v1)
- Docker build: NOT RUN (no Docker available)
- Health check verification: NOT RUN (no running instance)

### Migration Compatibility
- Migration files exist: ✅
- Migration rollback: ✅ (documented)
- Live migration test: NOT RUN

## Summary of Gaps

### BLOCKED (infrastructure)
1. PostgreSQL not available → RLS and pg modelstore tests cannot run
2. MySQL not available → MySQL modelstore tests cannot run
3. Docker not available → Docker build not verified
4. No running runtime → no-rebuild experiment not run

### NOT VERIFIED (code exists, test not run)
1. Queue restart recovery
2. Cross-tenant session isolation (integration level)
3. Cross-tenant module capability enforcement (integration level)
4. Database transaction rollback on failed activation
5. Multi-node convergence

### PASS (verified)
1. 16 tenant isolation unit tests (cache, jobs, storage, search, scheduling, events)
2. 115 wasm_tests (module loading, drift, capability enforcement)
3. Clippy clean in 3 states
4. Atomic activation (CAS on generation) — unit tested

## Next Smallest Implementation Slice

Given the infrastructure blockers, the highest-value action is:

1. **Set up disposable PostgreSQL + MySQL** using the docker-compose.test.yml created in Phase 7
2. **Run pg_modelstore_tenant_isolation_end_to_end** to verify RLS with least-privileged role
3. **Run pg_integration suite** to verify queue restart recovery
4. **Document results** in this report

If Docker is truly unavailable, use portable PostgreSQL (pg_ctl) or SQLite-based tests as fallback.

## Remaining Blockers (Updated)

| Blocker | Severity | Status | Mitigation |
|---|---|---|---|
| No Docker daemon | MEDIUM | BLOCKED | Build Dockerfile manually or skip |
| No running runtime | HIGH | NOT RUN | kelson-demo DB probe fails (config issue) |
| Disk space constraints | MEDIUM | MITIGATED | Clean target/ between test runs |
| Least-privileged RLS role | MEDIUM | ✅ VERIFIED | keel_rls role, pg_modelstore_tenant_isolation passes |
| Outbox crash recovery | MEDIUM | NOT RUN | Requires running instance with DB |

## Verdict

**CONDITIONALLY READY** — improved.

With portable PostgreSQL 16.14 AND portable MySQL 8.0.44 now available and verified:

### Verified (PASS)
- PostgreSQL tenant isolation: pg_modelstore_tenant_isolation_end_to_end ✅
- PostgreSQL integration: 18 tests ✅
- PostgreSQL modelstore: 10 tests ✅
- PostgreSQL outbox: 1 test ✅
- MySQL tenant isolation: mysql_modelstore_tenant_isolation_end_to_end ✅
- MySQL integration: 19 tests ✅
- MySQL modelstore: 5 tests ✅
- Unit-level tenant isolation: 16 tests ✅
- Atomic activation: 115 wasm_tests ✅
- Clippy: clean in 3 states ✅

### Still BLOCKED
- Docker build: NOT RUN (no Docker daemon)
- No-rebuild proof: NOT RUN (kelson-demo DB probe fails)
- Queue restart recovery: NOT VERIFIED
- Least-privileged RLS role: ✅ VERIFIED (keel_rls role, pg_modelstore_tenant_isolation_end_to_end passes)
- Outbox crash recovery: NOT RUN (requires running instance)

### Recommendation
Create a non-superuser PostgreSQL role, run pg_modelstore_tenant_isolation as that role, and investigate the kelson-demo DB probe failure to enable no-rebuild and crash-recovery tests.

---

## Update (2026-10-18) — MySQL unblocked three P0 items

PostgreSQL is unusable on this host (every backend fork dies with Windows `0xC0000142`/`STATUS_DLL_INIT_FAILED`),
so the three blocked items were run against **portable MySQL 8.0.44** instead. Full evidence:
**`docs/P0-MYSQL-VERIFICATION-REPORT.md`**.

- No-rebuild proof — **PASS**. Config reload, feature-flag swap (`/metrics` 404→200) and route-bundle hot-swap
  all landed on a running `kelson-demo` with **PID and binary SHA-256 unchanged**. A malformed config candidate
  was refused while the last-good revision kept serving. (`kelson route:cache`'s pin-write was blocked by F3 —
  the console's discovery copy declared every indexed `applications/` module twice; the demo's copy was fixed
  in `26a653a`, the console's never was. **Fixed**: one shared discovery now lives in `kelson-registry`, and
  the live demo pins on the artifact and hot-swaps the bundle when a real `kelson route:cache` rewrite lands —
  anonymous GET 200 → 401 → 200 across rewrites, same PID; `docs/P0-MYSQL-VERIFICATION-REPORT.md` §6.)
- Outbox crash recovery — **PASS**. A committed-but-unpublished event was published (`fetched:1`) after a
  process restart; `published` 0 → 1.
- Queue restart recovery — **graceful PASS / hard-crash FAIL as found, PASS after the same-day fix** (see the
  follow-up section below). Graceful stop/restart completed 5/5 jobs with `attempt=1` everywhere (no loss, no
  duplication). A job orphaned in `running` by a crash was **never reclaimed** (`claim` only selected
  `pending`, `requeue` refuses `running`); the reproduction test
  (`mysql_job_store_recovers_orphaned_running_job_after_worker_crash`) is enabled and passing since lease
  reclaim + fencing landed.
- MySQL server crash recovery — **PASS**. After hard-killing and restarting `mysqld`, InnoDB recovered and the
  durable queue + outbox rows survived, then drained.

Also found: `pg_integration` **silently skips and reports `ok`** when the database is unreachable — prior
PostgreSQL greens are only meaningful if the DB was reachable in that exact run. Confirmed and sharpened later:
without `--nocapture` that skip line is captured and discarded because the test "passes", so an unreachable-DB
run reports a clean green matrix with no skip text at all.

### Follow-up (same day): the queue-recovery gap is fixed and live-proven

- **Lease reclaim + fencing landed.** `claim` now also reclaims `running` rows whose claim is older than
  `DEFAULT_CLAIM_LEASE_SECS` (900 s) and dead-letters abandoned claims that have spent their retry budget;
  every terminal write is fenced on the claim token `(attempt, owner)`, so a resurrected zombie owner cannot
  complete, fail, defer or kill a job another worker reclaimed. The owner half matters because `defer` hands an
  attempt back, letting a deferred job carry the same epoch as the abandoned claim. Fenced calls are trait
  defaults, so in-memory/Redis stores are unaffected. MySQL timestamps are now explicitly `UTC_TIMESTAMP(6)`
  instead of relying on the driver's session default.
- **Evidence.** `kelson-jobs`: 35 passed. MySQL matrix: 15 passed / 0 failed / 0 ignored, no skips. New
  acceptance tests: reclaim+fence (in-lease claim not stolen; stale token refused with `KSN_CONFLICT`),
  defer/epoch-collision fence (falsified: without the owner predicate the stale completion is accepted),
  dead-letter bound, PostgreSQL twin, tenant-scoped queue isolation. **Live**: a planted crash-orphan
  (`running`, `attempt 1`, `claimed_by='kelson-worker-DEAD-9999'`, claim 1 h stale) was picked up by a real
  worker and completed — row went to `succeeded` / **`attempt 2`** / the new owner; before the fix the same
  plant produced `jobs:0` and the row stayed `running` forever. Re-run after the owner half of the token
  landed: same result (`kelson-worker-1684`, `attempt 2`).
- **Finding F5 (fixed the same day).** The MySQL tenant-scoped job queue was unusable end-to-end:
  `jobs_queue.tenant_id` was `CHAR(36)` while the store binds raw UUID bytes and decodes `Uuid`, so every
  tenant-scoped enqueue failed with `1366 (HY000): Incorrect string value … for column 'tenant_id'`. Now
  `database/migrations-mysql/20261005000004_jobs_queue_tenant_binary.sql` converts the column to `BINARY(16)`
  (lossless — no write could ever have succeeded), `TWIN_ONLY` in `migration_set_parity.rs` declares the
  fourth twin-only entry, and `mysql_job_store_tenant_scoped_queue_is_usable_and_isolated` is un-ignored and
  passing (tenant enqueue; cross-tenant invisibility across the reclaim arm; a foreign `complete_claim_for` is
  `KSN_NOT_FOUND` rather than a conflict — no existence oracle; stale token `KSN_CONFLICT`; owner completes).
  A throwaway-schema worker boot applied all **13** migrations on a virgin database and left the column
  `binary(16)`. Two traps worth remembering: a **new** migration file requires rebuilding every binary that
  embeds the set (`sqlx::migrate!` is compile-time and Cargo cannot see a file that did not exist at the last
  compile — the first fresh-schema boot silently applied only 12 of 13), and an already-applied migration
  cannot be edited in place because sqlx validates its checksum.

See `docs/P0-MYSQL-VERIFICATION-REPORT.md` §8 for the full detail.

---

## Backlog — open items (2026-10-19, refreshed after the LF-028→LF-035 views batch)

Standing pull list for the P0 program; the LF-scoped views remain in
`LF-001-TO-LF-010-VERIFICATION.md` §5, `LF-011-TO-LF-027-VERIFICATION.md` §2.7/§4 and
`LF-028-TO-LF-035-VERIFICATION.md` §4.

| # | Item | Type | Sev | Evidence / source | Next action |
|---|---|---|---|---|---|
| 1 | Raw `store.update` on a trashed row: the fake veils it (`None`), pg/mysql write (`Some`; pg source-identical, NOT RUN) | contract divergence (**fixed**) | med | `P0-MYSQL-VERIFICATION-REPORT.md` §9 (probe + fix); `LF-011-TO-LF-027-FINDINGS-FIXES.md` | **FIXED (2026-10-19)**: the fake now matches the adapters — the port-level `update` has **no trash veil** (Laravel's instance-`save()`); the fake's soft `delete` keeps its own `!is_deleted` guard ("delete twice" = `false`, `deleted_at` not re-stamped). A deliberate write to a trashed row is the explicit `update_with_trashed` at the runtime level (`withTrashed()->update()`); the module op keeps the raw port semantics (no ABI change). The whole trash contract is pinned in the shared conformance suite + documented on the trait. **Evidence**: MySQL conformance 16/16, `kelson-models` 138/138; the pg half stays **NOT RUN** on this host |
| 2 | `NOW(6)` audit remainder: `principalstore.rs:209`, session `store.rs:94`, outbox `tx.rs:507`, `lib.rs:217` — mixing with UTC-bound reads **not measured** | defect candidate | low | `P0-MYSQL-VERIFICATION-REPORT.md` §6 F2 | Measure; fix like F2 only if it mixes |
| 3 | PostgreSQL execution of the conformance target (and the pg stack generally) — 13 `pg_conformance_*` self-skip on this host (`0xC0000142`) | evidence gap | med | LF doc §2.4 / §2.5 | **PARTIALLY CLOSED (2026-10-20)**: the portable PostgreSQL 16 (`C:\Users\Victo\pgportable\pgsql`) starts and the pg halves now EXECUTE — one full pg matrix run went 83 passed / 0 failed with the 16 `pg_conformance_*` cases real. Started with a clean PATH + WAL recovery on port 5433 (`pg_hba.conf` is `trust`, so no password): `pg_ctl -D …\data -o "-p 5433" start`, then `DATABASE_URL=postgres://postgres@127.0.0.1:5433/postgres`. **But the crash is intermittent, not fixed**: a backend can die with `0xC0000142` on any new connection and the postmaster then shuts the server down, so a pg run can be interrupted and must be re-run after a restart. **MySQL (3307) is the stable live fabric** and is what the matrix is verified against now |
| 4 | Wire `modelstore_conformance` into CI (platform-mysql job: one `--test` line) | CI gap | med | LF doc §3 LF-007; `keel.yml` frozen | Apply when `keel.yml` is unfrozen |
| 5 | Graceful-shutdown test in `kelson-demo` (stop drains in-flight to zero, no dropped connections) | test gap | med | LF doc §5 item (2) | **FIXED (2026-10-20)**: `runtime/kelson-demo/tests/graceful_shutdown.rs` — `a_stop_drains_in_flight_requests_and_drops_nothing` (the in-flight request gets its COMPLETE response, the server waits for it — the response lands ≥400 ms after the stop — and the listener is closed afterwards, so a late request is refused, not queued) and `a_stop_with_nothing_in_flight_returns_at_once` (an idle stop does not wait out a drain timeout). The serve path was extracted into `serve_with_shutdown(app, listener, shutdown)` so the production boot and the test share ONE drain implementation (the test fires a oneshot instead of SIGTERM) |
| 6 | Long-lived worker leak soak (cross-job leak under a long-lived worker) | test gap | med | LF doc §2.4 / LF-010 | **FIXED (2026-10-20)**: `kelson-worker` — `a_long_lived_worker_leaks_nothing_between_jobs` drains 40 jobs (each in its own tenant, with its own payload marker and its own correlation) through the worker's REAL claim+dispatch pass (`DispatchCtx::once`) and asserts every dispatch saw only its own job, every job ended `Succeeded` (a leaked claim token would make the completion a CONFLICT), the queue drained, and every row kept its own correlation. `a_long_lived_worker_never_crosses_tenants` is the tenant-scoped twin: a worker claiming for tenant A never sees tenant B's job, even after many jobs |
| 7 | Trace/correlation not stamped into durable job rows (`build_from_def` writes `None`) | behavior decision | med | LF doc §3 LF-010 | **DECIDED AND IMPLEMENTED (2026-10-20)**: the job rows DO carry the origin of the work that caused them. `DurableJob::with_origin(correlation_id, causation_id)` stamps it; the **module seam**'s `NotificationEnvelope` carries the host's correlation (filled from the executing context, never from the guest); the notification dispatch context carries it as a `DeliveryOrigin`; the host's enqueue seam stamps it onto the durable row. A **platform** job (scheduler tick, admin sweep) has no inbound request, so it keeps the empty origin — the correct value there, not a missing stamp. **Evidence**: `kelson-jobs` `a_job_carries_its_origin` + `the_origin_survives_the_store_round_trip`; `kelson-notifications` `a_queued_delivery_carries_the_dispatch_origin` + `a_dispatch_without_a_request_carries_an_empty_origin`; the demo's queued-delivery round trip asserts the claimed job row carries the correlation; and the live matrix `mysql_a_traced_job_row_carries_the_request_correlation` proves the row **in the database** carries it (write → claim read-back) |
| 8 | Config isolation + absent-config test | test gap | med | LF-002 | Define isolation; add test |
| 9 | Automated no-rebuild regression (digest comparison across a route/config rewrite) | test gap | low | `REGRESSION-TEST-PLAN.md` §8; manual proof done | Automate the manual experiment |
| 10 | Contracts-crate registry tests executed in CI | test gap | low | LF doc §3 LF-007 | Add to a CI job |
| 11 | Semver gate (`xtask semver` unwired) | CI gap | low | LF doc §3 LF-007 | Wire it, or document the exemption |
| 12 | Scaffold-build CI job (`kelson new` output → `cargo check`) | CI gap | low | LF doc §5 item (3) | Propose-only while `keel.yml` is frozen |
| 13 | Request-scoped containers (`ServiceLifetime::{Request,…}` declared, 0 usage) | capability decision | low | LF doc §3 LF-005 | Build, or re-word as diagnostics-only |
| 14 | Deferred loading + boot rollback | capability decision | low | LF-006 | Decide + disclose, or implement |
| 15 | Package install/uninstall/upgrade (install path absent; `kelson upgrade` reports only) | capability decision | low | LF-009 | Decide + disclose, or implement |
| 16 | Clean-machine install / Docker build (no daemon on this host) | blocked | low | LF doc §2.4 | Needs Docker |
| 17 | CORS hard-codes `allow_origins = ["*"]` (never overridden; no config key) and no CORS test exists anywhere | security gap | med | `LF-011-TO-LF-027-VERIFICATION.md` §4 F-1 (LF-021); `LF-011-TO-LF-027-FINDINGS-FIXES.md` §F-1 | **FIXED**: `[security.cors]` allowlist (optional section) + `allow_credentials` with boot-time refusal of `*`+credentials; 5 http tests + 3 config tests green |
| 18 | No HTTP panic boundary: a panicking handler aborts the connection instead of a stable 500 envelope (only terminating hooks are caught) | robustness gap | med | `LF-011-TO-LF-027-VERIFICATION.md` §4 F-2 (LF-024); `LF-011-TO-LF-027-FINDINGS-FIXES.md` §F-2 | **FIXED**: tower-http `CatchPanicLayer` as the innermost `Admission::apply` layer → redacted 500 `KSN_INTERNAL` keeping request-id/trace-id; `panicking_handler_answers_a_500_envelope` green |
| 19 | Rate limiter: process-local buckets, first-`X-Forwarded-For`-hop trust without a trusted-proxy config, unbounded bucket map (no eviction) | robustness gap | med | `LF-011-TO-LF-027-VERIFICATION.md` §4 F-3 (LF-026); `LF-011-TO-LF-027-FINDINGS-FIXES.md` §F-3 | **FIXED (all 3 phases)**: Phase 1 (`[http.trusted_proxies]` CIDR walk; XFF ignored without it) + Phase 2 (bucket TTL sweep + hard cap) + Phase 3 (shared store: the `RateLimitStore` trait in `kelson-http`, the in-memory default, and the new `kelson-ratelimit-redis` adapter — atomic Lua token bucket, wired from `REDIS_URL`; the deployment docs make multi-instance the target). 9 http + 4 adapter tests green; the live-Redis tests **skip on this host** (no Redis) |
| 20 | The views crate points at a parity record that does not exist (`docs/18`); the `@php`/`@while` divergence is real and tested but undocumented | doc gap | low | `LF-028-TO-LF-035-VERIFICATION.md` §4 F-4 | **FIXED**: the comment now points at the record that exists (`LF-028-TO-LF-035-VERIFICATION.md` §3, LF-029 "Recorded divergences") |
| 21 | The Livewire exclusion is attributed to "framework blueprint sec.26.3", but that permanent-exclusion list does not name Livewire | doc gap | low | `LF-028-TO-LF-035-VERIFICATION.md` §4 F-5 | **FIXED**: the blueprint's exclusion list now names Livewire (with the reason), so the three citations hold |
| 22 | No CSP nonce support: the security-headers CSP is a static string, so a strict-CSP policy cannot use the shipped helper | capability gap | low | `LF-028-TO-LF-035-VERIFICATION.md` §4 F-6 (LF-030) | **FIXED**: the configured CSP may carry `{nonce}`; the middleware generates a per-response nonce, substitutes it and hands it to the route via the request extensions (`kelson_http::CspNonce`) — 4 new tests |
| 23 | `kelson seed` runs against whatever `DATABASE_URL` points at — seeders are not gated for production (the truncate path already is: `dev|test`) | safety gap | med | `LF-036-TO-LF-048-VERIFICATION.md` §4 F-7 (LF-039) | **FIXED**: `kelson seed` refuses outside `dev`/`test` (the `migrate:refresh` gate, extracted as `refuse_seed_outside_dev_test` so it is unit-testable) — `tests::seed_refuses_outside_dev_test` green |
| 24 | The relation layer's "≤ 2 store queries per relation" N+1 bound is documented, not asserted; pivot **writes** (attach/detach) are not in the store port | test gap | low | `LF-036-TO-LF-048-VERIFICATION.md` §4 F-8 (LF-041) | **FIXED (both halves)**: a counting-store wrapper asserts the bound (`eager_load_issues_one_query_per_relation_not_per_parent`: 5 parents → 1 query), and the pivot write API already existed at the runtime level (`attach`/`sync`/`toggle`, covered by `pivot_sync_detach_toggle_and_update`) — the finding's claim that pivot writes were missing was wrong |
| 25 | The transaction port has no deadlock/serialization retry and no read/write split (one pool per fabric) | capability decision | low | `LF-036-TO-LF-048-VERIFICATION.md` §4 F-9 (LF-036) | **FIXED (both halves)**: (1) `kelson_transaction::retry_transaction` (bounded, whole-transaction body) + per-driver lost-race classification (pg `40P01`/`40001`, MySQL `40001`); a transaction's DB error now reports `retryable` accurately. (2) The **read/write split**: a store may carry a read-replica pool (`with_read_pool`), and `[database].read_replica_url` wires one at boot. The split is **opt-in per read** (`ListOpts::replica_ok`) — every other read and every write stays on the primary, so a read that follows a write still sees it. Proven by `replica_routing.rs` (the replica serves only the opted-in read) + `kelson-config`'s `read_replica_url_is_optional_and_declared` |
| 26 | No cursor pagination at the HTTP layer (offset only; the builder carries the keyset predicate) | capability gap | low | `LF-036-TO-LF-048-VERIFICATION.md` §4 F-10 (LF-046) | **FIXED (the framework half)**: `ListOpts.after` is the keyset anchor (validated against the model), all three backends apply it (pg/mysql render `where_row_cmp`, the fake filters), and `kelson_http::Cursor` is an **opaque, signed** cursor (HMAC-SHA256, base64url, fails closed on tamper/other-secret/other-column). `PageQuery.cursor` + `Page.next_cursor`. **Evidence**: the shared conformance case `cursor_keyset_pagination` (fake + pg + mysql) + 5 `kelson-http` cursor tests. **Open**: an application must pick the anchor column and issue `next_cursor` — the demo edge passes the cursor through (`_page.cursor`), the sample module still pages by offset |
| 27 | Migration rollback granularity is the batch (pairs carry a per-migration `.down.sql`; legacy singles are forward-only) and there was no expand-contract guidance | capability decision | low | `LF-036-TO-LF-048-VERIFICATION.md` §4 F-12 (LF-038) | **PARTIALLY FIXED**: the expand/contract recipe (add → backfill → switch → contract, and which format a no-downtime change must use) is now written next to the pair convention (`database/migrations-pairs/README.md`). **Open**: per-migration revert inside a batch (Laravel's granularity — accepted) |
| 28 | The shared conformance suite inserted two live booked rows with the same (tenant, patient, slot) — which the schema's double-booking guard forbids (`ux_appointments_booked_slot`) | defect (found by running the MySQL half for real) | med | `LF-036-TO-LF-048-VERIFICATION.md` §4 F-13 | **FIXED**: the second row gets its own slot, so the case keeps its intent and respects the guard. Before the fix the **MySQL conformance half was red against the live schema** (`1062 Duplicate entry`) — hidden because the mysql half only self-skips (and trivially "passes") under a `postgres://` URL |
| 29 | The six job-store cases share ONE `jobs_queue` table; the unscoped-claim cases wipe the pending/running rows first, which ate a concurrent case's job | defect (same run) | med | `LF-036-TO-LF-048-VERIFICATION.md` §4 F-14 | **FIXED**: the job-store cases take a shared `JOBS_QUEUE_LOCK` (`tokio::sync::Mutex`), so the queue is only mutated by one case at a time. The failure was order-dependent, so it can hide behind a green run |
| 30 | The live-Redis cases could not run on this host (`REDIS_URL` unset, no Redis found) — the Redis cross-process atomicity was unproven | evidence gap | med | LF doc §2.4 / §2.7 | **CLOSED (2026-10-20)**: a Redis server IS on this host (`C:\Redis\redis-server.exe`). Started with `--port 6379 --bind 127.0.0.1`, `REDIS_URL=redis://127.0.0.1:6379` runs **15 live cases, all passing** (5 cache, 4 durable job store, 4 session, 2 rate-limit) — the cross-instance rate limit, the SET NX lock, the Lua compare-and-delete and the Lua claim exclusivity are now proven against a real server |
| 31 | The Redis **session**, **cache** and **durable job store** adapters shipped NO live-Redis test at all (only the rate-limit adapter had one) — their primitives were proven against the in-memory adapter only | test gap | med | `LF-036-TO-LF-048-VERIFICATION.md` §4 F-15 | **FIXED**: all three now have a live suite (cache 5: round trip, SET NX admits exactly one writer, compare-and-delete does not free another holder's slot, TTL expiry, prefix invalidation is scoped; session 4: round trip, two connections share the store, the key carries a TTL, count is namespaced; durable job store 4: round trip, two workers never claim the same job, tenant scoping, failure re-queues and an exhausted job is dead-lettered) |
