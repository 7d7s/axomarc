// Sovereign Application Runtime — F1 + F2 binary entry point.
// See docs/phase-00-mvp.md §F1 and §F2, and docs/tech-stack.md §3.

mod cli;
mod commands;
mod commands_backup;
mod commands_deploy;
mod commands_doctor;
mod commands_domain;
mod commands_rollback;
mod commands_secret;
mod exit;
mod lock;
mod output;

use anyhow::Result;
use clap::Parser;
use std::process::ExitCode;

use crate::cli::Cli;
use crate::exit::AppExit;
use crate::output::Output;

#[cfg(all(target_env = "musl", feature = "mimalloc-musl"))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(all(not(target_env = "musl"), feature = "jemalloc-gnu"))]
#[global_allocator]
static GLOBAL: jemallocator::Jemalloc = jemallocator::Jemalloc;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    match run().await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            std::process::ExitCode::from(1)
        }
    }
}

async fn run() -> Result<ExitCode> {
    // Initialize logging. The env-filter is honored (RUST_LOG=info,sovereign=debug).
    // F8 will replace the fmt layer with structured JSON; F2 only needs the
    // binary to boot cleanly so the CLI tests pass.
    sovereign_observability::init();

    // Parse the CLI. clap's `parse()` handles --help, --version, usage errors,
    // and the global flags. The output formatter is built from --format and
    // --no-color (with TTY + NO_COLOR auto-detection).
    let cli = Cli::parse();
    let out = Output::new(cli.effective_format(), cli.no_color);

    tracing::debug!(?cli, "parsed CLI");

    // Dispatch the subcommand. The Dispatch result carries the exit code;
    // the handler has already printed the success/error envelope.
    Ok(match commands::dispatch(&cli, &out).await {
        commands::Dispatch::Ok => AppExit::Success.into_process(),
        commands::Dispatch::Err(code) => code.into_process(),
    })
}
