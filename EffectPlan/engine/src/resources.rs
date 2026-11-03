//! Phase 5: capacity resources — validation and feasibility (milestone M3).
//!
//! A capacity resource is not a dependency resource. Dependency resources decide
//! *who must wait for whom* and live in the workspace frontier as `last_writer`
//! and `reader_head`; capacity resources decide *how many may run at once* and
//! live here, in their own index space, with no mapping between the two. A batch
//! can hold a dependency write on one record while sharing a connection slot with
//! nine other calls, and neither fact implies the other.
//!
//! This module answers one question and nothing else: **can the declared demands
//! be met by the declared capacities at all?** It does not schedule. An operation
//! asking for 3 of a resource whose capacity is 2 is not a scheduling delay — it
//! is a declaration that cannot be satisfied, and saying so before any placement
//! is attempted is the difference between an honest plan and a mysterious wait.
//!
//! Every value is a positive integer. Units are caller metadata and are never
//! interpreted: `24576 MiB` and `24576 connections` are the same arithmetic. The
//! engine never sees a resource key — the host interns them into `0..R` — so a
//! diagnostic can only name an index, which is also the leak-safety rule from
//! Phase 1.
//!
//! No heap, no `Vec`, no recursion: the working sets are fixed-size arrays sized
//! from [`MAX_CAPACITY_RESOURCES`], which is 64 — so the largest stack footprint
//! this module takes is a few hundred bytes, not a scan over unbounded input.

use crate::{
    rd_u32, wr_u32, ERR_RESOURCE_CAPACITY, ERR_RESOURCE_RANGE, ERR_RESOURCE_REGION,
    ERR_RESOURCE_UNDECLARED, MAX_CALLS, SENTINEL,
};

/// Revision of the resource contract. Published through `effectplan_info`.
pub const RESOURCE_REVISION: u32 = 1;

/// Capacity resources a batch may declare.
pub const MAX_CAPACITY_RESOURCES: usize = 64;
/// Requirement records a batch may declare, across all calls.
pub const MAX_DEMAND_RECORDS: usize = 1024;
/// Diagnostics the engine reports before it stops counting them.
pub const MAX_RESOURCE_DIAGNOSTICS: usize = 64;
/// The largest capacity or demand that keeps `demand × duration` inside a `u64`.
pub const MAX_CAPACITY: u32 = 0x7fff_ffff;

pub const CAPACITY_BYTES: usize = 4;
pub const DEMAND_OFFSET_BYTES: usize = 4;
pub const DEMAND_RECORD_BYTES: usize = 8;
pub const RESOURCE_DIAGNOSTIC_BYTES: usize = 16;

/// One `u32` capacity per declared resource, written by the host.
pub const CAPACITIES_REGION_LEN: usize = MAX_CAPACITY_RESOURCES * CAPACITY_BYTES;
/// Demand list bounds, one `u32` per call plus the terminal bound.
pub const DEMAND_OFFSETS_REGION_LEN: usize = (MAX_CALLS + 1) * DEMAND_OFFSET_BYTES;
/// `(resource index, amount)` pairs, flattened so no per-call vector exists.
pub const DEMAND_RECORDS_REGION_LEN: usize = MAX_DEMAND_RECORDS * DEMAND_RECORD_BYTES;
/// Live usage during placement. Engine-private.
pub const USAGE_REGION_LEN: usize = MAX_CAPACITY_RESOURCES * CAPACITY_BYTES;
/// Predecessors each call still waits on. Engine-private.
pub const INDEGREE_REGION_LEN: usize = MAX_CALLS * CAPACITY_BYTES;
/// Earliest tick each call may start, from the predecessors already finished.
/// Engine-private.
pub const READY_REGION_LEN: usize = MAX_CALLS * CAPACITY_BYTES;
/// Downstream dependents per call — the third ranking rule. Engine-private.
pub const OUTDEGREE_REGION_LEN: usize = MAX_CALLS * CAPACITY_BYTES;

/// Schedule header: revision, count, makespan, then the scheduling status.
pub const SCHEDULE_HEADER_BYTES: usize = 32;
pub const SCHEDULE_ENTRY_BYTES: usize = 8;
pub const SCHEDULE_MAX_ENTRIES: usize = MAX_CALLS;
pub const SCHEDULE_REGION_LEN: usize =
    SCHEDULE_HEADER_BYTES + SCHEDULE_MAX_ENTRIES * SCHEDULE_ENTRY_BYTES;

