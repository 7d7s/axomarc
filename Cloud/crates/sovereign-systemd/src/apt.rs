// APT wrapper. Wraps `apt-get install/remove/purge` with
// idempotent checks. All operations are async (tokio::process::Command).
//
// Per `docs/phase-0.6.md` §S1, apt is called via
// `tokio::process::Command::new("apt-get")` with `-y` flag.
// Non-Ubuntu platforms return `AppError::upstream("apt not available")`.

use std::time::Instant;

use tracing::{debug, instrument, warn};

use sovereign_core::error::AppError;

/// Check whether `apt-get` is available on this system.
#[instrument]
pub async fn is_apt_available() -> bool {
    let output = tokio::process::Command::new("apt-get")
        .arg("--version")
        .output()
        .await;
    match output {
        Ok(o) => o.status.success(),
        Err(e) => {
            debug!("apt-get not available: {e}");
            false
        }
    }
}

/// Return the installed version of a package, or `None` if not installed.
#[instrument(skip_all, fields(package))]
pub async fn installed_version(package: &str) -> Result<Option<String>, AppError> {
    let output = tokio::process::Command::new("dpkg-query")
        .args(["-W", "-f=${Version}", package])
        .output()
        .await
        .map_err(|e| AppError::upstream(format!("dpkg-query failed: {e}")))?;

    if output.status.success() {
        let v = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if v.is_empty() || v.contains("no packages found") {
            Ok(None)
        } else {
            Ok(Some(v))
        }
    } else {
        Ok(None)
    }
}

/// Install a package via apt-get. Returns `(version, was_already_installed)`.
#[instrument(skip_all, fields(package, version))]
pub async fn install(package: &str, version: Option<&str>) -> Result<(String, bool), AppError> {
    let already = installed_version(package).await?;
    if let Some(ref v) = already {
        let v_str: &str = v.as_str();
        if version.is_none() || version == Some(v_str) {
            debug!(package, version = v_str, "already installed");
            return Ok((v.clone(), true));
        }
    }

    let start = Instant::now();
    let pkg_arg = match version {
        Some(v) => format!("{package}={v}"),
        None => package.to_string(),
    };

    let output = tokio::process::Command::new("apt-get")
        .args(["install", "-y", "--no-install-recommends", &pkg_arg])
        .output()
        .await
        .map_err(|e| AppError::upstream(format!("apt-get install failed: {e}")))?;

    let elapsed = start.elapsed().as_millis();
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        warn!(package, stderr = %stderr, elapsed_ms = elapsed, "apt-get install failed");
        return Err(AppError::upstream(format!(
            "apt-get install {package} failed (exit {}): {stderr}",
            output.status.code().unwrap_or(-1)
        )));
    }

    let installed: String = installed_version(package)
        .await?
        .unwrap_or_else(|| "unknown".to_string());

    debug!(package, version = %installed, elapsed_ms = elapsed, "apt-get install completed");

    Ok((installed, false))
}

/// Remove (purge) a package via apt-get.
#[instrument(skip_all, fields(package, purge))]
pub async fn remove(package: &str, purge: bool) -> Result<(), AppError> {
    let start = Instant::now();
    let action = if purge { "purge" } else { "remove" };

    let output = tokio::process::Command::new("apt-get")
        .args([action, "-y", package])
        .output()
        .await
        .map_err(|e| AppError::upstream(format!("apt-get {action} failed: {e}")))?;

    let elapsed = start.elapsed().as_millis();
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        warn!(package, stderr = %stderr, elapsed_ms = elapsed, "apt-get {action} failed");
        return Err(AppError::upstream(format!(
            "apt-get {action} {package} failed (exit {}): {stderr}",
            output.status.code().unwrap_or(-1)
        )));
    }

    debug!(
        package,
        action,
        elapsed_ms = elapsed,
        "apt-get {action} completed"
    );
    Ok(())
}

/// Update apt package lists.
#[instrument]
pub async fn update() -> Result<(), AppError> {
    let output = tokio::process::Command::new("apt-get")
        .args(["update", "-qq"])
        .output()
        .await
        .map_err(|e| AppError::upstream(format!("apt-get update failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        warn!(stderr = %stderr, "apt-get update failed");
        return Err(AppError::upstream(format!(
            "apt-get update failed (exit {}): {stderr}",
            output.status.code().unwrap_or(-1)
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn apt_available_check_does_not_panic() {
        // This test does not assert the value — it only verifies the
        // async path compiles and runs without panicking.
        let _ = is_apt_available().await;
    }
}
