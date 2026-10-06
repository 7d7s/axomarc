//! `keel observe` family (Sentinel S5, doc §28/§34): operate the telemetry
//! runtime from local stores — exported JSONL events, WAL segments, exporter
//! health snapshots, cost rollups, the incident file control plane, the trace
//! store, and the audit chain (both drivers). No new ports: commands read
//! files or the configured database only.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Subcommand;
use keel_config::PlatformConfig;
use keel_execution::TraceStore;
use keel_mysql::DbPool;
use keel_observability::{IncidentControlPlane, TelemetryConfig};
use serde_json::Value;

#[derive(Subcommand)]
pub enum ObserveCmd {
    /// Runtime status assembled from local stores (no live host needed).
    Status,
    /// Tail exported log events (file exporter JSONL).
    Logs {
        /// How many recent events to show.
        #[arg(long, default_value_t = 100)]
        last: usize,
        /// Filter by level: debug | info | warn | error.
        #[arg(long)]
        level: Option<String>,
    },
    /// Error + exception events, grouped by fingerprint.
    Errors {
        #[arg(long, default_value_t = 50)]
        last: usize,
    },
    /// Recorded execution traces (`storage/framework/traces`).
    Traces {
        /// Show one execution in full.
        #[arg(long)]
        trace: Option<String>,
        #[arg(long, default_value_t = 10)]
        last: usize,
    },
    /// One-command request reconstruction: trace + events + context.
    Request {
        /// Execution id, request id or trace id.
        id: String,
    },
    /// Audit log from postgres; `--verify` recomputes the hash chain.
    Audit {
        /// Recompute and verify the audit hash chain (D-74).
        #[arg(long)]
        verify: bool,
        /// Restrict to one tenant (uuid).
        #[arg(long)]
        tenant: Option<String>,
        /// Only rows at/after this RFC3339 timestamp.
        #[arg(long)]
        since: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Active + historical incidents (file control plane).
    Incidents,
    /// Start or stop incident mode (writes the control file).
    Incident {
        /// start | stop.
        action: String,
        /// Recorded on the incident marker.
        #[arg(long)]
        operator: Option<String>,
    },
    /// Exporter health snapshots.
    Exporters,
    /// Effective telemetry policy rules + sampling rates.
    Policies,
    /// WAL buffer state (segments, bytes, watermark).
    Buffers,
    /// Health rollup: exporters + incident + WAL.
    Health,
    /// Cost rollup (cost.json).
    Cost,
    /// Where metric numbers live (live registry vs local rollups).
    Metrics,
}

pub fn dispatch(cmd: ObserveCmd) -> anyhow::Result<ExitCode> {
    let root = crate::workspace_root()?;
    let cfg = crate::load_config(&root)?;
    let tcfg = telemetry_config(&root, &cfg);
    match cmd {
        ObserveCmd::Status => status(&tcfg),
        ObserveCmd::Logs { last, level } => logs(&tcfg, last, level.as_deref()),
        ObserveCmd::Errors { last } => errors_cmd(&tcfg, last),
        ObserveCmd::Traces { trace, last } => traces(&root, trace.as_deref(), last),
        ObserveCmd::Request { id } => request(&root, &tcfg, &id),
        ObserveCmd::Audit {
            verify,
            tenant,
            since,
            limit,
        } => crate::rt(audit(&root, verify, tenant, since, limit)),
        ObserveCmd::Incidents => incidents(&tcfg),
        ObserveCmd::Incident { action, operator } => {
            incident_cmd(&tcfg, &action, operator.as_deref())
        }
        ObserveCmd::Exporters => exporters(&tcfg),
        ObserveCmd::Policies => policies(&tcfg),
        ObserveCmd::Buffers => buffers(&tcfg),
        ObserveCmd::Health => health(&tcfg),
        ObserveCmd::Cost => cost(&tcfg),
        ObserveCmd::Metrics => metrics_note(&tcfg),
    }
}

// ------------------------------------------------------------------ helpers

fn telemetry_config(root: &Path, cfg: &PlatformConfig) -> TelemetryConfig {
    let mut tcfg = TelemetryConfig::from_section(cfg.telemetry.as_ref());
    tcfg.apply_env();
    tcfg.rebase(root);
    tcfg
}

/// Path of the file exporter target (the canonical exported event stream).
fn log_file(tcfg: &TelemetryConfig) -> Option<PathBuf> {
    tcfg.exporters
        .values()
        .find(|e| e.kind == "file")
        .and_then(|e| e.target.clone())
        .map(PathBuf::from)
}

fn read_events(path: &Path) -> Vec<Value> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    raw.lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect()
}

