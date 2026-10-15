//! Stage C module artifact: the workspace's first compiled application module.
//!
//! Kelson already had everything *around* a module runtime. `WasmHost::prepare`
//! compiles a guest, refuses manifest drift and hands back a staged plugin;
//! `packages.toml` already declares where an artifact and its reviewed sidecar
//! live; ADR 0019 §2 fixes the ABI v1 surface (`kelson_manifest`,
//! `kelson_alloc`, `kelson_call`). What never existed was an artifact to load —
//! no `packages.toml` shipped one, so `load_wasm_plugins` returned `Ok(None)`
//! on every boot and the whole path was dormant.
//!
//! This crate is the smallest thing that exercises that path end to end, and it
//! is deliberately its own dependency closure: Stage C.3 hashes the module
//! target's closure and shows the kernel executable is not in it, so a module
//! that pulled the runtime in would fail that check by construction. Hence no
//! dependencies at all, not even `serde` — the two JSON documents this guest
//! emits are written out literally so their bytes are visible in review.
//!
//! Behaviour changes without touching a source file, which is the point of the
//! runtime-change proof. The marker is compiled in from the environment, so two
//! builds of byte-identical source produce different artifacts:
//!
//! ```text
//! cargo build --release --target wasm32-wasip1 -p kelson-module-marker
//! KELSON_MODULE_MARKER=v2 cargo build --release --target wasm32-wasip1 -p kelson-module-marker
//! ```
//!
//! The correct target is `wasm32-wasip1`, not `wasm32-unknown-unknown`: the
//! host links a hand-rolled `wasi_snapshot_preview1` shim (ADR 0019) that
//! supplies `args`/`environ`/`clock`/`random`/`fd_write` and friends, which is
//! what a `std`-linked wasm32-wasip1 binary imports at start.

/// The marker this build embeds. Resolved at compile time, so it is part of the
/// artifact's bytes and not something a request can influence.
const MARKER: &str = match option_env!("KELSON_MODULE_MARKER") {
    Some(value) => value,
    None => "v1",
};

/// The manifest this guest declares, as the one JSON document `prepare` will
/// compare against the reviewed sidecar (`module.toml` beside this crate).
///
/// The comparison normalizes both sides through `ModuleManifest` and re-encodes
/// them, so this string does not have to *look* like the TOML — it has to
/// *parse to the same document*. That means every field whose sidecar value
/// differs from its serde default must appear here; fields left out take the
/// same default on both sides and therefore match. `routes` is the one that
/// bites: the sidecar declares one, so an empty `routes` here is drift and the
/// load is refused.
const MANIFEST_JSON: &str = r#"{"module":{"name":"marker","version":"0.1.0","maturity":"L0","plane":"tenant","description":"Stage C activation marker","api":1},"capabilities":{"read":{"profile":"READ"}},"dependencies":{"contracts":[]},"routes":[{"method":"GET","path":"/api/v1/marker","operation":"read","capability":"read","name":"read","public":true}]}"#;

/// Linear memory the host writes one call's operation and input into.
///
/// `kelson_alloc` is called exactly once per dispatch (`dispatch_v1` copies
/// `op ‖ input` through it), so at most one buffer from this arena is live at a
/// time — the cursor may therefore wrap and reuse from the start without ever
/// aliasing a region the host is still writing.
const ARENA_LEN: usize = 64 * 1024;

/// The arena itself. It is never written through a Rust reference: the host
/// writes it through linear memory, and this module only ever takes its
/// address, so it stays a plain (immutable) static and `addr_of!` can form the
/// pointer without tripping `static_mut_refs`.
static ARENA: [u8; ARENA_LEN] = [0u8; ARENA_LEN];

/// Cursor into [`ARENA`] for the next [`kelson_alloc`].
static mut ARENA_CURSOR: usize = 0;

/// Where [`kelson_call`]'s result is published. The host reads it immediately
/// after the call returns, so one shared buffer serves every call and the
/// guest never leaks a response.
const RESPONSE_LEN: usize = 512;
static mut RESPONSE: [u8; RESPONSE_LEN] = [0u8; RESPONSE_LEN];

