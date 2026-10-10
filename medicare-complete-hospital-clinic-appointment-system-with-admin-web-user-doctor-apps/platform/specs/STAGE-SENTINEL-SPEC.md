# Kelson Stage Sentinel (S5) Specification — Production Observability Runtime (FROZEN — D-73)

> Status: FROZEN 2026-08-24. Inserted into the roadmap by D-73 (STAGE-0-SPEC §14/§15):
> S4 developer platform → **S5 Sentinel** → S6 AI/MCP → S7 public ecosystem.
> Rescopes the D-68 metrics deferral into this stage and closes gap-analysis item B-17
> (Prometheus-compatible `/metrics`). Normative for the observability capability (G17 facets).

## 1. Scope and non-goals

- Scope is `platform/` only; medicare-kernel stays untouched (D-70).
- Sentinel evolves `crates/observability/logging` (S1: `init_logging`, redaction, chain records)
  into an application-owned telemetry runtime: one runtime per host, every framework subsystem
  emits through it, and **telemetry failure never crashes the application**.
- Non-goals: ML-based anomaly detection (detectors are deterministic thresholds), OTLP
  protobuf encoding (D-75 ships OTLP/HTTP + JSON), any public dashboard product.

## 2. Crate topology (D-74)

| Path | Package | Responsibility |
|---|---|---|
| crates/observability/telemetry | kelson-telemetry | event bus, `EventEnvelope` + schemas, typed UUIDv7 event ids, classification, `TelemetryPlugin`/`TelemetryRegistry` |
| crates/observability/tracing | kelson-tracing | span model + causality, W3C `traceparent` helpers, adaptive sampler |
| crates/observability/metrics | kelson-metrics | counters/gauges/histograms, bounded label cardinality, Prometheus text renderer |
| crates/observability/audit | kelson-audit | audit event schema, retention types, tamper/equivalence report models |
| crates/observability/pipeline | kelson-pipeline | processing DAG (normalize → classify → enrich → redact → fingerprint → sample → dedupe → route), policy grammar |
| crates/observability/buffer | kelson-buffer | bounded buffers, durability classes, backpressure policy, disk WAL |
| crates/observability/export | kelson-export | `Exporter` trait + console/file/OTLP/HTTP/syslog, circuit breakers, retry budget, health snapshots |
| crates/observability/logging | kelson-observability | runtime host: `ObservabilityRuntime`, policy engine, incident control plane, `ObservabilityProvider`; keeps the S1 API |
| crates/security/integrity | kelson-integrity | audit hash-chain primitives (sha2) + HMAC batch signing (Security class) |

Name resolutions: `kelson-observability` keeps its published name and absorbs the design doc's
`kelson-log` role. The doc's `kelson-context` crate is **not created** — `crates/foundation/context`
owns that name; the propagation carrier `TelemetryContext` lives in `kelson-foundation` instead, so every
layer can carry it without matrix changes. `kelson-console` stays in `tools/`.

Matrix: Observability class depends on foundation only (never runtime, never data). The data
layer cannot depend on observability, hence hash-chaining primitives live in `kelson-integrity`
(Security class), which `kelson-postgres` and `kelson-console` may consume. New crates are
`publish = false` until a release wave; versions stay lockstep.

## 3. Event model

- `EventEnvelope`: `id` (typed UUIDv7), `at` (ISO-8601), `schema` + `schema_version`, service
  identity (name/environment), `TelemetryContext` snapshot, `durability`, `sensitivity`, payload.
- Frozen schema names: `kelson.log`, `kelson.span`, `kelson.metric`, `kelson.audit`, `kelson.exception`,
  `kelson.lifecycle`, `kelson.security`. Categories: Log, Span, Metric, Audit, Exception, Lifecycle,
  Security. Version bumps are additive-only; deserialization preserves unknown fields.
- Durability classes: `Ephemeral` (may be dropped under any pressure — debug chatter),
  `Buffered` (in-memory; survives exporter blips, not process death; the default),
  `Durable` (WAL-backed; survives process restart — audit, errors, security).
- Sensitivity classes: `Public`, `Internal` (default), `Sensitive`, `Secret`. Redaction acts on
  structured fields of Sensitive/Secret events; request bodies are never logged by default and
  everything under `storage/framework/` inherits the "never leaves the environment" rule.

