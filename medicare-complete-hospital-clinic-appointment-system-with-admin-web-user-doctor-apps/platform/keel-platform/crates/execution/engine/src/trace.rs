//! Execution diagnostics (S4 items 44/45: inspect/replay, N+1, assert_queries).
//!
//! - [`QueryTap`]: process-global sink where data layers record [`QueryProbe`]s
//!   (SQL executions or store operations) with their elapsed time.
//! - [`ExecutionTrace`]: serializable record of one engine execution — context,
//!   authorization, queries, commit receipt, timing — persisted by the engine
//!   when a [`TraceStore`] is attached (dev/test posture).
//! - [`TraceStore`] ports + file/in-memory implementations under
//!   `storage/framework/traces`.
//! - [`detect_n_plus_one`]: loop-shaped query detection over recorded traces.
//! - [`render_trace`]: the `inspect execution` display format (spec sec. 44).
//!
//! Concurrency note: the tap is process-global; probes are attributed to an
//! execution via mark/take boundaries. Interleaved concurrent executions can
//! interleave probes — diagnostics are a dev/test facility, not an accounting
//! system.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use keel_foundation::{AppError, ExecutionId, PrincipalId, TenantId};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// One observed data-layer operation (SQL statement or store op).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryProbe {
    /// Stable identifier of the query shape (SQL template / op name) — probes
    /// with the same shape inside one execution are N+1 candidates.
    pub shape: String,
    /// Human label, e.g. `pg:delta.write`, `memory:commit`.
    pub label: String,
    pub duration_ms: f64,
}

static TAP_COUNT: AtomicUsize = AtomicUsize::new(0);

fn tap_buffer() -> &'static Mutex<Vec<QueryProbe>> {
    static BUFFER: OnceLock<Mutex<Vec<QueryProbe>>> = OnceLock::new();
    BUFFER.get_or_init(|| Mutex::new(Vec::new()))
}

/// Cap on retained probes; the oldest half is dropped when exceeded so a
/// runaway loop cannot grow the buffer without bound.
const TAP_CAP: usize = 8192;

/// Process-wide measurement sink for data-layer operations.
pub struct QueryTap;

impl QueryTap {
    /// Record one probe. Backends call this around every executed statement
    /// or store operation.
    pub fn record(label: &str, shape: &str, elapsed: std::time::Duration) {
        let probe = QueryProbe {
            shape: shape.to_string(),
            label: label.to_string(),
            duration_ms: elapsed.as_secs_f64() * 1000.0,
        };
        let mut buffer = tap_buffer().lock().expect("query tap poisoned");
        if buffer.len() >= TAP_CAP {
            let drain = TAP_CAP / 2;
            buffer.drain(..drain);
        }
        buffer.push(probe);
        TAP_COUNT.fetch_add(1, Ordering::Relaxed);
    }

    /// Total probes recorded since process start (monotonic).
    pub fn count() -> usize {
        TAP_COUNT.load(Ordering::Relaxed)
    }

    /// Snapshot marker: current probe buffer length.
    pub fn mark() -> usize {
        tap_buffer().lock().expect("query tap poisoned").len()
    }

    /// Remove and return probes recorded after `mark`.
    pub fn take_since(mark: usize) -> Vec<QueryProbe> {
        let mut buffer = tap_buffer().lock().expect("query tap poisoned");
        let start = mark.min(buffer.len());
        buffer.drain(start..).collect()
    }

    /// Drop all buffered probes (test isolation).
    pub fn reset() {
        tap_buffer().lock().expect("query tap poisoned").clear();
    }
}

/// Serializable record of one completed (or failed) engine execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionTrace {
    pub execution_id: ExecutionId,
    pub operation: String,
    pub profile: String,
    pub principal: PrincipalId,
    pub principal_display: Option<String>,
    pub tenant: Option<TenantId>,
    pub tenant_slug: Option<String>,
    pub plane: String,
    pub scope_paths: Vec<String>,
    pub capability: Option<String>,
    /// `allowed` | `denied`.
    pub authz: String,
    pub idempotent_replay: bool,
    pub queries: Vec<QueryProbe>,
    pub deltas_written: usize,
    pub audit_written: usize,
    pub outbox_queued: usize,
    pub result_bytes: usize,
    pub error: Option<String>,
    /// Captured input — enables replay. Treat as sensitive: traces live under
    /// `storage/framework/traces` and must not leave the environment.
    pub input: serde_json::Value,
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
    pub duration_ms: f64,
}

