# 13 — Operations

Runbooks for the day-two surface.

## Health

`runtime/kelson-api` exposes `healthz`/`readyz`. Degraded boot is supported by design: the platform
comes up on in-memory ports when postgres is absent, with readiness reflecting the degraded state.

Probe semantics (tested in `runtime/kelson-api/tests/http_integration.rs`):

- `GET /healthz` is the **liveness** probe. It answers 200 with an `ok:true` envelope even when
  degraded, so orchestrators do not restart-loop an instance that is knowingly serving reduced
  capacity. The envelope carries `degraded` and the reason for dashboards.
- `GET /readyz` is the **readiness** probe. It answers 200 when the instance is fully capable and
  503 with a `BOOT` envelope (reason included) when degraded, so load balancers stop routing to it.
- Both probes bypass the admission chain's request-id stage (`ADMISSION_BYPASS`) so synthetic
  probe traffic does not pollute request metrics beyond the explicit scrape counters.

## Metrics and dashboard

`GET /metrics` serves a Prometheus text-format (0.0.4) scrape from the shared `MetricsRegistry`
(`crates/observability/metrics`): HTTP request counts and latency histograms per method/status
class recorded by the edge middleware, plus `kelson_uptime_seconds`, `kelson_degraded` and
`kelson_routes_declared` gauges. The endpoint is feature-flagged by `app.metrics` (disabled by
default); while the flag is off it answers a 404 envelope, so enabling it is an explicit act.

When a durable queue store is wired (the pool-gated `PgJobStore` in `serve()`), every scrape
also carries the queue census: `kelson_queue_depth{status}` (pending / running / dead /
succeeded), `kelson_queue_oldest_age_seconds` (age of the oldest pending job; 0 when none) and
`kelson_queue_finished{outcome}` (succeeded / dead — the cumulative throughput census, since
worker-side finishes land in the same store the API snapshots). The gauges are read at scrape
time, not incremented: without a durable store (no pool, test builder) the series stays
**absent** rather than reporting a lying zero, and a prune/clear can lower a census — they
observe the store, they are not event counters (test:
`metrics_expose_queue_depth_age_and_throughput`).

`GET /_dashboard` serves a Horizon-lite operations page (health, readiness, boot metadata, flags,
declared routes, metrics) that polls the JSON/meta endpoints every 5s. It is on by default in
dev/test and gated by the `app.dashboard` flag elsewhere.

## Telemetry runtime and incident mode (S5 Sentinel)

Both hosts run one `ObservabilityRuntime` (specs/STAGE-SENTINEL-SPEC.md): a bounded event bus,
processing pipeline (classify → redact → fingerprint → sample → dedupe → route), durability-classed
buffers with a WAL, circuit-broken exporters (console/file/OTLP-JSON/HTTP/syslog), a resource
monitor with adaptive sampling, and deterministic anomaly detectors. Telemetry never blocks or
crashes the application: overflow is dropped by policy and counted (`telemetry_dropped`), exporter
failures trip breakers, and a stderr fallback covers the case where no runtime is available.

Local stores (all under `storage/framework/telemetry/`, never leaving the environment):

- `logs/app.jsonl` — exported event envelopes (JSONL; per-tenant partitioning optional)
- `wal/` — durable-event write-ahead log (`segment-<LSN>.wal`, `watermark.json`)
- `control/incident.json` — incident control plane (source of truth)
- `exporters/<name>.json` — exporter health snapshots
- `cost.json` — cost rollup (events/bytes by source and exporter)

`kelson observe status|logs|errors|traces|request|audit|incidents|exporters|policies|buffers|health|cost|metrics`
operates on these stores without a live host (see docs/12-CLI.md §Telemetry).

### Incident runbook

1. **Start**: `kelson observe incident start --operator <name>` (or `POST /admin/telemetry/incident`
   with a session principal holding `telemetry.incident.manage`). Hosts pick the file up within 2 s.
2. **Effect**: error/span classes escalate to 100% sampling; events are tagged with `incident_id`;
   lifecycle events mark the transition. Deeper capture can be requested via flags in the control file.
3. **Watch**: `kelson observe incidents` (active + history), `kelson observe health` (exporters + WAL +
   incident rollup), `kelson observe errors --last n` (fingerprint-grouped exceptions).
4. **Reconstruct**: `kelson observe request <id>` joins the execution trace, exported events and
   context into one view; `kelson observe traces --trace <id>` walks the span tree.
