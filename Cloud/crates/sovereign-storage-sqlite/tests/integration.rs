// F3 integration tests for the SQLite storage adapter.
//
// Coverage:
//   - `open` opens the DB, runs migrations, and the 7 core tables exist.
//   - WAL mode is active.
//   - App CRUD: create, get, list, update with version check (OCC),
//     archive (soft delete), unique-name conflict.
//   - Deployment lifecycle: full happy path, all 7 states reachable,
//     invalid transitions rejected, version bumped on each transition.
//   - Secret CRUD + rotation.
//   - Domain CRUD.
//   - Server CRUD + touch.
//   - Backup CRUD (begin → complete → verify).
//   - User CRUD + role changes.
//   - Audit log: append, query, count, and the **trust anchor** — the
//     `audit_no_update` and `audit_no_delete` triggers reject the
//     mutation that the auditor most needs to forbid.

use std::path::PathBuf;

use sovereign_core::domain::{
    AppEnv, AppId, AppUpdate, AuditEvent, AuditKind, AuditQuery, Deployment, DeploymentEvent,
    DeploymentStatus, NewApp, NewBackup, NewDeployment, NewDomain, NewSecret, NewServer, NewUser,
    ServerRole, Strategy, Timestamp, UserRole,
};
use sovereign_core::error::AppError;
use sovereign_core::ports::StoragePort;
use sovereign_storage_sqlite::SqliteState;

/// Helper: a uniquely-named in-memory state for tests. Each call gets
/// its own SQLite database via the shared-cache name trick, so tests
/// don't see each other's state.
async fn fresh() -> SqliteState {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let pid = std::process::id();
    SqliteState::open_in_memory_named(&format!("test-{pid}-{n}"))
        .await
        .expect("open in-memory")
}

// ---------------------------------------------------------------------------
// Migrations and meta
// ---------------------------------------------------------------------------

#[tokio::test]
async fn open_runs_migrations_and_creates_7_core_tables() {
    let s = fresh().await;
    let tables = s.core_tables().await.expect("core_tables");
    let expected: Vec<&str> = vec![
        "app",
        "audit_event",
        "backup",
        "deployment",
        "domain",
        "secret",
        "server",
        "user",
    ];
    for t in &expected {
        assert!(
            tables.contains(&t.to_string()),
            "missing table `{t}` in {tables:?}"
        );
    }
    // We expect 8 (7 core + audit_event), not 7.
    assert_eq!(
        tables.len(),
        expected.len(),
        "expected exactly 8 tables (7 core + audit), got {tables:?}"
    );
}

#[tokio::test]
async fn open_records_schema_version() {
    let s = fresh().await;
    let v = s.schema_version().await.expect("schema_version");
    // sqlx sets `PRAGMA user_version` to the latest applied migration.
    assert!(v.is_some(), "schema_version should be Some after migrate");
    assert!(v.unwrap() >= 1, "schema version should be at least 1");
}

#[tokio::test]
async fn open_enables_wal() {
    // In-memory SQLite does not support WAL — it always reports
    // `journal_mode = memory`. Test the WAL contract on a real file.
    let dir = tempdir();
    let path = dir.join("wal.db");
    let s = SqliteState::open(&path).await.expect("open on-disk");
    assert!(
        s.is_wal().await.unwrap(),
        "WAL mode should be active on disk"
    );
}

#[tokio::test]
async fn open_creates_parent_dir() {
    let dir = tempdir();
    let path: PathBuf = dir.join("nested/sub/sovereign.db");
    let s = SqliteState::open(&path)
        .await
        .expect("open with nested dirs");
    assert!(path.exists(), "db file should be created");
    let tables = s.core_tables().await.unwrap();
    assert!(tables.contains(&"app".to_string()));
}