## 4. Context propagation and isolation

- `TelemetryContext` lives in `kelson-foundation` (D-74): trace_id, request_id, execution_id, span_id, tenant_id, principal_id, correlation_id, causation_id, incident_id. Every layer may carry it without matrix changes; the design doc's `kelson-context` crate is intentionally not created.
- kelson-api's `telemetry_context` middleware parses the inbound `traceparent` (else a fresh UUIDv7 trace id), joins the admission RequestId, and stores the context in request extensions before session/admission middleware.
- W3C helpers (`make_traceparent` / `parse_traceparent`) are pure functions in `kelson-tracing`, re-exported by `kelson-observability`.
- Jobs carry `telemetry: Option<TelemetryContext>` in job metadata (a kelson-foundation type, keeping the async layer compliant); hosts restore span context around execution.
- `tenant_id` is stamped from context only. The policy evaluator hard-rejects events whose tenant field contradicts the carrier context. File exporters support per-tenant partitioning.
- Contexts ride tracing spans + explicit carriers only (no ambient state). Isolation hammer tests assert zero cross-tenant/cross-trace leakage under concurrency (G18).

## 5. Processing pipeline

Per-category DAG: normalize → classify → enrich → redact → fingerprint → sample → dedupe → route. Redaction acts on structured fields (default redaction list + `[telemetry.redactions]`); a line-level redactor remains in console/file exporters as defense-in-depth. Fingerprint = normalized hash of exception type + top frames + route. Dedupe window defaults: 10 s / 4096 entries, keyed by fingerprint. Routing: default = all exporters; when a dedicated exporter exists whose name contains "audit", audit events route only there.

## 6. Policy engine

`[[telemetry.policy]]` rules: `name`, `when`, `actions`. `when` is a restricted boolean grammar — fields: `environment`/`env`, `level`, `sensitivity`/`classification`, `category`, `schema`, `tenant_id`/`tenant`, `incident`, `sampled`; operators `==`, `!=`, `>=` (level rank), `&&`, `||`, parentheses. No arbitrary code. Actions: `retain`, `drop`, `alert`, `export:<name>`, `redact:<field,...>`, `hash:<field,...>`, `sample:<rate>`. Evaluation is total: the default route is the terminal action, so misconfiguration cannot wedge the pipeline. Rules that fail to parse are skipped and announced at boot (never fatal). A policy-bypass suite is part of G18.

## 7. Configuration

Layering: compiled defaults ← `[telemetry]` section ← env overlay ← runtime `TelemetryControlState` ← request-local debug override. Base config is frozen at boot (STAGE-0 §10); only `TelemetryControlState` (incident state, sampling tier, debug retention) mutates at runtime, exclusively via the incident control plane (D-76).

`TelemetryConfig::from_section` never fails (unknown keys are ignored — forward compatibility). Defaults: level `info`, format `json`, buffer capacity 8192 / batch 256, dedupe 10 s / 4096 entries, max label sets 1024, retry budget 20 tokens/s, monitor tick 1 s, incident capability `telemetry.incident.manage`. Env overlay: `KEEL_TELEMETRY_LEVEL`, `_FORMAT`, `_WAL_DIR`, `_INCIDENT_DIR`, `_CAPACITY`, `_CONSOLE_DISABLED`. S1 compatibility (kelson-api + kelson-worker): `RUST_LOG` sets the level, `KEEL_LOG_FILE` adds a critical file exporter, and the console format is json in prod/staging else pretty when the section omits `format`.

Storage paths (rebased against the workspace root at boot): WAL `storage/framework/telemetry/wal`, incident control `storage/framework/telemetry/control`, cost rollup `storage/framework/telemetry/cost.json`, exporter health `storage/framework/telemetry/exporters/`. Everything under `storage/framework/` inherits the "never leaves the environment" sensitivity rule.

## 8. Sampling

Per-category base rates (`[telemetry.sampling.rates]`; missing categories keep everything). Audit, Exception, and Security events are never sampled away (enforced by the sample processor, not config). Adaptive tiers `normal → elevated → full` are driven by error rate, p95 latency shift against a learned baseline, queue backlog, and export failures. Incident mode forces error/span classes to 100% and tags events with `incident_id`.

