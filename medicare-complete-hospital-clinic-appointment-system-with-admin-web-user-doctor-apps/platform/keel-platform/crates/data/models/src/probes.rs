//! Row probes for the manifest database rules (`unique`/`exists`,
//! Wave 4 Phase 4).
//!
//! The well-known `model.probes` service mirrors `route.bindings`: modules
//! register one [`ModelProbe`] per model in `Provider::init`, and the HTTP
//! edge adapts the registry behind kelson-validation's `DbProbeLookup` before
//! running validation. A probe that cannot answer — an undeclared column or
//! a store failure — returns `Err`, which the edge renders as a `DB_ERROR`
//! 503: validation never passes against a store that cannot reply.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use kelson_foundation::TenantId;
use serde_json::Value as Json;

use crate::store::{FieldFilter, ListOpts, ModelStore};
use crate::ModelDef;

/// Membership probe for one model (implemented over a [`ModelStore`]).
pub trait ModelProbe: Send + Sync {
    /// Model name this probe serves (matches the rule's `<model>` token).
    fn model(&self) -> &str;

    /// `Ok(true)` when at least one LIVE row matches `column = value` plus
    /// every literal `wheres` pair under `tenant`, excluding `ignore_id`
    /// from the match when given. `Err(detail)` when the probe cannot
    /// answer (unknown column, store failure).
    fn has_row(
        &self,
        tenant: Option<TenantId>,
        column: &str,
        value: &str,
        wheres: &[(String, String)],
        ignore_id: Option<&str>,
    ) -> Result<bool, String>;
}

/// Default probe: a [`ModelDef`] plus its store backend.
pub struct StoreProbe {
    model: ModelDef,
    store: Arc<dyn ModelStore>,
}

impl StoreProbe {
    pub fn new(model: ModelDef, store: Arc<dyn ModelStore>) -> Self {
        Self { model, store }
    }
}

impl ModelProbe for StoreProbe {
    fn model(&self) -> &str {
        &self.model.name
    }

    fn has_row(
        &self,
        tenant: Option<TenantId>,
        column: &str,
        value: &str,
        wheres: &[(String, String)],
        ignore_id: Option<&str>,
    ) -> Result<bool, String> {
        // Fail closed on a column the model does not declare: a typo'd rule
        // (or a probe against the wrong model) is a store error, not a pass.
        let mut filters = vec![FieldFilter::eq(column, value)];
        if self.model.field_named(column).is_none() {
            return Err(format!("model {} has no column {column}", self.model.name));
        }
        for (col, val) in wheres {
            if self.model.field_named(col).is_none() {
                return Err(format!("model {} has no column {col}", self.model.name));
            }
            filters.push(FieldFilter::eq(col.clone(), val.clone()));
        }
        match ignore_id {
            // No exclusion: one live match answers the probe.
            None => self
                .store
                .exists(&self.model, tenant, &filters)
                .map_err(|e| format!("{e}")),
            // Exclusion needs every match (`find_by` could return only the
            // ignored row and hide a real duplicate).
            Some(ignore) => {
                let page = self
                    .store
                    .list(&self.model, tenant, &filters, &ListOpts::default())
                    .map_err(|e| format!("{e}"))?;
                Ok(page
                    .items
                    .iter()
                    .any(|row| row.get("id").and_then(Json::as_str) != Some(ignore)))
            }
        }
    }
}

/// Well-known `model.probes` service: model name -> row probe. Modules
/// register in `Provider::init`; a `unique`/`exists` rule whose model has
/// no registered probe fails closed (503), never a pass.
#[derive(Default)]
pub struct ProbeRegistry {
    probes: Mutex<HashMap<String, Arc<dyn ModelProbe>>>,
}

impl ProbeRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register (or replace) the probe for its model name.
    pub fn register(&self, probe: Arc<dyn ModelProbe>) {
        self.probes
            .lock()
            .unwrap()
            .insert(probe.model().to_string(), probe);
    }

    pub fn get(&self, model: &str) -> Option<Arc<dyn ModelProbe>> {
        self.probes.lock().unwrap().get(model).cloned()
    }

    /// All registered model names (introspection / boot diagnostics).
    pub fn models(&self) -> BTreeSet<String> {
        self.probes.lock().unwrap().keys().cloned().collect()
    }
}

