//! ObservabilityRuntime (Sentinel S5 §5/§6): the application-owned telemetry
//! runtime. One bounded ingest queue, one dedicated worker thread (std-only,
//! so every host can run it), the processing DAG, the policy engine,
//! durability-classed WAL durability, managed exporters with circuit
//! breakers and a global retry budget, the incident control plane poller,
//! the resource monitor with adaptive sampling and deterministic anomaly
//! detectors, exception grouping and cost accounting.
//!
//! Telemetry failure never crashes the application: `emit` is a bounded
//! try-push with counted drops, the worker contains every error, and a
//! stderr fallback covers the emergency path for durable events.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use crate::config::{ExporterConfig, TelemetryConfig};
use crate::groups::ExceptionGroups;
use crate::incident::{IncidentControlPlane, IncidentState};
use kelson_buffer::backpressure::{
    decide as backpressure_decide, shed_level, DropDecision, ShedLevel,
};
use kelson_buffer::wal::{Wal, WalConfig as BufferWalConfig, WalStats};
use kelson_export::{
    BreakerConfig, ConsoleExporter, ConsoleFormat, ExporterHealth, FileExporter,
    FileExporterConfig, HttpExporter, HttpExporterConfig, ManagedExporter, OtlpExporter,
    OtlpExporterConfig, RetryBudget, RetryPolicy, SyslogExporter,
};
use kelson_metrics::{CostLedger, MetricsRegistry};
use kelson_pipeline::graph::PipelineGraph;
use kelson_pipeline::policy::{PolicyEngine, PolicyFacts, PolicyRule};
use kelson_pipeline::processors::{
    Classify, Correlate, Dedupe, Fingerprint, Normalize, Redact, Sample,
};
use kelson_pipeline::router::RouteTable;
use kelson_telemetry::{
    Durability, Event, EventEnvelope, Fields, Level, LifecyclePayload, LogPayload, ServiceIdentity,
    TelemetryCategory, TelemetryPayload, SCHEMA_LIFECYCLE, SCHEMA_LIFECYCLE_VERSION, SCHEMA_LOG,
    SCHEMA_LOG_VERSION,
};
use kelson_tracing::sampler::{escalation, EscalationSignals, RateTier};
use std::sync::mpsc;

/// Runtime-mutable control state (D-76): the ONLY mutable telemetry posture.
/// Mutated exclusively by the incident control plane poller.
#[derive(Debug, Clone, Default)]
pub struct TelemetryControlState {
    pub incident: Option<IncidentState>,
    /// Deeper debug retention requested via incident flags.
    pub debug_retention: bool,
}

/// One event flowing through the worker, optionally WAL-anchored.
struct WorkItem {
    event: Event,
    lsn: Option<u64>,
    retry_count: u32,
}

/// Shared runtime state (worker thread + caller threads).
pub(crate) struct RuntimeInner {
    tier: AtomicU8,
    config: TelemetryConfig,
    service: ServiceIdentity,
    tx: mpsc::SyncSender<WorkItem>,
    capacity: usize,
    depth: Arc<AtomicU64>,
    dropped: Arc<AtomicU64>,
    started: AtomicBool,
    stopping: AtomicBool,
    shed: AtomicU8,
    events_total: AtomicU64,
    errors_total: AtomicU64,
    metrics: Arc<MetricsRegistry>,
    cost: Arc<CostLedger>,
    groups: ExceptionGroups,
    control: RwLock<TelemetryControlState>,
    incident_plane: IncidentControlPlane,
    policy_source: Vec<crate::config::PolicyRuleConfig>,
}

fn shed_to_u8(level: ShedLevel) -> u8 {
    match level {
        ShedLevel::None => 0,
        ShedLevel::DropEphemeral => 1,
        ShedLevel::AggressiveSample => 2,
        ShedLevel::ShedNonCritical => 3,
    }
}

fn u8_to_shed(value: u8) -> ShedLevel {
    match value {
        0 => ShedLevel::None,
        1 => ShedLevel::DropEphemeral,
        2 => ShedLevel::AggressiveSample,
        _ => ShedLevel::ShedNonCritical,
    }
}

/// The runtime handle. Cheap to clone (Arc inside).
pub struct ObservabilityRuntime {
    pub(crate) inner: Arc<RuntimeInner>,
    exporters: Vec<(String, Arc<ManagedExporter>)>,
    routes: RouteTable,
    policy: PolicyEngine,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
    incident_poller: Mutex<Option<std::thread::JoinHandle<()>>>,
    rx_slot: Mutex<Option<mpsc::Receiver<WorkItem>>>,
    wal_dir: Option<PathBuf>,
    shutdown_done: AtomicBool,
}

/// Operator-facing status snapshot (`kelson observe status`).
#[derive(Debug, Clone, serde::Serialize)]
pub struct RuntimeStatus {
    pub service: String,
    pub environment: String,
    pub queue_depth: u64,
    pub queue_capacity: usize,
    pub dropped_total: u64,
    pub events_total: u64,
    pub errors_total: u64,
    pub shed_level: String,
    pub incident: Option<IncidentState>,
    pub sampling_tier: String,
    pub exporters: Vec<ExporterHealth>,
    pub wal: Option<WalStatus>,
}

/// Serializable WAL summary (kelson-buffer's WalStats is storage-side).
#[derive(Debug, Clone, serde::Serialize)]
pub struct WalStatus {
    pub segments: usize,
    pub bytes_on_disk: u64,
    pub watermark: u64,
}

impl From<WalStats> for WalStatus {
    fn from(s: WalStats) -> Self {
        Self {
            segments: s.segments,
            bytes_on_disk: s.bytes_on_disk,
            watermark: s.watermark,
        }
    }
}

