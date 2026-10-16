# Architecture Decision Records

Index of ADRs (one file each: NNNN-title.md). The decision log in
`docs/rust/08-DECISIONS.md` is the master list; ADRs here carry long-form rationale.

| ADR | Title | Decisions |
|---|---|---|
| 0004-sentinel-telemetry-runtime.md | Sentinel: single-application telemetry runtime + stage insertion | D-73, D-76 |
| 0005-sentinel-crate-split.md | Sentinel crate split, TelemetryContext carrier, kelson-integrity | D-74 |
| 0006-otlp-over-reqwest.md | OTLP export over reqwest (no opentelemetry dependencies) | D-75 |
| 0007-json-api-spec-server.md | JSON:API spec server (compound documents over Wave 3 resources) | Wave 8 gate (P6) |
| 0008-precognition-header.md | Precognition (validate-only dispatch) | Wave 8 gate (P6) |
| 0009-ops-dashboard.md | Operations dashboard (read-only over inspect/job/scheduler stores) | Wave 8 gate (P6) |
| 0010-mysql-driver.md | MySQL as a first-class SQL driver (kelson-mysql) | Wave 9 B-31 |
| 0011-rs256-jwt.md | RS256 JWT interop verifier (budget-gated) | Wave 9 gate (advanced backlog) |
| 0012-password-breach-range-check.md | Password breach check (k-anonymity range API) | Wave 9 gate (advanced backlog) |
| 0013-recursive-cte-relations.md | Recursive/CTE relations in the query builder | Wave 9 gate (advanced backlog) |
| 0019-wasm-abi-v2-component-model.md | Wasm ABI v2: Component Model behind an opt-in feature, ABI v1 retained (amends root ADR 0016 §2/§3) | Wave A of the distribution plan |
| 0020-module-host-import-world.md | Application modules as WASM guests: the host-import world (guests implement the contract traits; JSON over imports) | Stage C.2 / G3 layer isolation |

Numbers 0014-0018 are reserved: they exist in the root tree's
`platform/specs/ADR/` record (owned by the 0.2a port, cited not edited), and
ADR 0014 makes that tree canonical at 0.2c — so numbering here continues at
0019 to avoid a collision when the trees converge.
