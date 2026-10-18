# P0 MySQL Verification Report — runtime independence & crash recovery

Date: 2026-10-18
Branch: `p0-runtime-safety`
Backend under test: portable **MySQL 8.0.44** at `mysql://keel:keel@127.0.0.1:3307/keel`
Hosts: `kelson-demo` (edge) and `kelson-worker` (durable job runner + outbox pump), built `dev`.
Config: `config/dev-mysql.toml` (`[database].driver = "mysql"`), so `config/dev.toml` stays pristine.

Every claim below is backed by a live process run, not a unit test and not a document.
"PASS/FAIL/BLOCKED" is used strictly.

---

## 0. Why MySQL, and the real PostgreSQL root cause

PostgreSQL is **not usable in this environment**, and the cause is not configuration:

- Postmaster starts and listens (`listening on IPv4 address "127.0.0.1", port 5432`); TCP 5432 is open.
- **Every backend fork dies immediately** with `server process (PID …) was terminated by exception 0xC0000142`
  (`STATUS_DLL_INIT_FAILED` — Windows process-creation/commit failure), after which the postmaster logs
  `all server processes terminated; reinitializing`.
- `C:\Users\Victo\pgportable\pg2.log` (captured live) shows this reproducibly; host free memory was ~1.5 GB.
- The app sees it as `[DB_ERROR] a database error occurred` and boots degraded.

MySQL is unaffected because Windows MySQL is single-process / thread-per-connection and never forks a backend
per connection.

### Verification-integrity hazard (must fix)

`crates/data/postgres/tests/pg_integration.rs` treats an unreachable database as a **skip that still reports `ok`**:

```
pg-tests: cannot connect ([DB_ERROR] a database error occurred); skipping
test pg_principal_and_credential_roundtrip ... ok   // finished in 0.02s
```

So every historical "PostgreSQL integration: 18/20 tests ✅" is only meaningful if the DB was reachable in that
exact run. The MySQL half proves reachability differently: 11 tests, **no skip**, 2.97 s, and a direct client
shows 25 tables (`principals` = 17 rows, `jobs_queue` = 3 rows). **A green matrix is not proof when it can
silently skip** — recommend escalating to a hard failure when `DATABASE_URL` is set but unreachable.

---

## 1. Database-backed boot (precondition)

- Migrations applied on MySQL; `kelson-demo` booted with **no** `database unreachable; booting degraded` line.
- `GET /healthz` → `{"ok":true,"data":{"ok":true,"degraded":false,"reason":"ok"}}` — `degraded:false`.
- `kelson-worker` booted `degraded:false` with services `… outbox.pump, outbox.store, jobs.store …`.

**PASS** — both hosts are genuinely DB-backed.

---

## 2. No-rebuild / hot-reconfig proof (process level)

Fixed identity for the whole experiment: **PID 776**, binary `target/debug/kelson-demo.exe`
**SHA-256 `B6A9D2CD4DC089475544A70510EC3AC7001577CF05DC43C1E45928B9CB238FA5`**, mtime 19:42:59.

| # | Change made to a running process | Observed | Verdict |
|---|---|---|---|
| 3a | `storage/framework/flags.json`: `app.metrics` = `percent100`, plus a real (hash-changing) config value edit | `/metrics` **404 → 200**; log `feature flags re-resolved {"flags":["app.metrics"],"version":2}` and `route bundle hot-swapped` | **PASS** |
| 3b | Write a **corrupt** `storage/framework/routes.json`; then clear it | corrupt → `route reload refused: keeping the current bundle (run kelson route:cache)`, `/metrics` still **200**; cleared → `route bundle hot-swapped` count 2 → 3 | **PASS** |
| 3c | Write **malformed** TOML into the config | `config reload refused: keeping the current revision` with the exact TOML parse error; `/metrics` still **200** (last-good); restore → `version 3` landed, `/metrics` **200** | **PASS** |