fn read_json(path: &Path) -> Option<Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
}

fn ev_str<'a>(v: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut cur = v;
    for key in path {
        cur = cur.get(key)?;
    }
    cur.as_str()
}

fn ev_level(v: &Value) -> Option<&str> {
    ev_str(v, &["payload", "level"])
}

fn ev_message(v: &Value) -> String {
    if let Some(message) = ev_str(v, &["payload", "message"]) {
        return message.to_string();
    }
    if let Some(ty) = ev_str(v, &["payload", "type_name"]) {
        let message = ev_str(v, &["payload", "message"]).unwrap_or("");
        return format!("{ty}: {message}");
    }
    if let Some(kind) = ev_str(v, &["payload", "kind"]) {
        return kind.to_string();
    }
    ev_str(v, &["schema"]).unwrap_or("?").to_string()
}

fn ev_context_suffix(v: &Value) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(trace) = ev_str(v, &["context", "trace_id"]) {
        parts.push(format!("trace={trace}"));
    }
    if let Some(execution) = ev_str(v, &["context", "execution_id"]) {
        parts.push(format!("exec={execution}"));
    }
    if let Some(tenant) = ev_str(v, &["context", "tenant_id"]) {
        parts.push(format!("tenant={tenant}"));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("  [{}]", parts.join(" "))
    }
}

fn ev_fields_json(v: &Value) -> Option<String> {
    let fields = v.get("payload")?.get("fields")?;
    let obj = fields.as_object()?;
    if obj.is_empty() {
        return None;
    }
    serde_json::to_string(fields).ok()
}

fn print_event_line(v: &Value) {
    let at = ev_str(v, &["at"]).unwrap_or("-");
    let schema = ev_str(v, &["schema"]).unwrap_or("-");
    let level = ev_level(v).map(|l| l.to_ascii_uppercase());
    let message = ev_message(v);
    let context = ev_context_suffix(v);
    let fields = ev_fields_json(v)
        .map(|f| format!("  {f}"))
        .unwrap_or_default();
    match level {
        Some(level) => println!("{at} {level:<5} {message}{context}{fields}"),
        None => println!("{at} {schema:<14} {message}{context}{fields}"),
    }
}

// ------------------------------------------------------------- incidents

