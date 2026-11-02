//! EffectPlan deterministic resource-conflict planner core.
//!
//! The shipped WebAssembly build is `#![no_std]`, allocates nothing, imports
//! nothing and touches no host capability: it reads and writes one statically
//! reserved linear-memory arena and speaks only the raw integer ABI below.
//!
//! Memory-safety strategy
//! ----------------------
//! * The arena layout is owned by Rust and is contiguous (const-asserted).
//! * The host learns every offset from `effectplan_info`, so there is one
//!   source of truth and no duplicated constant to drift.
//! * `effectplan_plan` accepts absolute linear-memory addresses, converts them
//!   to arena-relative offsets with checked arithmetic, and *refuses* any value
//!   that is not the canonical region base. JavaScript is therefore never
//!   trusted for a pointer, even though it created it.
//! * From that point on the algorithm only ever sees disjoint `&mut [u8]`
//!   halves produced by `split_at_mut`; no raw pointer reaches the scheduler.
//! * All arithmetic is checked, every byte access goes through a bounds-checked
//!   accessor, and every failure returns a stable status code instead of
//!   panicking.

//! The `no_std` claim is compile-enforced where it matters: only the
//! `wasm32-unknown-unknown` build is `#![no_std]`, so any accidental use of the
//! standard library fails the artifact build. Host builds enable `std` purely so
//! `cargo test` can link its harness against this same code.

#![cfg_attr(all(not(test), target_arch = "wasm32"), no_std)]

#[cfg(all(target_arch = "wasm32", not(test)))]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // Fails closed: the host observes a WebAssembly trap and maps it to
    // EP_ENGINE_ERROR. Reaching it would be a defect, never a silent fallback.
    core::arch::wasm32::unreachable()
}

#[cfg(test)]
mod tests;

pub mod levels;

pub mod schedule;

pub mod checkpoint;

pub mod resources;

pub mod scheduling;

/// The engine works on one statically reserved arena, so the test modules
/// that drive it through the real ABI must exclude each other.
#[cfg(test)]
pub(crate) static ARENA_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod levels_tests;

#[cfg(test)]
mod schedule_tests;

#[cfg(test)]
mod checkpoint_tests;

#[cfg(test)]
mod resources_tests;

#[cfg(test)]
mod scheduling_tests;

// ---------------------------------------------------------------------------
// Limits. These are the conservative Phase-1 prototype ceilings.
// ---------------------------------------------------------------------------

/// Stable raw ABI revision.
pub const ABI_VERSION: u32 = 1;

pub const MAX_CALLS: usize = 128;
pub const MAX_ACCESS: usize = 1024;
/// Explicit resources are `1..=MAX_RESOURCES`; index 0 is the global barrier.
pub const MAX_RESOURCES: usize = 1024;
pub const RES_SLOTS: usize = MAX_RESOURCES + 1;
/// One barrier event per call plus every explicit access event.
pub const MAX_EVENTS: usize = MAX_CALLS + MAX_ACCESS;
/// Proven bound: an event can create at most two records, verified against the
/// reference oracle and property tests.
pub const MAX_EDGES: usize = 2 * MAX_EVENTS;
pub const EDGE_BYTES: usize = 16;
pub const OUT_HEADER_BYTES: usize = 16;
pub const SENTINEL: u32 = u32::MAX;

pub const MAX_NAMESPACE_BYTES: usize = 256;
pub const MAX_RESOURCE_ID_BYTES: usize = 1024;

// Effect modes.
pub const MODE_PURE: u8 = 0;
pub const MODE_RESOURCES: u8 = 1;
pub const MODE_EXCLUSIVE: u8 = 2;
pub const MODE_UNKNOWN: u8 = 3;

// Access modes.
pub const ACCESS_READ: u8 = 0;
pub const ACCESS_WRITE: u8 = 1;

// Reason codes. 4..6 are the same relations evaluated against the barrier.
pub const REASON_WRITE_READ: u32 = 1;
pub const REASON_READ_WRITE: u32 = 2;
pub const REASON_WRITE_WRITE: u32 = 3;
pub const REASON_BARRIER_OFFSET: u32 = 3;

// Stable numeric ABI status codes. Never renumber: this is a wire contract.
pub const OK: i32 = 0;
pub const ERR_ABI_VERSION: i32 = 100;
pub const ERR_CALL_LIMIT: i32 = 101;
pub const ERR_ACCESS_LIMIT: i32 = 102;
pub const ERR_RESOURCE_LIMIT: i32 = 103;
pub const ERR_RANGE: i32 = 104;
pub const ERR_ALIGNMENT: i32 = 105;
pub const ERR_WORKSPACE_CAP: i32 = 106;
pub const ERR_OUTPUT_CAP: i32 = 107;
pub const ERR_LAYOUT: i32 = 108;
pub const ERR_CALL_MODE: i32 = 110;
pub const ERR_OFFSET_MONOTONIC: i32 = 111;
pub const ERR_OFFSET_INITIAL: i32 = 112;
pub const ERR_OFFSET_TERMINAL: i32 = 113;
pub const ERR_RESOURCE_INDEX: i32 = 114;
pub const ERR_ACCESS_MODE: i32 = 115;
/// The levels region is not the canonical region, or its length is wrong.
pub const ERR_LEVELS_CAP: i32 = 116;
/// The computed levels contradict the graph they came from.
pub const ERR_LEVELS_INVALID: i32 = 117;
/// A duration was missing or zero. The engine never reads that as free work.
pub const ERR_DURATION_ZERO: i32 = 118;
/// A durations or timing region is not the canonical one.
pub const ERR_TIMING_CAP: i32 = 119;
pub const ERR_OVERFLOW: i32 = 120;
/// Tick arithmetic exceeded `u32`. Reported, never wrapped around.
pub const ERR_TIMING_OVERFLOW: i32 = 121;
/// The written schedule contradicts the graph or the durations behind it.
pub const ERR_TIMING_INVALID: i32 = 122;
pub const ERR_OUTPUT_INVALID: i32 = 130;
/// A restored checkpoint does not match the input prefix it claims to come
/// from, or names state the planner could never have produced.
pub const ERR_CHECKPOINT_INVALID: i32 = 131;
/// A checkpoint's counters exceed what its prefix could have generated.
pub const ERR_CHECKPOINT_RANGE: i32 = 132;
/// A demand exceeds the capacity of the resource it names: the declaration can
/// never be satisfied, which is not the same fact as a schedule being slow.
pub const ERR_RESOURCE_CAPACITY: i32 = 133;
/// A capacity, demand or accumulated total is outside the integer range the
/// resource model supports. Refused rather than saturated.
pub const ERR_RESOURCE_RANGE: i32 = 134;
/// A requirement names a resource that was never declared.
pub const ERR_RESOURCE_UNDECLARED: i32 = 135;
/// The resource declarations are malformed: counts over ceilings, a zero
/// capacity or demand, a duplicated resource in one call, an offset vector that
/// does not describe its records. Nothing after this can be trusted, so the scan
/// stops instead of continuing on guessed boundaries.
pub const ERR_RESOURCE_REGION: i32 = 136;
/// A call could never be placed: nothing is running, nothing fits, and the
/// graph waits on something that will not finish. A forged or cyclic graph reaches
/// this; a genuine plan from this planner cannot.
pub const ERR_UNSCHEDULABLE: i32 = 137;
pub const ERR_INTERNAL: i32 = 140;

// ---------------------------------------------------------------------------
// Contiguous arena layout.
// ---------------------------------------------------------------------------

pub const IN_CALL_MODES_OFF: usize = 0;
pub const IN_CALL_MODES_LEN: usize = MAX_CALLS;
pub const IN_ACCESS_OFFSETS_OFF: usize = IN_CALL_MODES_OFF + IN_CALL_MODES_LEN;
pub const IN_ACCESS_OFFSETS_LEN: usize = (MAX_CALLS + 1) * 4;
pub const IN_RESOURCE_IDS_OFF: usize = IN_ACCESS_OFFSETS_OFF + IN_ACCESS_OFFSETS_LEN;
pub const IN_RESOURCE_IDS_LEN: usize = MAX_ACCESS * 4;
pub const IN_ACCESS_MODES_OFF: usize = IN_RESOURCE_IDS_OFF + IN_RESOURCE_IDS_LEN;
pub const IN_ACCESS_MODES_LEN: usize = MAX_ACCESS;
pub const INPUT_REGION_LEN: usize = IN_ACCESS_MODES_OFF + IN_ACCESS_MODES_LEN;

pub const WS_LAST_WRITER_OFF: usize = 0;
pub const WS_LAST_WRITER_LEN: usize = RES_SLOTS * 4;
pub const WS_READER_HEAD_OFF: usize = WS_LAST_WRITER_OFF + WS_LAST_WRITER_LEN;
pub const WS_READER_HEAD_LEN: usize = RES_SLOTS * 4;
pub const WS_EVENT_NEXT_OFF: usize = WS_READER_HEAD_OFF + WS_READER_HEAD_LEN;
pub const WS_EVENT_NEXT_LEN: usize = MAX_EVENTS * 4;
pub const WS_EVENT_OP_OFF: usize = WS_EVENT_NEXT_OFF + WS_EVENT_NEXT_LEN;
pub const WS_EVENT_OP_LEN: usize = MAX_EVENTS * 4;
pub const WORKSPACE_REGION_LEN: usize = WS_EVENT_OP_OFF + WS_EVENT_OP_LEN;

pub const OUT_HEADER_OFF: usize = 0;
pub const OUT_RECORDS_OFF: usize = OUT_HEADER_BYTES;
pub const OUT_RECORDS_LEN: usize = MAX_EDGES * EDGE_BYTES;
pub const OUTPUT_REGION_LEN: usize = OUT_RECORDS_OFF + OUT_RECORDS_LEN;

// ---------------------------------------------------------------------------
// Execution levels (Phase 2).
//
// The dependency graph is over *calls*, not over the internal event list: an
// edge always names two calls of the batch. The level of a call is therefore
// bounded by the call count, not by the event count.
// ---------------------------------------------------------------------------