PID and SHA-256 were **identical before and after every step**: config, flag set, and route bundle all changed
with **no rebuild and no restart**. Config-hash reload is what notifies subscribers; a comment-only edit does not
(hash unchanged) — a real value change does.

- **3b limitation — closed by the F3 fix:** `kelson route:cache` (the pin-write path) refused on this
  workspace (`GRAPH-DUP-OP: operation oauth2.token declared by oauth2 and oauth2`,
  `GRAPH-DUP-ROUTE: route GET /oauth/authorize declared by oauth2 and oauth2`, …) because of the discovery
  defect **F3** below. With the fix it writes the artifact and the demo pins on it; §6 F3 carries the live
  re-proof (real rewrite → bundle hot-swapped → anonymous GET 200 → 401 → 200, same PID).

---

## 3. Outbox crash recovery (process level)

- Planted a committed-but-unpublished event (`outbox`, `published = 0`) with the worker stopped — the exact
  post-crash DB state.
- Restarted `kelson-worker` (MySQL): boot `degraded:false`; pump logged
  `outbox pump pass {"fetched":1,"dispatched":1,"failed":0}`.
- `SELECT … FROM outbox` → **`published` 0 → 1**.

`claim_unpublished` is a read-only SELECT and rows are only marked after delivery, so a crash can only cause
redelivery (at-least-once), never loss.

**PASS** — durable outbox recovery across a process restart.

---

## 4. Queue restart recovery (process level)

### 4a. Graceful restart — PASS

5 `demo.job` rows enqueued. Worker run 1 with `--max-jobs 2`: 2 succeeded, log `max-jobs reached; exiting`,
`kelson-worker stopped {"jobs":2}`. Worker run 2 finished the rest.

Final: `succeeded = 5`, **every row `attempt = 1`** (owner changed mid-batch: `kelson-worker-18604` → `…-2952`).

**PASS** — no loss, no duplication across a graceful stop/restart.

### 4b. Hard crash — **FAIL (real gap)**

Post-crash DB state injected: a job `status='running'`, `claimed_by='kelson-worker-DEAD-9999'`,
`claimed_at` one hour old, deadline still valid, `attempt=1`.

- Fresh `kelson-worker --once` → `{"jobs":0}`: **nothing reclaimed**.
- State after: still `running`, still owned by the dead worker.
- Admin requeue, using the store's own predicate (`WHERE status IN ('failed','dead')`) → `ROW_COUNT() = 0`.

Code confirms why: MySQL `claim` selects only `WHERE status = 'pending' AND available_at <= NOW(6)`
(`crates/data/mysql/src/jobstore.rs:202`), and `requeue` only accepts `('failed','dead')`
(`…/jobstore.rs:358`; PostgreSQL identical at `…/postgres/src/jobstore.rs:240`). There is **no lease /
visibility-timeout / heartbeat** anywhere in the job store.

**Consequence:** a worker killed mid-execution (power loss, OOM, `kill -9`, container eviction) leaves that job
**permanently orphaned** — it can neither run again nor be requeued. Laravel's database queue avoids this with a
`retry_after`/`reserved_at` reclaim.

Reproduction test added, the acceptance gate for the fix:
`mysql_job_store_recovers_orphaned_running_job_after_worker_crash`
(`crates/data/postgres/tests/pg_integration.rs`). It failed before the fix (`left: None, right:
Some(01a14f7b-…)`) and is enabled and passing since §8 landed the lease reclaim.

---

## 5. MySQL server crash recovery — PASS

Hard-killed the portable `mysqld` process (no clean shutdown), restarted it, and re-checked:

- InnoDB recovered on start; TCP 3307 back up.
- The unpublished outbox row and the pending job **both survived** the crash.
- `kelson-worker --once` then published the event (`fetched:1, dispatched:1`) and completed the job
  (`jobs_succeeded = 1`, `outbox_published = 2`).

