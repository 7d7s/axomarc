//! The guest's half of the module ABI — ADR 0020.
//!
//! A module compiled to `wasm32-wasip1` keeps every line of its business
//! logic and gains exactly one thing: a different implementation behind the
//! contract traits it already names. `OauthClientStore` still holds a
//! `Arc<dyn ModelStore>` and still drives `ModelRuntime`; what changed is
//! that the trait object behind it answers by asking the host instead of
//! reading a map in this instance's memory.
//!
//! That is the whole design, and it is why the port of 2,381 lines of module
//! code is entry points and a backend rather than a rewrite: the contract
//! crates (`kelson-models`, `kelson-execution`, `kelson-tokens`) compile for
//! the guest target, so the traits a module names are the *same traits* on
//! both sides of the seam.
//!
//! # What this crate supplies
//!
//! * [`model::HostModelStore`] and [`token::HostTokenStore`] — the
//!   host-backed implementations of `ModelStore` and `TokenStore`;
//! * [`unit::GuestUnit`] — an `ExecutionUnit` for a guest, which has no
//!   engine behind it and buffers what it is told to record;
//! * [`dispatch`] and the export helpers — the arena, the packing convention
//!   and the envelope split, written once;
//! * [`guest_entry`] — a macro that emits the four `#[no_mangle]` exports so a
//!   module writes a dispatch table and nothing else.
//!
//! # The seam
//!
//! Four imports, split by capability rather than by trait method
//! (`crates/extension/wasm/src/imports.rs`), so a write is reachable only
//! through an import that checks the guest's capability profile:
//!
//! ```text
//! host.model_read(method, args)   host.model_write(method, args)
//! host.token_read(method, args)   host.token_write(method, args)
//! ```
//!
//! Arguments and answers are both JSON, and the shapes are shared with the
//! host through [`kelson_module_contract`] — this crate names the same
//! argument structs the host decodes, so there is no positional convention
//! to keep in step across a seam that is compiled from two different graphs.
//!
//! An import answers three ways and only three. `""` is the write gate
//! saying no, and it carries nothing — a guest cannot tell "denied" from
//! "no such record", which is the point, since that distinction is an
//! oracle. `{"ok": <value>}` is the store answering. `{"err": {...}}` is
//! the store failing, with the [`AppError`] intact so a module returns the
//! same status code it would have running in-process.
//!
//! [`AppError`]: kelson_foundation::AppError

use std::sync::Arc;

use kelson_context::ExecutionContext;
use serde_json::Value;

pub mod model;
pub mod token;
pub mod unit;

pub use model::HostModelStore;
pub use token::HostTokenStore;
pub use unit::GuestUnit;

/// Largest payload the guest will hand the host in one argument. Guards the
/// guest against its own runaway encoding; real arguments are JSON-sized.
const MAX_ARG: usize = 1 << 20; // 1 MiB

/// The input arena: the host writes one dispatch's `op ‖ input` here through
/// [`alloc`]. One buffer is live at a time, so the cursor may wrap.
const ARENA_LEN: usize = 256 * 1024;

/// The return arena: the host allocates *here* through [`response_alloc`]
/// when an import has to hand a string back mid-call. Separate from the
/// input arena because an import loop can produce many returns inside a
/// single dispatch, and the input arena's one-buffer contract must stay true.
const RETURN_LEN: usize = 256 * 1024;

/// Response buffer for the dispatch's own answer, read once after
/// `kelson_call` returns.
const RESPONSE_LEN: usize = 64 * 1024;

static ARENA: [u8; ARENA_LEN] = [0u8; ARENA_LEN];
static mut ARENA_CURSOR: usize = 0;

static RETURN_ARENA: [u8; RETURN_LEN] = [0u8; RETURN_LEN];
static mut RETURN_CURSOR: usize = 0;

static mut RESPONSE: [u8; RESPONSE_LEN] = [0u8; RESPONSE_LEN];

/// Pack a `(ptr, len)` pair the way the host reads it back:
/// `(ptr << 32) | len`. Masked rather than assumed, because this crate is
/// also linted and built for the host where `usize` is 64-bit.
pub fn pack(ptr: usize, len: usize) -> i64 {
    (((ptr as u64 & 0xFFFF_FFFF) << 32) | (len as u64 & 0xFFFF_FFFF)) as i64
}

/// Base address of a static in this instance's linear memory.
fn base_of(bytes: &[u8]) -> usize {
    bytes.as_ptr() as usize
}

