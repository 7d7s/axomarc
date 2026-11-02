//! Deterministic resource-constrained placement (Phase 5, M4 and M5).
//!
//! The question this answers is the one the dependency graph cannot: two calls may
//! be dependency-safe to run together and still be unable to run together, because
//! both need the only GPU. Resource capacity says how many may overlap; the
//! dependency graph says which must wait. They are different facts and this module
//! keeps them apart — it reads the graph the planner emitted and never modifies it.
//!
//! The algorithm is a parallel schedule generation scheme: at each decision tick,
//! take the ready calls that fit, place them, advance to the next completion,
//! release what finished, repeat. It is a heuristic, and the output says so —
//! nothing here claims optimality, because RCPSP in general is not cheaply
//! solvable and pretending otherwise would make every number downstream suspect.
//!
//! Three properties are load-bearing:
//!
//! * **Reservation is atomic.** A call reserves every resource it demands or none.
//!   Partial reservation is how a planner leaks capacity or deadlocks on itself,
//!   and this planner has no locks to deadlock with: it decides, it does not acquire.
//! * **Progress is guaranteed.** Every round either places a call or advances the
//!   clock to a completion that releases capacity. If neither is possible — no
//!   call running, none placeable — then the graph cannot be what it claims to be,
//!   and that is reported as `ERR_UNSCHEDULABLE` instead of looping. A round
//!   budget backs that argument up against any reasoning error: the engine
//!   terminates by construction, not by proof of luck.
//! * **The result is deterministic.** Calls are ranked by (smaller slack, more
//!   downstream dependents, lower index) in a single precomputed order, scanned in
//!   that order every tick. Same bytes in, same schedule out — the property every
//!   phase has shipped with.
//!
//! Half-open intervals, `[start, finish)`, inherited from Phase 3 without
//! amendment: a call finishing at tick 7 releases its capacity for a call starting
//! at tick 7. Were the two layers to disagree about this, the timing report and
//! the resource report would contradict each other about the same plan.

use crate::resources::{
    CAPACITY_BYTES, DEMAND_RECORD_BYTES, DIAGNOSTICS_HEADER_BYTES, DIAG_CONTENTION,
    DIAG_UNSCHEDULABLE, D_CALL, D_KIND, D_RESOURCE, D_VALUE, H_COUNT, MAX_CAPACITY_RESOURCES,
    MAX_RESOURCE_DIAGNOSTICS, RESOURCE_REVISION,
};
use crate::schedule::{heap_sort, wr_u64};
use crate::{
    rd_u32, wr_u32, ABI_VERSION, EDGE_BYTES, ERR_DURATION_ZERO, ERR_OUTPUT_INVALID,
    ERR_RESOURCE_RANGE, ERR_UNSCHEDULABLE, MAX_CALLS, MAX_EDGES, MAX_EVENTS, OK, OUT_HEADER_BYTES,
    RESOURCE_INDEGREE_BASE_OFF, RESOURCE_OUTDEGREE_BASE_OFF, RESOURCE_READY_BASE_OFF,
    RESOURCE_STATS_REGION_LEN, RESOURCE_USAGE_BASE_OFF, SCHEDULE_REGION_LEN, TIMING_HEADER_BYTES,
    TIMING_REVISION,
};

/// Schedule header fields. The header is written only when the placement
/// succeeded, so a host can never read a half-written schedule as a finished one.
pub const S_REVISION: usize = 0;
pub const S_PLACED: usize = 4;
pub const S_MAKESPAN: usize = 8;
pub const S_STATUS: usize = 12;
pub const S_DEFERRALS: usize = 16;
pub const S_ROUNDS: usize = 20;
pub const S_RESERVED_A: usize = 24;
pub const S_RESERVED_B: usize = 28;
pub const S_ENTRIES: usize = 32;

