//! End-to-end runtime tests (Sentinel S5 §31 reliability/failure-injection):
//! real worker thread, real file exporter, real WAL on a temp directory.
//! Deterministic by design: shutdown drains the queue, so assertions read
//! the exported file after `shutdown()` returns.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use kelson_telemetry::{
    ExportError, ExportOk, Exporter, ServiceIdentity, TelemetryPlugin, TelemetryRegistry,
};

use crate::config::{ExporterConfig, PolicyRuleConfig, TelemetryConfig};
use crate::runtime::ObservabilityRuntime;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "kelson-runtime-test-{name}-{}",
        uuid::Uuid::now_v7()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn base_config(base: &Path, log_file: &Path) -> TelemetryConfig {
    let mut cfg = TelemetryConfig::default();
    cfg.wal.dir = base.join("wal");
    cfg.paths.incident_dir = base.join("control");
    cfg.paths.cost_file = base.join("cost.json");
    cfg.paths.health_dir = base.join("exporters");
    cfg.tick_ms = 100;
    cfg.exporters.insert(
        "file".into(),
        ExporterConfig {
            kind: "file".into(),
            target: Some(log_file.to_string_lossy().into_owned()),
            format: None,
            critical: true,
            partition_by_tenant: false,
            retry: None,
            breaker: None,
        },
    );
    cfg
}

fn read_lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|s| s.to_string())
        .collect()
}

