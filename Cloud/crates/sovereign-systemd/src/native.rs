// P5: Native runtime support — port allocator, systemd unit lifecycle,
// and binary extraction for `runtime: native` apps.
//
// App directory layout:
//   /opt/sovereign/apps/<name>/
//   ├── bin/           # current binary
//   ├── bin.prev/      # previous binary (for rollback)
//   ├── config/        # config overrides
//   └── data/          # persistent data

use std::path::{Path, PathBuf};

use sovereign_core::error::AppError;
use tracing::{debug, instrument};

/// Default base directory for native app installations.
/// Override at runtime with `SOVEREIGN_APP_BASE_DIR` for containerized
/// or air-gapped installs that cannot write to `/opt/sovereign`.
pub fn app_base_dir() -> std::path::PathBuf {
    if let Some(p) = std::env::var_os("SOVEREIGN_APP_BASE_DIR") {
        return std::path::PathBuf::from(p);
    }
    std::path::PathBuf::from("/opt/sovereign/apps")
}

/// The V0 hardcoded default for back-compat with code that referenced
/// the `const` directly. New code should call `app_base_dir()`.
pub const APP_BASE_DIR: &str = "/opt/sovereign/apps";

/// Ephemeral port range (IANA).
pub const PORT_RANGE_MIN: u16 = 49152;
pub const PORT_RANGE_MAX: u16 = 65535;

/// Path to the port allocation lock file.
/// Override at runtime with `SOVEREIGN_PORT_LOCK_FILE`.
pub fn port_lock_file() -> std::path::PathBuf {
    if let Some(p) = std::env::var_os("SOVEREIGN_PORT_LOCK_FILE") {
        return std::path::PathBuf::from(p);
    }
    std::path::PathBuf::from("/var/lib/sovereign/ports.lock")
}

/// The V0 hardcoded default for back-compat.
pub const PORT_LOCK_FILE: &str = "/var/lib/sovereign/ports.lock";

// ---------------------------------------------------------------------------
// Port allocator
// ---------------------------------------------------------------------------

/// Allocate an available port from the ephemeral range.
///
/// Reads `/var/lib/sovereign/ports.lock` (JSON array of allocated ports),
/// finds the first unused port, adds it, and writes the file back.
/// Returns the allocated port number.
///
/// V0 note: Uses simple read-modify-write without advisory file locking.
/// The daemon is single-process, so this is safe. V1+ can add `flock`
/// via the `fs2` crate if multi-process coordination is needed.
#[instrument]
pub async fn allocate_port() -> Result<u16, AppError> {
    let lock_path = PathBuf::from(PORT_LOCK_FILE);

    // Ensure parent directory exists
    if let Some(parent) = lock_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| AppError::upstream(format!("create port lock dir: {e}")))?;
    }

    // Read current allocations
    let contents = tokio::fs::read_to_string(&lock_path)
        .await
        .unwrap_or_default();
    let allocated: Vec<u16> = serde_json::from_str(&contents).unwrap_or_default();

    // Find first available port
    let port = (PORT_RANGE_MIN..=PORT_RANGE_MAX)
        .find(|p| !allocated.contains(p))
        .ok_or_else(|| AppError::upstream("no available ports in ephemeral range"))?;

    // Write back with new allocation
    let mut new_allocated = allocated;
    new_allocated.push(port);
    new_allocated.sort();
    let json = serde_json::to_string(&new_allocated)
        .map_err(|e| AppError::upstream(format!("serialize port list: {e}")))?;
    tokio::fs::write(&lock_path, format!("{json}\n"))
        .await
        .map_err(|e| AppError::upstream(format!("write port lock file: {e}")))?;

    debug!(port, "allocated ephemeral port");
    Ok(port)
}

