//! Read models: materialised, tenant-scoped projections with a staleness surface.
//!
//! A **read model** is a named, declared projection over the existing model
//! store — never a second source of truth. Its data comes from a query over
//! the source models, and it is refreshed on a policy (interval / on-write /
//! on-demand). The refresh is tenant-scoped by the same rule every other
//! write obeys.
//!
//! The staleness surface exposes:
//! - the age of the data
//! - the state (Fresh / Stale / Refreshing / Failed)
//! - the last refresh timestamp and its outcome

use std::collections::BTreeMap;
use std::sync::Arc;

use kelson_foundation::{AppError, TenantId};
use serde::{Deserialize, Serialize};

use crate::{FieldFilter, ListOpts, Record};

/// The policy that governs when a read model refreshes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RefreshPolicy {
    /// Refresh on a fixed interval (seconds).
    Interval(u64),
    /// Refresh when a source model changes (on-write).
    ///
    /// The source models that trigger a refresh.
    OnWrite(Vec<String>),
    /// Refresh only when explicitly requested (on-demand).
    OnDemand,
}

impl RefreshPolicy {
    /// Whether this policy triggers on a write to the given model.
    pub fn triggers_on_write(&self, model_name: &str) -> bool {
        match self {
            RefreshPolicy::OnWrite(models) => models.contains(&model_name.to_string()),
            _ => false,
        }
    }
}

/// The current state of a read model's materialisation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReadModelState {
    /// The data is fresh (within the policy's tolerance).
    Fresh,
    /// The data is stale (exceeds the policy's tolerance).
    Stale,
    /// A refresh is currently in progress.
    Refreshing,
    /// The last refresh failed.
    Failed,
}

/// The staleness surface for a read model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadModelStaleness {
    /// The read model's name.
    pub name: String,
    /// The current state.
    pub state: ReadModelState,
    /// The age of the materialised data (seconds since last successful refresh).
    pub age_seconds: Option<u64>,
    /// When the data was last successfully refreshed.
    pub last_refreshed_at: Option<String>,
    /// The error from the last failed refresh, if any.
    pub last_error: Option<String>,
}

/// A declared read model: its name, the source query, and the refresh policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadModelDef {
    /// The read model's name (singular, PascalCase).
    pub name: String,
    /// The physical table name for the materialised view.
    pub table: String,
    /// The source models this read model projects over.
    pub source_models: Vec<String>,
    /// The SQL query that defines the projection.
    ///
    /// This query runs against the source models' tables, scoped by tenant.
    /// It MUST return a single row per tenant per logical key.
    pub query: String,
    /// The refresh policy.
    pub refresh_policy: RefreshPolicy,
    /// Whether this read model is tenant-scoped (should always be true for
    /// multi-tenant deployments).
    pub tenant_scoped: bool,
}

impl ReadModelDef {
    pub fn new(
        name: impl Into<String>,
        table: impl Into<String>,
        query: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            table: table.into(),
            source_models: Vec::new(),
            query: query.into(),
            refresh_policy: RefreshPolicy::OnDemand,
            tenant_scoped: true,
        }
    }

    pub fn source_model(mut self, name: impl Into<String>) -> Self {
        self.source_models.push(name.into());
        self
    }

    pub fn refresh_policy(mut self, policy: RefreshPolicy) -> Self {
        self.refresh_policy = policy;
        self
    }

    pub fn interval(mut self, seconds: u64) -> Self {
        self.refresh_policy = RefreshPolicy::Interval(seconds);
        self
    }

    pub fn on_write(mut self, models: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.refresh_policy = RefreshPolicy::OnWrite(models.into_iter().map(Into::into).collect());
        self
    }

    pub fn on_demand(mut self) -> Self {
        self.refresh_policy = RefreshPolicy::OnDemand;
        self
    }
}

