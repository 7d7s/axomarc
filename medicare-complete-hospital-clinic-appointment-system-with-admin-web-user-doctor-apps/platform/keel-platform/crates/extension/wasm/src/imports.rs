//! The fail-closed import surface for WASM plugins (Wave 9 B-30).
//!
//! Two module namespaces are linked, and nothing else:
//!
//! * `wasi_snapshot_preview1` — a hand-rolled, deliberately minimal shim
//!   (fd_write/proc_exit/args/environ/clock/random/sched_yield/fd_close).
//!   No sockets, no path opens, no polling: a guest importing anything the
//!   shim does not define (e.g. `sock_recv`) fails at **link** time, which
//!   `WasmHost::load` turns into a boot refusal rather than a runtime
//!   surprise (the D-52 / STAGE-0-SPEC fail-closed posture).
//! * `host` — the plugin primitives: `host.log` (structured log line),
//!   `host.capability_check(action, path)` (pre-dispatch authorization
//!   probe against the sidecar-declared capability set; see [`CallState`]),
//!   `host.capability_write(action, delta)` (record a mutation delta —
//!   ReadWrite/Admin profiles only) and `host.capability_audit(action,
//!   record)` (record an audit entry — every profile). The write imports
//!   BUFFER into the per-call [`CallState`]; the handler commits the
//!   buffers through the execution unit after a successful call and
//!   discards them when the guest fails.
//!
//! `fd_write` only writes to stdout/stderr; env/args are empty by design
//! (plugins get no ambient environment).

use std::io::Write;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use wasmtime::{Caller, Extern, Linker, Memory};

use crate::{CallState, WasmError};

// WASI preview1 errno subset (only the codes this shim can return).
const ERRNO_SUCCESS: i32 = 0;
const ERRNO_TOOBIG: i32 = 1;
const ERRNO_BADF: i32 = 8;
const ERRNO_FAULT: i32 = 21;

/// Largest guest string the `host.*` imports will copy (guards the host
/// against a bogus pointer/length pair; real inputs are JSON-sized).
const MAX_GUEST_STR: u32 = 1 << 20; // 1 MiB

/// Most mutation/audit records one guest call may buffer. Bounds the
/// host-side memory a guest can force; the 65th write refuses (returns 0)
/// instead of growing the buffers.
const MAX_CALL_EFFECTS: usize = 64;

fn memory_of(caller: &mut Caller<'_, CallState>) -> Option<Memory> {
    match caller.get_export("memory") {
        Some(Extern::Memory(memory)) => Some(memory),
        _ => None,
    }
}