impl ExecutionTrace {
    pub fn ok(&self) -> bool {
        self.error.is_none()
    }

    pub fn query_count(&self) -> usize {
        self.queries.len()
    }

    pub fn total_query_ms(&self) -> f64 {
        self.queries.iter().map(|q| q.duration_ms).sum()
    }
}

/// Persistence port for execution traces.
pub trait TraceStore: Send + Sync {
    fn save(&self, trace: &ExecutionTrace) -> Result<(), AppError>;
    fn get(&self, execution_id: ExecutionId) -> Result<Option<ExecutionTrace>, AppError>;
    /// Most recent first, newest `limit` entries.
    fn recent(&self, limit: usize) -> Result<Vec<ExecutionTrace>, AppError>;
}

/// In-memory trace store (tests + degraded boot).
#[derive(Debug, Default)]
pub struct InMemoryTraceStore {
    traces: Mutex<Vec<ExecutionTrace>>,
}

impl InMemoryTraceStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl TraceStore for InMemoryTraceStore {
    fn save(&self, trace: &ExecutionTrace) -> Result<(), AppError> {
        self.traces
            .lock()
            .expect("trace store poisoned")
            .push(trace.clone());
        Ok(())
    }

    fn get(&self, execution_id: ExecutionId) -> Result<Option<ExecutionTrace>, AppError> {
        Ok(self
            .traces
            .lock()
            .expect("trace store poisoned")
            .iter()
            .find(|t| t.execution_id == execution_id)
            .cloned())
    }

    fn recent(&self, limit: usize) -> Result<Vec<ExecutionTrace>, AppError> {
        let traces = self.traces.lock().expect("trace store poisoned");
        let mut out: Vec<ExecutionTrace> = traces.iter().rev().take(limit).cloned().collect();
        out.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        Ok(out)
    }
}

/// One-file-per-trace JSON store under a directory
/// (default `storage/framework/traces`).
pub struct FileTraceStore {
    dir: std::path::PathBuf,
}

impl FileTraceStore {
    pub fn new(dir: impl Into<std::path::PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn standard(root: &std::path::Path) -> Self {
        Self::new(root.join("storage/framework/traces"))
    }

    fn path_for(&self, execution_id: ExecutionId) -> std::path::PathBuf {
        self.dir.join(format!("{execution_id}.json"))
    }
}

impl TraceStore for FileTraceStore {
    fn save(&self, trace: &ExecutionTrace) -> Result<(), AppError> {
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| AppError::internal(format!("traces dir: {e}")))?;
        let raw =
            serde_json::to_string_pretty(trace).map_err(|e| AppError::internal(e.to_string()))?;
        std::fs::write(self.path_for(trace.execution_id), raw)
            .map_err(|e| AppError::internal(format!("trace write: {e}")))?;
        Ok(())
    }

    fn get(&self, execution_id: ExecutionId) -> Result<Option<ExecutionTrace>, AppError> {
        match std::fs::read_to_string(self.path_for(execution_id)) {
            Ok(raw) => {
                let trace = serde_json::from_str(&raw)
                    .map_err(|e| AppError::internal(format!("trace corrupt: {e}")))?;
                Ok(Some(trace))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(AppError::internal(format!("trace read: {e}"))),
        }
    }

    fn recent(&self, limit: usize) -> Result<Vec<ExecutionTrace>, AppError> {
        let mut traces = Vec::new();
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(traces),
            Err(e) => return Err(AppError::internal(format!("traces dir: {e}"))),
        };
        for entry in entries.flatten() {
            if entry.path().extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            if let Ok(raw) = std::fs::read_to_string(entry.path()) {
                if let Ok(trace) = serde_json::from_str::<ExecutionTrace>(&raw) {
                    traces.push(trace);
                }
            }
        }
        traces.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        traces.truncate(limit);
        Ok(traces)
    }
}

/// Loop-shaped query pattern: the same query shape executed repeatedly inside
/// a single execution is the classic N+1 signature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NPlusOneFinding {
    pub execution_id: ExecutionId,
    pub operation: String,
    pub shape: String,
    pub occurrences: usize,
}

