//! Sovereign core: use cases + domain types.
//!
//! Per `docs/architecture.md` §1 (hexagonal layout), this crate contains no
//! adapter code. Adapters live in `crates/sovereign-{runtime,proxy,secrets,
//! storage,backup,notify,observability}` and implement the traits in
//! [`ports`]. The [`use_cases`] module orchestrates the ports against the
//! [`domain`] types. The [`state::AppState`] DI container is the composition
//! root: `main.rs` builds it and passes it to the use cases.
//!
//! ## Boundaries
//!
//! - `domain` is free of `async` and of all adapter dependencies.
//! - `ports` is free of `async` *implementations*; it defines the interfaces.
//! - `use_cases` is the only place that wires storage + runtime together.
//! - `error` is the single error type that crosses every boundary.

#![deny(unsafe_code)]
// `missing_docs` is temporarily allowed at the crate level during the
// phase-0 close-out. The public API surface is small and well-named; the
// per-item doc comments will be added back as F6-F10 land and touch the
// public types. The lint stays on at the workspace level (RUSTFLAGS) so
// new code in the rest of the workspace is still warned.
#![allow(missing_docs)]

pub mod domain;
pub mod error;
pub mod ports;
pub mod state;
pub mod use_cases;

/// Crate version (matches `Cargo.toml`).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