/// Revision of the extended output contract. Bumped only for a breaking
/// change to the levels region.
pub const PLAN_V2_REVISION: u32 = 1;
/// A `u32` count followed by one `u16` level per call.
pub const LEVELS_HEADER_BYTES: usize = 4;
pub const LEVELS_MAX_ENTRIES: usize = MAX_CALLS;
pub const LEVELS_REGION_LEN: usize = LEVELS_HEADER_BYTES + LEVELS_MAX_ENTRIES * 2;
pub const LEVELS_BASE_OFF: usize = OUT_BASE_OFF + OUTPUT_REGION_LEN;

pub const WS_BASE_OFF: usize = INPUT_REGION_LEN;
pub const OUT_BASE_OFF: usize = WS_BASE_OFF + WORKSPACE_REGION_LEN;

// ---------------------------------------------------------------------------
// Critical Path Intelligence (Phase 3).
//
// Durations are one `u32` per call — the graph's node set — and the timing
// result is a header plus two `u32` per call. Sized from MAX_CALLS, not from
// the internal event list.
// ---------------------------------------------------------------------------

/// Revision of the scheduling contract.
pub const TIMING_REVISION: u32 = 1;
/// One `u32` tick count per call slot.
pub const DURATION_BYTES: usize = 4;
pub const DURATIONS_MAX_ENTRIES: usize = MAX_CALLS;
pub const DURATIONS_REGION_LEN: usize = DURATIONS_MAX_ENTRIES * DURATION_BYTES;
pub const DURATIONS_BASE_OFF: usize = LEVELS_BASE_OFF + LEVELS_REGION_LEN;
/// Header plus `(earliest_start, slack)` per call. Latest times are derived.
pub const TIMING_HEADER_BYTES: usize = 32;
pub const TIMING_EVENT_BYTES: usize = 8;
pub const TIMING_MAX_ENTRIES: usize = MAX_CALLS;
pub const TIMING_REGION_LEN: usize = TIMING_HEADER_BYTES + TIMING_MAX_ENTRIES * TIMING_EVENT_BYTES;
pub const TIMING_BASE_OFF: usize = DURATIONS_BASE_OFF + DURATIONS_REGION_LEN;

// ---------------------------------------------------------------------------
// Resumable planning (Phase 4).
//
// One 32-byte record: the engine commits the boundary it stopped at, the host
// stores it with the arena bytes it already has access to, and a later call can
// resume from that boundary instead of replanning the prefix. No session state
// lives in the arena between calls — see `engine/src/checkpoint.rs`.
// ---------------------------------------------------------------------------

pub const CHECKPOINT_BASE_OFF: usize = TIMING_BASE_OFF + TIMING_REGION_LEN;
pub const CHECKPOINT_REGION_LEN: usize = checkpoint::CHECKPOINT_RECORD_BYTES;

// ---------------------------------------------------------------------------
// Capacity resources (Phase 5).
//
// A separate index space from the dependency resources: those are interned by
// the host and indexed by `RES_SLOTS` lanes in the workspace, these are numbered
// `0..R` against their own capacity table, and no mapping between the two exists
// or should. Sized from `MAX_CAPACITY_RESOURCES` and `MAX_DEMAND_RECORDS`, both
// computed against the memory budget in `docs/RESOURCE-ABI.md` rather than picked
// for comfort — one page of address space covers every configuration the
// specification asked to benchmark, so the limits are chosen from function.
// ---------------------------------------------------------------------------

/// Capacities, one `u32` per declared resource. Written by the host.
pub const CAPACITIES_BASE_OFF: usize = CHECKPOINT_BASE_OFF + CHECKPOINT_REGION_LEN;
pub const CAPACITIES_REGION_LEN: usize = resources::CAPACITIES_REGION_LEN;

/// Demand bounds, one `u32` per call slot plus the terminal bound. Host-written.
pub const DEMAND_OFFSETS_BASE_OFF: usize = CAPACITIES_BASE_OFF + CAPACITIES_REGION_LEN;
pub const DEMAND_OFFSETS_REGION_LEN: usize = resources::DEMAND_OFFSETS_REGION_LEN;

/// `(resource, amount)` pairs, flattened: no per-call collection exists anywhere.
pub const DEMAND_RECORDS_BASE_OFF: usize = DEMAND_OFFSETS_BASE_OFF + DEMAND_OFFSETS_REGION_LEN;
pub const DEMAND_RECORDS_REGION_LEN: usize = resources::DEMAND_RECORDS_REGION_LEN;

/// Live usage while placing. Engine-private, never read by the host.
pub const RESOURCE_USAGE_BASE_OFF: usize = DEMAND_RECORDS_BASE_OFF + DEMAND_RECORDS_REGION_LEN;
pub const RESOURCE_USAGE_REGION_LEN: usize = resources::USAGE_REGION_LEN;

/// Working in-degree per call. Engine-private.
pub const RESOURCE_INDEGREE_BASE_OFF: usize = RESOURCE_USAGE_BASE_OFF + RESOURCE_USAGE_REGION_LEN;
pub const RESOURCE_INDEGREE_REGION_LEN: usize = resources::INDEGREE_REGION_LEN;

/// Earliest tick each call may start, from the predecessors already finished.
/// Engine-private.
pub const RESOURCE_READY_BASE_OFF: usize =
    RESOURCE_INDEGREE_BASE_OFF + RESOURCE_INDEGREE_REGION_LEN;
pub const RESOURCE_READY_REGION_LEN: usize = resources::READY_REGION_LEN;

/// Downstream dependents per call — the third ranking rule. Engine-private.
pub const RESOURCE_OUTDEGREE_BASE_OFF: usize = RESOURCE_READY_BASE_OFF + RESOURCE_READY_REGION_LEN;
pub const RESOURCE_OUTDEGREE_REGION_LEN: usize = resources::OUTDEGREE_REGION_LEN;

/// The resource schedule: `(start, finish)` per call. Engine-written, host-read.
pub const SCHEDULE_BASE_OFF: usize = RESOURCE_OUTDEGREE_BASE_OFF + RESOURCE_OUTDEGREE_REGION_LEN;
pub const SCHEDULE_REGION_LEN: usize = resources::SCHEDULE_REGION_LEN;

/// Per-resource capacity, peak and busy time. Engine-written, host-read.
pub const RESOURCE_STATS_BASE_OFF: usize = SCHEDULE_BASE_OFF + SCHEDULE_REGION_LEN;
pub const RESOURCE_STATS_REGION_LEN: usize = resources::STATS_REGION_LEN;

/// Why something could not be scheduled, by index and never by caller identifier.
pub const RESOURCE_DIAGNOSTICS_BASE_OFF: usize =
    RESOURCE_STATS_BASE_OFF + RESOURCE_STATS_REGION_LEN;
pub const RESOURCE_DIAGNOSTICS_REGION_LEN: usize = resources::DIAGNOSTICS_REGION_LEN;

pub const ARENA_BYTES: usize =
    align8(RESOURCE_DIAGNOSTICS_BASE_OFF + RESOURCE_DIAGNOSTICS_REGION_LEN);

const fn align8(n: usize) -> usize {
    (n + 7) / 8 * 8
}

// The layout must stay contiguous and four-byte aligned at every u32 boundary,
// otherwise the sequential split below would silently skip or overlap bytes.
const _: () = assert!(IN_ACCESS_OFFSETS_OFF == IN_CALL_MODES_OFF + IN_CALL_MODES_LEN);
const _: () = assert!(IN_RESOURCE_IDS_OFF == IN_ACCESS_OFFSETS_OFF + IN_ACCESS_OFFSETS_LEN);
const _: () = assert!(IN_ACCESS_MODES_OFF == IN_RESOURCE_IDS_OFF + IN_RESOURCE_IDS_LEN);
const _: () = assert!(INPUT_REGION_LEN == IN_ACCESS_MODES_OFF + IN_ACCESS_MODES_LEN);
const _: () = assert!(WS_BASE_OFF == INPUT_REGION_LEN);
const _: () = assert!(WS_BASE_OFF % 4 == 0);
const _: () = assert!(OUT_BASE_OFF == WS_BASE_OFF + WORKSPACE_REGION_LEN);
const _: () = assert!(OUT_BASE_OFF % 4 == 0);
const _: () = assert!(IN_ACCESS_OFFSETS_OFF % 4 == 0);
const _: () = assert!(IN_RESOURCE_IDS_OFF % 4 == 0);
const _: () = assert!(OUT_RECORDS_LEN / EDGE_BYTES == MAX_EDGES);
const _: () = assert!(LEVELS_BASE_OFF == OUT_BASE_OFF + OUTPUT_REGION_LEN);
const _: () = assert!(LEVELS_BASE_OFF % 4 == 0);
const _: () = assert!(LEVELS_REGION_LEN % 4 == 0);
const _: () = assert!(DURATIONS_BASE_OFF == LEVELS_BASE_OFF + LEVELS_REGION_LEN);
const _: () = assert!(DURATIONS_BASE_OFF % 4 == 0);
const _: () = assert!(TIMING_BASE_OFF == DURATIONS_BASE_OFF + DURATIONS_REGION_LEN);
const _: () = assert!(TIMING_BASE_OFF % 4 == 0);
const _: () = assert!(TIMING_REGION_LEN % 4 == 0);
const _: () = assert!(TIMING_REGION_LEN % 8 == 0);
const _: () = assert!(ARENA_BYTES >= TIMING_BASE_OFF + TIMING_REGION_LEN);
const _: () = assert!(CHECKPOINT_BASE_OFF == TIMING_BASE_OFF + TIMING_REGION_LEN);
const _: () = assert!(CHECKPOINT_BASE_OFF % 4 == 0);
const _: () = assert!(ARENA_BYTES >= CHECKPOINT_BASE_OFF + CHECKPOINT_REGION_LEN);

