// P0 wiring: the `sovereign daemon` subcommand.
//
// Sub-modes:
//   - `sovereign daemon start`         — start the HTTP server + poller
//   - `sovereign daemon install`       — write + enable the systemd unit
//   - `sovereign daemon status`        — check if the unit is running
//   - `sovereign daemon logs`          — tail recent journalctl output

use std::net::SocketAddr;
use std::process::Command;
use std::sync::Arc;

use sovereign_core::ports::StoragePort;
use sovereign_core::state::{no_runtime, AppState};
use sovereign_storage_sqlite::SqliteState;

use crate::cli::{Cmd, DaemonCmd};
use crate::commands::Dispatch;
use crate::connect;
use crate::daemon;
use crate::exit::AppExit;
use crate::output::{Envelope, Output};

/// Dispatch the daemon subcommand.
pub async fn run(cmd: &Cmd, out: &Output) -> Dispatch {
    let sub = match cmd {
        Cmd::Daemon { cmd } => cmd,
        _ => return Dispatch::Err(AppExit::Generic),
    };

    match sub {
        DaemonCmd::Start { listen } => run_start(listen, out).await,
        DaemonCmd::Install { unit_name } => run_install(unit_name, out),
        DaemonCmd::Status => run_status(out),
        DaemonCmd::Logs { lines } => run_logs(*lines, out),
    }
}

/// Build an `AppState` for the daemon. Connects storage, secrets,
/// proxy, and backup. Returns `None` if storage cannot be opened.
async fn build_app_state(out: &Output) -> Option<AppState> {
    let db_path = connect::default_db_path();
    let storage = match SqliteState::open(&db_path).await {
        Ok(s) => Arc::new(s) as Arc<dyn StoragePort>,
        Err(e) => {
            let _ = out.err(&format!(
                "cannot open data dir at {}: {e}. Run `sovereign init` first?",
                db_path.display()
            ));
            return None;
        }
    };

    let secrets = connect::connect_secrets();
    let proxy = connect::connect_proxy().await;
    let backup = connect::connect_backup();

    Some(AppState {
        storage,
        runtime: no_runtime(),
        proxy,
        secrets,
        backup,
        db_path,
    })
}

/// Build a `dyn StoragePort` for CLI commands that need direct DB access.
/// Returns `None` if storage cannot be opened.
pub async fn build_store() -> Option<Arc<dyn StoragePort>> {
    let db_path = connect::default_db_path();
    match SqliteState::open(&db_path).await {
        Ok(s) => Some(Arc::new(s) as Arc<dyn StoragePort>),
        Err(e) => {
            eprintln!("cannot open storage at {}: {e}", db_path.display());
            None
        }
    }
}

/// Start the daemon HTTP server. Blocks until SIGTERM/SIGINT.
async fn run_start(listen: &Option<String>, out: &Output) -> Dispatch {
    let addr: SocketAddr = match listen {
        Some(s) => match s.parse() {
            Ok(a) => a,
            Err(e) => {
                let _ = out.err(&format!("invalid listen address `{s}`: {e}"));
                return Dispatch::Err(AppExit::Usage);
            }
        },
        None => "127.0.0.1:8443".parse().unwrap(),
    };

    let app_state = match build_app_state(out).await {
        Some(s) => s,
        None => return Dispatch::Err(AppExit::Upstream),
    };

    if out.format() == crate::output::Format::Text {
        let _ = out.text(&format!(
            "starting sovereign daemon on {} (v{})...",
            addr,
            env!("CARGO_PKG_VERSION")
        ));
    } else {
        let env = Envelope::<serde_json::Value>::ok(serde_json::json!({
            "status": "starting",
            "listen": addr.to_string(),
            "version": env!("CARGO_PKG_VERSION"),
        }));
        let _ = out.success(&env);
    }

    if let Err(e) = daemon::run(addr, app_state).await {
        let _ = out.err(&format!("daemon exited with error: {e}"));
        return Dispatch::Err(AppExit::Generic);
    }
    Dispatch::Ok
}

