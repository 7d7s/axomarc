//! `ModelStore` as a guest sees it: every method marshalled to a host import.
//!
//! This is the whole of ADR 0020 §1 in one file. A module keeps calling
//! `ModelRuntime` against a `Arc<dyn ModelStore>`; what changes is that the
//! trait object behind that reference is this type, and its seventeen methods
//! are seventeen arguments-and-answers rather than seventeen hand-written
//! marshalling paths in each module.
//!
//! The arguments each method builds are the ones from
//! [`kelson_module_contract`] — the very structs the host decodes — so the
//! seam has one definition of every shape and no positional convention to
//! drift. Reads and writes reach *different imports*, which is the
//! capability gate rather than an optimisation: the host checks a write
//! against the guest's profile, and no method here can reach one through a
//! read-shaped call.

use kelson_foundation::{AppError, TenantId};
use kelson_models::{AggregateOp, FieldFilter, ListOpts, ModelDef, ModelStore, Record};
use kelson_module_contract::{
    AggregateArgs, DeleteArgs, DeleteFilteredArgs, ExistsArgs, FindArgs, FindByArgs,
    FindForUpdateArgs, ForceDeleteArgs, IncrementArgs, InsertArgs, InsertOrIgnoreArgs, ListArgs,
    RestoreArgs, TruncateArgs, UpdateArgs, UpdateFilteredArgs, UpsertArgs,
};
use kelson_query::Page;
use serde_json::Value as Json;
use uuid::Uuid;

use crate::{call, Port};

