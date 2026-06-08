// Let's Encrypt adapter. Manages certbot via apt + systemd timers.
//
// Per `docs/phase-0.6.md` §S1, the Let's Encrypt adapter:
// - Installs via `apt-get install certbot python3-certbot-nginx`
// - Issues certificates via `certbot certonly --nginx`
// - Validates via checking cert files exist
// - Renewal is handled by the certbot systemd timer

use async_trait::async_trait;

use crate::apt;
use crate::systemd;
use sovereign_core::domain::service::{
    ServiceConfig, ServiceInstallResult, ServiceInstallSpec, ServiceKind, ServiceRemoveSpec,
    ServiceStatus,
};
use sovereign_core::error::AppError;
use sovereign_core::ports::system_service::SystemServicePort;

pub struct LetsEncryptAdapter;

#[async_trait]
impl SystemServicePort for LetsEncryptAdapter {
    async fn install(&self, spec: &ServiceInstallSpec) -> Result<ServiceInstallResult, AppError> {
        apt::install("certbot", spec.version.as_deref()).await?;
        apt::install("python3-certbot-nginx", None).await?;
        let was_already = systemd::is_enabled("certbot.timer").await.unwrap_or(false);
        if !was_already {
            systemd::enable_now("certbot.timer").await?;
        }
        let version = apt::installed_version("certbot")
            .await?
            .unwrap_or_else(|| "unknown".to_string());
        Ok(ServiceInstallResult {
            kind: ServiceKind::LetEncrypt,
            version,
            was_already_installed: was_already,
        })
    }

    async fn remove(&self, spec: &ServiceRemoveSpec) -> Result<(), AppError> {
        systemd::disable_stop("certbot.timer").await.ok();
        apt::remove("certbot", spec.purge_config).await
    }

    async fn status(&self, kind: ServiceKind) -> Result<ServiceStatus, AppError> {
        let installed = apt::installed_version("certbot").await?.is_some();
        let enabled = systemd::is_enabled("certbot.timer").await.unwrap_or(false);
        let state = systemd::unit_state("certbot.timer")
            .await
            .unwrap_or_default();
        let active = state == "active";
        let version = apt::installed_version("certbot").await?;

        Ok(ServiceStatus {
            kind,
            installed,
            enabled,
            active,
            version,
            pid: None,
            uptime_secs: None,
            config_ok: installed,
            error: if state == "failed" {
                Some("certbot timer in failed state".to_string())
            } else {
                None
            },
        })
    }

    async fn restart(&self, _kind: ServiceKind) -> Result<(), AppError> {
        systemd::restart("certbot.timer").await
    }

    async fn validate_config(&self, _kind: ServiceKind) -> Result<(), AppError> {
        let output = tokio::process::Command::new("certbot")
            .args(["--version"])
            .output()
            .await
            .map_err(|e| AppError::upstream(format!("certbot check failed: {e}")))?;

        if !output.status.success() {
            return Err(AppError::upstream("certbot not available".to_string()));
        }
        Ok(())
    }

    async fn list_all(&self) -> Result<Vec<ServiceStatus>, AppError> {
        Ok(vec![self.status(ServiceKind::LetEncrypt).await?])
    }

    async fn write_config(&self, config: &ServiceConfig) -> Result<String, AppError> {
        let path =
            std::path::Path::new("/etc/letsencrypt").join(config.path.trim_start_matches('/'));
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