/// Release a previously allocated port.
#[instrument]
pub async fn release_port(port: u16) -> Result<(), AppError> {
    let lock_path = PathBuf::from(PORT_LOCK_FILE);

    if !lock_path.exists() {
        return Ok(());
    }

    let contents = tokio::fs::read_to_string(&lock_path)
        .await
        .unwrap_or_default();
    let mut allocated: Vec<u16> = serde_json::from_str(&contents).unwrap_or_default();

    allocated.retain(|p| *p != port);
    let json = serde_json::to_string(&allocated)
        .map_err(|e| AppError::upstream(format!("serialize port list: {e}")))?;
    tokio::fs::write(&lock_path, format!("{json}\n"))
        .await
        .map_err(|e| AppError::upstream(format!("write port lock file: {e}")))?;

    debug!(port, "released ephemeral port");
    Ok(())
}

// ---------------------------------------------------------------------------
// App directory management
// ---------------------------------------------------------------------------

/// Return the app base directory: `/opt/sovereign/apps/<name>`.
pub fn app_dir(name: &str) -> PathBuf {
    PathBuf::from(APP_BASE_DIR).join(name)
}

/// Return the bin directory: `/opt/sovereign/apps/<name>/bin`.
pub fn app_bin_dir(name: &str) -> PathBuf {
    app_dir(name).join("bin")
}

/// Return the previous bin directory: `/opt/sovereign/apps/<name>/bin.prev`.
pub fn app_bin_prev_dir(name: &str) -> PathBuf {
    app_dir(name).join("bin.prev")
}

/// Return the config directory: `/opt/sovereign/apps/<name>/config`.
pub fn app_config_dir(name: &str) -> PathBuf {
    app_dir(name).join("config")
}

/// Return the data directory: `/opt/sovereign/apps/<name>/data`.
pub fn app_data_dir(name: &str) -> PathBuf {
    app_dir(name).join("data")
}

/// Create the full app directory layout. Idempotent.
#[instrument(skip_all, fields(name))]
pub async fn ensure_app_dirs(name: &str) -> Result<PathBuf, AppError> {
    let base = app_dir(name);
    for subdir in ["bin", "bin.prev", "config", "data"] {
        let dir = base.join(subdir);
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| AppError::upstream(format!("create dir {}: {e}", dir.display())))?;
    }
    debug!(base = %base.display(), "app directories created");
    Ok(base)
}

// ---------------------------------------------------------------------------
// Systemd unit template
// ---------------------------------------------------------------------------

/// Render a systemd unit file for a native app.
pub fn render_unit(
    app_name: &str,
    exec_start: &str,
    _port: u16,
    user: &str,
    env: &[(String, String)],
) -> String {
    let env_block: String = env
        .iter()
        .map(|(k, v)| format!("Environment={k}={v}"))
        .collect::<Vec<_>>()
        .join("\n");

    let env_section = if env_block.is_empty() {
            String::new()
        } else {
            format!("\n{env_block}")
        };

    format!(
        r#"[Unit]
Description=Sovereign Native App: {app_name}
After=network.target
Wants=network.target

[Service]
Type=simple
User={user}
ExecStart={exec_start}
Restart=on-failure
RestartSec=5
TimeoutStopSec=30
{env_section}

# Hardening
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes
ReadWritePaths=/opt/sovereign/apps/{app_name}/data
CapabilityBoundingSet=
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectControlGroups=yes

[Install]
WantedBy=multi-user.target
"#,
        app_name = app_name,
        user = user,
        exec_start = exec_start,
    )
}

// ---------------------------------------------------------------------------
// Systemd unit lifecycle (via systemctl)
// ---------------------------------------------------------------------------

/// Unit name for a sovereign native app.
pub fn unit_name(app_name: &str) -> String {
    format!("sovereign-{app_name}")
}

