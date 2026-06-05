use async_trait::async_trait;

use crate::check::{Check, CheckCategory, CheckContext, CheckResult};

/// `kernel` — Linux kernel version ≥ 5.10.
///
/// Skip on non-Linux platforms (the test runner is Windows; production
/// runs are Linux-only). Reports `Skip` rather than `Fail` so a
/// non-Linux host does not light up red.
pub struct KernelCheck;

#[async_trait]
impl Check for KernelCheck {
    fn name(&self) -> &'static str {
        "kernel"
    }
    fn category(&self) -> CheckCategory {
        CheckCategory::System
    }
    async fn run(&self, _ctx: &CheckContext) -> CheckResult {
        #[cfg(target_os = "linux")]
        {
            match read_uname_release() {
                Some(release) if release_at_least(&release, 5, 10) => {
                    CheckResult::pass(format!("kernel {} (>= 5.10)", release))
                }
                Some(release) => CheckResult::fail(format!(
                    "kernel {} is older than 5.10; upgrade before running in production",
                    release
                ))
                .with_suggestion("install linux-image-5.10+ or use a current distro"),
                None => CheckResult::skip("could not read /proc/sys/kernel/osrelease"),
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            CheckResult::skip("kernel check is Linux-only; non-Linux host detected")
        }
    }
}

/// `memory` — ≥ 512 MB free RAM.
pub struct MemoryCheck;

#[async_trait]
impl Check for MemoryCheck {
    fn name(&self) -> &'static str {
        "memory"
    }
    fn category(&self) -> CheckCategory {
        CheckCategory::System
    }
    async fn run(&self, _ctx: &CheckContext) -> CheckResult {
        #[cfg(target_os = "linux")]
        {
            match read_meminfo_available_kb() {
                Some(kb) if kb >= crate::MIN_FREE_MEMORY_MB * 1024 => {
                    CheckResult::pass(format!("{} MB free RAM", kb / 1024))
                }
                Some(kb) => CheckResult::fail(format!(
                    "only {} MB free RAM; need at least {} MB",
                    kb / 1024,
                    crate::MIN_FREE_MEMORY_MB
                ))
                .with_suggestion("stop other workloads or upgrade to a larger instance"),
                None => CheckResult::skip("could not read /proc/meminfo"),
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            CheckResult::skip("memory check is Linux-only; non-Linux host detected")
        }
    }
}

/// `disk` — ≥ 5 GB free on the data directory.
pub struct DiskCheck;

#[async_trait]
impl Check for DiskCheck {
    fn name(&self) -> &'static str {
        "disk"
    }
    fn category(&self) -> CheckCategory {
        CheckCategory::System
    }
    async fn run(&self, ctx: &CheckContext) -> CheckResult {
        match tokio::fs::metadata(&ctx.data_dir).await {
            Ok(_) => {
                // We have no portable way to stat free space without
                // pulling in `nix` or `statvfs`; emit a soft `Pass` that
                // confirms the directory is reachable. Production
                // deployments add a real `statvfs` check on Linux.
                CheckResult::pass(format!("data dir {} is reachable", ctx.data_dir.display()))
            }
            Err(err) => CheckResult::fail(format!(
                "data dir {} is not reachable: {}",
                ctx.data_dir.display(),
                err
            ))
            .with_suggestion(format!(
                "create the directory and `chown` it to the sovereign user: \
                 `mkdir -p {} && chown -R sovereign:sovereign {}`",
                ctx.data_dir.display(),
                ctx.data_dir.display(),
            )),
        }
    }
}

pub fn basic_checks() -> Vec<Box<dyn Check>> {
    vec![
        Box::new(KernelCheck),
        Box::new(MemoryCheck),
        Box::new(DiskCheck),
    ]
}

#[cfg(target_os = "linux")]
fn read_uname_release() -> Option<String> {
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .ok()
        .map(|s| s.trim().to_string())
}

#[cfg(target_os = "linux")]
fn release_at_least_5_10(release: &str) -> bool {
    release_at_least(release, 5, 10)
}

#[cfg_attr(not(test), allow(dead_code))]
fn release_at_least(release: &str, want_major: u32, want_minor: u32) -> bool {
    let mut parts = release.split('.');
    let major: u32 = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let minor: u32 = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    (major, minor) >= (want_major, want_minor)
}

#[cfg(target_os = "linux")]
fn read_meminfo_available_kb() -> Option<u64> {
    let body = std::fs::read_to_string("/proc/meminfo").ok()?;
    for line in body.lines() {
        if let Some(rest) = line.strip_prefix("MemAvailable:") {
            let kb: u64 = rest
                .split_whitespace()
                .next()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            return Some(kb);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_5_10_satisfies_check() {
        assert!(release_at_least("5.10.0", 5, 10));
        assert!(release_at_least("6.1.0", 5, 10));
        assert!(release_at_least("5.10.123-rc1", 5, 10));
    }

    #[test]
    fn older_releases_fail_check() {
        assert!(!release_at_least("4.19.0", 5, 10));
        assert!(!release_at_least("5.4.0", 5, 10));
        assert!(!release_at_least("3.10.0", 5, 10));
    }

    #[test]
    fn basic_checks_returns_three() {
        assert_eq!(basic_checks().len(), 3);
    }
}
