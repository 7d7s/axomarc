// V0.6: System service commands. Implements `sovereign service install|remove|status|restart|validate|list`.
//
// Per `docs/phase-0.6.md` §S1, these commands manage the 7 system services
// (nginx, mysql, mariadb, redis, vsftpd, letsencrypt, phpmyadmin) via apt + systemd.

use std::str::FromStr;

use sovereign_core::ports::SystemServicePort;

use crate::cli::ServiceCmd;
use crate::commands::Dispatch;
use crate::exit::AppExit;
use crate::output::{Envelope, Output};

pub async fn run(cmd: &ServiceCmd, out: &Output) -> Dispatch {
    match cmd {
        ServiceCmd::Install {
            kind,
            version,
            no_start,
        } => {
            let kind = match ServiceKind::from_str(kind) {
                Ok(k) => k,
                Err(e) => return err(out, AppExit::Usage, &e.to_string()),
            };
            let svc = sovereign_systemd::adapters::CompositeService::new();
            let spec = sovereign_core::domain::service::ServiceInstallSpec {
                kind,
                version: version.clone(),
                no_start: *no_start,
            };
            match svc.install(&spec).await {
                Ok(result) => {
                    let msg = format!("installed {} {}", result.kind, result.version);
                    ok(out, &msg);
                    Dispatch::Ok
                }
                Err(e) => err(out, AppExit::Upstream, &e.to_string()),
            }
        }
        ServiceCmd::Remove { kind, purge } => {
            let kind = match ServiceKind::from_str(kind) {
                Ok(k) => k,
                Err(e) => return err(out, AppExit::Usage, &e.to_string()),
            };
            let svc = sovereign_systemd::adapters::CompositeService::new();
            let spec = sovereign_core::domain::service::ServiceRemoveSpec {
                kind,
                purge_config: *purge,
            };
            match svc.remove(&spec).await {
                Ok(()) => {
                    let msg = format!("removed {kind}");
                    ok(out, &msg);
                    Dispatch::Ok
                }
                Err(e) => err(out, AppExit::Upstream, &e.to_string()),
            }
        }
        ServiceCmd::Status { kind } => {
            let svc = sovereign_systemd::adapters::CompositeService::new();
            if kind == "all" {
                match svc.list_all().await {
                    Ok(statuses) => {
                        for s in &statuses {
                            let msg = format!(
                                "{}: installed={} active={}",
                                s.kind, s.installed, s.active
                            );
                            ok(out, &msg);
                        }
                        Dispatch::Ok
                    }
                    Err(e) => err(out, AppExit::Upstream, &e.to_string()),
                }
            } else {
                let kind = match ServiceKind::from_str(kind) {
                    Ok(k) => k,
                    Err(e) => return err(out, AppExit::Usage, &e.to_string()),
                };
                match svc.status(kind).await {
                    Ok(status) => {
                        let msg = format!("{status:?}");
                        ok(out, &msg);
                        Dispatch::Ok
                    }
                    Err(e) => err(out, AppExit::Upstream, &e.to_string()),
                }
            }
        }
        ServiceCmd::Restart { kind } => {
            let kind = match ServiceKind::from_str(kind) {
                Ok(k) => k,
                Err(e) => return err(out, AppExit::Usage, &e.to_string()),
            };
            let svc = sovereign_systemd::adapters::CompositeService::new();
            match svc.restart(kind).await {
                Ok(()) => {
                    let msg = format!("restarted {kind}");
                    ok(out, &msg);
                    Dispatch::Ok
                }
                Err(e) => err(out, AppExit::Upstream, &e.to_string()),
            }
        }
        ServiceCmd::Validate { kind } => {
            let kind = match ServiceKind::from_str(kind) {
                Ok(k) => k,
                Err(e) => return err(out, AppExit::Usage, &e.to_string()),
            };
            let svc = sovereign_systemd::adapters::CompositeService::new();
            match svc.validate_config(kind).await {
                Ok(()) => {
                    let msg = format!("config ok: {kind}");
                    ok(out, &msg);
                    Dispatch::Ok
                }
                Err(e) => err(out, AppExit::Upstream, &e.to_string()),
            }
        }
        ServiceCmd::List => {
            let svc = sovereign_systemd::adapters::CompositeService::new();
            match svc.list_all().await {
                Ok(statuses) => {
                    for s in &statuses {
                        let msg =
                            format!("{}: installed={} active={}", s.kind, s.installed, s.active);
                        ok(out, &msg);
                    }
                    Dispatch::Ok
                }
                Err(e) => err(out, AppExit::Upstream, &e.to_string()),
            }
        }
    }
}

use sovereign_core::domain::service::ServiceKind;

fn ok(out: &Output, msg: &str) {
    let _ = out.ok(msg);
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
