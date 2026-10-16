//! `ModelStore` as a guest sees it: every method marshalled to a host import.
//!
//! This is the whole of ADR 0020 §1 in one file. A module keeps calling
//! `ModelRuntime` against a `Arc<dyn ModelStore>`; what changes is that the
//! trait object behind that reference is this type, and its seventeen methods
//! are seventeen arguments-and-answers rather than seventeen hand-written
//! marshalling paths in each module.
//!
//! Reads and writes reach *different imports*, and that is the capability
//! gate rather than an optimisation: the host checks a read the way it checks
//! `capability_check` and a write the way it checks `capability_write`. A
//! module cannot reach a write through a read-shaped call, so the gate does
//! not depend on this file being careful.

use kelson_foundation::{AppError, TenantId};
use kelson_models::{
    AggregateOp, FieldFilter, ListOpts, ModelDef, ModelStore, Record,
};
use kelson_query::Page;
use serde_json::Value as Json;
use uuid::Uuid;

use crate::{host_call, Port};

/// Encode arguments as the JSON array the host decodes positionally.
fn encode<T: serde::Serialize>(args: T) -> Result<String, AppError> {
    serde_json::to_string(&args)
        .map_err(|e| AppError::internal(format!("module sdk: arguments are not encodable: {e}")))
}

/// Decode a host answer.
fn decode<T: serde::de::DeserializeOwned>(body: &str, method: &str) -> Result<T, AppError> {
    serde_json::from_str(body)
        .map_err(|e| AppError::internal(format!("module sdk: {method} answer is unreadable: {e}")))
}

/// A `ModelStore` whose rows live wherever the host keeps them.
///
/// Nothing here caches. A guest instance is not the store, and a cache would
/// make a module answer from memory the host cannot invalidate.
#[derive(Debug, Default, Clone, Copy)]
pub struct HostModelStore;

impl HostModelStore {
    pub const fn new() -> Self {
        Self
    }
}

impl ModelStore for HostModelStore {
    fn list(
        &self,
        model: &ModelDef,
        tenant: Option<TenantId>,
        filters: &[FieldFilter],
        opts: &ListOpts,
    ) -> Result<Page<Record>, AppError> {
        let out = host_call(Port::ModelRead, "list", &encode((model, tenant, filters, opts))?)?;
        decode(&out, "list")
    }

    fn find(
        &self,
        model: &ModelDef,
        id: Uuid,
        tenant: Option<TenantId>,
        include_deleted: bool,
    ) -> Result<Option<Record>, AppError> {
        let out = host_call(
            Port::ModelRead,
            "find",
            &encode((model, id, tenant, include_deleted))?,
        )?;
        decode(&out, "find")
    }

    fn insert(&self, model: &ModelDef, record: &Record) -> Result<Record, AppError> {
        let out = host_call(Port::ModelWrite, "insert", &encode((model, record))?)?;
        decode(&out, "insert")
    }

    fn update(
        &self,
        model: &ModelDef,
        id: Uuid,
        tenant: Option<TenantId>,
        changes: &Record,
    ) -> Result<Option<Record>, AppError> {
        let out = host_call(
            Port::ModelWrite,
            "update",
            &encode((model, id, tenant, changes))?,
        )?;
        decode(&out, "update")
    }

    fn delete(
        &self,
        model: &ModelDef,
        id: Uuid,
        tenant: Option<TenantId>,
    ) -> Result<bool, AppError> {
        let out = host_call(Port::ModelWrite, "delete", &encode((model, id, tenant))?)?;
        decode(&out, "delete")
    }

    fn upsert(
        &self,
        model: &ModelDef,
        record: &Record,
        conflict: &[String],
    ) -> Result<Record, AppError> {
        let out = host_call(Port::ModelWrite, "upsert", &encode((model, record, conflict))?)?;
        decode(&out, "upsert")
    }