/// Per-resource statistics: the declared capacity, the peak observed, and the
/// busy time in `u64` because `demand × duration` over a batch does not fit in 32
/// bits. Utilisation is deliberately *not* computed here: it is a ratio, and the
/// engine reports both terms of it as integers and lets the host divide.
pub const ST_REVISION: usize = 0;
pub const ST_COUNT: usize = 4;
pub const ST_MAKESPAN: usize = 8;
pub const ST_RESERVED: usize = 12;
pub const ST_ENTRIES: usize = 32;
pub const ST_CAPACITY: usize = 0;
pub const ST_PEAK: usize = 4;
pub const ST_BUSY: usize = 8;

/// Sentinel marking a call whose capacity has been handed back.
const RELEASED: u32 = u32::MAX;

/// Where every input and output lives, and what the batch contains.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Request {
    pub call_count: usize,
    pub resource_count: usize,
    pub demand_count: usize,
    pub capacities: usize,
    pub demand_offsets: usize,
    pub demand_records: usize,
    pub durations: usize,
    /// The planner's output region: header plus edge records.
    pub graph: usize,
    /// The timing region, read for slack when it describes this batch.
    pub timing: usize,
    pub schedule: usize,
    pub stats: usize,
    pub diagnostics: usize,
}

/// Rounds the placement loop may take. Every call is placed once and every
/// placement is followed by at most one completion round, so `4 × calls` is loose
/// by construction; reaching it means the reasoning above is wrong, which is
/// exactly the case a budget exists to catch rather than to hide.
const MAX_ROUNDS: usize = 4 * MAX_CALLS + 8;

#[inline]
fn dur_at(arena: &[u8], r: &Request, call: usize) -> u32 {
    rd_u32(arena, r.durations + call * CAPACITY_BYTES)
}

#[inline]
fn finish_at(arena: &[u8], s: usize, call: usize) -> u32 {
    rd_u32(arena, s + S_ENTRIES + call * 8 + 4)
}

/// Read the edge count from the graph header. The scheduler trusts nothing it
/// cannot check: the header must name this ABI, this call count, a plausible edge
/// count, and zero in the reserved word.
fn graph_edges(arena: &[u8], r: &Request) -> Result<(u32, usize), i32> {
    let abi = rd_u32(arena, r.graph);
    let calls = rd_u32(arena, r.graph + 4);
    let edges = rd_u32(arena, r.graph + 8);
    let reserved = rd_u32(arena, r.graph + 12);
    if abi != ABI_VERSION || calls != r.call_count as u32 || reserved != 0 {
        return Err(ERR_OUTPUT_INVALID);
    }
    if edges as usize > MAX_EDGES {
        return Err(ERR_OUTPUT_INVALID);
    }
    Ok((edges, r.graph + OUT_HEADER_BYTES))
}

