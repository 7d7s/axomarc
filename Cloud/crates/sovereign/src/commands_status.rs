// `sovereign status` — fleet status overview.
//
// Lists all apps with their current deployment status, deploy mode,
// and last deploy info. Pure storage read — no runtime needed.

use std::sync::Arc;

use anyhow::Context;
use sovereign_core::ports::StoragePort;
use sovereign_storage_sqlite::SqliteState;
use tracing::instrument;

use crate::cli::Cmd;
use crate::commands::Dispatch;
use crate::connect;
use crate::exit::AppExit;
use crate::output::{Envelope, Output};

#[instrument(skip(out))]
pub async fn run(cmd: &Cmd, out: &Output) -> Dispatch {
    let filter_app = match cmd {
        Cmd::Status { app } => app.clone(),
        _ => return Dispatch::Err(AppExit::Generic),
    };

    let db_path = connect::default_db_path();
    let storage = match SqliteState::open(&db_path).await {
        Ok(s) => Arc::new(s) as Arc<dyn StoragePort>,
        Err(e) => {
            if out.format() == crate::output::Format::Text {
                let _ = out.err(&format!(
                    "cannot open data dir at {}: {e}. Run `sovereign init` first?",
                    db_path.display()
                ));
            }
            return Dispatch::Err(AppExit::Upstream);
        }
    };

    let apps = match storage.list_apps(None).await {
        Ok(a) => a,
        Err(e) => {
            if out.format() == crate::output::Format::Text {
                let _ = out.err(&format!("failed to list apps: {e}"));
            }
            return Dispatch::Err(AppExit::Generic);
        }
    };

    // Filter to a single app if requested.
    let apps: Vec<_> = if let Some(ref name) = filter_app {
        apps.into_iter().filter(|a| &a.name == name).collect()
    } else {
        apps.into_iter().collect()
    };

    if apps.is_empty() {
        if out.format() == crate::output::Format::Text {
            let _ = out.ok("No apps found. Run `sovereign init` to create one.");
        } else {
            let env = Envelope::ok(serde_json::json!({ "apps": [] }));
            let _ = out.success(&env);
        }
        return Dispatch::Ok;
    }

    // Gather deployment info for each app.
    let mut rows: Vec<StatusRow> = Vec::new();
    for app in &apps {
        let deployment = storage
            .get_current_deployment(app.id)
            .await
            .context("get current deployment")
            .ok()
            .flatten();

        rows.push(StatusRow {
            name: app.name.clone(),
            env: app.env.to_string(),
            deploy_mode: app.deploy_mode.to_string(),
            status: app.status.to_string(),
            image: app.image_ref.clone().unwrap_or_default(),
            deployment_status: deployment.as_ref().map(|d| d.status.to_string()).unwrap_or_default(),
            deployment_id: deployment.as_ref().map(|d| format!("{}", d.id)).unwrap_or_default(),
        });
    }

    match out.format() {
        crate::output::Format::Text => {
            let _ = out.text(&format!(
                "{:<20} {:<10} {:<10} {:<10} {:<10} {:<30}",
                "APP", "ENV", "MODE", "STATUS", "DEPLOY", "IMAGE"
            ));
            let _ = out.text(&"-".repeat(90));
            for r in &rows {
                let _ = out.text(&format!(
                    "{:<20} {:<10} {:<10} {:<10} {:<10} {:<30}",
                    r.name, r.env, r.deploy_mode, r.status, r.deployment_status, r.image,
                ));
            }
        }
        _ => {
            let env = Envelope::ok(serde_json::json!({
                "apps": rows.iter().map(|r| serde_json::json!({
                    "name": r.name,
                    "env": r.env,
                    "deploy_mode": r.deploy_mode,
                    "status": r.status,
                    "deployment_status": r.deployment_status,
                    "deployment_id": r.deployment_id,
                    "image": r.image,
                })).collect::<Vec<_>>(),
            }));
            let _ = out.success(&env);
        }
    }

    Dispatch::Ok
}

#[derive(serde::Serialize)]
struct StatusRow {
    name: String,
    env: String,
    deploy_mode: String,
    status: String,
    image: String,
    deployment_status: String,
    deployment_id: String,
}