/// Scan traces for shapes executed at least `threshold` times in one execution.
pub fn detect_n_plus_one(traces: &[ExecutionTrace], threshold: usize) -> Vec<NPlusOneFinding> {
    let threshold = threshold.max(2);
    let mut findings = Vec::new();
    for trace in traces {
        let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
        for probe in &trace.queries {
            *counts.entry(probe.shape.as_str()).or_insert(0) += 1;
        }
        for (shape, occurrences) in counts {
            if occurrences >= threshold {
                findings.push(NPlusOneFinding {
                    execution_id: trace.execution_id,
                    operation: trace.operation.clone(),
                    shape: shape.to_string(),
                    occurrences,
                });
            }
        }
    }
    findings.sort_by(|a, b| b.occurrences.cmp(&a.occurrences));
    findings
}

/// Render a trace in the `inspect execution` format (spec section 44).
pub fn render_trace(trace: &ExecutionTrace) -> String {
    let mut out = String::with_capacity(512);
    out.push_str(&format!("Execution {}\n", trace.execution_id));
    out.push_str("────────────────────────────────────────\n");
    out.push_str(&format!("Operation\n  {}\n", trace.operation));
    out.push_str(&format!(
        "Principal\n  {}{}\n",
        trace.principal,
        trace
            .principal_display
            .as_ref()
            .map(|d| format!(" ({d})"))
            .unwrap_or_default()
    ));
    match trace.tenant {
        Some(tenant) => out.push_str(&format!(
            "Tenant\n  {}{}\n",
            tenant,
            trace
                .tenant_slug
                .as_ref()
                .map(|s| format!(" ({s})"))
                .unwrap_or_default()
        )),
        None => out.push_str("Tenant\n  (platform plane)\n"),
    }
    if trace.scope_paths.is_empty() {
        out.push_str("Scope\n  (none)\n");
    } else {
        out.push_str(&format!("Scope\n  {}\n", trace.scope_paths.join(" / ")));
    }
    out.push_str(&format!(
        "Capability\n  {}\n",
        trace.capability.clone().unwrap_or_else(|| "(none)".into())
    ));
    out.push_str(&format!("Policy\n  {}\n", trace.authz));
    out.push_str(&format!(
        "Database\n  {} queries\n  {:.1}ms\n",
        trace.query_count(),
        trace.total_query_ms()
    ));
    out.push_str(&format!(
        "Transaction\n  {}\n",
        if trace.deltas_written > 0 {
            "committed"
        } else {
            "none"
        }
    ));
    if trace.idempotent_replay {
        out.push_str("Idempotency\n  replayed from store\n");
    }
    match &trace.error {
        Some(err) => out.push_str(&format!("Result\n  FAILED: {err}\n")),
        None => out.push_str(&format!(
            "Delta\n  {} stone(s) written\n",
            trace.deltas_written
        )),
    }
    out.push_str(&format!(
        "Response\n  {} bytes\n  {:.1}ms\n",
        trace.result_bytes, trace.duration_ms
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn trace(op: &str, shapes: &[&str]) -> ExecutionTrace {
        let queries = shapes
            .iter()
            .map(|s| QueryProbe {
                shape: (*s).to_string(),
                label: "pg:test".into(),
                duration_ms: 0.5,
            })
            .collect();
        ExecutionTrace {
            execution_id: ExecutionId::new(),
            operation: op.to_string(),
            profile: "Read".into(),
            principal: PrincipalId::new(),
            principal_display: None,
            tenant: Some(TenantId::new()),
            tenant_slug: Some("t".into()),
            plane: "tenant".into(),
            scope_paths: vec![],
            capability: None,
            authz: "allowed".into(),
            idempotent_replay: false,
            queries,
            deltas_written: 0,
            audit_written: 0,
            outbox_queued: 0,
            result_bytes: 0,
            error: None,
            input: serde_json::json!({}),
            started_at: OffsetDateTime::now_utc(),
            duration_ms: 1.0,
        }
    }

    #[test]
    fn tap_mark_take_isolates_ranges() {
        // The tap is process-global and tests run in parallel, so assert on
        // our own unique shapes rather than absolute buffer contents.
        let before = QueryTap::count();
        QueryTap::record("pg:a", "TAP-TEST-SELECT a", Duration::from_millis(2));
        let mark = QueryTap::mark();
        QueryTap::record("pg:b", "TAP-TEST-SELECT b", Duration::from_millis(3));
        QueryTap::record("pg:c", "TAP-TEST-SELECT c", Duration::from_millis(1));
        let taken = QueryTap::take_since(mark);
        let ours: Vec<&QueryProbe> = taken
            .iter()
            .filter(|p| p.shape.starts_with("TAP-TEST-SELECT"))
            .collect();
        assert_eq!(ours.len(), 2, "both post-mark probes returned in order");
        assert_eq!(ours[0].shape, "TAP-TEST-SELECT b");
        assert_eq!(ours[1].shape, "TAP-TEST-SELECT c");
        assert!(QueryTap::count() >= before + 3, "counter monotonic");
    }

    #[test]
    fn probe_duration_ms_conversion() {
        let mark = QueryTap::mark();
        QueryTap::record("pg:x", "TAP-TEST-DUR", Duration::from_micros(2500));
        let probes = QueryTap::take_since(mark);
        let ours = probes
            .iter()
            .find(|p| p.shape == "TAP-TEST-DUR")
            .expect("our probe present");
        assert!((ours.duration_ms - 2.5).abs() < 1e-9);
    }

    #[test]
    fn memory_store_roundtrip_and_recent_order() {
        let store = InMemoryTraceStore::new();
        let mut t1 = trace("a.one", &[]);
        let t2 = trace("a.two", &["SELECT x"]);
        t1.started_at = t2.started_at - time::Duration::seconds(10);
        store.save(&t1).unwrap();
        store.save(&t2).unwrap();
        assert_eq!(
            store.get(t2.execution_id).unwrap().unwrap().operation,
            "a.two"
        );
        assert!(store.get(ExecutionId::new()).unwrap().is_none());
        let recent = store.recent(5).unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].operation, "a.two", "newest first");
    }

    #[test]
    fn file_store_roundtrip() {
        let dir = std::env::temp_dir().join(format!("keel-traces-{}", uuid::Uuid::now_v7()));
        let store = FileTraceStore::new(&dir);
        let t = trace("fs.op", &["SELECT 1", "SELECT 1", "SELECT 1"]);
        store.save(&t).unwrap();
        let back = store.get(t.execution_id).unwrap().unwrap();
        assert_eq!(back.operation, "fs.op");
        assert_eq!(back.queries.len(), 3);
        let recent = store.recent(10).unwrap();
        assert_eq!(recent.len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn n_plus_one_detection() {
        let clean = trace("clean", &["SELECT a", "SELECT b"]);
        let loopy = trace(
            "loopy",
            &["SELECT item", "SELECT item", "SELECT item", "COMMIT"],
        );
        let findings = detect_n_plus_one(&[clean.clone(), loopy.clone()], 3);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].operation, "loopy");
        assert_eq!(findings[0].shape, "SELECT item");
        assert_eq!(findings[0].occurrences, 3);
        assert!(detect_n_plus_one(&[clean, loopy], 5).is_empty());
    }

    #[test]
    fn render_trace_contains_required_sections() {
        let mut t = trace("invoice.read", &["SELECT invoices"]);
        t.capability = Some("invoice.read".into());
        t.scope_paths = vec!["BU-9".into(), "Location-4".into()];
        t.result_bytes = 128;
        let out = render_trace(&t);
        for section in [
            "Execution ",
            "Operation",
            "Principal",
            "Tenant",
            "Scope",
            "Capability\n  invoice.read",
            "Policy\n  allowed",
            "Database\n  1 queries",
            "Transaction\n  none",
            "Delta\n  0 stone(s) written",
            "Response",
        ] {
            assert!(
                out.contains(section),
                "missing section {section:?} in:\n{out}"
            );
        }
        let failed = ExecutionTrace {
            error: Some("denied".into()),
            ..t
        };
        assert!(render_trace(&failed).contains("FAILED: denied"));
    }

    #[test]
    fn trace_serde_roundtrip() {
        let t = trace("ser.op", &["SELECT 1"]);
        let raw = serde_json::to_string(&t).unwrap();
        let back: ExecutionTrace = serde_json::from_str(&raw).unwrap();
        assert_eq!(back.execution_id, t.execution_id);
        assert_eq!(back.queries.len(), 1);
    }
}
