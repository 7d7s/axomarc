// `sovereign logs` — stream logs from a running app.
//
// For Docker mode: streams container logs via bollard.
// For native mode: tails the systemd journal (journalctl -u sovereign-<app>).

use std::sync::Arc;

use futures::StreamExt;
use sovereign_core::ports::StoragePort;
use sovereign_runtime_docker::DockerRuntime;
use sovereign_storage_sqlite::SqliteState;
use tracing::instrument;

use crate::cli::Cmd;
use crate::commands::Dispatch;
use crate::connect;
use crate::exit::AppExit;
use crate::output::Output;

#[instrument(skip(out))]
pub async fn run(cmd: &Cmd, out: &Output) -> Dispatch {
    let (app_name, tail, follow) = match cmd {
        Cmd::Logs { app, tail, follow } => (app.clone(), *tail, *follow),
        _ => return Dispatch::Err(AppExit::Generic),
    };

    let db_path = connect::default_db_path();
    let storage = match SqliteState::open(&db_path).await {
        Ok(s) => Arc::new(s) as Arc<dyn StoragePort>,
        Err(e) => {
            let _ = out.err(&format!(
                "cannot open data dir at {}: {e}. Run `sovereign init` first?",
                db_path.display()
            ));
            return Dispatch::Err(AppExit::Upstream);
        }
    };

    let app = match storage.get_app_by_name(&app_name).await {
        Ok(Some(a)) => a,
        Ok(None) => {
            let _ = out.err(&format!("no app named `{app_name}`."));
            return Dispatch::Err(AppExit::Usage);
        }
        Err(e) => {
            let _ = out.err(&format!("storage error: {e}"));
            return Dispatch::Err(AppExit::Generic);
        }
    };

    // Get the current deployment to find the container name.
    let deployment = match storage.get_current_deployment(app.id).await {
        Ok(d) => d,
        Err(e) => {
            let _ = out.err(&format!("failed to get deployment: {e}"));
            return Dispatch::Err(AppExit::Generic);
        }
    };

    let deployment = match deployment {
        Some(d) => d,
        None => {
            let _ = out.err(&format!(
                "no deployment found for `{app_name}`. Run `sovereign deploy` first."
            ));
            return Dispatch::Err(AppExit::Usage);
        }
    };

    let container_name = format!("sovereign-{}-{}", app.id, deployment.id);

    // Connect Docker.
    let docker = match DockerRuntime::connect_from_env().await {
        Ok(r) => r,
        Err(e) => {
            let _ = out.err(&format!(
                "cannot connect to docker: {e}. Is the daemon running?"
            ));
            return Dispatch::Err(AppExit::Upstream);
        }
    };

    let mut stream = docker.logs(&container_name, &tail.to_string(), follow);

    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok((is_stderr, message)) => {
                let line = String::from_utf8_lossy(&message);
                if !line.is_empty() {
                    if is_stderr {
                        let _ = out.err(&line);
                    } else {
                        let _ = out.text(&line);
                    }
                }
            }
            Err(e) => {
                let _ = out.err(&format!("log stream error: {e}"));
                break;
            }
        }
    }

    Dispatch::Ok
}