// Phase 5 tiles the resource regions after the checkpoint record, and the total
// is pinned to the figure `docs/RESOURCE-ABI.md` derives: capacities 4R, demand
// offsets 4(V+1), demand records 8K, four engine-private arrays, then schedule,
// stats and diagnostics — 13,928 bytes new at R=64 and K=1,024, on top of the
// 61,920 the first four phases needed. A layout change moves this number, and the
// budget document has to move with it rather than drift.
const _: () = assert!(CAPACITIES_BASE_OFF == CHECKPOINT_BASE_OFF + CHECKPOINT_REGION_LEN);
const _: () = assert!(CAPACITIES_BASE_OFF % 4 == 0);
const _: () = assert!(DEMAND_OFFSETS_BASE_OFF == CAPACITIES_BASE_OFF + CAPACITIES_REGION_LEN);
const _: () = assert!(DEMAND_OFFSETS_BASE_OFF % 4 == 0);
const _: () =
    assert!(DEMAND_RECORDS_BASE_OFF == DEMAND_OFFSETS_BASE_OFF + DEMAND_OFFSETS_REGION_LEN);
const _: () = assert!(DEMAND_RECORDS_BASE_OFF % 4 == 0);
const _: () =
    assert!(RESOURCE_USAGE_BASE_OFF == DEMAND_RECORDS_BASE_OFF + DEMAND_RECORDS_REGION_LEN);
const _: () =
    assert!(RESOURCE_INDEGREE_BASE_OFF == RESOURCE_USAGE_BASE_OFF + RESOURCE_USAGE_REGION_LEN);
const _: () =
    assert!(RESOURCE_READY_BASE_OFF == RESOURCE_INDEGREE_BASE_OFF + RESOURCE_INDEGREE_REGION_LEN);
const _: () =
    assert!(RESOURCE_OUTDEGREE_BASE_OFF == RESOURCE_READY_BASE_OFF + RESOURCE_READY_REGION_LEN);
const _: () =
    assert!(SCHEDULE_BASE_OFF == RESOURCE_OUTDEGREE_BASE_OFF + RESOURCE_OUTDEGREE_REGION_LEN);
const _: () = assert!(RESOURCE_STATS_BASE_OFF == SCHEDULE_BASE_OFF + SCHEDULE_REGION_LEN);
const _: () =
    assert!(RESOURCE_DIAGNOSTICS_BASE_OFF == RESOURCE_STATS_BASE_OFF + RESOURCE_STATS_REGION_LEN);
const _: () =
    assert!(ARENA_BYTES == align8(RESOURCE_DIAGNOSTICS_BASE_OFF + RESOURCE_DIAGNOSTICS_REGION_LEN));
const _: () = assert!(ARENA_BYTES == 75_848);
const _: () = assert!(
    DEMAND_RECORDS_REGION_LEN / resources::DEMAND_RECORD_BYTES == resources::MAX_DEMAND_RECORDS
);
const _: () = assert!(ARENA_BYTES < 256 * 1024);

/// The single statically reserved linear-memory region. It is zero-initialised,
/// so the linker emits it as BSS and it costs nothing in the published file.
#[repr(C, align(8))]
pub struct Arena([u8; ARENA_BYTES]);

#[cfg(not(target_arch = "wasm32"))]
pub static mut ARENA: Arena = Arena([0u8; ARENA_BYTES]);

#[cfg(target_arch = "wasm32")]
static mut ARENA: Arena = Arena([0u8; ARENA_BYTES]);

// ---------------------------------------------------------------------------
// Little-endian byte access.
//
// Explicit little-endian keeps the packed format identical on every host and
// avoids unaligned-reference UB. Indices are proven in range by the callers;
// the checked accessors still fail closed rather than panicking.
// ---------------------------------------------------------------------------

#[inline]
/// A region of the arena, or `None` when the buffer is shorter than the region
/// the const layout says it should be. Only used where a slice is needed rather
/// than a single word.
pub(crate) fn region(buf: &[u8], at: usize, len: usize) -> Option<&[u8]> {
    buf.get(at..at.checked_add(len)?)
}

pub(crate) fn rd_u32(buf: &[u8], at: usize) -> u32 {
    match buf.get(at..at.saturating_add(4)) {
        Some(s) => u32::from_le_bytes([s[0], s[1], s[2], s[3]]),
        None => SENTINEL,
    }
}

#[inline]
pub(crate) fn wr_u32(buf: &mut [u8], at: usize, value: u32) -> bool {
    match buf.get_mut(at..at.saturating_add(4)) {
        Some(s) => {
            s.copy_from_slice(&value.to_le_bytes());
            true
        }
        None => false,
    }
}

fn fill(buf: &mut [u8], byte: u8) {
    let mut i = 0usize;
    while i < buf.len() {
        if let Some(slot) = buf.get_mut(i) {
            *slot = byte;
        }
        i = i.saturating_add(1);
    }
}

fn checked_mul(a: usize, b: usize) -> Option<usize> {
    a.checked_mul(b)
}

// ---------------------------------------------------------------------------
// Validation passes
// ---------------------------------------------------------------------------

/// Validates the ABI revision, every count ceiling and every declared capacity.
fn validate_input(
    abi_version: u32,
    call_count: usize,
    access_count: usize,
    resource_count: usize,
    workspace_len: usize,
    output_capacity: usize,
) -> i32 {
    if abi_version != ABI_VERSION {
        return ERR_ABI_VERSION;
    }
    if call_count > MAX_CALLS {
        return ERR_CALL_LIMIT;
    }
    if access_count > MAX_ACCESS {
        return ERR_ACCESS_LIMIT;
    }
    if resource_count == 0 || resource_count > RES_SLOTS {
        return ERR_RESOURCE_LIMIT;
    }
    if workspace_len < WORKSPACE_REGION_LEN {
        return ERR_WORKSPACE_CAP;
    }
    if output_capacity < OUT_HEADER_BYTES {
        return ERR_OUTPUT_CAP;
    }
    if output_capacity > OUTPUT_REGION_LEN {
        return ERR_OUTPUT_CAP;
    }
    // Anything the host could have derived by multiplication must not overflow.
    if checked_mul(call_count + 1, 4).is_none()
        || checked_mul(access_count, 4).is_none()
        || checked_mul(resource_count, 4).is_none()
        || checked_mul(call_count + access_count, 4).is_none()
        || checked_mul(2 * (call_count + access_count), EDGE_BYTES).is_none()
    {
        return ERR_OVERFLOW;
    }
    OK
}

/// Validates the CSR-style offsets array: anchored at zero, monotonic, and
/// terminating exactly at `access_count`.
fn validate_offsets(offsets: &[u8], call_count: usize, access_count: usize) -> i32 {
    if call_count == 0 {
        return if access_count == 0 {
            OK
        } else {
            ERR_OFFSET_TERMINAL
        };
    }
    if offsets.len() < (call_count + 1) * 4 {
        return ERR_RANGE;
    }
    if rd_u32(offsets, 0) != 0 {
        return ERR_OFFSET_INITIAL;
    }
    let mut i = 0usize;
    while i < call_count {
        let start = rd_u32(offsets, i * 4);
        let end = rd_u32(offsets, (i + 1) * 4);
        if end < start || end > access_count as u32 {
            return ERR_OFFSET_MONOTONIC;
        }
        i += 1;
    }
    if rd_u32(offsets, call_count * 4) != access_count as u32 {
        return ERR_OFFSET_TERMINAL;
    }
    OK
}

/// Validates every effect mode, interned resource index and access mode.
fn validate_accesses(
    call_modes: &[u8],
    resource_ids: &[u8],
    access_modes: &[u8],
    call_count: usize,
    access_count: usize,
    resource_count: usize,
) -> i32 {
    let mut i = 0usize;
    while i < call_count {
        let m = match call_modes.get(i) {
            Some(v) => *v,
            None => return ERR_RANGE,
        };
        if m > MODE_UNKNOWN {
            return ERR_CALL_MODE;
        }
        i += 1;
    }
    let mut k = 0usize;
    while k < access_count {
        let rid = rd_u32(resource_ids, k * 4);
        if rid == 0 || rid >= resource_count as u32 {
            return ERR_RESOURCE_INDEX;
        }
        match access_modes.get(k) {
            Some(&ACCESS_READ) | Some(&ACCESS_WRITE) => {}
            Some(_) => return ERR_ACCESS_MODE,
            None => return ERR_RANGE,
        }
        k += 1;
    }
    OK
}

// ---------------------------------------------------------------------------
// Planner
// ---------------------------------------------------------------------------

/// One planning pass over borrow-checked, mutually disjoint slices. No
/// allocation, no recursion, no host services.
struct Planner<'a> {
    call_modes: &'a [u8],
    offsets: &'a [u8],
    resource_ids: &'a [u8],
    access_modes: &'a [u8],
    last_writer: &'a mut [u8],
    reader_head: &'a mut [u8],
    event_next: &'a mut [u8],
    event_op: &'a mut [u8],
    header: &'a mut [u8],
    records: &'a mut [u8],
    call_count: usize,
    resource_count: usize,
    events: usize,
    edge_count: usize,
    edge_cap: usize,
    /// First call to process. Zero unless a checkpoint said where to resume.
    start_at: usize,
    /// True when the frontier and event arrays arrived already populated.
    resumed: bool,
    status: i32,
}

impl<'a> Planner<'a> {
    /// Resets resource state. `last_writer` uses the `u32::MAX` sentinel;
    /// `reader_head` and `event_next` store event index + 1 with 0 meaning empty,
    /// so zero initialisation is a valid empty list.
    fn initialize_workspace(&mut self) {
        fill(self.last_writer, 0xFF);
        fill(self.reader_head, 0);
        fill(self.event_next, 0);
        fill(self.event_op, 0);
        self.events = 0;
        self.edge_count = 0;
    }

    #[inline]
    fn last_writer_of(&self, rid: usize) -> u32 {
        rd_u32(self.last_writer, rid.saturating_mul(4))
    }