fn incidents(tcfg: &TelemetryConfig) -> anyhow::Result<ExitCode> {
    let plane = IncidentControlPlane::new(tcfg.paths.incident_dir.clone());
    match plane.read() {
        Some(active) => {
            println!("ACTIVE INCIDENT");
            println!("  id:         {}", active.id);
            println!("  started_at: {}", active.started_at);
            if let Some(by) = &active.started_by {
                println!("  started_by: {by}");
            }
            if !active.flags.is_empty() {
                println!("  flags:      {:?}", active.flags);
            }
        }
        None => println!("no active incident"),
    }
    let history = plane.history();
    if history.is_empty() {
        println!("\nhistory: empty");
    } else {
        println!("\nhistory ({}):", history.len());
        for item in history.iter().rev().take(20) {
            let by = item.started_by.clone().unwrap_or_else(|| "-".into());
            println!("  {}  started_at={}  by={}", item.id, item.started_at, by);
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn incident_cmd(
    tcfg: &TelemetryConfig,
    action: &str,
    operator: Option<&str>,
) -> anyhow::Result<ExitCode> {
    let plane = IncidentControlPlane::new(tcfg.paths.incident_dir.clone());
    match action {
        "start" => {
            let incident = plane.start(operator)?;
            println!("incident started: {}", incident.id);
            println!("  control file: {}", plane.active_path().display());
            println!("hosts poll every 2 s: error/span classes escalate to 100% sampling");
            Ok(ExitCode::SUCCESS)
        }
        "stop" => match plane.stop()? {
            Some(incident) => {
                println!("incident stopped: {}", incident.id);
                Ok(ExitCode::SUCCESS)
            }
            None => {
                println!("no incident was active");
                Ok(ExitCode::SUCCESS)
            }
        },
        other => anyhow::bail!("unknown action {other:?} (expected start|stop)"),
    }
}

// --------------------------------------------------------- exporters/health

fn load_exporter_health(tcfg: &TelemetryConfig) -> Vec<Value> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&tcfg.paths.health_dir) else {
        return out;
    };
    for entry in entries.flatten() {
        if entry.path().extension().and_then(|s| s.to_str()) == Some("json") {
            if let Some(value) = read_json(&entry.path()) {
                out.push(value);
            }
        }
    }
    out.sort_by_cached_key(|v| ev_str(v, &["name"]).unwrap_or("").to_string());
    out
}

fn exporters(tcfg: &TelemetryConfig) -> anyhow::Result<ExitCode> {
    let health = load_exporter_health(tcfg);
    if health.is_empty() {
        println!(
            "no exporter health snapshots under {}",
            tcfg.paths.health_dir.display()
        );
        println!("(snapshots appear once a host with the telemetry runtime has ticked)");
        return Ok(ExitCode::SUCCESS);
    }
    println!(
        "{:<16} {:<10} {:>9} {:>9} {:>9} {:>14} {:>12}",
        "NAME", "STATE", "ATTEMPTS", "FAILURES", "SUCCESSES", "EVENTS", "BYTES"
    );
    for h in &health {
        println!(
            "{:<16} {:<10} {:>9} {:>9} {:>9} {:>14} {:>12}",
            ev_str(h, &["name"]).unwrap_or("-"),
            ev_str(h, &["state"]).unwrap_or("-"),
            h.get("attempts").and_then(Value::as_u64).unwrap_or(0),
            h.get("failures").and_then(Value::as_u64).unwrap_or(0),
            h.get("successes").and_then(Value::as_u64).unwrap_or(0),
            h.get("exported_events")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            h.get("exported_bytes").and_then(Value::as_u64).unwrap_or(0),
        );
        if let Some(last_error) = ev_str(h, &["last_error"]) {
            println!("  last_error: {last_error}");
        }
    }
    Ok(ExitCode::SUCCESS)
}

// ------------------------------------------------------------------- buffers

fn wal_segments(dir: &Path) -> Vec<(u64, u64)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut segments: Vec<(u64, u64)> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(first_lsn) = name
            .strip_prefix("segment-")
            .and_then(|s| s.strip_suffix(".wal"))
            .and_then(|s| s.parse::<u64>().ok())
        {
            let bytes = entry.metadata().map(|m| m.len()).unwrap_or(0);
            segments.push((first_lsn, bytes));
        }
    }
    segments.sort_by_key(|(lsn, _)| *lsn);
    segments
}

fn wal_watermark(dir: &Path) -> Option<u64> {
    read_json(&dir.join("watermark.json")).and_then(|v| v.get("watermark").and_then(Value::as_u64))
}

fn buffers(tcfg: &TelemetryConfig) -> anyhow::Result<ExitCode> {
    if !tcfg.wal.enabled {
        println!("WAL disabled ([telemetry.wal].enabled = false)");
        return Ok(ExitCode::SUCCESS);
    }
    println!("wal dir: {}", tcfg.wal.dir.display());
    let segments = wal_segments(&tcfg.wal.dir);
    let watermark = wal_watermark(&tcfg.wal.dir);
    let total_bytes: u64 = segments.iter().map(|(_, b)| b).sum();
    println!("segments: {}", segments.len());
    println!("bytes on disk: {total_bytes}");
    match watermark {
        Some(w) => println!("watermark (exported LSN): {w}"),
        None => println!("watermark: none (nothing durably exported yet)"),
    }
    for (lsn, bytes) in &segments {
        println!("  segment-{lsn:020}.wal  {bytes} bytes");
    }
    Ok(ExitCode::SUCCESS)
}

// ----------------------------------------------------------------cost/status

fn cost_snapshot(tcfg: &TelemetryConfig) -> Option<Value> {
    read_json(&tcfg.paths.cost_file)
}

