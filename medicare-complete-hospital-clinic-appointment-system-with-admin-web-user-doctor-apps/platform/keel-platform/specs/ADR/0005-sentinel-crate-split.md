# ADR 0005 — Sentinel crate split, TelemetryContext carrier, keel-integrity

Status: **Accepted** (2026-08-24) · Decision: **D-74** · Spec: `specs/STAGE-SENTINEL-SPEC.md` §2

## Context

The Sentinel design doc proposes ~10 new crates (`keel-log`, `keel-context`, `keel-telemetry`,
`keel-tracing`, `keel-metrics`, `keel-audit`, `keel-pipeline`, `keel-buffer`, `keel-export`,
`keel-integrity`). Two collide with platform reality:

1. `crates/foundation/context` already owns the package name `keel-context`, and foundation is
   the trusted computing base — a second context crate would split the carrier story.
2. The frozen dependency matrix (STAGE-0-SPEC §3) makes observability depend on foundation only
   and forbids data (keel-postgres) from depending on observability. But audit hash-chaining must
   be computed **inside the engine transaction** (platform non-negotiable: no side channels),
   i.e. inside `keel-postgres`.

## Decision

1. **Split Sentinel into observability-class crates** under `crates/observability/`:
   `keel-telemetry` (event bus, envelope, ids, classification, plugin traits), `keel-tracing`
   (span model, W3C traceparent helpers, adaptive sampler), `keel-metrics` (counters/gauges/
   histograms, bounded cardinality, Prometheus renderer), `keel-audit` (audit schema, retention,
   verification report models), `keel-pipeline` (processing DAG + policy grammar), `keel-buffer`
   (bounded buffers, durability classes, backpressure, WAL), `keel-export` (exporters, breakers,
   retry budget, health snapshots). All resolve to the existing analyzer `Class::Observability`
   (allowed deps: foundation + observability) — no matrix change.
2. **`keel-observability` keeps its published name** and absorbs the doc's `keel-log` role:
   developer macros, `ObservabilityRuntime`, policy engine, incident mode, resource monitor,
   anomaly detectors, exception groups. The S1 public API stays available.
3. **No `keel-context` crate.** The propagation carrier `TelemetryContext { trace_id, request_id,
   execution_id, span_id, tenant_id, principal_id, correlation_id, causation_id, incident_id }`
   is a typed value in `keel-foundation` (foundation), so every layer — including async job metadata —
   can carry it without matrix changes.
4. **`keel-integrity` in the Security class** (`crates/security/integrity`): audit hash-chain
   primitives (`chain_id_for`, `canonical_audit_row`, `audit_hash`, `verify_chain`) over sha2,
   plus optional HMAC batch signing. Security may depend only on foundation, and data may depend
   on security — so `keel-postgres` computes chain hashes inside the audit-write transaction,
   and `keel-console` (tools class, may depend on almost everything) ships `observe audit --verify`.
   Observability never gains a data dependency.

## Consequences

- Workspace grows by 8 members; all new crates are `publish = false` with lockstep versions.
- `DEPENDENCY-POLICY.md` justification rows updated: sha2 now also serves keel-integrity +
  keel-buffer WAL checksums + fingerprinting; moka also bounds dedupe windows, label cardinality
  and exception groups. No new externals; budget stays 23/25.
- The design doc's `keel-console` becomes an `observe` subcommand family in the existing
  `tools/keel-console` (no new tool crate).
- Verification tooling lives in the CLI rather than in observability so that the verify path can
  read postgres rows and integrity primitives without inverting the matrix.