/// Per-resource statistics: capacity, peak, and busy time as a `u64` lane.
pub const STATS_HEADER_BYTES: usize = 32;
pub const STATS_ENTRY_BYTES: usize = 16;
pub const STATS_MAX_ENTRIES: usize = MAX_CAPACITY_RESOURCES;
pub const STATS_REGION_LEN: usize = STATS_HEADER_BYTES + STATS_MAX_ENTRIES * STATS_ENTRY_BYTES;

/// Diagnostics header: revision, count, status. Records follow.
pub const DIAGNOSTICS_HEADER_BYTES: usize = 32;
pub const DIAGNOSTICS_MAX_ENTRIES: usize = MAX_RESOURCE_DIAGNOSTICS;
pub const DIAGNOSTICS_REGION_LEN: usize =
    DIAGNOSTICS_HEADER_BYTES + DIAGNOSTICS_MAX_ENTRIES * RESOURCE_DIAGNOSTIC_BYTES;

// Diagnostic kinds. Numbers are part of the contract: they are what a caller
// switches on, so they never change meaning.
pub const DIAG_INSUFFICIENT_CAPACITY: u32 = 1;
pub const DIAG_RESOURCE_UNDECLARED: u32 = 2;
pub const DIAG_ZERO_CAPACITY: u32 = 3;
pub const DIAG_ZERO_DEMAND: u32 = 4;
pub const DIAG_DUPLICATE_DEMAND: u32 = 5;
pub const DIAG_RANGE: u32 = 6;
/// A ready call was deferred because something else held the capacity.
pub const DIAG_CONTENTION: u32 = 7;
/// A call can never be placed: the graph waits on something that will not finish.
pub const DIAG_UNSCHEDULABLE: u32 = 8;

// Diagnostic record fields, relative to the record start. Public because the host
// decoder reads exactly these lanes; they are part of the contract, not layout
// detail the caller is asked to guess.
pub const D_KIND: usize = 0;
pub const D_RESOURCE: usize = 4;
pub const D_CALL: usize = 8;
pub const D_VALUE: usize = 12;

// Region header fields.
pub const H_REVISION: usize = 0;
pub const H_COUNT: usize = 4;
pub const H_STATUS: usize = 8;

/// Demand offsets are read from the arena and walked in order, so the only
/// mutable state is the diagnostic list and this report.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Report {
    /// The status the engine returns: 0 when the declarations are satisfiable.
    pub status: i32,
    pub diagnostics: usize,
}

/// Write an empty, valid diagnostics header so a refused or successful run can
/// never leave the host reading a *previous* run's records. The whole region is
/// cleared rather than only the header for that reason.
pub fn clear(arena: &mut [u8], at: usize) -> bool {
    let end = match at.checked_add(DIAGNOSTICS_REGION_LEN) {
        Some(end) => end,
        None => return false,
    };
    let buf = match arena.get_mut(at..end) {
        Some(buf) => buf,
        None => return false,
    };
    for byte in buf.iter_mut() {
        *byte = 0;
    }
    wr_u32(arena, at + H_REVISION, RESOURCE_REVISION)
}

fn push_diagnostic(
    arena: &mut [u8],
    at: usize,
    count: &mut usize,
    kind: u32,
    resource: usize,
    call: usize,
    value: u32,
) {
    // Past the ceiling the list stops growing; the status still says what was
    // wrong, and the count stays at the ceiling so the host cannot be told there
    // are no more problems when the scan simply stopped recording them.
    if *count >= DIAGNOSTICS_MAX_ENTRIES {
        return;
    }
    let record = match at
        .checked_add(DIAGNOSTICS_HEADER_BYTES)
        .and_then(|base| base.checked_add(*count * RESOURCE_DIAGNOSTIC_BYTES))
    {
        Some(record) => record,
        None => return,
    };
    wr_u32(arena, record + D_KIND, kind);
    wr_u32(arena, record + D_RESOURCE, resource as u32);
    wr_u32(arena, record + D_CALL, call as u32);
    wr_u32(arena, record + D_VALUE, value);
    *count = count.saturating_add(1);
}