/// Place every call. Returns the status: `OK` with a schedule published, or the
/// reason the batch cannot be placed as declared.
///
/// The caller must have run `crate::resources::check` first: this function assumes
/// capacities and demands are well-formed and every demand fits its resource when
/// nothing else is running.
pub fn place(arena: &mut [u8], r: &Request) -> i32 {
    let call_count = r.call_count;
    if call_count > MAX_CALLS || r.resource_count > MAX_CAPACITY_RESOURCES {
        return ERR_OUTPUT_INVALID;
    }

    // Clear both output regions before anything is written, so a refused run can
    // never leave a plausible-looking schedule behind.
    clear_region(arena, r.schedule, SCHEDULE_REGION_LEN);
    clear_region(arena, r.stats, RESOURCE_STATS_REGION_LEN);

    let (edge_count, records_at) = match graph_edges(arena, r) {
        Ok(read) => read,
        Err(status) => {
            publish(arena, r, 0, 0, status, 0, 0);
            return status;
        }
    };

    // One pass over the records builds the predecessor counts, the dependent
    // counts and the ready times, and validates the forward-edge invariants the
    // planner holds: `from < to`, both in range, successors ascending. A graph
    // that fails this did not come from this planner for this batch.
    zero(arena, RESOURCE_INDEGREE_BASE_OFF, MAX_CALLS);
    zero(arena, RESOURCE_READY_BASE_OFF, MAX_CALLS);
    zero(arena, RESOURCE_OUTDEGREE_BASE_OFF, MAX_CALLS);
    let mut previous_successor = 0u32;
    for index in 0..edge_count as usize {
        let at = records_at + index * EDGE_BYTES;
        let from = rd_u32(arena, at);
        let to = rd_u32(arena, at + 4);
        if from >= to || to >= call_count as u32 || to < previous_successor {
            publish(arena, r, 0, 0, ERR_OUTPUT_INVALID, 0, 0);
            return ERR_OUTPUT_INVALID;
        }
        previous_successor = to;
        let indegree_at = RESOURCE_INDEGREE_BASE_OFF + to as usize * CAPACITY_BYTES;
        let outdegree_at = RESOURCE_OUTDEGREE_BASE_OFF + from as usize * CAPACITY_BYTES;
        let indegree = rd_u32(arena, indegree_at);
        let outdegree = rd_u32(arena, outdegree_at);
        if indegree == u32::MAX || outdegree == u32::MAX {
            publish(arena, r, 0, 0, ERR_UNSCHEDULABLE, 0, 0);
            return ERR_UNSCHEDULABLE;
        }
        wr_u32(arena, indegree_at, indegree + 1);
        wr_u32(arena, outdegree_at, outdegree + 1);
    }

    // The ranking order, computed once: smaller slack first, then more
    // dependents, then the lower index. Packed into one u64 so the existing
    // deterministic heap sort orders by all three keys at the same time, and
    // ties are impossible because the index is part of the key.
    let slack_valid = timing_describes_this_batch(arena, r);
    let mut order = [0u64; MAX_CALLS];
    for (call, slot) in order.iter_mut().enumerate().take(call_count) {
        let slack = if slack_valid {
            rd_u32(arena, r.timing + TIMING_HEADER_BYTES + call * 8 + 4)
        } else {
            0
        };
        let dependents = rd_u32(arena, RESOURCE_OUTDEGREE_BASE_OFF + call * CAPACITY_BYTES);
        // Invert the second key so a single ascending sort yields descending
        // dependents; `MAX_EVENTS` bounds it well above any real count.
        let inverted = (MAX_EVENTS as u32).saturating_sub(dependents);
        *slot = (u64::from(slack) << 32) | (u64::from(inverted) << 8) | u64::from(call as u32);
    }
    heap_sort(&mut order[..call_count]);

    zero(arena, RESOURCE_USAGE_BASE_OFF, MAX_CAPACITY_RESOURCES);
    let mut peaks = [0u32; MAX_CAPACITY_RESOURCES];

    let mut placed = 0usize;
    let mut tick = 0u32;
    let mut deferrals = 0u32;
    let mut rounds = 0usize;

    while placed < call_count {
        rounds += 1;
        if rounds > MAX_ROUNDS {
            // The loop above guarantees progress; running out of rounds means the
            // invariant broke, which is a defect rather than a schedule.
            publish(arena, r, placed, tick, ERR_UNSCHEDULABLE, deferrals, rounds);
            return ERR_UNSCHEDULABLE;
        }

        let mut placed_this_round = 0usize;
        for entry in order.iter().take(call_count) {
            let call = (entry & 0xff) as usize;
            if finish_at(arena, r.schedule, call) != 0 {
                continue; // already placed, or placed and finished
            }
            if rd_u32(arena, RESOURCE_INDEGREE_BASE_OFF + call * CAPACITY_BYTES) != 0 {
                continue; // a predecessor is still running, or it has been placed
            }
            let ready = rd_u32(arena, RESOURCE_READY_BASE_OFF + call * CAPACITY_BYTES);
            if ready > tick {
                continue; // predecessors are done but the earliest tick is later
            }
            let duration = dur_at(arena, r, call);
            if duration == 0 {
                // Resource scheduling needs durations: a call with no length has
                // no interval to hold capacity for. Same refusal as Phase 3.
                publish(arena, r, placed, tick, ERR_DURATION_ZERO, deferrals, rounds);
                return ERR_DURATION_ZERO;
            }
            if duration == RELEASED {
                publish(
                    arena,
                    r,
                    placed,
                    tick,
                    ERR_RESOURCE_RANGE,
                    deferrals,
                    rounds,
                );
                return ERR_RESOURCE_RANGE;
            }
            let Some(finish) = tick.checked_add(duration) else {
                publish(
                    arena,
                    r,
                    placed,
                    tick,
                    ERR_RESOURCE_RANGE,
                    deferrals,
                    rounds,
                );
                return ERR_RESOURCE_RANGE;
            };
            match reserve(arena, r, call, &mut peaks) {
                Reserve::Failed(resource) => {
                    deferrals += 1;
                    push_contention(arena, r, call, resource, tick);
                    continue;
                }
                Reserve::NotADemand | Reserve::Placed => {}
                Reserve::Unrepresentable => {
                    // The arena cannot hold the write, or a declared index slipped
                    // past its own region: a broken build, not an input problem.
                    publish(
                        arena,
                        r,
                        placed,
                        tick,
                        ERR_OUTPUT_INVALID,
                        deferrals,
                        rounds,
                    );
                    return ERR_OUTPUT_INVALID;
                }
            }
            let entry = r.schedule + S_ENTRIES + call * 8;
            if !wr_u32(arena, entry, tick) || !wr_u32(arena, entry + 4, finish) {
                publish(
                    arena,
                    r,
                    placed,
                    tick,
                    ERR_OUTPUT_INVALID,
                    deferrals,
                    rounds,
                );
                return ERR_OUTPUT_INVALID;
            }
            placed_this_round += 1;
        }
        placed += placed_this_round;

        if advance(arena, r, &mut tick, &mut rounds) {
            continue; // a completion released capacity
        }
        // Nothing was released and the clock could not move. If this round also
        // placed nothing, no call can ever be placed: the graph waits on something
        // that will not finish. That is reported, not looped on.
        if placed_this_round == 0 {
            let blocked = first_blocked(arena, r);
            if blocked < call_count {
                push_unschedulable(arena, r, blocked);
            }
            publish(arena, r, placed, tick, ERR_UNSCHEDULABLE, deferrals, rounds);
            return ERR_UNSCHEDULABLE;
        }
    }

    finish_up(arena, r, &peaks, tick, placed, deferrals, rounds)
}