#[test]
fn end_to_end_export_redacts_secrets() {
    let base = temp_dir("e2e");
    let log_file = base.join("app.jsonl");
    let cfg = base_config(&base, &log_file);
    let runtime =
        ObservabilityRuntime::new(cfg, ServiceIdentity::new("test-host", "test"), &[]).unwrap();
    runtime.start().unwrap();

    runtime.info(
        "user created",
        crate::fields!("user" => "alice", "password" => "hunter2"),
    );
    runtime.shutdown(Duration::from_secs(2));

    let lines = read_lines(&log_file);
    let created = lines
        .iter()
        .find(|l| l.contains("user created"))
        .expect("event exported");
    assert!(created.contains("alice"), "got: {created}");
    assert!(!created.contains("hunter2"), "secret leaked: {created}");
    assert!(created.contains("[REDACTED]"), "got: {created}");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn policy_drop_rule_stops_events() {
    let base = temp_dir("policy");
    let log_file = base.join("app.jsonl");
    let mut cfg = base_config(&base, &log_file);
    cfg.policy.push(PolicyRuleConfig {
        name: "drop-debug".into(),
        when: "level == \"debug\"".into(),
        actions: vec!["drop".into()],
    });
    let runtime = ObservabilityRuntime::new(cfg, ServiceIdentity::new("t", "test"), &[]).unwrap();
    runtime.start().unwrap();

    runtime.debug("hidden chatter", crate::fields!());
    runtime.info("visible", crate::fields!());
    runtime.shutdown(Duration::from_secs(2));

    let lines = read_lines(&log_file);
    assert!(lines.iter().any(|l| l.contains("visible")));
    assert!(!lines.iter().any(|l| l.contains("hidden chatter")));
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn queue_overflow_counts_drops_and_protects_queue() {
    let base = temp_dir("overflow");
    let log_file = base.join("app.jsonl");
    let mut cfg = base_config(&base, &log_file);
    cfg.buffer.capacity = 4;
    let runtime = ObservabilityRuntime::new(cfg, ServiceIdentity::new("t", "test"), &[]).unwrap();
    // Deliberately not started: no worker drains, so the bounded queue fills.
    for i in 0..32 {
        runtime.info(format!("msg {i}"), crate::fields!());
    }
    let status = runtime.status();
    assert!(
        status.dropped_total >= 16,
        "drops counted: {:?}",
        status.dropped_total
    );
    let _ = std::fs::remove_dir_all(&base);
}

/// Fault-injecting exporter: fails until `switch_to_success`, then succeeds.
struct FlakyExporter {
    failures_until: u64,
    attempts: AtomicU64,
}

impl Exporter for FlakyExporter {
    fn name(&self) -> &str {
        "flaky"
    }
    fn export(&self, batch: &[kelson_telemetry::Event]) -> Result<ExportOk, ExportError> {
        let attempt = self.attempts.fetch_add(1, Ordering::Relaxed);
        if attempt < self.failures_until {
            Err(ExportError::Transport("injected failure".into()))
        } else {
            Ok(ExportOk {
                bytes: batch.len(),
                events: batch.len(),
            })
        }
    }
}

struct FlakyPlugin {
    failures_until: u64,
}

impl TelemetryPlugin for FlakyPlugin {
    fn name(&self) -> &str {
        "flaky-plugin"
    }
    fn register(&self, registry: &mut TelemetryRegistry) {
        registry.register_exporter(
            "flaky",
            Arc::new(FlakyExporter {
                failures_until: self.failures_until,
                attempts: AtomicU64::new(0),
            }),
        );
    }
}

#[test]
fn wal_replay_recovers_durable_events_after_outage() {
    let base = temp_dir("wal-replay");
    let log_file = base.join("app.jsonl");

    // Phase 1: exporter always fails -> durable event must stay in the WAL.
    let mut cfg = base_config(&base, &log_file);
    cfg.exporters.remove("file"); // only the (failing) plugin exporter
    let runtime = ObservabilityRuntime::new(
        cfg,
        ServiceIdentity::new("t", "test"),
        &[Box::new(FlakyPlugin {
            failures_until: u64::MAX,
        })],
    )
    .unwrap();
    runtime.start().unwrap();
    // Error level => Durable by default durability rules.
    runtime.error("payment gateway down", crate::fields!("order" => "o-42"));
    // Give the worker a moment to attempt + park the durable event.
    std::thread::sleep(Duration::from_millis(400));
    runtime.shutdown(Duration::from_secs(2));
    assert!(
        !read_lines(&log_file)
            .iter()
            .any(|l| l.contains("payment gateway")),
        "phase 1 must not export"
    );
    let watermark_after_phase1 = runtime.wal_stats().map(|s| s.watermark);
    assert_eq!(watermark_after_phase1, Some(0), "nothing acked yet");

    // Phase 2: same WAL dir, healthy file exporter -> replay delivers it.
    let cfg_ok = base_config(&base, &log_file);
    let runtime2 =
        ObservabilityRuntime::new(cfg_ok, ServiceIdentity::new("t", "test"), &[]).unwrap();
    runtime2.start().unwrap();
    std::thread::sleep(Duration::from_millis(400));
    runtime2.shutdown(Duration::from_secs(2));

    let lines = read_lines(&log_file);
    assert!(
        lines.iter().any(|l| l.contains("payment gateway down")),
        "replayed event exported: {lines:?}"
    );
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn incident_mode_stamps_events_with_incident_id() {
    let base = temp_dir("incident");
    let log_file = base.join("app.jsonl");
    let cfg = base_config(&base, &log_file);
    let runtime = ObservabilityRuntime::new(cfg, ServiceIdentity::new("t", "test"), &[]).unwrap();
    runtime.start().unwrap();

    let incident = runtime
        .incident_plane()
        .start(Some("test-operator"))
        .unwrap();
    // Poller cadence is 2 s; wait for it to apply.
    std::thread::sleep(Duration::from_millis(2400));
    assert_eq!(
        runtime.control_state().incident.as_ref().map(|s| s.id),
        Some(incident.id)
    );

    runtime.info("during incident", crate::fields!());
    runtime.shutdown(Duration::from_secs(2));

    let line = read_lines(&log_file)
        .into_iter()
        .find(|l| l.contains("during incident"))
        .expect("event exported");
    assert!(
        line.contains(&incident.id.to_string()),
        "incident stamped: {line}"
    );
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn tenant_partitioned_file_exporter_separates_output() {
    let base = temp_dir("partition");
    let log_dir = base.join("tenants/app.jsonl");
    let mut cfg = base_config(&base, &log_dir);
    cfg.exporters.get_mut("file").unwrap().partition_by_tenant = true;

    let runtime = ObservabilityRuntime::new(cfg, ServiceIdentity::new("t", "test"), &[]).unwrap();
    runtime.start().unwrap();

    // Emit through the bus with explicit tenant contexts.
    let mk = |tenant: &str, msg: &str| {
        kelson_telemetry::EventEnvelope::new(
            kelson_telemetry::SCHEMA_LOG,
            kelson_telemetry::SCHEMA_LOG_VERSION,
            ServiceIdentity::new("t", "test"),
            kelson_foundation::TelemetryContext::root()
                .with_tenant(kelson_foundation::TenantId::from_uuid(uuid::Uuid::now_v7())),
            kelson_telemetry::TelemetryPayload::Log(kelson_telemetry::LogPayload {
                level: kelson_telemetry::Level::Info,
                message: msg.to_string(),
                target: None,
                fields: crate::fields!("tenant_hint" => tenant),
                extra: kelson_telemetry::Fields::new(),
            }),
        )
    };
    runtime.emit(mk("a", "event for tenant a"));
    runtime.emit(mk("b", "event for tenant b"));
    runtime.shutdown(Duration::from_secs(2));

    // Tenants get random uuids here, so assert two tenant partition dirs
    // exist (the tenant-less lifecycle events land in `default`).
    let tenants_root = base.join("tenants");
    let dirs: Vec<PathBuf> = std::fs::read_dir(&tenants_root)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .filter(|p| p.file_name().unwrap_or_default() != "default")
        .collect();
    assert_eq!(dirs.len(), 2, "two tenant partitions: {dirs:?}");
    let all: Vec<String> = dirs
        .iter()
        .flat_map(|d| read_lines(&d.join("app.jsonl")))
        .collect();
    assert!(all.iter().any(|l| l.contains("event for tenant a")));
    assert!(all.iter().any(|l| l.contains("event for tenant b")));
    let _ = std::fs::remove_dir_all(&base);
}

// --------------------------------------------------------------- G18 suite
// Failure-injection + isolation gates (STAGE-SENTINEL-SPEC §21 / D-73).

/// G18 isolation hammer: concurrent producers across 4 tenants must not
/// leak events into each other's partition. Tenant ids come from context
/// only; the partitioned file exporter materializes the separation on
/// disk so the assertion is physical, not logical.
#[test]
fn g18_isolation_hammer_no_cross_tenant_leakage() {
    let base = temp_dir("g18-hammer");
    let log_file = base.join("tenants/app.jsonl");
    let mut cfg = base_config(&base, &log_file);
    cfg.exporters.get_mut("file").unwrap().partition_by_tenant = true;
    let runtime = ObservabilityRuntime::new(cfg, ServiceIdentity::new("g18", "test"), &[]).unwrap();
    runtime.start().unwrap();

    let tenants: Vec<kelson_foundation::TenantId> =
        (0..4).map(|_| kelson_foundation::TenantId::new()).collect();
    let per_tenant = 150usize;

    std::thread::scope(|scope| {
        for (idx, tenant) in tenants.iter().enumerate() {
            let tenant = *tenant;
            let runtime = Arc::clone(&runtime);
            scope.spawn(move || {
                for i in 0..per_tenant {
                    // Unique messages: dedupe keys on tenant|category|message
                    // for info-and-below, so nothing is legitimately suppressed.
                    let msg = format!("tenant-{idx} event {i} {}", uuid::Uuid::now_v7());
                    let event = kelson_telemetry::EventEnvelope::new(
                        kelson_telemetry::SCHEMA_LOG,
                        kelson_telemetry::SCHEMA_LOG_VERSION,
                        ServiceIdentity::new("g18", "test"),
                        kelson_foundation::TelemetryContext::root().with_tenant(tenant),
                        kelson_telemetry::TelemetryPayload::Log(kelson_telemetry::LogPayload {
                            level: kelson_telemetry::Level::Info,
                            message: msg,
                            target: None,
                            fields: crate::fields!(),
                            extra: kelson_telemetry::Fields::new(),
                        }),
                    );
                    runtime.emit(event);
                }
            });
        }
    });
    runtime.shutdown(Duration::from_secs(5));

    let status = runtime.status();
    assert_eq!(
        status.dropped_total, 0,
        "R3 loss accounting: the hammer must not drop events at ingest"
    );

    // Every partition file must contain events from AT MOST one tenant, and
    // every tenant's events must all have landed somewhere (no loss).
    let tenants_root = base.join("tenants");
    let mut counts = vec![0usize; tenants.len()];
    let dirs: Vec<PathBuf> = std::fs::read_dir(&tenants_root)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    for dir in &dirs {
        let lines = read_lines(&dir.join("app.jsonl"));
        let mut owners: Vec<usize> = Vec::new();
        for line in &lines {
            for (idx, count) in counts.iter_mut().enumerate() {
                if line.contains(&format!("tenant-{idx} event")) {
                    *count += 1;
                    if !owners.contains(&idx) {
                        owners.push(idx);
                    }
                }
            }
        }
        assert!(
            owners.len() <= 1,
            "partition {dir:?} mixes tenants {owners:?} — context isolation broken"
        );
    }
    for (idx, count) in counts.iter().enumerate() {
        assert_eq!(
            *count, per_tenant,
            "tenant {idx} lost events: {count}/{per_tenant}"
        );
    }
    let _ = std::fs::remove_dir_all(&base);
}

/// Exporter that sleeps on every batch: simulates a slow downstream.
struct SlowExporter {
    sleep_ms: u64,
}

impl Exporter for SlowExporter {
    fn name(&self) -> &str {
        "slow"
    }
    fn export(&self, batch: &[kelson_telemetry::Event]) -> Result<ExportOk, ExportError> {
        std::thread::sleep(Duration::from_millis(self.sleep_ms));
        Ok(ExportOk {
            bytes: batch.len(),
            events: batch.len(),
        })
    }
}

struct SlowPlugin {
    sleep_ms: u64,
}

impl TelemetryPlugin for SlowPlugin {
    fn name(&self) -> &str {
        "slow-plugin"
    }
    fn register(&self, registry: &mut TelemetryRegistry) {
        registry.register_exporter(
            "slow",
            Arc::new(SlowExporter {
                sleep_ms: self.sleep_ms,
            }),
        );
    }
}

/// G18 shutdown-during-drain: against a downstream that cannot keep up,
/// the shutdown deadline must still bound teardown (no hang) — pending
/// events are shed rather than blocking the host's exit path.
#[test]
fn g18_shutdown_drain_deadline_is_bounded() {
    let base = temp_dir("g18-drain");
    let mut cfg = base_config(&base, &base.join("app.jsonl"));
    cfg.exporters.remove("file"); // only the slow plugin exporter
    cfg.buffer.capacity = 64;
    let runtime = ObservabilityRuntime::new(
        cfg,
        ServiceIdentity::new("g18", "test"),
        &[Box::new(SlowPlugin { sleep_ms: 150 })],
    )
    .unwrap();
    runtime.start().unwrap();

    for i in 0..512 {
        runtime.info(format!("flood {i}"), crate::fields!());
    }
    let start = std::time::Instant::now();
    runtime.shutdown(Duration::from_millis(400));
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_secs(3),
        "shutdown exceeded its deadline: {elapsed:?}"
    );
    let _ = std::fs::remove_dir_all(&base);
}