fn tempdir() -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("sovereign-test-{}", std::process::id()));
    p.push(format!("{}", chrono_now_nanos()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn chrono_now_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

// ---------------------------------------------------------------------------
// App CRUD + optimistic concurrency
// ---------------------------------------------------------------------------

fn sample_app(name: &str) -> NewApp {
    NewApp {
        name: name.to_string(),
        owner: "user:alice".to_string(),
        env: AppEnv::Dev,
        git_repo: Some("git@github.com:alice/api.git".to_string()),
        image_ref: None,
        config_yaml: "kind: app\nname: api\n".to_string(),
        health_path: Some("/healthz".to_string()),
    }
}

#[tokio::test]
async fn app_create_get_list() {
    let s = fresh().await;
    let app = s
        .create_app(sample_app("api"), "user:alice")
        .await
        .expect("create");
    assert_eq!(app.name, "api");
    assert_eq!(app.version, 1);
    assert_eq!(app.env, AppEnv::Dev);
    assert_eq!(app.status, sovereign_core::domain::AppStatus::Active);

    let got = s.get_app(app.id).await.expect("get").expect("row exists");
    assert_eq!(got, app);

    let by_name = s
        .get_app_by_name("api")
        .await
        .expect("get_by_name")
        .expect("row exists");
    assert_eq!(by_name, app);

    let all = s.list_apps(None).await.expect("list");
    assert_eq!(all.len(), 1);
    assert_eq!(all[0], app);

    let owned = s.list_apps(Some("user:alice")).await.expect("list owner");
    assert_eq!(owned.len(), 1);
    let unowned = s.list_apps(Some("user:bob")).await.expect("list owner");
    assert_eq!(unowned.len(), 0);
}

#[tokio::test]
async fn app_unique_name_conflict() {
    let s = fresh().await;
    s.create_app(sample_app("api"), "user:alice").await.unwrap();
    let err = s
        .create_app(sample_app("api"), "user:alice")
        .await
        .expect_err("duplicate name should fail");
    matches!(err, AppError::Validation(_));
}

#[tokio::test]
async fn app_update_uses_optimistic_concurrency() {
    let s = fresh().await;
    let app = s.create_app(sample_app("api"), "user:alice").await.unwrap();

    // First update succeeds and bumps version.
    let updated = s
        .update_app(
            app.id,
            AppUpdate {
                env: Some(AppEnv::Prod),
                ..Default::default()
            },
            app.version,
            "user:alice",
        )
        .await
        .expect("update");
    assert_eq!(updated.env, AppEnv::Prod);
    assert_eq!(updated.version, app.version + 1);

    // Re-using the old version is a conflict.
    let err = s
        .update_app(
            app.id,
            AppUpdate {
                env: Some(AppEnv::Staging),
                ..Default::default()
            },
            app.version,
            "user:alice",
        )
        .await
        .expect_err("stale version should be a Conflict");
    match err {
        AppError::Conflict("app", v) => assert_eq!(v, app.version + 1),
        other => panic!("expected Conflict, got {other:?}"),
    }
}

#[tokio::test]
async fn app_archive_is_soft_delete() {
    let s = fresh().await;
    let app = s.create_app(sample_app("api"), "user:alice").await.unwrap();
    let archived = s
        .archive_app(app.id, app.version, "user:alice")
        .await
        .expect("archive");
    assert_eq!(archived.status, sovereign_core::domain::AppStatus::Archived);
    // The row still exists; it is just not Active.
    let still = s.get_app(app.id).await.expect("get").expect("row exists");
    assert_eq!(still.status, sovereign_core::domain::AppStatus::Archived);
}

#[tokio::test]
async fn app_update_nonexistent_is_not_found() {
    let s = fresh().await;
    let err = s
        .update_app(AppId::generate(), AppUpdate::default(), 1, "user:alice")
        .await
        .expect_err("missing app should be NotFound");
    assert!(matches!(err, AppError::NotFound("app")));
}

// ---------------------------------------------------------------------------
// Deployment lifecycle
// ---------------------------------------------------------------------------

#[tokio::test]
async fn deployment_full_lifecycle_happy_path() {
    let s = fresh().await;
    let app = s.create_app(sample_app("api"), "user:alice").await.unwrap();

    let d = s
        .begin_deployment(
            NewDeployment {
                app_id: app.id,
                image_ref: "ghcr.io/me/api:v1".to_string(),
                strategy: Strategy::BlueGreen,
                triggered_by: "user:alice".to_string(),
                risk_score: None,
            },
            "user:alice",
        )
        .await
        .expect("begin");
    assert_eq!(d.status, DeploymentStatus::Pending);
    assert_eq!(d.version, 1);

    let d = s
        .transition_deployment(
            d.id,
            DeploymentEvent {
                to: DeploymentStatus::Building,
                error: None,
                actor: "user:alice".to_string(),
            },
            "user:alice",
        )
        .await
        .expect("building");
    assert_eq!(d.status, DeploymentStatus::Building);
    assert_eq!(d.version, 2);

    let d = s
        .transition_deployment(
            d.id,
            DeploymentEvent {
                to: DeploymentStatus::Pushing,
                error: None,
                actor: "system".to_string(),
            },
            "system",
        )
        .await
        .expect("pushing");
    assert_eq!(d.status, DeploymentStatus::Pushing);

    let d = s
        .transition_deployment(
            d.id,
            DeploymentEvent {
                to: DeploymentStatus::Starting,
                error: None,
                actor: "system".to_string(),
            },
            "system",
        )
        .await
        .expect("starting");
    assert_eq!(d.status, DeploymentStatus::Starting);

    let d = s
        .transition_deployment(
            d.id,
            DeploymentEvent {
                to: DeploymentStatus::Healthy,
                error: None,
                actor: "system".to_string(),
            },
            "system",
        )
        .await
        .expect("healthy");
    assert_eq!(d.status, DeploymentStatus::Healthy);
    assert!(d.finished_at.is_some(), "Healthy sets finished_at");

    // `app.image_ref` should have been updated to the deployed image.
    let app = s.get_app(app.id).await.unwrap().unwrap();
    assert_eq!(app.image_ref.as_deref(), Some("ghcr.io/me/api:v1"));

    // The deployment should appear in the app's list.
    let list = s.list_deployments(app.id, 50).await.expect("list");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, d.id);
}

#[tokio::test]
async fn deployment_rejects_invalid_transition() {
    let s = fresh().await;
    let app = s.create_app(sample_app("api"), "user:alice").await.unwrap();
    let d = s
        .begin_deployment(
            NewDeployment {
                app_id: app.id,
                image_ref: "x:v1".to_string(),
                strategy: Strategy::Recreate,
                triggered_by: "system".to_string(),
                risk_score: None,
            },
            "system",
        )
        .await
        .unwrap();

    // Pending -> Healthy is forbidden (must walk the full lifecycle).
    let err = s
        .transition_deployment(
            d.id,
            DeploymentEvent {
                to: DeploymentStatus::Healthy,
                error: None,
                actor: "system".to_string(),
            },
            "system",
        )
        .await
        .expect_err("Pending -> Healthy should be invalid");
    match err {
        AppError::InvalidTransition("deployment", from, to) => {
            assert_eq!(from, "pending");
            assert_eq!(to, "healthy");
        }
        other => panic!("expected InvalidTransition, got {other:?}"),
    }
}

#[tokio::test]
async fn deployment_rollback_works() {
    let s = fresh().await;
    let app = s.create_app(sample_app("api"), "user:alice").await.unwrap();
    let mut d = s
        .begin_deployment(
            NewDeployment {
                app_id: app.id,
                image_ref: "x:v1".to_string(),
                strategy: Strategy::Recreate,
                triggered_by: "system".to_string(),
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
                    actor: "system".to_string(),
                },
                "system",
            )
            .await
            .unwrap();
    }
    let rolled = s
        .transition_deployment(
            d.id,
            DeploymentEvent {
                to: DeploymentStatus::RolledBack,
                error: None,
                actor: "user:alice".to_string(),
            },
            "user:alice",
        )
        .await
        .unwrap();
    assert_eq!(rolled.status, DeploymentStatus::RolledBack);
}