/// Outcome of an atomic reservation attempt.
enum Reserve {
    /// Every demand fits; capacity is now held.
    Placed,
    /// The first resource that does not fit, so the diagnostic can name it.
    Failed(usize),
    /// The call declares no demands at all — always placeable.
    NotADemand,
    /// The write cannot be represented, or an index slipped past its own region.
    Unrepresentable,
}

/// Check every demand of one call, then apply every one of them. The two phases
/// are what make the reservation atomic: no resource is touched unless all of
/// them fit, so there is no half-held allocation to leak or to unwind.
fn reserve(
    arena: &mut [u8],
    r: &Request,
    call: usize,
    peaks: &mut [u32; MAX_CAPACITY_RESOURCES],
) -> Reserve {
    let start = rd_u32(arena, r.demand_offsets + call * CAPACITY_BYTES) as usize;
    let end = rd_u32(arena, r.demand_offsets + (call + 1) * CAPACITY_BYTES) as usize;
    if end <= start {
        return Reserve::NotADemand;
    }
    if end > r.demand_count {
        return Reserve::Unrepresentable;
    }
    for index in start..end {
        let at = r.demand_records + index * DEMAND_RECORD_BYTES;
        let resource = rd_u32(arena, at) as usize;
        let amount = rd_u32(arena, at + 4);
        if resource >= r.resource_count || amount == 0 || amount == RELEASED {
            return Reserve::Unrepresentable;
        }
        let capacity = rd_u32(arena, r.capacities + resource * CAPACITY_BYTES);
        let used = rd_u32(arena, RESOURCE_USAGE_BASE_OFF + resource * CAPACITY_BYTES);
        if used == RELEASED {
            return Reserve::Unrepresentable;
        }
        let Some(total) = used.checked_add(amount) else {
            return Reserve::Failed(resource);
        };
        if total > capacity {
            return Reserve::Failed(resource);
        }
    }
    for index in start..end {
        let at = r.demand_records + index * DEMAND_RECORD_BYTES;
        let resource = rd_u32(arena, at) as usize;
        let amount = rd_u32(arena, at + 4);
        let used_at = RESOURCE_USAGE_BASE_OFF + resource * CAPACITY_BYTES;
        let used = rd_u32(arena, used_at);
        let Some(total) = used.checked_add(amount) else {
            return Reserve::Unrepresentable;
        };
        if !wr_u32(arena, used_at, total) {
            return Reserve::Unrepresentable;
        }
        if total > peaks[resource] {
            peaks[resource] = total;
        }
    }
    Reserve::Placed
}