impl ObservabilityRuntime {
    /// Construct (inert): no threads, no I/O. Call [`start`](Self::start) to
    /// boot the worker, poller and tracing bridge. `service` identifies this
    /// host in every envelope (`name` / `environment`).
    pub fn new(
        config: TelemetryConfig,
        service: ServiceIdentity,
        plugins: &[Box<dyn kelson_telemetry::TelemetryPlugin>],
    ) -> Result<Arc<Self>, kelson_foundation::AppError> {
        let (tx, rx) = mpsc::sync_channel(config.buffer.capacity.max(1));
        let incident_plane = IncidentControlPlane::new(config.paths.incident_dir.clone());

        // Plugin registry: categories, processors, exporters (S5 §26).
        let mut registry = kelson_telemetry::plugin::TelemetryRegistry::new();
        for plugin in plugins {
            plugin.register(&mut registry);
        }

        // Exporters: config-declared first, then plugin-contributed.
        let budget = RetryBudget::new(config.retry_budget_per_sec);
        let mut exporters: Vec<(String, Arc<ManagedExporter>)> = Vec::new();
        for (name, cfg) in &config.exporters {
            let built = build_exporter(name, cfg, &service.name).ok_or_else(|| {
                kelson_foundation::AppError::validation(format!(
                    "telemetry exporter {name:?}: unsupported kind {:?}",
                    cfg.kind
                ))
            })?;
            exporters.push((
                name.clone(),
                wrap_exporter(name.clone(), built, cfg, budget.clone()),
            ));
        }
        for registered in registry.exporters {
            if exporters.iter().any(|(n, _)| *n == registered.name) {
                continue;
            }
            let default_cfg = ExporterConfig {
                kind: "plugin".into(),
                target: None,
                format: None,
                critical: false,
                partition_by_tenant: false,
                retry: None,
                breaker: None,
            };
            let managed = wrap_exporter(
                registered.name.clone(),
                registered.exporter,
                &default_cfg,
                budget.clone(),
            );
            exporters.push((registered.name.clone(), managed));
        }
        // A runtime always has at least one destination.
        if exporters.is_empty() {
            let console = Arc::new(ConsoleExporter::stdout(console_format(&config.format)))
                as Arc<dyn kelson_telemetry::Exporter>;
            let default_cfg = ExporterConfig {
                kind: "console".into(),
                target: None,
                format: None,
                critical: true,
                partition_by_tenant: false,
                retry: None,
                breaker: None,
            };
            exporters.push((
                "console".into(),
                wrap_exporter("console".into(), console, &default_cfg, budget.clone()),
            ));
        }

        // Route table: default = every exporter; audit partitioned when a
        // dedicated audit exporter exists.
        let all_names: Vec<String> = exporters.iter().map(|(n, _)| n.clone()).collect();
        let mut routes = RouteTable::new(all_names.clone());
        let audit_targets: Vec<String> = exporters
            .iter()
            .filter(|(n, _)| n.contains("audit"))
            .map(|(n, _)| n.clone())
            .collect();
        if !audit_targets.is_empty() {
            routes = routes.route_category(TelemetryCategory::Audit, audit_targets);
        }

        // Policy engine (restricted grammar; evaluation is total — the
        // default route is the terminal action, so misconfiguration cannot
        // wedge the pipeline).
        let mut policy = PolicyEngine::new(all_names);
        for rule_cfg in &config.policy {
            match PolicyRule::parse(rule_cfg.name.clone(), &rule_cfg.when, &rule_cfg.actions) {
                Ok(rule) => policy.rules.push(rule),
                Err(err) => {
                    // A broken rule is skipped, never fatal, and announced.
                    eprintln!(
                        "kelson-observability: policy rule {:?} skipped: {err}",
                        rule_cfg.name
                    );
                }
            }
        }

        let inner = Arc::new(RuntimeInner {
            config: config.clone(),
            service,
            tx,
            capacity: config.buffer.capacity.max(1),
            depth: Arc::new(AtomicU64::new(0)),
            dropped: Arc::new(AtomicU64::new(0)),
            started: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            shed: AtomicU8::new(0),
            events_total: AtomicU64::new(0),
            errors_total: AtomicU64::new(0),
            metrics: Arc::new(MetricsRegistry::new(config.cardinality.max_label_sets)),
            cost: Arc::new(CostLedger::new()),
            groups: ExceptionGroups::default(),
            control: RwLock::new(TelemetryControlState::default()),
            incident_plane,
            policy_source: config.policy.clone(),
            tier: AtomicU8::new(0),
        });

        Ok(Arc::new(Self {
            wal_dir: config.wal.enabled.then(|| config.wal.dir.clone()),
            inner,
            exporters,
            routes,
            policy,
            worker: Mutex::new(None),
            incident_poller: Mutex::new(None),
            rx_slot: Mutex::new(Some(rx)),
            shutdown_done: AtomicBool::new(false),
        }))
    }

    /// Boot: replay the WAL, start the worker + incident poller threads and
    /// install the tracing bridge (best-effort; an existing subscriber keeps
    /// working through the legacy path).
    pub fn start(self: &Arc<Self>) -> Result<(), kelson_foundation::AppError> {
        if self.inner.started.swap(true, Ordering::SeqCst) {
            return Ok(()); // idempotent
        }

        let rx = self
            .rx_slot
            .lock()
            .expect("rx slot")
            .take()
            .ok_or_else(|| {
                kelson_foundation::AppError::internal("telemetry runtime already started")
            })?;
        let worker_runtime = Arc::clone(self);
        let handle = std::thread::Builder::new()
            .name("kelson-telemetry-worker".into())
            .spawn(move || worker_thread(worker_runtime, rx))
            .map_err(|e| {
                kelson_foundation::AppError::internal(format!("telemetry worker spawn: {e}"))
            })?;
        *self.worker.lock().expect("worker lock") = Some(handle);

        let poller_runtime = Arc::clone(self);
        let poller = std::thread::Builder::new()
            .name("kelson-telemetry-incident".into())
            .spawn(move || incident_poll_thread(poller_runtime))
            .map_err(|e| {
                kelson_foundation::AppError::internal(format!("incident poller spawn: {e}"))
            })?;
        *self.incident_poller.lock().expect("poller lock") = Some(poller);

        install_tracing_bridge(Arc::clone(self), &self.inner.config.level);
        register_global(Arc::clone(self));

        self.emit_lifecycle("boot", {
            let mut fields = Fields::new();
            fields.insert(
                "service".into(),
                serde_json::Value::String(self.inner.service.name.clone()),
            );
            fields
        });
        Ok(())
    }

