// F5 wiring: the `sovereign rollback` subcommand.
//
// Mirrors `commands_deploy.rs`. The CLI resolves the app by name,
// opens storage + docker (same as deploy), and calls
// `sovereign_core::use_cases::rollback::start_rollback`.
//
// Sub-modes:
//   - `sovereign rollback <app>`                — roll to previous healthy
//   - `sovereign rollback <app> --to=<id>`      — roll to specific deployment
//   - `sovereign rollback <app> --list`         — print the table of healthy deploys
//   - `sovereign rollback <app> --list --json`  — print as JSON
//   - `sovereign rollback <app> --dry-run`      — print what we'd do, exit 0
//
// `--list` does NOT touch the runtime; it's a pure storage read.

use std::sync::Arc;

use anyhow::Context;
use sovereign_core::domain::Timestamp;
use sovereign_core::ports::{RuntimePort, StoragePort};
use sovereign_core::state::AppState;
use sovereign_core::use_cases::rollback::{self, RollbackRequest};
use sovereign_runtime_docker::DockerRuntime;
use sovereign_storage_sqlite::SqliteState;
use tracing::instrument;

use crate::cli::Cmd;
use crate::commands::Dispatch;
use crate::commands_deploy::{connect_proxy, default_db_path};
use crate::exit::AppExit;
use crate::output::{Envelope, Output};

/// Run the `rollback` subcommand. Pulls the parsed args from the
/// clap enum; the variant is collapsed in `commands.rs` first.
#[instrument(skip(out))]
pub async fn run(cmd: &Cmd, out: &Output) -> Dispatch {
    let (app_name, to, list, limit) = match cmd {
        Cmd::Rollback {
            app,
            to,
            list,
            limit,
        } => (app.clone(), to.clone(), *list, *limit),
        _ => return Dispatch::Err(AppExit::Generic),
    };

    if app_name.is_empty() {
        return err(
            out,
            AppExit::Usage,
            "rollback requires an app name (e.g. `sovereign rollback api`)",
        );
    }

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

    let app = match storage
        .get_app_by_name(&app_name)
        .await
        .context("lookup app")
    {
        Ok(Some(a)) => a,
        Ok(None) => {
            return err(out, AppExit::Usage, &format!("no app named `{app_name}`."));
        }
        Err(e) => return err(out, AppExit::Generic, &format!("storage error: {e}")),
    };

    if list {
        return run_list(&storage, app.id, limit, out).await;
    }

    // Live rollback path. We need the runtime to drive the container
    // swap. If docker isn't available, surface a clean error.
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
        proxy: connect_proxy().await,
    };

    let req = RollbackRequest {
        app_id: app.id,
        to: match to.as_deref() {
            Some(s) => match parse_deployment_id(s) {
                Ok(id) => Some(id),
                Err(e) => {
                    return err(out, AppExit::Usage, &format!("--to: {e}"));
                }
            },
            None => None,
        },
        actor: "cli".to_string(),
    };

    if out.format() == crate::output::Format::Text {
        let _ = out.text(&format!(
            "rolling back {} (to={})...",
            app.name,
            to.as_deref().unwrap_or("previous-healthy")
        ));
    }

    match rollback::start_rollback(&state, req).await {
        Ok(r) => {
            if out.format() == crate::output::Format::Text {
                let _ = out.text(&format!(
                    "{} rolled back; serving {} (deployment {})",
                    app.name, r.target_image, r.deployment.id
                ));
            } else {
                let env = Envelope::<serde_json::Value>::ok(serde_json::json!({
                    "app": app.name,
                    "rollback_deployment_id": r.deployment.id,
                    "target_image": r.target_image,
                }));
                let _ = out.success(&env);
            }
            Dispatch::Ok
        }
        Err(e) => err(
            out,
            match &e {
                AppError::NotFound(_) => AppExit::Usage,
                _ => AppExit::Upstream,
            },
            &format!("rollback failed: {e}"),
        ),
    }
}

/// `--list` mode: print the most recent healthy deployments for the
/// app. `--json` is honoured via the output formatter.
async fn run_list(
    storage: &Arc<dyn StoragePort>,
    app_id: sovereign_core::domain::AppId,
    limit: u32,
    out: &Output,
) -> Dispatch {
    // `list_healthy_deployments_before` requires a `before` timestamp;
    // pass `now + 1s` to get *all* healthy deploys in DESC order.
    let now = Timestamp::now();
    let dep = match storage
        .list_healthy_deployments_before(app_id, Timestamp(now.as_secs() + 1), limit)
        .await
    {
        Ok(d) => d,
        Err(e) => return err(out, AppExit::Generic, &format!("list deployments: {e}")),
    };

    if dep.is_empty() {
        return err(
            out,
            AppExit::Usage,
            "no healthy deployments to roll back to",
        );
    }

    if out.format() == crate::output::Format::Text {
        // Text table. The columns mirror the F5 spec example.
        let _ =
            out.text("ID        IMAGE                       STARTED              AGE     CURRENT");
        for d in &dep {
            let id_short = format!("{}", d.id).chars().take(8).collect::<String>();
            let started = format_timestamp(d.started_at);
            let age = human_age(now.as_secs().saturating_sub(d.started_at.as_secs()).max(0) as u64);
            let _ = out.text(&format!(
                "{:<9} {:<27} {:<19} {:<7}",
                id_short, d.image_ref, started, age
            ));
        }
    } else {
        let env = Envelope::<serde_json::Value>::ok(serde_json::json!({
            "deployments": dep.iter().map(|d| serde_json::json!({
                "id": d.id,
                "image": d.image_ref,
                "started_at": d.started_at,
                "finished_at": d.finished_at,
            })).collect::<Vec<_>>(),
        }));
        let _ = out.success(&env);
    }
    Dispatch::Ok
}

/// Parse a hyphenated UUID string into a `DeploymentId`. Returns
/// `AppError::Validation` on bad input (caller maps to AppExit::Usage).
fn parse_deployment_id(s: &str) -> Result<sovereign_core::domain::DeploymentId, AppError> {
    use std::str::FromStr;
    sovereign_core::domain::DeploymentId::from_str(s)
        .map_err(|e| AppError::validation(format!("bad deployment id: {e}")))
}

/// Render a unix-second timestamp as `YYYY-MM-DD HH:MM:SS` UTC. V0
/// uses UTC only; the F2/F8 timeline replaces this with chrono.
fn format_timestamp(ts: Timestamp) -> String {
    let secs = ts.as_secs();
    let (year, month, day, hour, min, sec) = epoch_to_ymdhms(secs);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{min:02}:{sec:02}")
}

/// Howard Hinnant's date.h algorithm (public domain). Same as the
/// copy in `sovereign/src/output.rs`.
fn epoch_to_ymdhms(epoch_secs: i64) -> (i32, u32, u32, u32, u32, u32) {
    let days = epoch_secs / 86400;
    let secs_of_day = (epoch_secs % 86400).rem_euclid(86400) as u32;
    let hour = secs_of_day / 3600;
    let min = (secs_of_day % 3600) / 60;
    let sec = secs_of_day % 60;
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if m <= 2 { y + 1 } else { y } as i32;
    (year, m, d, hour, min, sec)
}

/// Render a duration in seconds as a human age (`2d`, `3h`, `45m`).
fn human_age(secs: u64) -> String {
    if secs >= 86_400 {
        format!("{}d", secs / 86_400)
    } else if secs >= 3_600 {
        format!("{}h", secs / 3_600)
    } else if secs >= 60 {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
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

use sovereign_core::error::AppError; // re-import for the err() match above