/// Move the clock to the next completion and hand back everything that finished
/// there. Returns whether anything was released.
fn advance(arena: &mut [u8], r: &Request, tick: &mut u32, rounds: &mut usize) -> bool {
    let mut next = u32::MAX;
    let mut running = 0usize;
    for call in 0..r.call_count {
        let finish = finish_at(arena, r.schedule, call);
        if finish == 0 {
            continue;
        }
        if rd_u32(arena, RESOURCE_INDEGREE_BASE_OFF + call * CAPACITY_BYTES) == RELEASED {
            continue; // already handed back
        }
        running += 1;
        if finish < next {
            next = finish;
        }
    }
    if next == u32::MAX {
        // Nothing is running. Jump to the earliest tick a ready call could start,
        // so a graph with a gap does not stall the loop.
        let mut soonest = u32::MAX;
        for call in 0..r.call_count {
            if finish_at(arena, r.schedule, call) != 0 {
                continue;
            }
            let ready = rd_u32(arena, RESOURCE_READY_BASE_OFF + call * CAPACITY_BYTES);
            if ready > *tick && ready < soonest {
                soonest = ready;
            }
        }
        if soonest == u32::MAX {
            return false;
        }
        *tick = soonest;
        *rounds = rounds.saturating_add(1);
        return true;
    }
    if next > *tick {
        *tick = next;
    }
    *rounds = rounds.saturating_add(1);
    if running == 0 {
        return false;
    }

    // Two phases, because a record's `from` may appear in several records of the
    // same batch: mark-and-sweep in one pass would decrement only the first.
    let mut released_any = false;
    for index in 0..edge_records(arena, r) {
        let at = r.graph + OUT_HEADER_BYTES + index * EDGE_BYTES;
        let from = rd_u32(arena, at) as usize;
        let to = rd_u32(arena, at + 4) as usize;
        if from >= r.call_count || to >= r.call_count {
            continue;
        }
        if finish_at(arena, r.schedule, from) != *tick {
            continue;
        }
        let from_indeg_at = RESOURCE_INDEGREE_BASE_OFF + from * CAPACITY_BYTES;
        if rd_u32(arena, from_indeg_at) == RELEASED {
            continue;
        }
        let to_indeg_at = RESOURCE_INDEGREE_BASE_OFF + to * CAPACITY_BYTES;
        let remaining = rd_u32(arena, to_indeg_at);
        if remaining == 0 || remaining == RELEASED {
            continue;
        }
        wr_u32(arena, to_indeg_at, remaining - 1);
        let ready_at = RESOURCE_READY_BASE_OFF + to * CAPACITY_BYTES;
        let earliest = rd_u32(arena, ready_at);
        if *tick > earliest {
            wr_u32(arena, ready_at, *tick);
        }
        released_any = true;
    }
    // Hand back capacity and mark the call released. Reading the demands again is
    // cheaper than remembering per-call holdings, and the order does not matter
    // because every released call's demands are subtracted from the same lanes.
    for call in 0..r.call_count {
        if finish_at(arena, r.schedule, call) != *tick {
            continue;
        }
        let indeg_at = RESOURCE_INDEGREE_BASE_OFF + call * CAPACITY_BYTES;
        if rd_u32(arena, indeg_at) == RELEASED {
            continue;
        }
        let start = rd_u32(arena, r.demand_offsets + call * CAPACITY_BYTES) as usize;
        let end = rd_u32(arena, r.demand_offsets + (call + 1) * CAPACITY_BYTES) as usize;
        for index in start..end.min(r.demand_count) {
            let at = r.demand_records + index * DEMAND_RECORD_BYTES;
            let resource = rd_u32(arena, at) as usize;
            let amount = rd_u32(arena, at + 4);
            if resource >= r.resource_count {
                continue;
            }
            let used_at = RESOURCE_USAGE_BASE_OFF + resource * CAPACITY_BYTES;
            let used = rd_u32(arena, used_at);
            if used >= amount && used != RELEASED {
                wr_u32(arena, used_at, used - amount);
            }
        }
        wr_u32(arena, indeg_at, RELEASED);
        released_any = true;
    }
    released_any
}

