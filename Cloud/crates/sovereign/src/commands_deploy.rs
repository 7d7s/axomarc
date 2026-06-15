// F4 wiring: the `sovereign deploy` subcommand.
//
// At V0, the wiring is intentionally thin — the heavy lifting is in
// `sovereign_core::use_cases::deploy::start_deploy`. The CLI:
//   1. opens storage at the default path
//   2. resolves the app by name (the CLI gets a `String` from `--app`;
//      we look it up in storage by name, not by id)
//   3. if deploy_mode == Native, call native_deploy()
//   4. otherwise, connect Docker runtime and call start_deploy()
//   5. prints the URL on success / the error on failure
//
// If no Docker daemon is available the subcommand returns a clean
// Upstream error — the operator sees a clear "cannot connect to
// docker" message instead of a bollard stack trace.

use std::sync::Arc;

use anyhow::Context;
use sovereign_core::domain::{DeployMode, Strategy};
use sovereign_core::error::AppError;
use sovereign_core::ports::{RuntimePort, StoragePort};
use sovereign_core::state::AppState;
use sovereign_core::use_cases::deploy::{self, DeployRequest};
use sovereign_runtime_docker::DockerRuntime;
use sovereign_storage_sqlite::SqliteState;
use tracing::instrument;

use crate::cli::Cmd;
use crate::commands::Dispatch;
use crate::connect;
use crate::exit::AppExit;
use crate::output::{Envelope, Output};

