//! Horizon-lite dashboard (S10 B-20): a single static page served by keel-api
//! over the JSON meta surface (/healthz, /readyz, /meta/*, /metrics). No JS
//! framework, no build step, no extra dependency — mirrors the SPA starter
//! posture from S9. Read-only by design.
//!
//! Wave 8 ADR 0009 adds the operational half, `GET /_dashboard/ops`: the
//! same `dashboard_enabled` gate, the shared session-or-PAT identity
//! resolution (anonymous -> 401, no capability — B-33), and four read-only
//! sections over the queue, scheduler and trace stores rendered from the
//! views engine — degrading to the JSON envelope of the SAME sections when
//! no view renders (never a 500).

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::Json;
use keel_http::{Envelope, ResolvedSession};
use serde::Deserialize;
use serde_json::{json, Value};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::routes::AppState;

/// The complete dashboard page. Fetches boot/flags/routes/health and (when
/// enabled) /metrics, rendering each as a card. Failures degrade to "n/a"
/// so a disabled /metrics never breaks the page.
pub const DASHBOARD_HTML: &str = r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Keel Dashboard</title>
<style>
  :root { color-scheme: dark; --bg:#0b0e14; --card:#141a24; --edge:#232c3b;
          --fg:#dbe4f0; --dim:#7d8aa0; --ok:#3fb96f; --warn:#d9a441; --err:#d95757;
          --accent:#4f8cff; }
  * { box-sizing: border-box; }
  body { margin:0; background:var(--bg); color:var(--fg);
         font:14px/1.5 system-ui, -apple-system, "Segoe UI", sans-serif; }
  header { display:flex; align-items:baseline; gap:12px; padding:20px 28px 8px; }
  header h1 { font-size:20px; margin:0; letter-spacing:.4px; }
  header .sub { color:var(--dim); font-size:12px; }
  .badge { display:inline-block; padding:2px 10px; border-radius:999px; font-size:12px;
           border:1px solid var(--edge); background:var(--card); }
  .badge.ok { color:var(--ok); border-color:var(--ok); }
  .badge.err { color:var(--err); border-color:var(--err); }
  main { display:grid; grid-template-columns:repeat(auto-fit,minmax(320px,1fr));
         gap:16px; padding:16px 28px 32px; }
  section { background:var(--card); border:1px solid var(--edge); border-radius:10px;
            padding:16px; min-height:120px; }
  section h2 { font-size:13px; text-transform:uppercase; letter-spacing:1px;
               color:var(--dim); margin:0 0 10px; }
  table { width:100%; border-collapse:collapse; font-size:13px; }
  td, th { text-align:left; padding:3px 6px; border-bottom:1px solid var(--edge);
           vertical-align:top; }
  th { color:var(--dim); font-weight:500; }
  code, pre { font-family:ui-monospace, Consolas, monospace; }
  pre { white-space:pre-wrap; word-break:break-all; margin:0; font-size:12px;
        color:var(--fg); max-height:280px; overflow:auto; }
  .pill { display:inline-block; margin:2px 4px 2px 0; padding:1px 8px; border-radius:6px;
          background:#1c2534; border:1px solid var(--edge); font-size:12px; }
  .k { color:var(--accent); }
  .empty { color:var(--dim); font-style:italic; }
</style>
</head>
<body>
<header>
  <h1>Keel</h1>
  <span class="sub">Horizon-lite runtime dashboard</span>
  <span id="health" class="badge">…</span>
  <span id="ready" class="badge">…</span>
</header>
<main>
  <section><h2>Boot</h2><div id="boot" class="empty">loading…</div></section>
  <section><h2>Feature flags</h2><div id="flags" class="empty">loading…</div></section>
  <section><h2>Graph routes</h2><div id="routes" class="empty">loading…</div></section>
  <section><h2>Services</h2><div id="services" class="empty">loading…</div></section>
  <section style="grid-column:1/-1"><h2>Metrics (Prometheus)</h2><div id="metrics" class="empty">loading…</div></section>
</main>
<script>
async function j(url){ const r = await fetch(url); return { status:r.status, json: await r.json().catch(()=>null) }; }
function set(id, html){ document.getElementById(id).innerHTML = html; }
function esc(s){ return String(s).replace(/[&<>"]/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;'}[c])); }
async function load(){
  try {
    const h = await j("/healthz");
    const ok = h.json && h.json.data && h.json.data.ok;
    set("health", ""); const hb = document.getElementById("health");
    hb.textContent = ok ? "healthy" : (h.json && h.json.data ? "degraded" : "unknown");
    hb.className = "badge " + (ok ? "ok" : "err");
  } catch { set("health","n/a"); }
  try {
    const r = await fetch("/readyz");
    const rb = document.getElementById("ready");
    rb.textContent = r.status === 200 ? "ready" : "not ready";
    rb.className = "badge " + (r.status === 200 ? "ok" : "err");
  } catch { set("ready","n/a"); }
  try {
    const b = await j("/meta/boot");
    const d = b.json.data || {};
    set("boot", "<table><tr><th>name</th><td>" + esc(d.name||"") + "</td></tr>" +
                "<tr><th>version</th><td>" + esc(d.version||"") + "</td></tr></table>");
    const services = d.services || [];
    set("services", services.length
      ? "<table>" + services.map(s=>"<tr><td><code>"+esc(s)+"</code></td></tr>").join("") + "</table>"
      : '<span class="empty">none</span>');
  } catch { set("boot","n/a"); set("services","n/a"); }
  try {
    const f = await j("/meta/flags");
    const flags = (f.json.data || {}).enabled || [];
    set("flags", flags.length
      ? flags.map(x=>'<span class="pill">'+esc(x)+"</span>").join("")
      : '<span class="empty">none enabled</span>');
  } catch { set("flags","n/a"); }
  try {
    const rt = await j("/meta/routes");
    const routes = (rt.json.data || {}).routes || [];
    set("routes", routes.length
      ? "<table><tr><th>method</th><th>path</th><th>operation</th></tr>" +
        routes.map(r=>"<tr><td>"+esc(r.method)+"</td><td><code>"+esc(r.path)+"</code></td><td>"+esc(r.operation||"")+"</td></tr>").join("") +
        "</table>"
      : '<span class="empty">no graph routes</span>');
  } catch { set("routes","n/a"); }
  try {
    const m = await fetch("/metrics");
    if (m.status === 200) {
      const text = await m.text();
      set("metrics", "<pre>" + esc(text || "# (no series yet)") + "</pre>");
    } else {
      set("metrics", '<span class="empty">disabled (flag app.metrics off) — HTTP ' + m.status + "</span>");
    }
  } catch { set("metrics","n/a"); }
}
load(); setInterval(load, 5000);
</script>
</body>
</html>
"##;

// =========================== Wave 8 ADR 0009: ops dashboard =============
//
// `GET /_dashboard/ops`: read-only over the queue, scheduler and trace
// stores. Sections degrade inline (never a 500); the page meta-refreshes
// server-side (no JavaScript); secrets stay out of the HTML by
// construction — rows are whitelisted field sets, never store objects.

/// The shipped view (`views/dashboard/ops.html` + its partials), resolved
/// by the views engine's dotted-name convention (`.` -> path separator).
const OPS_VIEW: &str = "dashboard.ops";

/// ADR 0009: server-side refresh cadence — `<meta http-equiv="refresh">`,
/// no JavaScript.
const OPS_REFRESH_SECS: u64 = 10;

/// ADR 0009: the platform-shipped view root (dashboard/ops.html plus its
/// partials). Appended LAST to every views path list — the production
/// `[views] paths` section and the harness alike — so an app's own
/// `dashboard/ops.html` in any declared path overrides the shipped page.
pub fn builtin_views_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("views")
}

/// ADR 0009 query: `?trace=<execution id>` renders ONE execution's detail
/// row through `TraceStore::get`.
#[derive(Debug, Default, Deserialize)]
pub struct OpsQuery {
    pub trace: Option<String>,
}

/// Wave 8 ADR 0009: the operational dashboard. Same `dashboard_enabled`
/// gate as the lite page (disabled -> the existing 404 envelope); identity
/// through the shared resolver (session cookie or bearer PAT; anonymous ->
/// 401; no capability check — B-33); HTML from the views engine when one
/// renders, the JSON envelope of the SAME four sections otherwise. Every
/// section degrades inline — this route never 500s.
pub async fn ops_dashboard(
    State(state): State<AppState>,
    headers: HeaderMap,
    session: Option<axum::extract::Extension<ResolvedSession>>,
    Query(query): Query<OpsQuery>,
) -> Response {
    if !state.dashboard_enabled {
        let err =
            keel_foundation::AppError::not_found("dashboard disabled (enable flag app.dashboard)");
        return state.error_json(&err).into_response();
    }
    if let Err(err) =
        crate::graph_routes::resolve_identity(&state, &headers, session.as_deref())
    {
        return state.error_json(&err).into_response();
    }
    let body = json!({
        "dashboard": "ops",
        "generated_at": rfc3339(OffsetDateTime::now_utc()),
        "refresh_secs": OPS_REFRESH_SECS,
        "sections": collect_ops_sections(
            &state,
            query.trace.as_deref().filter(|raw| !raw.is_empty()),
        ),
    });
    match state.views.as_ref() {
        Some(engine) if engine.exists(OPS_VIEW) => match engine.render(OPS_VIEW, body.clone()) {
            Ok(html) => Html(html).into_response(),
            // A render failure degrades exactly like a missing engine:
            // the page contract is "never a 500".
            Err(_) => ops_json(body),
        },
        _ => ops_json(body),
    }
}

/// The JSON degradation: `{"ok":true,"data":{...}}` carrying the same
/// sections the view renders.
fn ops_json(body: Value) -> Response {
    (
        StatusCode::OK,
        Json(serde_json::to_value(Envelope::ok(body)).unwrap_or_default()),
    )
        .into_response()
}

/// The four sections as ONE data document — the single source for both the
/// view and the JSON envelope ("the same four sections"), built fresh at
/// render time from handles the process already has. No writes, no pollers.
fn collect_ops_sections(state: &AppState, trace: Option<&str>) -> Value {
    json!({
        "queue": queue_section(state),
        "jobs": jobs_section(state),
        "schedule": schedule_section(state),
        "inspect": inspect_section(state, trace),
    })
}

/// A degraded section: inline note, never an error.
fn note(text: impl Into<String>) -> Value {
    json!({ "ok": false, "note": text.into() })
}

fn rfc3339(at: OffsetDateTime) -> String {
    at.format(&Rfc3339).unwrap_or_default()
}

/// Queue depth half (`queue:*` CLI reads the same store).
fn queue_section(state: &AppState) -> Value {
    let Some(queue) = state.queue.as_ref() else {
        return note("queue store unavailable (degraded boot)");
    };
    match queue.snapshot() {
        Ok(snap) => json!({
            "ok": true,
            "pending": snap.pending,
            "running": snap.running,
            "dead": snap.dead,
            "succeeded": snap.succeeded,
            "oldest_pending_age_secs": snap
                .oldest_pending
                .map(|at| (OffsetDateTime::now_utc() - at).whole_seconds().max(0)),
        }),
        Err(err) => note(format!("queue store unavailable ({})", err.public_message)),
    }
}

/// Recent-jobs half. Whitelist (ADR 0009): `payload` is business data and
/// is never copied out — only these fields are rendered.
fn jobs_section(state: &AppState) -> Value {
    let Some(queue) = state.queue.as_ref() else {
        return note("queue store unavailable (degraded boot)");
    };
    match queue.list(None, 50) {
        Ok(jobs) => json!({
            "ok": true,
            "rows": jobs.iter().map(|job| json!({
                "id": job.id,
                "name": job.name,
                "queue": job.queue,
                "status": job.status,
                "attempt": job.attempt,
                "max_attempts": job.max_attempts,
                "priority": job.priority,
                "enqueued_at": rfc3339(job.enqueued_at),
                "finished_at": job.finished_at.map(rfc3339),
                "last_error": job.last_error,
            })).collect::<Vec<Value>>(),
        }),
        Err(err) => note(format!("queue store unavailable ({})", err.public_message)),
    }
}

/// Schedule half, two independent panes: the persisted tick state (the SAME
/// `storage/framework/schedule-state.json` the worker writes) and the
/// in-process registry. The root derives from the static-files handle
/// (`<root>/public` -> `<root>`); registration is programmatic (docs/13),
/// so an absent registry reports that honestly instead of a bare empty list.
fn schedule_section(state: &AppState) -> Value {
    let state_half = match state.static_files.as_ref().and_then(|sf| sf.root().parent()) {
        None => note("schedule state unavailable (no storage root)"),
        Some(root) => match keel_scheduling::load_state_doc(root) {
            Err(err) => note(format!("schedule state unavailable ({})", err.public_message)),
            Ok(doc) if doc.last_fired.is_empty() && doc.failures.is_empty() => json!({
                "ok": true,
                "note": "no schedule state yet",
                "last_fired": [],
                "failures": [],
            }),
            Ok(doc) => json!({
                "ok": true,
                "note": Value::Null,
                "last_fired": doc.last_fired.iter().map(|entry| json!({
                    "name": entry.name,
                    "at": rfc3339(entry.at),
                })).collect::<Vec<Value>>(),
                "failures": doc.failures.iter().map(|entry| json!({
                    "name": entry.name,
                    "at": rfc3339(entry.at),
                    "error": entry.error,
                })).collect::<Vec<Value>>(),
            }),
        },
    };
    let registry_half = match state.schedule.as_ref() {
        None => note("no schedules registered (registration is programmatic; see docs/13)"),
        Some(registry) => {
            let now = OffsetDateTime::now_utc();
            json!({
                "ok": true,
                "items": registry.items().iter().map(|item| json!({
                    "name": item.name,
                    "enabled": item.enabled,
                    "next_run": item.next_run(now).map(rfc3339).ok(),
                })).collect::<Vec<Value>>(),
            })
        }
    };
    json!({ "state": state_half, "registry": registry_half })
}

/// Execution-inspect half: recent rows, or ONE detail row behind
/// `?trace=<execution id>`. Whitelist (ADR 0009): `input` is documented
/// environment-internal and is never copied out.
fn inspect_section(state: &AppState, trace: Option<&str>) -> Value {
    let Some(store) = state.traces.as_ref() else {
        return note("trace store unavailable");
    };
    if let Some(raw) = trace {
        let found = uuid::Uuid::parse_str(raw)
            .ok()
            .map(keel_foundation::ExecutionId::from_uuid)
            .and_then(|id| store.get(id).ok().flatten());
        return match found {
            Some(execution) => json!({ "ok": true, "detail": trace_row(&execution) }),
            None => note(format!("trace not found: {raw}")),
        };
    }
    match store.recent(50) {
        Ok(executions) => json!({
            "ok": true,
            "rows": executions.iter().map(trace_row).collect::<Vec<Value>>(),
        }),
        Err(err) => note(format!("trace store unavailable ({})", err.public_message)),
    }
}

/// The inspect row's field set — both list rows and the `?trace=` detail.
fn trace_row(execution: &keel_execution::ExecutionTrace) -> Value {
    json!({
        "execution_id": execution.execution_id.to_string(),
        "operation": execution.operation,
        "profile": execution.profile,
        "principal": execution.principal.to_string(),
        "tenant": execution.tenant.map(|id| id.to_string()),
        "capability": execution.capability,
        "authz": execution.authz,
        "ok": execution.ok(),
        "error": execution.error,
        "query_count": execution.query_count(),
        "total_query_ms": execution.total_query_ms(),
        "deltas_written": execution.deltas_written,
        "audit_written": execution.audit_written,
        "outbox_queued": execution.outbox_queued,
        "started_at": rfc3339(execution.started_at),
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use keel_execution::TraceStore;
    use keel_jobs::JobStore;

    use super::*;

    /// Marker strings that must NEVER reach rendered output (ADR 0009
    /// whitelist: job payload and execution input stay environment-internal).
    const SECRET_PAYLOAD: &str = "SECRET-PAYLOAD-MARKER";
    const SECRET_INPUT: &str = "SECRET-INPUT-MARKER";

    fn sample_job(name: &str) -> keel_jobs::DurableJob {
        let now = OffsetDateTime::now_utc();
        keel_jobs::DurableJob {
            id: uuid::Uuid::now_v7(),
            execution_id: keel_foundation::ExecutionId::new(),
            name: name.to_string(),
            payload: json!({ "note": SECRET_PAYLOAD }),
            queue: "default".to_string(),
            attempt: 1,
            max_attempts: 3,
            deadline: now + time::Duration::hours(1),
            available_at: now,
            status: keel_jobs::JobStatus::Pending,
            priority: 5,
            claimed_by: None,
            claimed_at: None,
            causation_id: None,
            correlation_id: None,
            last_error: Some("boom: previous attempt failed".to_string()),
            enqueued_at: now,
            finished_at: None,
        }
    }

    fn sample_trace(operation: &str) -> keel_execution::ExecutionTrace {
        keel_execution::ExecutionTrace {
            execution_id: keel_foundation::ExecutionId::new(),
            operation: operation.to_string(),
            profile: "api".to_string(),
            principal: keel_foundation::PrincipalId::new(),
            principal_display: Some("ops test".to_string()),
            tenant: Some(keel_foundation::TenantId::new()),
            tenant_slug: None,
            plane: "api".to_string(),
            scope_paths: vec!["appointments.list".to_string()],
            capability: Some("appointments.list".to_string()),
            authz: "allowed".to_string(),
            idempotent_replay: false,
            queries: vec![keel_execution::QueryProbe {
                shape: "list".to_string(),
                label: "memory:read".to_string(),
                duration_ms: 1.5,
            }],
            deltas_written: 0,
            audit_written: 0,
            outbox_queued: 0,
            result_bytes: 42,
            error: None,
            input: json!({ "filter": SECRET_INPUT }),
            started_at: OffsetDateTime::now_utc(),
            duration_ms: 2.5,
        }
    }

    /// Render the shipped view over the builtin root only — proving the
    /// platform template ships, independent of any fixture.
    fn render_ops(sections: Value) -> String {
        keel_views::ViewsEngine::new(keel_views::ViewsConfig {
            paths: vec![builtin_views_root()],
            cache_dir: None,
            env: "test".into(),
        })
        .render(
            OPS_VIEW,
            json!({
                "generated_at": "2026-10-02T00:00:00Z",
                "refresh_secs": OPS_REFRESH_SECS,
                "sections": sections,
            }),
        )
        .expect("ops view renders")
    }

    #[test]
    fn dashboard_html_is_well_formed_enough() {
        assert!(DASHBOARD_HTML.starts_with("<!doctype html>"));
        assert!(DASHBOARD_HTML.contains("/meta/boot"));
        assert!(DASHBOARD_HTML.contains("/metrics"));
        assert!(DASHBOARD_HTML.contains("/_dashboard") || DASHBOARD_HTML.contains("Keel"));
    }

    #[test]
    fn ops_view_renders_all_four_sections_with_seeded_data() {
        let dir = std::env::temp_dir().join(format!("keel-api-ops-a-{}", uuid::Uuid::now_v7()));
        let (mut state, _graph, _store) = crate::build_app_state(&dir, true, true);

        // Harness boots without queue/traces handles (D-52) — seed them.
        let queue = keel_jobs::InMemoryJobStore::new();
        queue
            .enqueue(sample_job("ops-widget-check"))
            .expect("job enqueues");
        state.queue = Some(Arc::new(queue));
        let traces = keel_execution::InMemoryTraceStore::new();
        traces
            .save(&sample_trace("appointments.list"))
            .expect("trace saves");
        state.traces = Some(Arc::new(traces));

        let sections = collect_ops_sections(&state, None);
        assert_eq!(sections["queue"]["ok"], true);
        assert_eq!(sections["jobs"]["rows"][0]["name"], "ops-widget-check");
        assert_eq!(
            sections["inspect"]["rows"][0]["operation"],
            "appointments.list"
        );
        // No registry + no state file yet: both schedule panes say so.
        assert!(
            sections["schedule"]["state"]["note"]
                .as_str()
                .unwrap()
                .contains("no schedule state yet")
        );
        assert!(
            sections["schedule"]["registry"]["note"]
                .as_str()
                .unwrap()
                .contains("no schedules registered")
        );

        // The shipped view renders the page with the refresh contract.
        let engine = keel_views::ViewsEngine::new(keel_views::ViewsConfig {
            paths: vec![builtin_views_root()],
            cache_dir: None,
            env: "test".into(),
        });
        assert!(
            engine.exists(OPS_VIEW),
            "shipped dashboard/ops.html resolves"
        );
        let html = render_ops(sections);
        assert!(
            html.contains("<meta http-equiv=\"refresh\" content=\"10\""),
            "server-side refresh: {html}"
        );
        for heading in [
            "Queue depth",
            "Recent jobs",
            "Schedule state",
            "Execution inspect",
        ] {
            assert!(html.contains(heading), "section {heading} rendered");
        }
        assert!(html.contains("ops-widget-check"));
        assert!(html.contains("appointments.list"));
        assert!(html.contains("no schedule state yet"));
        assert!(html.contains("no schedules registered"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ops_sections_degrade_inline_without_handles() {
        let dir = std::env::temp_dir().join(format!("keel-api-ops-b-{}", uuid::Uuid::now_v7()));
        let (state, _graph, _store) = crate::build_app_state(&dir, true, true);
        // Harness defaults: queue, traces and schedule are all None (D-52).

        let sections = collect_ops_sections(&state, None);
        assert_eq!(sections["queue"]["ok"], false);
        assert!(
            sections["queue"]["note"]
                .as_str()
                .unwrap()
                .contains("degraded boot")
        );
        assert_eq!(sections["jobs"]["ok"], false);
        assert_eq!(sections["inspect"]["ok"], false);
        assert_eq!(sections["inspect"]["note"], "trace store unavailable");
        assert!(
            sections["schedule"]["state"]["note"]
                .as_str()
                .unwrap()
                .contains("no schedule state yet")
        );
        assert!(
            sections["schedule"]["registry"]["note"]
                .as_str()
                .unwrap()
                .contains("no schedules registered")
        );

        // The page still renders — degradation is inline, never a 500.
        let html = render_ops(sections);
        assert!(html.contains("queue store unavailable (degraded boot)"));
        assert!(html.contains("trace store unavailable"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ops_sections_whitelist_payload_and_input_out() {
        let dir = std::env::temp_dir().join(format!("keel-api-ops-c-{}", uuid::Uuid::now_v7()));
        let (mut state, _graph, _store) = crate::build_app_state(&dir, true, true);

        let queue = keel_jobs::InMemoryJobStore::new();
        queue
            .enqueue(sample_job("ops-secret-check"))
            .expect("job enqueues");
        state.queue = Some(Arc::new(queue));
        let traces = keel_execution::InMemoryTraceStore::new();
        let execution = sample_trace("secrets.read");
        let execution_id = execution.execution_id;
        traces.save(&execution).expect("trace saves");
        state.traces = Some(Arc::new(traces));

        let list = collect_ops_sections(&state, None);
        let wire = list.to_string();
        assert!(!wire.contains(SECRET_PAYLOAD), "payload rendered:\n{wire}");
        assert!(!wire.contains(SECRET_INPUT), "input rendered:\n{wire}");
        assert!(wire.contains("ops-secret-check"));
        assert!(wire.contains("secrets.read"));

        // The `?trace=` detail row whitelists the same way (TraceStore::get).
        let detail = collect_ops_sections(&state, Some(&execution_id.to_string()));
        assert_eq!(detail["inspect"]["detail"]["operation"], "secrets.read");
        let wire = detail.to_string();
        assert!(
            !wire.contains(SECRET_INPUT),
            "detail input rendered:\n{wire}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