## 9. Buffering and WAL

- Bounded in-memory queue (capacity 8192, batch 256 by default). Ingest is a bounded try-send: `emit` never blocks the caller; overflow applies the backpressure policy and increments `telemetry_dropped`.
- Durability classes gate WAL use: `Durable` events are appended to the WAL before export; `watermark.json` records the highest durably exported LSN.
- WAL format (hand-rolled, `kelson-buffer`; no new storage dependency): length-prefixed JSON records, per-record truncated sha2 checksum, monotonic LSN. Segments are named `segment-<first-LSN:020>.wal` and rotate at `max_segment_bytes` (default 10 MiB). `wal.fsync` (default true) fsyncs every durable append.
- Replay on start resumes from the watermark; corrupted records are skipped and counted (`corrupted_records_skipped`), never fatal. Events below the watermark are never re-exported.

## 10. Exporters, breakers, retry budget

- Kinds: `console` (stdout, pretty/json), `file` (JSONL; optional `partition_by_tenant`), `otlp` (OTLP/HTTP + JSON via reqwest, D-75), `http` (generic JSON POST), `syslog` (RFC 5424 over TCP).
- Every exporter is wrapped in a managed shell: circuit breaker CLOSED/OPEN/HALF_OPEN (defaults: open after 3 consecutive failures, 15 s cooldown), per-exporter retry (defaults: 5 attempts, exponential backoff 100 ms → 10 s, jitter), all constrained by a global retry-budget token bucket (default 20 tokens/s) so retry storms stay contained.
- Exporter health snapshots are written to `storage/framework/telemetry/exporters/<name>.json`: state, attempts, failures, successes, exported_events, exported_bytes, last_error, last_success_at, updated_at.
- `critical = true` exporters are never shed by backpressure.

## 11. Resource monitor and backpressure

- The monitor tick (default 1 s) samples queue depth, exporter/disk health, p95 latency against a learned baseline, and error rate.
- Shed ladder (queue fill + disk health): drop ephemeral/debug events first, then already-sampled events, then shed non-critical exporters; durable events keep an emergency stderr path when nothing can be exported. Escalation order: raise sampling tier → stop ephemeral emission → shed non-critical exporters → emergency console path.

## 12. Incident mode (D-76)

- The file control plane under `storage/framework/telemetry/control/` is the source of truth: `incident.json` = `{id, started_at (ISO-8601), started_by, flags}`.
- Writers: `kelson observe incident start|stop` (CLI) and `POST /admin/telemetry/incident` (kelson-api; session-authenticated principal must hold the `telemetry.incident.manage` capability). Both write the same file, so CLI, HTTP and pollers agree. `GET /admin/telemetry/status` exposes the runtime status.
- Every host polls the file every 2 s. Transitions emit `kelson.lifecycle` events (`incident_start`/`incident_stop`) and set the `telemetry_incident_active` gauge. While active, error/span classes are forced to 100% sampling, events are tagged with `incident_id`, and flags may request deeper debug retention.
- `kelson observe incidents` lists the active incident plus history.

## 13. Audit hash chain (D-74)

- Migration `20260823000001_audit_hash_chain.sql` (embedded in kelson-postgres, G08-ordered): `audit_records` gains `chain_id uuid`, `prev_hash bytea`, `chain_hash bytea` plus an index on `(tenant_id, chain_id, at)`. Backfilled rows stay NULL — chains start fresh.
- `chain_id = chain_id_for(tenant, "YYYY-MM-DD")`: chains are anchored per tenant + day segment, starting from `GENESIS_HASH`.
- Chain computation happens inside the engine transaction (kelson-postgres audit write path) via `kelson-integrity`: `canonical_audit_row(...)` → `audit_hash(prev_hash, canonical)`; `prev_hash` is read from the latest row of the same `(tenant_id, chain_id)`. No side channels (platform non-negotiable).
- `kelson observe audit --verify` rebuilds the canonical string from the stored columns exactly as the write path does and recomputes the chain, reporting the first divergence (`ChainBreak { position, row_id, reason: HashMismatch | BrokenLink }`).
- Optional HMAC batch signing keyed by `app.key` (`kelson-integrity::hmac`).

