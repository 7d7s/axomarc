//! Critical Path Intelligence: durations, earliest and latest times, slack,
//! critical events and the concurrency envelope.
//!
//! The math is textbook CPM. What is specific to this engine is how the passes
//! are scheduled over memory:
//!
//! The planner emits its records grouped by successor, in ascending successor
//! order, and every record points forward (see `levels.rs`). Two consequences:
//!
//! * The **forward pass** reads the records once, ascending. When a record is
//!   reached, everything that could raise its predecessor's earliest finish has
//!   already been read, because that predecessor is a smaller call index.
//! * The **backward pass** reads the *same* records once, descending. A node's
//!   latest finish is final before it is used, because all of that node's
//!   successors carry larger indices and sit later in the array.
//!
//! So neither pass needs reverse adjacency, an indegree array or a queue, and
//! neither ever scans all edges for one vertex. Both are O(V + E) over the
//! emitted graph.
//!
//! All arithmetic is on integer ticks in `u32` with checked addition and
//! subtraction: an impossible plan is reported as a status code, never wrapped
//! into a plausible-looking number. `total_work` accumulates in `u64` because
//! 128 maximum durations do not fit in 32 bits.
//!
//! Peak concurrency sweeps half-open intervals `[ES, EF)`: an operation
//! finishing at tick 5 and one starting at tick 5 do not overlap. Endpoints are
//! sorted in place by (time ascending, finish before start) with a heap sort —
//! deterministic, in-place, O(V log V), no allocation.

use crate::{
    rd_u32, wr_u32, DURATIONS_REGION_LEN, EDGE_BYTES, ERR_DURATION_ZERO, ERR_OUTPUT_INVALID,
    ERR_TIMING_INVALID, ERR_TIMING_OVERFLOW, MAX_CALLS, OK, TIMING_EVENT_BYTES,
    TIMING_HEADER_BYTES, TIMING_REGION_LEN, TIMING_REVISION,
};

/// Field offsets inside the timing region: a header, then one record per call
/// holding `earliest_start` and `slack`. Latest start and both finishes are
/// derived by the caller from the duration it already supplied
/// (`LS = ES + S`, `EF = ES + D`, `LF = LS + D`), which keeps two whole arrays
/// out of the arena.
pub const T_REVISION: usize = 0;
pub const T_EVENT_COUNT: usize = 4;
pub const T_MAKESPAN: usize = 8;
pub const T_PEAK: usize = 12;
pub const T_TOTAL_WORK: usize = 16; // u64, little-endian
pub const T_CRITICAL_COUNT: usize = 24;
pub const T_RESERVED: usize = 28;
pub const T_EVENTS: usize = TIMING_HEADER_BYTES;
/// Field offsets inside one per-call timing record. Named where they are written
/// so that a reader cannot guess a different layout, and so the certificate that
/// reads them cannot drift from the pass that emits them.
pub const T_EVENT_EARLIEST: usize = 0;
pub const T_EVENT_SLACK: usize = 4;

#[inline]
fn le32(row: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([row[at], row[at + 1], row[at + 2], row[at + 3]])
}

#[inline]
pub(crate) fn wr_u64(buf: &mut [u8], at: usize, value: u64) -> bool {
    if buf.len() < at.saturating_add(8) {
        return false;
    }
    buf[at..at + 8].copy_from_slice(&value.to_le_bytes());
    true
}

/// Every call must carry a positive duration. A missing entry reads as zero and
/// is refused here rather than silently becoming free work.
fn validate_durations(durations: &[u8], call_count: usize) -> i32 {
    if durations.len() < DURATIONS_REGION_LEN || call_count > MAX_CALLS {
        return ERR_TIMING_INVALID;
    }
    for call in 0..call_count {
        if rd_u32(durations, call.saturating_mul(4)) == 0 {
            return ERR_DURATION_ZERO;
        }
    }
    OK
}

/// Packed interval endpoint: the time in the high bits and the kind in the low
/// bit (0 = finish, 1 = start), so an ascending sort places a finish at tick `t`
/// before a start at the same tick.
#[inline]
fn endpoint(time: u32, is_start: bool) -> u64 {
    ((time as u64) << 1) | u64::from(is_start)
}