#[tokio::test]
async fn deployment_failed_records_error() {
    let s = fresh().await;
    let app = s.create_app(sample_app("api"), "user:alice").await.unwrap();
    let d = s
        .begin_deployment(
            NewDeployment {
                app_id: app.id,
                image_ref: "x:v1".to_string(),
                strategy: Strategy::Recreate,
                triggered_by: "system".to_string(),
                risk_score: None,
            },
            "system",
        )
        .await
        .unwrap();
    let failed = s
        .transition_deployment(
            d.id,
            DeploymentEvent {
                to: DeploymentStatus::Failed,
                error: Some("healthcheck 503".to_string()),
                actor: "system".to_string(),
            },
            "system",
        )
        .await
        .unwrap();
    assert_eq!(failed.status, DeploymentStatus::Failed);
    assert_eq!(failed.error.as_deref(), Some("healthcheck 503"));
}

// ---------------------------------------------------------------------------
// Secret / Domain / Server / Backup / User
// ---------------------------------------------------------------------------

#[tokio::test]
async fn secret_put_rotate_delete() {
    let s = fresh().await;
    let app = s.create_app(sample_app("api"), "user:alice").await.unwrap();

    let sec = s
        .put_secret(
            NewSecret {
                app_id: app.id,
                key: "DB_URL".to_string(),
                ciphertext: b"encrypted-1".to_vec(),
            },
            "user:alice",
        )
        .await
        .expect("put");
    assert_eq!(sec.key, "DB_URL");
    assert_eq!(sec.ciphertext, b"encrypted-1");

    // Rotate.
    let rotated = s
        .rotate_secret(sec.id, b"encrypted-2".to_vec(), "user:alice")
        .await
        .expect("rotate");
    assert_eq!(rotated.ciphertext, b"encrypted-2");
    assert!(rotated.rotated_at.is_some());
    assert!(rotated.version > sec.version);

    // List.
    let listed = s.list_secrets(app.id).await.expect("list");
    assert_eq!(listed.len(), 1);

    // Delete.
    s.delete_secret(sec.id, "user:alice").await.expect("delete");
    let after = s.list_secrets(app.id).await.expect("list");
    assert_eq!(after.len(), 0);
}

