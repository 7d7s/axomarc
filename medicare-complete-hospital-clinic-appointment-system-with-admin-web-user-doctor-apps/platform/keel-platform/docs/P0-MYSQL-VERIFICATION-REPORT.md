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

- **3b limitation (BLOCKED, separate cause):** `kelson route:cache` (the pin-write path) refuses on this
  workspace: `GRAPH-DUP-OP: operation oauth2.token declared by oauth2 and oauth2`,
  `GRAPH-DUP-ROUTE: route GET /oauth/authorize declared by oauth2 and oauth2`, …. The artifact-probe + swap
  path itself is proven by the clear/rebuild direction above; only the pin-write is gated.

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

Reproduction test added (currently `#[ignore]` so the suite stays green), the acceptance gate for the fix:
`mysql_job_store_recovers_orphaned_running_job_after_worker_crash`
(`crates/data/postgres/tests/pg_integration.rs`). Run with `-- --ignored`; it fails today:
`left: None, right: Some(01a14f7b-…)`.

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

### F1 — Queue reclaim gap (the FAIL above)
Fix direction (needs approval): give `claim` a lease arm — also select `running` rows whose `claimed_at` is older
than a lease (e.g. `job.timeout_secs`) and whose `claimed_by` != a live worker, or add a periodic sweep that
requeues stale `running` rows; plus an operator `queue:recover`. Must preserve the existing SKIP LOCKED
single-claim guarantee.

### F2 — MySQL timestamp handling mixes UTC parameters with server-local `NOW()`
`enqueue` binds the app's `OffsetDateTime` (UTC) for `available_at` (`jobstore.rs:156`), but retry backoff and
requeue write `available_at = NOW(6)` / `NOW(6) + INTERVAL …` (`jobstore.rs:290`, `…:356`), and `claim` compares
against `NOW(6)` (`jobstore.rs:202`). `DATETIME` carries no timezone.

Measured on this host: the worker's session `NOW(6)` evaluates in **UTC** while server-local is **IST (+5:30)**.

- A row with `available_at = UTC_TIMESTAMP(6) + INTERVAL 1 MINUTE` was **not** claimed.
- The same rows with `available_at = NOW(6) - INTERVAL 1 DAY` **were** claimed and succeeded.

So timestamps written via SQL `NOW()` are offset from the UTC values the app binds, and a retry/requeue can be
delayed by the server's UTC offset. Laravel addresses this with a connection `timezone` setting. Fix direction:
pin the connection to UTC (`MySqlConnectOptions::timezone`) **or** use `UTC_TIMESTAMP(6)` consistently in the
job-store SQL (and audit the other MySQL stores for the same pattern).

### F3 — `kelson route:cache` refuses on this workspace
Duplicate declarations surface as `GRAPH-DUP-OP`/`GRAPH-DUP-ROUTE` for the `oauth2` module, so the route-cache
pin is never written. This blocks only the pin-write direction of the no-rebuild proof; the graph gate correctly
refuses a non-deterministic graph (fail-closed behaviour is itself correct).

---

## 7. Verdict delta

| Item | Before | Now |
|---|---|---|
| No-rebuild proof | NOT RUN (demo DB probe failed) | **PASS** (config + flags + route bundle swapped live; PID+SHA unchanged). `route:cache` pin-write **BLOCKED** by F3 |
| Outbox crash recovery | NOT RUN | **PASS** (published 0→1 across a restart) |
| Queue restart recovery | NOT VERIFIED | **graceful PASS / hard-crash FAIL** (F1 gap; repro test added) |
| MySQL server crash recovery | NOT RUN | **PASS** (InnoDB recovered; queue+outbox intact) |
| PostgreSQL path | "verified" | **BLOCKED** by host-level `0xC0000142`; prior greens may have been silent skips |

Overall: **CONDITIONALLY READY** — the no-rebuild and outbox-durability claims are now evidence-backed; the
hard-crash queue-recovery gap (F1) is a genuine production-safety defect and is the next fix.