fn cost(tcfg: &TelemetryConfig) -> anyhow::Result<ExitCode> {
    let Some(snapshot) = cost_snapshot(tcfg) else {
        println!("no cost rollup yet at {}", tcfg.paths.cost_file.display());
        return Ok(ExitCode::SUCCESS);
    };
    let total_events = snapshot
        .get("total_events")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let total_bytes = snapshot
        .get("total_bytes")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    println!("total: {total_events} events, {total_bytes} bytes\n");
    let render = |title: &str, key: &str| {
        let Some(map) = snapshot.get(key).and_then(Value::as_object) else {
            return;
        };
        if map.is_empty() {
            return;
        }
        println!("{title}:");
        println!("  {:<32} {:>12} {:>14}", "SOURCE", "EVENTS", "BYTES");
        for (name, entry) in map {
            println!(
                "  {:<32} {:>12} {:>14}",
                name,
                entry.get("events").and_then(Value::as_u64).unwrap_or(0),
                entry.get("bytes").and_then(Value::as_u64).unwrap_or(0),
            );
        }
        println!();
    };
    render("by source", "by_source");
    render("by exporter", "by_exporter");
    Ok(ExitCode::SUCCESS)
}

fn status(tcfg: &TelemetryConfig) -> anyhow::Result<ExitCode> {
    println!("telemetry status (from local stores)");
    println!("  level: {}   format: {}", tcfg.level, tcfg.format);
    println!(
        "  exporters: {} configured; wal {}",
        tcfg.exporters.len(),
        if tcfg.wal.enabled {
            "enabled"
        } else {
            "disabled"
        }
    );

    let plane = IncidentControlPlane::new(tcfg.paths.incident_dir.clone());
    match plane.read() {
        Some(active) => println!(
            "  incident: ACTIVE {} (since {})",
            active.id, active.started_at
        ),
        None => println!("  incident: none"),
    }

    let segments = wal_segments(&tcfg.wal.dir);
    let bytes: u64 = segments.iter().map(|(_, b)| b).sum();
    match wal_watermark(&tcfg.wal.dir) {
        Some(w) => println!(
            "  wal: {} segment(s), {bytes} bytes, watermark={w}",
            segments.len()
        ),
        None => println!(
            "  wal: {} segment(s), {bytes} bytes, no watermark",
            segments.len()
        ),
    }

    let health = load_exporter_health(tcfg);
    if health.is_empty() {
        println!("  exporters: no health snapshots yet");
    } else {
        for h in &health {
            println!(
                "  exporter {}: state={} exported={} events/{} bytes, failures={}",
                ev_str(h, &["name"]).unwrap_or("-"),
                ev_str(h, &["state"]).unwrap_or("-"),
                h.get("exported_events")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                h.get("exported_bytes").and_then(Value::as_u64).unwrap_or(0),
                h.get("failures").and_then(Value::as_u64).unwrap_or(0),
            );
        }
    }

    if let Some(snapshot) = cost_snapshot(tcfg) {
        println!(
            "  cost: {} events / {} bytes emitted",
            snapshot
                .get("total_events")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            snapshot
                .get("total_bytes")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        );
    }
    println!("  policy: {} rule(s)", tcfg.policy.len());
    Ok(ExitCode::SUCCESS)
}

fn health(tcfg: &TelemetryConfig) -> anyhow::Result<ExitCode> {
    let mut degraded: Vec<String> = Vec::new();
    let health = load_exporter_health(tcfg);
    for h in &health {
        let state = ev_str(h, &["state"]).unwrap_or("unknown");
        if state != "ok" && state != "closed" {
            degraded.push(format!(
                "exporter {} in state {state}",
                ev_str(h, &["name"]).unwrap_or("?")
            ));
        }
        if let Some(last_error) = ev_str(h, &["last_error"]) {
            if state != "ok" {
                degraded.push(format!(
                    "exporter {}: {last_error}",
                    ev_str(h, &["name"]).unwrap_or("?")
                ));
            }
        }
    }
    let plane = IncidentControlPlane::new(tcfg.paths.incident_dir.clone());
    if let Some(active) = plane.read() {
        println!(
            "note: incident ACTIVE ({}) — elevated capture is intentional",
            active.id
        );
    }
    if degraded.is_empty() {
        println!("OK: all exporters healthy");
        Ok(ExitCode::SUCCESS)
    } else {
        for reason in &degraded {
            println!("DEGRADED: {reason}");
        }
        Ok(ExitCode::FAILURE)
    }
}