/// Hand the host `len` bytes to write one dispatch's `op ‖ input` into.
///
/// Zero on refusal — a non-positive result is out-of-memory, and the host
/// refuses the call rather than guessing. That is the fail-closed answer for
/// an over-large input.
pub fn alloc(len: i32) -> i32 {
    // SAFETY: `addr_of_mut!` forms a raw pointer without creating a reference
    // to the `static mut`; `arena_alloc` carries the single-writer argument
    // that keeps the pointer valid for the whole write.
    unsafe { arena_alloc(&ARENA, core::ptr::addr_of_mut!(ARENA_CURSOR), len) }
}

/// Hand the host `len` bytes to write one import's return value into.
///
/// Called from inside an import, which is why the return arena is separate:
/// the input arena's cursor is spoken for by the dispatch in flight.
pub fn response_alloc(len: i32) -> i32 {
    // SAFETY: as `alloc`, with its own cursor and its own arena.
    unsafe { arena_alloc(&RETURN_ARENA, core::ptr::addr_of_mut!(RETURN_CURSOR), len) }
}

/// Bump-allocate from one arena.
///
/// # Safety
///
/// `cursor` must point at the arena's own cursor, and no other writer may
/// touch it. That holds here by construction: one guest thread runs one call
/// at a time inside its own instance, and `kelson_alloc` /
/// `kelson_response_alloc` are each called by the host at most once per
/// import.
unsafe fn arena_alloc(bytes: &[u8], cursor: *mut usize, len: i32) -> i32 {
    let Ok(len) = usize::try_from(len) else {
        return 0;
    };
    if len == 0 || len > bytes.len() {
        return 0;
    }
    unsafe {
        let mut at = cursor.read();
        if at + len > bytes.len() {
            at = 0;
        }
        cursor.write(at + len);
        (base_of(bytes) + at) as i32
    }
}

/// Read a region the host wrote through [`alloc`] / [`response_alloc`].
///
/// Empty when the pointer falls outside the arena named: only the matching
/// allocator ever hands out addresses, so anything else is a host bug, and
/// answering for memory we did not receive would hide it behind a wrong
/// answer.
fn read_arena(arena: &[u8], ptr: i32, len: i32) -> &'static [u8] {
    if ptr <= 0 || len < 0 {
        return &[];
    }
    let base = base_of(arena);
    let Some(end) = base.checked_add(arena.len()) else {
        return &[];
    };
    let from = ptr as usize;
    let Some(to) = from.checked_add(len as usize) else {
        return &[];
    };
    if from < base || to > end {
        return &[];
    }
    // SAFETY: bounds-checked to lie entirely inside a `'static` arena, which
    // outlives this call and which the host finished writing before we read.
    unsafe { core::slice::from_raw_parts(from as *const u8, to - from) }
}

/// Read a region the host wrote into the input arena.
fn read_input(ptr: i32, len: i32) -> &'static [u8] {
    read_arena(&ARENA, ptr, len)
}

/// Read a region the host wrote into the return arena.
///
/// Only reachable where the imports exist: on a host build nothing writes
/// into this arena, because nothing can call back into the guest.
#[cfg(target_arch = "wasm32")]
fn read_return(ptr: i32, len: i32) -> &'static [u8] {
    read_arena(&RETURN_ARENA, ptr, len)
}

/// Copy `body` into the dispatch response buffer and pack it for the host.
fn respond(body: &str) -> i64 {
    let bytes = body.as_bytes();
    if bytes.len() > RESPONSE_LEN {
        return 0;
    }
    // SAFETY: `addr_of_mut!` forms the raw pointer without creating a
    // reference, nothing else holds a pointer into this buffer, and the host
    // cannot observe it until `kelson_call` returns.
    unsafe {
        let dst = core::ptr::addr_of_mut!(RESPONSE) as *mut u8;
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, bytes.len());
        pack(dst as usize, bytes.len())
    }
}

