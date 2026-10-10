# ADR 0005 — Sentinel crate split, TelemetryContext carrier, kelson-integrity

Status: **Accepted** (2026-08-24) · Decision: **D-74** · Spec: `specs/STAGE-SENTINEL-SPEC.md` §2

## Context

The Sentinel design doc proposes ~10 new crates (`kelson-log`, `kelson-context`, `kelson-telemetry`,
`kelson-tracing`, `kelson-metrics`, `kelson-audit`, `kelson-pipeline`, `kelson-buffer`, `kelson-export`,
`kelson-integrity`). Two collide with platform reality:

1. `crates/foundation/context` already owns the package name `kelson-context`, and foundation is
   the trusted computing base — a second context crate would split the carrier story.
2. The frozen dependency matrix (STAGE-0-SPEC §3) makes observability depend on foundation only
   and forbids data (kelson-postgres) from depending on observability. But audit hash-chaining must
   be computed **inside the engine transaction** (platform non-negotiable: no side channels),
   i.e. inside `kelson-postgres`.

## Decision

1. **Split Sentinel into observability-class crates** under `crates/observability/`:
   `kelson-telemetry` (event bus, envelope, ids, classification, plugin traits), `kelson-tracing`
   (span model, W3C traceparent helpers, adaptive sampler), `kelson-metrics` (counters/gauges/
   histograms, bounded cardinality, Prometheus renderer), `kelson-audit` (audit schema, retention,
   verification report models), `kelson-pipeline` (processing DAG + policy grammar), `kelson-buffer`
   (bounded buffers, durability classes, backpressure, WAL), `kelson-export` (exporters, breakers,
   retry budget, health snapshots). All resolve to the existing analyzer `Class::Observability`
   (allowed deps: foundation + observability) — no matrix change.
2. **`kelson-observability` keeps its published name** and absorbs the doc's `kelson-log` role:
   developer macros, `ObservabilityRuntime`, policy engine, incident mode, resource monitor,
   anomaly detectors, exception groups. The S1 public API stays available.
3. **No `kelson-context` crate.** The propagation carrier `TelemetryContext { trace_id, request_id,
   execution_id, span_id, tenant_id, principal_id, correlation_id, causation_id, incident_id }`
   is a typed value in `kelson-foundation` (foundation), so every layer — including async job metadata —
   can carry it without matrix changes.
4. **`kelson-integrity` in the Security class** (`crates/security/integrity`): audit hash-chain
   primitives (`chain_id_for`, `canonical_audit_row`, `audit_hash`, `verify_chain`) over sha2,
   plus optional HMAC batch signing. Security may depend only on foundation, and data may depend
   on security — so `kelson-postgres` computes chain hashes inside the audit-write transaction,
   and `kelson-console` (tools class, may depend on almost everything) ships `observe audit --verify`.
   Observability never gains a data dependency.

## Consequences

- Workspace grows by 8 members; all new crates are `publish = false` with lockstep versions.
- `DEPENDENCY-POLICY.md` justification rows updated: sha2 now also serves kelson-integrity +
  kelson-buffer WAL checksums + fingerprinting; moka also bounds dedupe windows, label cardinality
  and exception groups. No new externals; budget stays 23/25.
- The design doc's `kelson-console` becomes an `observe` subcommand family in the existing
  `tools/kelson-console` (no new tool crate).
- Verification tooling lives in the CLI rather than in observability so that the verify path can
  read postgres rows and integrity primitives without inverting the matrix.
