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
//! * `host` — the two plugin primitives: `host.log` (structured log line)
//!   and `host.capability_check(action, path)` (pre-dispatch authorization
//!   probe against the sidecar-declared capability set; see [`CallState`]).
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
            let plugin = caller.data().label.clone();
            tracing::info!(target: "kelson::wasm", plugin = %plugin, message = %message);
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
            if !state.declared.contains(&action) {
                return 0;
            }
            if path.is_empty() {
                return 1;
            }
            if !state.has_scope {
                return 0;
            }
            i32::from(
                state
                    .scope_paths
                    .iter()
                    .any(|p| *p == path || p.starts_with(&format!("{path}|"))),
            )
        },
    )?;
    Ok(())
}