/// Lifecycle-registry carrier (service name: `model.probes`).
#[derive(Clone, Default)]
pub struct ProbeRegistryRef(pub Arc<ProbeRegistry>);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Record;
    use crate::{FieldDef, FieldType};

    fn patient_model() -> ModelDef {
        ModelDef::new("Patient", "patients")
            .tenant_scoped()
            .hard_deletes()
            .field(FieldDef::new("id", FieldType::Uuid).required())
            .field(FieldDef::new("tenant_id", FieldType::Uuid).required())
            .field(FieldDef::new("email", FieldType::Text).required())
            .field(FieldDef::new("status", FieldType::Text).required())
    }

    fn row(tenant: TenantId, email: &str, status: &str) -> Record {
        let mut r = Record::new();
        r.insert(
            "id".into(),
            Json::String(uuid::Uuid::now_v7().to_string()),
        );
        r.insert("tenant_id".into(), Json::String(tenant.to_string()));
        r.insert("email".into(), Json::String(email.into()));
        r.insert("status".into(), Json::String(status.into()));
        r
    }

    #[test]
    fn store_probe_is_tenant_scoped_and_column_checked() {
        let store = Arc::new(crate::store::InMemoryModelStore::new());
        let model = patient_model();
        store.insert(&model, &row(TenantId::new(), "a@clinic.test", "active")).unwrap();
        let other = TenantId::new();
        store.insert(&model, &row(other, "b@clinic.test", "active")).unwrap();

        let probe = StoreProbe::new(model, store);
        // The default probe lives in the other tenant: no match under ours.
        assert!(!probe
            .has_row(Some(TenantId::new()), "email", "a@clinic.test", &[], None)
            .unwrap());
        // Wrong tenant never sees the other tenant's rows either.
        assert!(!probe
            .has_row(Some(TenantId::new()), "email", "b@clinic.test", &[], None)
            .unwrap());
        // The right tenant matches.
        let right = probe
            .has_row(Some(other), "email", "b@clinic.test", &[], None)
            .unwrap();
        assert!(right);

        // Literal where pairs narrow the match.
        assert!(!probe
            .has_row(
                Some(other),
                "email",
                "b@clinic.test",
                &[("status".into(), "cancelled".into())],
                None
            )
            .unwrap());
        assert!(probe
            .has_row(
                Some(other),
                "email",
                "b@clinic.test",
                &[("status".into(), "active".into())],
                None
            )
            .unwrap());

        // Undeclared columns fail closed as probe errors, not passes.
        let err = probe
            .has_row(Some(other), "ghost", "x", &[], None)
            .unwrap_err();
        assert!(err.contains("no column ghost"), "{err}");
        let err = probe
            .has_row(Some(other), "email", "x", &[("ghost".into(), "y".into())], None)
            .unwrap_err();
        assert!(err.contains("no column ghost"), "{err}");
    }

    #[test]
    fn ignore_id_excludes_only_that_row() {
        let store = Arc::new(crate::store::InMemoryModelStore::new());
        let model = patient_model();
        let tenant = TenantId::new();
        let dup = row(tenant, "dup@clinic.test", "active");
        let dup_id = dup.get("id").unwrap().as_str().unwrap().to_string();
        store.insert(&model, &dup).unwrap();

        let probe = StoreProbe::new(model.clone(), store.clone());
        // Only row is the ignored one → no OTHER match → unique passes.
        assert!(!probe
            .has_row(Some(tenant), "email", "dup@clinic.test", &[], Some(&dup_id))
            .unwrap());
        // A second row with the same value → duplicate detected.
        store
            .insert(&model, &row(tenant, "dup@clinic.test", "active"))
            .unwrap();
        assert!(probe
            .has_row(Some(tenant), "email", "dup@clinic.test", &[], Some(&dup_id))
            .unwrap());
        // A bogus ignore id excludes nothing (fail closed on the value).
        assert!(probe
            .has_row(Some(tenant), "email", "dup@clinic.test", &[], Some("not-a-uuid"))
            .unwrap());
    }

    #[test]
    fn registry_registers_and_looks_up_by_model() {
        let registry = ProbeRegistry::new();
        assert!(registry.get("Patient").is_none());
        registry.register(Arc::new(StoreProbe::new(
            patient_model(),
            Arc::new(crate::store::InMemoryModelStore::new()),
        )));
        let found = registry.get("Patient").expect("probe registered");
        assert_eq!(found.model(), "Patient");
        assert!(registry.get("Ghost").is_none());
        assert_eq!(
            registry.models().into_iter().collect::<Vec<_>>(),
            vec!["Patient".to_string()]
        );
    }
}
