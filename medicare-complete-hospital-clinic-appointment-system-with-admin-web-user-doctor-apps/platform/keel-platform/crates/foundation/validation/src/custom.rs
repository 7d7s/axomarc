//! Module-registered custom rules (Wave 4 Phase 4).
//!
//! Manifest entries reference a registered rule with `custom = "<name>"`;
//! the GRAPH-RULE-UNKNOWN boot gate rejects unknown names against this
//! process registry. Modules register from `Provider::init`, which runs
//! before the graph compile boot gate (the edge and the test harness
//! register directly when they build a graph by hand).
//!
//! A custom rule's failure is a plain field message (422). The fatal
//! 503 channel is reserved for row probes: a broken custom rule is a bad
//! answer, a broken store is no answer.

use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

/// One module-provided rule: `Ok(None)` passes the value, `Some(msg)` is
/// the field violation. `field` is the (possibly expanded) field path the
/// rules run against; `input` is the whole validated object so cross-field
/// rules can resolve siblings.
pub trait CustomRule: Send + Sync {
    fn check(&self, field: &str, value: &Value, input: &Value) -> Option<String>;
}

/// Name → rule lookup carried by the validation context.
pub trait CustomRuleLookup: Send + Sync {
    fn rule(&self, name: &str) -> Option<Arc<dyn CustomRule>>;
}

fn registry() -> &'static Mutex<BTreeMap<String, Arc<dyn CustomRule>>> {
    static REGISTRY: OnceLock<Mutex<BTreeMap<String, Arc<dyn CustomRule>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// Register a custom rule under `name` (a later registration under the
/// same name replaces the earlier one). Must run before the graph compile
/// boot gate so GRAPH-RULE-UNKNOWN sees the name.
pub fn register_custom_rule(name: impl Into<String>, rule: Arc<dyn CustomRule>) {
    let mut map = registry().lock().expect("custom rule registry poisoned");
    map.insert(name.into(), rule);
}

/// Every registered custom rule name — the source the GRAPH-RULE-UNKNOWN
/// gate consults at compile time.
pub fn custom_rule_names() -> Vec<String> {
    registry()
        .lock()
        .expect("custom rule registry poisoned")
        .keys()
        .cloned()
        .collect()
}

/// Process-global lookup the HTTP edge attaches to every validation
/// context (mirrors how the edge reads other boot-scoped registries).
pub static GLOBAL_CUSTOM_RULES: GlobalCustomRules = GlobalCustomRules;

/// Lookup over the process-global registry.
pub struct GlobalCustomRules;

impl CustomRuleLookup for GlobalCustomRules {
    fn rule(&self, name: &str) -> Option<Arc<dyn CustomRule>> {
        registry()
            .lock()
            .ok()?
            .get(name)
            .cloned()
    }
}
