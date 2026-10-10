# Compatibility

## Rust toolchain
Kelson builds on the pinned channel in `rust-toolchain.toml` (currently 1.92.0).
`workspace.package.rust-version` states the MSRV for published crates.

## Wire format
The public envelope (`kelson-http::Envelope`) and error body are the only wire
contracts. Fields may be added; existing fields never change meaning or vanish
without a deprecation window (see DEPRECATION.md).

## Database
Migrations are forward-only, additive, and idempotent. Destructive schema
changes require a two-phase plan documented in the ADR.