5. **Stop**: `kelson observe incident stop --operator <name>`. The incident moves to history; sampling
   returns to the adaptive baseline. CLI, HTTP and pollers all read/write the same file, so no host
   needs a restart.

### Exporter failure triage

`kelson observe exporters` shows per-exporter state (CLOSED/OPEN/HALF_OPEN), attempts, failures,
exported events/bytes and the last error. An OPEN breaker retries after its cooldown under the
global retry budget (default 20 retries/s); ENOSPC on the file exporter trips its breaker and
reroutes per policy. `kelson observe buffers` reports WAL segments/bytes/watermark — a watermark that
stops advancing while segments grow means exports are not completing.

### Shutdown and degraded boot

Hosts drain telemetry on exit (`runtime.shutdown(5s)` after serve/loop exit, even on error paths):
buffers flush, the WAL watermark advances, exporters close inside the deadline. Degraded boot
(no database) still initializes telemetry on file/in-memory backends (G10). The WAL guarantees
durability to the last fsync'd record; the SIGKILL gap is documented in STAGE-SENTINEL-SPEC §20.
## Diagnostics chain (S4)

Every execution can persist a trace (input, plan, authorization decision, duration, queries,
delta/audit/outbox counts). Traces are sensitive: they live under `storage/framework/traces` and
never leave the environment.

```bash
kelson inspect execution <id>       # one execution, end to end
kelson inspect delta                # delta stones written
kelson inspect authorization        # who was denied/admitted and why
kelson inspect n-plus-one           # repeated-query scan over recent traces
kelson replay <id> --dry-run        # what a replay would do, without executing
kelson replay <id>                  # re-execute under the recorded context
```

Recording is a dev/test concern; production keeps the trace recorder off unless investigating.

## Queue triage

```bash
kelson queue:monitor                    # depth + oldest pending age (start here)
kelson queue:failed            # dead jobs awaiting human decision
kelson queue:retry <job-id>    # requeue after fixing the cause
kelson queue:forget <job-id>   # drop one dead job (Wave 5)
kelson queue:prune-failed --hours 168    # age-bounded dead-letter pruning (Wave 5)
kelson job:list --status running --limit 50
kelson queue:restart            # stop workers after their current job (Wave 5)
```

Wave 5 added the admin surface on top of the Wave 2 dead-letter flow: `queue:monitor` for the
depth/age snapshot, `queue:clear`/`queue:flush` for bulk purges (clear refuses prod/staging and
prompts unless `KEEL_FORCE=1`), `queue:listen` to supervise the worker, and `queue:prune-*` for
age-bounded cleanup. Wave 6 P9 closed the two batch limits (B-32): `queue:prune-batches`
deletes finished batch records older than the cutoff against the durable `job_batches` ledger
(never unfinished ones) and `queue:retry-batch <id>` requeues every failed/dead member through
the same `BatchOps::retry` the HTTP `POST /api/v1/batches/{id}/retry` runs.

## Benchmark gate (G15)

`kelson benchmark` measures framework phases against `benchmarks/baseline.json` and rejects
regressions beyond the budget (default 10% per phase): routing, authorization, delta-write, read, serialization, startup, memory — and `telemetry-emit` (Sentinel S5: ingest overhead of the bounded telemetry bus against a live runtime with a file exporter + WAL). Promote new numbers deliberately with
`--save-baseline`. A phase without a baseline entry is reported but does not gate until it is baselined.

## Profiling (B-27)

CPU/alloc flamegraphs for the two long-running runtimes, out-of-process: samply attaches
to a child process, so nothing joins the workspace dependency budget and nothing runs
when you do not ask for it.

```bash
cargo install samply
kelson profile api        # cargo build -p kelson-api, then samply record target/debug/kelson-api
kelson profile worker     # the same for kelson-worker
```

Each subcommand builds its package first so the recording captures a fresh binary;
stopping the recording opens the flamegraph in the browser. If samply is not on PATH the
wrapper fails closed with the install line and this workflow — it never silently skips.
Manual equivalent without the wrapper: `cargo build -p kelson-api` then
`samply record target/debug/kelson-api` from the workspace root (set `KEEL_CONFIG` when
recording against a non-default config). Flamegraphs are a local diagnostic; production
metrics stay on `/metrics` (D-68 scope).

## Security posture

```bash
kelson security     # secrets hygiene + dependency policy + trace locality
kelson doctor       # environment sanity (run everywhere first)
```