/// Run the `deploy` subcommand. The clap-parsed args are passed in
/// directly so this is easy to test.
#[instrument(skip(out))]
pub async fn run(cmd: &Cmd, out: &Output) -> Dispatch {
    let (app_name, image, strategy, wait, no_lock, actor, native_binary, native_exec_start) =
        match cmd {
            Cmd::Deploy {
                app,
                image,
                strategy,
                wait,
                no_lock,
                native_binary,
                native_exec_start,
            } => (
                app.clone().unwrap_or_default(),
                image.clone(),
                *strategy,
                *wait,
                *no_lock,
                "cli".to_string(),
                native_binary.clone(),
                native_exec_start.clone(),
            ),
            _ => return Dispatch::Err(AppExit::Generic),
        };

    if app_name.is_empty() {
        return err(
            out,
            AppExit::Usage,
            "deploy requires --app <NAME> (or run `sovereign init` first)",
        );
    }

    // 1. Open storage.
    let db_path = connect::default_db_path();
    let storage = match SqliteState::open(&db_path).await {
        Ok(s) => Arc::new(s) as Arc<dyn StoragePort>,
        Err(e) => {
            return err(
                out,
                AppExit::Upstream,
                &format!(
                    "cannot open data dir at {}: {e}. Run `sovereign init` first?",
                    db_path.display()
                ),
            );
        }
    };

    // 2. Resolve the app by name.
    let app = match storage
        .get_app_by_name(&app_name)
        .await
        .context("looking up app")
    {
        Ok(Some(a)) => a,
        Ok(None) => {
            return err(
                out,
                AppExit::Usage,
                &format!("no app named `{app_name}`. Run `sovereign init` to create one."),
            );
        }
        Err(e) => return err(out, AppExit::Generic, &format!("storage error: {e}")),
    };

    // 3. Branch on deploy mode.
    if app.deploy_mode == DeployMode::Native {
        return run_native(out, &app_name, &app, image, native_binary, native_exec_start, &db_path).await;
    }

    // Docker path (pull / build / pack modes).
    let image_ref = image
        .or_else(|| app.image_ref.clone())
        .ok_or_else(|| AppError::validation("either --image or app.image_ref must be set"));
    let image_ref = match image_ref {
        Ok(s) => s,
        Err(e) => return err(out, AppExit::Usage, &format!("{e}")),
    };

    let runtime = match DockerRuntime::connect_from_env().await {
        Ok(r) => Arc::new(r) as Arc<dyn RuntimePort>,
        Err(e) => {
            return err(
                out,
                AppExit::Upstream,
                &format!(
                    "cannot connect to docker: {e}. Is the daemon running? (DOCKER_HOST={:?})",
                    std::env::var("DOCKER_HOST").ok()
                ),
            );
        }
    };

    let state = AppState {
        storage,
        runtime,
        proxy: connect::connect_proxy().await,
        secrets: connect::connect_secrets(),
        backup: connect::connect_backup(),
        db_path,
    };
    let req = DeployRequest {
        app_id: app.id,
        image_ref: Some(image_ref),
        strategy: cli_strategy_to_core(strategy),
        wait,
        actor: actor.clone(),
    };

    if out.format() == crate::output::Format::Text {
        let _ = out.text(&format!(
            "deploying {} (strategy={:?}, wait={})...",
            app.name, req.strategy, wait
        ));
    }

    match deploy::start_deploy(&state, req).await {
        Ok(result) => {
            let lock_outcome = if !no_lock
                && result.deployment.status == sovereign_core::domain::DeploymentStatus::Healthy
            {
                crate::lock::write(
                    &app.name,
                    &result.deployment,
                    &actor,
                    app.git_repo.as_deref(),
                )
                .ok()
            } else {
                None
            };

            if out.format() == crate::output::Format::Text {
                if let Some(o) = &lock_outcome {
                    let _ = out.text(&format!(
                        "{} is live at {} (wrote {})",
                        app.name,
                        result.url,
                        o.path.display()
                    ));
                } else {
                    let _ = out.text(&format!("{} is live at {}", app.name, result.url));
                }
            } else {
                let mut body = serde_json::json!({
                    "app": app.name,
                    "deployment_id": result.deployment.id,
                    "status": result.deployment.status,
                    "url": result.url,
                    "lock": lock_outcome.as_ref().map(|o| serde_json::json!({
                        "path": o.path,
                        "committed": o.committed,
                        "pushed": o.pushed,
                    })),
                });
                if !no_lock && lock_outcome.is_none() {
                    body["lock_warning"] =
                        serde_json::json!("deploy is healthy but sovereign.lock was not written");
                }
                let env = Envelope::<serde_json::Value>::ok(body);
                let _ = out.success(&env);
            }
            Dispatch::Ok
        }
        Err(e) => err(out, AppExit::Upstream, &format!("deploy failed: {e}")),
    }
}

