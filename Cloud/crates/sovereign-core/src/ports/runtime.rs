// The runtime port. Implemented by `sovereign-runtime-docker` (bollard)
// and any future adapter (e.g. containerd, podman, firecracker).
//
// Per `docs/architecture.md` §1.3, the runtime owns: image pull,
// container create/start/stop/remove, log streaming, and healthcheck.
// The runtime MUST be a pure port — it does not know about storage,
// audit, or the CLI. A use case composes the runtime with storage.

use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::AppError;

/// The result of a single health probe. Reported in the deployment
/// payload so the operator can see latency + cause on failure.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HealthResult {
    /// `true` if the probe returned 2xx (HTTP) or the TCP port was open.
    pub ok: bool,
    /// Wall-clock latency from probe start to first response byte.
    pub latency_ms: u64,
    /// On failure, the cause (timeout, 5xx, refused, dns, etc.).
    pub error: Option<String>,
}

/// The container spec the deploy use case hands to the runtime. The
/// runtime is free to extend it (network mode, cgroup limits, etc.) —
/// the use case only needs the minimum to get a container serving.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerSpec {
    /// Image ref to run (e.g. `ghcr.io/me/api:v1`).
    pub image: String,
    /// Container name (`sovereign-app-<app_id>` by convention).
    pub name: String,
    /// Port the app listens on inside the container. Mapped to a
    /// host port by the runtime.
    pub port: u16,
    /// Env vars (secrets are injected by the proxy adapter, not here).
    pub env: Vec<(String, String)>,
    /// Volume mounts. Use sparingly; sovereign's F7 encrypted-secret
    /// path avoids mounts entirely.
    pub mounts: Vec<MountSpec>,
}

/// A single bind mount.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MountSpec {
    pub host_path: String,
    pub container_path: String,
    pub read_only: bool,
}

/// Where the runtime finds its daemon. The CLI flag/env is resolved
/// into one of these values in the runtime adapter.
#[derive(Debug, Clone)]
pub enum RuntimeEndpoint {
    /// Unix socket (the default on Linux): `/var/run/docker.sock`.
    UnixSocket(String),
    /// TCP endpoint (e.g. `tcp://docker:2375` on a remote host).
    Tcp(String),
    /// Named pipe (Windows): `//./pipe/docker_engine`.
    NamedPipe(String),
}

impl RuntimeEndpoint {
    /// Resolve from the `DOCKER_HOST` env var or the platform default.
    pub fn from_env() -> Self {
        if let Ok(host) = std::env::var("DOCKER_HOST") {
            if let Some(path) = host.strip_prefix("unix://") {
                return Self::UnixSocket(path.to_string());
            }
            if let Some(addr) = host.strip_prefix("tcp://") {
                return Self::Tcp(addr.to_string());
            }
            if let Some(path) = host.strip_prefix("npipe://") {
                return Self::NamedPipe(path.to_string());
            }
        }
        #[cfg(unix)]
        return Self::UnixSocket("/var/run/docker.sock".to_string());
        #[cfg(windows)]
        return Self::NamedPipe(r"\\.\pipe\docker_engine".to_string());
    }
}

/// The runtime port. All methods return `AppError::Upstream` on
/// daemon errors and `AppError::Validation` on bad inputs.
#[async_trait]
pub trait RuntimePort: Send + Sync {
    /// Pull the image into the local daemon's image cache. Idempotent
    /// (a second call with the same ref is a no-op fast path).
    async fn pull_image(&self, image: &str) -> Result<(), AppError>;

    /// Create the container per the spec. Returns the daemon-assigned
    /// container id. The container is NOT yet started.
    async fn create_container(&self, spec: ContainerSpec) -> Result<String, AppError>;

    /// Start a previously-created container.
    async fn start_container(&self, id: &str) -> Result<(), AppError>;

    /// Graceful stop with a timeout. SIGTERM, then SIGKILL after the
    /// grace period. The runtime MUST honour the timeout — V0 deploy
    /// hangs are the #1 user pain.
    async fn stop_container(&self, id: &str, timeout: Duration) -> Result<(), AppError>;

    /// Remove the container (and its anonymous volumes). Must be called
    /// after `stop_container` returns; bollard will force-stop if not.
    async fn remove_container(&self, id: &str) -> Result<(), AppError>;

    /// Probe the container's health. HTTP if `path` starts with `/`,
    /// TCP otherwise. Retries per the runtime's policy (V0: 3 retries,
    /// exponential backoff up to 5s total).
    async fn healthcheck(
        &self,
        id: &str,
        port: u16,
        path: &str,
        timeout: Duration,
    ) -> Result<HealthResult, AppError>;
}

/// Spec for a native (systemd-managed) app. Used by the native runtime
/// adapter to generate systemd unit files and manage the app lifecycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NativeUnitSpec {
    /// App name (DNS-label safe).
    pub name: String,
    /// Absolute path to the binary.
    pub bin_path: String,
    /// systemd `ExecStart` template. Supports `{port}` substitution.
    pub exec_start: String,
    /// Port the app listens on.
    pub port: u16,
    /// Environment variables.
    pub env: Vec<(String, String)>,
    /// HTTP health check path (e.g. `/health`).
    pub health_path: String,
    /// System user to run as (e.g. `sovereign-myapp`).
    pub user: String,
    /// App data directory (e.g. `/opt/sovereign/apps/myapp/data`).
    pub data_dir: String,
}

/// The systemd-native runtime port. Manages systemd units for native
/// (non-container) apps.
#[async_trait]
pub trait SystemdNativePort: Send + Sync {
    /// Install and start a native app's systemd unit.
    async fn install_unit(&self, spec: &NativeUnitSpec) -> Result<(), AppError>;

    /// Stop and remove a native app's systemd unit.
    async fn remove_unit(&self, app_name: &str) -> Result<(), AppError>;

    /// Restart a native app's systemd unit.
    async fn restart_unit(&self, app_name: &str) -> Result<(), AppError>;

    /// Check if a native app's systemd unit is active.
    async fn is_active(&self, app_name: &str) -> Result<bool, AppError>;

    /// Allocate an available port in the ephemeral range.
    async fn allocate_port(&self) -> Result<u16, AppError>;

    /// Release a previously allocated port.
    async fn release_port(&self, port: u16) -> Result<(), AppError>;
}