    /// Ingest one event. Never blocks beyond a bounded try-push; overflow is
    /// counted (`telemetry_dropped_total`) and shed per backpressure policy.
    pub fn emit(&self, mut event: Event) {
        let inner = &self.inner;
        // Incident stamp from the control plane (never hand-set by callers).
        {
            let state = inner.control.read().expect("control state poisoned");
            if let Some(incident) = &state.incident {
                event.context.incident_id = Some(incident.id);
            }
        }
        let shed = u8_to_shed(inner.shed.load(Ordering::Relaxed));
        let is_ephemeral = event.durability == Durability::Ephemeral;
        let category = event.payload.category();
        let sampled_category = matches!(
            category,
            TelemetryCategory::Log | TelemetryCategory::Span | TelemetryCategory::Metric
        );
        match backpressure_decide(shed, is_ephemeral, sampled_category) {
            DropDecision::Enqueue => {}
            DropDecision::DroppedEphemeral => {
                self.count_drop("ephemeral_shed");
                return;
            }
            DropDecision::DroppedSampled => {
                self.count_drop("sampled_shed");
                return;
            }
        }
        if event.durability == Durability::Buffered {
            let default = event.payload.default_durability();
            if default == Durability::Durable {
                event.durability = Durability::Durable;
            }
        }
        let is_error = event
            .payload
            .level_hint()
            .map(|l| l >= Level::Error)
            .unwrap_or(false)
            || matches!(category, TelemetryCategory::Exception);
        inner.events_total.fetch_add(1, Ordering::Relaxed);
        if is_error {
            inner.errors_total.fetch_add(1, Ordering::Relaxed);
        }
        inner.metrics.inc(
            "telemetry_events_total",
            labels(&[("category", category.as_str())]),
            1.0,
        );
        inner.cost.record_source(&self.inner.service.name, 1, 0);
        let work = WorkItem {
            event,
            lsn: None,
            retry_count: 0,
        };
        match inner.tx.try_send(work) {
            Ok(()) => {
                inner.depth.fetch_add(1, Ordering::Relaxed);
            }
            Err(mpsc::TrySendError::Full(item)) | Err(mpsc::TrySendError::Disconnected(item)) => {
                inner.dropped.fetch_add(1, Ordering::Relaxed);
                self.count_drop("queue_full");
                if item.event.durability == Durability::Durable {
                    // Emergency path: durable events get a stderr fallback so
                    // a saturated runtime never silently loses audit data.
                    if let Ok(line) = serde_json::to_string(&item.event) {
                        eprintln!("kelson-telemetry EMERGENCY (queue full): {line}");
                    }
                }
            }
        }
    }

    /// Convenience constructor for log events through the bus.
    pub fn emit_log(&self, level: Level, message: impl Into<String>, fields: Fields) {
        let event = EventEnvelope::new(
            SCHEMA_LOG,
            SCHEMA_LOG_VERSION,
            self.inner.service.clone(),
            kelson_foundation::TelemetryContext::root(),
            TelemetryPayload::Log(LogPayload {
                level,
                message: message.into(),
                target: None,
                fields,
                extra: Fields::new(),
            }),
        );
        self.emit(event);
    }

    pub fn info(&self, message: impl Into<String>, fields: Fields) {
        self.emit_log(Level::Info, message, fields);
    }
    pub fn warn(&self, message: impl Into<String>, fields: Fields) {
        self.emit_log(Level::Warn, message, fields);
    }
    pub fn error(&self, message: impl Into<String>, fields: Fields) {
        self.emit_log(Level::Error, message, fields);
    }
    pub fn debug(&self, message: impl Into<String>, fields: Fields) {
        self.emit_log(Level::Debug, message, fields);
    }

    /// Lifecycle event (boot/shutdown/incident transitions, anomalies).
    pub fn emit_lifecycle(&self, kind: &str, fields: Fields) {
        let event = EventEnvelope::new(
            SCHEMA_LIFECYCLE,
            SCHEMA_LIFECYCLE_VERSION,
            self.inner.service.clone(),
            kelson_foundation::TelemetryContext::root(),
            TelemetryPayload::Lifecycle(LifecyclePayload {
                kind: kind.into(),
                fields,
                extra: Fields::new(),
            }),
        );
        self.emit(event);
    }

    fn count_drop(&self, reason: &str) {
        self.inner.metrics.inc(
            "telemetry_dropped_total",
            labels(&[("reason", reason)]),
            1.0,
        );
    }

    // --- accessors ---

    pub fn config(&self) -> &TelemetryConfig {
        &self.inner.config
    }

    pub fn service(&self) -> &ServiceIdentity {
        &self.inner.service
    }

    pub fn metrics(&self) -> Arc<MetricsRegistry> {
        Arc::clone(&self.inner.metrics)
    }

    pub fn cost(&self) -> Arc<CostLedger> {
        Arc::clone(&self.inner.cost)
    }

    pub fn groups(&self) -> &ExceptionGroups {
        &self.inner.groups
    }

    pub fn incident_plane(&self) -> &IncidentControlPlane {
        &self.inner.incident_plane
    }

    pub fn control_state(&self) -> TelemetryControlState {
        self.inner
            .control
            .read()
            .expect("control state poisoned")
            .clone()
    }

    pub fn policy_rules(&self) -> &[crate::config::PolicyRuleConfig] {
        &self.inner.policy_source
    }

    pub fn exporter_health(&self) -> Vec<ExporterHealth> {
        self.exporters.iter().map(|(_, e)| e.health()).collect()
    }

    pub fn wal_stats(&self) -> Option<WalStats> {
        let dir = self.wal_dir.as_ref()?;
        let cfg = BufferWalConfig::new(dir.clone());
        Wal::open(cfg).ok()?.stats().ok()
    }

    /// Operator snapshot for `kelson observe status`.
    pub fn status(&self) -> RuntimeStatus {
        let state = self.control_state();
        RuntimeStatus {
            service: self.inner.service.name.clone(),
            environment: self.inner.service.environment.clone(),
            queue_depth: self.inner.depth.load(Ordering::Relaxed),
            queue_capacity: self.inner.capacity,
            dropped_total: self.inner.dropped.load(Ordering::Relaxed),
            events_total: self.inner.events_total.load(Ordering::Relaxed),
            errors_total: self.inner.errors_total.load(Ordering::Relaxed),
            shed_level: format!("{:?}", u8_to_shed(self.inner.shed.load(Ordering::Relaxed))),
            incident: state.incident,
            sampling_tier: tier_name(self.inner.tier.load(Ordering::Relaxed)).to_string(),
            exporters: self.exporter_health(),
            wal: self.wal_stats().map(WalStatus::from),
        }
    }

    /// Graceful shutdown: stop ingestion, drain the queue, flush the WAL,
    /// write health + cost snapshots, join threads within `deadline`. Safe to
    /// call multiple times. Telemetry shutdown never panics the host.
    pub fn shutdown(&self, deadline: Duration) {
        if self.shutdown_done.swap(true, Ordering::SeqCst) {
            return;
        }
        // Emit the shutdown marker BEFORE signalling the worker so it rides
        // the drain instead of being dropped by a racing exit.
        self.emit_lifecycle("shutdown", Fields::new());
        std::thread::sleep(Duration::from_millis(30));
        self.inner.stopping.store(true, Ordering::SeqCst);
        let started = Instant::now();
        let mut worker = self.worker.lock().expect("worker lock").take();
        let mut poller = self.incident_poller.lock().expect("poller lock").take();
        // Give the worker the bulk of the deadline to drain.
        let worker_deadline = deadline
            .saturating_mul(3)
            .checked_div(4)
            .unwrap_or(deadline);
        if let Some(handle) = worker.take() {
            while !handle.is_finished() && started.elapsed() < worker_deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            // Detach: if still running past the deadline, the process is
            // going away; the WAL already protects durable events.
            drop(handle);
        }
        if let Some(handle) = poller.take() {
            let poll_deadline = deadline.saturating_sub(started.elapsed());
            let poll_started = Instant::now();
            while !handle.is_finished() && poll_started.elapsed() < poll_deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            drop(handle);
        }
        self.write_health_snapshots();
        self.write_cost_snapshot();
    }
}

