//! The module seam's data contract — ADR 0020.
//!
//! A WASM application module and its host are two programs that must agree on
//! every byte crossing between them, and they are compiled from two different
//! build graphs. If each side wrote its own marshalling, the agreement would
//! be a convention held in two files, and conventions drift.
//!
//! This crate is where the agreement is written once. It holds
//!
//! * the **argument shapes** — one struct per trait method, so the guest
//!   encodes `ListArgs` and the host decodes `ListArgs` and there is no
//!   position to get wrong;
//! * the **reply envelope** — [`SeamResult`], which separates "the host said
//!   no" from "the store answered with a failure";
//! * the **dispatcher** — [`model_read`] and friends, the single match over
//!   the trait's methods, which is where a method lives and where a refusal
//!   is decided.
//!
//! The guest crate and the host crate both depend on this one. `kelson-wasm`
//! does not contain a single argument struct, and `kelson-module-sdk` does not
//! contain a single match arm over a method name.
//!
//! # The two answers an import can give
//!
//! Every returning import answers one of exactly three ways:
//!
//! | answer | meaning |
//! |---|---|
//! | `""` (empty) | **the import gate refused.** A write import called by a guest whose profile cannot write. A policy verdict, deliberately carrying no detail — a read-only guest learns nothing from it. |
//! | `{"ok": <value>}` | the store answered. `()`-returning methods carry `null` here, which is non-empty on purpose: an empty answer would make "stored" and "refused" indistinguishable. |
//! | `{"err": {...AppError...}}` | the store failed, or the arguments did not decode. The full [`AppError`] crosses so the module answers with the status code it would have produced locally. |
//!
//! Nothing else exists, and nothing traps. A guest can be wrong about method
//! names and argument shapes as much as it likes; it gets an error, never a
//! host crash.

use kelson_foundation::{AppError, PrincipalId, TenantId};
use kelson_models::{AggregateOp, FieldFilter, ListOpts, ModelDef, Record};
use kelson_tokens::{AuthToken, TokenPurpose};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// The two traits the dispatcher speaks
// ---------------------------------------------------------------------------
//
// Re-exported, not redefined. `ModelStore` and `TokenStore` are owned by the
// data and security classes, and STAGE-0-SPEC §3 says the plugin runtime may
// depend on neither. This re-export is what lets `kelson-wasm` name the type
// of the service it marshals across the import boundary without reaching past
// the seam to find it.
//
// It is the same trait either way — one definition, one vtable, not a second
// one that could drift. What it does *not* do is move ownership: the
// implementations live where they lived, the runtime hands the values in at
// boot, and no path opens from the extension class to a store service.
pub use kelson_models::ModelStore;
pub use kelson_tokens::TokenStore;

// ---------------------------------------------------------------------------
// The reply envelope
// ---------------------------------------------------------------------------

/// What a seam import answers with.
///
/// Externally tagged with snake-case variants, so the wire shape is
/// `{"ok": <value>}` or `{"err": <AppError>}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeamResult {
    /// The store answered. For a `()`-returning method this is `null`.
    Ok(Json),
    /// The store failed, or the request itself was malformed.
    Err(AppError),
}

impl SeamResult {
    /// Encode for the wire. Serialization of this shape cannot fail — both
    /// variants are plain JSON — so an empty answer here would only be a
    /// bug, and returning `""` (the gate-refusal answer) would hide it
    /// behind "the host said no".
    pub fn encode(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| {
            r#"{"err":{"code":"KSN_INTERNAL","category":"internal","severity":"high","retryable":"not_retryable","public_message":"seam reply could not be encoded"}}"#
                .to_string()
        })
    }

    /// Decode from the wire. `body` is never `""`: the transport layer has
    /// already turned an empty answer into a refusal before this sees it.
    pub fn decode(body: &str) -> Result<SeamResult, AppError> {
        serde_json::from_str(body)
            .map_err(|e| AppError::internal(format!("seam reply is not readable: {e}")))
    }
}