## Version upgrades

`kelson upgrade` reports: current vs target versions, deprecated APIs (from the API-ID registry),
config keys that moved, pending migrations, and dependency deltas — before you run a release.

## Graceful shutdown

Both runtimes treat `SIGINT`/`Ctrl-C` (`tokio::signal::ctrl_c`) as the shutdown trigger:

- **kelson-api** serves through `axum::serve(...).with_graceful_shutdown(...)`: the listener stops
  accepting new connections, in-flight requests run to completion, then the process exits. The
  shutdown signal is logged (`shutdown signal received`) before draining begins.
- **kelson-worker** flips an atomic stop flag. The claim loop checks the flag **between** jobs, so an
  in-flight job always finishes and acks before exit; the scheduler tick and outbox pump tasks
  observe the same flag. After the loop exits the worker performs one final outbox drain, aborts
  the background tasks, logs the processed count (`kelson-worker stopped`), and exits. Bounded runs
  (`--once`, `--max-jobs`) follow the same final-drain path.

There is no forced-kill path in either runtime: shutdown is cooperative and bounded by the longest
in-flight unit of work (one HTTP request, one job).

## Deployment cache semantics

Laravel’s `route:cache` / `config:cache` exist because PHP re-parses routing and configuration on
every request. Kelson’s runtime model keeps both off the request path, and Wave 1 adds an optional,
hash-pinned route artifact for deployments:

- **Routing.** Module manifests (`applications/*/module.toml` plus every package indexed by the
  root `packages.toml`) are compiled into the
  `PlatformGraph` once at boot — the same compile path used by `kelson check`, `kelson graph` and the
  architecture gates. The graph is then registered into the axum router in one pass. Per-request
  dispatch reads the compiled router only; no manifest parsing ever happens on the request path.
  Since Wave 1, `kelson route:cache` additionally writes `storage/framework/routes.json` — every
  route’s resolved dispatch plan (middleware chain, bindings, validators, flags) pinned to a
  SHA-256 content hash over all `module.toml` manifests + `packages.toml` + indexed package
  manifests + `middleware.toml` + `MODULE_ABI`.
  Boot semantics are strictly stricter than Laravel: the graph always recompiles; an absent
  artifact falls back to live compilation; a present artifact with a matching hash registers
  routes from the plan list; a stale or corrupt artifact refuses boot with a pointer to re-run
  `kelson route:cache` (no silent live fallback for drift). `kelson route:clear` deletes the
  artifact. The artifact is a deployment pin, never a correctness dependency — the live compile
  path remains the source of truth.
- **Configuration.** Configuration is read from the environment (`KEEL_CONFIG` file plus `KEEL_*`
  variables) during the Config lifecycle phase at boot. There is no runtime config-cache file to
  warm or clear; a configuration change is deployed by restarting the process with the new
  environment, which is the intended 12-factor behaviour.
- **Secrets across restarts.** `KEEL_SESSION_SECRET` must be set to a stable value for sessions to
  survive restarts and to be shared across instances; when unset, each boot derives a fresh random
  secret (acceptable for the in-memory session store, which never survives a restart anyway).
- **Release warm-up (Wave 5).** `kelson optimize` warms all three boot-pinning artifacts in order
  (route:cache → config:cache → view:cache) and aborts at the first refusal;
  `kelson optimize:clear` drops all three, attempting every step so a failed delete never strands one
  artifact behind. Both are thin orchestrations over the same commands above — the artifacts remain
  deployment pins, never correctness dependencies.
- **If boot-time compilation ever became a measurable cost** (it is bounded by the manifest count
  and verified by the benchmark gate), a serialized graph snapshot would be a startup optimization
  only — never a correctness dependency — because the live compile path remains the source of
  truth.

---

# Runbooks

> **Known doc-location gap (Phase 5, recorded not fixed).** `kelson-console` builds paths from
> `workspace_root()` (`tools/kelson-console/src/main.rs:553`), which walks up to the first
> `[workspace]` `Cargo.toml`. Run from `platform/keel-platform/`, that root is `keel-platform/`
> itself — so the `docs/13-OPERATIONS.md` pointer emitted by `kelson profile` and by the
> `kelson benchmark` report resolves to `platform/keel-platform/docs/13-OPERATIONS.md`, **which does
> not exist**. The framework docs live here in `platform/docs/`. The Phase 2 relocation carried
> source but not the framework docs (nor `benchmarks/baseline.json`, see §Performance baseline
> below). Fixing it means either carrying this doc set into `keel-platform/docs/` or re-pointing
> the CLI; carrying was declined in favour of editing these files in place, so the gap stands as a
> known operator-facing rough edge.

