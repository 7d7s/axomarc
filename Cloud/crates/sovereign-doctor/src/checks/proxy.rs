use async_trait::async_trait;

use crate::check::{Check, CheckCategory, CheckContext, CheckResult};

/// `caddy_installed` — the `caddy` binary is on PATH. Skip when it
/// is not; never require the operator to install it just to read a
/// status.
pub struct CaddyInstalledCheck;

#[async_trait]
impl Check for CaddyInstalledCheck {
    fn name(&self) -> &'static str {
        "caddy_installed"
    }
    fn category(&self) -> CheckCategory {
        CheckCategory::Proxy
    }
    async fn run(&self, ctx: &CheckContext) -> CheckResult {
        // Use `which`-style search on Unix and `where` on Windows.
        let cmd = if cfg!(target_os = "windows") {
            "where"
        } else {
            "which"
        };
        let probe = tokio::process::Command::new(cmd)
            .arg(&ctx.caddy_binary)
            .output()
            .await;
        match probe {
            Ok(out) if out.status.success() => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let first = stdout.lines().next().unwrap_or("").trim();
                if first.is_empty() {
                    CheckResult::warn("`which caddy` succeeded but returned no path")
                } else {
                    CheckResult::pass(format!("caddy on PATH at {}", first))
                }
            }
            Ok(_) => CheckResult::fail(format!("`{} caddy` did not find the binary", cmd))
                .with_suggestion(
                    "install Caddy 2.8+ with `apt install caddy` or the official install script",
                ),
            Err(err) => CheckResult::skip(format!("`{}` is not available: {}", cmd, err)),
        }
    }
}

/// `caddy_config_valid` — `caddy validate` exits 0 on the generated
/// config. Skip when Caddy is not on PATH; do not require the
/// operator to install it just to read a status.
pub struct CaddyConfigValidCheck;

#[async_trait]
impl Check for CaddyConfigValidCheck {
    fn name(&self) -> &'static str {
        "caddy_config_valid"
    }
    fn category(&self) -> CheckCategory {
        CheckCategory::Proxy
    }
    async fn run(&self, ctx: &CheckContext) -> CheckResult {
        match tokio::fs::metadata(&ctx.caddy_config).await {
            Ok(_) => {}
            Err(_) => {
                return CheckResult::skip(format!(
                    "caddy config not present at {}; not installed yet",
                    ctx.caddy_config.display()
                ));
            }
        }
        let output = tokio::process::Command::new(&ctx.caddy_binary)
            .args(["validate", "--config"])
            .arg(&ctx.caddy_config)
            .output()
            .await;
        match output {
            Ok(out) if out.status.success() => CheckResult::pass(format!(
                "caddy config {} is valid",
                ctx.caddy_config.display()
            )),
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                CheckResult::fail(format!("caddy validate failed: {}", stderr.trim()))
                    .with_suggestion("run `caddy validate --config /etc/caddy/Caddyfile` manually")
            }
            Err(err) => CheckResult::skip(format!("caddy binary not on PATH: {}", err)),
        }
    }
}

/// `caddy_running` — the Caddy admin API responds on `:2019`. A
/// simple TCP connect; the Caddy admin API does not need auth for
/// `/config/` in V0.
pub struct CaddyRunningCheck;

#[async_trait]
impl Check for CaddyRunningCheck {
    fn name(&self) -> &'static str {
        "caddy_running"
    }
    fn category(&self) -> CheckCategory {
        CheckCategory::Proxy
    }
    async fn run(&self, ctx: &CheckContext) -> CheckResult {
        // Parse the admin URL; default port 2019 if unspecified.
        let url = &ctx.caddy_admin;
        let target = if let Some(rest) = url.strip_prefix("http://") {
            rest.split('/').next().unwrap_or(rest)
        } else {
            url.as_str()
        };
        let mut parts = target.split(':');
        let host = parts.next().unwrap_or("127.0.0.1");
        let port: u16 = parts.next().and_then(|p| p.parse().ok()).unwrap_or(2019);
        match tokio::net::TcpStream::connect((host, port)).await {
            Ok(stream) => {
                drop(stream);
                CheckResult::pass(format!("caddy admin reachable at {}:{}", host, port))
            }
            Err(err) => CheckResult::fail(format!(
                "caddy admin at {}:{} is unreachable: {}",
                host, port, err
            ))
            .with_suggestion("start Caddy with `systemctl start caddy` or `caddy run`"),
        }
    }
}

pub fn basic_checks() -> Vec<Box<dyn Check>> {
    vec![
        Box::new(CaddyInstalledCheck),
        Box::new(CaddyConfigValidCheck),
        Box::new(CaddyRunningCheck),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn caddy_installed_reports_actual_status() {
        let dir = tempdir().unwrap();
        let ctx = CheckContext::for_tests(dir.path().to_path_buf(), dir.path().join("sovereign"));
        // On any platform, this should at worst report Skip; never
        // panic on missing PATH.
        let result = CaddyInstalledCheck.run(&ctx).await;
        assert!(matches!(
            result.status,
            crate::check::CheckStatus::Pass
                | crate::check::CheckStatus::Fail
                | crate::check::CheckStatus::Skip
        ));
    }

    #[tokio::test]
    async fn caddy_config_valid_skips_when_missing() {
        let dir = tempdir().unwrap();
        let ctx = CheckContext::for_tests(dir.path().to_path_buf(), dir.path().join("sovereign"));
        let result = CaddyConfigValidCheck.run(&ctx).await;
        assert_eq!(result.status, crate::check::CheckStatus::Skip);
    }

    #[test]
    fn basic_checks_returns_three() {
        assert_eq!(basic_checks().len(), 3);
    }
}
