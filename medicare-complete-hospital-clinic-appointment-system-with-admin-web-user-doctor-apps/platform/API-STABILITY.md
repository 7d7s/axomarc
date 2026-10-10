# API Stability

## Public surface
- Wire: `Envelope`, `ErrorBody` (kelson-http), error `code` strings (kelson-foundation).
- Rust: items re-exported from crate roots of the `crates/**` kelson-* crates.
- Ops: `OperationId` values and capability names in `module.toml` manifests.

## Guarantees
- Error codes are append-only (`kelson-foundation::codes`): never renumber, never reuse.
- Operation ids are permanent once exposed; changing one is a breaking change
  requiring the deprecation flow (DEPRECATION.md).
- Envelope field additions are non-breaking; removals/rename are breaking.

## Pre-1.0
Breaking changes are allowed but must ship with: decision entry, CHANGELOG note,
and one release of migration guidance.