//! Route binding resolvers (Wave 1 model binding).
//!
//! The HTTP edge resolves declared `{param}` path bindings through the
//! well-known `route.bindings` service: modules register one resolver per
//! model in `Provider::init`; dispatch looks the model up here, performs a
//! tenant-scoped find, and injects the record as `_bindings.<param>`.
//! Misses and cross-tenant lookups surface as 404 envelopes at the edge.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use kelson_foundation::{AppError, TenantId};
use uuid::Uuid;

use crate::store::Record;

/// Resolve one path parameter value to a record for a named model.
pub trait RouteBindingResolver: Send + Sync {
    /// Model name this resolver serves (matches `BindingSpec.model`).
    fn model(&self) -> &str;

    /// Tenant-scoped find. `Ok(None)` = miss (the edge maps it to 404).
    fn resolve(&self, tenant: Option<TenantId>, id: Uuid) -> Result<Option<Record>, AppError>;

    /// Trashed-aware find (Wave 3 Phase 6, `with_trashed` route bindings):
    /// resolves soft-deleted rows too. The default FAILS CLOSED — it
    /// delegates to [`Self::resolve`], so a resolver that does not opt in
    /// (e.g. one wired to a hard-delete store) never exposes trashed rows.
    fn resolve_trashed(&self, tenant: Option<TenantId>, id: Uuid) -> Result<Option<Record>, AppError> {
        self.resolve(tenant, id)
    }
}

/// Dispatch one binding resolution honoring `BindingSpec::with_trashed`.
/// Kept as a free function so the edge (and unit tests) share exactly one
/// branch: `with_trashed` → [`RouteBindingResolver::resolve_trashed`],
/// otherwise the plain [`RouteBindingResolver::resolve`].
pub fn resolve_binding(
    resolver: &dyn RouteBindingResolver,
    with_trashed: bool,
    tenant: Option<TenantId>,
    id: Uuid,
) -> Result<Option<Record>, AppError> {
    if with_trashed {
        resolver.resolve_trashed(tenant, id)
    } else {
        resolver.resolve(tenant, id)
    }
}

/// Well-known `route.bindings` service: model name -> resolver. Modules
/// register in `Provider::init`; the boot gate GRAPH-BINDING-RESOLVER
/// refuses routes whose model has no resolver (fail closed, collect-all).
#[derive(Default)]
pub struct RouteBindingRegistry {
    resolvers: Mutex<HashMap<String, Arc<dyn RouteBindingResolver>>>,
}

impl RouteBindingRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register (or replace) the resolver for its model name.
    pub fn register(&self, resolver: Arc<dyn RouteBindingResolver>) {
        self.resolvers
            .lock()
            .unwrap()
            .insert(resolver.model().to_string(), resolver);
    }

    pub fn get(&self, model: &str) -> Option<Arc<dyn RouteBindingResolver>> {
        self.resolvers.lock().unwrap().get(model).cloned()
    }

    /// All registered model names (boot-gate input).
    pub fn models(&self) -> BTreeSet<String> {
        self.resolvers.lock().unwrap().keys().cloned().collect()
    }
}

/// Lifecycle-registry carrier (service name: `route.bindings`).
#[derive(Clone, Default)]
pub struct RouteBindingsRef(pub Arc<RouteBindingRegistry>);

#[cfg(test)]
mod tests {
    use super::*;

    struct StubResolver {
        model: &'static str,
        found: bool,
    }

    impl RouteBindingResolver for StubResolver {
        fn model(&self) -> &str {
            self.model
        }
        fn resolve(
            &self,
            _tenant: Option<TenantId>,
            _id: Uuid,
        ) -> Result<Option<Record>, AppError> {
            Ok(self.found.then(Record::new))
        }
    }

    #[test]
    fn registry_registers_and_looks_up_by_model() {
        let registry = RouteBindingRegistry::new();
        registry.register(Arc::new(StubResolver {
            model: "Thing",
            found: true,
        }));
        registry.register(Arc::new(StubResolver {
            model: "Other",
            found: false,
        }));
        assert_eq!(registry.models().len(), 2);
        assert!(registry
            .get("Thing")
            .unwrap()
            .resolve(None, Uuid::now_v7())
            .unwrap()
            .is_some());
        assert!(registry
            .get("Other")
            .unwrap()
            .resolve(None, Uuid::now_v7())
            .unwrap()
            .is_none());
        assert!(registry.get("Missing").is_none());
    }

    #[test]
    fn register_replaces_same_model() {
        let registry = RouteBindingRegistry::new();
        registry.register(Arc::new(StubResolver {
            model: "Thing",
            found: false,
        }));
        registry.register(Arc::new(StubResolver {
            model: "Thing",
            found: true,
        }));
        assert_eq!(registry.models().len(), 1);
        assert!(registry
            .get("Thing")
            .unwrap()
            .resolve(None, Uuid::now_v7())
            .unwrap()
            .is_some());
    }

    /// A resolver that DOES opt into trashed rows: `resolve` hides them,
    /// `resolve_trashed` surfaces them.
    struct TrashedAwareResolver {
        trashed: bool,
    }

    impl RouteBindingResolver for TrashedAwareResolver {
        fn model(&self) -> &str {
            "Thing"
        }
        fn resolve(
            &self,
            _tenant: Option<TenantId>,
            _id: Uuid,
        ) -> Result<Option<Record>, AppError> {
            Ok((!self.trashed).then(Record::new))
        }
        fn resolve_trashed(
            &self,
            _tenant: Option<TenantId>,
            _id: Uuid,
        ) -> Result<Option<Record>, AppError> {
            Ok(Some(Record::new()))
        }
    }

    #[test]
    fn resolve_binding_flag_routes_to_the_matching_hook() {
        // with_trashed=true reaches resolve_trashed (the trashed row resolves).
        let aware = TrashedAwareResolver { trashed: true };
        assert!(resolve_binding(&aware, true, None, Uuid::now_v7())
            .unwrap()
            .is_some());
        // with_trashed=false stays on resolve (the trashed row is a miss).
        assert!(resolve_binding(&aware, false, None, Uuid::now_v7())
            .unwrap()
            .is_none());
    }

    #[test]
    fn resolve_binding_fails_closed_without_an_override() {
        // StubResolver does not override resolve_trashed: the default
        // delegates to resolve, so both flag states agree (no trashed leak).
        let stub = StubResolver {
            model: "Thing",
            found: true,
        };
        let miss = StubResolver {
            model: "Thing",
            found: false,
        };
        assert!(resolve_binding(&stub, true, None, Uuid::now_v7())
            .unwrap()
            .is_some());
        assert!(resolve_binding(&miss, true, None, Uuid::now_v7())
            .unwrap()
            .is_none());
    }
}