// ---------------------------------------------------------------------------
// The four host imports
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "host")]
extern "C" {
    fn log(ptr: u32, len: u32);
    fn capability_check(action_ptr: u32, action_len: u32, path_ptr: u32, path_len: u32) -> i32;
    fn capability_write(action_ptr: u32, action_len: u32, delta_ptr: u32, delta_len: u32) -> i32;
    fn capability_audit(action_ptr: u32, action_len: u32, record_ptr: u32, record_len: u32) -> i32;
    fn model_read(method_ptr: u32, method_len: u32, args_ptr: u32, args_len: u32) -> i64;
    fn model_write(method_ptr: u32, method_len: u32, args_ptr: u32, args_len: u32) -> i64;
    fn token_read(method_ptr: u32, method_len: u32, args_ptr: u32, args_len: u32) -> i64;
    fn token_write(method_ptr: u32, method_len: u32, args_ptr: u32, args_len: u32) -> i64;
    fn notify_enqueue(request_ptr: u32, request_len: u32) -> i32;
}

/// Which of the four import families a call reaches, and therefore which
/// capability posture the host applies to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Port {
    /// Reads: gated the way `capability_check` gates.
    ModelRead,
    /// Writes: gated the way `capability_write` gates, and buffered.
    ModelWrite,
    /// Token reads.
    TokenRead,
    /// Token writes.
    TokenWrite,
}

