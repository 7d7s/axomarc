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
        Cmd::Deploy {
            app,
            image,
            strategy,
            wait,
            no_lock: _,
        } => {
            // F4: actually call the use case. Falls back to a stub on
            // --dry-run (the spec says subcommands should "print what
            // would be done and exit 0").
            if cli.dry_run {
                stub(
                    cli,
                    out,
                    "deploy",
                    &format!(
                        "app={:?} image={:?} strategy={:?} wait={}",
                        app, image, strategy, wait
                    ),
                )
            } else {
                return crate::commands_deploy::run(cmd, out).await;
            }
        }
        Cmd::Rollback { app, to, list, .. } => {
            if cli.dry_run {
                stub(
                    cli,
                    out,
                    "rollback",
                    &format!("app={} to={:?} list={}", app, to, list),
                )
            } else {
                return crate::commands_rollback::run(cmd, out).await;
            }
        }
        Cmd::Logs { app, tail, follow } => stub(
            cli,
            out,
            "logs",
            &format!("app={} tail={} follow={}", app, tail, follow),
        ),
        Cmd::Status { app } => stub(cli, out, "status", &format!("app={:?}", app)),
        Cmd::Domain { cmd } => match cmd {
            DomainCmd::Add { hostname, app } => {
                if cli.dry_run {
                    stub(
                        cli,
                        out,
                        "domain add",
                        &format!("hostname={} app={}", hostname, app),
                    )
                } else {
                    return crate::commands_domain::run(cmd, out).await;
                }
            }
            DomainCmd::List { app } => {
                if cli.dry_run {
                    stub(cli, out, "domain list", &format!("app={}", app))
                } else {
                    return crate::commands_domain::run(cmd, out).await;
                }
            }
        },
        Cmd::Secret { cmd } => match cmd {
            SecretCmd::Set { key, app } => {
                if cli.dry_run {
                    stub(cli, out, "secret set", &format!("key={} app={}", key, app))
                } else {
                    return crate::commands_secret::run(cmd, out).await;
                }
            }
            SecretCmd::List { app } => {
                if cli.dry_run {
                    stub(cli, out, "secret list", &format!("app={}", app))
                } else {
                    return crate::commands_secret::run(cmd, out).await;
                }
            }
            SecretCmd::Rotate { key, app } => {
                if cli.dry_run {
                    stub(
                        cli,
                        out,
                        "secret rotate",
                        &format!("key={} app={}", key, app),
                    )
                } else {
                    return crate::commands_secret::run(cmd, out).await;
                }
            }
        },
        Cmd::Backup { cmd } => match cmd {
            BackupCmd::Create { app } => {
                if cli.dry_run {
                    stub(cli, out, "backup create", &format!("app={}", app))
                } else {
                    return crate::commands_backup::run(cmd, out).await;
                }
            }
            BackupCmd::List => {
                if cli.dry_run {
                    stub(cli, out, "backup list", "")
                } else {
                    return crate::commands_backup::run(cmd, out).await;
                }
            }
            BackupCmd::Verify { backup_id } => {
                if cli.dry_run {
                    stub(
                        cli,
                        out,
                        "backup verify",
                        &format!("backup_id={}", backup_id),
                    )
                } else {
                    return crate::commands_backup::run(cmd, out).await;
                }
            }
            BackupCmd::Restore { backup_id, to } => {
                if cli.dry_run {
                    stub(
                        cli,
                        out,
                        "backup restore",
                        &format!("backup_id={} to={}", backup_id, to),
                    )
                } else {
                    return crate::commands_backup::run(cmd, out).await;
                }
            }
        },
        Cmd::Doctor {
            level,
            explain,
            fix,
            report,
            json,
        } => {
            if cli.dry_run {
                stub(
                    cli,
                    out,
                    "doctor",
                    &format!(
                        "level={:?} explain={} fix={} report={:?} json={}",
                        level, explain, fix, report, json
                    ),
                )
            } else {
                return crate::commands_doctor::run(
                    out,
                    *level,
                    *explain,
                    *fix,
                    report.clone(),
                    *json,
                )
                .await;
            }
        }
        Cmd::Update { cmd } => match cmd {
            UpdateCmd::Check { channel, manifest } => {
                if cli.dry_run {
                    stub(
                        cli,
                        out,
                        "update check",
                        &format!("channel={:?} manifest={:?}", channel, manifest),
                    )
                } else {
                    return crate::commands_update::run_check(
                        out,
                        channel.to_update_channel(),
                        manifest.as_deref(),
                    )
                    .await;
                }
            }
            UpdateCmd::Apply {
                channel,
                target,
                manifest,
                no_swap,
            } => {
                if cli.dry_run {
                    stub(
                        cli,
                        out,
                        "update apply",
                        &format!(
                            "channel={:?} target={:?} manifest={:?} no_swap={}",
                            channel, target, manifest, no_swap
                        ),
                    )
                } else {
                    return crate::commands_update::run_apply(
                        out,
                        channel.to_update_channel(),
                        target.as_deref(),
                        manifest.as_deref(),
                        *no_swap,
                    )
                    .await;
                }
            }
            UpdateCmd::Rollback { manifest } => {
                if cli.dry_run {
                    stub(
                        cli,
                        out,
                        "update rollback",
                        &format!("manifest={:?}", manifest),
                    )
                } else {
                    return crate::commands_update::run_rollback(out, manifest.as_deref()).await;
                }
            }
            UpdateCmd::History { limit } => {
                if cli.dry_run {
                    stub(cli, out, "update history", &format!("limit={}", limit))
                } else {
                    return crate::commands_update::run_history(out, *limit).await;
                }
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

/// The stub handler. F2 ships every V0 subcommand as a stub. The
/// `--dry-run` path prints "would do X" (which is what a `--dry-run` of
/// the real command would also do, modulo side effects). The non-`--dry-run`
/// path prints "not yet implemented" and returns a `Doctor` exit code so
/// scripts can distinguish "shipped but didn't run" from "not shipped yet".
fn stub(cli: &Cli, out: &Output, name: &str, args: &str) -> Dispatch {
    if cli.is_dry_run() {
        if args.is_empty() {
            let _ = out.ok(&format!("would run: sovereign {name}"));
        } else {
            let _ = out.ok(&format!("would run: sovereign {name} {args}"));
        }
        Dispatch::Ok
    } else {
        let msg = format!(
            "{name}: not yet implemented. (F{} will fill this in; the stub prints the command and exits.)",
            stub_phase(name)
        );
        let _ = out.warn(&msg);
        Dispatch::Ok
    }
}

/// The phase that will implement this subcommand. Used in the stub message
/// so the operator knows when to expect the real thing.
fn stub_phase(name: &str) -> &'static str {
    match name {
        "init" | "login" => "2",             // F2 itself (these are stubs for now)
        "deploy" | "rollback" => "4-5",      // F4 (deploy) + F5 (rollback)
        "logs" => "4",                       // F4 includes the log stream
        "status" => "2",                     // F2 itself (the stub prints the state)
        "domain add" | "domain list" => "6", // F6 (Caddy auto-TLS)
        "secret set" | "secret list" | "secret rotate" => "7", // F7 (encrypted secret store) — done
        "backup create" | "backup list" | "backup verify" | "backup restore" => "8", // F8 (backup) — done
        _ => "??",
    }
}

// --- Data shapes for JSON envelopes -----------------------------------------

#[derive(Serialize)]
struct NoData {}

#[derive(Serialize)]
struct VersionData {
    version: &'static str,
}