    fn set_last_writer(&mut self, rid: usize, op: u32) {
        if !wr_u32(self.last_writer, rid.saturating_mul(4), op) {
            self.status = ERR_INTERNAL;
        }
    }

    #[inline]
    fn head_of(&self, rid: usize) -> u32 {
        rd_u32(self.reader_head, rid.saturating_mul(4))
    }

    fn set_head(&mut self, rid: usize, value: u32) {
        if !wr_u32(self.reader_head, rid.saturating_mul(4), value) {
            self.status = ERR_INTERNAL;
        }
    }

    /// Appends one edge record. Capacity exhaustion is an explicit status,
    /// never a silent truncation.
    fn emit_edge(&mut self, pred: usize, succ: usize, res: usize, reason: u32) {
        if self.status != OK {
            return;
        }
        if self.edge_count >= self.edge_cap {
            self.status = ERR_OUTPUT_CAP;
            return;
        }
        let at = self.edge_count.saturating_mul(EDGE_BYTES);
        let code = if res == 0 {
            reason.saturating_add(REASON_BARRIER_OFFSET)
        } else {
            reason
        };
        let ok = wr_u32(self.records, at, pred as u32)
            && wr_u32(self.records, at.saturating_add(4), succ as u32)
            && wr_u32(self.records, at.saturating_add(8), res as u32)
            && wr_u32(self.records, at.saturating_add(12), code);
        if !ok {
            self.status = ERR_OUTPUT_CAP;
            return;
        }
        self.edge_count += 1;
    }

    /// A read waits for the current writer of that resource, then registers
    /// itself as a pending reader. Read/read pairs never create a dependency.
    fn process_read(&mut self, op: usize, rid: usize) {
        let writer = self.last_writer_of(rid);
        if writer != SENTINEL && (writer as usize) != op {
            self.emit_edge(writer as usize, op, rid, REASON_WRITE_READ);
        }
        if self.status != OK {
            return;
        }
        if self.events >= MAX_EVENTS {
            self.status = ERR_OVERFLOW;
            return;
        }
        let at = self.events.saturating_mul(4);
        let prev = self.head_of(rid);
        let ok = wr_u32(self.event_op, at, op as u32) && wr_u32(self.event_next, at, prev);
        if !ok {
            self.status = ERR_INTERNAL;
            return;
        }
        self.set_head(rid, self.events as u32 + 1);
        self.events += 1;
    }

    /// A write waits for the previous writer and for every pending reader, then
    /// takes ownership of the resource and clears the reader list.
    fn process_write(&mut self, op: usize, rid: usize) {
        let writer = self.last_writer_of(rid);
        if writer != SENTINEL && (writer as usize) != op {
            self.emit_edge(writer as usize, op, rid, REASON_WRITE_WRITE);
        }
        let mut head = self.head_of(rid);
        // The chain decreases by construction — a node's link names an older
        // node — and `checkpoint::validate` re-proves it for restored state. The
        // bound is the belt to that braces: a reader list can never be longer
        // than the events that exist.
        let mut steps = self.events;
        while head != 0 && self.status == OK {
            if steps == 0 {
                self.status = ERR_OUTPUT_INVALID;
                break;
            }
            steps -= 1;
            let event = head as usize - 1;
            let reader = rd_u32(self.event_op, event.saturating_mul(4)) as usize;
            if reader != op {
                self.emit_edge(reader, op, rid, REASON_READ_WRITE);
            }
            head = rd_u32(self.event_next, event.saturating_mul(4));
        }
        if self.status != OK {
            return;
        }
        self.set_head(rid, 0);
        self.set_last_writer(rid, op as u32);
    }

    #[inline]
    fn rec(&self, idx: usize) -> (u32, u32, u32, u32) {
        let at = idx.saturating_mul(EDGE_BYTES);
        (
            rd_u32(self.records, at),
            rd_u32(self.records, at.saturating_add(4)),
            rd_u32(self.records, at.saturating_add(8)),
            rd_u32(self.records, at.saturating_add(12)),
        )
    }

    /// Sorts one successor's records into `(pred, resource, reason)` order and
    /// drops exact duplicates, which makes the whole output deterministic.
    fn finish_group(&mut self, start: usize) {
        let mut i = start.saturating_add(1);
        while i < self.edge_count {
            let mut j = i;
            while j > start && self.rec(j) < self.rec(j - 1) {
                self.swap(j, j - 1);
                j -= 1;
            }
            i += 1;
        }
        let mut keep = start;
        let mut i = start;
        while i < self.edge_count {
            if i == start || self.rec(i) != self.rec(keep - 1) {
                if i != keep {
                    self.copy_record(i, keep);
                }
                keep += 1;
            }
            i += 1;
        }
        self.edge_count = keep;
    }

    fn swap(&mut self, a: usize, b: usize) {
        let ra = self.rec(a);
        let rb = self.rec(b);
        self.write_record(b, ra);
        self.write_record(a, rb);
    }

    fn write_record(&mut self, idx: usize, r: (u32, u32, u32, u32)) {
        let at = idx.saturating_mul(EDGE_BYTES);
        let ok = wr_u32(self.records, at, r.0)
            && wr_u32(self.records, at.saturating_add(4), r.1)
            && wr_u32(self.records, at.saturating_add(8), r.2)
            && wr_u32(self.records, at.saturating_add(12), r.3);
        if !ok {
            self.status = ERR_INTERNAL;
        }
    }

    fn copy_record(&mut self, from: usize, to: usize) {
        let r = self.rec(from);
        self.write_record(to, r);
    }

    /// Processes calls in input order. The implicit barrier access happens
    /// first, so exclusive and unknown operations are ordered against
    /// everything before them and everything after them.
    ///
    /// A resumed run starts at `start_at` with the frontier and event arrays the
    /// host restored. The prefix's records are already in place and are never
    /// revisited: `finish_group` only ever reorders the group it is closing.
    fn plan_batch(&mut self) {
        if self.resumed {
            self.start_at = self.start_at.min(self.call_count);
        } else {
            self.initialize_workspace();
        }
        let mut i = self.start_at;
        while i < self.call_count && self.status == OK {
            let mode = match self.call_modes.get(i) {
                Some(m) => *m,
                None => {
                    self.status = ERR_RANGE;
                    break;
                }
            };
            let group_start = self.edge_count;

            if mode == MODE_EXCLUSIVE || mode == MODE_UNKNOWN {
                self.process_write(i, 0);
            } else {
                self.process_read(i, 0);
            }

            let start = rd_u32(self.offsets, i.saturating_mul(4)) as usize;
            let end = rd_u32(self.offsets, i.saturating_mul(4).saturating_add(4)) as usize;
            let mut k = start;
            while k < end && self.status == OK {
                let rid = rd_u32(self.resource_ids, k.saturating_mul(4)) as usize;
                let am = match self.access_modes.get(k) {
                    Some(v) => *v,
                    None => {
                        self.status = ERR_RANGE;
                        break;
                    }
                };
                if am == ACCESS_READ {
                    self.process_read(i, rid);
                } else {
                    self.process_write(i, rid);
                }
                k += 1;
            }

            if self.status == OK {
                self.finish_group(group_start);
            }
            i += 1;
        }
    }

    /// Re-reads the emitted graph and refuses to return anything structurally
    /// invalid, including a self-dependency or an unordered record.
    fn validate_output(&mut self) -> i32 {
        if self.status != OK {
            return self.status;
        }
        let need = match checked_mul(self.edge_count, EDGE_BYTES) {
            Some(v) => v,
            None => return ERR_OVERFLOW,
        };
        if need > self.records.len() {
            return ERR_OUTPUT_CAP;
        }
        let mut e = 0usize;
        let mut prev_succ: Option<u32> = None;
        while e < self.edge_count {
            let (pred, succ, res, reason) = self.rec(e);
            if succ >= self.call_count as u32 || pred >= succ {
                return ERR_OUTPUT_INVALID;
            }
            if res >= self.resource_count as u32 {
                return ERR_OUTPUT_INVALID;
            }
            if reason == 0 || reason > 6 {
                return ERR_OUTPUT_INVALID;
            }
            if let Some(p) = prev_succ {
                if succ < p {
                    return ERR_OUTPUT_INVALID;
                }
            }
            prev_succ = Some(succ);
            e += 1;
        }
        let ok = wr_u32(self.header, 0, ABI_VERSION)
            && wr_u32(self.header, 4, self.call_count as u32)
            && wr_u32(self.header, 8, self.edge_count as u32)
            && wr_u32(self.header, 12, 0);
        if !ok {
            return ERR_INTERNAL;
        }
        OK
    }

    fn run(&mut self) -> i32 {
        self.plan_batch();
        self.validate_output()
    }
}

/// Runs one planning pass over a fresh frontier. Pointers are arena-relative
/// offsets. This is the Phase 1 entry point and its behaviour is unchanged.
///
/// # Safety
/// `arena` must be at least `OUT_BASE_OFF + OUTPUT_REGION_LEN` bytes long. Every
/// argument is validated before any byte is read or written.
pub unsafe fn plan_arena(
    arena: &mut [u8],
    abi_version: u32,
    call_count: u32,
    access_count: u32,
    resource_count: u32,
    call_modes_ptr: u32,
    access_offsets_ptr: u32,
    resource_ids_ptr: u32,
    access_modes_ptr: u32,
    workspace_ptr: u32,
    workspace_len: u32,
    output_ptr: u32,
    output_capacity: u32,
) -> i32 {
    plan_arena_resuming(
        arena,
        abi_version,
        call_count,
        access_count,
        resource_count,
        call_modes_ptr,
        access_offsets_ptr,
        resource_ids_ptr,
        access_modes_ptr,
        workspace_ptr,
        workspace_len,
        output_ptr,
        output_capacity,
        None,
        None,
    )
}

