// Wire types (REST/JSON DTOs) shared between the binary and the control plane.
// Per docs/architecture.md §4, all API traffic is REST + JSON, URL-prefix
// `/v1`, cursor-paginated. The DTOs live here so the binary and the control
// plane can never drift.
//
// F1 only stubs the version endpoint. F2 will add the full endpoint catalog
// per docs/architecture.md §4.1.

#![deny(unsafe_code)]
#![warn(missing_docs)]

use serde::{Deserialize, Serialize};

/// `GET /v1/version` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionResponse {
    /// The binary version, e.g. "0.1.0".
    pub version: &'static str,
    /// The git commit SHA, if available.
    pub commit: Option<&'static str>,
    /// The build target, e.g. "x86_64-unknown-linux-musl". `None` if the
    /// `VERGEN_BUILD_TARGET` env var was not set at compile time (i.e. no
    /// `vergen` build script was used for the build).
    pub target: Option<&'static str>,
    /// The build profile, "release" or "dev".
    pub profile: &'static str,
}

impl VersionResponse {
    /// Build a `VersionResponse` from compile-time `CARGO_PKG_*` env vars.
    pub const fn current() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION"),
            commit: option_env!("GIT_COMMIT"),
            target: option_env!("VERGEN_BUILD_TARGET"),
            profile: if cfg!(debug_assertions) { "dev" } else { "release" },
        }
    }
}

/// Crate version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