    fn aggregate(
        &self,
        model: &ModelDef,
        tenant: Option<TenantId>,
        filters: &[FieldFilter],
        op: AggregateOp,
    ) -> Result<Json, AppError> {
        let out = host_call(
            Port::ModelRead,
            "aggregate",
            &encode((model, tenant, filters, op))?,
        )?;
        decode(&out, "aggregate")
    }

    fn exists(
        &self,
        model: &ModelDef,
        tenant: Option<TenantId>,
        filters: &[FieldFilter],
    ) -> Result<bool, AppError> {
        let out = host_call(Port::ModelRead, "exists", &encode((model, tenant, filters))?)?;
        decode(&out, "exists")
    }

    fn find_by(
        &self,
        model: &ModelDef,
        tenant: Option<TenantId>,
        filters: &[FieldFilter],
        include_deleted: bool,
    ) -> Result<Option<Record>, AppError> {
        let out = host_call(
            Port::ModelRead,
            "find_by",
            &encode((model, tenant, filters, include_deleted))?,
        )?;
        decode(&out, "find_by")
    }

    fn find_for_update(
        &self,
        model: &ModelDef,
        id: Uuid,
        tenant: Option<TenantId>,
        include_deleted: bool,
    ) -> Result<Option<Record>, AppError> {
        let out = host_call(
            Port::ModelRead,
            "find_for_update",
            &encode((model, id, tenant, include_deleted))?,
        )?;
        decode(&out, "find_for_update")
    }

    fn insert_or_ignore(
        &self,
        model: &ModelDef,
        record: &Record,
    ) -> Result<Option<Record>, AppError> {
        let out = host_call(
            Port::ModelWrite,
            "insert_or_ignore",
            &encode((model, record))?,
        )?;
        decode(&out, "insert_or_ignore")
    }

    fn update_filtered(
        &self,
        model: &ModelDef,
        tenant: Option<TenantId>,
        filters: &[FieldFilter],
        changes: &Record,
    ) -> Result<Vec<Record>, AppError> {
        let out = host_call(
            Port::ModelWrite,
            "update_filtered",
            &encode((model, tenant, filters, changes))?,
        )?;
        decode(&out, "update_filtered")
    }

    fn delete_filtered(
        &self,
        model: &ModelDef,
        tenant: Option<TenantId>,
        filters: &[FieldFilter],
    ) -> Result<Vec<Record>, AppError> {
        let out = host_call(
            Port::ModelWrite,
            "delete_filtered",
            &encode((model, tenant, filters))?,
        )?;
        decode(&out, "delete_filtered")
    }

    fn force_delete(
        &self,
        model: &ModelDef,
        id: Uuid,
        tenant: Option<TenantId>,
    ) -> Result<bool, AppError> {
        let out = host_call(
            Port::ModelWrite,
            "force_delete",
            &encode((model, id, tenant))?,
        )?;
        decode(&out, "force_delete")
    }

    fn truncate(&self, model: &ModelDef, env: &str) -> Result<(), AppError> {
        // `truncate` answers nothing, and an empty answer is the refusal
        // every returning import uses. A `()` decode therefore cannot work:
        // the host's success value and its refusal are the same bytes, so
        // this method reads its verdict off a non-empty acknowledgement
        // instead of pretending to decode a unit.
        let out = host_call(Port::ModelWrite, "truncate", &encode((model, env))?)?;
        if out.trim().is_empty() {
            return Err(AppError::internal("module sdk: truncate was refused"));
        }
        Ok(())
    }

    fn increment(
        &self,
        model: &ModelDef,
        id: Uuid,
        tenant: Option<TenantId>,
        field: &str,
        by: i64,
        extra: &Record,
    ) -> Result<Option<Record>, AppError> {
        let out = host_call(
            Port::ModelWrite,
            "increment",
            &encode((model, id, tenant, field, by, extra))?,
        )?;
        decode(&out, "increment")
    }

    fn restore(
        &self,
        model: &ModelDef,
        id: Uuid,
        tenant: Option<TenantId>,
    ) -> Result<Option<Record>, AppError> {
        let out = host_call(Port::ModelWrite, "restore", &encode((model, id, tenant))?)?;
        decode(&out, "restore")
    }
}