fn metrics_note(tcfg: &TelemetryConfig) -> anyhow::Result<ExitCode> {
    println!("The live metrics registry is in-process; hosts expose it via GET /metrics");
    println!("(flag-gated by `app.metrics`). Local rollups available here:\n");
    let health = load_exporter_health(tcfg);
    let events: u64 = health
        .iter()
        .map(|h| {
            h.get("exported_events")
                .and_then(Value::as_u64)
                .unwrap_or(0)
        })
        .sum();
    let bytes: u64 = health
        .iter()
        .map(|h| h.get("exported_bytes").and_then(Value::as_u64).unwrap_or(0))
        .sum();
    println!("  telemetry_events_exported (sum across exporters): {events}");
    println!("  telemetry_bytes_exported  (sum across exporters): {bytes}");
    if let Some(snapshot) = cost_snapshot(tcfg) {
        println!(
            "  cost totals: {} events / {} bytes (see `keel observe cost`)",
            snapshot
                .get("total_events")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            snapshot
                .get("total_bytes")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn policies(tcfg: &TelemetryConfig) -> anyhow::Result<ExitCode> {
    println!("level: {}   format: {}", tcfg.level, tcfg.format);
    println!("sampling rates by category:");
    if tcfg.sampling.rates.is_empty() {
        println!("  (defaults)");
    }
    for (category, rate) in &tcfg.sampling.rates {
        println!("  {category}: {rate}");
    }
    println!("\npolicy rules: {}", tcfg.policy.len());
    for rule in &tcfg.policy {
        println!("  [{}]", rule.name);
        println!("    when:    {}", rule.when);
        println!("    actions: {}", rule.actions.join(", "));
    }
    let redactions = if tcfg.redactions.is_empty() {
        "(defaults only)".to_string()
    } else {
        format!("{} extra pattern(s)", tcfg.redactions.len())
    };
    println!("\nredactions: {redactions}");
    Ok(ExitCode::SUCCESS)
}

// ---------------------------------------------------------------- logs/errors

fn logs(tcfg: &TelemetryConfig, last: usize, level: Option<&str>) -> anyhow::Result<ExitCode> {
    let Some(path) = log_file(tcfg) else {
        println!("no file exporter configured ([telemetry.exporters.<name>] kind = \"file\");");
        println!("console-only hosts leave no exported event stream to tail.");
        return Ok(ExitCode::SUCCESS);
    };
    if !path.exists() {
        println!("no exported events yet at {}", path.display());
        return Ok(ExitCode::SUCCESS);
    }
    let events = read_events(&path);
    let level_filter = level.map(|l| l.to_ascii_lowercase());
    let filtered: Vec<&Value> = events
        .iter()
        .filter(|v| ev_str(v, &["schema"]) == Some("keel.log"))
        .filter(|v| match (&level_filter, ev_level(v)) {
            (Some(want), Some(have)) => have.eq_ignore_ascii_case(want),
            _ => level_filter.is_none(),
        })
        .collect();
    let start = filtered.len().saturating_sub(last);
    for v in &filtered[start..] {
        print_event_line(v);
    }
    println!(
        "\n({} of {} exported log events{})",
        filtered.len() - start,
        filtered.len(),
        level_filter
            .map(|l| format!(", level={l}"))
            .unwrap_or_default()
    );
    Ok(ExitCode::SUCCESS)
}

fn errors_cmd(tcfg: &TelemetryConfig, last: usize) -> anyhow::Result<ExitCode> {
    let Some(path) = log_file(tcfg) else {
        println!("no file exporter configured; nothing to inspect.");
        return Ok(ExitCode::SUCCESS);
    };
    if !path.exists() {
        println!("no exported events yet at {}", path.display());
        return Ok(ExitCode::SUCCESS);
    }
    let events = read_events(&path);

    // Exception groups: aggregate by fingerprint across the exported stream.
    let mut groups: BTreeMap<String, (u64, String, String, Vec<String>)> = BTreeMap::new();
    for v in events
        .iter()
        .filter(|v| ev_str(v, &["schema"]) == Some("keel.exception"))
    {
        let fingerprint = ev_str(v, &["payload", "fingerprint"])
            .unwrap_or("?")
            .to_string();
        let at = ev_str(v, &["at"]).unwrap_or("-").to_string();
        let ty = ev_str(v, &["payload", "type_name"])
            .unwrap_or("?")
            .to_string();
        let message = ev_str(v, &["payload", "message"]).unwrap_or("").to_string();
        let count = v
            .pointer("/payload/count")
            .and_then(Value::as_u64)
            .unwrap_or(1);
        let entry = groups
            .entry(fingerprint)
            .or_insert_with(|| (0, at.clone(), String::new(), Vec::new()));
        entry.0 += count;
        if at >= entry.1 {
            entry.1 = at;
        }
        if entry.2.is_empty() {
            entry.2 = format!("{ty}: {message}");
        }
        if let Some(route) = ev_str(v, &["payload", "route"]) {
            if !entry.3.iter().any(|r| r == route) && entry.3.len() < 5 {
                entry.3.push(route.to_string());
            }
        }
    }
    if !groups.is_empty() {
        let mut ordered: Vec<_> = groups.iter().collect();
        ordered.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
        println!("exception groups (by fingerprint):");
        for (fingerprint, (count, last_seen, label, routes)) in ordered.iter().take(last) {
            println!("  {count:>6}x  last={last_seen}  {label}");
            println!("          fp={fingerprint}");
            if !routes.is_empty() {
                println!("          routes: {}", routes.join(", "));
            }
        }
        println!();
    }

    // Recent error-level log lines.
    let errors: Vec<&Value> = events
        .iter()
        .filter(|v| {
            ev_str(v, &["schema"]) == Some("keel.exception") || matches!(ev_level(v), Some("error"))
        })
        .collect();
    let start = errors.len().saturating_sub(last);
    println!("recent error events:");
    for v in &errors[start..] {
        print_event_line(v);
    }
    if errors.is_empty() {
        println!("  none");
    }
    Ok(ExitCode::SUCCESS)
}

// ---------------------------------------------------------------- traces

fn trace_summary(trace: &keel_execution::ExecutionTrace) {
    println!(
        "{}  {:<42} {}  authz={}  {:>8.2}ms  queries={}  deltas={}  audit={}  outbox={}",
        trace.started_at,
        trace.operation,
        trace.execution_id,
        trace.authz,
        trace.duration_ms,
        trace.query_count(),
        trace.deltas_written,
        trace.audit_written,
        trace.outbox_queued
    );
    if let Some(error) = &trace.error {
        println!("    error: {error}");
    }
}

fn traces(root: &Path, trace: Option<&str>, last: usize) -> anyhow::Result<ExitCode> {
    let store = keel_execution::FileTraceStore::standard(root);
    if let Some(raw) = trace {
        let id = keel_foundation::ExecutionId::from_uuid(
            uuid::Uuid::parse_str(raw)
                .map_err(|_| anyhow::anyhow!("invalid execution id {raw:?}"))?,
        );
        match store.get(id)? {
            Some(t) => {
                println!("execution {} — {}", t.execution_id, t.operation);
                println!(
                    "  principal: {} {}",
                    t.principal,
                    t.principal_display.clone().unwrap_or_default()
                );
                println!(
                    "  tenant:    {}",
                    t.tenant
                        .map(|t| t.to_string())
                        .unwrap_or_else(|| "-".into())
                );
                println!(
                    "  plane: {}  profile: {}  capability: {}",
                    t.plane,
                    t.profile,
                    t.capability.clone().unwrap_or_default()
                );
                println!(
                    "  authz: {}  idempotent_replay: {}",
                    t.authz, t.idempotent_replay
                );
                println!(
                    "  duration: {:.2}ms  queries: {} ({:.2}ms)",
                    t.duration_ms,
                    t.query_count(),
                    t.total_query_ms()
                );
                println!(
                    "  deltas: {}  audit: {}  outbox: {}  result_bytes: {}",
                    t.deltas_written, t.audit_written, t.outbox_queued, t.result_bytes
                );
                if let Some(error) = &t.error {
                    println!("  error: {error}");
                }
                if !t.queries.is_empty() {
                    println!("  queries:");
                    for q in &t.queries {
                        println!("    {:>8.2}ms  {:<24} {}", q.duration_ms, q.label, q.shape);
                    }
                }
            }
            None => {
                eprintln!("no trace for {raw} under storage/framework/traces");
                return Ok(ExitCode::FAILURE);
            }
        }
        return Ok(ExitCode::SUCCESS);
    }
    let recent = store.recent(last)?;
    if recent.is_empty() {
        println!("no recorded traces under storage/framework/traces");
        return Ok(ExitCode::SUCCESS);
    }
    let mut recent = recent;
    recent.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    for t in &recent {
        trace_summary(t);
    }
    Ok(ExitCode::SUCCESS)
}

// --------------------------------------------------------------- request

fn request(root: &Path, tcfg: &TelemetryConfig, id: &str) -> anyhow::Result<ExitCode> {
    println!("request reconstruction for {id}\n");
    let mut matched = false;

    // 1) Execution trace (id as execution uuid).
    if let Ok(uuid) = uuid::Uuid::parse_str(id) {
        let store = keel_execution::FileTraceStore::standard(root);
        if let Ok(Some(t)) = store.get(keel_foundation::ExecutionId::from_uuid(uuid)) {
            matched = true;
            println!("-- execution --------------------------------------------");
            trace_summary(&t);
        }
    }

    // 2) Telemetry events joined by execution/request/trace id.
    let events = log_file(tcfg).map(|p| read_events(&p)).unwrap_or_default();
    let joined: Vec<&Value> = events
        .iter()
        .filter(|v| {
            ev_str(v, &["context", "execution_id"]) == Some(id)
                || ev_str(v, &["context", "request_id"]) == Some(id)
                || ev_str(v, &["context", "trace_id"]) == Some(id)
        })
        .collect();
    if !joined.is_empty() {
        matched = true;
        println!(
            "-- telemetry events ({}): -------------------------------",
            joined.len()
        );
        if let Some(first) = joined.first() {
            let ctx = first.get("context").cloned().unwrap_or(Value::Null);
            println!(
                "  context: {}",
                serde_json::to_string(&ctx).unwrap_or_default()
            );
        }
        for v in joined.iter().take(200) {
            print_event_line(v);
        }
    }

    if matched {
        Ok(ExitCode::SUCCESS)
    } else {
        eprintln!("nothing found for {id}: no trace file and no exported events reference it");
        Ok(ExitCode::FAILURE)
    }
}

// ----------------------------------------------------------------- audit

async fn audit(
    root: &Path,
    verify: bool,
    tenant: Option<String>,
    since: Option<String>,
    limit: usize,
) -> anyhow::Result<ExitCode> {
    let pool = crate::db_pool(root).await?;

    // B-31: filter literals split by driver - MySQL stores uuid columns as
    // BINARY(16) (a quoted uuid string never equals one) and rejects the
    // RFC3339 'Z' suffix in datetime literals - while the validation itself
    // stays shared and runs up front.
    let mut filters: Vec<String> = vec!["chain_hash IS NOT NULL".into()];
    if let Some(tid) = &tenant {
        let tid = uuid::Uuid::parse_str(tid)
            .map_err(|_| anyhow::anyhow!("invalid tenant id {tid:?}"))?;
        filters.push(match &pool {
            DbPool::Postgres(_) => format!("tenant_id = '{tid}'"),
            // The same 16 bytes sqlx binds for a uuid, as a MySQL hex literal.
            DbPool::MySql(_) => format!("tenant_id = 0x{}", tid.as_simple()),
        });
    }
    if let Some(since) = &since {
        let parsed = time::OffsetDateTime::parse(
            since,
            &time::format_description::well_known::Rfc3339,
        )
        .map_err(|_| anyhow::anyhow!("invalid --since timestamp {since:?} (RFC3339)"))?;
        filters.push(match &pool {
            DbPool::Postgres(_) => format!("at >= '{since}'"),
            // MySQL: 'YYYY-MM-DD HH:MM:SS.ffffff' in UTC - no 'T', no 'Z'.
            DbPool::MySql(_) => {
                let d = parsed.to_offset(time::UtcOffset::UTC);
                format!(
                    "at >= '{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:06}'",
                    d.year(),
                    d.month(),
                    d.day(),
                    d.hour(),
                    d.minute(),
                    d.second(),
                    d.microsecond()
                )
            }
        });
    }
    let where_clause = filters.join(" AND ");

    if !verify {
        let sql = format!(
            "SELECT at, tenant_id, principal_id, action, resource, authority, outcome, chain_hash IS NOT NULL AS chained
               FROM audit_records
              WHERE {where_clause}
              ORDER BY at DESC, id DESC
              LIMIT {limit}"
        );
        // Driver-neutral row shape: sqlx's tuple FromRow is generic over any
        // driver, so both arms feed one print path.
        type Listed = (
            time::OffsetDateTime,
            Option<uuid::Uuid>,
            Option<uuid::Uuid>,
            String,
            String,
            String,
            String,
            bool,
        );
        let rows: Vec<Listed> = match &pool {
            DbPool::Postgres(p) => sqlx::query_as(&sql).fetch_all(p).await?,
            DbPool::MySql(p) => sqlx::query_as(&sql).fetch_all(p).await?,
        };
        if rows.is_empty() {
            println!("no audit rows match");
            return Ok(ExitCode::SUCCESS);
        }
        for (at, tenant_id, principal_id, action, resource, authority, outcome, chained) in rows {
            println!(
                "{}  tenant={}  principal={}  {}  {}/{}  outcome={}  chained={}",
                at,
                tenant_id
                    .map(|t| t.to_string())
                    .unwrap_or_else(|| "-".into()),
                principal_id
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "-".into()),
                action,
                resource,
                authority,
                outcome,
                chained
            );
        }
        return Ok(ExitCode::SUCCESS);
    }

    // --verify: recompute the chain per (tenant, chain_id) segment. The
    // ORDER BY splits by driver: pg sorts NULLs last by default, MySQL has
    // no NULLS FIRST syntax but already sorts NULLs before every value.
    let order_by = match &pool {
        DbPool::Postgres(_) => "ORDER BY tenant_id NULLS FIRST, chain_id, at, id",
        DbPool::MySql(_) => "ORDER BY tenant_id, chain_id, at, id",
    };
    let sql = format!(
        "SELECT id, tenant_id, principal_id, action, resource, authority, outcome, detail, at,
                chain_id, prev_hash, chain_hash
           FROM audit_records
          WHERE {where_clause}
          {order_by}"
    );
    type Chained = (
        uuid::Uuid,
        Option<uuid::Uuid>,
        Option<uuid::Uuid>,
        String,
        String,
        String,
        String,
        serde_json::Value,
        time::OffsetDateTime,
        uuid::Uuid,
        Vec<u8>,
        Vec<u8>,
    );
    let rows: Vec<Chained> = match &pool {
        DbPool::Postgres(p) => sqlx::query_as(&sql).fetch_all(p).await?,
        DbPool::MySql(p) => sqlx::query_as(&sql).fetch_all(p).await?,
    };
    if rows.is_empty() {
        println!("no chained audit rows; nothing to verify");
        return Ok(ExitCode::SUCCESS);
    }

    let mut segments: BTreeMap<(Option<uuid::Uuid>, uuid::Uuid), Vec<keel_integrity::ChainRow>> =
        BTreeMap::new();
    for (id, tenant_id, principal_id, action, resource, authority, outcome, detail, at, chain_id, prev_hash, chain_hash) in rows
    {

        // Mirror the write path (keel-postgres tx): merged detail + Rfc3339.
        let merged_detail = serde_json::json!({ "detail": detail, "outcome": outcome });
        let at_iso = at.format(&time::format_description::well_known::Rfc3339)?;
        let canonical = keel_integrity::canonical_audit_row(
            tenant_id.as_ref().map(|t| t.to_string()).as_deref(),
            principal_id.as_ref().map(|p| p.to_string()).as_deref(),
            &action,
            &resource,
            &authority,
            &merged_detail,
            &at_iso,
        );
        let prev: [u8; 32] = prev_hash
            .as_slice()
            .try_into()
            .map_err(|_| anyhow::anyhow!("row {id}: prev_hash has bad length"))?;
        let head: [u8; 32] = chain_hash
            .as_slice()
            .try_into()
            .map_err(|_| anyhow::anyhow!("row {id}: chain_hash has bad length"))?;
        segments
            .entry((tenant_id, chain_id))
            .or_default()
            .push(keel_integrity::ChainRow {
                row_id: id,
                prev_hash: Some(prev),
                chain_hash: head,
                canonical,
            });
    }

    let mut verified_rows = 0u64;
    let mut failed = false;
    for ((tenant_id, chain_id), chain) in &segments {
        match keel_integrity::verify_chain(chain) {
            Ok(count) => verified_rows += count,
            Err(break_) => {
                failed = true;
                println!(
                    "CHAIN BREAK: tenant={} chain={} position={} row={} reason={:?}",
                    tenant_id
                        .map(|t| t.to_string())
                        .unwrap_or_else(|| "-".into()),
                    chain_id,
                    break_.position,
                    break_.row_id,
                    break_.reason
                );
            }
        }
    }
    println!(
        "verified {} chained row(s) across {} segment(s)",
        verified_rows,
        segments.len()
    );
    if failed {
        println!("RESULT: FAILED — chain divergence detected");
        Ok(ExitCode::FAILURE)
    } else {
        println!("RESULT: OK — all chain links recompute");
        Ok(ExitCode::SUCCESS)
    }
}
