// F2: Subcommand handlers.
// Every subcommand in `cli::Cmd` has a `dispatch` function here. F2 wires the
// 10 V0 subcommands as stubs (they print "not yet implemented" or the
// `--dry-run` "would do X" line and return). F3+ replaces each stub with
// the real implementation.

use crate::cli::{BackupCmd, Cli, Cmd, DomainCmd, SecretCmd, UpdateCmd};
use crate::exit::AppExit;
use crate::output::{Envelope, Output};
use serde::Serialize;

use crate::commands_init;
use crate::commands_login;

/// The result of dispatching a subcommand. The exit code is set by the
/// top-level `main`, not by the handler.
pub enum Dispatch {
    /// The command succeeded; the `i32` is the process exit code (0).
    Ok,
    /// The command printed an error envelope; the `AppExit` is the
    /// process exit code to use.
    Err(AppExit),
}

/// Dispatch a parsed CLI to the right handler. Async because F4's
/// `deploy` use case hits the storage + Docker runtime, which are
/// both async. F2's stubs are still synchronous; the `.await` on
/// the future is a no-op for them.
#[allow(unused_variables)] // the destructured fields below are not used here; the real handler re-destructures from cmd.
pub async fn dispatch(cli: &Cli, out: &Output) -> Dispatch {
    let Some(cmd) = &cli.cmd else {
        // No subcommand: print the friendly first-run message.
        if matches!(out.format(), crate::output::Format::Text) {
            let _ = out.text(&format!(
                "sovereign {} — run `sovereign --help` to get started",
                env!("CARGO_PKG_VERSION")
            ));
        } else {
            let env = Envelope::<NoData>::ok(NoData {});
            let _ = out.success(&env);
        }
        return Dispatch::Ok;
    };

    match cmd {
        Cmd::Version => {
            if matches!(out.format(), crate::output::Format::Text) {
                let _ = out.text(&format!("sovereign {}", env!("CARGO_PKG_VERSION")));
            } else {
                let env = Envelope::<VersionData>::ok(VersionData {
                    version: env!("CARGO_PKG_VERSION"),
                });
                let _ = out.success(&env);
            }
            Dispatch::Ok
        }
        Cmd::Init {
            framework,
            force,
            output,
            name,
            print_frameworks,
        } => {
            let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            commands_init::run(
                out,
                &cwd,
                *framework,
                *force,
                output,
                name.clone(),
                *print_frameworks,
            )
            .await
        }
        Cmd::Login {
            no_input,
            master_key,
            migrate,
        } => commands_login::run(out, *no_input, master_key.as_deref(), *migrate).await,
        Cmd::Deploy { .. } => {
            // F4: actually call the use case. Falls back to a stub on
            // --dry-run (the spec says subcommands should "print what
            // would be done and exit 0").
            return crate::commands_deploy::run(cmd, out).await; // --dry-run: forward to the real handler
        }
        Cmd::Rollback { .. } => {
            return crate::commands_rollback::run(cmd, out).await; // --dry-run: forward to the real handler
        }
        Cmd::Logs { .. } => {
            return crate::commands_logs::run(cmd, out).await; // --dry-run: forward to the real handler
        }
        Cmd::Status { .. } => {
            return crate::commands_status::run(cmd, out).await; // --dry-run: forward to the real handler
        }
        Cmd::Domain { cmd } => match cmd {
            DomainCmd::Add { hostname, app } => {
                return crate::commands_domain::run(cmd, out).await; // --dry-run: forward to the real handler
            }
            DomainCmd::List { app } => {
                return crate::commands_domain::run(cmd, out).await; // --dry-run: forward to the real handler
            }
        },
        Cmd::Secret { cmd } => match cmd {
            SecretCmd::Set { key, app } => {
                return crate::commands_secret::run(cmd, out).await; // --dry-run: forward to the real handler
            }
            SecretCmd::List { .. } => {
                return crate::commands_secret::run(cmd, out).await; // --dry-run: forward to the real handler
            }
            SecretCmd::Rotate { key, app } => {
                return crate::commands_secret::run(cmd, out).await; // --dry-run: forward to the real handler
            }
        },
        Cmd::Backup { cmd } => match cmd {
            BackupCmd::Create { app } => {
                return crate::commands_backup::run(cmd, out).await; // --dry-run: forward to the real handler
            }
            BackupCmd::List => {
                return crate::commands_backup::run(cmd, out).await; // --dry-run: forward to the real handler
            }
            BackupCmd::Verify { .. } => {
                return crate::commands_backup::run(cmd, out).await; // --dry-run: forward to the real handler
            }
            BackupCmd::Restore { .. } => {
                return crate::commands_backup::run(cmd, out).await; // --dry-run: forward to the real handler
            }
        },
        Cmd::Doctor {
            level,
            explain,
            fix,
            report,
            json,
        } => {
            return crate::commands_doctor::run(
                    out,
                    *level,
                    *explain,
                    *fix,
                    report.clone(),
                    *json,
                )
                .await; // --dry-run: forward to the real handler
        }
        Cmd::Update { cmd } => match cmd {
            UpdateCmd::Check { channel, manifest } => {
                return crate::commands_update::run_check(
                        out,
                        channel.to_update_channel(),
                        manifest.as_deref(),
                    )
                    .await; // --dry-run: forward to the real handler
            }
            UpdateCmd::Apply {
                channel,
                target,
                manifest,
                no_swap,
            } => {
                return crate::commands_update::run_apply(
                        out,
                        channel.to_update_channel(),
                        target.as_deref(),
                        manifest.as_deref(),
                        *no_swap,
                    )
                    .await; // --dry-run: forward to the real handler
            }
            UpdateCmd::Rollback { manifest } => {
                return crate::commands_update::run_rollback(out, manifest.as_deref()).await; // --dry-run: forward to the real handler
            }
            UpdateCmd::History { limit } => {
                return crate::commands_update::run_history(out, *limit).await; // --dry-run: forward to the real handler
            }
        },
        Cmd::Completions { shell } => {
            use clap::CommandFactory;
            let mut cmd = Cli::command();
            let bin = cmd.get_name().to_string();
            // Nushell is not yet in clap_complete's Shell enum; we emit a
            // shell script compatible with Nushell's `source` command
            // (same as Bash) until the upstream crate adds it.
            let shell = match shell {
                crate::cli::Shell::Bash => clap_complete::Shell::Bash,
                crate::cli::Shell::Zsh => clap_complete::Shell::Zsh,
                crate::cli::Shell::Fish => clap_complete::Shell::Fish,
                crate::cli::Shell::Nushell => clap_complete::Shell::Bash,
                crate::cli::Shell::Powershell => clap_complete::Shell::PowerShell,
            };
            clap_complete::generate(shell, &mut cmd, bin, &mut std::io::stdout());
            Dispatch::Ok
        }
        Cmd::Validate { path } => {
            // Forward to the dedicated `commands_validate` module.
            // The path is moved into validate_path which is owned
            // by `app_yaml`; we just pass the &Path here.
            crate::commands_validate::run(out, path).await
        }
        Cmd::Service { cmd } => {
            return crate::commands_service::run(cmd, out).await;
        }
        Cmd::Daemon { .. } => {
            return crate::commands_daemon::run(cmd, out).await;
        }
        Cmd::Chatops { cmd } => {
            return crate::commands_chatops::run(cmd, out).await;
        }
        Cmd::User { cmd } => {
            let store = crate::commands_daemon::build_store().await;
            let store = match store {
                Some(s) => s,
                None => return Dispatch::Err(AppExit::Generic),
            };
            let token = cli.token.as_deref();
            if let Err(e) = crate::commands_user::dispatch(cmd.clone(), store.as_ref(), token, out).await {
                let _ = out.err(&e.to_string());
                return Dispatch::Err(AppExit::Generic);
            }
            Dispatch::Ok
        }
        Cmd::Token { cmd } => {
            let store = crate::commands_daemon::build_store().await;
            let store = match store {
                Some(s) => s,
                None => return Dispatch::Err(AppExit::Generic),
            };
            let token = cli.token.as_deref();
            if let Err(e) = crate::commands_token::dispatch(cmd.clone(), store.as_ref(), token, out).await {
                let _ = out.err(&e.to_string());
                return Dispatch::Err(AppExit::Generic);
            }
            Dispatch::Ok
        }
        Cmd::Man { dir } => {
            use clap::CommandFactory;
            use clap_mangen::Man;
            let cmd = Cli::command();
            let path = std::path::Path::new(dir).join(format!("{}.1", cmd.get_name()));
            let mut file = match std::fs::File::create(&path) {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("sovereign: cannot create {}: {}", path.display(), e);
                    return Dispatch::Err(AppExit::Usage);
                }
            };
            let man = Man::new(cmd);
            if let Err(e) = man.render(&mut file) {
                eprintln!("sovereign: cannot render man page: {e}");
                return Dispatch::Err(AppExit::Generic);
            }
            let _ = out.ok(&format!("wrote {}", path.display()));
            Dispatch::Ok
        }
    }
}

// The V0 stub helper is removed; the --dry-run path forwards
// directly to the real handler. The real handler decides what dry-run
// means for its command.


// --- Data shapes for JSON envelopes -----------------------------------------

#[derive(Serialize)]
struct NoData {}

#[derive(Serialize)]
struct VersionData {
    version: &'static str,
}
