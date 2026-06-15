//! Caddy reverse-proxy adapter.
//!
//! Implements [`ProxyPort`](sovereign_core::ports::ProxyPort) by
//! talking to Caddy's admin API at `http://127.0.0.1:2019` over plain
//! HTTP. Caddy terminates TLS and reverse-proxies plaintext to
//! `127.0.0.1:<upstream_port>`.
//!
//! # V0 trust model
//!
//! - We use Caddy's **`tls internal`** directive (auto-issued, self-
//!   signed CA on first run). Browsers will warn; `curl -k` works.
//!   The point of V0 is to get the route plumbing right; real
//!   Let's Encrypt is V1 once DNS-01 ACME is wired.
//! - The admin API is bound to loopback only; it is not exposed to
//!   the network. The `CADDY_ADMIN_URL` env var is the only way to
//!   override the default.
//! - Caddy is expected to be running as a system service (or via
//!   `caddy run --config /etc/sovereign/Caddyfile`); we do NOT spawn
//!   it from this crate in V0. See `docs/operations-runbook.md`.
//!
//! # Failure modes
//!
//! - `connect` to a non-existent Caddy returns
//!   `AppError::Upstream("caddy admin API not reachable at ...")`. The
//!   CLI maps this to exit code 3 (`AppExit::Upstream`).
//! - `add_route` HTTP failures (non-2xx) include the response body in
//!   the error so the operator can see why Caddy rejected the route
//!   (bad host format, port conflict, etc.).
//! - `remove_route` of a non-existent host returns `Ok(())` (the
//!   use case may call it without a pre-check).

#![deny(unsafe_code)]
#![allow(missing_docs)]

use std::time::Duration;

use async_trait::async_trait;
use reqwest::{Client, StatusCode};
use serde_json::json;
use tracing::{debug, info, warn};

use sovereign_core::{error::AppError, ports::ProxyPort};

/// Default Caddy admin API endpoint. Loopback only; not exposed.
pub const DEFAULT_ADMIN_URL: &str = "http://127.0.0.1:2019";

/// Timeout for a single admin-API call.
const HTTP_TIMEOUT: Duration = Duration::from_secs(5);

/// The Caddy adapter. Cheap to clone (`reqwest::Client` is internally
/// `Arc`'d); the canonical instance lives in the composition root.
#[derive(Debug, Clone)]
pub struct CaddyProxy {
    pub(crate) client: Client,
    pub(crate) admin_url: String,
}

impl CaddyProxy {
    /// Build a `CaddyProxy` and verify Caddy is reachable. `admin_url`
    /// is the full base (e.g. `http://127.0.0.1:2019`).
    pub async fn connect(admin_url: &str) -> Result<Self, AppError> {
        let client = Client::builder()
            .timeout(HTTP_TIMEOUT)
            .build()
            .map_err(|e| AppError::upstream(format!("caddy http client: {e}")))?;

        let health_url = format!("{}/config/", admin_url.trim_end_matches('/'));
        let resp = client.get(&health_url).send().await.map_err(|e| {
            AppError::upstream(format!("caddy admin API not reachable at {admin_url}: {e}"))
        })?;

        if !resp.status().is_success() {
            return Err(AppError::upstream(format!(
                "caddy admin API at {admin_url} returned {}",
                resp.status()
            )));
        }

        info!(admin_url, "caddy admin API reachable");
        Ok(Self {
            client,
            admin_url: admin_url.trim_end_matches('/').to_string(),
        })
    }

    /// Same as [`connect`](Self::connect) but uses
    /// `CADDY_ADMIN_URL` or [`DEFAULT_ADMIN_URL`].
    pub async fn connect_from_env() -> Result<Self, AppError> {
        let url =
            std::env::var("CADDY_ADMIN_URL").unwrap_or_else(|_| DEFAULT_ADMIN_URL.to_string());
        Self::connect(&url).await
    }

    /// The route id used in Caddy's admin API for a given host. Stable
    /// and predictable so we can also issue a DELETE against the same
    /// path. Replaces dots and dashes to stay inside Caddy's path-
    /// segment rules (`/`, `?`, `#` would break the URL).
    fn route_id(host: &str) -> String {
        let mut safe = String::with_capacity(host.len() + 6);
        safe.push_str("route-");
        for c in host.chars() {
            if c.is_ascii_alphanumeric() || c == '-' {
                safe.push(c);
            } else {
                safe.push('_');
            }
        }
        safe
    }

    /// The path for a host's route in Caddy's admin API.
    fn route_path(host: &str) -> String {
        format!(
            "/config/apps/http/servers/srv0/routes/{}",
            Self::route_id(host)
        )
    }
}

