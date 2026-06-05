use async_trait::async_trait;
#[cfg(target_os = "linux")]
use std::os::unix::fs::PermissionsExt;
#[cfg(target_os = "linux")]
use std::path::Path;
use std::str::FromStr;

use crate::check::{Check, CheckCategory, CheckContext, CheckResult};
use crate::fix::{DoctorFix, FixError, FixOutcome};

/// `master_key_present` — `/var/lib/sovereign/master.key` exists and
/// is a regular file. Missing master key means no app can decrypt its
/// secrets, so this is a `Fail` (not a `Warn`).
pub struct MasterKeyPresentCheck;

#[async_trait]
impl Check for MasterKeyPresentCheck {
    fn name(&self) -> &'static str {
        "master_key_present"
    }
    fn category(&self) -> CheckCategory {
        CheckCategory::Secrets
    }
    async fn run(&self, ctx: &CheckContext) -> CheckResult {
        let path = ctx.data_dir.join(ctx.master_key_filename);
        match tokio::fs::metadata(&path).await {
            Ok(meta) if meta.is_file() => {
                CheckResult::pass(format!("master key at {}", path.display()))
            }
            Ok(_) => CheckResult::fail(format!("{} is not a regular file", path.display())),
            Err(err) => {
                CheckResult::fail(format!("master key {} is missing: {}", path.display(), err))
                    .with_suggestion(
                    "reinitialise the secret store with `sovereign init` or restore from a backup",
                )
            }
        }
    }
}

/// `master_key_perms` — the master key file is `0600`. The fix is a
/// non-destructive `chmod`.
pub struct MasterKeyPermsCheck;

#[async_trait]
impl Check for MasterKeyPermsCheck {
    fn name(&self) -> &'static str {
        "master_key_perms"
    }
    fn category(&self) -> CheckCategory {
        CheckCategory::Secrets
    }
    async fn run(&self, ctx: &CheckContext) -> CheckResult {
        let path = ctx.data_dir.join(ctx.master_key_filename);
        let meta = match tokio::fs::metadata(&path).await {
            Ok(m) => m,
            Err(_) => {
                return CheckResult::skip("master key not present; master_key_present covers it")
            }
        };
        #[cfg(target_os = "linux")]
        {
            let mode = meta.permissions().mode() & 0o777;
            if mode == 0o600 {
                CheckResult::pass(format!("master key is 0600"))
            } else {
                CheckResult::fail(format!("master key is {:o}, expected 600", mode))
                    .with_suggestion("`chmod 0600 /var/lib/sovereign/master.key`")
                    .with_fix(Box::new(MasterKeyRepairPermsFix { path: path.clone() }))
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            // On non-Linux we cannot inspect POSIX mode bits. The
            // file is present, so we pass; ACL checks are a V1+
            // feature.
            let _ = meta;
            CheckResult::skip("perms check is Linux-only; non-Linux host detected")
        }
    }
}

#[allow(dead_code)]
struct MasterKeyRepairPermsFix;

#[async_trait]
impl DoctorFix for MasterKeyRepairPermsFix {
    fn id(&self) -> &'static str {
        "master_key_repair_perms"
    }
    fn description(&self) -> &'static str {
        "chmod 0600 the master key file (non-destructive)"
    }
    fn is_destructive(&self) -> bool {
        false
    }
    async fn apply(&self, _ctx: &CheckContext) -> Result<FixOutcome, FixError> {
        let _path = _ctx.data_dir.join(_ctx.master_key_filename);
        #[cfg(target_os = "linux")]
        {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| FixError::Failed(e.to_string()))?;
            Ok(FixOutcome::Fixed {
                message: format!("chmod 0600 {}", path.display()),
            })
        }
        #[cfg(not(target_os = "linux"))]
        {
            Ok(FixOutcome::Skipped {
                reason: "chmod 0600 is a no-op on non-Linux hosts".to_string(),
            })
        }
    }
}

/// `master_key_decrypts` — the master key is a valid Bech32 age
/// identity (parses with `age::x25519::Identity::from_str`). We do
/// not encrypt/decrypt a real payload in the basic level; the
/// `Standard` level (V1) adds a self-test round-trip.
pub struct MasterKeyDecryptsCheck;

#[async_trait]
impl Check for MasterKeyDecryptsCheck {
    fn name(&self) -> &'static str {
        "master_key_decrypts"
    }
    fn category(&self) -> CheckCategory {
        CheckCategory::Secrets
    }
    async fn run(&self, ctx: &CheckContext) -> CheckResult {
        let path = ctx.data_dir.join(ctx.master_key_filename);
        let body = match tokio::fs::read_to_string(&path).await {
            Ok(b) => b,
            Err(_) => {
                return CheckResult::skip("master key not present; master_key_present covers it")
            }
        };
        let trimmed = body.trim();
        match age::x25519::Identity::from_str(trimmed) {
            Ok(_identity) => CheckResult::pass(format!(
                "master key parses as age x25519 identity ({} bytes)",
                trimmed.len()
            )),
            Err(err) => CheckResult::fail(format!(
                "master key at {} is not a valid age x25519 identity: {}",
                path.display(),
                err
            ))
            .with_suggestion(
                "rotate the master key with `sovereign secret rotate` and re-encrypt all secrets",
            ),
        }
    }
}

pub fn basic_checks() -> Vec<Box<dyn Check>> {
    vec![
        Box::new(MasterKeyPresentCheck),
        Box::new(MasterKeyPermsCheck),
        Box::new(MasterKeyDecryptsCheck),
    ]
}

#[cfg(target_os = "linux")]
fn _path_marker(_p: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn master_key_present_fails_when_missing() {
        let dir = tempdir().unwrap();
        let ctx = CheckContext::for_tests(dir.path().to_path_buf(), dir.path().join("sovereign"));
        let result = MasterKeyPresentCheck.run(&ctx).await;
        assert_eq!(result.status, crate::check::CheckStatus::Fail);
    }

    #[tokio::test]
    async fn master_key_present_passes_when_present() {
        let dir = tempdir().unwrap();
        let key = dir.path().join("master.key");
        tokio::fs::write(
            &key,
            b"AGE-SECRET-KEY-1QQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQ\n",
        )
        .await
        .unwrap();
        let ctx = CheckContext::for_tests(dir.path().to_path_buf(), dir.path().join("sovereign"));
        let result = MasterKeyPresentCheck.run(&ctx).await;
        assert_eq!(result.status, crate::check::CheckStatus::Pass);
    }

    #[tokio::test]
    async fn master_key_perms_skips_when_missing() {
        let dir = tempdir().unwrap();
        let ctx = CheckContext::for_tests(dir.path().to_path_buf(), dir.path().join("sovereign"));
        let result = MasterKeyPermsCheck.run(&ctx).await;
        assert_eq!(result.status, crate::check::CheckStatus::Skip);
    }

    #[test]
    fn basic_checks_returns_three() {
        assert_eq!(basic_checks().len(), 3);
    }
}