/// The planning pass, optionally continuing from a checkpoint whose frontier and
/// event arrays the host has already restored into the workspace region and
/// whose records sit in the output region. `None` is exactly the Phase 1 run:
/// reset the frontier, plan every call.
///
/// A restored state is treated as hostile input. Nothing is planned until
/// `checkpoint::validate` has proved the counters, the frontier ranges and the
/// reader-chain shape, and until the digest proves the prefix still describes
/// the calls that produced it.
///
/// # Safety
/// As `plan_arena`.
unsafe fn plan_arena_resuming(
    arena: &mut [u8],
    abi_version: u32,
    call_count: u32,
    access_count: u32,
    resource_count: u32,
    call_modes_ptr: u32,
    access_offsets_ptr: u32,
    resource_ids_ptr: u32,
    access_modes_ptr: u32,
    workspace_ptr: u32,
    workspace_len: u32,
    output_ptr: u32,
    output_capacity: u32,
    resume: Option<checkpoint::Resume>,
    published: Option<&mut checkpoint::Resume>,
) -> i32 {
    let calls = call_count as usize;
    let accesses = access_count as usize;
    let resources = resource_count as usize;

    let status = validate_input(
        abi_version,
        calls,
        accesses,
        resources,
        workspace_len as usize,
        output_capacity as usize,
    );
    if status != OK {
        return status;
    }

    // The host must use the exported layout. A pointer that is merely "inside
    // the arena" is rejected: it could alias a live region, which would be
    // undefined behaviour once the slices below are formed.
    if call_modes_ptr as usize != IN_CALL_MODES_OFF
        || access_offsets_ptr as usize != IN_ACCESS_OFFSETS_OFF
        || resource_ids_ptr as usize != IN_RESOURCE_IDS_OFF
        || access_modes_ptr as usize != IN_ACCESS_MODES_OFF
        || workspace_ptr as usize != WS_BASE_OFF
        || output_ptr as usize != OUT_BASE_OFF
    {
        return ERR_LAYOUT;
    }
    if (call_modes_ptr
        | access_offsets_ptr
        | resource_ids_ptr
        | access_modes_ptr
        | workspace_ptr
        | output_ptr)
        % 4
        != 0
    {
        return ERR_ALIGNMENT;
    }
    if arena.len() < OUT_BASE_OFF + OUTPUT_REGION_LEN {
        return ERR_RANGE;
    }

    // ---- Sequential disjoint splits following the const-asserted layout. ----
    let rest = &mut arena[..OUT_BASE_OFF + OUTPUT_REGION_LEN];
    let (call_modes_full, rest) = rest.split_at_mut(IN_CALL_MODES_LEN);
    let (access_offsets_full, rest) = rest.split_at_mut(IN_ACCESS_OFFSETS_LEN);
    let (resource_ids_full, rest) = rest.split_at_mut(IN_RESOURCE_IDS_LEN);
    let (access_modes_full, rest) = rest.split_at_mut(IN_ACCESS_MODES_LEN);
    let (workspace, rest) = rest.split_at_mut(WORKSPACE_REGION_LEN);
    let (output, _rest) = rest.split_at_mut(output_capacity as usize);

    let (call_modes, _) = call_modes_full.split_at_mut(calls);
    let (offsets, _) = access_offsets_full.split_at_mut((calls + 1) * 4);
    let (resource_ids, _) = resource_ids_full.split_at_mut(accesses * 4);
    let (access_modes, _) = access_modes_full.split_at_mut(accesses);

    let (w_last_writer, rest_w) = workspace.split_at_mut(WS_LAST_WRITER_LEN);
    let (w_reader_head, rest_w) = rest_w.split_at_mut(WS_READER_HEAD_LEN);
    let (w_event_next, rest_w) = rest_w.split_at_mut(WS_EVENT_NEXT_LEN);
    let w_event_op = rest_w;

    let (header, records_full) = output.split_at_mut(OUT_HEADER_BYTES);
    let record_bytes = records_full
        .len()
        .saturating_sub(records_full.len() % EDGE_BYTES);
    let (records, _) = records_full.split_at_mut(record_bytes);

    let edge_cap = records.len() / EDGE_BYTES;

    let start_at = resume.map_or(0, |r| r.calls);
    let seeded_events = resume.map_or(0, |r| r.events);
    let seeded_edges = resume.map_or(0, |r| r.edges);
    let resumed = resume.is_some();

    let mut planner = Planner {
        call_modes,
        offsets,
        resource_ids,
        access_modes,
        last_writer: w_last_writer,
        reader_head: w_reader_head,
        event_next: w_event_next,
        event_op: w_event_op,
        header,
        records,
        call_count: calls,
        resource_count: resources,
        events: seeded_events,
        edge_count: seeded_edges,
        edge_cap,
        start_at,
        resumed,
        status: OK,
    };

    // Structural validation on the exact slices the algorithm will use.
    let status = validate_offsets(planner.offsets, calls, accesses);
    if status != OK {
        return status;
    }
    let status = validate_accesses(
        planner.call_modes,
        planner.resource_ids,
        planner.access_modes,
        calls,
        accesses,
        resources,
    );
    if status != OK {
        return status;
    }

    if let Some(record) = resume {
        // The accesses that belong to the prefix: the CSR offset at the
        // boundary says how many there are.
        let prefix_accesses = rd_u32(planner.offsets, record.calls.saturating_mul(4)) as usize;
        let status = checkpoint::validate(
            &record,
            planner.call_modes,
            planner.resource_ids,
            planner.access_modes,
            planner.last_writer,
            planner.reader_head,
            planner.event_op,
            planner.event_next,
            planner.records,
            calls,
            resources,
            accesses,
            prefix_accesses,
        );
        if status != OK {
            return status;
        }
        // The prefix that the checkpoint claims to have planned must still be
        // the prefix that is here. A removed call shifts the sequence, an edited
        // call changes it, and either one makes the restored frontier a lie about
        // the graph it is about to extend.
        let expected = checkpoint::digest(
            planner.call_modes,
            planner.offsets,
            planner.resource_ids,
            planner.access_modes,
            planner.last_writer,
            planner.reader_head,
            planner.event_op,
            planner.event_next,
            planner.records,
            record.calls,
            prefix_accesses,
            record.events,
            record.edges,
        );
        if expected != record.digest {
            return ERR_CHECKPOINT_INVALID;
        }
    }

    let status = planner.run();
    if status == OK {
        if let Some(published) = published {
            published.calls = calls;
            published.events = planner.events;
            published.edges = planner.edge_count;
            published.digest = 0;
        }
    }
    status
}

/// Runs one planning pass and computes the execution levels. Pointers are
/// arena-relative offsets.
///
/// The levels region is the last canonical region of the arena, so it is split
/// off first and the planning pass runs on the remaining prefix. The graph
/// that comes back is the only input to the level computation; the levels are
/// never derived from anything the host supplies.
///
/// # Safety
/// `arena` must be at least `LEVELS_BASE_OFF + LEVELS_REGION_LEN` bytes long.
/// Every argument is validated before any byte is read or written.
pub unsafe fn plan_arena_v2(
    arena: &mut [u8],
    abi_version: u32,
    call_count: u32,
    access_count: u32,
    resource_count: u32,
    call_modes_ptr: u32,
    access_offsets_ptr: u32,
    resource_ids_ptr: u32,
    access_modes_ptr: u32,
    workspace_ptr: u32,
    workspace_len: u32,
    output_ptr: u32,
    output_capacity: u32,
    levels_ptr: u32,
    levels_len: u32,
) -> i32 {
    plan_arena_v2_resuming(
        arena,
        abi_version,
        call_count,
        access_count,
        resource_count,
        call_modes_ptr,
        access_offsets_ptr,
        resource_ids_ptr,
        access_modes_ptr,
        workspace_ptr,
        workspace_len,
        output_ptr,
        output_capacity,
        levels_ptr,
        levels_len,
        None,
        None,
    )
}

/// `plan_arena_v2` with an optional resume point. The level pass always runs over
/// the complete graph, prefix included: levels are a function of the whole
/// graph's depth, and one O(V + E) walk of 2,304 records costs about 17 µs, which
/// is not a number worth risking a stale-prefix argument for.
///
/// # Safety
/// As `plan_arena_v2`.
unsafe fn plan_arena_v2_resuming(
    arena: &mut [u8],
    abi_version: u32,
    call_count: u32,
    access_count: u32,
    resource_count: u32,
    call_modes_ptr: u32,
    access_offsets_ptr: u32,
    resource_ids_ptr: u32,
    access_modes_ptr: u32,
    workspace_ptr: u32,
    workspace_len: u32,
    output_ptr: u32,
    output_capacity: u32,
    levels_ptr: u32,
    levels_len: u32,
    resume: Option<checkpoint::Resume>,
    published: Option<&mut checkpoint::Resume>,
) -> i32 {
    // The host must name the canonical levels region exactly: a pointer that
    // merely points into the arena could alias the output region.
    if levels_ptr as usize != LEVELS_BASE_OFF {
        return ERR_LAYOUT;
    }
    if levels_len as usize != LEVELS_REGION_LEN {
        return ERR_LEVELS_CAP;
    }
    if arena.len() < LEVELS_BASE_OFF + LEVELS_REGION_LEN {
        return ERR_RANGE;
    }

    let (head, levels) = arena.split_at_mut(LEVELS_BASE_OFF);
    let status = plan_arena_resuming(
        head,
        abi_version,
        call_count,
        access_count,
        resource_count,
        call_modes_ptr,
        access_offsets_ptr,
        resource_ids_ptr,
        access_modes_ptr,
        workspace_ptr,
        workspace_len,
        output_ptr,
        output_capacity,
        resume,
        published,
    );
    if status != OK {
        return status;
    }

    let edge_count = rd_u32(head, OUT_BASE_OFF + 8) as usize;
    let records_end = OUT_BASE_OFF + output_capacity as usize;
    if records_end > head.len() {
        return ERR_RANGE;
    }
    let records = &head[OUT_BASE_OFF + OUT_HEADER_BYTES..records_end];
    levels::compute_execution_levels(records, edge_count, levels, call_count as usize)
}

