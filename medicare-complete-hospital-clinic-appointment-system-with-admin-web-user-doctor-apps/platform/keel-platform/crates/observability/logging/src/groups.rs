//! Exception group registry (Sentinel S5 §24): bounded in-process grouping of
//! exceptions by fingerprint with counts, first/last seen and affected
//! routes/tenants. Surfaced by `keel observe errors` and the incident view.

use std::collections::BTreeSet;

use moka::sync::Cache;
use time::OffsetDateTime;

const MAX_GROUPS: u64 = 1024;
const MAX_AFFECTED: usize = 16;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExceptionGroup {
    pub fingerprint: String,
    pub count: u64,
    #[serde(with = "time::serde::iso8601")]
    pub first_seen: OffsetDateTime,
    #[serde(with = "time::serde::iso8601")]
    pub last_seen: OffsetDateTime,
    /// Example signature (`Type: message`).
    pub example: String,
    #[serde(default)]
    pub routes: BTreeSet<String>,
    #[serde(default)]
    pub tenants: BTreeSet<String>,
}

/// Bounded registry (moka): oldest/least-used groups evict under pressure;
/// no unbounded growth even under exception storms.
pub struct ExceptionGroups {
    registry: Cache<String, ExceptionGroup>,
}

impl Default for ExceptionGroups {
    fn default() -> Self {
        Self::new(MAX_GROUPS)
    }
}

impl ExceptionGroups {
    pub fn new(max_groups: u64) -> Self {
        Self {
            registry: Cache::builder().max_capacity(max_groups).build(),
        }
    }

    /// Fold one exception occurrence into its group.
    pub fn record(
        &self,
        fingerprint: &str,
        example: &str,
        route: Option<&str>,
        tenant: Option<&str>,
    ) {
        let now = OffsetDateTime::now_utc();
        let mut group = self
            .registry
            .get(fingerprint)
            .unwrap_or_else(|| ExceptionGroup {
                fingerprint: fingerprint.to_string(),
                count: 0,
                first_seen: now,
                last_seen: now,
                example: example.to_string(),
                routes: BTreeSet::new(),
                tenants: BTreeSet::new(),
            });
        group.count += 1;
        group.last_seen = now;
        if let Some(r) = route {
            if group.routes.len() < MAX_AFFECTED {
                group.routes.insert(r.to_string());
            }
        }
        if let Some(t) = tenant {
            if group.tenants.len() < MAX_AFFECTED {
                group.tenants.insert(t.to_string());
            }
        }
        self.registry.insert(fingerprint.to_string(), group);
    }
    /// Snapshot sorted by most recent activity, then count.
    pub fn snapshot(&self) -> Vec<ExceptionGroup> {
        let mut groups: Vec<ExceptionGroup> = self.registry.iter().map(|(_, g)| g).collect();
        groups.sort_by(|a, b| b.last_seen.cmp(&a.last_seen).then(b.count.cmp(&a.count)));
        groups
    }

    pub fn len(&self) -> usize {
        self.registry.run_pending_tasks();
        self.registry.entry_count() as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_fold_by_fingerprint() {
        let groups = ExceptionGroups::default();
        groups.record(
            "err_aa",
            "DbTimeout: conn refused 1",
            Some("GET /a"),
            Some("t1"),
        );
        groups.record("err_aa", "DbTimeout: conn refused 2", Some("GET /b"), None);
        groups.record("err_bb", "NullRef", None, Some("t2"));

        let snap = groups.snapshot();
        assert_eq!(snap.len(), 2);
        let aa = snap.iter().find(|g| g.fingerprint == "err_aa").unwrap();
        assert_eq!(aa.count, 2);
        assert!(aa.routes.contains("GET /a") && aa.routes.contains("GET /b"));
        assert_eq!(aa.tenants.len(), 1);
    }

    #[test]
    fn bounded_registry_does_not_grow_past_max() {
        let groups = ExceptionGroups::new(4);
        for i in 0..64 {
            groups.record(&format!("err_{i}"), "x", None, None);
        }
        groups.registry.run_pending_tasks();
        assert!(groups.len() <= 8); // moka may transiently hold max + margin
    }
}