## Runbook: config reload and rollback

**What reload does.** `ConfigRuntime` keeps an immutable, content-addressed revision
(SHA-256 over the canonical effective config) plus a history ring of the last `HISTORY_MAX`
revisions. A reload loads, schema-gates, hashes, compares, and — only if the hash differs — swaps
the current revision and notifies subscribers (route cache, feature flags). Equal configs hash
equal, so a *touched but unchanged* file does not bump the version and does not notify.

Triggers: file mtime via `poll()` (a cheap probe — no work when unchanged) and `reload()` (the
`kelson config:reload` CLI path). SIGHUP and etcd/consul are explicit plug points — implement
`ConfigSource`; no watcher dependency ships.

**Pre-flight.**

```powershell
kelson config:reload
```

Schema-gates the file, prints the content hash and the drift state against the pinned
`storage/framework/config.json`, with timing. **Exits non-zero on invalid config** — so it is safe
to wire as a gate in front of the edit reaching a running server.

**Budget.** A reload swaps the revision and the subscribers' route/flag views with **no restart in
well under 50 ms** (§10 "boot-time freeze"), asserted by
`config_reload_swaps_routes_and_flags_under_50ms` (`crates/application/configuration/src/runtime.rs`)
and measured as the `config-reload` phase of `kelson benchmark` (≈ 2.0 ms median over 300 iterations
in the Phase 5 baseline run).

**Limitation you must plan around — rollback is process-local (§5.4).** `ConfigRuntime::rollback`
swaps the *running* revision and the subscribers' views. It **never rewrites the source file**.
Consequences:

- The file on disk still holds the rejected value. A second process reading it sees the old config.
- A multi-instance fleet does **not** converge by running `kelson rollback` on one instance.
- To make a rollback durable: fix the source (or restore it from version control), then let each
  instance's poller converge on the next mtime change. The in-process rollback is for stopping the
  bleeding on one running process while you do that.

**Fail-safe posture.** A half-written or invalid file makes the reload return an error with the
current revision **untouched** — it never partially applies.

**Chaos evidence (Phase 5).** `config_reload_under_load_never_tears_or_regresses_a_revision`
hammers `get()` from four reader threads while four writers reload a mutating file, asserting that
every observed snapshot is content-addressed, that the revision never goes backwards, that
subscribers are notified once per swap in ascending order, and that the last notified revision is
the active one.

> **This test found a real race.** `reload()` drew version numbers with `fetch_add` and then took
> the write lock, so two concurrent reloads could commit in the *reverse* order — regressing the
> active revision to an older payload (a lost update) and notifying subscribers `[.. 7, 6 ..]`,
> i.e. installing stale routes. Fixed by a `commit` mutex serializing
> load → compare → version bump → swap → history push → notify. Readers deliberately do not take
> that lock, so the reader guarantee above is preserved.

## Runbook: promotion failure

`kelson promote` checkpoints first, then runs four stages, and restores the checkpoint automatically
if any health gate fails. Stage semantics and the target fields are in
`14-DEPLOYMENT.md` §Promotion pipeline.

**Reading the failure.** The error is a `WORKFLOW` failure carrying:

| Field | Meaning |
|---|---|
| `stage` | which gate failed — `post-deploy`, `post-migrate`, `post-traffic`, `canary` |
| `release` | the release id from the plan |
| `rolled_back` | `true` = the restore completed; if absent, the restore itself failed — **stop and escalate** |
| `config_version` | the revision now live after the restore |
| `reverted` | pair-migration batches unwound (named by the batch they took the watermark past) |
| `pins_written` | module pins written back |

**Decide by stage.**

1. **`post-deploy`** — nothing but module pins had moved. The restore pins them back. Diagnose the
   deployment itself (artifact, module ABI), then re-run.
2. **`post-migrate`** — the database moved. `reverted` lists the batches unwound; confirm the
   watermark with `kelson migrate:status` before doing anything else. Do **not** re-run until the
   migration failure is understood.
3. **`post-traffic`** — config was activated and then restored. Confirm the effective hash matches
   the pre-promotion value (`kelson config:reload`).