/// Plans, layers, and then runs Critical Path Intelligence over the same graph.
/// Pointers are arena-relative offsets.
///
/// The durations region is host-supplied input and the timing region is engine
/// output; both must be the canonical regions. The CPM pass reads only the graph
/// the planner just emitted and the durations the host just wrote, so it can
/// never schedule from a stale plan.
///
/// # Safety
/// `arena` must be at least `TIMING_BASE_OFF + TIMING_REGION_LEN` bytes long.
pub unsafe fn plan_arena_v3(
    arena: &mut [u8],
    abi_version: u32,
    call_count: u32,
    access_count: u32,
    resource_count: u32,
    call_modes_ptr: u32,
    access_offsets_ptr: u32,
    resource_ids_ptr: u32,
    access_modes_ptr: u32,
    workspace_ptr: u32,
    workspace_len: u32,
    output_ptr: u32,
    output_capacity: u32,
    levels_ptr: u32,
    levels_len: u32,
    durations_ptr: u32,
    durations_len: u32,
    timing_ptr: u32,
    timing_len: u32,
) -> i32 {
    plan_arena_v3_resuming(
        arena,
        abi_version,
        call_count,
        access_count,
        resource_count,
        call_modes_ptr,
        access_offsets_ptr,
        resource_ids_ptr,
        access_modes_ptr,
        workspace_ptr,
        workspace_len,
        output_ptr,
        output_capacity,
        levels_ptr,
        levels_len,
        durations_ptr,
        durations_len,
        timing_ptr,
        timing_len,
        None,
        None,
    )
}

/// `plan_arena_v3` with an optional resume point.
///
/// # Safety
/// As `plan_arena_v3`.
unsafe fn plan_arena_v3_resuming(
    arena: &mut [u8],
    abi_version: u32,
    call_count: u32,
    access_count: u32,
    resource_count: u32,
    call_modes_ptr: u32,
    access_offsets_ptr: u32,
    resource_ids_ptr: u32,
    access_modes_ptr: u32,
    workspace_ptr: u32,
    workspace_len: u32,
    output_ptr: u32,
    output_capacity: u32,
    levels_ptr: u32,
    levels_len: u32,
    durations_ptr: u32,
    durations_len: u32,
    timing_ptr: u32,
    timing_len: u32,
    resume: Option<checkpoint::Resume>,
    published: Option<&mut checkpoint::Resume>,
) -> i32 {
    if durations_ptr as usize != DURATIONS_BASE_OFF || timing_ptr as usize != TIMING_BASE_OFF {
        return ERR_LAYOUT;
    }
    if durations_len as usize != DURATIONS_REGION_LEN || timing_len as usize != TIMING_REGION_LEN {
        return ERR_TIMING_CAP;
    }
    if arena.len() < TIMING_BASE_OFF + TIMING_REGION_LEN {
        return ERR_RANGE;
    }

    let (head, tail) = arena.split_at_mut(DURATIONS_BASE_OFF);
    let (durations, timing) = tail.split_at_mut(DURATIONS_REGION_LEN);
    // Invalidate the result region before anything runs. If the plan, the level
    // pass or the CPM pass refuses the batch, no header survives to be read as
    // this batch's answer.
    schedule::clear_timing(timing);
    let status = plan_arena_v2_resuming(
        head,
        abi_version,
        call_count,
        access_count,
        resource_count,
        call_modes_ptr,
        access_offsets_ptr,
        resource_ids_ptr,
        access_modes_ptr,
        workspace_ptr,
        workspace_len,
        output_ptr,
        output_capacity,
        levels_ptr,
        levels_len,
        resume,
        published,
    );
    if status != OK {
        return status;
    }

    let edge_count = rd_u32(head, OUT_BASE_OFF + 8) as usize;
    let records_end = OUT_BASE_OFF + output_capacity as usize;
    if records_end > head.len() {
        return ERR_RANGE;
    }
    let records = &head[OUT_BASE_OFF + OUT_HEADER_BYTES..records_end];
    schedule::compute_schedule(records, edge_count, durations, timing, call_count as usize)
}

/// Plans, layers, times and then commits a checkpoint record describing the
/// state the arena is now in. Pointers are arena-relative offsets.
///
/// The checkpoint region is the resume mechanism's whole interface: the host
/// leaves it zeroed for a batch planned from nothing, or writes back the record
/// and the state it saved with it — the frontier and event arrays in the
/// workspace region, the emitted records in the output region — and the engine
/// either proves that state belongs to this input prefix or refuses. Nothing
/// about a session is stored in the arena between calls, so the linear-memory
/// budget Phase 3 shipped with is unchanged.
///
/// # Safety
/// `arena` must be at least `CHECKPOINT_BASE_OFF + CHECKPOINT_REGION_LEN` bytes
/// long. A restored state is host-supplied data and is validated completely
/// before a single call is planned from it.
pub unsafe fn plan_arena_v4(
    arena: &mut [u8],
    abi_version: u32,
    call_count: u32,
    access_count: u32,
    resource_count: u32,
    call_modes_ptr: u32,
    access_offsets_ptr: u32,
    resource_ids_ptr: u32,
    access_modes_ptr: u32,
    workspace_ptr: u32,
    workspace_len: u32,
    output_ptr: u32,
    output_capacity: u32,
    levels_ptr: u32,
    levels_len: u32,
    durations_ptr: u32,
    durations_len: u32,
    timing_ptr: u32,
    timing_len: u32,
    checkpoint_ptr: u32,
    checkpoint_len: u32,
) -> i32 {
    if checkpoint_ptr as usize != CHECKPOINT_BASE_OFF {
        return ERR_LAYOUT;
    }
    if checkpoint_len as usize != CHECKPOINT_REGION_LEN {
        return ERR_CHECKPOINT_RANGE;
    }
    if arena.len() < CHECKPOINT_BASE_OFF + CHECKPOINT_REGION_LEN {
        return ERR_RANGE;
    }

    let (head, record) = arena.split_at_mut(CHECKPOINT_BASE_OFF);
    let pushed = match checkpoint::parse(record) {
        Some(pushed) => pushed,
        // A record whose shape is not readable is not "no checkpoint": it is a
        // host that has written something the engine never published.
        None => return ERR_CHECKPOINT_INVALID,
    };
    let resume = pushed.resume;
    checkpoint::clear(record);

    let mut published = checkpoint::Resume {
        calls: 0,
        events: 0,
        edges: 0,
        digest: 0,
        flags: pushed.flags,
    };
    let without_timing = pushed.flags & checkpoint::FLAG_WITHOUT_TIMING != 0;
    let status = if without_timing {
        // The session wants a graph and waves only. Skipping the scheduling
        // pass also means the timing region is left exactly as the lower entry
        // points leave it, which is the Phase 2 contract, not a new one.
        plan_arena_v2_resuming(
            head,
            abi_version,
            call_count,
            access_count,
            resource_count,
            call_modes_ptr,
            access_offsets_ptr,
            resource_ids_ptr,
            access_modes_ptr,
            workspace_ptr,
            workspace_len,
            output_ptr,
            output_capacity,
            levels_ptr,
            levels_len,
            resume,
            Some(&mut published),
        )
    } else {
        plan_arena_v3_resuming(
            head,
            abi_version,
            call_count,
            access_count,
            resource_count,
            call_modes_ptr,
            access_offsets_ptr,
            resource_ids_ptr,
            access_modes_ptr,
            workspace_ptr,
            workspace_len,
            output_ptr,
            output_capacity,
            levels_ptr,
            levels_len,
            durations_ptr,
            durations_len,
            timing_ptr,
            timing_len,
            resume,
            Some(&mut published),
        )
    };
    if status != OK {
        return status;
    }

    let accesses = access_count as usize;
    let views = match head.get(..CHECKPOINT_BASE_OFF) {
        Some(views) => views,
        None => return ERR_RANGE,
    };
    let Some(call_modes) = region(views, IN_CALL_MODES_OFF, IN_CALL_MODES_LEN) else {
        return ERR_RANGE;
    };
    let Some(offsets) = region(views, IN_ACCESS_OFFSETS_OFF, IN_ACCESS_OFFSETS_LEN) else {
        return ERR_RANGE;
    };
    let Some(resource_ids) = region(views, IN_RESOURCE_IDS_OFF, IN_RESOURCE_IDS_LEN) else {
        return ERR_RANGE;
    };
    let Some(access_modes) = region(views, IN_ACCESS_MODES_OFF, IN_ACCESS_MODES_LEN) else {
        return ERR_RANGE;
    };
    let Some(workspace) = region(views, WS_BASE_OFF, WORKSPACE_REGION_LEN) else {
        return ERR_RANGE;
    };
    let (last_writer, rest) = workspace.split_at(WS_LAST_WRITER_LEN);
    let (reader_head, rest) = rest.split_at(WS_READER_HEAD_LEN);
    let (event_next, event_op) = rest.split_at(WS_EVENT_NEXT_LEN);
    let Some(records) = region(views, OUT_BASE_OFF + OUT_HEADER_BYTES, OUT_RECORDS_LEN) else {
        return ERR_RANGE;
    };

    // The run consumed the whole batch, so the committed boundary is the end of
    // it: the host can now append at this point without touching the prefix.
    published.digest = checkpoint::digest(
        call_modes,
        offsets,
        resource_ids,
        access_modes,
        last_writer,
        reader_head,
        event_op,
        event_next,
        records,
        published.calls,
        accesses,
        published.events,
        published.edges,
    );
    if !checkpoint::commit(record, &published) {
        return ERR_INTERNAL;
    }
    OK
}

/// Base address of the arena, so the host can turn offsets into absolute
/// linear-memory pointers. It is `0` off-target, where there is no linear
/// memory and the offsets act as pointers directly.
#[inline]
pub fn arena_base() -> usize {
    #[cfg(target_arch = "wasm32")]
    {
        core::ptr::addr_of_mut!(ARENA) as usize
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        0
    }
}

unsafe fn arena_slice() -> &'static mut [u8] {
    core::slice::from_raw_parts_mut(core::ptr::addr_of_mut!(ARENA) as *mut u8, ARENA_BYTES)
}

