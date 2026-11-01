//! Execution levels: which calls may run in parallel.
//!
//! The dependency graph the planner emits is a *covering* graph: it names the
//! relationships a host must respect, not every conflicting pair. Its records
//! are ordered by successor, and every record points forward, from a lower
//! call index to a higher one. That is what makes the level computation a
//! single linear pass instead of a topological sort:
//!
//! ```text
//! L(v) = 0                    if v has no predecessor
//! L(v) = 1 + max(L(u))        over the predecessors u of v
//! ```
//!
//! The forward invariant is not an assumption. `Planner::process_read` and
//! `Planner::process_write` only ever relate a call to an *earlier* call —
//! the current writer of a resource was written by an earlier call, and the
//! pending readers of a resource registered before the current call — and
//! `Planner::validate_output` rejects any record that does not point
//! forward. The pass below additionally refuses a record that contradicts the
//! invariant, so a defect would surface as a status code rather than as a
//! wrong wave.
//!
//! The graph is over calls. The engine's internal event list (one node per
//! pending reader) is an implementation detail that never reaches the output,
//! so the level of a call is bounded by the call count, not by the event
//! count.

use crate::{
    rd_u32, wr_u32, EDGE_BYTES, ERR_INTERNAL, ERR_LEVELS_CAP, ERR_LEVELS_INVALID,
    ERR_OUTPUT_INVALID, LEVELS_HEADER_BYTES, LEVELS_MAX_ENTRIES, LEVELS_REGION_LEN, MAX_CALLS, OK,
};

/// Reads one edge record. The records are 16 bytes: predecessor, successor,
/// resource, reason.
#[inline]
fn record(records: &[u8], index: usize) -> (u32, u32, u32, u32) {
    let at = index.saturating_mul(EDGE_BYTES);
    (
        rd_u32(records, at),
        rd_u32(records, at.saturating_add(4)),
        rd_u32(records, at.saturating_add(8)),
        rd_u32(records, at.saturating_add(12)),
    )
}

/// Writes one `u16` little-endian.
#[inline]
fn wr_u16(buf: &mut [u8], at: usize, value: u16) -> bool {
    match buf.get_mut(at..at.saturating_add(2)) {
        Some(s) => {
            s.copy_from_slice(&value.to_le_bytes());
            true
        }
        None => false,
    }
}

/// Computes the execution level of every call and writes them to the canonical
/// levels region.
///
/// `levels` holds a `u32` count followed by one `u16` level per call. The
/// count is written first so a reader can never mistake stale entries for
/// current ones.
pub fn compute_execution_levels(
    records: &[u8],
    edge_count: usize,
    levels: &mut [u8],
    call_count: usize,
) -> i32 {
    if levels.len() < LEVELS_REGION_LEN {
        return ERR_LEVELS_CAP;
    }
    if call_count > MAX_CALLS {
        return ERR_LEVELS_CAP;
    }
    if edge_count.saturating_mul(EDGE_BYTES) > records.len() {
        return ERR_OUTPUT_INVALID;
    }

    let mut scratch = [0u16; MAX_CALLS];

    // One pass over the records. They are ordered by successor, and every
    // predecessor of a record has a smaller index, so the level of a
    // predecessor is already final when its record is read.
    let mut index = 0usize;
    while index < edge_count {
        let (pred, succ, _, _) = record(records, index);
        if pred >= succ || succ >= call_count as u32 {
            // The forward invariant is broken; refuse rather than plan on it.
            return ERR_OUTPUT_INVALID;
        }
        let next = scratch[pred as usize].saturating_add(1);
        if next > scratch[succ as usize] {
            scratch[succ as usize] = next;
        }
        index += 1;
    }

    // Every level must be strictly below the call count: a chain of n calls
    // has n levels, from 0 to n - 1.
    for call in 0..call_count {
        if scratch[call] as usize >= call_count {
            return ERR_LEVELS_INVALID;
        }
    }

    // The levels must explain the graph they came from.
    index = 0;
    while index < edge_count {
        let (pred, succ, _, _) = record(records, index);
        if scratch[succ as usize] <= scratch[pred as usize] {
            return ERR_LEVELS_INVALID;
        }
        index += 1;
    }

    if !wr_u32(levels, 0, call_count as u32) {
        return ERR_INTERNAL;
    }
    for call in 0..call_count {
        let at = LEVELS_HEADER_BYTES + call * 2;
        if !wr_u16(levels, at, scratch[call]) {
            return ERR_INTERNAL;
        }
    }
    OK
}

/// The number of level entries a region of this size can hold.
pub fn level_capacity(region_bytes: usize) -> usize {
    if region_bytes < LEVELS_HEADER_BYTES {
        0
    } else {
        (region_bytes - LEVELS_HEADER_BYTES) / 2
    }
}

/// The maximum level this engine can ever produce: a chain over every call.
pub const MAX_LEVEL: usize = MAX_CALLS - 1;

const _: () = assert!(LEVELS_MAX_ENTRIES == MAX_CALLS);
const _: () = assert!(LEVELS_REGION_LEN == LEVELS_HEADER_BYTES + MAX_CALLS * 2);