4. **`canary`** — everything applied and the final gate failed. This is a *behavioral* failure, not
   a wiring one: the system is back at the checkpoint, so investigate the canary signal itself.

**Inspect without acting:**

```powershell
kelson promote --dry-run        # plan + stages that would run
kelson rollback --dry-run       # the checkpoint that would be restored
kelson rollback                 # restore all five dimensions deliberately
```

**Guarantee under failure.** The read-only job and event compatibility guards run **before any
mutation**, so an incompatible target refuses with *nothing* half-applied — you will not find a
config rolled forward with a database left behind.

**Chaos evidence (Phase 5).**
`every_stage_fails_closed_and_restores_the_checkpoint` walks all four stages × both failure modes
(`Ok(false)` and `Err(_)`), asserting per cell that the failing stage is named, every dimension is
byte-identical to the start, work the failed stage had already done is actually undone, no later
stage's probe ever ran, and the restore ran strictly after the failure with the guards first.

## Runbook: tenant isolation incident

**Trigger.** A tenant sees data, names, counters, jobs, schedules or objects it should not — or its
own work silently disappears (suppressed events, stolen locks, vanished schedules).

**Do first:** capture the affected tenant ids, the subsystem, and the exact operation. Then check
whether it is *visibility* (data returned) or *suppression* (its own work not happening) — they
point at different layers.

**Per subsystem.**

| Subsystem | Check | Contain by |
|---|---|---|
| cache | Same logical key resolving across tenants | `invalidate_prefix` with `""` under the affected tenant — wipes that tenant's namespace only (see below) |
| queue | A job claimed, completed or failed under the wrong tenant | Re-queue from the ledger; the scoped APIs refuse foreign ids with `NOT_FOUND` |
| events | An event suppressed as a "duplicate" it never emitted | Inspect the idempotency key: `tenant` is a first-class member of the hash, so a cross-tenant collision means the key formula regressed |
| scheduler | A schedule not firing, or firing into the wrong tenant | Guard locks and last-fired state are `t:{tenant}:{name}`-qualified; check for a name collision (below) |
| storage | An object reachable under the wrong tenant | Namespacing is `tenant/{id}/{key}`; verify with `list` under each tenant |

**Two invariants that must hold — and the defects that broke them (Phase 5):**

1. **`t:{tenant}:` must not be injectable.** It is safe only because `TenantId` is a UUID newtype
   with **no `FromStr`** (`crates/foundation/core/src/id.rs`): UUID text carries no `:`, and every
   UUID is exactly 36 characters, so no namespace can contain another. If anyone adds a
   string-constructed tenant id, an empty-prefix cache wipe becomes a cross-tenant wipe. The cache
   pentest pins this behaviorally.
2. **Schedule uniqueness must be checked on the runtime key.** Registration deduplicates on
   `(name, tenant)` while the runtime keys state on `scoped_name` — a platform schedule named
   literally `t:{uuid}:{name}` would otherwise alias a tenant schedule's lock and last-fired slot.
   *Fixed in Phase 5*: `ScheduleRegistry::register` now rejects a `scoped_name` collision.

**Evidence and full method:** `keel-platform/docs/PENTEST-TENANT-ISOLATION.md` — threat model,
the 15 pre-existing probes, the 7 adversarial tests added in Phase 5, and the composed
cache → queue → events → scheduler escalation test.

**Not covered by this runbook:** the live-database RLS probes
(`pg_modelstore_tenant_isolation_end_to_end` and its MySQL twin) skip when `DATABASE_URL` is
unset. Run them when a database is available:

```powershell
$env:DATABASE_URL = "postgres://user:pass@localhost:5432/keel"
cargo test -p kelson-postgres --features pg-tests
```

## Runbook: manual rollback of forward-only migrations

> `CORRECTED-TARGET-ARCHITECTURE.md` §11 records "foundation migrations remain forward-only with
> manual rollback runbook". That runbook did not exist until this section — `kelson migrate:rollback`
> **explicitly skips** forward-only single-file migrations (it applies only `NNN.up.sql` /
> `NNN.down.sql` pairs), so there was no documented path for the case the spec assumed.

**Scope.** Single-file migrations under `database/migrations/` (and their MySQL mirror under
`database/migrations-mysql/`). Pair migrations are covered by `kelson migrate:rollback` and must
**not** be handled here.

**Procedure.**