#[tokio::test]
async fn secret_unique_per_app() {
    let s = fresh().await;
    let app = s.create_app(sample_app("api"), "user:alice").await.unwrap();
    s.put_secret(
        NewSecret {
            app_id: app.id,
            key: "DB_URL".to_string(),
            ciphertext: vec![1],
        },
        "user:alice",
    )
    .await
    .unwrap();
    let err = s
        .put_secret(
            NewSecret {
                app_id: app.id,
                key: "DB_URL".to_string(),
                ciphertext: vec![2],
            },
            "user:alice",
        )
        .await
        .expect_err("dup secret should be rejected");
    assert!(matches!(err, AppError::Validation(_)));
}

#[tokio::test]
async fn domain_add_list_remove() {
    let s = fresh().await;
    let app = s.create_app(sample_app("api"), "user:alice").await.unwrap();
    let d = s
        .add_domain(
            NewDomain {
                app_id: app.id,
                hostname: "api.example.com".to_string(),
            },
            "user:alice",
        )
        .await
        .expect("add");
    let list = s.list_domains(app.id).await.expect("list");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].hostname, "api.example.com");
    s.remove_domain(d.id, "user:alice").await.expect("remove");
    let after = s.list_domains(app.id).await.expect("list");
    assert_eq!(after.len(), 0);
}

#[tokio::test]
async fn server_add_touch_set_status() {
    let s = fresh().await;
    let srv = s
        .add_server(NewServer {
            hostname: "box-1".to_string(),
            role: ServerRole::ControlPlane,
            api_url: "http://127.0.0.1:7878".to_string(),
            cpu_cores: Some(4),
            mem_mb: Some(8192),
            disk_gb: Some(80),
        })
        .await
        .expect("add");
    let got = s.get_server(srv.id).await.expect("get").expect("exists");
    assert_eq!(got.hostname, "box-1");
    assert_eq!(got.role, ServerRole::ControlPlane);

    let drained = s
        .set_server_status(
            srv.id,
            sovereign_core::domain::ServerStatus::Drained,
            "user:alice",
        )
        .await
        .expect("set_status");
    assert_eq!(
        drained.status,
        sovereign_core::domain::ServerStatus::Drained
    );
    s.touch_server(srv.id, Timestamp::now())
        .await
        .expect("touch");
}