/// Introspection of engine constants and the arena layout, so the host never
/// hardcodes an offset or a ceiling. Unknown kinds return `SENTINEL`.
#[no_mangle]
pub extern "C" fn effectplan_info(kind: u32) -> u32 {
    let base = arena_base() as u32;
    #[allow(clippy::match_same_arms)]
    match kind {
        0 => ABI_VERSION,
        1 => base,
        2 => ARENA_BYTES as u32,
        3 => base + IN_CALL_MODES_OFF as u32,
        4 => INPUT_REGION_LEN as u32,
        5 => base + WS_BASE_OFF as u32,
        6 => WORKSPACE_REGION_LEN as u32,
        7 => base + OUT_BASE_OFF as u32,
        8 => OUTPUT_REGION_LEN as u32,
        9 => MAX_CALLS as u32,
        10 => MAX_ACCESS as u32,
        11 => MAX_RESOURCES as u32,
        12 => MAX_EVENTS as u32,
        13 => MAX_EDGES as u32,
        14 => EDGE_BYTES as u32,
        15 => MAX_NAMESPACE_BYTES as u32,
        16 => MAX_RESOURCE_ID_BYTES as u32,
        17 => OUT_HEADER_BYTES as u32,
        18 => RES_SLOTS as u32,
        19 => (MAX_EDGES * EDGE_BYTES) as u32,
        20 => ABI_VERSION, // engine feature marker: reason-code revision
        21 => PLAN_V2_REVISION,
        22 => base + LEVELS_BASE_OFF as u32,
        23 => LEVELS_REGION_LEN as u32,
        24 => LEVELS_MAX_ENTRIES as u32,
        25 => TIMING_REVISION,
        26 => base + DURATIONS_BASE_OFF as u32,
        27 => DURATIONS_REGION_LEN as u32,
        28 => DURATIONS_MAX_ENTRIES as u32,
        29 => base + TIMING_BASE_OFF as u32,
        30 => TIMING_REGION_LEN as u32,
        31 => TIMING_MAX_ENTRIES as u32,
        32 => checkpoint::CHECKPOINT_REVISION,
        33 => base + CHECKPOINT_BASE_OFF as u32,
        34 => CHECKPOINT_REGION_LEN as u32,
        35 => resources::RESOURCE_REVISION,
        36 => base + CAPACITIES_BASE_OFF as u32,
        37 => CAPACITIES_REGION_LEN as u32,
        38 => base + DEMAND_OFFSETS_BASE_OFF as u32,
        39 => DEMAND_OFFSETS_REGION_LEN as u32,
        40 => base + DEMAND_RECORDS_BASE_OFF as u32,
        41 => DEMAND_RECORDS_REGION_LEN as u32,
        42 => base + SCHEDULE_BASE_OFF as u32,
        43 => SCHEDULE_REGION_LEN as u32,
        44 => base + RESOURCE_STATS_BASE_OFF as u32,
        45 => RESOURCE_STATS_REGION_LEN as u32,
        46 => base + RESOURCE_DIAGNOSTICS_BASE_OFF as u32,
        47 => RESOURCE_DIAGNOSTICS_REGION_LEN as u32,
        48 => resources::MAX_CAPACITY_RESOURCES as u32,
        49 => resources::MAX_DEMAND_RECORDS as u32,
        50 => resources::MAX_RESOURCE_DIAGNOSTICS as u32,
        51 => resources::MAX_CAPACITY,
        _ => SENTINEL,
    }
}

/// The single planning entry point. Pointers are absolute linear-memory
/// addresses; each is converted with checked arithmetic and must match the
/// canonical region base exactly.
///
/// # Safety
/// Intended to be called from WebAssembly with host-supplied addresses. Every
/// argument is validated before any byte is read or written; a malformed call
/// returns a status code.
#[no_mangle]
pub unsafe extern "C" fn effectplan_plan(
    abi_version: u32,
    call_count: u32,
    access_count: u32,
    resource_count: u32,
    call_modes_ptr: u32,
    access_offsets_ptr: u32,
    resource_ids_ptr: u32,
    access_modes_ptr: u32,
    workspace_ptr: u32,
    workspace_len: u32,
    output_ptr: u32,
    output_capacity: u32,
) -> i32 {
    let base = arena_base();
    let to_off = |p: u32| -> Option<u32> {
        (p as usize)
            .checked_sub(base)
            .and_then(|d| u32::try_from(d).ok())
    };
    let converted = [
        to_off(call_modes_ptr),
        to_off(access_offsets_ptr),
        to_off(resource_ids_ptr),
        to_off(access_modes_ptr),
        to_off(workspace_ptr),
        to_off(output_ptr),
    ];
    let mut offs = [0u32; 6];
    for (slot, value) in offs.iter_mut().zip(converted.iter()) {
        match value {
            Some(v) => *slot = *v,
            None => return ERR_RANGE,
        }
    }
    plan_arena(
        arena_slice(),
        abi_version,
        call_count,
        access_count,
        resource_count,
        offs[0],
        offs[1],
        offs[2],
        offs[3],
        offs[4],
        workspace_len,
        offs[5],
        output_capacity,
    )
}

/// The extended planning entry point: the graph, and the execution levels that
/// say which calls may run in parallel. Pointers are absolute linear-memory
/// addresses; each is converted with checked arithmetic and must match the
/// canonical region base exactly.
///
/// The twelve arguments of `effectplan_plan` keep their meaning. The two
/// extra arguments name the levels region, which must be the canonical one.
///
/// # Safety
/// Intended to be called from WebAssembly with host-supplied addresses. Every
/// argument is validated before any byte is read or written; a malformed call
/// returns a status code.
#[no_mangle]
pub unsafe extern "C" fn effectplan_plan_v2(
    abi_version: u32,
    call_count: u32,
    access_count: u32,
    resource_count: u32,
    call_modes_ptr: u32,
    access_offsets_ptr: u32,
    resource_ids_ptr: u32,
    access_modes_ptr: u32,
    workspace_ptr: u32,
    workspace_len: u32,
    output_ptr: u32,
    output_capacity: u32,
    levels_ptr: u32,
    levels_len: u32,
) -> i32 {
    let base = arena_base();
    let to_off = |p: u32| -> Option<u32> {
        (p as usize)
            .checked_sub(base)
            .and_then(|d| u32::try_from(d).ok())
    };
    let converted = [
        to_off(call_modes_ptr),
        to_off(access_offsets_ptr),
        to_off(resource_ids_ptr),
        to_off(access_modes_ptr),
        to_off(workspace_ptr),
        to_off(output_ptr),
        to_off(levels_ptr),
    ];
    let mut offs = [0u32; 7];
    for (slot, value) in offs.iter_mut().zip(converted.iter()) {
        match value {
            Some(v) => *slot = *v,
            None => return ERR_RANGE,
        }
    }
    plan_arena_v2(
        arena_slice(),
        abi_version,
        call_count,
        access_count,
        resource_count,
        offs[0],
        offs[1],
        offs[2],
        offs[3],
        offs[4],
        workspace_len,
        offs[5],
        output_capacity,
        offs[6],
        levels_len,
    )
}

/// The scheduling entry point: the graph, the execution levels, and the
/// duration-aware analysis — earliest and latest times, slack, critical events
/// and the concurrency envelope.
///
/// The fourteen arguments of `effectplan_plan_v2` keep their meaning. The four
/// extra arguments name the durations region (host input) and the timing region
/// (engine output), each validated against its canonical base exactly like
/// every other pointer here.
///
/// # Safety
/// Intended to be called from WebAssembly with host-supplied addresses. Every
/// argument is validated before any byte is read or written; a malformed call
/// returns a status code.
#[no_mangle]
pub unsafe extern "C" fn effectplan_plan_v3(
    abi_version: u32,
    call_count: u32,
    access_count: u32,
    resource_count: u32,
    call_modes_ptr: u32,
    access_offsets_ptr: u32,
    resource_ids_ptr: u32,
    access_modes_ptr: u32,
    workspace_ptr: u32,
    workspace_len: u32,
    output_ptr: u32,
    output_capacity: u32,
    levels_ptr: u32,
    levels_len: u32,
    durations_ptr: u32,
    durations_len: u32,
    timing_ptr: u32,
    timing_len: u32,
) -> i32 {
    let base = arena_base();
    let to_off = |p: u32| -> Option<u32> {
        (p as usize)
            .checked_sub(base)
            .and_then(|d| u32::try_from(d).ok())
    };
    let converted = [
        to_off(call_modes_ptr),
        to_off(access_offsets_ptr),
        to_off(resource_ids_ptr),
        to_off(access_modes_ptr),
        to_off(workspace_ptr),
        to_off(output_ptr),
        to_off(levels_ptr),
        to_off(durations_ptr),
        to_off(timing_ptr),
    ];
    let mut offs = [0u32; 9];
    for (slot, value) in offs.iter_mut().zip(converted.iter()) {
        match value {
            Some(v) => *slot = *v,
            None => return ERR_RANGE,
        }
    }
    plan_arena_v3(
        arena_slice(),
        abi_version,
        call_count,
        access_count,
        resource_count,
        offs[0],
        offs[1],
        offs[2],
        offs[3],
        offs[4],
        workspace_len,
        offs[5],
        output_capacity,
        offs[6],
        levels_len,
        offs[7],
        durations_len,
        offs[8],
        timing_len,
    )
}