/// Write the systemd unit file and enable + start the service.
fn run_install(unit_name: &str, out: &Output) -> Dispatch {
    let unit_path = format!("/etc/systemd/system/{unit_name}.service");
    let bin_path = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "/usr/bin/sovereign".to_string());

    let unit_content = daemon::systemd_unit(unit_name, &bin_path);

    if let Err(e) = std::fs::write(&unit_path, &unit_content) {
        let _ = out.err(&format!("failed to write {unit_path}: {e} (run as root?)"));
        return Dispatch::Err(AppExit::Upstream);
    }

    // systemctl daemon-reload
    let status = Command::new("systemctl")
        .args(["daemon-reload"])
        .status();
    match status {
        Ok(s) if s.success() => {}
        Ok(s) => {
            let _ = out.err(&format!(
                "systemctl daemon-reload failed (exit {})",
                s.code().unwrap_or(-1)
            ));
            return Dispatch::Err(AppExit::Upstream);
        }
        Err(e) => {
            let _ = out.err(&format!("failed to run systemctl: {e}"));
            return Dispatch::Err(AppExit::Upstream);
        }
    }

    // systemctl enable --now
    let status = Command::new("systemctl")
        .args(["enable", "--now", unit_name])
        .status();
    match status {
        Ok(s) if s.success() => {}
        Ok(s) => {
            let _ = out.err(&format!(
                "systemctl enable --now failed (exit {})",
                s.code().unwrap_or(-1)
            ));
            return Dispatch::Err(AppExit::Upstream);
        }
        Err(e) => {
            let _ = out.err(&format!("failed to run systemctl: {e}"));
            return Dispatch::Err(AppExit::Upstream);
        }
    }

    if out.format() == crate::output::Format::Text {
        let _ = out.ok(&format!(
            "wrote {unit_path}, enabled and started {unit_name}"
        ));
    } else {
        let env = Envelope::<serde_json::Value>::ok(serde_json::json!({
            "unit_path": unit_path,
            "unit_name": unit_name,
            "status": "enabled",
        }));
        let _ = out.success(&env);
    }
    Dispatch::Ok
}

/// Check if the systemd unit is active.
fn run_status(out: &Output) -> Dispatch {
    let output = Command::new("systemctl")
        .args(["is-active", "sovereign-daemon"])
        .output();

    match output {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout).trim().to_string();
            let is_active = o.status.success();

            if out.format() == crate::output::Format::Text {
                let _ = out.text(&format!("sovereign-daemon: {stdout}"));
            } else {
                let env = Envelope::<serde_json::Value>::ok(serde_json::json!({
                    "unit": "sovereign-daemon",
                    "active": is_active,
                    "status": stdout,
                }));
                let _ = out.success(&env);
            }
            Dispatch::Ok
        }
        Err(e) => {
            let _ = out.err(&format!("failed to run systemctl: {e}"));
            Dispatch::Err(AppExit::Upstream)
        }
    }
}

/// Show recent daemon logs via journalctl.
fn run_logs(lines: u32, out: &Output) -> Dispatch {
    let output = Command::new("journalctl")
        .args([
            "-u",
            "sovereign-daemon",
            "-n",
            &lines.to_string(),
            "--no-pager",
        ])
        .output();

    match output {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            if out.format() == crate::output::Format::Text {
                let _ = out.text(&stdout);
            } else {
                let env = Envelope::<serde_json::Value>::ok(serde_json::json!({
                    "unit": "sovereign-daemon",
                    "lines": lines,
                    "log": stdout.trim(),
                }));
                let _ = out.success(&env);
            }
            Dispatch::Ok
        }
        Err(e) => {
            let _ = out.err(&format!("failed to run journalctl: {e}"));
            Dispatch::Err(AppExit::Upstream)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_listen_is_loopback() {
        let addr: SocketAddr = "127.0.0.1:8443".parse().unwrap();
        assert_eq!(addr.ip(), std::net::Ipv4Addr::LOCALHOST);
        assert_eq!(addr.port(), 8443);
    }

    #[test]
    fn parse_listen_override() {
        let addr: SocketAddr = "0.0.0.0:9090".parse().unwrap();
        assert_eq!(addr.port(), 9090);
    }
}