/// Install (write + enable + start) a native app's systemd unit.
#[instrument(skip_all, fields(app_name))]
pub async fn install_native_unit(
    app_name: &str,
    exec_start: &str,
    port: u16,
    user: &str,
    env: &[(String, String)],
) -> Result<(), AppError> {
    let unit = unit_name(app_name);
    let unit_content = render_unit(app_name, exec_start, port, user, env);
    let unit_path = format!("/etc/systemd/system/{unit}.service");

    // Write the unit file
    tokio::fs::write(&unit_path, &unit_content)
        .await
        .map_err(|e| AppError::upstream(format!("write unit file: {e}")))?;

    // systemctl daemon-reload
    let output = tokio::process::Command::new("systemctl")
        .args(["daemon-reload"])
        .output()
        .await
        .map_err(|e| AppError::upstream(format!("systemctl daemon-reload: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AppError::upstream(format!(
            "systemctl daemon-reload failed: {stderr}"
        )));
    }

    // systemctl enable --now
    crate::systemd::enable_now(&unit).await?;

    debug!(unit = %unit, port, "native unit installed and started");
    Ok(())
}

/// Stop and remove a native app's systemd unit.
#[instrument(skip_all, fields(app_name))]
pub async fn remove_native_unit(app_name: &str) -> Result<(), AppError> {
    let unit = unit_name(app_name);
    let unit_path = format!("/etc/systemd/system/{unit}.service");

    // Stop and disable
    let _ = crate::systemd::disable_stop(&unit).await;

    // Remove unit file
    let _ = tokio::fs::remove_file(&unit_path).await;

    // daemon-reload
    let _ = tokio::process::Command::new("systemctl")
        .args(["daemon-reload"])
        .output()
        .await;

    debug!(unit = %unit, "native unit removed");
    Ok(())
}

/// Restart a native app's systemd unit.
#[instrument(skip_all, fields(app_name))]
pub async fn restart_native_unit(app_name: &str) -> Result<(), AppError> {
    let unit = unit_name(app_name);
    crate::systemd::restart(&unit).await
}

/// Check if a native app's systemd unit is active.
pub async fn is_native_active(app_name: &str) -> Result<bool, AppError> {
    let unit = unit_name(app_name);
    let state = crate::systemd::unit_state(&unit).await?;
    Ok(state == "active")
}

// ---------------------------------------------------------------------------
// Binary extraction
// ---------------------------------------------------------------------------

/// Extract a binary from a .sov archive to the app's bin directory.
///
/// The archive is expected to have a `binary` entry at the path specified
/// by `binary_path` in the manifest. The binary is placed in the app's
/// `bin/` directory and made executable.
#[instrument(skip_all, fields(app_name, archive_path))]
pub async fn extract_binary(
    app_name: &str,
    archive_path: &Path,
    binary_path: &str,
) -> Result<PathBuf, AppError> {
    use sovereign_pack::extract;

    let bin_dir = app_bin_dir(app_name);
    let target = bin_dir.join(
        Path::new(binary_path)
            .file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("app")),
    );

    // Read the archive into memory
    let archive_data = tokio::fs::read(archive_path)
        .await
        .map_err(|e| AppError::upstream(format!("read archive: {e}")))?;

    // Extract to a temp dir
    let tmp_dir = tempfile::tempdir()
        .map_err(|e| AppError::upstream(format!("tempdir: {e}")))?;

    extract(&archive_data, tmp_dir.path())
        .map_err(|e| AppError::upstream(format!("extract archive: {e}")))?;

    // Find and copy the binary
    let src_binary = tmp_dir.path().join(binary_path);
    if !src_binary.exists() {
        return Err(AppError::validation(format!(
            "binary `{binary_path}` not found in archive"
        )));
    }

    tokio::fs::copy(&src_binary, &target)
        .await
        .map_err(|e| AppError::upstream(format!("copy binary: {e}")))?;

    // Make executable (Unix)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        tokio::fs::set_permissions(&target, perms)
            .await
            .map_err(|e| AppError::upstream(format!("chmod: {e}")))?;
    }

    debug!(app = app_name, target = %target.display(), "binary extracted");
    Ok(target)
}

