//! Phase 6 M3: the schedule quality certificate.
//!
//! Given a placement that has already been made, this computes the two lower
//! bounds Phase 6 reports, the objective component candidates will be compared
//! by, and the one claim that can be made honestly without any search: whether
//! the schedule's makespan equals the bound, in which case no schedule can be
//! shorter.
//!
//! It computes no schedule, writes no other region, and — like the utilisation
//! ratio before it — performs no division whose result reaches a caller as a
//! fraction. The workload bound is an integer ceiling, so the engine produces
//! that integer and the host divides for display. The distinction matters:
//! `Gap_LB` is a distance from a bound, not the optimality gap, and the only
//! place an optimality claim is made is the exact integer comparison below.
//!
//! No allocation, no recursion, checked arithmetic throughout, every region read
//! through bounds-checked accessors, and the revision commits last: a caller can
//! never read a half-written certificate.

use crate::resources::RESOURCE_REVISION;
use crate::schedule::wr_u64;
use crate::schedule::{T_EVENT_EARLIEST, T_MAKESPAN};
use crate::scheduling::{
    ST_BUSY, ST_CAPACITY, ST_COUNT, ST_ENTRIES, S_ENTRIES, S_MAKESPAN, S_PLACED,
};
use crate::{
    rd_u32, wr_u32, CAPACITIES_REGION_LEN, ERR_QUALITY_RANGE, ERR_QUALITY_UNAVAILABLE, OK,
    RESOURCE_STATS_REGION_LEN, SCHEDULE_REGION_LEN, TIMING_HEADER_BYTES, TIMING_REGION_LEN,
    TIMING_REVISION,
};

/// Revision of the certificate record. Bumped whenever a field changes meaning.
pub const QUALITY_REVISION: u32 = 1;

/// The certificate: nine words of which the last two are reserved, padded so the
/// region stays eight-byte aligned and three scalars can be added before it
/// grows.
pub const QUALITY_BYTES: usize = 64;

pub const Q_REVISION: usize = 0;
/// Calls the certificate covers; matches the placement it describes.
pub const Q_CALL_COUNT: usize = 4;
/// The schedule's makespan, recomputed from the windows, not copied from a header.
pub const Q_MAKESPAN: usize = 8;
/// Dependency lower bound: the critical-path duration at unlimited capacity.
pub const Q_LB_DEP: usize = 12;
/// Workload lower bound: `max_r ceil(busy_r / Q_r)`.
pub const Q_LB_RES: usize = 16;
/// The combined bound, `max(LB_dep, LB_res)`.
pub const Q_LB: usize = 20;
/// `1` when `T == LB`, so no valid schedule can be shorter. Never `1` otherwise.
pub const Q_PROVABLY_OPTIMAL: usize = 24;
/// Capacity resources the placement declared.
pub const Q_RESOURCE_COUNT: usize = 28;
/// Aggregate resource waiting, `Σ_i (S_i − ES_i)`, in ticks.
pub const Q_WAITING: usize = 32;
/// Always zero, until the next objective component needs it.
pub const Q_RESERVED: usize = 40;

/// Width of one schedule and one statistics record, restated here as the stride
/// the walk needs rather than re-derived from field offsets.
const RECORD_BYTES: usize = 8;
const STAT_BYTES: usize = 16;

/// Where every region lives and how big the batch is. Bundled because seven loose
/// `usize` arguments in a fixed order is a signature that invites exactly the
/// mistake it looks like: swapping two of them compiles.
pub struct Request {
    pub call_count: usize,
    pub resource_count: usize,
    pub schedule: usize,
    pub timing: usize,
    pub stats: usize,
    pub capacities: usize,
    pub quality: usize,
}

/// Read a little-endian `u64` from a bounds-checked slice.
fn rd_u64(buf: &[u8], at: usize) -> Option<u64> {
    let end = at.checked_add(8)?;
    let bytes: [u8; 8] = buf.get(at..end)?.try_into().ok()?;
    Some(u64::from_le_bytes(bytes))
}

/// `ceil(numerator / denominator)`, with the addition that avoids a floor checked
/// rather than assumed safe.
fn ceil_div(numerator: u64, denominator: u64) -> Option<u64> {
    Some(numerator.checked_add(denominator)?.checked_sub(1)? / denominator)
}