/// In-place heap sort over a fixed window: O(n log n), no allocation, total
/// order on `(time, kind)` so the sweep never depends on input order.
pub(crate) fn heap_sort(endpoints: &mut [u64]) {
    let len = endpoints.len();
    if len < 2 {
        return;
    }
    for root in (0..len / 2).rev() {
        sift_down(endpoints, root, len);
    }
    let mut size = len;
    while size > 1 {
        size -= 1;
        endpoints.swap(0, size);
        sift_down(endpoints, 0, size);
    }
}

fn sift_down(endpoints: &mut [u64], mut root: usize, size: usize) {
    loop {
        let left = root.saturating_mul(2).saturating_add(1);
        if left >= size {
            return;
        }
        let right = left.saturating_add(1);
        let mut child = left;
        if right < size && endpoints[right] > endpoints[left] {
            child = right;
        }
        if endpoints[root] >= endpoints[child] {
            return;
        }
        endpoints.swap(root, child);
        root = child;
    }
}

/// The maximum number of intervals alive at one tick, half-open semantics.
fn compute_peak_concurrency(
    earliest: &[u32; MAX_CALLS],
    duration: &[u32; MAX_CALLS],
    call_count: usize,
) -> u32 {
    if call_count == 0 {
        return 0;
    }
    let mut endpoints: [u64; MAX_CALLS * 2] = [0; MAX_CALLS * 2];
    for call in 0..call_count {
        let finish = earliest[call].saturating_add(duration[call]);
        endpoints[call.saturating_mul(2)] = endpoint(finish, false);
        endpoints[call.saturating_mul(2).saturating_add(1)] = endpoint(earliest[call], true);
    }
    let window = &mut endpoints[..call_count.saturating_mul(2)];
    heap_sort(window);
    let mut active = 0i64;
    let mut peak = 0i64;
    for value in window.iter() {
        if value & 1 == 1 {
            active += 1;
            peak = peak.max(active);
        } else {
            active -= 1;
        }
    }
    if active != 0 || peak < 0 {
        return u32::MAX; // the sweep did not balance; the audit below refuses
    }
    peak as u32
}

