//! # sovereign-core
//!
//! **Domain types, use cases, and port traits for Sovereign.**
//!
//! This crate is the heart of Sovereign. It contains:
//!
//! - **Domain types** (`domain/`) — `App`, `Deployment`, `User`, `Secret`, `AuditEvent`, etc.
//! - **Use cases** (`use_cases/`) — `deploy`, `rollback`, `secret`, `backup`, `health`, `migrate`
//! - **Port traits** (`ports/`) — `StoragePort`, `RuntimePort`, `ProxyPort`, `SecretsPort`, `BackupPort`
//! - **RBAC** (`rbac/`) — `Actor`, `Action`, role hierarchy, scope-based access control
//! - **Error type** (`error/`) — `AppError` with HTTP status mapping
//! - **State** (`state/`) — `AppState` dependency injection container
//!
//! ## Architecture
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
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use sovereign_core::domain::{AppId, DeployMode, Strategy};
//! use sovereign_core::use_cases::deploy::{DeployRequest, start_deploy};
//!
//! // Build a deploy request
//! let req = DeployRequest {
//!     app_id: AppId::generate(),
//!     image_ref: Some("nginx:alpine".to_string()),
//!     strategy: Strategy::BlueGreen,
//!     wait: true,
//!     actor: "cli".to_string(),
//! };
//! ```

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
pub mod rbac;
pub mod state;
pub mod use_cases;

/// Crate version (matches `Cargo.toml`).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