/// Issue one import call and decode the answer.
///
/// The import answers one of three ways, and only the third is an error the
/// guest can act on:
///
/// * `""` — the import gate refused (a write from a read-only guest), or the
///   host could not deliver an answer at all. Both are refusals to a module,
///   and neither can be told apart from here — deliberately, so a guest
///   learns nothing from a gate that said no.
/// * `{"ok": <value>}` — the store answered, and this returns the value.
/// * `{"err": {...}}` — the store failed, and the [`AppError`] crosses whole:
///   the module answers its own caller with the status code it would have
///   produced running in-process, which is the property the whole port is
///   there to preserve.
pub fn host_call(
    port: Port,
    method: &str,
    args: &str,
) -> Result<serde_json::Value, kelson_foundation::AppError> {
    if method.is_empty() {
        return Err(kelson_foundation::AppError::validation(
            "module sdk: empty method name",
        ));
    }
    if args.len() > MAX_ARG {
        return Err(kelson_foundation::AppError::validation(
            "module sdk: argument payload too large",
        ));
    }

    #[cfg(target_arch = "wasm32")]
    {
        let out = unsafe {
            let m = method.as_bytes();
            let a = args.as_bytes();
            match port {
                Port::ModelRead => model_read(m.as_ptr() as u32, m.len() as u32, a.as_ptr() as u32, a.len() as u32),
                Port::ModelWrite => {
                    model_write(m.as_ptr() as u32, m.len() as u32, a.as_ptr() as u32, a.len() as u32)
                }
                Port::TokenRead => {
                    token_read(m.as_ptr() as u32, m.len() as u32, a.as_ptr() as u32, a.len() as u32)
                }
                Port::TokenWrite => {
                    token_write(m.as_ptr() as u32, m.len() as u32, a.as_ptr() as u32, a.len() as u32)
                }
            }
        };
        let ptr = (out >> 32) as i32;
        let len = (out & 0xFFFF_FFFF) as i32;
        if len <= 0 {
            return Err(kelson_foundation::AppError::validation(format!(
                "module sdk: host refused {method}"
            )));
        }
        let body = read_return(ptr, len);
        let text = String::from_utf8_lossy(body).into_owned();
        match kelson_module_contract::SeamResult::decode(&text)? {
            kelson_module_contract::SeamResult::Ok(value) => Ok(value),
            kelson_module_contract::SeamResult::Err(err) => Err(err),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (port, method, args);
        Err(kelson_foundation::AppError::validation(
            "module sdk: host imports exist only in a wasm32 guest",
        ))
    }
}

/// Encode one call's arguments and issue it.
///
/// Both sides build the same [`kelson_module_contract`] argument struct, so
/// there is no positional convention to keep in step across the seam.
pub fn call<T: serde::Serialize>(
    port: Port,
    method: &str,
    args: &T,
) -> Result<serde_json::Value, kelson_foundation::AppError> {
    let payload = serde_json::to_string(args).map_err(|e| {
        kelson_foundation::AppError::internal(format!("module sdk: arguments are not encodable: {e}"))
    })?;
    host_call(port, method, &payload)
}

/// Structured log line into the host's plugin-tagged logger.
pub fn log_line(message: &str) {
    #[cfg(target_arch = "wasm32")]
    unsafe {
        let b = message.as_bytes();
        log(b.as_ptr() as u32, b.len() as u32);
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = message;
}

/// Pre-dispatch authorization probe against the sidecar-declared capability
/// set. False when the host has no scope for this execution — fail-closed,
/// never a trap.
pub fn capability_allowed(action: &str, path: &str) -> bool {
    #[cfg(target_arch = "wasm32")]
    unsafe {
        let a = action.as_bytes();
        let p = path.as_bytes();
        capability_check(
            a.as_ptr() as u32,
            a.len() as u32,
            p.as_ptr() as u32,
            p.len() as u32,
        ) != 0
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (action, path);
        false
    }
}

/// Declare one mutation delta to the host.
///
/// This is not the seam: a `MutationRecord` a handler wrote through the
/// execution unit is *record*, not application — the host applies the same
/// buffer to its own unit after a successful call and discards it on failure.
/// `false` means the host refused it (undeclared action, malformed record,
/// buffer full), and the caller must treat that as a failure rather than
/// carry on with one fewer effect than the handler recorded.
pub fn record_write(action: &str, delta: &serde_json::Value) -> bool {
    emit("capability_write", action, delta)
}

/// Declare one audit entry to the host. Every profile may record, including
/// a read-only one: accountability is append-only and is not a mutation.
pub fn record_audit(action: &str, entry: &serde_json::Value) -> bool {
    emit("capability_audit", action, entry)
}

/// Declare one notification the module wants delivered (ADR 0020 §5).
///
/// `request` is a [`kelson_module_contract::NotificationRequest`]: the name
/// of a notification this module registered, plus the producer's template
/// vars. It is *intent*, not a rendered message — the host resolves the
/// def, the catalog, the locale and the channels, because that is where
/// those live.
///
/// Unlike a mutation, the host buffers this and only delivers once the whole
/// call has succeeded, so a handler that notifies and then fails leaves
/// nothing behind. Call it on the success path, after the writes that the
/// notification is about have been accepted. `false` means the host refused
/// it (a read-only profile, or a host wired with no notification sink), and
/// the caller must treat that as a failure rather than carry on as though
/// the notification went out.
pub fn record_notification(request: &serde_json::Value) -> bool {
    emit("notify_enqueue", "", request)
}

#[cfg(target_arch = "wasm32")]
fn emit(import: &str, action: &str, payload: &serde_json::Value) -> bool {
    let Ok(payload) = serde_json::to_string(payload) else {
        return false;
    };
    let (a, p) = (action.as_bytes(), payload.as_bytes());
    let (a_ptr, a_len) = (a.as_ptr() as u32, a.len() as u32);
    let (p_ptr, p_len) = (p.as_ptr() as u32, p.len() as u32);
    unsafe {
        let verdict = match import {
            "capability_write" => capability_write(a_ptr, a_len, p_ptr, p_len),
            "notify_enqueue" => notify_enqueue(p_ptr, p_len),
            _ => capability_audit(a_ptr, a_len, p_ptr, p_len),
        };
        verdict != 0
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn emit(import: &str, action: &str, payload: &serde_json::Value) -> bool {
    // No host behind an import on this build. Refusing is the honest answer
    // rather than pretending the record was delivered: the whole point of the
    // return value is that a caller must not continue as though it were.
    let _ = (import, action, payload);
    false
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

/// The envelope the host writes an operation's payload in.
///
/// The `ExecutionContext` rides inside the input rather than behind a
/// `context_*` import family: it is serde-serializable already, one field
/// costs nothing, and a second import family would have to be versioned for
/// a type that already has a wire shape.
const ENVELOPE_CTX: &str = kelson_module_contract::ENVELOPE_CTX;
const ENVELOPE_INPUT: &str = kelson_module_contract::ENVELOPE_INPUT;

/// Run one operation against the module's handlers.
///
/// `input` is the host's envelope; the returned `Value` is what the module
/// answers. Everything between is the part a module would otherwise
/// re-implement: split the envelope, rebuild the context, find the handler,
/// give it a unit, and turn its answer back into bytes.
pub fn dispatch(
    handlers: &[Arc<dyn OperationHandler>],
    op: &str,
    input: &[u8],
) -> Result<Value, kelson_foundation::AppError> {
    let raw = std::str::from_utf8(input).map_err(|_| {
        kelson_foundation::AppError::validation("module sdk: input is not UTF-8")
    })?;
    let envelope: Value = serde_json::from_str(raw)
        .map_err(|_| kelson_foundation::AppError::validation("module sdk: input is not JSON"))?;

    let (ctx_json, payload) = match envelope.get(ENVELOPE_CTX) {
        Some(ctx) => (
            ctx.clone(),
            envelope
                .get(ENVELOPE_INPUT)
                .cloned()
                .unwrap_or(Value::Null),
        ),
        // An envelope without a context is still dispatchable: the payload is
        // the whole document. A module that needs a principal checks for one.
        None => (Value::Null, envelope),
    };

    let ctx: ExecutionContext = serde_json::from_value(ctx_json).map_err(|_| {
        kelson_foundation::AppError::validation("module sdk: execution context is not readable")
    })?;
    let ctx = Arc::new(ctx);

    let handler = handlers
        .iter()
        .find(|h| h.plan().operation.as_str() == op)
        .ok_or_else(|| kelson_foundation::AppError::not_found(op))?;

    let mut unit = unit::GuestUnit::for_context(&ctx);
    let future = handler.handle(ctx, payload, &mut unit);
    // One call, one instance, one thread: the future is polled to completion
    // here rather than handed to an executor a guest does not have.
    let result = poll(future);

    // The handler recorded its effects into THIS unit, which is not the
    // engine's and dies with this call — nothing else carries them home. So a
    // success is the moment they go to the host, through the same two imports
    // the marker uses and the same buffer `WasmHandler` drains after a
    // successful call and drops on a failed one.
    //
    // A record the host will not take fails the call. The handler wrote it
    // because it happened, and a body saying "created" beside an effect log
    // that does not contain the creation is worse than an error.
    #[cfg(target_arch = "wasm32")]
    if result.is_ok() {
        replay_effects(capability_of(op), &unit)?;
    }
    result
}

/// The capability a qualified operation names.
///
/// The host's declared set names CAPABILITIES ("book"), because that is what a
/// sidecar can review and refuse drift over; the operation it dispatches is
/// the qualified route ("appointments.book"), because that is what a caller
/// names. Routes are `{module}.{capability}`, so the capability is the last
/// segment - and naming it here means the guest asks the host for exactly what
/// was reviewed instead of for a string the host has to guess about. Guessing
/// cost this port every delta a module recorded: the write was refused, the
/// unit came back empty, and the engine reported a successful mutation
/// operation that had mutated nothing.
#[cfg(target_arch = "wasm32")]
fn capability_of(operation: &str) -> &str {
    operation.rsplit('.').next().unwrap_or(operation)
}

/// Hand a completed handler's unit records to the host.
///
/// Two channels exist and no third is invented here:
///
/// * mutations become `capability_write` deltas in the shape
///   `decode_mutation` reads, and audits become `capability_audit` entries in
///   the shape `decode_audit` reads — both schemas are field-for-field the
///   record types, so nothing is translated, only named.
/// * `MutationRecord::stream` does not cross, because the delta schema has
///   never had a field for it (the host hardcodes `None` and falls back to the
///   capability's own plan). That is a pre-existing limit of the import, not
///   something this path widens.
/// * `OutboxMessage`s do not cross either: there is no outbox import, and
///   inventing one mid-port would be a second ABI to get wrong. A module that
///   queues outbox work is not yet fully ported, and that is recorded in the
///   SDK rather than hidden by an empty success.
#[cfg(target_arch = "wasm32")]
fn replay_effects(
    action: &str,
    unit: &unit::GuestUnit,
) -> Result<(), kelson_foundation::AppError> {
    let refused = |what: &str, index: usize| {
        kelson_foundation::AppError::internal(format!(
            "module sdk: the host refused the {what} this handler recorded at index {index}"
        ))
    };
    for (i, m) in unit.mutations().iter().enumerate() {
        let delta = serde_json::json!({
            "resource_type": m.resource_type,
            "resource_id": m.resource_id.map(|id| id.to_string()),
            "resource_version": m.resource_version,
            "before": m.before,
            "after": m.after,
        });
        if !record_write(action, &delta) {
            return Err(refused("mutation", i));
        }
    }
    for (i, a) in unit.audits().iter().enumerate() {
        let entry = serde_json::json!({
            "action": a.action,
            "resource": a.resource,
            "authority": a.authority,
            "outcome": match a.outcome {
                kelson_execution::AuditOutcome::Success => "success",
                kelson_execution::AuditOutcome::Denied => "denied",
                kelson_execution::AuditOutcome::Failed => "failed",
            },
            "detail": a.detail,
        });
        if !record_audit(action, &entry) {
            return Err(refused("audit", i));
        }
    }
    Ok(())
}

/// Drive a `'static` future to completion inside a guest.
///
/// A WASM guest has no reactor, and every in-tree handler's future is
/// synchronous work wrapped in `BoxFuture`. Polling once per wake is the
/// whole of the executor this crate needs — and it is honest about being
/// that, rather than pretending to be a runtime it is not.
fn poll(future: kelson_execution::BoxFuture<'static, Result<Value, kelson_foundation::AppError>>) -> Result<Value, kelson_foundation::AppError> {
    use std::future::Future;
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    // A guest is single-threaded and this future is driven by its own code:
    // a waker that never wakes is correct for futures that are ready on the
    // first poll, and a future that genuinely suspends cannot be driven at
    // all here — which is the same reason notifications are out of ADR 0020.
    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    let mut future = pin!(future);
    match future.as_mut().poll(&mut cx) {
        Poll::Ready(result) => result,
        Poll::Pending => Err(kelson_foundation::AppError::validation(
            "module sdk: a handler suspended, and a guest has no reactor",
        )),
    }
}

/// Emit the module's `kelson_call` body: answer the dispatch and encode the
/// result for the host.
pub fn encode_call(
    handlers: &[Arc<dyn OperationHandler>],
    op_ptr: i32,
    op_len: i32,
    in_ptr: i32,
    in_len: i32,
) -> i64 {
    let Some(op) = core::str::from_utf8(read_input(op_ptr, op_len))
        .ok()
        .filter(|s| !s.is_empty())
    else {
        // An operation that is not UTF-8, not in the arena, or empty is not a
        // call to answer: refuse with no body rather than guess.
        return 0;
    };
    let input = read_input(in_ptr, in_len);
    let encoded = match dispatch(handlers, op, input) {
        Ok(value) => serde_json::to_string(&value),
        // A failure is answered in its OWN envelope, never as bare text: a
        // bare document is a successful body, so an error encoded as a string
        // makes every rejection this module raises look like a 200.
        Err(err) => serde_json::to_string(&serde_json::json!({
            kelson_module_contract::ENVELOPE_ERR: err,
        })),
    };
    match encoded {
        Ok(body) => respond(&body),
        Err(_) => 0,
    }
}

/// Packed pointer to the module's manifest, for `kelson_manifest`.
pub fn manifest_answer(manifest: &'static str) -> i64 {
    pack(manifest.as_ptr() as usize, manifest.len())
}

/// Build the four `#[no_mangle]` exports every module needs.
///
/// A module writes a dispatch table and a manifest constant; the arena, the
/// packing convention and the envelope are this crate's job, written once.
///
/// ```ignore
/// kelson_module_sdk::guest_entry! {
///     manifest: MANIFEST_JSON,
///     handlers: build_handlers,
/// }
/// ```
///
/// `build_handlers` is `fn() -> Vec<Arc<dyn OperationHandler>>` and is where
/// a module wires its own stores.
#[macro_export]
macro_rules! guest_entry {
    { manifest: $manifest:expr, handlers: $build:expr } => {
        /// The manifest this guest declares; `prepare` compares it against
        /// the reviewed sidecar and refuses drift.
        #[no_mangle]
        pub extern "C" fn kelson_manifest() -> i64 {
            $crate::manifest_answer($manifest)
        }

        /// Hand the host `len` bytes of this instance's linear memory for one
        /// dispatch's `op ‖ input`.
        #[no_mangle]
        pub extern "C" fn kelson_alloc(len: i32) -> i32 {
            $crate::alloc(len)
        }

        /// Hand the host `len` bytes for one import's return value.
        #[no_mangle]
        pub extern "C" fn kelson_response_alloc(len: i32) -> i32 {
            $crate::response_alloc(len)
        }

        /// Dispatch one operation: the module's whole behaviour.
        #[no_mangle]
        pub extern "C" fn kelson_call(
            op_ptr: i32,
            op_len: i32,
            in_ptr: i32,
            in_len: i32,
        ) -> i64 {
            static HANDLERS: ::std::sync::OnceLock<
                ::std::vec::Vec<::std::sync::Arc<dyn $crate::OperationHandler>>,
            > = ::std::sync::OnceLock::new();
            let handlers = HANDLERS.get_or_init($build);
            $crate::encode_call(handlers, op_ptr, op_len, in_ptr, in_len)
        }
    };
}

/// Re-exported so a module and [`guest_entry!`] can name the trait without
/// each spelling the dependency itself.
pub use kelson_execution::OperationHandler;
