// The dependency-injection container for the use cases.
//
// `AppState` is the single argument every use case takes. It owns the
// port implementations as `Arc<dyn ...>` so they can be shared across
// the CLI, the TUI, and (in V1) the control-plane HTTP server without
// copying state. Use cases call `state.storage.<method>(...)` and
// `state.runtime.<method>(...)` — never the concrete types directly.

use std::sync::Arc;

use crate::ports::{ProxyPort, RuntimePort, StoragePort};

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
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState").finish_non_exhaustive()
    }
}
