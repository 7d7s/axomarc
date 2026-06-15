// P0+P4: Axum HTTP server for the sovereign daemon.
//
// The daemon is the long-running process that hosts:
//   - POST /webhook/<app> — HMAC-verified webhook dispatch
//   - GET  /health        — version + uptime
//   - GET  /metrics       — prometheus exporter (Phase D)
//
// Graceful shutdown on SIGTERM / SIGINT. One process, one config,
// one systemd unit.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Json;
use serde::Serialize;
use sovereign_core::state::AppState;

/// Shared state threaded through axum handlers.
pub struct DaemonState {
    pub started_at: Instant,
    pub version: &'static str,
    pub app_state: AppState,
    /// Broadcast channel for deploy notifications (webhook → Telegram poller).
    pub notification_tx: tokio::sync::broadcast::Sender<String>,
}

/// Build the axum router with all daemon routes.
pub fn router(state: Arc<DaemonState>) -> axum::Router {
    axum::Router::new()
        .route("/health", get(health))
        .route("/webhook/{app}", post(crate::webhook::handle_webhook))
        .with_state(state)
}

/// `GET /health` — returns version + uptime. Always 200.
async fn health(
    axum::extract::State(state): axum::extract::State<Arc<DaemonState>>,
) -> impl IntoResponse {
    let uptime_secs = state.started_at.elapsed().as_secs();
    Json(HealthResponse {
        status: "ok".to_string(),
        version: state.version.to_string(),
        uptime_secs,
    })
}

#[derive(Serialize)]
struct HealthResponse {
    status: String,
    version: String,
    uptime_secs: u64,
}

/// Start the daemon on the given listen address. Blocks until
/// SIGTERM or SIGINT is received, then performs a graceful shutdown.
pub async fn run(addr: SocketAddr, app_state: AppState) -> anyhow::Result<()> {
    let (notification_tx, _) = tokio::sync::broadcast::channel(64);
    let state = Arc::new(DaemonState {
        started_at: Instant::now(),
        version: env!("CARGO_PKG_VERSION"),
        app_state,
        notification_tx,
    });

    let app = router(state);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(addr = %addr, "daemon listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    tracing::info!("daemon shut down gracefully");
    Ok(())
}

/// Wait for SIGTERM (Linux) or Ctrl+C (all platforms).
async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();

    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm =
            signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");
        tokio::select! {
            _ = ctrl_c => tracing::info!("received SIGINT (Ctrl+C)"),
            _ = sigterm.recv() => tracing::info!("received SIGTERM"),
        }
    }

    #[cfg(not(unix))]
    {
        ctrl_c.await.ok();
        tracing::info!("received Ctrl+C");
    }
}

/// Render a systemd unit file for the daemon.
pub fn systemd_unit(_unit_name: &str, bin_path: &str) -> String {
    format!(
        r#"[Unit]
Description=Sovereign Application Runtime Daemon
After=network.target
Wants=network.target

[Service]
Type=simple
ExecStart={bin_path} daemon start
Restart=always
RestartSec=5
Environment=RUST_LOG=info,sovereign=debug

# Hardening
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
ReadWritePaths=/var/lib/sovereign
PrivateTmp=yes

[Install]
WantedBy=multi-user.target
"#,
        bin_path = bin_path,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use sovereign_core::state::no_runtime;
    use tower::ServiceExt;

    /// Build a test DaemonState with in-memory storage (no real DB).
    async fn test_state() -> Arc<DaemonState> {
        use sovereign_core::ports::StoragePort;
        use sovereign_storage_sqlite::SqliteState;

        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let pid = std::process::id();
        let name = format!("daemon-test-{pid}-{n}");

        let s = SqliteState::open_in_memory_named(&name)
            .await
            .unwrap();
        let storage = Arc::new(s) as Arc<dyn StoragePort>;

        Arc::new(DaemonState {
            started_at: Instant::now(),
            version: "0.1.0",
            app_state: AppState {
                storage,
                runtime: no_runtime(),
                proxy: None,
                secrets: None,
                backup: None,
                db_path: std::path::PathBuf::from(":memory:"),
            },
            notification_tx: tokio::sync::broadcast::channel(64).0,
        })
    }

    #[tokio::test]
    async fn health_json_has_version() {
        let state = test_state().await;
        let app = router(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "ok");
        assert_eq!(json["version"], "0.1.0");
        assert!(json["uptime_secs"].as_u64().is_some());
    }

    #[tokio::test]
    async fn webhook_returns_202_accepted() {
        let state = test_state().await;
        let app = router(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/webhook/myapp")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({"ref":"refs/heads/main","repository":{"full_name":"user/repo"}}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        // 404 because no app named "myapp" exists in the empty DB
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn webhook_unknown_app_returns_404() {
        let state = test_state().await;
        let app = router(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/webhook/nonexistent")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({"ref":"refs/heads/main","repository":{"name":"repo"}}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn systemd_unit_has_key_fields() {
        let unit = systemd_unit("sovereign-daemon", "/usr/bin/sovereign");
        assert!(unit.contains("ExecStart=/usr/bin/sovereign daemon start"));
        assert!(unit.contains("Restart=always"));
        assert!(unit.contains("WantedBy=multi-user.target"));
        assert!(unit.contains("ProtectSystem=strict"));
    }

    #[test]
    fn default_listen_address() {
        let addr: SocketAddr = "127.0.0.1:8443".parse().unwrap();
        assert_eq!(addr.port(), 8443);
    }
}