**PASS** — the durable queue/outbox state survives a database crash.

---

## 6. Additional findings

### F1 — Queue reclaim gap (the FAIL above) — **fixed**
Landed: `claim` grew a lease arm — it also selects `running` rows whose `claimed_at` is older than
`DEFAULT_CLAIM_LEASE_SECS` (900 s, `kelson-jobs`) and that still have retry budget, dead-letters exhausted
orphans, and every terminal write is fenced on the claim token `(attempt, owner)` so a resurrected zombie owner
cannot complete, fail, defer or kill a job another worker reclaimed. The existing single-claim guarantee is
unchanged, and because reclaim is automatic an operator `queue:recover` was unnecessary. Evidence: §8 and the
follow-up section of `docs/P0-EXECUTION-REPORT.md`.

### F2 — MySQL timestamp handling mixes UTC parameters with server-local `NOW()` — **fixed**
`enqueue` binds the app's `OffsetDateTime` (UTC) for `available_at` (`jobstore.rs:156`), but retry backoff and
requeue write `available_at = NOW(6)` / `NOW(6) + INTERVAL …` (`jobstore.rs:290`, `…:356`), and `claim` compares
against `NOW(6)` (`jobstore.rs:202`). `DATETIME` carries no timezone.

Measured on this host: the worker's session `NOW(6)` evaluates in **UTC** while server-local is **IST (+5:30)**.

- A row with `available_at = UTC_TIMESTAMP(6) + INTERVAL 1 MINUTE` was **not** claimed.
- The same rows with `available_at = NOW(6) - INTERVAL 1 DAY` **were** claimed and succeeded.

So timestamps written via SQL `NOW()` are offset from the UTC values the app binds, and a retry/requeue can be
delayed by the server's UTC offset. Laravel addresses this with a connection `timezone` setting.

Fixed the second way: every SQL-written timestamp in the MySQL job store is now `UTC_TIMESTAMP(6)` — claim
comparison, retry/requeue backoff, dead-letter sweeps, completion — so `jobstore.rs` contains no `NOW(` at all
and SQL-written values agree with the bound ones regardless of the session timezone. Audit of the other MySQL
stores found `NOW(6)` still in `principalstore.rs:209`, session `store.rs:94`, outbox `tx.rs:507` and migration
bookkeeping `lib.rs:217`; whether those mix with UTC-bound reads the way the job store did was **not** measured,
so it is recorded as a follow-up candidate rather than a finding.

### F3 — `kelson route:cache` refused on this workspace — **fixed (shared discovery; live-verified)**
`kelson graph`, `kelson check`, `kelson route:cache` and `kelson optimize` all refused because every module under
`applications/` was discovered **twice** by the console's own copy: it walked `applications/*/module.toml` and then
appends `PackageIndex::manifests(root)` — and `packages.toml` indexes those very files
(`manifest = "applications/appointments/module.toml"`), so each was read twice and the graph's duplicate gate (a
correct, fail-closed gate) reports `GRAPH-DUP-MODULE: module appointments declared more than once` plus a dup
line for every operation, event and route — 4 modules (appointments, counter, echo, oauth2), 14 routes, 13
operations, 3 events; deterministic across runs and identical under `kelson graph` (exit 1).

The demo had already hit and fixed exactly this in `26a653a` ("Stop discovery from declaring an indexed module
twice"): its `discover_manifests` collapses only **byte-identical** declarations (canonical-JSON compare — a
name the index resolves to *different* content stays a duplicate, so the dedupe cannot hide drift). The console
kept a second, never-updated copy of the function, and the three-way divergence (console, demo, architecture
test) is why nothing caught it: the architecture test's copy never ran the index pass at all. The demo
therefore served while the console refused.