/// Turn a store method's `Result` into the wire answer.
fn answer<T: Serialize>(result: Result<T, AppError>) -> String {
    match result {
        Ok(value) => match serde_json::to_value(value) {
            Ok(json) => SeamResult::Ok(json).encode(),
            Err(e) => SeamResult::Err(AppError::internal(format!(
                "seam reply is not encodable: {e}"
            )))
            .encode(),
        },
        Err(err) => SeamResult::Err(err).encode(),
    }
}

/// Decode one method's arguments.
fn arguments<T: DeserializeOwned>(args: &str) -> Result<T, AppError> {
    serde_json::from_str::<T>(args)
        .map_err(|e| AppError::internal(format!("seam arguments are not readable: {e}")))
}

/// The wire answer for a decode failure — distinct from a gate refusal,
/// because a guest that sent the wrong shape deserves to be told so.
fn malformed(method: &str, e: AppError) -> String {
    SeamResult::Err(e.with_meta("seam_method", method)).encode()
}

/// No configured store for this import: a boot misconfiguration, not a
/// policy verdict, so it is an error rather than a silent refusal.
fn unconfigured(what: &str) -> String {
    SeamResult::Err(AppError::internal(format!(
        "module host has no {what} configured for the module seam"
    )))
    .encode()
}

// ---------------------------------------------------------------------------
// ModelStore arguments — one struct per trait method
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListArgs {
    pub model: ModelDef,
    pub tenant: Option<TenantId>,
    pub filters: Vec<FieldFilter>,
    pub opts: ListOpts,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindArgs {
    pub model: ModelDef,
    pub id: Uuid,
    pub tenant: Option<TenantId>,
    pub include_deleted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InsertArgs {
    pub model: ModelDef,
    pub record: Record,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateArgs {
    pub model: ModelDef,
    pub id: Uuid,
    pub tenant: Option<TenantId>,
    pub changes: Record,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteArgs {
    pub model: ModelDef,
    pub id: Uuid,
    pub tenant: Option<TenantId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpsertArgs {
    pub model: ModelDef,
    pub record: Record,
    pub conflict: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregateArgs {
    pub model: ModelDef,
    pub tenant: Option<TenantId>,
    pub filters: Vec<FieldFilter>,
    pub op: AggregateOp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExistsArgs {
    pub model: ModelDef,
    pub tenant: Option<TenantId>,
    pub filters: Vec<FieldFilter>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindByArgs {
    pub model: ModelDef,
    pub tenant: Option<TenantId>,
    pub filters: Vec<FieldFilter>,
    pub include_deleted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindForUpdateArgs {
    pub model: ModelDef,
    pub id: Uuid,
    pub tenant: Option<TenantId>,
    pub include_deleted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InsertOrIgnoreArgs {
    pub model: ModelDef,
    pub record: Record,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateFilteredArgs {
    pub model: ModelDef,
    pub tenant: Option<TenantId>,
    pub filters: Vec<FieldFilter>,
    pub changes: Record,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteFilteredArgs {
    pub model: ModelDef,
    pub tenant: Option<TenantId>,
    pub filters: Vec<FieldFilter>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForceDeleteArgs {
    pub model: ModelDef,
    pub id: Uuid,
    pub tenant: Option<TenantId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TruncateArgs {
    pub model: ModelDef,
    pub env: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncrementArgs {
    pub model: ModelDef,
    pub id: Uuid,
    pub tenant: Option<TenantId>,
    pub field: String,
    pub by: i64,
    pub extra: Record,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreArgs {
    pub model: ModelDef,
    pub id: Uuid,
    pub tenant: Option<TenantId>,
}

// ---------------------------------------------------------------------------
// TokenStore arguments
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenInsertArgs {
    pub token: AuthToken,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindByHashArgs {
    pub purpose: TokenPurpose,
    pub hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarkConsumedArgs {
    pub id: Uuid,
    #[serde(with = "time::serde::iso8601")]
    pub at: time::OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevokeArgs {
    pub id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListForArgs {
    pub principal: PrincipalId,
}

// ---------------------------------------------------------------------------
// The dispatcher — the one match over method names
// ---------------------------------------------------------------------------

/// `model-read` (ADR 0020 §4): reads carry no capability gate of their own.
///
/// `capability_check` gates a *named action under a scope*, and a model read
/// is neither — it is tenant-scoped by [`ModelStore`] itself, in the tenant
/// the guest passes in its arguments and the store enforces. Adding a second
/// gate here would be decoration that reads as defence.
pub fn model_read(store: Option<&dyn ModelStore>, method: &str, args: &str) -> String {
    let Some(store) = store else {
        return unconfigured("model store");
    };
    match method {
        "list" => {
            let a = match arguments::<ListArgs>(args) {
                Ok(a) => a,
                Err(e) => return malformed(method, e),
            };
            answer(store.list(&a.model, a.tenant, &a.filters, &a.opts))
        }
        "find" => {
            let a = match arguments::<FindArgs>(args) {
                Ok(a) => a,
                Err(e) => return malformed(method, e),
            };
            answer(store.find(&a.model, a.id, a.tenant, a.include_deleted))
        }
        "aggregate" => {
            let a = match arguments::<AggregateArgs>(args) {
                Ok(a) => a,
                Err(e) => return malformed(method, e),
            };
            answer(store.aggregate(&a.model, a.tenant, &a.filters, a.op))
        }
        "exists" => {
            let a = match arguments::<ExistsArgs>(args) {
                Ok(a) => a,
                Err(e) => return malformed(method, e),
            };
            answer(store.exists(&a.model, a.tenant, &a.filters))
        }
        "find_by" => {
            let a = match arguments::<FindByArgs>(args) {
                Ok(a) => a,
                Err(e) => return malformed(method, e),
            };
            answer(store.find_by(&a.model, a.tenant, &a.filters, a.include_deleted))
        }
        "find_for_update" => {
            let a = match arguments::<FindForUpdateArgs>(args) {
                Ok(a) => a,
                Err(e) => return malformed(method, e),
            };
            answer(store.find_for_update(&a.model, a.id, a.tenant, a.include_deleted))
        }
        other => unknown("model", other),
    }
}

/// `model-write` (ADR 0020 §4): gated by the guest's capability profile.
///
/// `can_write` is the same enabling condition [`capability_write`] checks —
/// a ReadWrite/Admin profile — so a read-only guest cannot reach a write
/// even though it names the import, and the check sits on the *import*
/// rather than on any arm below: a method added to `ModelStore` tomorrow
/// inherits the right posture without anyone remembering to gate it.
///
/// Writes go **through** to the store rather than buffering, because
/// `ModelStore::insert` is defined as returning the stored row. A buffered
/// write could not answer that, and every first-party module already writes
/// through its store mid-handler — the seam's whole job is to behave like
/// what it replaced.
///
/// [`capability_write`]: https://docs.rs/
pub fn model_write(
    store: Option<&dyn ModelStore>,
    method: &str,
    args: &str,
    can_write: bool,
) -> String {
    if !can_write {
        return String::new();
    }
    let Some(store) = store else {
        return unconfigured("model store");
    };
    match method {
        "insert" => {
            let a = match arguments::<InsertArgs>(args) {
                Ok(a) => a,
                Err(e) => return malformed(method, e),
            };
            answer(store.insert(&a.model, &a.record))
        }
        "update" => {
            let a = match arguments::<UpdateArgs>(args) {
                Ok(a) => a,
                Err(e) => return malformed(method, e),
            };
            answer(store.update(&a.model, a.id, a.tenant, &a.changes))
        }
        "delete" => {
            let a = match arguments::<DeleteArgs>(args) {
                Ok(a) => a,
                Err(e) => return malformed(method, e),
            };
            answer(store.delete(&a.model, a.id, a.tenant))
        }
        "upsert" => {
            let a = match arguments::<UpsertArgs>(args) {
                Ok(a) => a,
                Err(e) => return malformed(method, e),
            };
            answer(store.upsert(&a.model, &a.record, &a.conflict))
        }
        "insert_or_ignore" => {
            let a = match arguments::<InsertOrIgnoreArgs>(args) {
                Ok(a) => a,
                Err(e) => return malformed(method, e),
            };
            answer(store.insert_or_ignore(&a.model, &a.record))
        }
        "update_filtered" => {
            let a = match arguments::<UpdateFilteredArgs>(args) {
                Ok(a) => a,
                Err(e) => return malformed(method, e),
            };
            answer(store.update_filtered(&a.model, a.tenant, &a.filters, &a.changes))
        }
        "delete_filtered" => {
            let a = match arguments::<DeleteFilteredArgs>(args) {
                Ok(a) => a,
                Err(e) => return malformed(method, e),
            };
            answer(store.delete_filtered(&a.model, a.tenant, &a.filters))
        }
        "force_delete" => {
            let a = match arguments::<ForceDeleteArgs>(args) {
                Ok(a) => a,
                Err(e) => return malformed(method, e),
            };
            answer(store.force_delete(&a.model, a.id, a.tenant))
        }
        "truncate" => {
            let a = match arguments::<TruncateArgs>(args) {
                Ok(a) => a,
                Err(e) => return malformed(method, e),
            };
            answer(store.truncate(&a.model, &a.env))
        }
        "increment" => {
            let a = match arguments::<IncrementArgs>(args) {
                Ok(a) => a,
                Err(e) => return malformed(method, e),
            };
            answer(store.increment(&a.model, a.id, a.tenant, &a.field, a.by, &a.extra))
        }
        "restore" => {
            let a = match arguments::<RestoreArgs>(args) {
                Ok(a) => a,
                Err(e) => return malformed(method, e),
            };
            answer(store.restore(&a.model, a.id, a.tenant))
        }
        other => unknown("model-write", other),
    }
}

/// `token-read`.
pub fn token_read(store: Option<&dyn TokenStore>, method: &str, args: &str) -> String {
    let Some(store) = store else {
        return unconfigured("token store");
    };
    match method {
        "find_by_hash" => {
            let a = match arguments::<FindByHashArgs>(args) {
                Ok(a) => a,
                Err(e) => return malformed(method, e),
            };
            answer(store.find_by_hash(a.purpose, &a.hash))
        }
        "list_for" => {
            let a = match arguments::<ListForArgs>(args) {
                Ok(a) => a,
                Err(e) => return malformed(method, e),
            };
            answer(store.list_for(a.principal))
        }
        other => unknown("token", other),
    }
}

/// `token-write` — the same gate as [`model_write`].
pub fn token_write(
    store: Option<&dyn TokenStore>,
    method: &str,
    args: &str,
    can_write: bool,
) -> String {
    if !can_write {
        return String::new();
    }
    let Some(store) = store else {
        return unconfigured("token store");
    };
    match method {
        "insert" => {
            let a = match arguments::<TokenInsertArgs>(args) {
                Ok(a) => a,
                Err(e) => return malformed(method, e),
            };
            answer(store.insert(a.token))
        }
        "mark_consumed" => {
            let a = match arguments::<MarkConsumedArgs>(args) {
                Ok(a) => a,
                Err(e) => return malformed(method, e),
            };
            answer(store.mark_consumed(a.id, a.at))
        }
        "revoke" => {
            let a = match arguments::<RevokeArgs>(args) {
                Ok(a) => a,
                Err(e) => return malformed(method, e),
            };
            answer(store.revoke(a.id))
        }
        other => unknown("token-write", other),
    }
}

/// A method this dispatch does not know: the guest is asking for something
/// the seam does not carry. An error rather than an empty answer, so a
/// module that grows a new trait method gets told loudly on the first call
/// instead of quietly getting "no rows".
fn unknown(port: &str, method: &str) -> String {
    SeamResult::Err(AppError::internal(format!(
        "the {port} seam does not carry a method named {method}"
    )))
    .encode()
}