#[tokio::test]
async fn backup_begin_complete_verify() {
    let s = fresh().await;
    let b = s
        .begin_backup(
            NewBackup {
                target: "sqlite".to_string(),
            },
            "system",
        )
        .await
        .expect("begin");
    assert_eq!(b.status, sovereign_core::domain::BackupStatus::Pending);

    let done = s
        .complete_backup(
            b.id,
            sovereign_core::domain::BackupStatus::Success,
            Some(1024 * 1024),
            Some("/var/lib/sovereign/backups/2026-06-04.db".to_string()),
            None,
            "system",
        )
        .await
        .expect("complete");
    assert_eq!(done.status, sovereign_core::domain::BackupStatus::Success);
    assert_eq!(done.size_bytes, Some(1024 * 1024));

    let verified = s
        .verify_backup(
            b.id,
            serde_json::json!({ "row_count_diff": [], "tables": 8 }),
            "system",
        )
        .await
        .expect("verify");
    assert_eq!(
        verified.status,
        sovereign_core::domain::BackupStatus::Verified
    );
    assert!(verified.verified_at.is_some());
    assert!(verified.verify_result.is_some());
}

#[tokio::test]
async fn user_create_role_change() {
    let s = fresh().await;
    let u = s
        .create_user(NewUser {
            email: "alice@example.com".to_string(),
            role: UserRole::Developer,
        })
        .await
        .expect("create");
    assert_eq!(u.role, UserRole::Developer);

    let promoted = s
        .set_user_role(u.id, UserRole::Admin, "user:root")
        .await
        .expect("set_role");
    assert_eq!(promoted.role, UserRole::Admin);
    s.touch_user(u.id, Timestamp::now()).await.expect("touch");
    let fetched = s.get_user_by_email("alice@example.com").await.unwrap();
    assert!(fetched.is_some());
}

// ---------------------------------------------------------------------------
// Audit log
// ---------------------------------------------------------------------------

#[tokio::test]
async fn audit_appends_and_queries() {
    let s = fresh().await;
    s.append_audit(AuditEvent {
        id: None,
        ts: Timestamp::now(),
        actor: "user:alice".to_string(),
        kind: AuditKind::System,
        target: Some("system:boot".to_string()),
        payload: serde_json::json!({ "msg": "first event" }),
        policy_decision: None,
    })
    .await
    .expect("append");

    let count = s.count_audit().await.expect("count");
    assert_eq!(count, 1);

    let events = s.query_audit(AuditQuery::default()).await.expect("query");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].actor, "user:alice");
    assert!(events[0].id.is_some(), "id is populated after insert");
}

#[tokio::test]
async fn audit_filter_by_kind_and_actor() {
    let s = fresh().await;
    for (i, kind) in [
        AuditKind::AppLifecycle,
        AuditKind::Deploy,
        AuditKind::SecretChange,
    ]
    .iter()
    .enumerate()
    {
        s.append_audit(AuditEvent {
            id: None,
            ts: Timestamp::now(),
            actor: format!("user:{}", i),
            kind: kind.clone(),
            target: Some(format!("t:{i}")),
            payload: serde_json::json!({"i": i}),
            policy_decision: None,
        })
        .await
        .unwrap();
    }
    let q = AuditQuery {
        kind: Some(AuditKind::Deploy),
        ..Default::default()
    };
    let events = s.query_audit(q).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, AuditKind::Deploy);
}

#[tokio::test]
async fn audit_mutation_writes_a_corresponding_event() {
    // Every mutating use case (create_app, begin_deployment, …) writes
    // an audit row in the same transaction. This test asserts that the
    // audit count grows in lockstep with the mutations.
    let s = fresh().await;
    assert_eq!(s.count_audit().await.unwrap(), 0);
    s.create_app(sample_app("api"), "user:alice").await.unwrap();
    assert_eq!(s.count_audit().await.unwrap(), 1);
    s.archive_app(s.list_apps(None).await.unwrap()[0].id, 1, "user:alice")
        .await
        .unwrap();
    assert_eq!(s.count_audit().await.unwrap(), 2);
}