1. **Confirm it is forward-only.** `kelson migrate:status` — the file has no `.down.sql` sibling.
2. **Stop writes.** `kelson down --retry 60`, or place the affected service in maintenance mode.
   Concurrent DDL against live traffic is the main way this goes wrong.
3. **Write the inverse by hand.** Additive-first discipline applies (`COMPATIBILITY.md`):
   dropping a column, dropping an index, or dropping a table each need an explicit, reviewed
   statement. Preserve data you may need — a forward-only rollback that destroys data is
   unrecoverable by design.
4. **Take the advisory lock.** Hold `pg_advisory_lock(7259)` for the manual run, exactly as
   `kelson migrate` does, so you cannot interleave with a scheduled migrator. On MySQL use
   `GET_LOCK('kelson:migrate', …)`.
5. **Apply, then verify.** Run the statements, then `kelson migrate:status` and `kelson db:show` to
   confirm row counts and schema shape.
6. **Record it.** Forward-only files are not tracked as reverted, so the bookkeeping table will not
   show this. Note the file, the timestamp, the statements and the operator in the change record,
   otherwise the next `kelson migrate` may re-apply against a schema you changed underneath it.

**Never** hand-edit a foundation migration file in place: migrations are ordered by filename and
G08 enforces ordering, so editing an applied file silently desynchronizes every environment that
already ran it.

## Performance baseline (hot-reload, rollback)

**Measured — hot-reload < 50 ms.** Two independent pieces of evidence:

| Source | Result |
|---|---|
| `config_reload_swaps_routes_and_flags_under_50ms` | asserts elapsed < 50 ms on a real file edit + reload |
| `kelson benchmark` → `config-reload` phase | ≈ 2.0 ms median over 300 iterations (Phase 5 run) |

`kelson benchmark` also reports `config-rollback` (≈ 0.5 µs — process-local in-memory swap, §5.4).
Neither phase is in `benchmarks/baseline.json`, so they are **reported but do not gate** until
someone deliberately runs `kelson benchmark --save-baseline` (G15 policy: a phase without a baseline
entry is reported, not gated).

**Not measurable here — deployment rollback < 30 s.** The `RollbackExecutor` figure needs a live
database; without one the migrations dimension no-ops and the number would be meaningless, and
with one the test would be **destructive** (a real checkpoint drives `pairs::rollback`, unwinding
real batches). That is why it is a procedure and not an automated test — the same posture
`migrate:rollback` has.

**Procedure (use a scratch database, never production):**

```powershell
# 1. disposable database with the schema at the target state
$env:DATABASE_URL = "postgres://user:pass@localhost:5432/keel_rollback_bench"
kelson migrate

# 2. a promotion that lands somewhere we can roll back FROM
kelson promote --dry-run          # confirm plan + stages first
kelson promote                    # writes storage/framework/promotion.checkpoint.json

# 3. time the restore
Measure-Command { kelson rollback }    # budget: < 30 s
```

Judge the result against 30 s and record the machine, the migration count and the batch count with
it — the number is dominated by how many pair batches `pairs::rollback` has to unwind, so a figure
without those is not comparable to the next run.

**Finding — the G15 baseline is inert in `keel-platform`.** `benchmarks/baseline.json` exists only
at `platform/benchmarks/`; it was never carried into the workspace during the Phase 2 relocation,
so `kelson benchmark` run from `keel-platform` reports every phase as `-` and gates nothing.

For reference, the original file (captured 2026-08-22 against pre-Phase-2 code) compared against
the current workspace in the Phase 5 run:

| Phase | Baseline | Current | Delta |
|---|---|---|---|
| authorization | 3.0 µs | 1.1 µs | −63.3% |
| delta-write | 46.6 µs | 24.0 µs | −48.5% |
| serialization | 144.7 µs | 59.1 µs | −59.2% |
| read | 10.5 µs | 8.6 µs | −18.1% |
| routing | 0.3 µs | 0.2 µs | −33.3% |
| memory | 307783 B | 307783 B | ±0.0% |
| **startup** | 1363.4 µs | 1608.2 µs | **+18.0%** |

Restoring that file makes `kelson benchmark` **exit 1** on `startup` (+18.0% against a 10% budget).
Six phases reading 18–63% *faster* while memory is byte-identical indicates the old baseline is not
a fair reference for the post-relocation code, so the file was left **absent** (status quo) rather
than restored or re-baselined. Establishing the reference for `keel-platform` is an operator
decision: run `kelson benchmark --save-baseline` there once the current numbers are accepted.