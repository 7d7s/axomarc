// F4 wiring: the `sovereign deploy` subcommand.
//
// At V0, the wiring is intentionally thin — the heavy lifting is in
// `sovereign_core::use_cases::deploy::start_deploy`. The CLI:
//   1. opens storage at the default path
//   2. connects the Docker runtime (env DOCKER_HOST or platform default)
//   3. resolves the app by name (the CLI gets a `String` from `--app`;
//      we look it up in storage by name, not by id)
//   4. calls the use case
//   5. prints the URL on success / the error on failure
//
// If no Docker daemon is available the subcommand returns a clean
// Upstream error — the operator sees a clear "cannot connect to
// docker" message instead of a bollard stack trace.

use std::sync::Arc;

use anyhow::Context;
use sovereign_core::domain::Strategy;
use sovereign_core::error::AppError;
use sovereign_core::ports::{RuntimePort, StoragePort};
use sovereign_core::state::AppState;
use sovereign_core::use_cases::deploy::{self, DeployRequest};
use sovereign_runtime_docker::DockerRuntime;
use sovereign_storage_sqlite::SqliteState;
use tracing::instrument;

use crate::cli::Cmd;
use crate::commands::Dispatch;
use crate::exit::AppExit;
use crate::output::{Envelope, Output};

/// Run the `deploy` subcommand. The clap-parsed args are passed in
/// directly so this is easy to test.
#[instrument(skip(out))]
pub async fn run(cmd: &Cmd, out: &Output) -> Dispatch {
    let (app_name, image, strategy, wait, no_lock, actor) = match cmd {
        Cmd::Deploy {
            app,
            image,
            strategy,
            wait,
            no_lock,
        } => (
            app.clone().unwrap_or_default(),
            image.clone(),
            *strategy,
            *wait,
            *no_lock,
            "cli".to_string(),
        ),
        _ => return Dispatch::Err(AppExit::Generic),
    };

    if app_name.is_empty() {
        return err(out, AppExit::Usage, "deploy requires --app <NAME> (or run `sovereign init` first)");
    }

    // 1. Open storage. The DB lives in the data dir; for V0 we just
    //    look in the platform-default location and let the open fail
    //    cleanly with a "data dir not initialized" message.
    let db_path = default_db_path();
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

    // 2. Resolve the app by name. If it's not there, we fail fast
    //    with a NotFound error message (rather than starting a
    //    half-deploy).
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

    // 3. Resolve the image. Prefer the CLI's --image flag, then the
    //    app's pinned `image_ref` (set by the last successful deploy
    //    or by `sovereign init`).
    let image_ref = image
        .or_else(|| app.image_ref.clone())
        .ok_or_else(|| AppError::validation("either --image or app.image_ref must be set"));
    let image_ref = match image_ref {
        Ok(s) => s,
        Err(e) => return err(out, AppExit::Usage, &format!("{e}")),
    };

    // 4. Connect the Docker runtime. If it fails, the operator sees
    //    a clear "cannot connect to docker" error.
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

    let state = AppState { storage, runtime };
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
            // F5 sub-task 5: write the `sovereign.lock` receipt for
            // healthy deploys. Best-effort — a failure to write the
            // lock or to `git push` is a warning, not a deploy fail
            // (the audit log + DB row are the source of truth).
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
                    body["lock_warning"] = serde_json::json!(
                        "deploy is healthy but sovereign.lock was not written"
                    );
                }
                let env = Envelope::<serde_json::Value>::ok(body);
                let _ = out.success(&env);
            }
            Dispatch::Ok
        }
        Err(e) => err(out, AppExit::Upstream, &format!("deploy failed: {e}")),
    }
}

/// Convert the CLI's clap-derive `Strategy` enum into the core's
/// domain `Strategy` enum. They have the same variants today; the
/// indirection lets the CLI surface stay in `cli.rs` (clap deps)
/// while the domain stays free of clap.
fn cli_strategy_to_core(s: crate::cli::Strategy) -> Strategy {
    match s {
        crate::cli::Strategy::BlueGreen => Strategy::BlueGreen,
        crate::cli::Strategy::Rolling => Strategy::Rolling,
        crate::cli::Strategy::Recreate => Strategy::Recreate,
    }
}

/// Resolve the default data dir. Linux: `~/.local/share/sovereign/sovereign.db`.
/// macOS: `~/Library/Application Support/sovereign/sovereign.db`.
/// Windows: `%APPDATA%\sovereign\sovereign.db`. Falls back to `./sovereign.db`.
pub(crate) fn default_db_path() -> std::path::PathBuf {
    if let Some(dir) = dirs_data() {
        dir.join("sovereign").join("sovereign.db")
    } else {
        std::path::PathBuf::from("./sovereign.db")
    }
}

#[cfg(unix)]
fn dirs_data() -> Option<std::path::PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| {
                let mut p = std::path::PathBuf::from(h);
                p.push(".local/share");
                p
            })
        })
}

#[cfg(windows)]
fn dirs_data() -> Option<std::path::PathBuf> {
    std::env::var_os("APPDATA").map(std::path::PathBuf::from)
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