/// Runs the CPM passes over the emitted graph and writes the timing region.
///
/// `records` is the graph the planner just produced, `durations` the canonical
/// durations region the host wrote, `timing` the canonical timing region this
/// function owns. Nothing here trusts a pointer: the caller checked that each
/// one names its canonical region before splitting the arena.
pub fn compute_schedule(
    records: &[u8],
    edge_count: usize,
    durations: &[u8],
    timing: &mut [u8],
    call_count: usize,
) -> i32 {
    if timing.len() < TIMING_REGION_LEN {
        return ERR_TIMING_INVALID;
    }
    let status = validate_durations(durations, call_count);
    if status != OK {
        return status;
    }
    let used = match edge_count.checked_mul(EDGE_BYTES) {
        Some(needed) => needed,
        None => return ERR_OUTPUT_INVALID,
    };
    let live = match records.get(..used) {
        Some(window) => window,
        None => return ERR_OUTPUT_INVALID,
    };
    // Exact 16-byte rows: the planner writes one record per dependency, so a
    // window that does not divide leaves no partial row to guess about.
    let rows = live.chunks_exact(EDGE_BYTES);

    let mut duration = [0u32; MAX_CALLS];
    for call in 0..call_count {
        duration[call] = rd_u32(durations, call.saturating_mul(4));
    }

    // ---- Forward pass: earliest start, ascending over the records.
    let mut earliest = [0u32; MAX_CALLS];
    for row in rows.clone() {
        let pred = le32(row, 0) as usize;
        let succ = le32(row, 4) as usize;
        if pred >= succ || succ >= call_count {
            return ERR_OUTPUT_INVALID;
        }
        let pred_finish = match earliest[pred].checked_add(duration[pred]) {
            Some(value) => value,
            None => return ERR_TIMING_OVERFLOW,
        };
        if pred_finish > earliest[succ] {
            earliest[succ] = pred_finish;
        }
    }

    // Makespan and total work.
    let mut makespan = 0u32;
    let mut total_work = 0u64;
    for call in 0..call_count {
        let finish = match earliest[call].checked_add(duration[call]) {
            Some(value) => value,
            None => return ERR_TIMING_OVERFLOW,
        };
        makespan = makespan.max(finish);
        total_work = total_work.saturating_add(duration[call] as u64);
    }

    // ---- Backward pass, descending over the same records, carrying the
    // precedence audit with it. By the time a record is reached on the way down,
    // `earliest` is final and its successor's `latest_finish` is final, so the
    // audit compares settled values and reads nothing twice.
    let mut latest_finish = [makespan; MAX_CALLS];
    for row in rows.rev() {
        let pred = le32(row, 0) as usize;
        let succ = le32(row, 4) as usize;
        let pred_finish = match earliest[pred].checked_add(duration[pred]) {
            Some(value) => value,
            None => return ERR_TIMING_OVERFLOW,
        };
        if pred_finish > earliest[succ] {
            // A predecessor that finishes after its successor starts could only
            // come from a graph the passes did not actually walk in order.
            return ERR_TIMING_INVALID;
        }
        let succ_start = match latest_finish[succ].checked_sub(duration[succ]) {
            Some(value) => value,
            None => return ERR_TIMING_INVALID,
        };
        if succ_start < latest_finish[pred] {
            latest_finish[pred] = succ_start;
        }
    }

    // ---- Slack and the critical count.
    let mut slack = [0u32; MAX_CALLS];
    let mut critical = 0u32;
    for call in 0..call_count {
        let finish = earliest[call].saturating_add(duration[call]);
        let value = match latest_finish[call].checked_sub(finish) {
            Some(value) => value,
            None => return ERR_TIMING_INVALID, // slack would be negative
        };
        slack[call] = value;
        if value == 0 {
            critical += 1;
        }
    }

    // ---- Concurrency envelope.
    let peak = compute_peak_concurrency(&earliest, &duration, call_count);
    if peak as usize > call_count {
        return ERR_TIMING_INVALID;
    }

    // ---- Write the canonical timing region.
    //
    // The per-call records go first and the header goes last: the header is the
    // commit record. A caller that reads a revision it does not expect knows the
    // pass never finished, so a refused schedule can never be mistaken for a
    // result — including a result from the previous batch.
    let mut ok = wr_u32(timing, T_RESERVED, 0);
    for call in 0..call_count {
        let at = T_EVENTS + call.saturating_mul(TIMING_EVENT_BYTES);
        ok = ok && wr_u32(timing, at + T_EVENT_EARLIEST, earliest[call]);
        ok = ok && wr_u32(timing, at + T_EVENT_SLACK, slack[call]);
    }
    ok = ok
        && wr_u32(timing, T_MAKESPAN, makespan)
        && wr_u32(timing, T_PEAK, peak)
        && wr_u64(timing, T_TOTAL_WORK, total_work)
        && wr_u32(timing, T_CRITICAL_COUNT, critical)
        && wr_u32(timing, T_EVENT_COUNT, call_count as u32)
        && wr_u32(timing, T_REVISION, TIMING_REVISION);
    if !ok {
        return ERR_TIMING_INVALID;
    }
    OK
}

/// Invalidates the timing region before a pass starts, so only a completed pass
/// can leave a readable header.
pub fn clear_timing(timing: &mut [u8]) {
    if timing.len() < TIMING_HEADER_BYTES {
        return;
    }
    wr_u32(timing, T_REVISION, 0);
    wr_u32(timing, T_EVENT_COUNT, 0);
}

const _: () = assert!(TIMING_HEADER_BYTES == 32);
const _: () = assert!(TIMING_REGION_LEN == TIMING_HEADER_BYTES + MAX_CALLS * TIMING_EVENT_BYTES);
const _: () = assert!(DURATIONS_REGION_LEN == MAX_CALLS * 4);
