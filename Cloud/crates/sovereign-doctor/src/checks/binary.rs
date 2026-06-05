use async_trait::async_trait;
use std::path::Path;

use crate::check::{Check, CheckCategory, CheckContext, CheckResult};

/// `binary_self` — the running binary's path, version, and mtime are
/// all readable. Fails when the binary has been removed/moved mid-run.
pub struct BinarySelfCheck;

#[async_trait]
impl Check for BinarySelfCheck {
    fn name(&self) -> &'static str {
        "binary_self"
    }
    fn category(&self) -> CheckCategory {
        CheckCategory::Binary
    }
    async fn run(&self, ctx: &CheckContext) -> CheckResult {
        let meta = match tokio::fs::metadata(&ctx.binary_path).await {
            Ok(m) => m,
            Err(err) => {
                return CheckResult::fail(format!(
                    "binary {} is not readable: {}",
                    ctx.binary_path.display(),
                    err
                ))
                .with_suggestion("reinstall with `sovereign update` or `curl -sSf https://sovereignruntime.dev/install.sh | sh`");
            }
        };
        let size_mb = meta.len() / (1024 * 1024);
        CheckResult::pass(format!(
            "{} ({} MB, version {})",
            ctx.binary_path.display(),
            size_mb,
            ctx.binary_version
        ))
    }
}

/// `binary_stripped` — the binary is reasonably small (under 100 MB)
/// which catches unstripped dev builds accidentally shipped to prod.
pub struct BinaryStrippedCheck;

#[async_trait]
impl Check for BinaryStrippedCheck {
    fn name(&self) -> &'static str {
        "binary_stripped"
    }
    fn category(&self) -> CheckCategory {
        CheckCategory::Binary
    }
    async fn run(&self, ctx: &CheckContext) -> CheckResult {
        const MAX_BINARY_MB: u64 = 100;
        let Ok(meta) = tokio::fs::metadata(&ctx.binary_path).await else {
            return CheckResult::skip("binary is not readable; binary_self check covers it");
        };
        let size_mb = meta.len() / (1024 * 1024);
        if size_mb > MAX_BINARY_MB {
            return CheckResult::fail(format!(
                "binary is {} MB which is larger than the {} MB V0 budget",
                size_mb, MAX_BINARY_MB
            ))
            .with_suggestion("rebuild with `cargo build --release` and reinstall");
        }
        CheckResult::pass(format!(
            "binary is {} MB (under {} MB budget)",
            size_mb, MAX_BINARY_MB
        ))
    }
}

/// `binary_under_dir` — the binary lives under a recognised install
/// path (`/usr/local/bin`, `/var/lib/sovereign/bin`, or the current
/// working dir in dev). Warns on odd paths.
pub struct BinaryUnderDirCheck;

#[async_trait]
impl Check for BinaryUnderDirCheck {
    fn name(&self) -> &'static str {
        "binary_under_dir"
    }
    fn category(&self) -> CheckCategory {
        CheckCategory::Binary
    }
    async fn run(&self, ctx: &CheckContext) -> CheckResult {
        let path = &ctx.binary_path;
        let recognised = is_recognised_install_path(path);
        if recognised {
            CheckResult::pass(format!("{} is a recognised install path", path.display()))
        } else {
            CheckResult::warn(format!(
                "{} is not a recognised install path; consider /usr/local/bin or /var/lib/sovereign/bin",
                path.display()
            ))
        }
    }
}

pub fn basic_checks() -> Vec<Box<dyn Check>> {
    vec![
        Box::new(BinarySelfCheck),
        Box::new(BinaryStrippedCheck),
        Box::new(BinaryUnderDirCheck),
    ]
}

fn is_recognised_install_path(p: &Path) -> bool {
    let s = p.to_string_lossy();
    s.starts_with("/usr/local/bin")
        || s.starts_with("/var/lib/sovereign/bin")
        || s.starts_with("/opt/sovereign/bin")
        || s.starts_with("/usr/bin")
        // dev/test runs: the binary under the workspace target dir
        || s.contains("target/release")
        || s.contains("target\\release")
        || s.contains("target/debug")
        || s.contains("target\\debug")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn binary_self_passes_when_present() {
        let dir = tempdir().unwrap();
        let bin = dir.path().join("sovereign");
        tokio::fs::write(&bin, b"#!/bin/sh\necho ok\n")
            .await
            .unwrap();
        let ctx = CheckContext::for_tests(dir.path().to_path_buf(), bin);
        let result = BinarySelfCheck.run(&ctx).await;
        assert_eq!(result.status, crate::check::CheckStatus::Pass);
    }

    #[tokio::test]
    async fn binary_self_fails_when_missing() {
        let dir = tempdir().unwrap();
        let bin = dir.path().join("sovereign");
        let ctx = CheckContext::for_tests(dir.path().to_path_buf(), bin);
        let result = BinarySelfCheck.run(&ctx).await;
        assert_eq!(result.status, crate::check::CheckStatus::Fail);
    }

    #[tokio::test]
    async fn binary_under_dir_passes_for_target_release() {
        let dir = tempdir().unwrap();
        let bin = dir
            .path()
            .join("..")
            .join("target")
            .join("release")
            .join("sovereign");
        let ctx = CheckContext::for_tests(dir.path().to_path_buf(), bin);
        let result = BinaryUnderDirCheck.run(&ctx).await;
        assert_eq!(result.status, crate::check::CheckStatus::Pass);
    }

    #[tokio::test]
    async fn binary_under_dir_warns_for_odd_path() {
        let dir = tempdir().unwrap();
        let bin = dir.path().join("sovereign");
        let ctx = CheckContext::for_tests(dir.path().to_path_buf(), bin);
        let result = BinaryUnderDirCheck.run(&ctx).await;
        assert_eq!(result.status, crate::check::CheckStatus::Warn);
    }

    #[test]
    fn basic_checks_returns_three() {
        assert_eq!(basic_checks().len(), 3);
    }
}