/// Native deploy path: extract binary from .sov archive, install systemd unit.
async fn run_native(
    out: &Output,
    app_name: &str,
    app: &sovereign_core::domain::App,
    archive_path: Option<String>,
    native_binary: Option<String>,
    native_exec_start: Option<String>,
    db_path: &std::path::Path,
) -> Dispatch {
    let archive = match archive_path {
        Some(p) => std::path::PathBuf::from(p),
        None => {
            return err(
                out,
                AppExit::Usage,
                "native deploy requires --image <path-to.sov>",
            );
        }
    };
    if !archive.exists() {
        return err(
            out,
            AppExit::Usage,
            &format!("archive not found: {}", archive.display()),
        );
    }

    let binary_path = native_binary.unwrap_or_else(|| app_name.to_string());
    let exec_start = native_exec_start.unwrap_or_else(|| format!("./{app_name} --port {{port}}"));
    let health_path = app.health_path.as_deref().unwrap_or("/health");

    // Decrypt secrets for env injection. Build a minimal AppState
    // just for secret resolution (native deploy doesn't use RuntimePort).
    let storage = match SqliteState::open(db_path).await {
        Ok(s) => Arc::new(s) as Arc<dyn StoragePort>,
        Err(e) => {
            return err(out, AppExit::Upstream, &format!("reopen storage: {e}"));
        }
    };
    let secrets = connect::connect_secrets();
    // env_for_deploy needs AppState but we don't have a RuntimePort.
    // Build a minimal state with a dummy runtime (never used for native).
    let dummy_runtime: Arc<dyn RuntimePort> = Arc::new(DummyRuntime);
    let state = AppState {
        storage,
        runtime: dummy_runtime,
        proxy: None,
        secrets,
        backup: None,
        db_path: db_path.to_path_buf(),
    };
    let env = match sovereign_core::use_cases::secret::env_for_deploy(&state, app.id).await {
        Ok(e) => e,
        Err(e) => {
            return err(out, AppExit::Upstream, &format!("secret resolve: {e}"));
        }
    };

    if out.format() == crate::output::Format::Text {
        let _ = out.text(&format!(
            "deploying {} (native mode, archive={})...",
            app.name,
            archive.display()
        ));
    }

    match crate::native_runtime::native_deploy(
        app_name,
        &archive,
        &binary_path,
        &exec_start,
        &env,
        health_path,
    )
    .await
    {
        Ok(result) => {
            let url = format!("http://127.0.0.1:{}", result.port);
            if out.format() == crate::output::Format::Text {
                let _ = out.text(&format!(
                    "{} is live at {} (port={}, user={})",
                    app.name, url, result.port, result.user
                ));
            } else {
                let env = Envelope::<serde_json::Value>::ok(serde_json::json!({
                    "app": app.name,
                    "mode": "native",
                    "port": result.port,
                    "url": url,
                    "bin_path": result.bin_path,
                    "user": result.user,
                }));
                let _ = out.success(&env);
            }
            Dispatch::Ok
        }
        Err(e) => err(out, AppExit::Upstream, &format!("native deploy failed: {e}")),
    }
}

/// Convert the CLI's clap-derive `Strategy` enum into the core's
/// domain `Strategy` enum.
fn cli_strategy_to_core(s: crate::cli::Strategy) -> Strategy {
    match s {
        crate::cli::Strategy::BlueGreen => Strategy::BlueGreen,
        crate::cli::Strategy::Rolling => Strategy::Rolling,
        crate::cli::Strategy::Recreate => Strategy::Recreate,
    }
}

/// Re-export `connect::default_db_path` for callers that still
/// reference it as `commands_deploy::default_db_path`.
pub(crate) fn default_db_path() -> std::path::PathBuf {
    connect::default_db_path()
}

fn err(out: &Output, code: AppExit, msg: &str) -> Dispatch {
    if out.format() == crate::output::Format::Text {
        let _ = out.err(msg);
    } else {
        let env = Envelope::<serde_json::Value>::err(code, serde_json::json!({"error": msg}), msg);
        let _ = out.error(&env);
    }
    Dispatch::Err(code)
}

/// Placeholder runtime for native deploy (secret resolution only).
/// Never actually called — native deploy uses sovereign-systemd directly.
struct DummyRuntime;

#[async_trait::async_trait]
impl RuntimePort for DummyRuntime {
    async fn pull_image(&self, _: &str) -> Result<(), AppError> {
        Err(AppError::internal("dummy runtime"))
    }
    async fn create_container(&self, _: sovereign_core::ports::ContainerSpec) -> Result<String, AppError> {
        Err(AppError::internal("dummy runtime"))
    }
    async fn start_container(&self, _: &str) -> Result<(), AppError> {
        Err(AppError::internal("dummy runtime"))
    }
    async fn stop_container(&self, _: &str, _: std::time::Duration) -> Result<(), AppError> {
        Err(AppError::internal("dummy runtime"))
    }
    async fn remove_container(&self, _: &str) -> Result<(), AppError> {
        Err(AppError::internal("dummy runtime"))
    }
    async fn healthcheck(&self, _: &str, _: u16, _: &str, _: std::time::Duration) -> Result<sovereign_core::ports::HealthResult, AppError> {
        Err(AppError::internal("dummy runtime"))
    }
}
