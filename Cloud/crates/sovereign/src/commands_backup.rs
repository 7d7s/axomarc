// F8a wiring: the `sovereign backup` subcommand.
//
// At V0 the backup target is always the SQLite state DB
// (`target: "sqlite"` in the `backup` row); the `--app` flag is kept
// for forward-compatibility with V1 (per-app Postgres backups). The
// four operations match the use case module one-for-one:
//
//   create  -> use_cases::backup::create_backup
//   list    -> use_cases::backup::list_backups
//   verify  -> use_cases::backup::verify_backup
//   restore -> use_cases::backup::restore_backup (rejects live path)

use std::path::PathBuf;
use std::sync::Arc;

use sovereign_core::domain::BackupId;
use sovereign_core::ports::StoragePort;
use sovereign_core::state::AppState;
use sovereign_storage_sqlite::SqliteState;

use crate::cli::BackupCmd;
use crate::commands::Dispatch;
use crate::connect;
use crate::exit::AppExit;
use crate::output::{Envelope, Output};

pub async fn run(cmd: &BackupCmd, out: &Output) -> Dispatch {
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
    let state = AppState {
        storage: storage.clone(),
        runtime: sovereign_core::state::no_runtime(),
        proxy: None,
        secrets: None,
        backup: connect::connect_backup(),
        db_path,
    };

    match cmd {
        BackupCmd::Create { app } => run_create(&state, app, out).await,
        BackupCmd::List => run_list(&state, out).await,
        BackupCmd::Verify { backup_id } => run_verify(&state, backup_id, out).await,
        BackupCmd::Restore { backup_id, to } => run_restore(&state, backup_id, to, out).await,
    }
}

async fn run_create(state: &AppState, app_name: &str, out: &Output) -> Dispatch {
    // V0 stores one `target = "sqlite"` row per snapshot; the
    // `--app` flag is required by the CLI surface but we don't
    // cross-link it into the row yet (V1+ will add a `target` column
    // that can be `"sqlite"` or `"postgres:<app>"`). The lookup
    // doubles as a "did the operator spell it right?" sanity check.
    if let Err(msg) = state
        .storage
        .get_app_by_name(app_name)
        .await
        .map_err(|e| format!("storage: {e}"))
        .and_then(|opt| opt.ok_or_else(|| format!("no app named `{app_name}`")))
    {
        return err(out, AppExit::Usage, &msg);
    }
    let actor = "cli".to_string();
    match sovereign_core::use_cases::backup::create_backup(state, &actor).await {
        Ok(b) => {
            if out.format() == crate::output::Format::Text {
                let _ = out.ok(&format!(
                    "{} backup {} ({} bytes) at {}",
                    b.status,
                    b.id,
                    b.size_bytes
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| "?".into()),
                    b.location.unwrap_or_else(|| "<no-location>".into()),
                ));
            } else {
                let _ = out.success(&Envelope::<serde_json::Value>::ok(serde_json::json!({
                    "id": b.id,
                    "status": b.status,
                    "size": b.size_bytes,
                    "location": b.location,
                    "started_at": b.started_at,
                    "finished_at": b.finished_at,
                })));
            }
            Dispatch::Ok
        }
        Err(e) => {
            // Failed snapshots may still have produced a row in
            // `backup` with status=Failed; surface that, not just the
            // error.
            let _ = e;
            err(
                out,
                AppExit::Upstream,
                "backup failed; check `sovereign backup list` for the row (status: failed)",
            )
        }
    }
}

async fn run_list(state: &AppState, out: &Output) -> Dispatch {
    match sovereign_core::use_cases::backup::list_backups(state, 100).await {
        Ok(rows) => {
            if out.format() == crate::output::Format::Text {
                if rows.is_empty() {
                    let _ = out.text("no backups yet");
                } else {
                    for b in &rows {
                        let _ = out.text(&format!(
                            "{}  {:>10}  {:>10}  {}  {}",
                            b.id,
                            b.status,
                            b.size_bytes
                                .map(|n| format!("{n} B"))
                                .unwrap_or_else(|| "?".into()),
                            b.started_at,
                            b.location.clone().unwrap_or_else(|| "-".into()),
                        ));
                    }
                }
            } else {
                let arr: Vec<_> = rows
                    .iter()
                    .map(|b| {
                        serde_json::json!({
                            "id": b.id,
                            "status": b.status,
                            "size": b.size_bytes,
                            "location": b.location,
                            "started_at": b.started_at,
                            "finished_at": b.finished_at,
                            "verified_at": b.verified_at,
                            "error": b.error,
                        })
                    })
                    .collect();
                let _ = out.success(&Envelope::<serde_json::Value>::ok(serde_json::json!({
                    "backups": arr,
                })));
            }
            Dispatch::Ok
        }
        Err(e) => err(out, AppExit::Upstream, &format!("list backups: {e}")),
    }
}

async fn run_verify(state: &AppState, backup_id: &str, out: &Output) -> Dispatch {
    let id = match parse_backup_id(backup_id) {
        Ok(id) => id,
        Err(msg) => return err(out, AppExit::Usage, &msg),
    };
    let actor = "cli".to_string();
    match sovereign_core::use_cases::backup::verify_backup(state, id, &actor).await {
        Ok(b) => {
            if out.format() == crate::output::Format::Text {
                let _ = out.ok(&format!("{} verified (status={})", b.id, b.status));
            } else {
                let _ = out.success(&Envelope::<serde_json::Value>::ok(serde_json::json!({
                    "id": b.id,
                    "status": b.status,
                    "verified_at": b.verified_at,
                    "verify_result": b.verify_result,
                })));
            }
            Dispatch::Ok
        }
        Err(e) => err(out, AppExit::Upstream, &format!("verify backup: {e}")),
    }
}

async fn run_restore(state: &AppState, backup_id: &str, to: &str, out: &Output) -> Dispatch {
    let id = match parse_backup_id(backup_id) {
        Ok(id) => id,
        Err(msg) => return err(out, AppExit::Usage, &msg),
    };
    let target = PathBuf::from(to);
    if let Ok(live) = state.db_path.canonicalize() {
        if let Ok(tgt) = target.canonicalize() {
            if tgt == live {
                return err(
                    out,
                    AppExit::Usage,
                    "refusing to restore onto the live state.db. Stop the service first \
                     (or pass --to a different path).",
                );
            }
        }
    }
    let actor = "cli".to_string();
    match sovereign_core::use_cases::backup::restore_backup(state, id, &target, &actor).await {
        Ok(b) => {
            if out.format() == crate::output::Format::Text {
                let _ = out.ok(&format!(
                    "restored {} ({} bytes) -> {}",
                    b.id,
                    b.size_bytes
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| "?".into()),
                    target.display(),
                ));
            } else {
                let _ = out.success(&Envelope::<serde_json::Value>::ok(serde_json::json!({
                    "id": b.id,
                    "target": target.display().to_string(),
                })));
            }
            Dispatch::Ok
        }
        Err(e) => err(out, AppExit::Upstream, &format!("restore backup: {e}")),
    }
}

fn parse_backup_id(s: &str) -> Result<BackupId, String> {
    let u = uuid::Uuid::parse_str(s)
        .map_err(|e| format!("backup_id must be a UUID, got `{s}`: {e}"))?;
    Ok(BackupId(u))
}

fn err(out: &Output, code: AppExit, msg: &str) -> Dispatch {
    let _ = out.err(msg);
    Dispatch::Err(code)
}
