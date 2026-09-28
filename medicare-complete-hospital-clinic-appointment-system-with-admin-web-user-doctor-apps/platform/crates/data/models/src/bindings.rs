//! Route binding resolvers (Wave 1 model binding).
//!
//! The HTTP edge resolves declared `{param}` path bindings through the
//! well-known `route.bindings` service: modules register one resolver per
//! model in `Provider::init`; dispatch looks the model up here, performs a
//! tenant-scoped find, and injects the record as `_bindings.<param>`.
//! Misses and cross-tenant lookups surface as 404 envelopes at the edge.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use keel_foundation::{AppError, TenantId};
use uuid::Uuid;

use crate::store::Record;

/// Resolve one path parameter value to a record for a named model.
pub trait RouteBindingResolver: Send + Sync {
    /// Model name this resolver serves (matches `BindingSpec.model`).
    fn model(&self) -> &str;

    /// Tenant-scoped find. `Ok(None)` = miss (the edge maps it to 404).
    fn resolve(&self, tenant: Option<TenantId>, id: Uuid) -> Result<Option<Record>, AppError>;
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
}