## 14. Metrics, cost, cardinality

- `kelson-metrics`: counters/gauges/histograms with bounded label cardinality (`[telemetry.cardinality] max_label_sets`, default 1024; overflow collapses into a hashed label set rather than growing unboundedly).
- Prometheus text exposition (0.0.4) is hand-rolled; `GET /metrics` is flag-gated by `app.metrics` (recording stays unconditional). This rescopes D-68 and closes gap-analysis item B-17.
- Framework counter bridges live in runtime/observability only (HTTP request totals/latency from edge middleware, QueryProbe-derived DB latency, job queue depth/age, cache hit/miss): nothing in web/services/async/data gains an observability dependency.
- Cost accounting: per-source and per-exporter event+byte counters roll up to `storage/framework/telemetry/cost.json` (`by_source`, `by_exporter {events, bytes}`, totals); rendered by `kelson observe cost`.

## 15. Anomaly detection (deterministic)

The resource-monitor tick runs threshold detectors (no ML) and emit `telemetry.lifecycle` events with kind `anomaly.<kind>`:
- `anomaly.error_rate_elevated` — error rate >= 5% with >= 20 events in the window
- `anomaly.error_rate_spike` — error rate >= 20%
- `anomaly.latency_spike` — p95 >= 10x the learned baseline (a 3x ratio escalates the sampling tier without an anomaly event)
- `anomaly.queue_backlog` — queue fill >= 90%
- `anomaly.export_failure` — critical exporter breaker open / disk unhealthy

Thresholds are constants in `kelson-tracing` (`ERROR_RATE_ELEVATED=0.05`, `ERROR_RATE_FULL=0.20`, `LATENCY_RATIO_ELEVATED=3.0`, `LATENCY_RATIO_FULL=10.0`, `QUEUE_FILL_FULL=0.90`). Anomalies feed sampling escalation.

## 16. Exception grouping

Bounded in-process registry (moka) keyed by fingerprint: count, first_seen/last_seen, example signature (`Type: message`), affected routes and tenants. Surfaced by `kelson observe errors` and the incident view.

## 17. Plugin architecture

`TelemetryPlugin::register(&self, registry: &mut TelemetryRegistry)` contributes categories, processors, and exporters. Plugins register through `ObservabilityProvider::with_plugin(...)`; applications/modules may contribute plugins (the matrix allows applications -> observability). Framework subsystems keep producing framework-neutral records (`QueryProbe`, `ExecutionTrace`, job/outbox counters); runtime bridges convert them into events — nothing in web/services/async/data gains an observability dependency.

## 18. Runtime wiring and developer API

- `ObservabilityProvider` implements the kelson-lifecycle `Provider`: `Phase::Foundation`, depends only on config. `init` registers the inert runtime + metrics registry in the service container; `boot` starts the runtime (idempotent).
- Hosts construct + start the runtime BEFORE the lifecycle host boots (boot-time logging parity) and adopt it via `ObservabilityProvider::from_runtime` — a host never builds two runtimes.
- Shutdown: hosts call `runtime.shutdown(5s)` after serve/loop exit, including error paths; `Drop` is a 2 s backstop. Drain flushes buffers + WAL within the deadline; exporters close even on timeout.
- Developer API: `tlog_debug!/tlog_info!/tlog_warn!/tlog_error!` and plain `tracing::*` macros both reach the same bus (tracing bridge). `global_runtime()` / `emit_log_global` provide a stderr fallback when no runtime is running — events are never silently lost.
- Telemetry never blocks or crashes the application: bounded try-send with counted drops, isolated exporter worker threads, panics contained to the stderr fallback.

## 19. CLI — kelson observe (tools/kelson-console)

Operates on local stores (exported JSONL, WAL segments, exporter health files, cost.json, the incident control file, the trace store, the postgres audit chain); no live host required:
`status`, `logs [--last n] [--level l]`, `errors [--last n]`, `traces [--trace id] [--last n]`, `request <id>`, `audit [--verify] [--tenant id] [--since ts] [--limit n]`, `incidents`, `incident start|stop [--operator name]`, `exporters`, `policies`, `buffers`, `health`, `cost`, `metrics`.
`audit --verify` needs the configured database; everything else is file-only. The same incident plane is exposed over HTTP (`POST /admin/telemetry/incident`, `GET /admin/telemetry/status`), so CLI, HTTP, and pollers agree.