fn read_guest_str(
    memory: &Memory,
    caller: &mut Caller<'_, CallState>,
    ptr: u32,
    len: u32,
) -> Option<String> {
    if len > MAX_GUEST_STR {
        return None;
    }
    let mut buf = vec![0u8; len as usize];
    memory.read(caller, ptr as usize, &mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

// ---------------------------------------------------------------------------
// The four host primitives, written ONCE.
// ---------------------------------------------------------------------------
//
// Two front doors reach these methods:
//
// * **ABI v1** — `host.*` registrations below, whose closures first copy the
//   arguments out of the guest's linear memory and then hand over `&str`.
// * **ABI v2** — the generated `kelson:plugin::host::Host` trait at the bottom
//   of this file, whose arguments the Component Model canonical ABI has
//   already lifted into owned `String`s.
//
// Both call the same four methods, so the two generations cannot disagree
// about what a capability verdict *means*. That is the mirror ADR 0019 §5
// insisted on preserving (including the `%record` spelling, which exists so
// v2's `record` parameter keeps v1's name rather than being renamed and the
// mirror quietly broken). Drift between the generations is now structurally
// impossible rather than merely tested-for.
//
// Every method **answers** and none ever traps: a denial, an unparseable
// payload or a full buffer is `false`. A guest can therefore never turn a
// refusal into a host crash, on either ABI.
impl CallState {
    /// `log(message)` — one structured line, tagged with the plugin label.
    pub(crate) fn host_log(&self, message: &str) {
        tracing::info!(target: "kelson::wasm", plugin = %self.label, message = %message);
    }

    /// `capability_check(action, path)` — allow?
    ///
    /// Verdict = `action` is in the sidecar-verified capability set (a plugin
    /// can never elevate itself), AND a non-empty `path` must sit inside the
    /// execution's resolved scope chain. No scope means deny — fail-closed,
    /// never a trap.
    pub(crate) fn host_capability_check(&self, action: &str, path: &str) -> bool {
        if !self.declared.contains(action) {
            return false;
        }
        if path.is_empty() {
            return true;
        }
        if !self.has_scope {
            return false;
        }
        self.scope_paths
            .iter()
            .any(|p| *p == path || p.starts_with(&format!("{path}|")))
    }

    /// `capability_write(action, delta)` — recorded?
    ///
    /// Refuses unless ALL hold: the executing capability runs a
    /// ReadWrite/Admin profile (read-only guests can never write, even when
    /// they import the function), `action` is in the sidecar-verified declared
    /// set (no self-elevation), `delta` parses as a JSON object (the handler
    /// validates the shape), and the per-call buffer is not full.
    pub(crate) fn host_capability_write(&mut self, action: &str, delta: &str) -> bool {
        if !self.profile.can_write() {
            return false;
        }
        if !self.declared.contains(action) {
            return false;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(delta) else {
            return false;
        };
        if !value.is_object() {
            return false;
        }
        if self.deltas.len() >= MAX_CALL_EFFECTS {
            return false;
        }
        self.deltas.push(value);
        true
    }

    /// `capability_audit(action, record)` — recorded?
    ///
    /// Audit is append-only, so EVERY profile may record (including
    /// ReadOnly); `action` must still be declared and `record` must parse as
    /// a JSON object. Same buffer bound as writes.
    pub(crate) fn host_capability_audit(&mut self, action: &str, record: &str) -> bool {
        if !self.declared.contains(action) {
            return false;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(record) else {
            return false;
        };
        if !value.is_object() {
            return false;
        }
        if self.audits.len() >= MAX_CALL_EFFECTS {
            return false;
        }
        self.audits.push(value);
        true
    }
}

/// The ABI v2 face of the same four primitives: the `Host` trait generated
/// from `wit/kelson-plugin.wit` by `wasmtime::component::bindgen!`.
///
/// This is the *production* implementation — the one
/// [`WasmHost::prepare`](crate::WasmHost::prepare) links a component against,
/// so `component-model` is no longer a compiled-but-unreachable feature.
/// Every method is a one-line delegation to [`CallState`] above.
#[cfg(feature = "component-model")]
impl crate::component::kelson::plugin::host::Host for CallState {
    fn log(&mut self, message: String) {
        self.host_log(&message);
    }

    fn capability_check(&mut self, action: String, path: String) -> bool {
        self.host_capability_check(&action, &path)
    }

    fn capability_write(&mut self, action: String, delta: String) -> bool {
        self.host_capability_write(&action, &delta)
    }

    fn capability_audit(&mut self, action: String, record: String) -> bool {
        self.host_capability_audit(&action, &record)
    }
}

/// Register every linked import on `linker`. Any registration failure is a
/// host construction bug, surfaced as a load error (never a panic).
pub(crate) fn define(linker: &mut Linker<CallState>) -> Result<(), WasmError> {
    define_wasi(linker)?;
    define_host(linker)
}

/// Direct `func_wrap` with fail-closed diagnostics. Kept as a macro (not a
/// generic fn) so `IntoFunc`'s `Caller<'_, T>` parameter keeps its
/// higher-ranked lifetime at the call site.
macro_rules! wrap {
    ($linker:expr, $module:literal, $name:literal, $f:expr $(,)?) => {
        $linker
            .func_wrap($module, $name, $f)
            .map_err(|e| WasmError::Load(format!("import {}.{}: {e}", $module, $name)))
    };
}

fn define_wasi(linker: &mut Linker<CallState>) -> Result<(), WasmError> {
    // fd_write(fd, iovs, iovs_len, nwritten) — stdout/stderr only.
    wrap!(
        linker,
        "wasi_snapshot_preview1",
        "fd_write",
        |mut caller: Caller<'_, CallState>,
         fd: i32,
         iovs: u32,
         iovs_len: u32,
         nwritten: u32|
         -> i32 {
            if fd != 1 && fd != 2 {
                return ERRNO_BADF;
            }
            let Some(memory) = memory_of(&mut caller) else {
                return ERRNO_FAULT;
            };
            let mut total: u32 = 0;
            let mut i: u32 = 0;
            while i < iovs_len {
                let mut entry = [0u8; 8];
                if memory
                    .read(&mut caller, iovs as usize + i as usize * 8, &mut entry)
                    .is_err()
                {
                    return ERRNO_FAULT;
                }
                let ptr =
                    u32::from_le_bytes([entry[0], entry[1], entry[2], entry[3]]) as usize;
                let len =
                    u32::from_le_bytes([entry[4], entry[5], entry[6], entry[7]]) as usize;
                let mut chunk = vec![0u8; len];
                if memory.read(&mut caller, ptr, &mut chunk).is_err() {
                    return ERRNO_FAULT;
                }
                let written = if fd == 1 {
                    std::io::stdout().write_all(&chunk)
                } else {
                    std::io::stderr().write_all(&chunk)
                };
                if written.is_err() {
                    return ERRNO_FAULT;
                }
                total = total.saturating_add(len as u32);
                i += 1;
            }
            if memory
                .write(&mut caller, nwritten as usize, &total.to_le_bytes())
                .is_err()
            {
                return ERRNO_FAULT;
            }
            ERRNO_SUCCESS
        },
    )?;

    // proc_exit(code) — aborts the guest call (WASI says it never returns;
    // returning an error here keeps the sandbox from continuing past it).
    wrap!(
        linker,
        "wasi_snapshot_preview1",
        "proc_exit",
        |_caller: Caller<'_, CallState>, code: i32| -> Result<(), wasmtime::Error> {
            Err(wasmtime::Error::msg(format!(
                "wasi proc_exit({code}) terminated the plugin call"
            )))
        },
    )?;

    // args/environ: deliberately EMPTY — plugins receive no ambient env
    // (no new env vars for the platform, no host secrets in guest memory).
    wrap!(
        linker,
        "wasi_snapshot_preview1",
        "args_sizes_get",
        |mut caller: Caller<'_, CallState>, count: u32, buf_size: u32| -> i32 {
            let Some(memory) = memory_of(&mut caller) else {
                return ERRNO_FAULT;
            };
            if memory
                .write(&mut caller, count as usize, &0u32.to_le_bytes())
                .is_err()
            {
                return ERRNO_FAULT;
            }
            if memory
                .write(&mut caller, buf_size as usize, &0u32.to_le_bytes())
                .is_err()
            {
                return ERRNO_FAULT;
            }
            ERRNO_SUCCESS
        },
    )?;
    wrap!(
        linker,
        "wasi_snapshot_preview1",
        "args_get",
        |_caller: Caller<'_, CallState>, _argv: u32, _buf: u32| -> i32 { ERRNO_SUCCESS },
    )?;
    wrap!(
        linker,
        "wasi_snapshot_preview1",
        "environ_sizes_get",
        |mut caller: Caller<'_, CallState>, count: u32, buf_size: u32| -> i32 {
            let Some(memory) = memory_of(&mut caller) else {
                return ERRNO_FAULT;
            };
            if memory
                .write(&mut caller, count as usize, &0u32.to_le_bytes())
                .is_err()
            {
                return ERRNO_FAULT;
            }
            if memory
                .write(&mut caller, buf_size as usize, &0u32.to_le_bytes())
                .is_err()
            {
                return ERRNO_FAULT;
            }
            ERRNO_SUCCESS
        },
    )?;
    wrap!(
        linker,
        "wasi_snapshot_preview1",
        "environ_get",
        |_caller: Caller<'_, CallState>, _environ: u32, _buf: u32| -> i32 { ERRNO_SUCCESS },
    )?;

    // clock_time_get(id, precision, out) — id 0/4 = realtime (wall clock),
    // everything else is served from process start (monotonic-ish; v1 makes
    // no clock promises beyond "does not go backwards within a call").
    wrap!(
        linker,
        "wasi_snapshot_preview1",
        "clock_time_get",
        |mut caller: Caller<'_, CallState>, clock_id: i32, _precision: i64, out: u32|
         -> i32 {
            static PROCESS_START: OnceLock<std::time::Instant> = OnceLock::new();
            let nanos: u64 = if clock_id == 0 || clock_id == 4 {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_nanos() as u64)
                    .unwrap_or(0)
            } else {
                PROCESS_START
                    .get_or_init(std::time::Instant::now)
                    .elapsed()
                    .as_nanos() as u64
            };
            let Some(memory) = memory_of(&mut caller) else {
                return ERRNO_FAULT;
            };
            if memory
                .write(&mut caller, out as usize, &nanos.to_le_bytes())
                .is_err()
            {
                return ERRNO_FAULT;
            }
            ERRNO_SUCCESS
        },
    )?;

    // random_get — CSPRNG bytes from `rand` (host-quality entropy; no
    // guest-visible source to influence).
    wrap!(
        linker,
        "wasi_snapshot_preview1",
        "random_get",
        |mut caller: Caller<'_, CallState>, buf: u32, len: u32| -> i32 {
            if len > MAX_GUEST_STR {
                return ERRNO_TOOBIG;
            }
            let Some(memory) = memory_of(&mut caller) else {
                return ERRNO_FAULT;
            };
            let mut bytes = vec![0u8; len as usize];
            rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut bytes);
            if memory.write(&mut caller, buf as usize, &bytes).is_err() {
                return ERRNO_FAULT;
            }
            ERRNO_SUCCESS
        },
    )?;

    wrap!(
        linker,
        "wasi_snapshot_preview1",
        "sched_yield",
        |_caller: Caller<'_, CallState>| -> i32 {
            std::thread::yield_now();
            ERRNO_SUCCESS
        },
    )?;
    wrap!(
        linker,
        "wasi_snapshot_preview1",
        "fd_close",
        |_caller: Caller<'_, CallState>, fd: i32| -> i32 {
            if fd == 1 || fd == 2 {
                ERRNO_SUCCESS
            } else {
                ERRNO_BADF
            }
        },
    )?;
    Ok(())
}