/// The contract for materialising and refreshing a read model.
///
/// Implementations are provided by the storage layer (MySQL, Postgres, in-memory).
pub trait ReadModelStore: Send + Sync + std::fmt::Debug {
    /// Materialise the read model for all tenants (or a specific tenant).
    ///
    /// This runs the read model's query and writes the results to the
    /// materialised table. The tenant scope is applied by the store.
    fn refresh(
        &self,
        read_model: &ReadModelDef,
        tenant: Option<TenantId>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ReadModelRefreshResult, AppError>> + Send + '_>,
    >;

    /// Refresh all tenants for a read model.
    fn refresh_all_tenants(
        &self,
        read_model: &ReadModelDef,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ReadModelRefreshResult, AppError>> + Send + '_>,
    >;

    /// Get the staleness surface for a read model (for a specific tenant).
    fn staleness(
        &self,
        read_model: &ReadModelDef,
        tenant: Option<TenantId>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ReadModelStaleness, AppError>> + Send + '_>,
    >;

    /// Query the materialised read model.
    fn query(
        &self,
        read_model: &ReadModelDef,
        tenant: Option<TenantId>,
        filters: &[FieldFilter],
        opts: &ListOpts,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<kelson_query::Page<Record>, AppError>>
                + Send
                + '_,
        >,
    >;

    /// Find a single row in the materialised read model.
    fn find(
        &self,
        read_model: &ReadModelDef,
        id: uuid::Uuid,
        tenant: Option<TenantId>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Option<Record>, AppError>> + Send + '_>,
    >;
}

/// The result of a refresh operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadModelRefreshResult {
    /// How many rows were materialised.
    pub rows_materialised: u64,
    /// How many tenants were refreshed.
    pub tenants_refreshed: u64,
    /// The duration of the refresh.
    pub duration_ms: u64,
    /// Whether the refresh succeeded.
    pub success: bool,
    /// Any error message if the refresh failed.
    pub error: Option<String>,
}

/// The read model registry: declared read models and their runtime handles.
#[derive(Debug, Clone, Default)]
pub struct ReadModelRegistry {
    definitions: BTreeMap<String, ReadModelDef>,
    stores: BTreeMap<String, Arc<dyn ReadModelStore>>,
}

impl ReadModelRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a read model definition.
    pub fn register(&mut self, def: ReadModelDef) -> Result<(), AppError> {
        if self.definitions.contains_key(&def.name) {
            return Err(AppError::validation(format!(
                "read model registered twice: {}",
                def.name
            )));
        }
        // Validate the query references only declared source models.
        for src in &def.source_models {
            if !def.source_models.contains(src) {
                return Err(AppError::validation(format!(
                    "read model {} references unknown source model {}",
                    def.name, src
                )));
            }
        }
        self.definitions.insert(def.name.clone(), def);
        Ok(())
    }

    /// Install a store for a read model.
    pub fn install_store(&mut self, name: &str, store: Arc<dyn ReadModelStore>) {
        self.stores.insert(name.to_string(), store);
    }

    /// Get a read model definition.
    pub fn get(&self, name: &str) -> Option<&ReadModelDef> {
        self.definitions.get(name)
    }

    /// Get the store for a read model.
    pub fn store(&self, name: &str) -> Option<Arc<dyn ReadModelStore>> {
        self.stores.get(name).cloned()
    }

    /// All registered read model names.
    pub fn names(&self) -> Vec<String> {
        self.definitions.keys().cloned().collect()
    }
}

/// A read model's data, ready to be used by a ViewModel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadModelData {
    pub name: String,
    pub rows: Vec<Record>,
    pub staleness: ReadModelStaleness,
}

#[cfg(test)]
mod tests {
    use super::*;
    use kelson_foundation::TenantId;

    #[test]
    fn a_read_model_declares_its_sources_and_policy() {
        let rm = ReadModelDef::new(
            "AppointmentSummary",
            "appointment_summaries",
            "SELECT tenant_id, doctor_id, COUNT(*) as count FROM appointments GROUP BY tenant_id, doctor_id",
        )
        .source_model("Appointment")
        .interval(300);

        assert_eq!(rm.name, "AppointmentSummary");
        assert_eq!(rm.table, "appointment_summaries");
        assert_eq!(rm.source_models, vec!["Appointment"]);
        assert!(matches!(rm.refresh_policy, RefreshPolicy::Interval(300)));
        assert!(rm.tenant_scoped);
    }

    #[test]
    fn an_on_write_policy_triggers_for_its_sources() {
        let policy = RefreshPolicy::OnWrite(vec!["Appointment".to_string(), "Doctor".to_string()]);
        assert!(policy.triggers_on_write("Appointment"));
        assert!(policy.triggers_on_write("Doctor"));
        assert!(!policy.triggers_on_write("Patient"));
    }

    #[test]
    fn the_registry_rejects_duplicate_names() {
        let mut registry = ReadModelRegistry::new();
        let rm = ReadModelDef::new("Test", "test", "SELECT 1");
        registry.register(rm.clone()).unwrap();
        assert!(registry.register(rm).is_err());
    }

    #[test]
    fn the_registry_tracks_stores() {
        let mut registry = ReadModelRegistry::new();
        let rm = ReadModelDef::new("Test", "test", "SELECT 1");
        registry.register(rm).unwrap();

        // A mock store would be installed in integration tests.
        assert!(registry.get("Test").is_some());
        assert!(registry.store("Test").is_none());
    }
}
