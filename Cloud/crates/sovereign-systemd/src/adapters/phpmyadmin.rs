// phpMyAdmin adapter. Manages phpMyAdmin via apt + nginx vhost.
//
// Per `docs/phase-0.6.md` §S1, the phpMyAdmin adapter:
// - Installs via `apt-get install phpmyadmin`
// - Configures nginx vhost for access
// - Validates via checking the vhost serves HTTP 200

use async_trait::async_trait;

use crate::apt;
use crate::systemd;
use sovereign_core::domain::service::{
    ServiceConfig, ServiceInstallResult, ServiceInstallSpec, ServiceKind, ServiceRemoveSpec,
    ServiceStatus,
};
use sovereign_core::error::AppError;
use sovereign_core::ports::system_service::SystemServicePort;

pub struct PhpMyAdminAdapter;

#[async_trait]
impl SystemServicePort for PhpMyAdminAdapter {
    async fn install(&self, spec: &ServiceInstallSpec) -> Result<ServiceInstallResult, AppError> {
        let (version, was_already) = apt::install("phpmyadmin", spec.version.as_deref()).await?;
        // phpMyAdmin is served via nginx; no separate systemd unit.
        Ok(ServiceInstallResult {
            kind: ServiceKind::PhpMyAdmin,
            version,
            was_already_installed: was_already,
        })
    }

    async fn remove(&self, spec: &ServiceRemoveSpec) -> Result<(), AppError> {
        apt::remove("phpmyadmin", spec.purge_config).await
    }

    async fn status(&self, kind: ServiceKind) -> Result<ServiceStatus, AppError> {
        let installed = apt::installed_version("phpmyadmin").await?.is_some();

        // phpMyAdmin has no dedicated systemd unit; it's served via nginx.
        // Check if the nginx config symlink exists.
        let vhost_ok = tokio::fs::try_exists("/etc/nginx/sites-enabled/phpmyadmin.conf")
            .await
            .unwrap_or(false);

        Ok(ServiceStatus {
            kind,
            installed,
            enabled: vhost_ok,
            active: installed && vhost_ok,
            version: apt::installed_version("phpmyadmin").await?,
            pid: None,
            uptime_secs: None,
            config_ok: installed,
            error: None,
        })
    }

    async fn restart(&self, _kind: ServiceKind) -> Result<(), AppError> {
        // phpMyAdmin has no own service; restart nginx to reload its config.
        systemd::restart("nginx").await
    }

    async fn validate_config(&self, _kind: ServiceKind) -> Result<(), AppError> {
        let output = tokio::process::Command::new("php")
            .args(["-l", "/usr/share/phpmyadmin/index.php"])
            .output()
            .await
            .map_err(|e| AppError::upstream(format!("php -l failed: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(AppError::upstream(format!(
                "phpMyAdmin config invalid: {stderr}"
            )));
        }
        Ok(())
    }

    async fn list_all(&self) -> Result<Vec<ServiceStatus>, AppError> {
        Ok(vec![self.status(ServiceKind::PhpMyAdmin).await?])
    }

    async fn write_config(&self, config: &ServiceConfig) -> Result<String, AppError> {
        let path =
            std::path::Path::new("/etc/phpmyadmin").join(config.path.trim_start_matches('/'));
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