fn define_host(linker: &mut Linker<CallState>) -> Result<(), WasmError> {
    // host.log(ptr, len) — structured line, tagged with the plugin label.
    wrap!(
        linker,
        "host",
        "log",
        |mut caller: Caller<'_, CallState>, ptr: u32, len: u32| {
            let Some(memory) = memory_of(&mut caller) else {
                return;
            };
            let Some(message) = read_guest_str(&memory, &mut caller, ptr, len) else {
                return;
            };
            caller.data().host_log(&message);
        },
    )?;

    // host.capability_check(action, path) -> 1 allowed / 0 denied.
    // Verdict = action is in the sidecar-verified capability set (a plugin
    // can never elevate itself), AND a non-empty path must sit inside the
    // execution's resolved scope chain (fail-closed when no scope).
    wrap!(
        linker,
        "host",
        "capability_check",
        |mut caller: Caller<'_, CallState>,
         action_ptr: u32,
         action_len: u32,
         path_ptr: u32,
         path_len: u32|
         -> i32 {
            let (action, path) = {
                let Some(memory) = memory_of(&mut caller) else {
                    return 0;
                };
                let Some(action) = read_guest_str(&memory, &mut caller, action_ptr, action_len)
                else {
                    return 0;
                };
                let Some(path) = read_guest_str(&memory, &mut caller, path_ptr, path_len)
                else {
                    return 0;
                };
                (action, path)
            };
            let state = caller.data();
            i32::from(state.host_capability_check(&action, &path))
        },
    )?;

    // host.capability_write(action, delta) -> 1 recorded / 0 refused.
    // Records a mutation delta for the handler to commit through the
    // execution unit. Refuses unless ALL hold: the executing capability
    // runs a ReadWrite/Admin profile (read-only guests can never write,
    // even when they import the function), the action is in the
    // sidecar-verified declared set (no self-elevation), the delta parses
    // as a JSON object (the handler validates the shape), and the
    // per-call buffer is not full.
    wrap!(
        linker,
        "host",
        "capability_write",
        |mut caller: Caller<'_, CallState>,
         action_ptr: u32,
         action_len: u32,
         delta_ptr: u32,
         delta_len: u32|
         -> i32 {
            let (action, delta) = {
                let Some(memory) = memory_of(&mut caller) else {
                    return 0;
                };
                let Some(action) = read_guest_str(&memory, &mut caller, action_ptr, action_len)
                else {
                    return 0;
                };
                let Some(delta) = read_guest_str(&memory, &mut caller, delta_ptr, delta_len)
                else {
                    return 0;
                };
                (action, delta)
            };
            i32::from(caller.data_mut().host_capability_write(&action, &delta))
        },
    )?;

    // host.capability_audit(action, record) -> 1 recorded / 0 refused.
    // Audit is append-only, so EVERY profile may record (including
    // ReadOnly); the action must still be declared and the record must
    // parse as a JSON object. Same buffer bound as writes.
    wrap!(
        linker,
        "host",
        "capability_audit",
        |mut caller: Caller<'_, CallState>,
         action_ptr: u32,
         action_len: u32,
         record_ptr: u32,
         record_len: u32|
         -> i32 {
            let (action, record) = {
                let Some(memory) = memory_of(&mut caller) else {
                    return 0;
                };
                let Some(action) = read_guest_str(&memory, &mut caller, action_ptr, action_len)
                else {
                    return 0;
                };
                let Some(record) = read_guest_str(&memory, &mut caller, record_ptr, record_len)
                else {
                    return 0;
                };
                (action, record)
            };
            i32::from(caller.data_mut().host_capability_audit(&action, &record))
        },
    )?;
    Ok(())
}