#[async_trait]
impl ProxyPort for CaddyProxy {
    async fn add_route(&self, host: &str, upstream_port: u16) -> Result<(), AppError> {
        if host.is_empty() {
            return Err(AppError::validation("host is empty"));
        }
        if upstream_port == 0 {
            return Err(AppError::validation("upstream_port is zero"));
        }

        let route = json!({
            "@id": Self::route_id(host),
            "match": [{"host": [host]}],
            "handle": [{
                "handler": "reverse_proxy",
                "upstreams": [{"dial": format!("127.0.0.1:{upstream_port}")}]
            }],
            "terminal": true
        });

        let path = Self::route_path(host);
        let url = format!("{}{}", self.admin_url, path);
        debug!(host, upstream_port, %url, "caddy add_route");

        let resp = self
            .client
            .put(&url)
            .json(&route)
            .send()
            .await
            .map_err(|e| AppError::upstream(format!("caddy add_route {host}: {e}")))?;

        match resp.status() {
            StatusCode::OK
            | StatusCode::CREATED
            | StatusCode::ACCEPTED
            | StatusCode::NO_CONTENT => {
                info!(host, upstream_port, "caddy route added");
                Ok(())
            }
            status => {
                let body = resp.text().await.unwrap_or_default();
                let body = body.chars().take(500).collect::<String>();
                Err(AppError::upstream(format!(
                    "caddy add_route {host} returned {status}: {body}"
                )))
            }
        }
    }

    async fn remove_route(&self, host: &str) -> Result<(), AppError> {
        if host.is_empty() {
            return Err(AppError::validation("host is empty"));
        }

        let path = Self::route_path(host);
        let url = format!("{}{}", self.admin_url, path);
        debug!(host, %url, "caddy remove_route");

        let resp = self
            .client
            .delete(&url)
            .send()
            .await
            .map_err(|e| AppError::upstream(format!("caddy remove_route {host}: {e}")))?;

        match resp.status() {
            // 200/202/204: route existed and is gone.
            StatusCode::OK
            | StatusCode::ACCEPTED
            | StatusCode::NO_CONTENT
            // 404: route did not exist. Idempotent — we treat this as success
            // because the post-condition ("no route for this host") holds.
            | StatusCode::NOT_FOUND => {
                info!(host, "caddy route removed (or absent)");
                Ok(())
            }
            status => {
                let body = resp.text().await.unwrap_or_default();
                let body = body.chars().take(500).collect::<String>();
                warn!(host, %status, body, "caddy remove_route unexpected status");
                Err(AppError::upstream(format!(
                    "caddy remove_route {host} returned {status}: {body}"
                )))
            }
        }
    }

    async fn reload(&self) -> Result<(), AppError> {
        // V0: Caddy applies admin-API mutations live (no `caddy reload`
        // is required). The trait method exists for V1 fail-over; we
        // no-op with an info log so operators can see the call happened.
        info!("caddy reload called (no-op in V0; admin-API mutations are live)");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_id_replaces_unsafe_chars() {
        assert_eq!(CaddyProxy::route_id("example.com"), "route-example_com");
        assert_eq!(CaddyProxy::route_id("a-b-c"), "route-a-b-c");
        assert_eq!(CaddyProxy::route_id("a/b?c#d"), "route-a_b_c_d");
    }

    #[test]
    fn route_path_is_stable() {
        assert_eq!(
            CaddyProxy::route_path("abc12345.sovereign.local"),
            "/config/apps/http/servers/srv0/routes/route-abc12345_sovereign_local"
        );
    }

    #[test]
    fn route_id_starts_with_route_prefix() {
        // The prefix is what makes it grep-able in `curl http://127.0.0.1:2019/config/`.
        assert!(CaddyProxy::route_id("foo").starts_with("route-"));
    }

    // The real round-trip (PUT + curl + DELETE) is covered by the
    // `testcontainers` integration test in `tests/caddy_integration.rs`
    // and is gated on the dev machine having a Docker daemon — see
    // the test file for the skip conditions.

    #[tokio::test]
    async fn add_route_rejects_empty_host() {
        // No Caddy needed — this is a validation path.
        let proxy = CaddyProxy {
            client: Client::new(),
            admin_url: "http://127.0.0.1:1".to_string(),
        };
        let err = proxy.add_route("", 8080).await.unwrap_err();
        assert!(matches!(err, AppError::Validation(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn add_route_rejects_zero_port() {
        let proxy = CaddyProxy {
            client: Client::new(),
            admin_url: "http://127.0.0.1:1".to_string(),
        };
        let err = proxy.add_route("foo.example", 0).await.unwrap_err();
        assert!(matches!(err, AppError::Validation(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn connect_to_missing_caddy_returns_upstream_error() {
        // Port 1 is reserved and not bound on any sane system.
        let err = CaddyProxy::connect("http://127.0.0.1:1").await.unwrap_err();
        match err {
            AppError::Upstream(msg) => {
                assert!(msg.contains("caddy admin API not reachable"));
            }
            other => panic!("expected Upstream, got {other:?}"),
        }
    }
}
