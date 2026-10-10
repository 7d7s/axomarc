# ADR 0004 — Sentinel: single-application telemetry runtime + stage insertion

Status: **Accepted** (2026-08-24) · Decisions: **D-73**, **D-76** · Spec: `specs/STAGE-SENTINEL-SPEC.md`

## Context

S1 shipped logging as `init_logging` + redaction + diagnostic chain records (~400 LOC in
`kelson-observability`). D-68 deliberately deferred metrics/profiling to stub contracts. Production
operation needs more: one telemetry runtime per host that every framework subsystem emits
through, durability-classed buffering that survives exporter blips and process restarts,
circuit-broken exporters with contained retries, a declarative policy engine, incident mode that
operators can flip without a redeploy, tamper-evident audit, and Prometheus exposition (gap
analysis item B-17). The design doc covers this in 32 sections; this ADR records the governing
architecture decisions.

## Decision

1. **Insert frozen stage S5 "Sentinel"** between S4 developer platform and AI/MCP (D-73).
   AI/MCP becomes S6, public ecosystem becomes S7. D-68's metrics deferral is rescoped into S5;
   `GET /metrics` (Prometheus text 0.0.4, flag-gated by `app.metrics`) closes B-17.
2. **One application-owned `ObservabilityRuntime` per host** (`kelson-observability`): ingestor,
   processing DAG, router, buffer manager, export manager, incident poller, resource monitor.
   Hosts construct and start it before the lifecycle host boots (boot-time logging parity) and
   adopt it via `ObservabilityProvider::from_runtime` — never two runtimes per process. The S1
   API (`init_logging`, `LogConfig`, `redact_str`, chain records) stays available.
3. **Telemetry never blocks or crashes the application**: bounded try-send ingest with counted
   drops, isolated exporter worker threads, stderr fallback when no runtime/subscriber is
   available. Degraded boot (no DB) still initializes telemetry on file/in-memory backends (G10).
4. **Base configuration is frozen at boot** (STAGE-0 §10). The only runtime-mutable telemetry
   posture is `TelemetryControlState`, mutated exclusively by the incident control plane.
5. **Incident control plane (D-76)**: the file plane under `storage/framework/telemetry/control/`
   (`incident.json`) is the source of truth. `kelson observe incident start|stop` (CLI) and
   `POST /admin/telemetry/incident` (kelson-api; session auth + `telemetry.incident.manage`
   capability) write the same file; every host polls it every 2 s. While active, error/span
   classes are forced to 100% sampling and events are tagged with `incident_id`. CLI, HTTP and
   pollers therefore always agree, and a host restart loses nothing.
6. **New CI gate G18 "telemetry resilience"**: failure-injection (ENOSPC, exporter crash,
   queue-full, WAL corruption, shutdown-during-drain) + tenant-isolation hammer + policy-bypass
   suites green in CI.

## Consequences

- Roadmap renumbered: STAGE-0-SPEC §14/§15, `docs/rust/08-DECISIONS.md` §8; the S7-era matrix
  extension formerly recorded as D-73 is renumbered D-77.
- `kelson-api` and `kelson-worker` both replace `init_logging` with the runtime + provider.
- WAL durability is guaranteed to the last fsync'd record; the SIGKILL gap is documented and
  accepted (STAGE-SENTINEL-SPEC §20 R5).
- New observability crates are `publish = false` until a release wave; versions stay lockstep.
- medicare-kernel untouched (D-70); scope is `platform/` only.
