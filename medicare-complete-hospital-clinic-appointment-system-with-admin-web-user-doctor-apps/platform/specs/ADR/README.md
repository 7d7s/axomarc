# Architecture Decision Records

Index of ADRs (one file each: NNNN-title.md). The decision log in
`docs/rust/08-DECISIONS.md` is the master list; ADRs here carry long-form rationale.

| ADR | Title | Decisions |
|---|---|---|
| 0004-sentinel-telemetry-runtime.md | Sentinel: single-application telemetry runtime + stage insertion | D-73, D-76 |
| 0005-sentinel-crate-split.md | Sentinel crate split, TelemetryContext carrier, keel-integrity | D-74 |
| 0006-otlp-over-reqwest.md | OTLP export over reqwest (no opentelemetry dependencies) | D-75 |
| 0007-json-api-spec-server.md | JSON:API spec server (compound documents over Wave 3 resources) | Wave 8 gate (P6) |
| 0008-precognition-header.md | Precognition (validate-only dispatch) | Wave 8 gate (P6) |
| 0009-ops-dashboard.md | Operations dashboard (read-only over inspect/job/scheduler stores) | Wave 8 gate (P6) |
| 0010-mysql-driver.md | MySQL as a first-class SQL driver (keel-mysql) | Wave 9 B-31 |