/// Compute busy time and peaks, then publish both regions. The schedule header is
/// written last: until then the region reads as empty.
fn finish_up(
    arena: &mut [u8],
    r: &Request,
    peaks: &[u32; MAX_CAPACITY_RESOURCES],
    makespan: u32,
    placed: usize,
    deferrals: u32,
    rounds: usize,
) -> i32 {
    let mut makespan = makespan;
    for call in 0..r.call_count {
        let finish = finish_at(arena, r.schedule, call);
        if finish > makespan {
            makespan = finish;
        }
    }

    for (resource, peak) in peaks.iter().enumerate().take(r.resource_count) {
        let busy_at = r.stats + ST_ENTRIES + resource * 16 + ST_BUSY;
        let mut busy: u64 = 0;
        for call in 0..r.call_count {
            let start = rd_u32(arena, r.demand_offsets + call * CAPACITY_BYTES) as usize;
            let end = rd_u32(arena, r.demand_offsets + (call + 1) * CAPACITY_BYTES) as usize;
            for index in start..end.min(r.demand_count) {
                let at = r.demand_records + index * DEMAND_RECORD_BYTES;
                if (rd_u32(arena, at) as usize) != resource {
                    continue;
                }
                let amount = rd_u32(arena, at + 4);
                let duration = dur_at(arena, r, call);
                let Some(term) = u64::from(amount).checked_mul(u64::from(duration)) else {
                    publish(
                        arena,
                        r,
                        placed,
                        makespan,
                        ERR_RESOURCE_RANGE,
                        deferrals,
                        rounds,
                    );
                    return ERR_RESOURCE_RANGE;
                };
                let Some(total) = busy.checked_add(term) else {
                    publish(
                        arena,
                        r,
                        placed,
                        makespan,
                        ERR_RESOURCE_RANGE,
                        deferrals,
                        rounds,
                    );
                    return ERR_RESOURCE_RANGE;
                };
                busy = total;
            }
        }
        wr_u32(
            arena,
            r.stats + ST_ENTRIES + resource * 16 + ST_CAPACITY,
            rd_u32(arena, r.capacities + resource * CAPACITY_BYTES),
        );
        wr_u32(arena, r.stats + ST_ENTRIES + resource * 16 + ST_PEAK, *peak);
        if !wr_u64(arena, busy_at, busy) {
            publish(
                arena,
                r,
                placed,
                makespan,
                ERR_RESOURCE_RANGE,
                deferrals,
                rounds,
            );
            return ERR_RESOURCE_RANGE;
        }
    }

    wr_u32(arena, r.stats + ST_COUNT, r.resource_count as u32);
    wr_u32(arena, r.stats + ST_MAKESPAN, makespan);
    wr_u32(arena, r.stats + ST_REVISION, RESOURCE_REVISION);
    publish(arena, r, placed, makespan, OK, deferrals, rounds);
    OK
}