// ---------------------------------------------------------------------------
// Trust anchor: the audit triggers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn audit_update_is_rejected_by_trigger() {
    // The `audit_no_update` trigger is the structural tamper-evidence
    // for the audit log. If this test ever passes a non-error result,
    // the trust anchor has been compromised.
    let s = fresh().await;
    s.append_audit(AuditEvent {
        id: None,
        ts: Timestamp::now(),
        actor: "user:alice".to_string(),
        kind: AuditKind::System,
        target: None,
        payload: serde_json::json!({}),
        policy_decision: None,
    })
    .await
    .unwrap();

    let res = sqlx::query("UPDATE audit_event SET actor = 'attacker' WHERE id = 1")
        .execute(s.pool())
        .await;
    let err = res.expect_err("UPDATE on audit_event should be rejected");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("append-only") || msg.contains("ABORT"),
        "expected append-only abort, got: {msg}"
    );
}

#[tokio::test]
async fn audit_delete_is_rejected_by_trigger() {
    let s = fresh().await;
    s.append_audit(AuditEvent {
        id: None,
        ts: Timestamp::now(),
        actor: "user:alice".to_string(),
        kind: AuditKind::System,
        target: None,
        payload: serde_json::json!({}),
        policy_decision: None,
    })
    .await
    .unwrap();

    let res = sqlx::query("DELETE FROM audit_event WHERE id = 1")
        .execute(s.pool())
        .await;
    let err = res.expect_err("DELETE on audit_event should be rejected");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("append-only") || msg.contains("ABORT"),
        "expected append-only abort, got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// F5: rollback primitives
// ---------------------------------------------------------------------------
//
// Three new `StoragePort` methods power the `sovereign rollback` flow:
//
//   - `get_current_deployment(app_id)` — the *most recent* `Healthy` row
//     for the app (what's running right now).
//   - `list_healthy_deployments_before(app_id, before, limit)` — every
//     `Healthy` row with `started_at < before`, DESC, capped at `limit`.
//   - `set_rollback_target(deployment_id, target_id, actor)` —
//     stamps `target_deployment_id` on a deployment and bumps its
//     version (OCC).

/// Helper: drive a deployment to `Healthy` and return it.
async fn drive_to_healthy(s: &SqliteState, app_id: AppId, image: &str) -> Deployment {
    let mut d = s
        .begin_deployment(
            NewDeployment {
                app_id,
                image_ref: image.to_string(),
                strategy: Strategy::Recreate,
                triggered_by: "system".to_string(),
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
                    actor: "system".to_string(),
                },
                "system",
            )
            .await
            .unwrap();
    }
    d
}

#[tokio::test]
async fn rollback_get_current_deployment_returns_most_recent_healthy() {
    let s = fresh().await;
    let app = s.create_app(sample_app("api"), "user:alice").await.unwrap();

    // No deploys yet — must be `None`, not a panic.
    assert!(s.get_current_deployment(app.id).await.unwrap().is_none());

    // Two healthy deploys; the second one is "current". A full
    // second of sleep is needed because `started_at` is unix-second
    // precision — sub-second sleeps would tie and the secondary
    // `id DESC` tiebreak would be meaningless for random UUIDs.
    let _older = drive_to_healthy(&s, app.id, "x:v1").await;
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let newer = drive_to_healthy(&s, app.id, "x:v2").await;

    let current = s
        .get_current_deployment(app.id)
        .await
        .unwrap()
        .expect("current");
    assert_eq!(current.id, newer.id);
    assert_eq!(current.image_ref, "x:v2");
    assert_eq!(current.status, DeploymentStatus::Healthy);
}