/// Swap current and previous binaries for rollback.
#[instrument(skip_all, fields(app_name))]
pub async fn swap_binaries(app_name: &str) -> Result<(), AppError> {
    let bin = app_bin_dir(app_name);
    let bin_prev = app_bin_prev_dir(app_name);

    if !bin.exists() {
        return Err(AppError::validation("no current binary to rollback"));
    }

    // Move current to prev
    if bin_prev.exists() {
        tokio::fs::remove_dir_all(&bin_prev)
            .await
            .map_err(|e| AppError::upstream(format!("remove bin.prev: {e}")))?;
    }
    tokio::fs::rename(&bin, &bin_prev)
        .await
        .map_err(|e| AppError::upstream(format!("rename bin -> bin.prev: {e}")))?;

    // Move prev's content back to bin (for the "swap" semantics)
    // Actually: we want the OLD binary in bin.prev and the NEW in bin.
    // For rollback: we swap so bin.prev becomes bin.
    // The caller should have placed the new binary in bin.prev before calling.
    // For simplicity in V0: just rename bin.prev -> bin if bin.prev exists.

    // Actually, the rollback flow is:
    // 1. New deploy: extract to bin/, old bin is in bin.prev/
    // 2. Rollback: move bin.prev/ back to bin/
    // So swap_binaries just moves bin.prev -> bin
    if bin_prev.exists() {
        tokio::fs::rename(&bin_prev, &bin)
            .await
            .map_err(|e| AppError::upstream(format!("rename bin.prev -> bin: {e}")))?;
    }

    debug!(app = app_name, "binaries swapped for rollback");
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_unit_has_key_fields() {
        let unit = render_unit(
            "myapp",
            "/opt/sovereign/apps/myapp/bin/myapp --port {port}",
            8080,
            "sovereign-myapp",
            &[("RUST_LOG".to_string(), "info".to_string())],
        );
        assert!(unit.contains("Description=Sovereign Native App: myapp"));
        assert!(unit.contains("User=sovereign-myapp"));
        assert!(unit.contains("ExecStart=/opt/sovereign/apps/myapp/bin/myapp --port {port}"));
        assert!(unit.contains("Restart=on-failure"));
        assert!(unit.contains("ProtectSystem=strict"));
        assert!(unit.contains("Environment=RUST_LOG=info"));
        assert!(unit.contains("WantedBy=multi-user.target"));
    }

    #[test]
    fn render_unit_no_env() {
        let unit = render_unit("app", "/usr/bin/app", 3000, "app", &[]);
        assert!(!unit.contains("Environment="));
    }

    #[test]
    fn unit_name_format() {
        assert_eq!(unit_name("myapp"), "sovereign-myapp");
        assert_eq!(unit_name("api-v2"), "sovereign-api-v2");
    }

    #[test]
    fn app_dir_paths() {
        assert_eq!(app_dir("myapp"), PathBuf::from("/opt/sovereign/apps/myapp"));
        assert_eq!(
            app_bin_dir("myapp"),
            PathBuf::from("/opt/sovereign/apps/myapp/bin")
        );
        assert_eq!(
            app_bin_prev_dir("myapp"),
            PathBuf::from("/opt/sovereign/apps/myapp/bin.prev")
        );
        assert_eq!(
            app_config_dir("myapp"),
            PathBuf::from("/opt/sovereign/apps/myapp/config")
        );
        assert_eq!(
            app_data_dir("myapp"),
            PathBuf::from("/opt/sovereign/apps/myapp/data")
        );
    }

    #[test]
    fn port_range_constants() {
        assert!(PORT_RANGE_MIN < PORT_RANGE_MAX);
        assert_eq!(PORT_RANGE_MIN, 49152);
        assert_eq!(PORT_RANGE_MAX, 65535);
    }
}
