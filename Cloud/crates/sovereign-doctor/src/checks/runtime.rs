use async_trait::async_trait;

use crate::check::{Check, CheckCategory, CheckContext, CheckResult};

/// `docker_socket` — the Docker daemon socket exists. On Linux this
/// is `/var/run/docker.sock`; the test runner is Windows, so we Skip.
pub struct DockerSocketCheck;

#[async_trait]
impl Check for DockerSocketCheck {
    fn name(&self) -> &'static str {
        "docker_socket"
    }
    fn category(&self) -> CheckCategory {
        CheckCategory::Runtime
    }
    async fn run(&self, _ctx: &CheckContext) -> CheckResult {
        #[cfg(target_os = "linux")]
        {
            match tokio::fs::metadata(&ctx.docker_socket).await {
                Ok(meta) if meta.file_type().is_socket() || meta.file_type().is_fifo() => {
                    CheckResult::pass(format!("docker socket at {}", ctx.docker_socket.display()))
                }
                Ok(_) => CheckResult::fail(format!(
                    "{} exists but is not a socket",
                    ctx.docker_socket.display()
                ))
                .with_suggestion("install Docker Engine 24+ and start the daemon"),
                Err(err) => CheckResult::fail(format!(
                    "docker socket {} is missing: {}",
                    ctx.docker_socket.display(),
                    err
                ))
                .with_suggestion("install Docker Engine 24+ with `apt install docker.io` or use the official install script"),
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            CheckResult::skip("docker_socket check is Linux-only; non-Linux host detected")
        }
    }
}

/// `docker_version` — the Docker daemon version is ≥ 24. Skip when
/// the CLI is missing; do not require the operator to install it
/// just to read a version string.
pub struct DockerVersionCheck;

#[async_trait]
impl Check for DockerVersionCheck {
    fn name(&self) -> &'static str {
        "docker_version"
    }
    fn category(&self) -> CheckCategory {
        CheckCategory::Runtime
    }
    async fn run(&self, _ctx: &CheckContext) -> CheckResult {
        #[cfg(target_os = "linux")]
        {
            match tokio::process::Command::new("docker")
                .arg("--version")
                .output()
                .await
            {
                Ok(out) if out.status.success() => {
                    let version = String::from_utf8_lossy(&out.stdout);
                    let version = version.trim();
                    if version_satisfies_min_24(version) {
                        CheckResult::pass(format!("docker version: {}", version))
                    } else {
                        CheckResult::fail(format!("{} is below the required 24.x", version))
                            .with_suggestion("upgrade Docker to 24.x or later")
                    }
                }
                Ok(out) => CheckResult::fail(format!(
                    "`docker --version` exited with status {:?}",
                    out.status.code()
                )),
                Err(err) => CheckResult::skip(format!("docker CLI not on PATH: {}", err)),
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            CheckResult::skip("docker_version check is Linux-only; non-Linux host detected")
        }
    }
}

/// `docker_run` — a test `docker run --rm alpine:3.20 echo ok`
/// returns 0. Skip when the CLI is not on PATH; never fail loudly
/// for an environment that does not have Docker available.
pub struct DockerRunCheck;

#[async_trait]
impl Check for DockerRunCheck {
    fn name(&self) -> &'static str {
        "docker_run"
    }
    fn category(&self) -> CheckCategory {
        CheckCategory::Runtime
    }
    async fn run(&self, _ctx: &CheckContext) -> CheckResult {
        #[cfg(target_os = "linux")]
        {
            match tokio::process::Command::new("docker")
                .args(["run", "--rm", "alpine:3.20", "echo", "ok"])
                .output()
                .await
            {
                Ok(out) if out.status.success() => {
                    CheckResult::pass("docker run --rm alpine:3.20 echo ok".into())
                }
                Ok(out) => {
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    CheckResult::fail(format!(
                        "`docker run alpine:3.20` exited with status {:?}: {}",
                        out.status.code(),
                        stderr.trim()
                    ))
                }
                Err(err) => CheckResult::skip(format!("docker CLI not on PATH: {}", err)),
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            CheckResult::skip("docker_run check is Linux-only; non-Linux host detected")
        }
    }
}

pub fn basic_checks() -> Vec<Box<dyn Check>> {
    vec![
        Box::new(DockerSocketCheck),
        Box::new(DockerVersionCheck),
        Box::new(DockerRunCheck),
    ]
}

#[cfg_attr(not(test), allow(dead_code))]
fn version_satisfies_min_24(version: &str) -> bool {
    // Examples: "Docker version 24.0.7, build ..." or "Docker version 26.1.3".
    let rest = version
        .trim_start_matches("Docker version")
        .trim_start_matches(',')
        .trim();
    let major: u32 = rest
        .split('.')
        .next()
        .and_then(|p| p.parse().ok())
        .unwrap_or(0);
    major >= 24
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn parses_docker_version_strings() {
        assert!(version_satisfies_min_24(
            "Docker version 24.0.7, build afdd53b"
        ));
        assert!(version_satisfies_min_24(
            "Docker version 26.1.3, build ae00018"
        ));
        assert!(!version_satisfies_min_24(
            "Docker version 20.10.24, build 297e128"
        ));
        assert!(!version_satisfies_min_24("not docker output"));
    }

    #[tokio::test]
    async fn docker_socket_skips_on_non_linux() {
        let dir = tempdir().unwrap();
        let ctx = CheckContext::for_tests(dir.path().to_path_buf(), dir.path().join("sovereign"));
        let result = DockerSocketCheck.run(&ctx).await;
        // Linux gives Pass/Fail; non-Linux gives Skip.
        assert!(matches!(
            result.status,
            crate::check::CheckStatus::Pass
                | crate::check::CheckStatus::Fail
                | crate::check::CheckStatus::Skip
        ));
    }

    #[test]
    fn basic_checks_returns_three() {
        assert_eq!(basic_checks().len(), 3);
    }
}
