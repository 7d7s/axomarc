# ADR 0006 — OTLP export over reqwest (no opentelemetry dependencies)

Status: **Accepted** (2026-08-24) · Decision: **D-75** · Spec: `specs/STAGE-SENTINEL-SPEC.md` §10

## Context

Sentinel needs an OTLP exporter so events can reach standard collectors (Tempo, Loki, OTel
Collector, vendor backends). The canonical route is the `opentelemetry*` crate family. Problems:

1. **Dependency budget.** D-62 caps direct externals at 25 (currently 23). The OTel SDK stack is
   multiple crates (api, sdk, otlp exporter, proto, tonic/http transport layers) — adopting it
   would consume most of the remaining budget in one feature.
2. **Weight and churn.** The OTel Rust stack is fast-moving with frequent breaking releases and
   heavy transitive trees (tonic/prost), conflicting with the "small, focused crates" admission
   criterion and the MSRV policy.
3. **Redundancy.** Sentinel already owns the event model, sampling, batching, breakers and retry
   budget. The SDK's own pipeline would duplicate (and fight) the platform's processing DAG.

## Decision

- Implement OTLP export directly in `keel-export` as **OTLP/HTTP with JSON encoding over
  reqwest** (`POST {endpoint}/v1/logs|traces|metrics` with the OTLP JSON wire shape). reqwest is
  already an admitted dependency (default-features OFF; json + rustls-tls).
- **No `opentelemetry*` dependencies.** Protobuf encoding is deferred; JSON is accepted by all
  major OTLP receivers, which keeps the default path dependency-free.
- The exporter participates in the standard managed shell: circuit breaker, per-exporter retry,
  global retry budget, health snapshots — identical semantics to console/file/http/syslog.

## Consequences

- Dependency budget stays 23/25 (D-62 unbreached); the justification table is unchanged.
- Receivers that hard-require protobuf need a future change; adopting protobuf (or an OTel SDK)
  later requires a new ADR plus a `DEPENDENCY-POLICY.md` row first.
- Mapping is lossless for the fields Sentinel emits (schema, context ids, severity, payload);
  OTLP-specific attributes are generated at export time, not stored in envelopes.