Repro committed: `tools/kelson-console/src/main.rs`
`discovery_does_not_declare_an_indexed_module_twice` — one manifest in `applications/alpha` indexed by
`packages.toml` must discover as ONE declaration and compile, and an index that resolves the same name to
different content must still reach the gate. Pre-fix it failed with `left: ["alpha", "alpha"]`; with the fix
it is un-ignored and green.

Fixed with the approved shape: ONE shared discovery in `crates/application/registry/src/discovery.rs`
(`discover_manifests` + `discover_route_files`, carrying `26a653a`'s rule — only byte-identical declarations
collapse; a name the index resolves to different content stays a duplicate). `kelson-console`, `kelson-demo`
and the architecture test now delegate to it, so the three copies cannot diverge again. Evidence:

- `cargo test -p kelson-registry --lib` green including 4 new discovery tests (indexed module declared once;
  same name + different content still refused; absent manifest fails closed; route-file merge).
- Console repro green; `kelson-demo`'s `package_discovery_merges_indexed_manifests_and_fails_closed` green;
  `kelson-arch-validation`'s `platform_graph_compiles_from_manifests` green (it now runs the shared discovery
  over this real workspace).
- Live: `kelson graph` exits 0 — `platform graph: 6 module(s), 14 operation(s), 3 event(s)`;
  `kelson route:cache` exits 0 and writes `storage/framework/routes.json` (15 routes, 12139 bytes);
  `kelson optimize` exits 0 (route + config + view caches warm).
- Live pin + swap on one demo process (PID 14372, `process_instance_id` `0522a79f…`, booted against artifact
  sha `88D6FA18…`): removing the availability route's `public = true` plus a real `kelson route:cache`
  changed the artifact (`0C9FD787…`) and flipped the anonymous GET **200 → 401 within 0.5 s** with **no
  restart** (same PID + `process_instance_id`, zero `route reload refused`); reverting the manifest and
  re-running `route:cache` restored the artifact byte-identical and the GET answered **200** again.
- Log-surface note: the `logging` runtime dedupes identical messages (`tenant|category|message`) inside
  `dedupe_window_ms` (default 10 s), so rapid repeated `route bundle hot-swapped` lines do not all surface in
  captured stdout — the swap is evidenced by served behavior and the artifact sha, not by line counts.

---

## 7. Verdict delta

| Item | Before | Now |
|---|---|---|
| No-rebuild proof | NOT RUN (demo DB probe failed) | **PASS** (config + flags + route bundle swapped live; PID+SHA unchanged; pin-write closed — `graph`/`route:cache`/`optimize` run, and a real rewrite hot-swaps the running demo) |
| Outbox crash recovery | NOT RUN | **PASS** (published 0→1 across a restart) |
| Queue restart recovery | NOT VERIFIED | **graceful PASS / hard-crash FAIL** (F1 gap; repro test added) |
| MySQL server crash recovery | NOT RUN | **PASS** (InnoDB recovered; queue+outbox intact) |
| PostgreSQL path | "verified" | **BLOCKED** by host-level `0xC0000142`; prior greens may have been silent skips |

Overall: **CONDITIONALLY READY** — the no-rebuild and outbox-durability claims are now evidence-backed; the
hard-crash queue-recovery gap (F1) was a genuine production-safety defect — **fixed and live-proven in §8**
(F5 in §8.3).

---

## 8. F1 fix landed (lease reclaim + fencing), live-proven

Status: implemented, tested and proven at process level on branch `p0-runtime-safety`.

### 8.1 What changed

- **`kelson_jobs`** — `DEFAULT_CLAIM_LEASE_SECS = 900` and a fenced claim-completion API, added as trait
  *defaults* so non-durable stores keep working: `JobStore::complete_claim(id, attempt)` /
  `fail_claim(id, attempt, error)`, `TenantJobStore::complete_claim_for` / `fail_claim_for`, and
  `JobStoreAdmin::defer_claim` / `kill_claim`. `attempt` is the claim epoch: it advances on every
  claim/reclaim, so a stale holder is refused.