fn edge_records(arena: &[u8], r: &Request) -> usize {
    rd_u32(arena, r.graph + 8) as usize
}

fn first_blocked(arena: &[u8], r: &Request) -> usize {
    for call in 0..r.call_count {
        if finish_at(arena, r.schedule, call) == 0 {
            return call;
        }
    }
    r.call_count
}

fn timing_describes_this_batch(arena: &[u8], r: &Request) -> bool {
    rd_u32(arena, r.timing) == TIMING_REVISION
        && rd_u32(arena, r.timing + TIMING_EVENT_COUNT) == r.call_count as u32
}

/// Header word 4 of the timing region: the number of scheduled events, which must
/// equal the batch this call is placing.
const TIMING_EVENT_COUNT: usize = 4;

fn clear_region(arena: &mut [u8], at: usize, len: usize) {
    if let Some(slice) = arena.get_mut(at..at + len) {
        for byte in slice.iter_mut() {
            *byte = 0;
        }
    }
}

fn zero(arena: &mut [u8], at: usize, entries: usize) {
    for index in 0..entries {
        wr_u32(arena, at + index * CAPACITY_BYTES, 0);
    }
}

fn publish(
    arena: &mut [u8],
    r: &Request,
    placed: usize,
    makespan: u32,
    status: i32,
    deferrals: u32,
    rounds: usize,
) {
    // A refused run clears both regions rather than publishing a header: the same
    // rule the timing pass has held since Phase 3, so a schedule that was
    // rejected can never be read as one that was merely short. The reason survives
    // in the diagnostics region, which carries its own revision and status.
    if status != OK {
        clear_region(arena, r.schedule, SCHEDULE_REGION_LEN);
        clear_region(arena, r.stats, RESOURCE_STATS_REGION_LEN);
        return;
    }
    wr_u32(arena, r.schedule + S_PLACED, placed as u32);
    wr_u32(arena, r.schedule + S_MAKESPAN, makespan);
    wr_u32(arena, r.schedule + S_STATUS, 0);
    wr_u32(arena, r.schedule + S_DEFERRALS, deferrals);
    wr_u32(arena, r.schedule + S_ROUNDS, rounds as u32);
    wr_u32(arena, r.schedule + S_REVISION, RESOURCE_REVISION);
    wr_u32(arena, r.stats + ST_REVISION, RESOURCE_REVISION);
    wr_u32(arena, r.stats + ST_COUNT, r.resource_count as u32);
    wr_u32(arena, r.stats + ST_MAKESPAN, makespan);
}

fn push_contention(arena: &mut [u8], r: &Request, call: usize, resource: usize, tick: u32) {
    push(arena, r.diagnostics, DIAG_CONTENTION, resource, call, tick);
}

fn push_unschedulable(arena: &mut [u8], r: &Request, call: usize) {
    push(arena, r.diagnostics, DIAG_UNSCHEDULABLE, 0, call, 0);
}

/// Append one diagnostic to the list `resources::check` already published.
fn push(arena: &mut [u8], at: usize, kind: u32, resource: usize, call: usize, value: u32) {
    let count = rd_u32(arena, at + H_COUNT) as usize;
    if count >= MAX_RESOURCE_DIAGNOSTICS {
        return;
    }
    let record = at + DIAGNOSTICS_HEADER_BYTES + count * 16;
    wr_u32(arena, record + D_KIND, kind);
    wr_u32(arena, record + D_RESOURCE, resource as u32);
    wr_u32(arena, record + D_CALL, call as u32);
    wr_u32(arena, record + D_VALUE, value);
    wr_u32(arena, at + H_COUNT, count as u32 + 1);
}