#[tokio::test]
async fn rollback_list_healthy_deployments_before_excludes_current_and_failed() {
    let s = fresh().await;
    let app = s.create_app(sample_app("api"), "user:alice").await.unwrap();

    let old = drive_to_healthy(&s, app.id, "x:v1").await;
    // Force a distinct `started_at` second so DESC ordering is
    // deterministic. Sub-second gaps rely on `id DESC` tiebreak,
    // which is meaningless for random UUIDs.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let middle = drive_to_healthy(&s, app.id, "x:v2").await;
    // A `Failed` deploy after `middle` should be filtered out by the
    // `status = 'healthy'` clause.
    let failed = s
        .begin_deployment(
            NewDeployment {
                app_id: app.id,
                image_ref: "x:v3-bad".to_string(),
                strategy: Strategy::Recreate,
                triggered_by: "system".to_string(),
                risk_score: None,
            },
            "system",
        )
        .await
        .unwrap();
    s.transition_deployment(
        failed.id,
        DeploymentEvent {
            to: DeploymentStatus::Building,
            error: None,
            actor: "system".to_string(),
        },
        "system",
    )
    .await
    .unwrap();
    s.transition_deployment(
        failed.id,
        DeploymentEvent {
            to: DeploymentStatus::Failed,
            error: Some("boom".into()),
            actor: "system".to_string(),
        },
        "system",
    )
    .await
    .unwrap();

    // The boundary is "started_at < now+1s" — v1 and v2 are *before* failed.
    let before = Timestamp(failed.started_at.as_secs() + 1);
    let history = s
        .list_healthy_deployments_before(app.id, before, 10)
        .await
        .unwrap();
    let ids: Vec<_> = history.iter().map(|d| d.id).collect();

    assert!(
        ids.contains(&old.id),
        "v1 should be in the rollback history"
    );
    assert!(
        ids.contains(&middle.id),
        "v2 should be in the rollback history"
    );
    assert!(
        !ids.contains(&failed.id),
        "Failed deploy is excluded by status filter"
    );
    assert!(
        ids.iter().position(|id| *id == middle.id).unwrap()
            < ids.iter().position(|id| *id == old.id).unwrap(),
        "DESC order: v2 (newer) should come before v1 (older)"
    );
    assert!(history
        .iter()
        .all(|d| d.status == DeploymentStatus::Healthy));
    assert_eq!(history.len(), 2, "expected v1+v2, got {history:?}");
}

#[tokio::test]
async fn rollback_list_healthy_deployments_before_respects_limit() {
    let s = fresh().await;
    let app = s.create_app(sample_app("api"), "user:alice").await.unwrap();
    for v in 0..5 {
        let img = format!("x:v{v}");
        drive_to_healthy(&s, app.id, &img).await;
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    }
    let now = Timestamp::now();
    let h2 = s
        .list_healthy_deployments_before(app.id, Timestamp(now.as_secs() + 1), 2)
        .await
        .unwrap();
    let h5 = s
        .list_healthy_deployments_before(app.id, Timestamp(now.as_secs() + 1), 5)
        .await
        .unwrap();
    assert_eq!(h2.len(), 2, "limit=2 should cap to 2");
    assert_eq!(h5.len(), 5, "limit=5 should return all 5");
}

#[tokio::test]
async fn rollback_set_target_stamps_and_audits() {
    let s = fresh().await;
    let app = s.create_app(sample_app("api"), "user:alice").await.unwrap();
    let target = drive_to_healthy(&s, app.id, "x:v1").await;
    let mut current = drive_to_healthy(&s, app.id, "x:v2").await;
    assert!(
        current.target_deployment_id.is_none(),
        "no target on a fresh deploy"
    );

    current = s
        .set_rollback_target(current.id, target.id, "user:alice")
        .await
        .unwrap();
    assert_eq!(current.target_deployment_id, Some(target.id));
    assert!(
        current.version >= 2,
        "version should bump on OCC update, got {}",
        current.version
    );

    // The audit log records the rollback decision. The writer
    // formats the `target` column as `"deployment:<uuid>"`, so we
    // search by the prefixed form (the `query_audit` filter is
    // exact-match).
    let aud = s
        .query_audit(AuditQuery {
            target: Some(format!("deployment:{}", current.id)),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(!aud.is_empty(), "rollback should append an audit event");
    let payloads: Vec<&serde_json::Value> = aud.iter().map(|a| &a.payload).collect();
    assert!(
        payloads.iter().any(|p| {
            p.get("rollback").and_then(|v| v.as_str()) == Some("set_target")
                && p.get("target_deployment_id").is_some()
        }),
        "expected an audit event with rollback=set_target, got {payloads:?}"
    );
}

#[tokio::test]
async fn rollback_set_target_unknown_deployment_is_not_found() {
    let s = fresh().await;
    let app = s.create_app(sample_app("api"), "user:alice").await.unwrap();
    let _d = drive_to_healthy(&s, app.id, "x:v1").await;
    let bogus = sovereign_core::domain::DeploymentId::generate();
    let res = s.set_rollback_target(bogus, bogus, "user:alice").await;
    assert!(
        matches!(res, Err(AppError::NotFound(_))),
        "expected NotFound, got {res:?}"
    );
}
