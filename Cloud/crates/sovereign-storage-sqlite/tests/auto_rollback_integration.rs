// F8b end-to-end test: the prober fires on N consecutive health
// failures and triggers an auto-rollback to the previous Healthy
// deployment. Uses an in-memory SQLite + a `MockRuntime` (no Docker
// required). The real-Docker end-to-end test is queued for Step 9
// (Hetzner CX22).
//
// We put this test in `sovereign-storage-sqlite/tests/` rather than
// in `sovereign-core/src/use_cases/auto_rollback.rs` because:
//   1. The test needs a real `StoragePort` impl; sovereign-core is
//      intentionally adapter-free.
//   2. Sovereign-storage-sqlite already depends on sovereign-core AND
//      sovereign-core's test target has trouble resolving the impl
//      when a dev-dep is used (Rust trait coherence + the storage
//      crate being a dev-dep of core is a known sharp edge).

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use sovereign_core::domain::{
    AppEnv, AppId, AuditKind, AuditQuery, DeploymentEvent, DeploymentStatus, NewApp, NewDeployment,
    Strategy,
};
use sovereign_core::error::AppError;
use sovereign_core::ports::{ContainerSpec, HealthResult, RuntimePort, StoragePort};
use sovereign_core::state::AppState;
use sovereign_core::use_cases::auto_rollback::{trigger_auto_rollback, Prober, ProberConfig};
use sovereign_storage_sqlite::SqliteState;

/// A `RuntimePort` that returns `HealthResult { ok: false }` for
/// the first `n_failures_for(id)` calls per container id, then
/// `ok: true`. The test seeds the map.
struct MockRuntime {
    failures_for: std::sync::Mutex<std::collections::HashMap<String, u32>>,
    calls: AtomicU32,
}

impl MockRuntime {
    fn new() -> Self {
        Self {
            failures_for: std::sync::Mutex::new(std::collections::HashMap::new()),
            calls: AtomicU32::new(0),
        }
    }
    fn fail_n_times(&self, container_id: &str, n: u32) {
        self.failures_for
            .lock()
            .unwrap()
            .insert(container_id.to_string(), n);
    }
}

#[async_trait]
impl RuntimePort for MockRuntime {
    async fn pull_image(&self, _image: &str) -> Result<(), AppError> {
        Ok(())
    }
    async fn create_container(&self, _spec: ContainerSpec) -> Result<String, AppError> {
        Ok("mock-container".into())
    }
    async fn start_container(&self, _id: &str) -> Result<(), AppError> {
        Ok(())
    }
    async fn stop_container(&self, _id: &str, _t: Duration) -> Result<(), AppError> {
        Ok(())
    }
    async fn remove_container(&self, _id: &str) -> Result<(), AppError> {
        Ok(())
    }
    async fn healthcheck(
        &self,
        id: &str,
        _port: u16,
        _path: &str,
        _t: Duration,
    ) -> Result<HealthResult, AppError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut g = self.failures_for.lock().unwrap();
        let remaining = g.entry(id.to_string()).or_insert(0);
        if *remaining > 0 {
            *remaining -= 1;
            Ok(HealthResult {
                ok: false,
                latency_ms: 1,
                error: Some("mock 5xx".into()),
            })
        } else {
            Ok(HealthResult {
                ok: true,
                latency_ms: 1,
                error: None,
            })
        }
    }
}

async fn fresh_state() -> (Arc<dyn StoragePort>, Arc<MockRuntime>, AppState) {
    let storage: Arc<dyn StoragePort> = Arc::new(
        SqliteState::open_in_memory_named(&format!("f8b-{}", uuid::Uuid::new_v4()))
            .await
            .expect("open mem"),
    );
    let runtime = Arc::new(MockRuntime::new());
    let state = AppState {
        storage: storage.clone(),
        runtime: runtime.clone(),
        proxy: None,
        secrets: None,
        backup: None,
        db_path: std::path::PathBuf::from("(test-no-db)"),
    };
    (storage, runtime, state)
}

async fn drive_to_healthy(
    s: &dyn StoragePort,
    app_id: AppId,
    image: &str,
) -> sovereign_core::domain::Deployment {
    let mut d = s
        .begin_deployment(
            NewDeployment {
                app_id,
                image_ref: image.to_string(),
                strategy: Strategy::Recreate,
                triggered_by: "system".into(),
                risk_score: None,
            },
            "system",
        )
        .await
        .unwrap();
    for next in [
        DeploymentStatus::Building,
        DeploymentStatus::Pushing,
        DeploymentStatus::Starting,
        DeploymentStatus::Healthy,
    ] {
        d = s
            .transition_deployment(
                d.id,
                DeploymentEvent {
                    to: next,
                    error: None,
                    actor: "system".into(),
                },
                "system",
            )
            .await
            .unwrap();
    }
    d
}

