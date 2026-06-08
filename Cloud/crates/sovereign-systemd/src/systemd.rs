// Systemd wrapper. Wraps `systemctl enable/disable/start/stop/restart`
// with idempotent checks. All operations are async.
//
// Per `docs/phase-0.6.md` §S1, systemd is called via
// `tokio::process::Command::new("systemctl")`.

use tracing::{debug, instrument, warn};

use sovereign_core::error::AppError;

/// Return the current state of a systemd unit.
/// Returns one of: "active", "inactive", "failed", "activating",
/// "deactivating", "not-found", or "unknown".
#[instrument(skip_all, fields(unit))]
pub async fn unit_state(unit: &str) -> Result<String, AppError> {
    let output = tokio::process::Command::new("systemctl")
        .args(["is-active", unit])
        .output()
        .await
        .map_err(|e| AppError::upstream(format!("systemctl is-active failed: {e}")))?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() {
        return Ok("unknown".to_string());
    }

    Ok(stdout)
}

/// Check whether a unit is enabled (starts on boot).
#[instrument(skip_all, fields(unit))]
pub async fn is_enabled(unit: &str) -> Result<bool, AppError> {
    let output = tokio::process::Command::new("systemctl")
        .args(["is-enabled", unit])
        .output()
        .await
        .map_err(|e| AppError::upstream(format!("systemctl is-enabled failed: {e}")))?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(stdout == "enabled")
}

/// Enable and start a systemd unit.
#[instrument(skip_all, fields(unit))]
pub async fn enable_now(unit: &str) -> Result<(), AppError> {
    let output = tokio::process::Command::new("systemctl")
        .args(["enable", "--now", unit])
        .output()
        .await
        .map_err(|e| AppError::upstream(format!("systemctl enable --now failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        warn!(unit, stderr = %stderr, "systemctl enable --now failed");
        return Err(AppError::upstream(format!(
            "systemctl enable --now {unit} failed (exit {}): {stderr}",
            output.status.code().unwrap_or(-1)
        )));
    }

    debug!(unit, "systemctl enable --now completed");
    Ok(())
}

/// Disable and stop a systemd unit.
#[instrument(skip_all, fields(unit))]
pub async fn disable_stop(unit: &str) -> Result<(), AppError> {
    let output = tokio::process::Command::new("systemctl")
        .args(["disable", "--now", unit])
        .output()
        .await
        .map_err(|e| AppError::upstream(format!("systemctl disable --now failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        warn!(unit, stderr = %stderr, "systemctl disable --now failed");
        return Err(AppError::upstream(format!(
            "systemctl disable --now {unit} failed (exit {}): {stderr}",
            output.status.code().unwrap_or(-1)
        )));
    }

    debug!(unit, "systemctl disable --now completed");
    Ok(())
}

/// Restart a systemd unit.
#[instrument(skip_all, fields(unit))]
pub async fn restart(unit: &str) -> Result<(), AppError> {
    let output = tokio::process::Command::new("systemctl")
        .args(["restart", unit])
        .output()
        .await
        .map_err(|e| AppError::upstream(format!("systemctl restart failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        warn!(unit, stderr = %stderr, "systemctl restart failed");
        return Err(AppError::upstream(format!(
            "systemctl restart {unit} failed (exit {}): {stderr}",
            output.status.code().unwrap_or(-1)
        )));
    }

    debug!(unit, "systemctl restart completed");
    Ok(())
}

/// Return the PID of the main process for a unit, if active.
#[instrument(skip_all, fields(unit))]
pub async fn main_pid(unit: &str) -> Result<Option<u32>, AppError> {
    let output = tokio::process::Command::new("systemctl")
        .args(["show", unit, "--property=MainPID", "--value"])
        .output()
        .await
        .map_err(|e| AppError::upstream(format!("systemctl show failed: {e}")))?;

    if !output.status.success() {
        return Ok(None);
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    match stdout.parse::<u32>() {
        Ok(0) => Ok(None),
        Ok(pid) => Ok(Some(pid)),
        Err(_) => Ok(None),
    }
}

/// Return the elapsed uptime for a unit in seconds, if active.
#[instrument(skip_all, fields(unit))]
pub async fn uptime_secs(unit: &str) -> Result<Option<u64>, AppError> {
    let output = tokio::process::Command::new("systemctl")
        .args(["show", unit, "--property=ActiveEnterTimestamp", "--value"])
        .output()
        .await
        .map_err(|e| AppError::upstream(format!("systemctl show failed: {e}")))?;

    if !output.status.success() {
        return Ok(None);
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() || stdout == "n/a" {
        return Ok(None);
    }

    // ActiveEnterTimestamp is like "Mon 2026-01-01 12:00:00 UTC"
    // Parse it with chrono
    use chrono::NaiveDateTime;
    let parsed = NaiveDateTime::parse_from_str(&stdout, "%Y-%m-%d %H:%M:%S");
    match parsed {
        Ok(dt) => {
            let now = chrono::Utc::now().naive_utc();
            let diff = (now - dt).num_seconds().max(0) as u64;
            Ok(Some(diff))
        }
        Err(_) => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unit_state_check_does_not_panic() {
        let _ = unit_state("nonexistent-unit-12345").await;
    }

    #[tokio::test]
    async fn is_enabled_check_does_not_panic() {
        let _ = is_enabled("nonexistent-unit-12345").await;
    }
}
