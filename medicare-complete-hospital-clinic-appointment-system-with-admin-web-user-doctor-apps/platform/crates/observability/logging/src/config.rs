//! Telemetry configuration (Sentinel S5 §13): the `[telemetry]` section of
//! the platform config, deserialized defensively (every field optional with
//! safe defaults), plus the env overlay (`KEEL_TELEMETRY_*`). The base config
//! freezes at boot (STAGE-0 §10); only `TelemetryControlState` mutates at
//! runtime, via the incident control plane.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

fn default_level() -> String {
    "info".into()
}

fn default_format() -> String {
    "json".into()
}

fn default_capacity() -> usize {
    8192
}

fn default_batch() -> usize {
    256
}

fn default_dedupe_window_ms() -> u64 {
    10_000
}

fn default_dedupe_entries() -> u64 {
    4096
}

fn default_max_label_sets() -> u64 {
    1024
}

fn default_retry_budget() -> u64 {
    20
}

fn default_tick_ms() -> u64 {
    1000
}

fn default_wal_dir() -> PathBuf {
    PathBuf::from("storage/framework/telemetry/wal")
}

fn default_incident_dir() -> PathBuf {
    PathBuf::from("storage/framework/telemetry/control")
}

fn default_cost_file() -> PathBuf {
    PathBuf::from("storage/framework/telemetry/cost.json")
}