impl Drop for ObservabilityRuntime {
    fn drop(&mut self) {
        self.shutdown(Duration::from_secs(2));
    }
}

fn labels(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn console_format(raw: &str) -> ConsoleFormat {
    match raw.to_ascii_lowercase().as_str() {
        "pretty" | "text" | "human" => ConsoleFormat::Pretty,
        _ => ConsoleFormat::Json,
    }
}

fn build_exporter(
    name: &str,
    cfg: &ExporterConfig,
    app_name: &str,
) -> Option<Arc<dyn kelson_telemetry::Exporter>> {
    let target = cfg.target.clone().unwrap_or_default();
    match cfg.kind.as_str() {
        "console" => Some(Arc::new(ConsoleExporter::stdout(console_format(
            cfg.format.as_deref().unwrap_or("json"),
        )))),
        "file" => {
            if target.is_empty() {
                eprintln!("kelson-observability: exporter {name:?} (file) has no target; skipped");
                return None;
            }
            if cfg.partition_by_tenant {
                // One file per tenant under the target directory.
                return Some(Arc::new(PartitioningFileExporter::new(
                    FileExporterConfig::new(target),
                )));
            }
            Some(Arc::new(FileExporter::new(FileExporterConfig::new(target))))
        }
        "otlp" => Some(Arc::new(OtlpExporter::new(OtlpExporterConfig {
            endpoint: target,
            timeout: Duration::from_secs(5),
        }))),
        "http" => Some(Arc::new(HttpExporter::new(HttpExporterConfig {
            url: target,
            timeout: Duration::from_secs(5),
            bearer_token: None,
        }))),
        "syslog" => Some(Arc::new(SyslogExporter::new(target, app_name.to_string()))),
        _ => None,
    }
}

fn wrap_exporter(
    name: String,
    exporter: Arc<dyn kelson_telemetry::Exporter>,
    cfg: &ExporterConfig,
    budget: Arc<RetryBudget>,
) -> Arc<ManagedExporter> {
    let breaker = match &cfg.breaker {
        Some(b) => BreakerConfig {
            failure_threshold: if b.failure_threshold == 0 {
                3
            } else {
                b.failure_threshold
            },
            cooldown: Duration::from_millis(if b.cooldown_ms == 0 {
                15_000
            } else {
                b.cooldown_ms
            }),
        },
        None => BreakerConfig::default(),
    };
    let retry = match &cfg.retry {
        Some(r) => RetryPolicy {
            max_attempts: if r.max_attempts == 0 {
                3
            } else {
                r.max_attempts
            },
            initial_backoff: Duration::from_millis(if r.initial_backoff_ms == 0 {
                100
            } else {
                r.initial_backoff_ms
            }),
            max_backoff: Duration::from_millis(if r.max_backoff_ms == 0 {
                2_000
            } else {
                r.max_backoff_ms
            }),
            jitter: r.jitter,
        },
        None => RetryPolicy {
            max_attempts: 3,
            initial_backoff: Duration::from_millis(50),
            max_backoff: Duration::from_millis(1_000),
            jitter: true,
        },
    };
    let critical = cfg.critical || matches!(cfg.kind.as_str(), "console");
    ManagedExporter::new(name, exporter, breaker, retry, budget, critical)
}

/// File exporter wrapper partitioning output per tenant (subdirectory named
/// after the event's tenant id; context-less events go to `default`).
struct PartitioningFileExporter {
    dir: PathBuf,
    file_name: String,
    max_bytes: u64,
    keep_rotations: u32,
    /// Cached per-tenant exporters: file handles stay open across batches so
    /// drains do not pay create_dir + open costs per event (G18 hammer).
    tenants: Mutex<BTreeMap<String, Arc<FileExporter>>>,
}

impl PartitioningFileExporter {
    fn new(cfg: FileExporterConfig) -> Self {
        let dir = cfg
            .path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        let file_name = cfg
            .path
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_else(|| "app.jsonl".into());
        Self {
            dir,
            file_name,
            max_bytes: cfg.max_bytes,
            keep_rotations: cfg.keep_rotations,
            tenants: Mutex::new(BTreeMap::new()),
        }
    }

    fn exporter_for(&self, tenant: &str) -> Arc<FileExporter> {
        let mut map = self.tenants.lock().expect("tenant exporter map poisoned");
        map.entry(tenant.to_string())
            .or_insert_with(|| {
                let mut cfg = FileExporterConfig::new(self.dir.join(tenant).join(&self.file_name));
                cfg.max_bytes = self.max_bytes;
                cfg.keep_rotations = self.keep_rotations;
                Arc::new(FileExporter::new(cfg))
            })
            .clone()
    }
}

impl kelson_telemetry::Exporter for PartitioningFileExporter {
    fn name(&self) -> &str {
        "file_partitioned"
    }

    fn export(
        &self,
        batch: &[Event],
    ) -> Result<kelson_telemetry::ExportOk, kelson_telemetry::ExportError> {
        let mut by_tenant: BTreeMap<String, Vec<&Event>> = BTreeMap::new();
        for event in batch {
            let tenant = event
                .context
                .tenant_id
                .map(|t| t.to_string())
                .unwrap_or_else(|| "default".into());
            by_tenant.entry(tenant).or_default().push(event);
        }
        let mut total = kelson_telemetry::ExportOk::default();
        for (tenant, events) in &by_tenant {
            let owned: Vec<Event> = events.iter().map(|e| (*e).clone()).collect();
            let exporter = self.exporter_for(tenant);
            let ok = exporter.export(&owned)?;
            total.bytes += ok.bytes;
            total.events += ok.events;
        }
        Ok(total)
    }
}

// ---------------------------------------------------------------------------
// Worker thread
// ---------------------------------------------------------------------------

/// Max times a failed durable batch is retried in-process before we leave it
/// to WAL replay on the next boot.
const MAX_DURABLE_RETRIES: u32 = 5;

fn worker_thread(runtime: Arc<ObservabilityRuntime>, rx: mpsc::Receiver<WorkItem>) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        worker_loop(&runtime, rx);
    }));
    if result.is_err() {
        eprintln!("kelson-observability: telemetry worker panicked; fallback logging only");
        runtime.inner.stopping.store(true, Ordering::SeqCst);
        runtime.write_health_snapshots();
        runtime.write_cost_snapshot();
    }
}

