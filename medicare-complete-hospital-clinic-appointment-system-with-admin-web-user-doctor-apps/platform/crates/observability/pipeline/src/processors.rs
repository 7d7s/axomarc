//! Built-in processors (Sentinel S5 §9): normalize → classify → redact →
//! fingerprint → sample → dedupe → correlate. Each is a named `Processor`;
//! the runtime assembles the DAG order in `graph.rs`.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use kelson_telemetry::{
    classify_fields, exception_fingerprint, Event, Level, ProcessOutcome, Processor,
    TelemetryCategory, TelemetryPayload,
};
use moka::sync::Cache;

/// Cap oversized messages; truncation is always marked (never silent).
pub const DEFAULT_MAX_MESSAGE_LEN: usize = 8 * 1024;
pub const DEFAULT_MAX_FIELDS: usize = 64;

/// Fill defaults, bound sizes. Runs first in the DAG.
pub struct Normalize {
    pub max_message_len: usize,
    pub max_fields: usize,
}

impl Default for Normalize {
    fn default() -> Self {
        Self {
            max_message_len: DEFAULT_MAX_MESSAGE_LEN,
            max_fields: DEFAULT_MAX_FIELDS,
        }
    }
}

impl Processor for Normalize {
    fn name(&self) -> &str {
        "normalize"
    }
    fn process(&self, event: &mut Event) -> ProcessOutcome {
        if event.durability == kelson_telemetry::Durability::Buffered
            && matches!(
                event.payload.default_durability(),
                kelson_telemetry::Durability::Durable
            )
        {
            event.durability = event.payload.default_durability();
        }
        if let TelemetryPayload::Log(l) = &mut event.payload {
            if l.message.len() > self.max_message_len {
                // MSRV 1.85: str::floor_char_boundary is 1.91+; walk back to a
                // char boundary manually (0 is always one).
                let mut cut = self.max_message_len;
                while !l.message.is_char_boundary(cut) {
                    cut -= 1;
                }
                l.message.truncate(cut);
                l.message.push_str("...[truncated]");
            }
            if l.fields.len() > self.max_fields {
                let overflow = l.fields.len() - self.max_fields;
                let keys: Vec<String> = l.fields.keys().take(overflow).cloned().collect();
                for key in keys {
                    l.fields.remove(&key);
                }
                l.fields.insert(
                    "_fields_truncated".into(),
                    serde_json::Value::Number(overflow.into()),
                );
            }
        }
        ProcessOutcome::Continue
    }
}

/// Escalate envelope sensitivity from field names (Secret > Sensitive > Internal).
pub struct Classify;

impl Processor for Classify {
    fn name(&self) -> &str {
        "classify"
    }
    fn process(&self, event: &mut Event) -> ProcessOutcome {
        if let Some(fields) = event.payload.fields_ref() {
            let s = classify_fields(fields);
            if s > event.sensitivity {
                event.sensitivity = s;
            }
        }
        ProcessOutcome::Continue
    }
}

/// Mask values of sensitive field names in the payload field bag (top level
/// and one level of nesting). Defense-in-depth: exporters also line-redact.
pub struct Redact {
    pub fields: Arc<Vec<String>>,
    pub redactions: AtomicU64,
}

impl Redact {
    pub fn new(fields: Vec<String>) -> Self {
        let mut lower: Vec<String> = fields.into_iter().map(|f| f.to_ascii_lowercase()).collect();
        for builtin in kelson_telemetry::DEFAULT_SECRET_FIELDS {
            let b = builtin.to_ascii_lowercase();
            if !lower.contains(&b) {
                lower.push(b);
            }
        }
        Self {
            fields: Arc::new(lower),
            redactions: AtomicU64::new(0),
        }
    }

    pub fn redaction_count(&self) -> u64 {
        self.redactions.load(Ordering::Relaxed)
    }

    fn matches(&self, key: &str) -> bool {
        let lower = key.to_ascii_lowercase();
        let squashed: String = lower.chars().filter(|c| *c != '-' && *c != '_').collect();
        self.fields.iter().any(|f| {
            lower == *f || lower.contains(f.as_str()) || squashed.contains(&f.replace('_', ""))
        })
    }

    fn redact_map(&self, map: &mut serde_json::Map<String, serde_json::Value>) -> u64 {
        let mut count = 0u64;
        let keys: Vec<String> = map.keys().cloned().collect();
        for key in keys {
            if self.matches(&key) {
                map.insert(key, serde_json::Value::String("[REDACTED]".into()));
                count += 1;
                continue;
            }
            if let Some(serde_json::Value::Object(nested)) = map.get_mut(&key) {
                count += self.redact_map(nested);
            }
        }
        count
    }
}

impl Processor for Redact {
    fn name(&self) -> &str {
        "redact"
    }
    fn process(&self, event: &mut Event) -> ProcessOutcome {
        if let Some(fields) = event.payload.fields_mut() {
            let n = self.redact_map(fields);
            if n > 0 {
                self.redactions.fetch_add(n, Ordering::Relaxed);
            }
        }
        ProcessOutcome::Continue
    }
}

/// Apply pending policy `hash:<fields>` actions (sha256-hex prefix replaces
/// the value; still joinable, no longer readable).
pub fn hash_fields(event: &mut Event, fields: &[String]) {
    let Some(bag) = event.payload.fields_mut() else {
        return;
    };
    for name in fields {
        if let Some(value) = bag.get(name).cloned() {
            let raw = serde_json::to_string(&value).unwrap_or_default();
            use sha2::{Digest, Sha256};
            let digest = Sha256::digest(raw.as_bytes());
            let mut hex = String::with_capacity(16);
            for b in digest.iter().take(8) {
                hex.push_str(&format!("{b:02x}"));
            }
            bag.insert(
                name.clone(),
                serde_json::Value::String(format!("sha256:{hex}")),
            );
        }
    }
}

