use std::path::PathBuf;
use std::time::Instant;

use sovereign_doctor::{CheckContext, CheckStatus, Doctor, DoctorLevel, DoctorReport};
use tracing::info;

use crate::cli::DoctorLevelArg as CliLevel;
use crate::commands::Dispatch;
use crate::exit::AppExit;
use crate::output::{Color, Output};

/// Wire the `CheckContext` for the current invocation. The
/// `runtime` field is left as `None` in V0; runtime-port-aware
/// checks are V1+.
fn build_context(data_dir: PathBuf, binary_path: PathBuf) -> CheckContext {
    CheckContext {
        data_dir,
        binary_path,
        binary_version: env!("CARGO_PKG_VERSION"),
        db_filename: "sovereign.db",
        master_key_filename: "master.key",
        docker_socket: PathBuf::from("/var/run/docker.sock"),
        caddy_admin: std::env::var("CADDY_ADMIN")
            .unwrap_or_else(|_| "http://127.0.0.1:2019".into()),
        caddy_binary: PathBuf::from(
            std::env::var("CADDY_BINARY").unwrap_or_else(|_| "caddy".into()),
        ),
        caddy_config: PathBuf::from(
            std::env::var("CADDY_CONFIG").unwrap_or_else(|_| "/etc/caddy/Caddyfile".into()),
        ),
        runtime: None,
    }
}

/// The on-call's first command. Runs the basic-level checks
/// synchronously, prints a human-readable summary, and exits with
/// `0` for pass, `1` (Usage) for warn, `2` (Usage) for fail.
pub async fn run(
    out: &Output,
    level: CliLevel,
    explain: bool,
    fix: bool,
    report: Option<PathBuf>,
    json: bool,
) -> Dispatch {
    let level: DoctorLevel = level.to_doctor_level();
    if !level.is_shipped_in_v0() {
        out.err(&format!(
            "doctor level '{}' is V1+; upgrade with `sovereign update`",
            level.label()
        ))
        .ok();
        return Dispatch::Err(AppExit::Usage);
    }
    let doctor = Doctor::for_level(level);
    if doctor.checks().is_empty() {
        out.err("doctor level requested has no checks registered")
            .ok();
        return Dispatch::Err(AppExit::Usage);
    }
    let data_dir = crate::commands_deploy::default_db_path()
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("/var/lib/sovereign"));
    let binary_path = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("sovereign"));
    let ctx = build_context(data_dir, binary_path);

    info!(level = level.label(), "running sovereign doctor");
    let started = Instant::now();
    let doctor_report = doctor.run(&ctx).await;
    let elapsed_ms = started.elapsed().as_millis();

    if json {
        match serde_json::to_string_pretty(&doctor_report) {
            Ok(payload) => println!("{}", payload),
            Err(err) => {
                out.err(&format!("json serialise failed: {}", err)).ok();
                return Dispatch::Err(AppExit::Generic);
            }
        }
    } else {
        print_human(out, &doctor_report, explain);
    }

    if let Some(path) = report {
        match write_markdown_report(&path, &doctor_report, explain) {
            Ok(()) => {
                out.text_colored(
                    Color::Gray,
                    &format!("report written to {}", path.display()),
                )
                .ok();
            }
            Err(err) => {
                out.err(&format!(
                    "warning: could not write report to {}: {}",
                    path.display(),
                    err
                ))
                .ok();
            }
        }
    }

    if fix {
        apply_fixes(out, &doctor_report, &ctx);
    }

    let counts = doctor_report.counts();
    out.text_colored(
        Color::Gray,
        &format!(
            "{} checks: pass={} warn={} fail={} skip={} in {}ms",
            doctor_report.entries.len(),
            counts.pass,
            counts.warn,
            counts.fail,
            counts.skip,
            elapsed_ms
        ),
    )
    .ok();

    match doctor_report.summary() {
        CheckStatus::Pass | CheckStatus::Skip => Dispatch::Ok,
        CheckStatus::Warn => {
            out.err("doctor: warnings present").ok();
            Dispatch::Err(AppExit::Usage)
        }
        CheckStatus::Fail => {
            out.err("doctor: failures present").ok();
            Dispatch::Err(AppExit::Usage)
        }
    }
}

fn print_human(out: &Output, report: &DoctorReport, explain: bool) {
    for entry in &report.entries {
        let color = match entry.status {
            CheckStatus::Pass => Color::Green,
            CheckStatus::Warn => Color::Yellow,
            CheckStatus::Fail => Color::Red,
            CheckStatus::Skip => Color::Gray,
        };
        let line = format!(
            "  [{:<2}] {:<10} {:<22} {}",
            entry.status.glyph(),
            entry.category,
            entry.name,
            entry.message
        );
        out.text_colored(color, &line).ok();
        if explain {
            if let Some(s) = &entry.suggestion {
                out.text_colored(Color::Gray, &format!("        suggestion: {}", s))
                    .ok();
            }
            if entry.has_fix {
                out.text_colored(Color::Gray, "        fix: available; pass --fix to apply")
                    .ok();
            }
        }
    }
}

fn write_markdown_report(
    path: &PathBuf,
    report: &DoctorReport,
    explain: bool,
) -> std::io::Result<()> {
    let body = render_markdown(report, explain);
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(path, body)
}

fn render_markdown(report: &DoctorReport, explain: bool) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    let _ = writeln!(s, "# sovereign doctor report ({})", report.level);
    let counts = report.counts();
    let _ = writeln!(
        s,
        "\nSummary: pass={} warn={} fail={} skip={}\n",
        counts.pass, counts.warn, counts.fail, counts.skip
    );
    let _ = writeln!(s, "| status | category | check | message |");
    let _ = writeln!(s, "| ------ | -------- | ----- | ------- |");
    for entry in &report.entries {
        let _ = writeln!(
            s,
            "| {} | {} | {} | {} |",
            entry.status.glyph(),
            entry.category,
            entry.name,
            entry.message.replace('|', "\\|")
        );
    }
    if explain {
        let _ = writeln!(s, "\n## Suggestions\n");
        for entry in &report.entries {
            if let Some(sg) = &entry.suggestion {
                let _ = writeln!(s, "- **{}**: {}\n", entry.name, sg);
            }
        }
    }
    s.push_str("\n_Share this report with care: it includes host paths, kernel release, and runtime versions._\n");
    s
}

fn apply_fixes(out: &Output, _report: &DoctorReport, _ctx: &CheckContext) {
    // The basic-level fix registry walks the report's entries and
    // re-invokes the check's `fix`. In V0 we only ship
    // `master_key_repair_perms` and `sqlite_enable_wal`; the
    // runtime/proxy fixes (docker system prune, caddy reload) are
    // V1+ because they shell out to a host binary.
    out.text_colored(
        Color::Gray,
        "--fix is reserved for V1+; the basic-level fixes run automatically from the check itself",
    )
    .ok();
}
