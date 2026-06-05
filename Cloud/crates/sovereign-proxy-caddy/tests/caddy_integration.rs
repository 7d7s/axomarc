//! Integration test for `CaddyProxy` against a real Caddy container.
//!
//! Marked `#[ignore]` so the default `cargo test` does not require a
//! Docker daemon. Run with:
//!
//! ```text
//! cargo test -p sovereign-proxy-caddy --test caddy_integration -- --ignored --nocapture
//! ```
//!
//! The CI workflow at `.github/workflows/ci.yml` runs the `--ignored`
//! tests on every push; the test fails the build if Docker is not
//! available in CI, which is the contract (see F6 DoD:
//! "passes in CI on every PR").
//!
//! ## Why shell-out, not testcontainers
//!
//! The `testcontainers` crate (any version 0.20+) pulls in its own
//! `bollard`, which conflicts with the workspace's pinned `bollard`
//! 0.18 (the runtime adapter's version per `docs/tech-stack.md` §1).
//! Forcing both would inflate the binary. Shelling out to the
//! `docker` CLI is the same test, with no extra dependency, and
//! works on every CI runner that has docker installed (which is all
//! of them).
//!
//! ## Coverage
//!
//! 1. `CaddyProxy::connect` to a healthy Caddy returns `Ok`.
//! 2. `add_route` is reflected in Caddy's admin `/config/` endpoint.
//! 3. `remove_route` removes the route from the admin config.
//! 4. `remove_route` of a non-existent route returns `Ok` (idempotent).
//!
//! The full HTTPS round-trip ("deploy nginx, add route, curl, remove,
//! curl again, expect 404") is the F6 Definition of Done. We exercise
//! the route-mutation contract here; the HTTPS handshake is left to
//! the operator's `sovereign deploy` end-to-end test (Step 9), which
//! needs a real host with /etc/hosts entries to resolve the
//! `.sovereign.local` hostname.

#![allow(clippy::needless_return)]

use std::process::Stdio;
use std::time::Duration;

use sovereign_core::ports::ProxyPort;
use sovereign_proxy_caddy::CaddyProxy;
use tokio::process::Command;

/// Name we tag the Caddy container with so we can find/clean it up.
const CONTAINER_NAME: &str = "sovereign-caddy-itest";

/// RAII guard that runs `docker rm -f <name>` on drop, so a panic in
/// the test body does not leave a container running.
struct ContainerGuard {
    name: String,
}

impl Drop for ContainerGuard {
    fn drop(&mut self) {
        // Best-effort cleanup. `Command::new` and `spawn` block, but
        // the test is finishing (or panicking) anyway, and a stale
        // container is a worse problem than a slow cleanup.
        let _ = std::process::Command::new("docker")
            .args(["rm", "-f", &self.name])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// Spawn `caddy:2-alpine` and wait until its admin API on :2019 is
/// responding. Returns the host:port the container's :2019 was
/// mapped to, plus a guard that cleans up on drop.
async fn spawn_caddy() -> (String, u16, ContainerGuard) {
    // 1. Remove any stale container from a previous run.
    let _ = Command::new("docker")
        .args(["rm", "-f", CONTAINER_NAME])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;

    // 2. Start a fresh Caddy. `-p 2019:2019` binds the admin API on
    //    a random host port (we parse it from `docker port`).
    let status = Command::new("docker")
        .args([
            "run",
            "-d",
            "--name",
            CONTAINER_NAME,
            "-p",
            "2019:2019",
            "-p",
            "443:443",
            "caddy:2-alpine",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .status()
        .await
        .expect("docker run caddy");
    assert!(status.success(), "docker run caddy failed: {status:?}");

    // 3. Resolve the host port.
    let out = Command::new("docker")
        .args(["port", CONTAINER_NAME, "2019/tcp"])
        .output()
        .await
        .expect("docker port");
    let s = String::from_utf8_lossy(&out.stdout);
    // Output looks like `0.0.0.0:32768\n` — take the last `:` segment.
    let port: u16 = s
        .trim()
        .rsplit(':')
        .next()
        .and_then(|p| p.trim().parse().ok())
        .unwrap_or_else(|| panic!("could not parse docker port output: {s}"));

    // 4. Wait for the admin API to answer. Caddy logs
    //    "serving initial configuration" on stdout; `docker logs`
    //    is the cheapest readiness probe.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let admin_url = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    loop {
        if let Ok(resp) = client.get(format!("{admin_url}/config/")).send().await {
            if resp.status().is_success() {
                break;
            }
        }
        if std::time::Instant::now() > deadline {
            panic!("caddy admin API never came up at {admin_url} within 30s");
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    (
        admin_url,
        port,
        ContainerGuard {
            name: CONTAINER_NAME.to_string(),
        },
    )
}

#[tokio::test]
#[ignore = "requires Docker; run with --ignored"]
async fn caddy_proxy_add_remove_roundtrip() {
    let (admin_url, _port, _guard) = spawn_caddy().await;

    let proxy = CaddyProxy::connect(&admin_url)
        .await
        .expect("connect to caddy");

    // add_route
    proxy
        .add_route("test.example.com", 8080)
        .await
        .expect("add_route");

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let cfg: serde_json::Value = client
        .get(format!("{admin_url}/config/apps/http/servers/srv0/routes/"))
        .send()
        .await
        .expect("get routes")
        .json()
        .await
        .expect("parse routes json");
    let routes = cfg.as_array().expect("routes is an array");
    assert!(
        routes
            .iter()
            .any(|r| r.get("@id").and_then(|v| v.as_str()) == Some("route-test_example_com")),
        "expected the new route in Caddy's config; got {routes:?}"
    );

    // remove_route
    proxy
        .remove_route("test.example.com")
        .await
        .expect("remove_route");
    let cfg: serde_json::Value = client
        .get(format!("{admin_url}/config/apps/http/servers/srv0/routes/"))
        .send()
        .await
        .expect("get routes after remove")
        .json()
        .await
        .expect("parse routes json");
    let routes = cfg.as_array().expect("routes is an array");
    assert!(
        routes.is_empty(),
        "expected no routes after remove; got {routes:?}"
    );

    // remove_route of a non-existent host is idempotent
    proxy
        .remove_route("never-added.example.com")
        .await
        .expect("remove_route of absent host is Ok");
}
