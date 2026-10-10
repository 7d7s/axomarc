//! Prune registry (Wave 3 Phase 6): well-known `model.prune` service.
//!
//! Modules register a [`ModelRuntime`] that declared a
//! [`crate::runtime::Prunable`] predicate in `Provider::init`; the
//! `kelson model:prune` maintenance command sweeps every registered model
//! through the [`ModelStore`] port in `chunk`-sized pages (see
//! [`ModelRuntime::prune`]). Runtimes without a predicate are skipped at
//! registration, so an empty registry simply reports "none" — the same
//! posture as `schedule:list` when nothing registers a schedule.

use std::sync::{Arc, Mutex};

use kelson_foundation::{AppError, TenantId};

use crate::runtime::ModelRuntime;
use crate::store::ModelStore;

/// Registry of prunable model runtimes. Registration is a cheap filter:
/// only runtimes that called [`ModelRuntime::with_prunable`] are kept, so
/// callers may register every module runtime unconditionally.
#[derive(Default)]
pub struct PruneRegistry {
    runtimes: Mutex<Vec<ModelRuntime>>,
}

impl PruneRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a runtime; returns whether it was kept (i.e. it declared a
    /// prune predicate). Non-prunable runtimes are silently skipped.
    pub fn register(&self, runtime: ModelRuntime) -> bool {
        if !runtime.is_prunable() {
            return false;
        }
        self.runtimes
            .lock()
            .expect("prune registry poisoned")
            .push(runtime);
        true
    }

    /// How many prunable runtimes are registered.
    pub fn len(&self) -> usize {
        self.runtimes.lock().expect("prune registry poisoned").len()
    }

    /// Whether nothing is registered (the CLI reports "none").
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Sweep every registered prunable model through the store, one model
    /// at a time, returning `(model name, purged rows)` in registration
    /// order. Runtimes are cloned out of the lock first so store I/O never
    /// holds it. One model's error aborts the sweep (the CLI reports it);
    /// already-purged models stay purged — `prune` is idempotent.
    pub fn sweep(
        &self,
        store: &dyn ModelStore,
        tenant: Option<TenantId>,
        chunk: u32,
    ) -> Result<Vec<(String, u64)>, AppError> {
        let runtimes: Vec<ModelRuntime> = self
            .runtimes
            .lock()
            .expect("prune registry poisoned")
            .clone();
        let mut out = Vec::with_capacity(runtimes.len());
        for runtime in &runtimes {
            let count = runtime.prune(store, tenant, chunk)?;
            out.push((runtime.model().name.clone(), count));
        }
        Ok(out)
    }
}

/// Service-registry carrier for [`PruneRegistry`] (well-known service key
/// `model.prune`), mirroring [`crate::store::ModelStoreRef`] and
/// [`crate::bindings::RouteBindingsRef`].
#[derive(Clone, Default)]
pub struct PruneRegistryRef(pub Arc<PruneRegistry>);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{FieldFilter, InMemoryModelStore, ListOpts, ModelStore, Record};
    use crate::{FieldDef, FieldType, ModelDef};
    use serde_json::Value as Json;

    /// A prunable model: keep rows whose `status` is `stale`.
    fn prunable_model() -> ModelDef {
        ModelDef::new("prune_target", "prune_targets")
            .field(FieldDef::new("id", FieldType::Uuid).required())
            .field(FieldDef::new("status", FieldType::Text))
    }

    fn stale_predicate() -> Vec<FieldFilter> {
        vec![FieldFilter::eq("status", Json::String("stale".into()))]
    }

    fn runtime(model: ModelDef, prunable: bool) -> ModelRuntime {
        let rt = ModelRuntime::new(model);
        if prunable {
            rt.with_prunable(stale_predicate)
        } else {
            rt
        }
    }

    #[test]
    fn register_skips_non_prunable_runtimes() {
        let registry = PruneRegistry::new();
        assert!(!registry.register(runtime(prunable_model(), false)));
        assert!(registry.is_empty());
        assert!(registry.register(runtime(prunable_model(), true)));
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn sweep_purges_matches_per_model_and_counts() {
        let registry = PruneRegistry::new();
        assert!(registry.register(runtime(prunable_model(), true)));
        let store = InMemoryModelStore::new();

        // Three stale rows + one live row: only the stale ones purge.
        let model = prunable_model();
        for status in ["stale", "stale", "stale", "live"] {
            let mut row = Record::new();
            row.insert("id".into(), Json::String(uuid::Uuid::now_v7().to_string()));
            row.insert("status".into(), Json::String(status.into()));
            store.insert(&model, &row).expect("row inserts");
        }

        let results = registry.sweep(&*store, None, 1).expect("sweep succeeds");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "prune_target");
        assert_eq!(results[0].1, 3, "chunk=1 walks all three stale rows");

        let page = store
            .list(&model, None, &[], &ListOpts::default())
            .expect("list");
        assert_eq!(page.items.len(), 1, "the live row survives");
        let again = registry.sweep(&*store, None, 200).expect("second sweep");
        assert_eq!(again[0].1, 0, "an exhausted sweep purges nothing");
    }
}