- **Both durable stores** (`kelson-mysql`, `kelson-postgres`) — `claim`/`claim_for` now have a second
  candidate class: `status = 'running' AND attempt < max_attempts AND claimed_at < now - lease`
  (the SKIP LOCKED / candidate-lock structure is unchanged, so single-claim still holds), plus a
  **dead-letter sweep** that turns an abandoned claim with no retries left into `dead` with
  `last_error = 'claim lease expired; retries exhausted'` instead of leaving it `running` forever.
- **Fence on every terminal write** — a worker whose claim was reclaimed can no longer complete, fail,
  defer or kill the job: those writes require `AND attempt = <epoch> AND claimed_by = <worker>` and return
  `KSN_CONFLICT` otherwise. The token carries the **owner** as well as the epoch because `defer` hands an
  attempt back (admission `Hold` must not burn the retry budget), so a deferred job that is re-claimed later
  can carry the same epoch as the abandoned claim — an epoch-only fence would let the deferred worker finish
  its successor's run. Falsified: with the owner predicate removed the deferred worker's stale completion
  *succeeds* and `mysql_job_store_fences_a_deferred_job_reclaimed_at_the_same_epoch` fails on exactly that
  assertion.
- **`kelson-worker`** — `DispatchCtx::once` captures the epoch at claim time and routes all four terminal
  transitions through the fenced calls.
- **F2, made explicit** — the MySQL job store now writes/compares `UTC_TIMESTAMP(6)` rather than `NOW(6)`,
  so correctness no longer rests on sqlx's implicit session default (`time_zone='+00:00'`).

One implementation bug was caught by the new tests and fixed before landing: the MySQL dead-letter sweep was
originally written *inside* the claim transaction, which is rolled back whenever no candidate is found — the
sweep was therefore undone (test evidence: exhausted orphan stayed `running`). It now runs in autocommit
*ahead* of the claim transaction. PostgreSQL does not have the hazard: the sweep is a leading
data-modifying CTE of the same statement, which always commits.

### 8.2 Evidence

| Check | Result | Evidence |
|---|---|---|
| `kelson-jobs` unit tests | **PASS** | 35 passed, 0 failed |
| MySQL integration matrix (serial, live DB) | **PASS** | 15 passed / 0 failed / 0 ignored, 2.01 s, no skip lines |
| Reclaim + fence acceptance | **PASS** | `mysql_job_store_recovers_orphaned_running_job_after_worker_crash`: an in-lease claim is **not** stolen; an expired one is reclaimed with `attempt` 1→2 and the owner moved to worker B; worker A's `complete`/`fail`/`defer`/`kill` at (epoch 1, worker A) are all refused with `KSN_CONFLICT`; worker B's `complete` at (epoch 2, worker B) succeeds |
| Defer/epoch-collision fence | **PASS** | `mysql_job_store_fences_a_deferred_job_reclaimed_at_the_same_epoch`: worker A defers (attempt given back), worker B re-claims at the *same* epoch 1, A's completion is refused while B's succeeds. Re-run with the owner predicate stripped: the stale completion is **accepted** and the test fails — the gate is real |
| Poison-job bound | **PASS** | `mysql_job_store_dead_letters_an_exhausted_orphan_claim`: `max_attempts = 1` + expired claim → not reclaimable, row becomes `dead` with the lease-expired error |
| **Live process-level reclaim** | **PASS** | Planted `running` / `attempt 1` / `claimed_by='kelson-worker-DEAD-9999'` / claim 1 h stale → real worker `kelson-worker-8072` booted `degraded:false`, logged `job succeeded {"job":"demo.job","job_id":"eeeeeeee-0000-0000-0000-000000000001","ms":3}`, stopped `{"jobs":1}`; row after: `succeeded` / **`attempt 2`** / `claimed_by='kelson-worker-8072'`. Before the fix the same plant produced `jobs:0` and the row stayed `running` forever |
| PostgreSQL twin | **compiled, skipped here** | `pg_job_store_recovers_orphaned_running_job_after_worker_crash` mirrors the acceptance; PostgreSQL is unusable on this host (see §0) |

