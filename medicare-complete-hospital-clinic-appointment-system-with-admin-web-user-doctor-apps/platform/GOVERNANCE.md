# Governance

Kelson is governed by frozen specs first, people second.

## Authority order
1. `docs/rust/11-FOUNDATION-MASTER-SPEC.md` (topology, dependency rules, gates, decisions)
2. These governance documents
3. Maintainer judgment

## Change classes
- **Spec change** — requires a superseding decision entry (D-xx) in
  `docs/rust/08-DECISIONS.md` and an ADR under `specs/ADR/`.
- **Kernel change** (`crates/foundation/`, `crates/execution/`, `crates/security/`) — two reviewers minimum; all gates green.
- **Framework/adapter change** — one reviewer; gates green.
- **Module addition** — via `kelson make:module`, then `kelson check` + `kelson graph`.

## Gates
Release requires G00–G20 (spec §13) green. The architecture gate (G07) is
machine-enforced in CI via `validation/architecture`.

## Decision log
Decisions are append-only. A decision is superseded, never silently edited.