/// The precedence between problem classes: a malformed declaration cannot be
/// scheduled at all, an undeclared resource cannot be accounted for, an
/// impossible demand is infeasible but well-formed, and a range violation is a
/// caller that has exceeded the arithmetic the model supports.
fn worst(a: i32, b: i32) -> i32 {
    let rank = |code: i32| match code {
        ERR_RESOURCE_REGION => 4,
        ERR_RESOURCE_UNDECLARED => 3,
        ERR_RESOURCE_CAPACITY => 2,
        ERR_RESOURCE_RANGE => 1,
        0 => 0,
        _ => 0,
    };
    if rank(b) > rank(a) {
        b
    } else {
        a
    }
}

/// Where the declarations live in the arena and how many there are. Bundled
/// because seven loose `usize` arguments in a fixed order is a signature that
/// invites exactly the mistake it looks like: swapping two of them compiles.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Request {
    pub capacities: usize,
    pub offsets: usize,
    pub records: usize,
    pub diagnostics: usize,
    pub call_count: usize,
    pub resource_count: usize,
    pub demand_count: usize,
}

/// Validate a declared resource table and publish the result.
///
/// The diagnostics header is written on every path — success, structural refusal
/// or semantic failure — because a host that reads the region after a refused run
/// must never be handed the *previous* run's list.
pub fn check(arena: &mut [u8], request: &Request) -> Report {
    let report = scan(arena, request);
    wr_u32(
        arena,
        request.diagnostics + H_COUNT,
        report.diagnostics as u32,
    );
    wr_u32(arena, request.diagnostics + H_STATUS, report.status as u32);
    report
}