fn worker_loop(runtime: &Arc<ObservabilityRuntime>, rx: mpsc::Receiver<WorkItem>) {
    let config = &runtime.inner.config;

    // Build the processing DAG. Plugin processors run after the built-ins
    // so they can observe normalized, classified events.
    let sample_rates: Vec<(TelemetryCategory, f64)> = config
        .sampling
        .rates
        .iter()
        .filter_map(|(name, rate)| match name.as_str() {
            "log" => Some((TelemetryCategory::Log, *rate)),
            "span" => Some((TelemetryCategory::Span, *rate)),
            "metric" => Some((TelemetryCategory::Metric, *rate)),
            "lifecycle" => Some((TelemetryCategory::Lifecycle, *rate)),
            _ => None,
        })
        .collect();
    let sampler = Sample::new(sample_rates);
    let multiplier_bits = sampler.multiplier_bits.clone();
    let dedupe = Dedupe::new(
        Duration::from_millis(config.buffer.dedupe_window_ms),
        config.buffer.dedupe_max_entries,
    );
    let redact_fields = config.redactions.clone();
    let graph = PipelineGraph::new()
        .push(Box::new(Normalize::default()))
        .push(Box::new(Classify))
        .push(Box::new(Redact::new(redact_fields)))
        .push(Box::new(Fingerprint))
        .push(Box::new(sampler))
        .push(Box::new(dedupe))
        .push(Box::new(Correlate));

    // WAL (durability class: Durable events only).
    let mut wal: Option<Wal> = None;
    if let Some(dir) = &runtime.wal_dir {
        let mut wal_cfg = BufferWalConfig::new(dir.clone());
        wal_cfg.fsync_durable = config.wal.fsync;
        if config.wal.max_segment_bytes > 0 {
            wal_cfg.max_segment_bytes = config.wal.max_segment_bytes;
        }
        match Wal::open(wal_cfg) {
            Ok(w) => wal = Some(w),
            Err(e) => eprintln!(
                "kelson-observability: WAL unavailable ({}); durable events degrade to buffered",
                e
            ),
        }
    }

    // Replay unexported durable events from the previous run (ordering: LSN).
    let mut replay_pending: Vec<WorkItem> = Vec::new();
    if let Some(w) = &wal {
        let since = w.watermark();
        match w.replay(since) {
            Ok((records, skipped)) => {
                if skipped > 0 {
                    eprintln!(
                        "kelson-observability: WAL replay skipped {skipped} corrupted records"
                    );
                }
                for record in records {
                    match serde_json::from_slice::<Event>(&record.bytes) {
                        Ok(event) => replay_pending.push(WorkItem {
                            event,
                            lsn: Some(record.lsn),
                            retry_count: 0,
                        }),
                        Err(_) => {
                            runtime.inner.metrics.inc(
                                "telemetry_replay_invalid_total",
                                BTreeMap::new(),
                                1.0,
                            );
                        }
                    }
                }
            }
            Err(e) => eprintln!("kelson-observability: WAL replay failed: {e}"),
        }
    }
    if !replay_pending.is_empty() {
        runtime.inner.metrics.inc(
            "telemetry_replayed_total",
            BTreeMap::new(),
            replay_pending.len() as f64,
        );
    }

    let mut lsn_counter = lsn_start(wal.as_ref(), &replay_pending);
    let mut last_processed_lsn: u64 = wal.as_ref().map(|w| w.watermark()).unwrap_or(0);
    let mut pending_retry: Vec<WorkItem> = replay_pending;
    let mut last_tick = Instant::now();
    let mut last_error_total = runtime.inner.errors_total.load(Ordering::Relaxed);
    let mut last_events_total = runtime.inner.events_total.load(Ordering::Relaxed);
    let tick = Duration::from_millis(config.tick_ms.max(100));

    loop {
        // 1) Retry previously failed durable work (bounded; WAL is the
        //    backstop beyond MAX_DURABLE_RETRIES).
        if !pending_retry.is_empty() {
            let mut still_pending = Vec::new();
            for mut item in pending_retry.drain(..) {
                item.retry_count += 1;
                if item.retry_count > MAX_DURABLE_RETRIES {
                    // Leave in WAL for next boot; stop hammering exporters.
                    runtime.inner.metrics.inc(
                        "telemetry_durable_parked_total",
                        BTreeMap::new(),
                        1.0,
                    );
                    // Still count as processed so the watermark can advance
                    // past it only when nothing smaller is outstanding — it
                    // stays outstanding, so we keep it parked separately.
                    still_pending.push(item);
                    continue;
                }
                if !process_one(
                    runtime,
                    &graph,
                    &mut item,
                    wal.as_mut(),
                    &mut lsn_counter,
                    &mut last_processed_lsn,
                ) {
                    still_pending.push(item);
                }
            }
            pending_retry = still_pending;
            ack_watermark(wal.as_mut(), &pending_retry, last_processed_lsn);
        }

        // 2) Consume the queue.
        let mut batch: Vec<WorkItem> = Vec::with_capacity(config.buffer.batch_size);
        match rx.recv_timeout(Duration::from_millis(25)) {
            Ok(first) => {
                inner_depth_dec(runtime);
                batch.push(first);
                while batch.len() < config.buffer.batch_size {
                    match rx.recv_timeout(Duration::ZERO) {
                        Ok(item) => {
                            inner_depth_dec(runtime);
                            batch.push(item);
                        }
                        Err(_) => break,
                    }
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
        for mut item in batch {
            if !process_one(
                runtime,
                &graph,
                &mut item,
                wal.as_mut(),
                &mut lsn_counter,
                &mut last_processed_lsn,
            ) {
                pending_retry.push(item);
            }
        }
        if !batch_was_empty_or(&mut pending_retry) {
            ack_watermark(wal.as_mut(), &pending_retry, last_processed_lsn);
        }

        // 3) Periodic monitor tick.
        if last_tick.elapsed() >= tick {
            run_tick(
                runtime,
                wal.as_ref(),
                &multiplier_bits,
                &mut last_error_total,
                &mut last_events_total,
            );
            last_tick = Instant::now();
        }

        // 4) Shutdown: drain the rest, flush, exit.
        if runtime.inner.stopping.load(Ordering::SeqCst) {
            while let Ok(mut item) = rx.recv_timeout(Duration::ZERO) {
                inner_depth_dec(runtime);
                if !process_one(
                    runtime,
                    &graph,
                    &mut item,
                    wal.as_mut(),
                    &mut lsn_counter,
                    &mut last_processed_lsn,
                ) {
                    pending_retry.push(item);
                }
            }
            ack_watermark(wal.as_mut(), &pending_retry, last_processed_lsn);
            if let Some(w) = wal.as_mut() {
                let _ = w.flush();
            }
            runtime.write_health_snapshots();
            runtime.write_cost_snapshot();
            break;
        }
    }
}

fn batch_was_empty_or(_pending: &mut Vec<WorkItem>) -> bool {
    true // watermark recomputation is cheap; do it whenever work happened
}

fn inner_depth_dec(runtime: &Arc<ObservabilityRuntime>) {
    runtime.inner.depth.fetch_sub(1, Ordering::Relaxed);
}

fn lsn_start(wal: Option<&Wal>, replay: &[WorkItem]) -> u64 {
    let watermark = wal.map(|w| w.watermark()).unwrap_or(0);
    let max_replay = replay.iter().filter_map(|i| i.lsn).max().unwrap_or(0);
    watermark.max(max_replay)
}

/// Process one work item through DAG → policy → export. Returns true when
/// the item reached a terminal state (exported or deliberately dropped),
/// false when it must be retried.
fn process_one(
    runtime: &Arc<ObservabilityRuntime>,
    graph: &PipelineGraph,
    item: &mut WorkItem,
    wal: Option<&mut Wal>,
    lsn_counter: &mut u64,
    last_processed_lsn: &mut u64,
) -> bool {
    let event = &mut item.event;

    // WAL anchoring happens AFTER normalize (durability may escalate) and
    // BEFORE export. Replayed items keep their original LSN.
    if event.durability == Durability::Durable && item.lsn.is_none() {
        if let Some(w) = wal {
            *lsn_counter += 1;
            let lsn = *lsn_counter;
            if let Ok(bytes) = serde_json::to_vec(&*event) {
                match w.append(lsn, &bytes, true) {
                    Ok(()) => item.lsn = Some(lsn),
                    Err(e) => {
                        runtime.inner.metrics.inc(
                            "telemetry_wal_errors_total",
                            BTreeMap::new(),
                            1.0,
                        );
                        eprintln!("kelson-observability: WAL append failed: {e}");
                    }
                }
            }
        }
    }

    // Processing DAG.
    if let Err(reason) = graph.process(event) {
        runtime.count_drop(&reason);
        terminal_lsn(item, last_processed_lsn);
        return true;
    }

    // Tenant isolation hard-guard: payload tenant must agree with context.
    if let Err(err) = PolicyEngine::enforce_tenant_consistency(event) {
        runtime
            .inner
            .metrics
            .inc("telemetry_tenant_rejected_total", BTreeMap::new(), 1.0);
        runtime.emit_security("tenant_mismatch", {
            let mut fields = Fields::new();
            fields.insert("detail".into(), serde_json::Value::String(err));
            fields
        });
        terminal_lsn(item, last_processed_lsn);
        return true;
    }

    // Policy evaluation (total: default_route terminates every event).
    let incident_active = runtime
        .inner
        .control
        .read()
        .map(|s| s.incident.is_some())
        .unwrap_or(false);
    let facts = PolicyFacts { incident_active };
    let decision = runtime.policy.evaluate(event, &facts);
    if decision.drop && !decision.retain {
        runtime.count_drop("policy");
        terminal_lsn(item, last_processed_lsn);
        return true;
    }
    if !decision.redact.is_empty() || !decision.hash.is_empty() {
        apply_field_actions(event, &decision.redact, &decision.hash);
    }
    if decision.alert {
        if let Some(fields) = event.payload.fields_mut() {
            fields.insert("alert".into(), serde_json::Value::Bool(true));
        }
    }

    // Exception grouping (post-fingerprint).
    if let TelemetryPayload::Exception(x) = &event.payload {
        runtime.inner.groups.record(
            &x.fingerprint,
            &format!("{}: {}", x.type_name, x.message),
            x.route.as_deref(),
            event.context.tenant_id.map(|t| t.to_string()).as_deref(),
        );
    }

    // Resolve destinations: policy override wins, then the route table.
    let mut destinations = if decision.exporters.is_empty() {
        runtime.routes.resolve(event)
    } else {
        decision.exporters.clone()
    };
    // Backpressure: shed non-critical exporters unless the event is retained.
    let shed = u8_to_shed(runtime.inner.shed.load(Ordering::Relaxed));
    if shed == ShedLevel::ShedNonCritical && !decision.retain {
        let critical_only: Vec<String> = destinations
            .iter()
            .filter(|name| {
                runtime
                    .exporters
                    .iter()
                    .find(|(n, _)| n == *name)
                    .map(|(_, e)| e.is_critical())
                    .unwrap_or(false)
            })
            .cloned()
            .collect();
        if !critical_only.is_empty() {
            destinations = critical_only;
        }
    }

    let is_durable = event.durability == Durability::Durable;
    export_to(runtime, event, &destinations, is_durable)
}

/// Attempt export to every destination. Returns true when the event reached
/// terminal state (all destinations accepted, or no destinations).
fn export_to(
    runtime: &Arc<ObservabilityRuntime>,
    event: &Event,
    destinations: &[String],
    is_durable: bool,
) -> bool {
    if destinations.is_empty() {
        runtime.count_drop("no_route");
        return true;
    }
    let batch = std::slice::from_ref(event);
    let mut all_ok = true;
    for name in destinations {
        let Some((_, exporter)) = runtime.exporters.iter().find(|(n, _)| n == name) else {
            continue; // unknown exporter name in policy/route: skip, never wedge
        };
        match exporter.export(batch) {
            Ok(ok) => {
                runtime
                    .inner
                    .cost
                    .record_export(name, ok.events as u64, ok.bytes as u64);
                runtime.inner.metrics.inc(
                    "telemetry_exported_total",
                    labels(&[("exporter", name)]),
                    ok.events as f64,
                );
            }
            Err(err) => {
                all_ok = false;
                runtime.inner.metrics.inc(
                    "telemetry_export_failures_total",
                    labels(&[("exporter", name)]),
                    1.0,
                );
                if is_durable {
                    let msg = err.to_string().to_ascii_lowercase();
                    if msg.contains("space") || msg.contains("enospc") || msg.contains("disk") {
                        eprintln!("kelson-observability: disk pressure on exporter {name}: {err}");
                    }
                }
            }
        }
    }
    all_ok
}

fn terminal_lsn(item: &WorkItem, last_processed_lsn: &mut u64) {
    if let Some(lsn) = item.lsn {
        *last_processed_lsn = (*last_processed_lsn).max(lsn);
    }
}

fn ack_watermark(wal: Option<&mut Wal>, pending: &[WorkItem], last_processed_lsn: u64) {
    let Some(w) = wal else { return };
    let outstanding_min = pending.iter().filter_map(|i| i.lsn).min();
    let ack_to = match outstanding_min {
        Some(min) => min.saturating_sub(1),
        None => last_processed_lsn,
    };
    if ack_to > w.watermark() {
        if let Err(e) = w.ack(ack_to) {
            eprintln!("kelson-observability: WAL ack failed: {e}");
        }
    }
}

fn apply_field_actions(event: &mut Event, redact: &[String], hash: &[String]) {
    let Some(fields) = event.payload.fields_mut() else {
        return;
    };
    for key in redact {
        if fields.contains_key(key) {
            fields.insert(key.clone(), serde_json::Value::String("[REDACTED]".into()));
        }
    }
    for key in hash {
        if let Some(value) = fields.get(key).cloned() {
            let digest = kelson_telemetry::fingerprint::normalize_component(&value.to_string());
            let mut bytes = [0u8; 8];
            let src = digest.as_bytes();
            let n = src.len().min(8);
            bytes[..n].copy_from_slice(&src[..n]);
            let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
            fields.insert(
                key.clone(),
                serde_json::Value::String(format!("hash:{hex}")),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Resource monitor + adaptive sampling + anomaly detection (deterministic)
// ---------------------------------------------------------------------------

fn run_tick(
    runtime: &Arc<ObservabilityRuntime>,
    wal: Option<&Wal>,
    multiplier_bits: &Arc<AtomicU64>,
    last_error_total: &mut u64,
    last_events_total: &mut u64,
) {
    let inner = &runtime.inner;
    let metrics = &inner.metrics;

    // --- signals ---
    let events_total = inner.events_total.load(Ordering::Relaxed);
    let errors_total = inner.errors_total.load(Ordering::Relaxed);
    let events_delta = events_total.saturating_sub(*last_events_total);
    let errors_delta = errors_total.saturating_sub(*last_error_total);
    *last_events_total = events_total;
    *last_error_total = errors_total;
    let error_rate = if events_delta > 0 {
        errors_delta as f64 / events_delta as f64
    } else {
        0.0
    };
    let queue_fill = inner.depth.load(Ordering::Relaxed) as f64 / inner.capacity.max(1) as f64;
    let incident_active = inner
        .control
        .read()
        .map(|s| s.incident.is_some())
        .unwrap_or(false);

    // Disk/export health: any critical exporter with an open breaker counts
    // as disk/egress pressure for the shed ladder.
    let disk_healthy = runtime
        .exporters
        .iter()
        .filter(|(_, e)| e.is_critical())
        .all(|(_, e)| e.breaker_state() != kelson_export::BreakerState::Open);

    // --- backpressure ladder ---
    let shed = shed_level(queue_fill, disk_healthy);
    inner.shed.store(shed_to_u8(shed), Ordering::Relaxed);
    metrics.set_gauge(
        "telemetry_queue_depth",
        BTreeMap::new(),
        inner.depth.load(Ordering::Relaxed) as f64,
    );
    metrics.set_gauge(
        "telemetry_shed_level",
        BTreeMap::new(),
        shed_to_u8(shed) as f64,
    );
    metrics.set_gauge("telemetry_error_rate", BTreeMap::new(), error_rate);

    // --- adaptive sampling (deterministic ladder) ---
    let baseline = baseline_p95(runtime);
    let p95 = histogram_p95(&metrics.snapshot(), "http_request_duration_ms").unwrap_or(0.0);
    let signals = EscalationSignals {
        error_rate,
        p95_latency_ms: p95,
        baseline_latency_ms: baseline,
        queue_fill,
        incident: incident_active,
    };
    let tier = escalation(&signals);
    inner.tier.store(tier_code(tier), Ordering::Relaxed);
    let base_rate = inner
        .config
        .sampling
        .rates
        .values()
        .cloned()
        .fold(1.0f64, f64::min)
        .max(0.0001);
    let multiplier = if incident_active {
        1.0 / base_rate // incident mode: 100% for sampled classes
    } else {
        tier.multiplier(base_rate)
    };
    multiplier_bits.store(
        f64::to_bits(multiplier.clamp(0.0, 1.0 / base_rate)),
        Ordering::Relaxed,
    );
    metrics.set_gauge("telemetry_sampling_multiplier", BTreeMap::new(), multiplier);

    // --- deterministic anomaly detectors (threshold, no ML) ---
    let mut anomalies: Vec<(&str, Vec<(&str, serde_json::Value)>)> = Vec::new();
    if error_rate >= kelson_tracing::ERROR_RATE_FULL {
        anomalies.push((
            "error_rate_spike",
            vec![("error_rate", serde_json::json!(error_rate))],
        ));
    } else if error_rate >= kelson_tracing::ERROR_RATE_ELEVATED && events_delta >= 20 {
        anomalies.push((
            "error_rate_elevated",
            vec![("error_rate", serde_json::json!(error_rate))],
        ));
    }
    if baseline > 0.0 && p95 >= baseline * kelson_tracing::LATENCY_RATIO_FULL {
        anomalies.push((
            "latency_spike",
            vec![
                ("p95_ms", serde_json::json!(p95)),
                ("baseline_ms", serde_json::json!(baseline)),
            ],
        ));
    }
    if queue_fill >= kelson_tracing::QUEUE_FILL_FULL {
        anomalies.push((
            "queue_backlog",
            vec![("queue_fill", serde_json::json!(queue_fill))],
        ));
    }
    if !disk_healthy {
        anomalies.push((
            "export_failure",
            vec![(
                "detail",
                serde_json::json!("critical exporter breaker open"),
            )],
        ));
    }
    for (kind, fields) in anomalies {
        let mut field_map = Fields::new();
        for (k, v) in fields {
            field_map.insert(k.into(), v);
        }
        runtime.emit_anomaly(kind, field_map);
    }

    // --- health + cost snapshots (staggered) ---
    runtime.write_health_snapshots();
    if events_total % 5000 < events_delta.max(1) || !incident_active && events_total == 0 {
        runtime.write_cost_snapshot();
    }
    let _ = wal; // WAL stats are read on demand by status()/CLI
}

fn tier_code(tier: RateTier) -> u8 {
    match tier {
        RateTier::Normal => 0,
        RateTier::Elevated => 1,
        RateTier::Full => 2,
    }
}

fn tier_name(code: u8) -> &'static str {
    match code {
        0 => "normal",
        1 => "elevated",
        _ => "full",
    }
}

fn baseline_p95(runtime: &Arc<ObservabilityRuntime>) -> f64 {
    static BASELINE: OnceLock<f64> = OnceLock::new();
    *BASELINE.get_or_init(|| {
        histogram_p95(
            &runtime.inner.metrics.snapshot(),
            "http_request_duration_ms",
        )
        .unwrap_or(100.0)
    })
}

/// p95 from histogram buckets (linear interpolation over cumulative counts).
fn histogram_p95(snapshot: &kelson_metrics::MetricsSnapshot, name: &str) -> Option<f64> {
    let (_, state) = snapshot.histograms.iter().find(|(n, _)| n == name)?;
    if state.count == 0 {
        return None;
    }
    let target = (state.count as f64) * 0.95;
    let mut prev_bound = 0.0f64;
    let mut prev_count = 0u64;
    for (bound, count) in &state.buckets {
        if *count as f64 >= target {
            let span = (count - prev_count).max(1) as f64;
            let into = target - prev_count as f64;
            return Some(prev_bound + (bound - prev_bound) * (into / span).clamp(0.0, 1.0));
        }
        prev_bound = *bound;
        prev_count = *count;
    }
    state.buckets.last().map(|(bound, _)| *bound)
}

// ---------------------------------------------------------------------------
// Incident control-plane poller (2 s cadence; file is the source of truth)
// ---------------------------------------------------------------------------

fn incident_poll_thread(runtime: Arc<ObservabilityRuntime>) {
    let mut current: Option<IncidentState> = None;
    loop {
        std::thread::sleep(Duration::from_secs(2));
        if runtime.inner.stopping.load(Ordering::SeqCst) {
            break;
        }
        let next = runtime.inner.incident_plane.read();
        let changed = match (&current, &next) {
            (None, None) => false,
            (Some(a), Some(b)) => a.id != b.id,
            _ => true,
        };
        if changed {
            match &next {
                Some(state) => {
                    let mut fields = Fields::new();
                    fields.insert(
                        "incident_id".into(),
                        serde_json::json!(state.id.to_string()),
                    );
                    runtime.emit_lifecycle("incident_start", fields);
                    runtime.inner.metrics.set_gauge(
                        "telemetry_incident_active",
                        BTreeMap::new(),
                        1.0,
                    );
                }
                None => {
                    runtime.emit_lifecycle("incident_stop", Fields::new());
                    runtime.inner.metrics.set_gauge(
                        "telemetry_incident_active",
                        BTreeMap::new(),
                        0.0,
                    );
                }
            }
            let mut control = runtime.inner.control.write().expect("control poisoned");
            control.debug_retention = next
                .as_ref()
                .map(|s| s.flags.get("debug_retention") == Some(&"true".to_string()))
                .unwrap_or(false);
            control.incident = next.clone();
            current = next;
        }
    }
}

// ---------------------------------------------------------------------------
// Tracing bridge + global handle
// ---------------------------------------------------------------------------

fn install_tracing_bridge(runtime: Arc<ObservabilityRuntime>, level: &str) {
    use tracing_subscriber::prelude::*;
    let filter = tracing_subscriber::EnvFilter::try_new(level)
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let bridge = crate::bridge::TelemetryBridge::new(runtime);
    let subscriber = tracing_subscriber::registry().with(filter).with(bridge);
    if subscriber.try_init().is_err() {
        eprintln!(
            "kelson-observability: global tracing subscriber already installed; \
             tracing events use the existing path, direct emits still flow"
        );
    }
}

static GLOBAL_RUNTIME: OnceLock<Arc<ObservabilityRuntime>> = OnceLock::new();

fn register_global(runtime: Arc<ObservabilityRuntime>) {
    let _ = GLOBAL_RUNTIME.set(runtime);
}

/// Process-wide runtime handle, when one has been started (providers set it;
/// tests and short-lived hosts may not).
pub fn global_runtime() -> Option<Arc<ObservabilityRuntime>> {
    GLOBAL_RUNTIME.get().cloned()
}

/// Emit through the global runtime when present (no-op otherwise).
pub fn emit_global(event: Event) {
    if let Some(runtime) = GLOBAL_RUNTIME.get() {
        runtime.emit(event);
    }
}

/// Global log emit (developer-macro target): stderr fallback when no runtime
/// has been started — telemetry messages are never silently lost.
pub fn emit_log_global(level: Level, message: impl Into<String>, fields: Fields) {
    match GLOBAL_RUNTIME.get() {
        Some(runtime) => runtime.emit_log(level, message, fields),
        None => {
            let msg = message.into();
            eprintln!(
                "[kelson-telemetry {}] {msg}",
                level.as_str().to_ascii_uppercase()
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Snapshots + misc emitters
// ---------------------------------------------------------------------------

impl ObservabilityRuntime {
    fn emit_security(&self, kind: &str, fields: Fields) {
        let event = EventEnvelope::new(
            kelson_telemetry::SCHEMA_SECURITY,
            kelson_telemetry::SCHEMA_SECURITY_VERSION,
            self.inner.service.clone(),
            kelson_foundation::TelemetryContext::root(),
            TelemetryPayload::Security(kelson_telemetry::SecurityPayload {
                kind: kind.into(),
                fields,
                extra: Fields::new(),
            }),
        );
        self.emit(event);
    }

    fn emit_anomaly(&self, kind: &str, fields: Fields) {
        let mut fields = fields;
        fields.insert("detector".into(), serde_json::json!(kind));
        let event = EventEnvelope::new(
            "telemetry.anomaly.detected",
            SCHEMA_LIFECYCLE_VERSION,
            self.inner.service.clone(),
            kelson_foundation::TelemetryContext::root(),
            TelemetryPayload::Lifecycle(LifecyclePayload {
                kind: format!("anomaly.{kind}"),
                fields,
                extra: Fields::new(),
            }),
        );
        self.emit(event);
    }

    /// Per-exporter health files under `storage/framework/telemetry/exporters/`
    /// (consumed by `kelson observe exporters` / `health`).
    pub(crate) fn write_health_snapshots(&self) {
        let dir = &self.inner.config.paths.health_dir;
        if std::fs::create_dir_all(dir).is_err() {
            return;
        }
        for (name, exporter) in &self.exporters {
            let health = exporter.health();
            let safe = name.replace(['/', '\\', ':'], "_");
            let path = dir.join(format!("{safe}.json"));
            if let Ok(raw) = serde_json::to_string_pretty(&health) {
                let _ = std::fs::write(path, raw);
            }
        }
    }

    /// Cost rollup (`storage/framework/telemetry/cost.json`, S5 §21).
    pub(crate) fn write_cost_snapshot(&self) {
        let path = &self.inner.config.paths.cost_file;
        if let Some(parent) = path.parent() {
            if std::fs::create_dir_all(parent).is_err() {
                return;
            }
        }
        let snapshot = self.inner.cost.snapshot();
        if let Ok(raw) = serde_json::to_string_pretty(&snapshot) {
            let _ = std::fs::write(path, raw);
        }
    }
}
