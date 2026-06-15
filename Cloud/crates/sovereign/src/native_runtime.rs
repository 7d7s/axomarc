// P5: NativeRuntime adapter — bridges sovereign-core's SystemdNativePort
// trait with the sovereign-systemd native module (unit lifecycle, port
// allocator, binary extraction). Used by webhook dispatch and the CLI
// when deploy_mode == Native.

use async_trait::async_trait;
use sovereign_core::error::AppError;
use sovereign_core::ports::NativeUnitSpec;
use sovereign_core::ports::SystemdNativePort;
use tracing::{debug, instrument};

/// Systemd-native runtime adapter. Implements `SystemdNativePort` by
/// delegating to `sovereign_systemd::native::*`.
pub struct NativeRuntime;

impl NativeRuntime {
    pub fn new() -> Self {
        Self
    }
}

impl Default for NativeRuntime {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SystemdNativePort for NativeRuntime {
    #[instrument(skip(self, spec), fields(app = %spec.name))]
    async fn install_unit(&self, spec: &NativeUnitSpec) -> Result<(), AppError> {
        // Resolve env vars
        let env: Vec<(String, String)> = spec.env.clone();

        // Substitute {port} in exec_start template
        let exec_start = spec.exec_start.replace("{port}", &spec.port.to_string());

        sovereign_systemd::native::install_native_unit(
            &spec.name,
            &exec_start,
            spec.port,
            &spec.user,
            &env,
        )
        .await
    }

    #[instrument(skip(self))]
    async fn remove_unit(&self, app_name: &str) -> Result<(), AppError> {
        sovereign_systemd::native::remove_native_unit(app_name).await
    }

    #[instrument(skip(self))]
    async fn restart_unit(&self, app_name: &str) -> Result<(), AppError> {
        sovereign_systemd::native::restart_native_unit(app_name).await
    }

    #[instrument(skip(self))]
    async fn is_active(&self, app_name: &str) -> Result<bool, AppError> {
        sovereign_systemd::native::is_native_active(app_name).await
    }

    #[instrument(skip(self))]
    async fn allocate_port(&self) -> Result<u16, AppError> {
        sovereign_systemd::native::allocate_port().await
    }

    #[instrument(skip(self))]
    async fn release_port(&self, port: u16) -> Result<(), AppError> {
        sovereign_systemd::native::release_port(port).await
    }
}

/// High-level native deploy: extracts binary from a .sov archive,
/// allocates a port, installs the systemd unit, and returns the
/// port the app is listening on.
///
/// This is the entry point used by `dispatch_native` in the webhook
/// handler and by `sovereign deploy --mode native`.
#[instrument(skip_all, fields(app_name, archive_path = %archive_path.display()))]
pub async fn native_deploy(
    app_name: &str,
    archive_path: &std::path::Path,
    binary_path: &str,
    exec_start_template: &str,
    env: &[(String, String)],
    health_path: &str,
) -> Result<NativeDeployResult, AppError> {
    let runtime = NativeRuntime::new();

    // 1. Ensure app directories exist
    sovereign_systemd::native::ensure_app_dirs(app_name).await?;

    // 2. Extract binary from .sov archive
    let bin_path =
        sovereign_systemd::native::extract_binary(app_name, archive_path, binary_path).await?;
    let bin_path_str = bin_path.to_string_lossy().to_string();

    // 3. Allocate port
    let port = runtime.allocate_port().await?;
    debug!(port, "allocated port for native deploy");

    // 4. Create systemd user name
    let user = format!("sovereign-{app_name}");

    // 5. Build NativeUnitSpec
    let spec = NativeUnitSpec {
        name: app_name.to_string(),
        bin_path: bin_path_str.clone(),
        exec_start: exec_start_template.to_string(),
        port,
        env: env.to_vec(),
        health_path: health_path.to_string(),
        user: user.clone(),
        data_dir: sovereign_systemd::native::app_data_dir(app_name)
            .to_string_lossy()
            .to_string(),
    };

    // 6. Install + start the systemd unit
    runtime.install_unit(&spec).await?;

    debug!(app = app_name, port, "native deploy complete");

    Ok(NativeDeployResult {
        port,
        bin_path: bin_path_str,
        user,
    })
}

/// Result of a successful native deploy.
#[derive(Debug, Clone)]
pub struct NativeDeployResult {
    pub port: u16,
    pub bin_path: String,
    pub user: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_runtime_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<NativeRuntime>();
    }

    #[test]
    fn native_deploy_result_fields() {
        let r = NativeDeployResult {
            port: 49152,
            bin_path: "/opt/sovereign/apps/myapp/bin/myapp".to_string(),
            user: "sovereign-myapp".to_string(),
        };
        assert_eq!(r.port, 49152);
        assert!(r.bin_path.contains("myapp"));
    }
}