#[tokio::test]
async fn prober_does_not_fire_below_threshold() {
    let (storage, runtime, state) = fresh_state().await;
    let app = storage
        .create_app(
            NewApp {
                name: "api".into(),
                owner: "user:alice".into(),
                env: AppEnv::Dev,
                git_repo: None,
                image_ref: Some("nginx:alpine".into()),
                config_yaml: "kind: app\nname: api\n".into(),
                health_path: Some("/health".into()),
            },
            "user:alice",
        )
        .await
        .expect("create app");
    let dep = drive_to_healthy(&*storage, app.id, "nginx:alpine").await;
    let cid = format!("sovereign-{}-{}", app.id, dep.id);
    runtime.fail_n_times(&cid, 2); // < threshold of 3

    let prober = Prober::new(
        app.id,
        ProberConfig {
            interval: Duration::from_millis(1),
            ..ProberConfig::default()
        },
    );
    for _ in 0..2 {
        assert!(!prober.step(&state).await.expect("step"));
    }
    let s = prober.state.lock().await;
    assert_eq!(s.consecutive_failures, 2);
    assert!(!s.rolled_back);
}

#[tokio::test]
async fn prober_fires_on_threshold_and_records_audit() {
    let (storage, runtime, state) = fresh_state().await;
    let app = storage
        .create_app(
            NewApp {
                name: "api".into(),
                owner: "user:alice".into(),
                env: AppEnv::Dev,
                git_repo: None,
                image_ref: Some("nginx:alpine".into()),
                config_yaml: "kind: app\nname: api\n".into(),
                health_path: Some("/health".into()),
            },
            "user:alice",
        )
        .await
        .expect("create app");
    let dep = drive_to_healthy(&*storage, app.id, "nginx:alpine").await;
    let cid = format!("sovereign-{}-{}", app.id, dep.id);
    runtime.fail_n_times(&cid, 3);

    let prober = Prober::new(
        app.id,
        ProberConfig {
            interval: Duration::from_millis(1),
            ..ProberConfig::default()
        },
    );
    let mut fired = false;
    for _ in 0..3 {
        if prober.step(&state).await.expect("step") {
            fired = true;
        }
    }
    assert!(fired, "rollback should have fired on the 3rd failure");

    // Audit event recorded.
    let audit = state
        .storage
        .query_audit(AuditQuery {
            kind: Some(AuditKind::Rollback),
            limit: 100,
            ..Default::default()
        })
        .await
        .expect("query");
    assert!(
        audit
            .iter()
            .any(
                |e| e.payload.get("reason").and_then(|v| v.as_str()) == Some("health_failure")
                    && e.payload.get("deployment_id").is_some()
            ),
        "expected an auto-rollback audit event, got {audit:?}"
    );

    // `state.rolled_back == true` after the fire.
    let s = prober.state.lock().await;
    assert!(s.rolled_back);
}

#[tokio::test]
async fn prober_resets_streak_on_success() {
    let (storage, runtime, state) = fresh_state().await;
    let app = storage
        .create_app(
            NewApp {
                name: "api".into(),
                owner: "user:alice".into(),
                env: AppEnv::Dev,
                git_repo: None,
                image_ref: Some("nginx:alpine".into()),
                config_yaml: "kind: app\nname: api\n".into(),
                health_path: Some("/health".into()),
            },
            "user:alice",
        )
        .await
        .expect("create app");
    let dep = drive_to_healthy(&*storage, app.id, "nginx:alpine").await;
    let cid = format!("sovereign-{}-{}", app.id, dep.id);
    runtime.fail_n_times(&cid, 2);
    let prober = Prober::new(
        app.id,
        ProberConfig {
            interval: Duration::from_millis(1),
            ..ProberConfig::default()
        },
    );
    for _ in 0..2 {
        let _ = prober.step(&state).await;
    }
    // third probe succeeds -> streak resets
    let _ = prober.step(&state).await;
    let s = prober.state.lock().await;
    assert_eq!(s.consecutive_failures, 0);
    assert!(!s.rolled_back);
}

#[tokio::test]
async fn auto_rollback_with_no_previous_healthy_marks_failed() {
    // Edge case: a single deployment, no previous. The prober
    // should mark the current as Failed (no rollback target
    // available).
    let (storage, _runtime, state) = fresh_state().await;
    let app = storage
        .create_app(
            NewApp {
                name: "api".into(),
                owner: "user:alice".into(),
                env: AppEnv::Dev,
                git_repo: None,
                image_ref: Some("nginx:alpine".into()),
                config_yaml: "kind: app\nname: api\n".into(),
                health_path: Some("/health".into()),
            },
            "user:alice",
        )
        .await
        .expect("create app");
    let dep = drive_to_healthy(&*storage, app.id, "nginx:alpine").await;

    trigger_auto_rollback(
        &state,
        app.id,
        dep.id,
        &HealthResult {
            ok: false,
            latency_ms: 1,
            error: Some("simulated 5xx".into()),
        },
    )
    .await
    .expect("auto-rollback");

    let now = state
        .storage
        .get_deployment(dep.id)
        .await
        .unwrap()
        .expect("deployment still there");
    assert_eq!(now.status, DeploymentStatus::Failed);
}
