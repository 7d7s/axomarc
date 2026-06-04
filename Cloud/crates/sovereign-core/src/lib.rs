// Sovereign core: use cases + domain types.
// Per docs/architecture.md §1 (hexagonal layout), this crate contains no
// adapter code. Adapters live in crates/sovereign-{runtime,proxy,secrets,
// storage,backup,notify,observability}.

#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod domain;
pub mod error;
pub mod ports;

/// Crate version (matches Cargo.toml).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