/// Fill missing exception fingerprints so grouping has a stable key.
pub struct Fingerprint;

impl Processor for Fingerprint {
    fn name(&self) -> &str {
        "fingerprint"
    }
    fn process(&self, event: &mut Event) -> ProcessOutcome {
        if let TelemetryPayload::Exception(x) = &mut event.payload {
            if x.fingerprint.is_empty() {
                x.fingerprint =
                    exception_fingerprint(&x.type_name, &x.message, &x.frames, x.route.as_deref());
            }
        }
        ProcessOutcome::Continue
    }
}

/// Deterministic per-event sampling: the decision is a pure function of the
/// event id, so replays and duplicates agree. Errors/audit/security and
/// incident contexts are never sampled away.
pub struct Sample {
    /// Base keep-rate per category (missing categories default to 1.0).
    pub rates: Vec<(TelemetryCategory, f64)>,
    /// Runtime sampling multiplier (control plane), f64 bits in an AtomicU64.
    pub multiplier_bits: Arc<AtomicU64>,
    pub sampled_dropped: AtomicU64,
}

impl Sample {
    pub fn new(rates: Vec<(TelemetryCategory, f64)>) -> Self {
        Self {
            rates,
            multiplier_bits: Arc::new(AtomicU64::new(f64::to_bits(1.0))),
            sampled_dropped: AtomicU64::new(0),
        }
    }

    pub fn set_multiplier(&self, m: f64) {
        self.multiplier_bits
            .store(f64::to_bits(m.clamp(0.0, 1.0)), Ordering::Relaxed);
    }

    pub fn multiplier(&self) -> f64 {
        f64::from_bits(self.multiplier_bits.load(Ordering::Relaxed))
    }

    fn rate_for(&self, category: TelemetryCategory) -> f64 {
        self.rates
            .iter()
            .find(|(c, _)| *c == category)
            .map(|(_, r)| *r)
            .unwrap_or(1.0)
    }
}

impl Processor for Sample {
    fn name(&self) -> &str {
        "sample"
    }
    fn process(&self, event: &mut Event) -> ProcessOutcome {
        let category = event.payload.category();
        let critical = matches!(
            category,
            TelemetryCategory::Audit | TelemetryCategory::Exception | TelemetryCategory::Security
        ) || event
            .payload
            .level_hint()
            .map(|l| l >= Level::Error)
            .unwrap_or(false)
            || event.context.incident_id.is_some();
        if critical {
            return ProcessOutcome::Continue;
        }
        let rate = (self.rate_for(category) * self.multiplier()).clamp(0.0, 1.0);
        if rate >= 1.0 {
            return ProcessOutcome::Continue;
        }
        let mut h = DefaultHasher::new();
        event.id.as_uuid().as_bytes().hash(&mut h);
        let bucket = (h.finish() % 10_000) as f64 / 10_000.0;
        if bucket < rate {
            ProcessOutcome::Continue
        } else {
            self.sampled_dropped.fetch_add(1, Ordering::Relaxed);
            ProcessOutcome::Drop("sampled".into())
        }
    }
}

/// Suppress repeats of the same event signature within a window (dedupe,
/// §processing graph). Keys on fingerprint (exceptions) or category+message.
pub struct Dedupe {
    seen: Cache<String, ()>,
    pub suppressed: AtomicU64,
}

impl Dedupe {
    pub fn new(window: Duration, max_entries: u64) -> Self {
        Self {
            seen: Cache::builder()
                .time_to_live(window)
                .max_capacity(max_entries)
                .build(),
            suppressed: AtomicU64::new(0),
        }
    }

    pub fn suppressed_count(&self) -> u64 {
        self.suppressed.load(Ordering::Relaxed)
    }

    fn key_for(event: &Event) -> Option<String> {
        let category = event.payload.category();
        let basis = match &event.payload {
            TelemetryPayload::Exception(x) => format!("x:{}", x.fingerprint),
            TelemetryPayload::Log(l) if l.level <= Level::Info => {
                format!("l:{}:{}", category, l.message)
            }
            _ => return None, // errors and above are never deduped
        };
        let tenant = event
            .context
            .tenant_id
            .map(|t| t.to_string())
            .unwrap_or_default();
        Some(format!("{tenant}|{basis}"))
    }
}

impl Processor for Dedupe {
    fn name(&self) -> &str {
        "dedupe"
    }
    fn process(&self, event: &mut Event) -> ProcessOutcome {
        let Some(key) = Self::key_for(event) else {
            return ProcessOutcome::Continue;
        };
        if self.seen.get(&key).is_some() {
            self.suppressed.fetch_add(1, Ordering::Relaxed);
            return ProcessOutcome::Drop("duplicate-within-window".into());
        }
        self.seen.insert(key, ());
        ProcessOutcome::Continue
    }
}

/// Enrich with correlation facts already present on the context (join keys
/// operators ask for first).
pub struct Correlate;

impl Processor for Correlate {
    fn name(&self) -> &str {
        "correlate"
    }
    fn process(&self, event: &mut Event) -> ProcessOutcome {
        let Some(fields) = event.payload.fields_mut() else {
            return ProcessOutcome::Continue;
        };
        if !fields.contains_key("correlation_id") {
            if let Some(c) = event.context.correlation_id {
                fields.insert(
                    "correlation_id".into(),
                    serde_json::Value::String(c.to_string()),
                );
            }
        }
        if !fields.contains_key("execution_id") {
            if let Some(e) = event.context.execution_id {
                fields.insert(
                    "execution_id".into(),
                    serde_json::Value::String(e.to_string()),
                );
            }
        }
        ProcessOutcome::Continue
    }
}
