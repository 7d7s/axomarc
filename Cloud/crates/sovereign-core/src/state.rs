// The dependency-injection container for the use cases.
//
// `AppState` is the single argument every use case takes. It owns the
// port implementations as `Arc<dyn ...>` so they can be shared across
// the CLI, the TUI, and (in V1) the control-plane HTTP server without
// copying state. Use cases call `state.storage.<method>(...)` and
// `state.runtime.<method>(...)` — never the concrete types directly.

use std::sync::Arc;

use crate::error::AppError;
use crate::ports::{ProxyPort, RuntimePort, SecretsPort, StoragePort};

/// The shared application state. Cheap to clone (`Arc`s inside).
#[derive(Clone)]
pub struct AppState {
    /// The storage adapter (SQLite in V0).
    pub storage: Arc<dyn StoragePort>,
    /// The container runtime adapter (Docker in V0).
    pub runtime: Arc<dyn RuntimePort>,
    /// The reverse-proxy adapter (Caddy in V0). `None` when the host
    /// has no proxy configured (test environments, `--no-proxy` on
    /// the CLI, or a Caddy install that the binary cannot reach).
    /// Use cases MUST tolerate `None` and fall back to the
    /// container-only URL.
    pub proxy: Option<Arc<dyn ProxyPort>>,
    /// The encrypted-secrets adapter (age in V0). `None` when the
    /// master key is unavailable (no sovereign init yet, or the
    /// `sovereign secret` subcommand is run before the first
    /// `sovereign init`). Use cases MUST tolerate `None` and
    /// surface a "run sovereign init first" error.
    pub secrets: Option<Arc<dyn SecretsPort>>,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState").finish_non_exhaustive()
    }
}

/// Sentinel: an `AppState` for use cases that do not touch the
/// container runtime (e.g. the `secret` CLI handler). Calling any
/// runtime method on it will return `AppError::Internal("no
/// runtime configured")` — by design, so a use case can never
/// accidentally run a container in a no-runtime context.
pub fn no_runtime() -> Arc<dyn RuntimePort> {
    struct NoRuntime;
    #[async_trait::async_trait]
    impl RuntimePort for NoRuntime {
        async fn pull_image(&self, _image: &str) -> Result<(), AppError> {
            Err(AppError::internal("no runtime configured"))
        }
        async fn create_container(
            &self,
            _spec: crate::ports::ContainerSpec,
        ) -> Result<String, AppError> {
            Err(AppError::internal("no runtime configured"))
        }
        async fn start_container(&self, _id: &str) -> Result<(), AppError> {
            Err(AppError::internal("no runtime configured"))
        }
        async fn stop_container(
            &self,
            _id: &str,
            _timeout: std::time::Duration,
        ) -> Result<(), AppError> {
            Err(AppError::internal("no runtime configured"))
        }
        async fn remove_container(&self, _id: &str) -> Result<(), AppError> {
            Err(AppError::internal("no runtime configured"))
        }
        async fn healthcheck(
            &self,
            _id: &str,
            _port: u16,
            _path: &str,
            _timeout: std::time::Duration,
        ) -> Result<crate::ports::HealthResult, AppError> {
            Err(AppError::internal("no runtime configured"))
        }
    }
    Arc::new(NoRuntime)
}