/// The resumable entry point: everything `effectplan_plan_v3` produces, plus a
/// committed checkpoint of the planning state at the end of the run.
///
/// The nineteen arguments of `effectplan_plan_v3` keep their meaning; the two
/// extra arguments name the checkpoint region. What makes an update incremental
/// is not in the argument list: it is whether the host has written a readable
/// record into that region and restored the frontier, the event arrays and the
/// emitted records that go with it. An empty record plans the batch from
/// nothing, exactly as `v3` does, and still commits a checkpoint.
///
/// # Safety
/// Intended to be called from WebAssembly with host-supplied addresses. A
/// restored state is host data: it is range-checked, structurally checked and
/// digest-checked before the first call of a suffix is planned.
#[no_mangle]
pub unsafe extern "C" fn effectplan_plan_v4(
    abi_version: u32,
    call_count: u32,
    access_count: u32,
    resource_count: u32,
    call_modes_ptr: u32,
    access_offsets_ptr: u32,
    resource_ids_ptr: u32,
    access_modes_ptr: u32,
    workspace_ptr: u32,
    workspace_len: u32,
    output_ptr: u32,
    output_capacity: u32,
    levels_ptr: u32,
    levels_len: u32,
    durations_ptr: u32,
    durations_len: u32,
    timing_ptr: u32,
    timing_len: u32,
    checkpoint_ptr: u32,
    checkpoint_len: u32,
) -> i32 {
    let base = arena_base();
    let to_off = |p: u32| -> Option<u32> {
        (p as usize)
            .checked_sub(base)
            .and_then(|d| u32::try_from(d).ok())
    };
    let converted = [
        to_off(call_modes_ptr),
        to_off(access_offsets_ptr),
        to_off(resource_ids_ptr),
        to_off(access_modes_ptr),
        to_off(workspace_ptr),
        to_off(output_ptr),
        to_off(levels_ptr),
        to_off(durations_ptr),
        to_off(timing_ptr),
        to_off(checkpoint_ptr),
    ];
    let mut offs = [0u32; 10];
    for (slot, value) in offs.iter_mut().zip(converted.iter()) {
        match value {
            Some(v) => *slot = *v,
            None => return ERR_RANGE,
        }
    }
    plan_arena_v4(
        arena_slice(),
        abi_version,
        call_count,
        access_count,
        resource_count,
        offs[0],
        offs[1],
        offs[2],
        offs[3],
        offs[4],
        workspace_len,
        offs[5],
        output_capacity,
        offs[6],
        levels_len,
        offs[7],
        durations_len,
        offs[8],
        timing_len,
        offs[9],
        checkpoint_len,
    )
}

/// The Phase 5 feasibility entry point: validate a declared capacity table and
/// its per-call demands, and report why the declarations cannot be satisfied —
/// by index, never by a caller's identifier.
///
/// This call schedules nothing. It answers the question that comes before any
/// schedule: is what the caller asked for possible at all. Separating it from the
/// planner also keeps the two resource models apart: nothing here reads or writes
/// the dependency frontier, and no capacity constraint can become a fake edge.
///
/// # Safety
/// Intended to be called from WebAssembly with host-supplied addresses. Every
/// pointer must name its canonical region exactly; every count is checked before
/// a byte is read. A malformed call returns a status code.
#[no_mangle]
pub unsafe extern "C" fn effectplan_resource_check(
    abi_version: u32,
    call_count: u32,
    resource_count: u32,
    demand_count: u32,
    capacities_ptr: u32,
    capacities_len: u32,
    offsets_ptr: u32,
    offsets_len: u32,
    records_ptr: u32,
    records_len: u32,
    diagnostics_ptr: u32,
    diagnostics_len: u32,
) -> i32 {
    if abi_version != ABI_VERSION {
        return ERR_ABI_VERSION;
    }
    if call_count as usize > MAX_CALLS {
        return ERR_CALL_LIMIT;
    }
    let base = arena_base();
    let given = [
        (capacities_ptr, capacities_len),
        (offsets_ptr, offsets_len),
        (records_ptr, records_len),
        (diagnostics_ptr, diagnostics_len),
    ];
    let expected = [
        (CAPACITIES_BASE_OFF, CAPACITIES_REGION_LEN),
        (DEMAND_OFFSETS_BASE_OFF, DEMAND_OFFSETS_REGION_LEN),
        (DEMAND_RECORDS_BASE_OFF, DEMAND_RECORDS_REGION_LEN),
        (
            RESOURCE_DIAGNOSTICS_BASE_OFF,
            RESOURCE_DIAGNOSTICS_REGION_LEN,
        ),
    ];
    let mut region_offs = [0u32; 4];
    for (slot, (given_pair, want)) in region_offs
        .iter_mut()
        .zip(given.iter().zip(expected.iter()))
    {
        let relative = match (given_pair.0 as usize)
            .checked_sub(base)
            .and_then(|delta| u32::try_from(delta).ok())
        {
            Some(delta) => delta,
            None => return ERR_RANGE,
        };
        // A pointer that merely lands inside the arena could alias another
        // region, so each argument must name its own region's base exactly.
        if relative as usize != want.0 || given_pair.1 as usize != want.1 {
            return ERR_LAYOUT;
        }
        *slot = relative;
    }

    let arena = arena_slice();
    let diag_at = region_offs[3] as usize;
    if !resources::clear(arena, diag_at) {
        return ERR_RANGE;
    }
    let report = resources::check(
        arena,
        &resources::Request {
            capacities: region_offs[0] as usize,
            offsets: region_offs[1] as usize,
            records: region_offs[2] as usize,
            diagnostics: diag_at,
            call_count: call_count as usize,
            resource_count: resource_count as usize,
            demand_count: demand_count as usize,
        },
    );
    report.status
}

/// The Phase 5 placement entry point: given a graph the planner already emitted,
/// the durations it was given and a declared capacity table, produce a schedule
/// that respects every dependency and never exceeds any capacity.
///
/// It reads the planner's regions and writes only the resource ones. That
/// separation is the point: a change to a duration or a demand can be re-placed
/// without rebuilding the dependency graph, which is what the specification's
/// scheduling-invalidation rule requires and what a single all-in-one call cannot
/// express. The output is a proposed schedule under the declared model, never an
/// executed one, and it makes no claim of optimality.
///
/// # Safety
/// Intended to be called from WebAssembly with host-supplied addresses. Every
/// pointer must name its canonical region; every count is checked before a byte is
/// read. A malformed call returns a status code.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn effectplan_resource_schedule(
    abi_version: u32,
    call_count: u32,
    resource_count: u32,
    demand_count: u32,
    capacities_ptr: u32,
    capacities_len: u32,
    offsets_ptr: u32,
    offsets_len: u32,
    records_ptr: u32,
    records_len: u32,
    durations_ptr: u32,
    durations_len: u32,
    graph_ptr: u32,
    graph_len: u32,
    timing_ptr: u32,
    timing_len: u32,
    schedule_ptr: u32,
    schedule_len: u32,
    stats_ptr: u32,
    stats_len: u32,
    diagnostics_ptr: u32,
    diagnostics_len: u32,
) -> i32 {
    if abi_version != ABI_VERSION {
        return ERR_ABI_VERSION;
    }
    if call_count as usize > MAX_CALLS {
        return ERR_CALL_LIMIT;
    }
    if resource_count as usize > resources::MAX_CAPACITY_RESOURCES
        || demand_count as usize > resources::MAX_DEMAND_RECORDS
    {
        return ERR_RESOURCE_REGION;
    }
    let base = arena_base();
    let given = [
        (capacities_ptr, capacities_len),
        (offsets_ptr, offsets_len),
        (records_ptr, records_len),
        (durations_ptr, durations_len),
        (graph_ptr, graph_len),
        (timing_ptr, timing_len),
        (schedule_ptr, schedule_len),
        (stats_ptr, stats_len),
        (diagnostics_ptr, diagnostics_len),
    ];
    let expected = [
        (CAPACITIES_BASE_OFF, CAPACITIES_REGION_LEN),
        (DEMAND_OFFSETS_BASE_OFF, DEMAND_OFFSETS_REGION_LEN),
        (DEMAND_RECORDS_BASE_OFF, DEMAND_RECORDS_REGION_LEN),
        (DURATIONS_BASE_OFF, DURATIONS_REGION_LEN),
        (OUT_BASE_OFF, OUTPUT_REGION_LEN),
        (TIMING_BASE_OFF, TIMING_REGION_LEN),
        (SCHEDULE_BASE_OFF, SCHEDULE_REGION_LEN),
        (RESOURCE_STATS_BASE_OFF, RESOURCE_STATS_REGION_LEN),
        (
            RESOURCE_DIAGNOSTICS_BASE_OFF,
            RESOURCE_DIAGNOSTICS_REGION_LEN,
        ),
    ];
    let mut at = [0u32; 9];
    for (slot, (given_pair, want)) in at.iter_mut().zip(given.iter().zip(expected.iter())) {
        let relative = match (given_pair.0 as usize)
            .checked_sub(base)
            .and_then(|delta| u32::try_from(delta).ok())
        {
            Some(delta) => delta,
            None => return ERR_RANGE,
        };
        if relative as usize != want.0 || given_pair.1 as usize != want.1 {
            return ERR_LAYOUT;
        }
        *slot = relative;
    }

    let arena = arena_slice();
    // The diagnostics region is published before anything is written into it,
    // exactly as the check entry point does: the host reads a revision there
    // and must never find a stale one from an earlier run.
    let diag_at = at[8] as usize;
    if !resources::clear(arena, diag_at) {
        return ERR_RANGE;
    }
    // Feasibility first, and only its verdict is allowed to reach placement: a
    // demand that can never be met is reported before a schedule is pretended.
    let report = resources::check(
        arena,
        &resources::Request {
            capacities: at[0] as usize,
            offsets: at[1] as usize,
            records: at[2] as usize,
            diagnostics: diag_at,
            call_count: call_count as usize,
            resource_count: resource_count as usize,
            demand_count: demand_count as usize,
        },
    );
    if report.status != OK {
        return report.status;
    }
    scheduling::place(
        arena,
        &scheduling::Request {
            call_count: call_count as usize,
            resource_count: resource_count as usize,
            demand_count: demand_count as usize,
            capacities: at[0] as usize,
            demand_offsets: at[1] as usize,
            demand_records: at[2] as usize,
            durations: at[3] as usize,
            graph: at[4] as usize,
            timing: at[5] as usize,
            schedule: at[6] as usize,
            stats: at[7] as usize,
            diagnostics: at[8] as usize,
        },
    )
}
