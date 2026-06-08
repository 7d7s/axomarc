// vsftpd adapter. Manages vsftpd via apt + systemd.
//
// Per `docs/phase-0.6.md` §S1, the vsftpd adapter:
// - Installs via `apt-get install vsftpd`
// - Validates via checking config file
// - Restarts via `systemctl restart vsftpd`
// - Configures virtual users for per-app jail (S5 scope)

use async_trait::async_trait;

use crate::apt;
use crate::systemd;
use sovereign_core::domain::service::{
    ServiceConfig, ServiceInstallResult, ServiceInstallSpec, ServiceKind, ServiceRemoveSpec,
    ServiceStatus,
};
use sovereign_core::error::AppError;
use sovereign_core::ports::system_service::SystemServicePort;

pub struct VsFtpdAdapter;

#[async_trait]
impl SystemServicePort for VsFtpdAdapter {
    async fn install(&self, spec: &ServiceInstallSpec) -> Result<ServiceInstallResult, AppError> {
        let (version, was_already) = apt::install("vsftpd", spec.version.as_deref()).await?;
        if !spec.no_start {
            systemd::enable_now("vsftpd").await?;
        }
        Ok(ServiceInstallResult {
            kind: ServiceKind::VsFtpd,
            version,
            was_already_installed: was_already,
        })
    }

    async fn remove(&self, spec: &ServiceRemoveSpec) -> Result<(), AppError> {
        systemd::disable_stop("vsftpd").await.ok();
        apt::remove("vsftpd", spec.purge_config).await
    }

    async fn status(&self, kind: ServiceKind) -> Result<ServiceStatus, AppError> {
        let installed = apt::installed_version("vsftpd").await?.is_some();
        let enabled = systemd::is_enabled("vsftpd").await.unwrap_or(false);
        let state = systemd::unit_state("vsftpd").await.unwrap_or_default();
        let active = state == "active";
        let pid = systemd::main_pid("vsftpd").await.unwrap_or(None);
        let uptime = systemd::uptime_secs("vsftpd").await.unwrap_or(None);
        let version = apt::installed_version("vsftpd").await?;

        Ok(ServiceStatus {
            kind,
            installed,
            enabled,
            active,
            version,
            pid,
            uptime_secs: uptime,
            config_ok: active,
            error: if state == "failed" {
                Some("unit in failed state".to_string())
            } else {
                None
            },
        })
    }

    async fn restart(&self, _kind: ServiceKind) -> Result<(), AppError> {
        systemd::restart("vsftpd").await
    }

    async fn validate_config(&self, _kind: ServiceKind) -> Result<(), AppError> {
        let output = tokio::process::Command::new("vsftpd")
            .args(["conf=/etc/vsftpd.conf"])
            .output()
            .await
            .map_err(|e| AppError::upstream(format!("vsftpd config check failed: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(AppError::upstream(format!(
                "vsftpd config invalid: {stderr}"
            )));
        }
        Ok(())
    }

    async fn list_all(&self) -> Result<Vec<ServiceStatus>, AppError> {
        Ok(vec![self.status(ServiceKind::VsFtpd).await?])
    }

    async fn write_config(&self, config: &ServiceConfig) -> Result<String, AppError> {
        let path = std::path::Path::new("/etc").join(config.path.trim_start_matches('/'));
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| AppError::upstream(format!("mkdir failed: {e}")))?;
        }
        tokio::fs::write(&path, config.content.as_bytes())
            .await
            .map_err(|e| AppError::upstream(format!("write failed: {e}")))?;

        use sha2::{Digest, Sha256};
        let hash = Sha256::digest(config.content.as_bytes());
        Ok(format!("{hash:x}"))
    }
}