/// Pack a `(ptr, len)` pair the way ABI v1 reads it back: `(ptr << 32) | len`.
///
/// Masked rather than assumed: `usize` is 64-bit in the host build this crate
/// is still linted and tested in, and a shift of a full pointer would overflow
/// there. ABI v1 pointers are 32-bit either way.
fn pack(ptr: usize, len: usize) -> i64 {
    (((ptr as u64 & 0xFFFF_FFFF) << 32) | (len as u64 & 0xFFFF_FFFF)) as i64
}

/// Base address of the arena in this instance's linear memory.
fn arena_start() -> usize {
    core::ptr::addr_of!(ARENA) as usize
}

/// Copy `body` into the response buffer and pack it for the host.
fn respond(body: &str) -> i64 {
    let bytes = body.as_bytes();
    if bytes.len() > RESPONSE_LEN {
        return 0;
    }
    // SAFETY: `addr_of_mut!` forms the raw pointer without creating a
    // reference to the `static mut`, nothing else holds a pointer into this
    // buffer, and the host cannot observe it until `kelson_call` returns.
    unsafe {
        let dst = core::ptr::addr_of_mut!(RESPONSE) as *mut u8;
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, bytes.len());
        pack(dst as usize, bytes.len())
    }
}

/// The manifest this guest ships, packed for `kelson_manifest`.
///
/// It lives in the data section, so the pointer stays valid for the life of the
/// instance and nothing needs to allocate to answer.
#[no_mangle]
pub extern "C" fn kelson_manifest() -> i64 {
    pack(MANIFEST_JSON.as_ptr() as usize, MANIFEST_JSON.len())
}

/// Hand the host `len` bytes of this instance's linear memory to write one
/// call's `op ‖ input` into.
///
/// Returns null rather than a pointer when the request does not fit: the host
/// treats a non-positive result as out-of-memory and refuses the call, which is
/// the fail-closed answer for an over-large input.
#[no_mangle]
pub extern "C" fn kelson_alloc(len: i32) -> i32 {
    let Ok(len) = usize::try_from(len) else {
        return 0;
    };
    if len == 0 || len > ARENA_LEN {
        return 0;
    }
    // SAFETY: `addr_of_mut!` forms a raw pointer without creating a reference
    // to the `static mut`, and this cursor has no other writer — one guest
    // thread runs one call at a time inside its own instance.
    unsafe {
        let cursor = core::ptr::addr_of_mut!(ARENA_CURSOR);
        let mut at = *cursor;
        if at + len > ARENA_LEN {
            at = 0;
        }
        *cursor = at + len;
        (arena_start() + at) as i32
    }
}

/// Read a region the host wrote through [`kelson_alloc`].
///
/// Empty when the pointer falls outside the arena: only `kelson_alloc` ever
/// hands out addresses, so anything else is a host bug, and answering for
/// memory we did not receive would hide it behind a wrong answer.
fn read_input(ptr: i32, len: i32) -> &'static [u8] {
    if ptr <= 0 || len < 0 {
        return &[];
    }
    let base = arena_start();
    let Some(end) = base.checked_add(ARENA_LEN) else {
        return &[];
    };
    let from = ptr as usize;
    let Some(to) = from.checked_add(len as usize) else {
        return &[];
    };
    if from < base || to > end {
        return &[];
    }
    // SAFETY: bounds-checked above to lie entirely inside the arena, which
    // outlives this call (the arena is a static of the running instance) and
    // which the host finished writing before `kelson_call` began.
    unsafe { core::slice::from_raw_parts(from as *const u8, to - from) }
}

/// Dispatch one operation: the marker's whole behaviour.
///
/// The module takes no payload of its own — it reports the build marker, the
/// operation it was called for, and how many input bytes arrived, so the round
/// trip through `kelson_alloc` is visible in the answer instead of assumed.
/// Swapping `MARKER` and rebuilding is the change the runtime-change proof
/// captures: different artifact bytes, different answer, untouched kernel.
#[no_mangle]
pub extern "C" fn kelson_call(op_ptr: i32, op_len: i32, in_ptr: i32, in_len: i32) -> i64 {
    let Some(op) = core::str::from_utf8(read_input(op_ptr, op_len)).ok().filter(|s| !s.is_empty())
    else {
        // An operation that is not UTF-8, not in the arena, or empty is not a
        // call to answer: refuse with no body rather than guess.
        return 0;
    };
    let received = read_input(in_ptr, in_len).len();
    let body = format!(
        r#"{{"marker":"{MARKER}","operation":"{op}","received":{received}}}"#
    );
    respond(&body)
}