/// Compute the certificate for a completed placement and publish it.
///
/// Reads the schedule, timing, statistics and capacity regions; writes only the
/// quality region. Refuses rather than publishes a bound that cannot hold: a
/// region that does not describe this batch, a schedule that contradicts its own
/// timing, a bound above the makespan, or an overflow all leave the region
/// cleared and return a stable status.
pub fn assess(arena: &mut [u8], r: &Request) -> i32 {
    // Bounds first: every accessor below is called after this, so the check is
    // what makes the reads total rather than the arithmetic being careful.
    let highest = [
        r.schedule.checked_add(SCHEDULE_REGION_LEN),
        r.timing.checked_add(TIMING_REGION_LEN),
        r.stats.checked_add(RESOURCE_STATS_REGION_LEN),
        r.capacities.checked_add(CAPACITIES_REGION_LEN),
        r.quality.checked_add(QUALITY_BYTES),
    ];
    for end in highest.iter().flatten() {
        if *end > arena.len() {
            return ERR_QUALITY_RANGE;
        }
    }

    clear(arena, r.quality);

    // The timing region is the reference the certificate measures against, and
    // the schedule and statistics regions are what it certifies. Each must carry
    // its revision and describe this exact batch, or there is nothing to certify.
    if rd_u32(arena, r.timing) != TIMING_REVISION
        || rd_u32(arena, r.timing + 4) != r.call_count as u32
    {
        // 150, not 118: the durations the caller supplied are not in question
        // here, the engine's own timing region is not describing this batch, and
        // a status that means "your input was bad" would blame the wrong party.
        return ERR_QUALITY_UNAVAILABLE;
    }
    // Compared against RESOURCE_REVISION, the value the placement publishes at
    // those offsets: S_REVISION and ST_REVISION are field positions, and
    // confusing the two is what a shared name invites.
    if rd_u32(arena, r.schedule + crate::scheduling::S_REVISION) != RESOURCE_REVISION
        || rd_u32(arena, r.schedule + S_PLACED) as usize != r.call_count
    {
        return ERR_QUALITY_UNAVAILABLE;
    }
    if rd_u32(arena, r.stats + crate::scheduling::ST_REVISION) != RESOURCE_REVISION
        || rd_u32(arena, r.stats + ST_COUNT) as usize != r.resource_count
    {
        return ERR_QUALITY_UNAVAILABLE;
    }

    // The makespan and the waiting total are recomputed from the windows rather
    // than copied from the headers: the certificate is how a caller finds out
    // whether those headers are telling the truth, so it cannot borrow their
    // own answer.
    let mut makespan: u32 = 0;
    let mut waiting: u64 = 0;
    for call in 0..r.call_count {
        let window = r.schedule + S_ENTRIES + call * RECORD_BYTES;
        let start = rd_u32(arena, window);
        let finish = rd_u32(arena, window + 4);
        if finish < start {
            // A negative window is not a schedule; the placement that emitted one
            // has bigger problems than its makespan.
            return ERR_QUALITY_UNAVAILABLE;
        }
        let event = r.timing + TIMING_HEADER_BYTES + call * RECORD_BYTES;
        let earliest = rd_u32(arena, event + T_EVENT_EARLIEST);
        if start < earliest {
            // A call starting before its dependencies allow is not a schedule,
            // and a bound computed against it would be a number attached to a
            // lie.
            return ERR_QUALITY_UNAVAILABLE;
        }
        let Some(total) = waiting.checked_add(u64::from(start - earliest)) else {
            return ERR_QUALITY_RANGE;
        };
        waiting = total;
        if finish > makespan {
            makespan = finish;
        }
    }
    if makespan != rd_u32(arena, r.schedule + S_MAKESPAN) {
        return ERR_QUALITY_UNAVAILABLE;
    }

    let lb_dep = rd_u32(arena, r.timing + T_MAKESPAN);

    // Workload bound, per resource, from the busy time the placement published
    // and the capacity the caller declared. The two must agree about capacity —
    // if they do not, one of them is stale and no bound derived from it is safe.
    let mut lb_res: u64 = 0;
    for resource in 0..r.resource_count {
        let declared = rd_u32(arena, r.capacities + resource * 4);
        let row = r.stats + ST_ENTRIES + resource * STAT_BYTES;
        if declared == 0 || rd_u32(arena, row + ST_CAPACITY) != declared {
            return ERR_QUALITY_UNAVAILABLE;
        }
        let Some(busy) = rd_u64(arena, row + ST_BUSY) else {
            return ERR_QUALITY_RANGE;
        };
        let Some(needed) = ceil_div(busy, u64::from(declared)) else {
            return ERR_QUALITY_RANGE;
        };
        if needed > lb_res {
            lb_res = needed;
        }
    }
    let Ok(lb_res) = u32::try_from(lb_res) else {
        return ERR_QUALITY_RANGE;
    };

    let lb = lb_dep.max(lb_res);
    // A bound above the schedule is not a surprise worth reporting: it means the
    // certificate's inputs disagree with each other, and publishing it would
    // advertise a schedule that beats a bound no schedule can beat.
    if lb > makespan {
        return ERR_QUALITY_UNAVAILABLE;
    }

    let ok = wr_u32(arena, r.quality + Q_CALL_COUNT, r.call_count as u32)
        && wr_u32(arena, r.quality + Q_MAKESPAN, makespan)
        && wr_u32(arena, r.quality + Q_LB_DEP, lb_dep)
        && wr_u32(arena, r.quality + Q_LB_RES, lb_res)
        && wr_u32(arena, r.quality + Q_LB, lb)
        && wr_u32(arena, r.quality + Q_PROVABLY_OPTIMAL, u32::from(lb == makespan))
        && wr_u32(arena, r.quality + Q_RESOURCE_COUNT, r.resource_count as u32)
        && wr_u64(arena, r.quality + Q_WAITING, waiting)
        && wr_u32(arena, r.quality + Q_RESERVED, 0)
        // The revision commits the record last, exactly as the timing header
        // does: until it is written, there is no certificate here.
        && wr_u32(arena, r.quality + Q_REVISION, QUALITY_REVISION);
    if ok {
        OK
    } else {
        ERR_QUALITY_RANGE
    }
}

/// Zero the certificate region and leave no revision behind, so a reader cannot
/// mistake an abandoned computation for a result.
pub fn clear(arena: &mut [u8], at: usize) -> bool {
    let Some(end) = at.checked_add(QUALITY_BYTES) else {
        return false;
    };
    let Some(buf) = arena.get_mut(at..end) else {
        return false;
    };
    for byte in buf.iter_mut() {
        *byte = 0;
    }
    true
}
