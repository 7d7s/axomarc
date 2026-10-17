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

/// Decode one method's arguments — the single point every method decodes at.
///
/// The object check is the whole reason this is one function. Serde accepts a
/// derived struct from a JSON *array* as well as from an object, in field
/// declaration order, so a guest that hand-rolled its marshalling could send a
/// positional array and have it decode with the fields in the wrong place. The
/// contract exists so that there is no position to get wrong; this is where
/// that is made true rather than assumed.
pub fn arguments<T: DeserializeOwned>(args: &str) -> Result<T, AppError> {
    let value: Json = serde_json::from_str(args)
        .map_err(|e| AppError::internal(format!("seam arguments are not readable: {e}")))?;
    if !value.is_object() {
        return Err(AppError::internal(
            "seam arguments must be a named object, not a positional array",
        ));
    }
    serde_json::from_value(value)
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

// ---------------------------------------------------------------------------
// Notifications - ADR 0020 �5
// ---------------------------------------------------------------------------

/// One notification a module asked its host to deliver.
///
/// Carried by `host.notify_enqueue`, the fifth seam import. It is *intent*,
/// not a rendered message: the module names a registered notification and
/// hands over the producer's template vars, and the host resolves the def,
/// the catalog, the locale and the channels — exactly the work
/// [`NotificationSink`]'s implementations do on the host side, where the
/// catalog and the store live.
///
/// The `event` is never invented by the host and never computed by the
/// module from user input: it is one of the module's own registered
/// notifications, so a module cannot ask the host to notify on an event that
/// is not its own.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotificationRequest {
    /// Stable type key of the registered notification (`appointment.booked`).
    /// Maps to `notifications.type` and the `notifications.<key>` view.
    pub event: String,
    /// Template vars. D-17 nudge-not-payload: the producer's own fields,
    /// never free-form data the module made up at call time.
    #[serde(default)]
    pub payload: Json,
}

/// A delivery future for one [`NotificationRequest`].
///
/// Boxed and `'static` because the Database channel writes rows through an
/// async store port, so delivery cannot be synchronous — the same reason a
/// first-party handler returns a `'static` future rather than borrowing its
/// unit.
pub type DeliveryFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), AppError>> + Send + 'static>>;

/// The delivery side of a [`NotificationRequest`], filled in by the host and
/// never by the module.
///
/// The principal, the tenant and the locale are the executing context's own,
/// taken from the envelope the host built for the call — so a module cannot
/// notify as somebody else, under another tenant, in a locale the request
/// never negotiated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotificationEnvelope {
    pub principal_id: PrincipalId,
    pub tenant_id: Option<TenantId>,
    /// Catalog locale for every rendered string (`None` = catalog default).
    pub locale: Option<String>,
}

/// The notification half of the seam: what the host offers a module.
///
/// Owned here rather than in `kelson-wasm` for the same reason
/// [`ModelStore`] and [`TokenStore`] are: this is the name the extension
/// class gives the service it marshals across the import boundary, and the
/// implementations stay where they lived. `kelson-wasm` depends on this
/// crate and never on the notification service.
pub trait NotificationSink: Send + Sync {
    /// Deliver one notification the module declared, in the envelope the
    /// host determined.
    fn deliver(&self, request: NotificationRequest, envelope: NotificationEnvelope) -> DeliveryFuture;
}

/// `notify_enqueue(request)` — accepted for delivery, or a gate refusal.///
/// Same write gate as [`model_write`] and [`token_write`]: a read-only guest
/// may not make the host notify anyone, and the answer is the same
/// deliberate empty string, carrying no detail back to a guest that was not
/// supposed to be able to ask.
///
/// What crosses back is *acceptance*, not completion. Delivery is buffered
/// on the call and only performed once the whole call has succeeded, so a
/// handler that notifies and then fails leaves nothing behind — the same
/// all-or-nothing posture [`capability_write`] gives a mutation delta.
pub fn notify_enqueue(sink: Option<&dyn NotificationSink>, request: &str, can_write: bool) -> String {
    if !can_write {
        return String::new();
    }
    let Some(sink) = sink else {
        return unconfigured("notification sink");
    };
    // Acceptance is not completion: the sink is only here so that a boot
    // that forgot to wire one refuses loudly instead of letting the module
    // believe the notification went out.
    let _ = sink;
    let a = match arguments::<NotificationRequest>(request) {
        Ok(a) => a,
        Err(e) => return malformed("notify_enqueue", e),
    };
    if a.event.trim().is_empty() {
        return malformed(
            "notify_enqueue",
            AppError::internal("a notification must name the registered event it is"),
        );
    }
    SeamResult::Ok(Json::Null).encode()
}

// ---------------------------------------------------------------------------
// The dispatch envelope
// ---------------------------------------------------------------------------

/// The key the executing context travels under.
///
/// A guest has no `ctx` parameter: `OperationHandler::handle` receives one and
/// a guest entrypoint does not, so the host carries the context *inside* the
/// document it already sends. One name, declared here, is what stops the two
/// sides from agreeing on it by coincidence — which is exactly the failure a
/// guest reports as "execution context is not readable" and a host reports as
/// a module that mysteriously has no principal.
pub const ENVELOPE_CTX: &str = "__kelson_ctx";

/// The key the caller's own payload travels under, beside the context.
pub const ENVELOPE_INPUT: &str = "input";

/// The key a guest answers under when the call FAILED rather than when it
/// produced a document.
///
/// A bare JSON string is a perfectly good document, so encoding a rejection as
/// one makes it indistinguishable from success - which is how a module's every
/// refusal once reached the caller as a 200 carrying the error text. The value
/// is the whole [`kelson_foundation::AppError`], so nothing is lost crossing
/// the boundary but the stack.
pub const ENVELOPE_ERR: &str = "__kelson_err";

/// Wrap a payload and its executing context into the document a guest reads.
///
/// The host calls this; the guest reads the result back through
/// `kelson-module-sdk`'s `dispatch`, which looks for the same two keys. Both
/// halves name this crate, so a rename is one edit that cannot desynchronise.
pub fn encode_envelope(ctx: &serde_json::Value, payload: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        ENVELOPE_CTX: ctx,
        ENVELOPE_INPUT: payload,
    })
}