/// Validate a declared resource table against a declared set of demands.
///
/// Every count is checked before a byte is read and every address is derived with
/// `checked_add`, so a hostile count cannot reach past its region. Structural
/// faults (a count over its ceiling, a malformed offset vector) stop the scan,
/// because nothing after them can be trusted; semantic faults are all collected so
/// a caller fixing one problem learns about the others in the same pass.
fn scan(arena: &mut [u8], request: &Request) -> Report {
    let Request {
        capacities: caps_at,
        offsets: offs_at,
        records: recs_at,
        diagnostics: diag_at,
        call_count,
        resource_count,
        demand_count,
    } = *request;
    if resource_count > MAX_CAPACITY_RESOURCES || demand_count > MAX_DEMAND_RECORDS {
        return Report {
            status: ERR_RESOURCE_REGION,
            diagnostics: 0,
        };
    }
    if call_count > MAX_CALLS {
        return Report {
            status: ERR_RESOURCE_REGION,
            diagnostics: 0,
        };
    }

    // Capacities first: they bound every demand check that follows.
    let mut capacities = [0u32; MAX_CAPACITY_RESOURCES];
    let mut status: i32 = 0;
    let mut count = 0usize;
    for (index, slot) in capacities.iter_mut().enumerate().take(resource_count) {
        let at = match caps_at.checked_add(index * CAPACITY_BYTES) {
            Some(at) => at,
            None => {
                return Report {
                    status: ERR_RESOURCE_REGION,
                    diagnostics: 0,
                }
            }
        };
        let capacity = rd_u32(arena, at);
        if capacity == SENTINEL {
            return Report {
                status: ERR_RESOURCE_REGION,
                diagnostics: 0,
            };
        }
        if capacity == 0 {
            push_diagnostic(arena, diag_at, &mut count, DIAG_ZERO_CAPACITY, index, 0, 0);
            status = worst(status, ERR_RESOURCE_REGION);
            continue;
        }
        if capacity > MAX_CAPACITY {
            push_diagnostic(arena, diag_at, &mut count, DIAG_RANGE, index, 0, capacity);
            status = worst(status, ERR_RESOURCE_RANGE);
            continue;
        }
        *slot = capacity;
    }

    // The offset vector must describe exactly `demand_count` records: monotonic,
    // starting at zero, ending at the count. Anything else would make the record
    // walk read the wrong bytes, so it is refused rather than worked around.
    let first = rd_u32(arena, offs_at);
    if first == SENTINEL || first != 0 {
        return Report {
            status: ERR_RESOURCE_REGION,
            diagnostics: count,
        };
    }
    let mut previous = 0u32;
    for call in 0..=call_count {
        let at = match offs_at.checked_add(call * DEMAND_OFFSET_BYTES) {
            Some(at) => at,
            None => {
                return Report {
                    status: ERR_RESOURCE_REGION,
                    diagnostics: count,
                }
            }
        };
        let offset = rd_u32(arena, at);
        if offset == SENTINEL || offset > demand_count as u32 || offset < previous {
            return Report {
                status: ERR_RESOURCE_REGION,
                diagnostics: count,
            };
        }
        if call == call_count && offset != demand_count as u32 {
            return Report {
                status: ERR_RESOURCE_REGION,
                diagnostics: count,
            };
        }
        previous = offset;
    }

    // Per-resource accumulators, bounded by the resource ceiling rather than by
    // the input: `seen` catches a resource named twice by one call, `busy` totals
    // demand so an implausible aggregate is reported instead of silently large.
    let mut seen = [SENTINEL; MAX_CAPACITY_RESOURCES];
    let mut busy = [0u64; MAX_CAPACITY_RESOURCES];

    for call in 0..call_count {
        let start_at = match offs_at.checked_add(call * DEMAND_OFFSET_BYTES) {
            Some(at) => at,
            None => {
                return Report {
                    status: ERR_RESOURCE_REGION,
                    diagnostics: count,
                }
            }
        };
        let end_at = match offs_at.checked_add((call + 1) * DEMAND_OFFSET_BYTES) {
            Some(at) => at,
            None => {
                return Report {
                    status: ERR_RESOURCE_REGION,
                    diagnostics: count,
                }
            }
        };
        let start = rd_u32(arena, start_at) as usize;
        let end = rd_u32(arena, end_at) as usize;
        for record in start..end {
            let at = match recs_at.checked_add(record * DEMAND_RECORD_BYTES) {
                Some(at) => at,
                None => {
                    return Report {
                        status: ERR_RESOURCE_REGION,
                        diagnostics: count,
                    }
                }
            };
            let resource = rd_u32(arena, at) as usize;
            let amount = rd_u32(arena, at + DEMAND_RECORD_BYTES / 2);
            if resource == SENTINEL as usize || amount == SENTINEL {
                return Report {
                    status: ERR_RESOURCE_REGION,
                    diagnostics: count,
                };
            }
            if resource >= resource_count {
                push_diagnostic(
                    arena,
                    diag_at,
                    &mut count,
                    DIAG_RESOURCE_UNDECLARED,
                    resource,
                    call,
                    amount,
                );
                status = worst(status, ERR_RESOURCE_UNDECLARED);
                continue;
            }
            if amount == 0 {
                push_diagnostic(
                    arena,
                    diag_at,
                    &mut count,
                    DIAG_ZERO_DEMAND,
                    resource,
                    call,
                    0,
                );
                status = worst(status, ERR_RESOURCE_REGION);
                continue;
            }
            if seen[resource] == call as u32 {
                push_diagnostic(
                    arena,
                    diag_at,
                    &mut count,
                    DIAG_DUPLICATE_DEMAND,
                    resource,
                    call,
                    amount,
                );
                status = worst(status, ERR_RESOURCE_REGION);
                continue;
            }
            seen[resource] = call as u32;
            let capacity = capacities[resource];
            if capacity == 0 {
                // The resource itself was refused above; its demands are already
                // accounted for by that diagnostic and must not be compared to 0.
                continue;
            }
            if amount > capacity {
                push_diagnostic(
                    arena,
                    diag_at,
                    &mut count,
                    DIAG_INSUFFICIENT_CAPACITY,
                    resource,
                    call,
                    amount,
                );
                status = worst(status, ERR_RESOURCE_CAPACITY);
            }
            match busy[resource].checked_add(u64::from(amount)) {
                Some(total) => busy[resource] = total,
                None => {
                    push_diagnostic(
                        arena, diag_at, &mut count, DIAG_RANGE, resource, call, amount,
                    );
                    status = worst(status, ERR_RESOURCE_RANGE);
                }
            }
        }
    }

    Report {
        status,
        diagnostics: count,
    }
}
