# 13 — Operations

Runbooks for the day-two surface.

## Health

`runtime/keel-api` exposes `healthz`/`readyz`. Degraded boot is supported by design: the platform
comes up on in-memory ports when postgres is absent, with readiness reflecting the degraded state.

Probe semantics (tested in `runtime/keel-api/tests/http_integration.rs`):

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
class recorded by the edge middleware, plus `keel_uptime_seconds`, `keel_degraded` and
`keel_routes_declared` gauges. The endpoint is feature-flagged by `app.metrics` (disabled by
default); while the flag is off it answers a 404 envelope, so enabling it is an explicit act.

When a durable queue store is wired (the pool-gated `PgJobStore` in `serve()`), every scrape
also carries the queue census: `keel_queue_depth{status}` (pending / running / dead /
succeeded), `keel_queue_oldest_age_seconds` (age of the oldest pending job; 0 when none) and
`keel_queue_finished{outcome}` (succeeded / dead — the cumulative throughput census, since
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

`keel observe status|logs|errors|traces|request|audit|incidents|exporters|policies|buffers|health|cost|metrics`
operates on these stores without a live host (see docs/12-CLI.md §Telemetry).

### Incident runbook

1. **Start**: `keel observe incident start --operator <name>` (or `POST /admin/telemetry/incident`
   with a session principal holding `telemetry.incident.manage`). Hosts pick the file up within 2 s.
2. **Effect**: error/span classes escalate to 100% sampling; events are tagged with `incident_id`;
   lifecycle events mark the transition. Deeper capture can be requested via flags in the control file.
3. **Watch**: `keel observe incidents` (active + history), `keel observe health` (exporters + WAL +
   incident rollup), `keel observe errors --last n` (fingerprint-grouped exceptions).
4. **Reconstruct**: `keel observe request <id>` joins the execution trace, exported events and
   context into one view; `keel observe traces --trace <id>` walks the span tree.
5. **Stop**: `keel observe incident stop --operator <name>`. The incident moves to history; sampling
   returns to the adaptive baseline. CLI, HTTP and pollers all read/write the same file, so no host
   needs a restart.

### Exporter failure triage

`keel observe exporters` shows per-exporter state (CLOSED/OPEN/HALF_OPEN), attempts, failures,
exported events/bytes and the last error. An OPEN breaker retries after its cooldown under the
global retry budget (default 20 retries/s); ENOSPC on the file exporter trips its breaker and
reroutes per policy. `keel observe buffers` reports WAL segments/bytes/watermark — a watermark that
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
keel inspect execution <id>       # one execution, end to end
keel inspect delta                # delta stones written
keel inspect authorization        # who was denied/admitted and why
keel inspect n-plus-one           # repeated-query scan over recent traces
keel replay <id> --dry-run        # what a replay would do, without executing
keel replay <id>                  # re-execute under the recorded context
```

Recording is a dev/test concern; production keeps the trace recorder off unless investigating.

## Queue triage

```bash
keel queue:monitor                    # depth + oldest pending age (start here)
keel queue:failed            # dead jobs awaiting human decision
keel queue:retry <job-id>    # requeue after fixing the cause
keel queue:forget <job-id>   # drop one dead job (Wave 5)
keel queue:prune-failed --hours 168    # age-bounded dead-letter pruning (Wave 5)
keel job:list --status running --limit 50
keel queue:restart            # stop workers after their current job (Wave 5)
```

Wave 5 added the admin surface on top of the Wave 2 dead-letter flow: `queue:monitor` for the
depth/age snapshot, `queue:clear`/`queue:flush` for bulk purges (clear refuses prod/staging and
prompts unless `KEEL_FORCE=1`), `queue:listen` to supervise the worker, and `queue:prune-*` for
age-bounded cleanup. Two honest limits: `queue:prune-batches` reports an honest empty run and
`queue:retry-batch <id>` fails closed — the batch ledger is in-memory only until B-32 lands, so
requeue batch members individually with `queue:retry`.

## Benchmark gate (G15)

`keel benchmark` measures framework phases against `benchmarks/baseline.json` and rejects
regressions beyond the budget (default 10% per phase): routing, authorization, delta-write, read, serialization, startup, memory — and `telemetry-emit` (Sentinel S5: ingest overhead of the bounded telemetry bus against a live runtime with a file exporter + WAL). Promote new numbers deliberately with
`--save-baseline`. A phase without a baseline entry is reported but does not gate until it is baselined.

## Profiling (B-27)

CPU/alloc flamegraphs for the two long-running runtimes, out-of-process: samply attaches
to a child process, so nothing joins the workspace dependency budget and nothing runs
when you do not ask for it.

```bash
cargo install samply
keel profile api        # cargo build -p keel-api, then samply record target/debug/keel-api
keel profile worker     # the same for keel-worker
```

Each subcommand builds its package first so the recording captures a fresh binary;
stopping the recording opens the flamegraph in the browser. If samply is not on PATH the
wrapper fails closed with the install line and this workflow — it never silently skips.
Manual equivalent without the wrapper: `cargo build -p keel-api` then
`samply record target/debug/keel-api` from the workspace root (set `KEEL_CONFIG` when
recording against a non-default config). Flamegraphs are a local diagnostic; production
metrics stay on `/metrics` (D-68 scope).

## Security posture

```bash
keel security     # secrets hygiene + dependency policy + trace locality
keel doctor       # environment sanity (run everywhere first)
```

## Version upgrades

`keel upgrade` reports: current vs target versions, deprecated APIs (from the API-ID registry),
config keys that moved, pending migrations, and dependency deltas — before you run a release.

## Graceful shutdown

Both runtimes treat `SIGINT`/`Ctrl-C` (`tokio::signal::ctrl_c`) as the shutdown trigger:

- **keel-api** serves through `axum::serve(...).with_graceful_shutdown(...)`: the listener stops
  accepting new connections, in-flight requests run to completion, then the process exits. The
  shutdown signal is logged (`shutdown signal received`) before draining begins.
- **keel-worker** flips an atomic stop flag. The claim loop checks the flag **between** jobs, so an
  in-flight job always finishes and acks before exit; the scheduler tick and outbox pump tasks
  observe the same flag. After the loop exits the worker performs one final outbox drain, aborts
  the background tasks, logs the processed count (`keel-worker stopped`), and exits. Bounded runs
  (`--once`, `--max-jobs`) follow the same final-drain path.

There is no forced-kill path in either runtime: shutdown is cooperative and bounded by the longest
in-flight unit of work (one HTTP request, one job).

## Deployment cache semantics

Laravel’s `route:cache` / `config:cache` exist because PHP re-parses routing and configuration on
every request. Keel’s runtime model keeps both off the request path, and Wave 1 adds an optional,
hash-pinned route artifact for deployments:

- **Routing.** Module manifests (`applications/*/module.toml`) are compiled into the
  `PlatformGraph` once at boot — the same compile path used by `keel check`, `keel graph` and the
  architecture gates. The graph is then registered into the axum router in one pass. Per-request
  dispatch reads the compiled router only; no manifest parsing ever happens on the request path.
  Since Wave 1, `keel route:cache` additionally writes `storage/framework/routes.json` — every
  route’s resolved dispatch plan (middleware chain, bindings, validators, flags) pinned to a
  SHA-256 content hash over all `module.toml` manifests + `middleware.toml` + `MODULE_ABI`.
  Boot semantics are strictly stricter than Laravel: the graph always recompiles; an absent
  artifact falls back to live compilation; a present artifact with a matching hash registers
  routes from the plan list; a stale or corrupt artifact refuses boot with a pointer to re-run
  `keel route:cache` (no silent live fallback for drift). `keel route:clear` deletes the
  artifact. The artifact is a deployment pin, never a correctness dependency — the live compile
  path remains the source of truth.
- **Configuration.** Configuration is read from the environment (`KEEL_CONFIG` file plus `KEEL_*`
  variables) during the Config lifecycle phase at boot. There is no runtime config-cache file to
  warm or clear; a configuration change is deployed by restarting the process with the new
  environment, which is the intended 12-factor behaviour.
- **Secrets across restarts.** `KEEL_SESSION_SECRET` must be set to a stable value for sessions to
  survive restarts and to be shared across instances; when unset, each boot derives a fresh random
  secret (acceptable for the in-memory session store, which never survives a restart anyway).
- **Release warm-up (Wave 5).** `keel optimize` warms all three boot-pinning artifacts in order
  (route:cache → config:cache → view:cache) and aborts at the first refusal;
  `keel optimize:clear` drops all three, attempting every step so a failed delete never strands one
  artifact behind. Both are thin orchestrations over the same commands above — the artifacts remain
  deployment pins, never correctness dependencies.
- **If boot-time compilation ever became a measurable cost** (it is bounded by the manifest count
  and verified by the benchmark gate), a serialized graph snapshot would be a startup optimization
  only — never a correctness dependency — because the live compile path remains the source of
  truth.