### 8.3 Finding F5 — the tenant-scoped job queue was unusable on MySQL; **fixed**

`jobs_queue.tenant_id` was declared `CHAR(36)` (`database/migrations-mysql/20261005000001_jobs_queue_tenant.sql`)
while **both** data paths treat it as a 16-byte UUID: `enqueue` binds `*tenant.as_uuid()` (raw bytes) and
`job_from_row` decodes `Option<Uuid>`. Every tenant-scoped enqueue therefore failed server-side:

```
1366 (HY000): Incorrect string value: '\xA1O\xB1\xFA\xF1w...' for column 'tenant_id' at row 1
```

Only the unscoped (tenant-NULL) path worked — which is exactly why the rest of the matrix was green, and it meant
the tenancy filter the reclaim arm carries could not be exercised on MySQL at all.

**Fix applied:** `database/migrations-mysql/20261005000004_jobs_queue_tenant_binary.sql`
(`ALTER TABLE jobs_queue MODIFY COLUMN tenant_id BINARY(16) NULL;`) — the form the store's own binds and
`job_from_row` already assume, the `outbox` table's form (`tenant_id binary(16)`), and the only UUID form in the
twin (`uuid: BINARY(16) (sqlx MySQL Uuid codec is binary)`). No row can hold a value (the write always failed),
so the conversion is lossless; the server rebuilds `ix_jobs_queue_tenant`. PostgreSQL needs no counterpart — its
twin-arch migration declares the column `uuid`, which is that codec's native form. `TWIN_ONLY` in
`crates/data/mysql/tests/migration_set_parity.rs` was extended to declare the fourth twin-only entry.

Evidence: live column is `binary(16)` and `20261005000004` is recorded in `_sqlx_migrations`;
`mysql_job_store_tenant_scoped_queue_is_usable_and_isolated` is **un-ignored and passing** (tenant enqueue works,
a foreign tenant sees neither the pending row nor the expired claim, a foreign `complete_claim_for` is
`KSN_NOT_FOUND` rather than a conflict — no existence oracle — a stale token is `KSN_CONFLICT`, and the current
owner completes); MySQL matrix is **15 passed / 0 failed / 0 ignored, no skips**. A **fresh-database bootstrap**
(a throwaway schema, worker boot) applied all **13** migrations and left the column `binary(16)` with
`degraded:false`.

Two operational traps found while landing it, worth knowing for every future migration:

1. **A new migration file needs a rebuild of every binary that embeds the set.** `sqlx::migrate!` is
   compile-time, and Cargo cannot see a *file that did not exist* at the last compile as an input, so the
   incremental build kept the old 12-file set: the first fresh-schema boot applied 12 migrations and left
   `CHAR(36)`. `cargo clean -p kelson-mysql` (or a dependency-aware rebuild trigger) is required. This is the
   one exception to "config/route changes need no rebuild": schema changes do.
2. **An applied migration cannot be edited in place.** sqlx stores a checksum per applied version, so fixing
   `20261005000001` directly would have failed every existing database with "previously applied but has been
   modified". The correction therefore ships as a new version.


### 8.4 Refinement of the skip hazard (§0)

The MySQL skip behaviour is real but the PostgreSQL half is worse than first reported: **without `--nocapture`
the skip line is invisible**. `eprintln!` output from a *passing* test is captured and discarded by the test
harness, so a full run against an unreachable database reported `10 pg_* tests ok, 0 failed` with **zero** skip
text in the captured log; only the single test run with `--nocapture` showed
`pg-tests: cannot connect ([DB_ERROR] a database error occurred); skipping`. A green PG matrix is therefore not
evidence of anything unless the run shows `--nocapture` output or the DB is independently confirmed reachable.