## 20. Reliability requirements (SLO-style)

- R1 Non-blocking emit: `emit` is a bounded try-send; overflow applies the backpressure policy and increments `telemetry_dropped`.
- R2 Telemetry failure never crashes the application: exporter panics are isolated; a stderr fallback covers the case where the subscriber cannot initialize.
- R3 Event-loss accounting: dropped vs emitted counters are always reconcilable from `status()`.
- R4 WAL replay resumes from the watermark; corrupted records are skipped and counted; events below the watermark are never re-exported.
- R5 SIGKILL gap (documented, accepted): durability is guaranteed to the last fsync'd record; records appended after the last fsync (`wal.fsync = false`, or an OS-level crash between append and fsync) may be lost.
- R6 Graceful shutdown drains within the deadline (hosts: 5 s); buffers + WAL flush inside the deadline; exporters close even on timeout.
- R7 Degraded boot (G10): telemetry initializes on file/in-memory backends even without a database.
- R8 Retry storms stay contained under the global retry budget (default 20 tokens/s).
- R9 Request bodies are never logged by default; everything under `storage/framework/` never leaves the environment.

## 21. Gates and test requirements

- **G18 telemetry resilience** (CI): the failure-injection + isolation suite — ENOSPC on the file exporter, exporter timeout/crash, queue-full backpressure, corrupted WAL segments, partial writes, shutdown-during-drain, degraded boot with all exporters failing (app still serves); the tenant-isolation hammer (N concurrent executions across >= 2 tenants, zero trace/tenant leakage); the policy-bypass suite; the incident-endpoint capability requirement.
- Unit coverage: envelope serde roundtrip incl. unknown-field preservation; ID ordering/uniqueness; sampler math incl. adaptive transitions; policy grammar + terminal-action totality; redaction regression; fingerprint stability; WAL write/replay/corruption recovery; breaker state machine; retry-budget exhaustion; dedupe windows; cardinality overflow hashing; hash-chain build/verify/tamper-detect; Prometheus rendering.
- Security: PII/secret leakage matrix across exporters; log-injection (CRLF/newline) escaping in console/file/syslog; audit tamper detection.
- Performance: emit overhead + throughput fold into the G15 benchmark baseline (no new bench dependencies) as the `kelson benchmark` phase `telemetry-emit`; soft targets are tracked as baseline regressions, not as separate CI numbers.
- Compatibility: ubuntu + windows matrix; portable paths only (no unix-only APIs); SIGTERM via existing tokio signal wiring.

## 22. Implementation waves and locked assumptions

Waves: **S5.1** core bus (kelson-telemetry, TelemetryContext in kelson-foundation, ingestor, runtime skeleton, tracing bridge, console/file exporters; S1 API preserved) · **S5.2** pipeline + resilience (kelson-pipeline, kelson-buffer incl. WAL + backpressure, breakers/retry budget in kelson-export, resource monitor) · **S5.3** policy + incidents (policy engine + grammar, file + HTTP control plane, incident mode, adaptive sampling) · **S5.4** metrics + cost (kelson-metrics, /metrics, counter bridges, cost accounting, cardinality protection, anomaly detectors) · **S5.5** audit + CLI + plugins (kelson-integrity, migration, chain writes + verify, kelson observe family, plugin API, exception grouping) · **S5.6** hardening (full §21 suites as G18, G15 baseline, docs/ADRs, examples wiring).

Locked assumptions: OTLP = OTLP/HTTP + JSON over reqwest, no `opentelemetry*` crates (D-75); WAL hand-rolled under `storage/framework/telemetry/wal/` (no new storage dependency); `kelson-observability` keeps its name and absorbs the doc's `kelson-log`; the doc's `kelson-context` crate is not created (carrier in kelson-foundation, D-74); base config frozen at boot — only `TelemetryControlState` mutates at runtime, solely via the incident control plane (D-76); the file plane is the source of truth for incident mode; chain computation inside the engine transaction, verification in the CLI; new crates `publish = false` until a release wave, lockstep versions; medicare-kernel untouched (D-70); scope is `platform/` only.