/// Decode a host answer into the method's result type.
///
/// A shape the guest and host disagree about is a bug in whichever of them
/// changed, and it surfaces here rather than as plausible-looking rows.
fn decode<T: serde::de::DeserializeOwned>(value: Json) -> Result<T, AppError> {
    serde_json::from_value(value)
        .map_err(|e| AppError::internal(format!("module sdk: seam answer is unreadable: {e}")))
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
        decode(call(
            Port::ModelRead,
            "list",
            &ListArgs {
                model: model.clone(),
                tenant,
                filters: filters.to_vec(),
                opts: opts.clone(),
            },
        )?)
    }

    fn find(
        &self,
        model: &ModelDef,
        id: Uuid,
        tenant: Option<TenantId>,
        include_deleted: bool,
    ) -> Result<Option<Record>, AppError> {
        decode(call(
            Port::ModelRead,
            "find",
            &FindArgs {
                model: model.clone(),
                id,
                tenant,
                include_deleted,
            },
        )?)
    }

    fn insert(&self, model: &ModelDef, record: &Record) -> Result<Record, AppError> {
        decode(call(
            Port::ModelWrite,
            "insert",
            &InsertArgs {
                model: model.clone(),
                record: record.clone(),
            },
        )?)
    }

    fn update(
        &self,
        model: &ModelDef,
        id: Uuid,
        tenant: Option<TenantId>,
        changes: &Record,
    ) -> Result<Option<Record>, AppError> {
        decode(call(
            Port::ModelWrite,
            "update",
            &UpdateArgs {
                model: model.clone(),
                id,
                tenant,
                changes: changes.clone(),
            },
        )?)
    }

    fn delete(
        &self,
        model: &ModelDef,
        id: Uuid,
        tenant: Option<TenantId>,
    ) -> Result<bool, AppError> {
        decode(call(
            Port::ModelWrite,
            "delete",
            &DeleteArgs {
                model: model.clone(),
                id,
                tenant,
            },
        )?)
    }

    fn upsert(
        &self,
        model: &ModelDef,
        record: &Record,
        conflict: &[String],
    ) -> Result<Record, AppError> {
        decode(call(
            Port::ModelWrite,
            "upsert",
            &UpsertArgs {
                model: model.clone(),
                record: record.clone(),
                conflict: conflict.to_vec(),
            },
        )?)
    }

    fn aggregate(
        &self,
        model: &ModelDef,
        tenant: Option<TenantId>,
        filters: &[FieldFilter],
        op: AggregateOp,
    ) -> Result<Json, AppError> {
        decode(call(
            Port::ModelRead,
            "aggregate",
            &AggregateArgs {
                model: model.clone(),
                tenant,
                filters: filters.to_vec(),
                op,
            },
        )?)
    }

    fn exists(
        &self,
        model: &ModelDef,
        tenant: Option<TenantId>,
        filters: &[FieldFilter],
    ) -> Result<bool, AppError> {
        decode(call(
            Port::ModelRead,
            "exists",
            &ExistsArgs {
                model: model.clone(),
                tenant,
                filters: filters.to_vec(),
            },
        )?)
    }

    fn find_by(
        &self,
        model: &ModelDef,
        tenant: Option<TenantId>,
        filters: &[FieldFilter],
        include_deleted: bool,
    ) -> Result<Option<Record>, AppError> {
        decode(call(
            Port::ModelRead,
            "find_by",
            &FindByArgs {
                model: model.clone(),
                tenant,
                filters: filters.to_vec(),
                include_deleted,
            },
        )?)
    }

    fn find_for_update(
        &self,
        model: &ModelDef,
        id: Uuid,
        tenant: Option<TenantId>,
        include_deleted: bool,
    ) -> Result<Option<Record>, AppError> {
        decode(call(
            Port::ModelRead,
            "find_for_update",
            &FindForUpdateArgs {
                model: model.clone(),
                id,
                tenant,
                include_deleted,
            },
        )?)
    }

    fn insert_or_ignore(
        &self,
        model: &ModelDef,
        record: &Record,
    ) -> Result<Option<Record>, AppError> {
        decode(call(
            Port::ModelWrite,
            "insert_or_ignore",
            &InsertOrIgnoreArgs {
                model: model.clone(),
                record: record.clone(),
            },
        )?)
    }

    fn update_filtered(
        &self,
        model: &ModelDef,
        tenant: Option<TenantId>,
        filters: &[FieldFilter],
        changes: &Record,
    ) -> Result<Vec<Record>, AppError> {
        decode(call(
            Port::ModelWrite,
            "update_filtered",
            &UpdateFilteredArgs {
                model: model.clone(),
                tenant,
                filters: filters.to_vec(),
                changes: changes.clone(),
            },
        )?)
    }

    fn delete_filtered(
        &self,
        model: &ModelDef,
        tenant: Option<TenantId>,
        filters: &[FieldFilter],
    ) -> Result<Vec<Record>, AppError> {
        decode(call(
            Port::ModelWrite,
            "delete_filtered",
            &DeleteFilteredArgs {
                model: model.clone(),
                tenant,
                filters: filters.to_vec(),
            },
        )?)
    }

    fn force_delete(
        &self,
        model: &ModelDef,
        id: Uuid,
        tenant: Option<TenantId>,
    ) -> Result<bool, AppError> {
        decode(call(
            Port::ModelWrite,
            "force_delete",
            &ForceDeleteArgs {
                model: model.clone(),
                id,
                tenant,
            },
        )?)
    }

    fn truncate(&self, model: &ModelDef, env: &str) -> Result<(), AppError> {
        // A `()` answer has nothing to decode, and the envelope settles what
        // used to be ambiguous: `{"ok":null}` is "truncated" and the failure
        // arm carries the reason. Both are non-empty, so "stored" and
        // "refused" can no longer look alike.
        call(
            Port::ModelWrite,
            "truncate",
            &TruncateArgs {
                model: model.clone(),
                env: env.to_string(),
            },
        )
        .map(|_| ())
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
        decode(call(
            Port::ModelWrite,
            "increment",
            &IncrementArgs {
                model: model.clone(),
                id,
                tenant,
                field: field.to_string(),
                by,
                extra: extra.clone(),
            },
        )?)
    }

    fn restore(
        &self,
        model: &ModelDef,
        id: Uuid,
        tenant: Option<TenantId>,
    ) -> Result<Option<Record>, AppError> {
        decode(call(
            Port::ModelWrite,
            "restore",
            &RestoreArgs {
                model: model.clone(),
                id,
                tenant,
            },
        )?)
    }
}