fn default_health_dir() -> PathBuf {
    PathBuf::from("storage/framework/telemetry/exporters")
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SamplingConfig {
    /// Base keep-rate per category name (`log`, `span`, `metric`, ...).
    /// Missing categories keep 1.0; errors/audit/security are never sampled
    /// away (enforced by the Sample processor, not config).
    #[serde(default)]
    pub rates: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BufferConfig {
    #[serde(default = "default_capacity")]
    pub capacity: usize,
    #[serde(default = "default_batch")]
    pub batch_size: usize,
    #[serde(default = "default_dedupe_window_ms")]
    pub dedupe_window_ms: u64,
    #[serde(default = "default_dedupe_entries")]
    pub dedupe_max_entries: u64,
}

impl Default for BufferConfig {
    fn default() -> Self {
        Self {
            capacity: default_capacity(),
            batch_size: default_batch(),
            dedupe_window_ms: default_dedupe_window_ms(),
            dedupe_max_entries: default_dedupe_entries(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalConfig {
    #[serde(default = "default_wal_dir")]
    pub dir: PathBuf,
    /// fsync after every durable append.
    #[serde(default = "default_true")]
    pub fsync: bool,
    #[serde(default)]
    pub max_segment_bytes: u64,
    /// Set false to disable the WAL entirely (tests, ephemeral hosts).
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

impl Default for WalConfig {
    fn default() -> Self {
        Self {
            dir: default_wal_dir(),
            fsync: true,
            max_segment_bytes: 0, // 0 = kelson-buffer default
            enabled: true,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RetryConfig {
    #[serde(default)]
    pub max_attempts: u32,
    #[serde(default)]
    pub initial_backoff_ms: u64,
    #[serde(default)]
    pub max_backoff_ms: u64,
    #[serde(default)]
    pub jitter: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BreakerCfg {
    #[serde(default)]
    pub failure_threshold: u32,
    #[serde(default)]
    pub cooldown_ms: u64,
}

/// One named exporter in `[telemetry.exporters.<name>]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExporterConfig {
    /// `console | file | otlp | http | syslog`.
    pub kind: String,
    /// Path (file), endpoint URL (otlp/http) or `host:port` (syslog).
    #[serde(default)]
    pub target: Option<String>,
    /// `json | pretty` where the exporter kind supports it.
    #[serde(default)]
    pub format: Option<String>,
    /// Critical exporters are never shed by backpressure.
    #[serde(default)]
    pub critical: bool,
    /// File exporter: partition output per tenant (subdirectories).
    #[serde(default)]
    pub partition_by_tenant: bool,
    #[serde(default)]
    pub retry: Option<RetryConfig>,
    #[serde(default)]
    pub breaker: Option<BreakerCfg>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyRuleConfig {
    pub name: String,
    /// Restricted boolean grammar (kelson-pipeline §policy); no arbitrary code.
    pub when: String,
    pub actions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CardinalityConfig {
    #[serde(default = "default_max_label_sets")]
    pub max_label_sets: u64,
}

impl Default for CardinalityConfig {
    fn default() -> Self {
        Self {
            max_label_sets: default_max_label_sets(),
        }
    }
}

/// Compiled `[telemetry]` section. Every field has a safe default; unknown
/// keys are ignored (forward compatibility).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryConfig {
    #[serde(default = "default_level")]
    pub level: String,
    #[serde(default = "default_format")]
    pub format: String,
    #[serde(default)]
    pub sampling: SamplingConfig,
    #[serde(default)]
    pub buffer: BufferConfig,
    #[serde(default)]
    pub wal: WalConfig,
    #[serde(default)]
    pub exporters: BTreeMap<String, ExporterConfig>,
    #[serde(default)]
    pub redactions: Vec<String>,
    #[serde(default)]
    pub cardinality: CardinalityConfig,
    #[serde(default)]
    pub policy: Vec<PolicyRuleConfig>,
    /// Paths (TOML cannot express them here; resolved against the base dir).
    #[serde(default)]
    pub paths: PathsConfig,
    #[serde(default = "default_retry_budget")]
    pub retry_budget_per_sec: u64,
    /// Resource monitor / health snapshot cadence.
    #[serde(default = "default_tick_ms")]
    pub tick_ms: u64,
    /// Capability name required by the admin incident endpoint.
    #[serde(default = "default_incident_capability")]
    pub incident_capability: String,
}

fn default_incident_capability() -> String {
    "telemetry.incident.manage".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathsConfig {
    #[serde(default = "default_incident_dir")]
    pub incident_dir: PathBuf,
    #[serde(default = "default_cost_file")]
    pub cost_file: PathBuf,
    #[serde(default = "default_health_dir")]
    pub health_dir: PathBuf,
}

impl Default for PathsConfig {
    fn default() -> Self {
        Self {
            incident_dir: default_incident_dir(),
            cost_file: default_cost_file(),
            health_dir: default_health_dir(),
        }
    }
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            level: default_level(),
            format: default_format(),
            sampling: SamplingConfig::default(),
            buffer: BufferConfig::default(),
            wal: WalConfig::default(),
            exporters: BTreeMap::new(),
            redactions: Vec::new(),
            cardinality: CardinalityConfig::default(),
            policy: Vec::new(),
            paths: PathsConfig::default(),
            retry_budget_per_sec: default_retry_budget(),
            tick_ms: default_tick_ms(),
            incident_capability: default_incident_capability(),
        }
    }
}

impl TelemetryConfig {
    /// Parse the optional `[telemetry]` section value (from PlatformConfig)
    /// and apply the env overlay. Never fails: malformed sections fall back
    /// to defaults (telemetry must never block boot).
    pub fn from_section(section: Option<&serde_json::Value>) -> Self {
        let mut cfg = match section {
            Some(value) => serde_json::from_value(value.clone()).unwrap_or_default(),
            None => Self::default(),
        };
        cfg.apply_env();
        cfg
    }

    /// Parse from a TOML string (used by `kelson observe config` and tests).
    pub fn from_toml(raw: &str) -> Result<Self, String> {
        let value: toml::Value = toml::from_str(raw).map_err(|e| e.to_string())?;
        let section = match value {
            toml::Value::Table(t) => t.get("telemetry").cloned(),
            _ => None,
        };
        let json = match section {
            Some(v) => serde_json::to_value(v).map_err(|e| e.to_string())?,
            None => serde_json::Value::Null,
        };
        Ok(Self::from_section(Some(&json)))
    }

    /// Environment overlay (file ← .env ← env vars convention). Only a small,
    /// documented set of knobs is overlayable; structure stays frozen.
    pub fn apply_env(&mut self) {
        let get = |k: &str| std::env::var(k).ok();
        if let Some(v) = get("KEEL_TELEMETRY_LEVEL") {
            self.level = v;
        }
        if let Some(v) = get("KEEL_TELEMETRY_FORMAT") {
            self.format = v;
        }
        if let Some(v) = get("KEEL_TELEMETRY_WAL_DIR") {
            self.wal.dir = PathBuf::from(v);
        }
        if let Some(v) = get("KEEL_TELEMETRY_INCIDENT_DIR") {
            self.paths.incident_dir = PathBuf::from(v);
        }
        if let Some(v) = get("KEEL_TELEMETRY_CAPACITY") {
            if let Ok(n) = v.parse() {
                self.buffer.capacity = n;
            }
        }
        if get("KEEL_TELEMETRY_CONSOLE_DISABLED")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false)
        {
            self.exporters.remove("console");
        }
    }

    /// Resolve all storage paths against `base` when they are relative
    /// (hosts booting from a workspace root keep the storage/ convention).
    pub fn rebase(&mut self, base: &Path) {
        let rebase = |p: &mut PathBuf| {
            if p.is_relative() {
                *p = base.join(&*p);
            }
        };
        rebase(&mut self.wal.dir);
        rebase(&mut self.paths.incident_dir);
        rebase(&mut self.paths.cost_file);
        rebase(&mut self.paths.health_dir);
        for exporter in self.exporters.values_mut() {
            if let Some(target) = &exporter.target {
                let path = PathBuf::from(target);
                if matches!(exporter.kind.as_str(), "file") && path.is_relative() {
                    exporter.target = Some(base.join(path).to_string_lossy().into_owned());
                }
            }
        }
    }

    /// Default file exporter (JSONL under storage/framework/telemetry/logs).
    pub fn default_file_exporter_path(base: &Path) -> PathBuf {
        base.join("storage/framework/telemetry/logs/app.jsonl")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `KEEL_TELEMETRY_*` is process-global: tests that set the vars (or read
    /// them via `apply_env`) serialize here to avoid parallel-test races.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn env_guard() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn defaults_are_safe() {
        let cfg = TelemetryConfig::default();
        assert_eq!(cfg.level, "info");
        assert_eq!(cfg.format, "json");
        assert!(cfg.wal.enabled && cfg.wal.fsync);
        assert!(cfg.buffer.capacity >= 1024);
        assert!(cfg.retry_budget_per_sec >= 1);
    }

    #[test]
    fn section_merge_overrides_and_keeps_defaults() {
        let _env = env_guard();
        let value = serde_json::json!({
            "level": "debug",
            "sampling": { "rates": { "log": 0.5 } },
            "exporters": {
                "console": { "kind": "console", "critical": true },
                "otlp": { "kind": "otlp", "target": "http://otel:4318/v1/logs" }
            },
            "policy": [
                { "name": "drop-debug-prod", "when": "level == \"debug\"", "actions": ["drop"] }
            ],
            "unknown_future_key": { "x": 1 }
        });
        let cfg = TelemetryConfig::from_section(Some(&value));
        assert_eq!(cfg.level, "debug");
        assert_eq!(cfg.sampling.rates.get("log"), Some(&0.5));
        assert_eq!(cfg.exporters.len(), 2);
        assert_eq!(cfg.policy.len(), 1);
        assert_eq!(cfg.buffer.capacity, default_capacity()); // untouched default
    }

    #[test]
    fn malformed_section_falls_back_to_defaults() {
        let _env = env_guard();
        let value = serde_json::json!({ "level": 42, "buffer": "nope" });
        let cfg = TelemetryConfig::from_section(Some(&value));
        assert_eq!(cfg.level, "info"); // whole-section fallback
    }

    #[test]
    fn from_toml_parses_section() {
        let _env = env_guard();
        let raw = r#"
[telemetry]
level = "warn"

[telemetry.exporters.console]
kind = "console"
critical = true
"#;
        let cfg = TelemetryConfig::from_toml(raw).unwrap();
        assert_eq!(cfg.level, "warn");
        assert!(cfg.exporters.contains_key("console"));
    }

    #[test]
    fn env_overlay_applies() {
        let _env = env_guard();
        std::env::set_var("KEEL_TELEMETRY_LEVEL", "trace-test");
        std::env::set_var("KEEL_TELEMETRY_CAPACITY", "77");
        let mut cfg = TelemetryConfig::default();
        cfg.apply_env();
        assert_eq!(cfg.level, "trace-test");
        assert_eq!(cfg.buffer.capacity, 77);
        std::env::remove_var("KEEL_TELEMETRY_LEVEL");
        std::env::remove_var("KEEL_TELEMETRY_CAPACITY");
    }

    #[test]
    fn rebase_resolves_relative_paths() {
        let mut cfg = TelemetryConfig::default();
        cfg.rebase(Path::new("/srv/app"));
        assert_eq!(
            cfg.wal.dir,
            PathBuf::from("/srv/app/storage/framework/telemetry/wal")
        );
        assert_eq!(
            cfg.paths.incident_dir,
            PathBuf::from("/srv/app/storage/framework/telemetry/control")
        );
    }
}
