//! Tests for resource-constrained placement (Phase 5, M4 and M5 gates).
//!
//! The invariant checked here is the one the phase is judged on, and it is checked
//! *directly* rather than against a golden schedule: for every tick of the
//! makespan, the demands of the calls overlapping that tick must fit every
//! capacity, and every dependency edge must still be respected. A scheduler that
//! returns a plausible-looking but infeasible schedule is the one failure mode this
//! phase could produce on its own, and a golden-output test would not have noticed.
//!
//! The graphs come from the real pipeline — `plan_v4` emits the edges and the
//! timing, `effectplan_resource_schedule` places against them — and the edges are
//! read back out of the output region rather than assumed, so the checks cannot
//! drift from what the planner actually produced.

use super::*;
use crate::resources::{DEMAND_RECORD_BYTES, D_CALL, D_KIND, D_RESOURCE, D_VALUE, H_COUNT};
use crate::scheduling::{
    ST_BUSY, ST_CAPACITY, ST_COUNT, ST_ENTRIES, ST_PEAK, S_DEFERRALS, S_ENTRIES, S_MAKESPAN,
    S_PLACED, S_REVISION, S_ROUNDS, S_STATUS,
};
use crate::ARENA_LOCK as LOCK;

unsafe fn arena_bytes() -> &'static mut [u8] {
    core::slice::from_raw_parts_mut(core::ptr::addr_of_mut!(ARENA) as *mut u8, ARENA_BYTES)
}

#[inline]
fn word(at: usize) -> u32 {
    unsafe { u32::from_le_bytes(arena_bytes()[at..at + 4].try_into().unwrap()) }
}

#[inline]
fn double_word(at: usize) -> u64 {
    unsafe { u64::from_le_bytes(arena_bytes()[at..at + 8].try_into().unwrap()) }
}

/// One operation, in both models: the dependency accesses that order it, the
/// capacities it holds, and how long it holds them.
struct Op {
    accesses: Vec<(u32, u8)>,
    demands: Vec<(u32, u32)>,
    duration: u32,
}

fn op(accesses: &[(u32, u8)], demands: &[(u32, u32)], duration: u32) -> Op {
    Op {
        accesses: accesses.to_vec(),
        demands: demands.to_vec(),
        duration,
    }
}

fn write(rid: u32) -> (u32, u8) {
    (rid, ACCESS_WRITE)
}

fn read(rid: u32) -> (u32, u8) {
    (rid, ACCESS_READ)
}

/// What a placement run published.
struct Placed {
    status: i32,
    schedule_status: u32,
    placed: u32,
    makespan: u32,
    deferrals: u32,
    rounds: u32,
    windows: Vec<(u32, u32)>,
    diagnostics: Vec<[u32; 4]>,
    /// The output and timing regions as they stood after planning, compared after
    /// placing: the placement reads them and must not write them.
    graph_intact: bool,
    timing_intact: bool,
}

/// Write the dependency batch, plan it, write the capacities and demands, place.
///
/// `declared_resources` overrides the resource count handed to the placement call,
/// so a test can declare fewer resources than the demands name.
fn place_with(ops: &[Op], capacities: &[u32], declared_resources: Option<usize>) -> Placed {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    unsafe {
        let a = arena_bytes();
        for b in a.iter_mut() {
            *b = 0;
        }

        // Dependency input, exactly as the planner expects it.
        let mut modes = Vec::new();
        let mut offsets = vec![0u32];
        let mut rids = Vec::new();
        let mut amodes = Vec::new();
        for entry in ops {
            modes.push(if entry.accesses.is_empty() {
                MODE_PURE
            } else {
                MODE_RESOURCES
            });
            for (rid, mode) in &entry.accesses {
                rids.push(*rid);
                amodes.push(*mode);
            }
            offsets.push(rids.len() as u32);
        }
        let dependency_resources = rids.iter().copied().max().unwrap_or(0) as usize + 1;
        a[IN_CALL_MODES_OFF..IN_CALL_MODES_OFF + modes.len()].copy_from_slice(&modes);
        for (index, value) in offsets.iter().enumerate() {
            let at = IN_ACCESS_OFFSETS_OFF + index * 4;
            a[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        for (index, value) in rids.iter().enumerate() {
            let at = IN_RESOURCE_IDS_OFF + index * 4;
            a[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        a[IN_ACCESS_MODES_OFF..IN_ACCESS_MODES_OFF + amodes.len()].copy_from_slice(&amodes);
        for (index, entry) in ops.iter().enumerate() {
            let at = DURATIONS_BASE_OFF + index * 4;
            a[at..at + 4].copy_from_slice(&entry.duration.to_le_bytes());
        }

        let base = arena_base() as u32;
        let plan_status = effectplan_plan_v4(
            ABI_VERSION,
            ops.len() as u32,
            rids.len() as u32,
            dependency_resources as u32,
            base + IN_CALL_MODES_OFF as u32,
            base + IN_ACCESS_OFFSETS_OFF as u32,
            base + IN_RESOURCE_IDS_OFF as u32,
            base + IN_ACCESS_MODES_OFF as u32,
            base + WS_BASE_OFF as u32,
            WORKSPACE_REGION_LEN as u32,
            base + OUT_BASE_OFF as u32,
            (OUT_RECORDS_OFF + OUT_RECORDS_LEN) as u32,
            base + LEVELS_BASE_OFF as u32,
            LEVELS_REGION_LEN as u32,
            base + DURATIONS_BASE_OFF as u32,
            DURATIONS_REGION_LEN as u32,
            base + TIMING_BASE_OFF as u32,
            TIMING_REGION_LEN as u32,
            base + CHECKPOINT_BASE_OFF as u32,
            CHECKPOINT_REGION_LEN as u32,
        );
        assert_eq!(
            plan_status, OK,
            "the dependency batch must plan before placing"
        );
        // Snapshot the planner's output at the only moment the comparison means
        // anything: after planning, before placing.
        let graph_after_plan: Vec<u8> = a[OUT_BASE_OFF..OUT_BASE_OFF + 64].to_vec();
        let timing_after_plan: Vec<u8> = a[TIMING_BASE_OFF..TIMING_BASE_OFF + 64].to_vec();

        // Capacity table and demands.
        for (index, capacity) in capacities.iter().enumerate() {
            let at = CAPACITIES_BASE_OFF + index * 4;
            a[at..at + 4].copy_from_slice(&capacity.to_le_bytes());
        }
        let mut demand_offsets = vec![0u32];
        let mut records: Vec<(u32, u32)> = Vec::new();
        for entry in ops {
            for pair in &entry.demands {
                records.push(*pair);
            }
            demand_offsets.push(records.len() as u32);
        }
        for (index, value) in demand_offsets.iter().enumerate() {
            let at = DEMAND_OFFSETS_BASE_OFF + index * 4;
            a[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        for (index, (resource, amount)) in records.iter().enumerate() {
            let at = DEMAND_RECORDS_BASE_OFF + index * DEMAND_RECORD_BYTES;
            a[at..at + 4].copy_from_slice(&resource.to_le_bytes());
            a[at + 4..at + 8].copy_from_slice(&amount.to_le_bytes());
        }

        let resource_count = declared_resources.unwrap_or(capacities.len());
        let status = effectplan_resource_schedule(
            ABI_VERSION,
            ops.len() as u32,
            resource_count as u32,
            records.len() as u32,
            base + CAPACITIES_BASE_OFF as u32,
            CAPACITIES_REGION_LEN as u32,
            base + DEMAND_OFFSETS_BASE_OFF as u32,
            DEMAND_OFFSETS_REGION_LEN as u32,
            base + DEMAND_RECORDS_BASE_OFF as u32,
            DEMAND_RECORDS_REGION_LEN as u32,
            base + DURATIONS_BASE_OFF as u32,
            DURATIONS_REGION_LEN as u32,
            base + OUT_BASE_OFF as u32,
            OUTPUT_REGION_LEN as u32,
            base + TIMING_BASE_OFF as u32,
            TIMING_REGION_LEN as u32,
            base + SCHEDULE_BASE_OFF as u32,
            SCHEDULE_REGION_LEN as u32,
            base + RESOURCE_STATS_BASE_OFF as u32,
            RESOURCE_STATS_REGION_LEN as u32,
            base + RESOURCE_DIAGNOSTICS_BASE_OFF as u32,
            RESOURCE_DIAGNOSTICS_REGION_LEN as u32,
        );

        let placed = word(SCHEDULE_BASE_OFF + S_PLACED);
        let mut windows = Vec::new();
        for index in 0..ops.len() {
            let at = SCHEDULE_BASE_OFF + S_ENTRIES + index * 8;
            windows.push((word(at), word(at + 4)));
        }
        let diagnostics = read_diagnostics();
        Placed {
            status,
            schedule_status: word(SCHEDULE_BASE_OFF + S_STATUS),
            placed,
            makespan: word(SCHEDULE_BASE_OFF + S_MAKESPAN),
            deferrals: word(SCHEDULE_BASE_OFF + S_DEFERRALS),
            rounds: word(SCHEDULE_BASE_OFF + S_ROUNDS),
            windows,
            diagnostics,
            graph_intact: a[OUT_BASE_OFF..OUT_BASE_OFF + 64] == graph_after_plan[..],
            timing_intact: a[TIMING_BASE_OFF..TIMING_BASE_OFF + 64] == timing_after_plan[..],
        }
    }
}

fn place(ops: &[Op], capacities: &[u32]) -> Placed {
    place_with(ops, capacities, None)
}

/// Invoke the placement alone, against whatever the arena currently holds. Used by
/// the tests that need input the planner would never have accepted: the scheduler
/// validates its own inputs rather than trusting what came before it.
unsafe fn schedule_only(call_count: usize, resource_count: usize, demand_count: usize) -> i32 {
    let base = arena_base() as u32;
    effectplan_resource_schedule(
        ABI_VERSION,
        call_count as u32,
        resource_count as u32,
        demand_count as u32,
        base + CAPACITIES_BASE_OFF as u32,
        CAPACITIES_REGION_LEN as u32,
        base + DEMAND_OFFSETS_BASE_OFF as u32,
        DEMAND_OFFSETS_REGION_LEN as u32,
        base + DEMAND_RECORDS_BASE_OFF as u32,
        DEMAND_RECORDS_REGION_LEN as u32,
        base + DURATIONS_BASE_OFF as u32,
        DURATIONS_REGION_LEN as u32,
        base + OUT_BASE_OFF as u32,
        OUTPUT_REGION_LEN as u32,
        base + TIMING_BASE_OFF as u32,
        TIMING_REGION_LEN as u32,
        base + SCHEDULE_BASE_OFF as u32,
        SCHEDULE_REGION_LEN as u32,
        base + RESOURCE_STATS_BASE_OFF as u32,
        RESOURCE_STATS_REGION_LEN as u32,
        base + RESOURCE_DIAGNOSTICS_BASE_OFF as u32,
        RESOURCE_DIAGNOSTICS_REGION_LEN as u32,
    )
}

unsafe fn read_diagnostics() -> Vec<[u32; 4]> {
    let count = word(RESOURCE_DIAGNOSTICS_BASE_OFF + H_COUNT) as usize;
    (0..count)
        .map(|index| {
            let at = RESOURCE_DIAGNOSTICS_BASE_OFF + 32 + index * 16;
            [
                word(at + D_KIND),
                word(at + D_RESOURCE),
                word(at + D_CALL),
                word(at + D_VALUE),
            ]
        })
        .collect()
}

/// The edges the planner actually emitted, read back rather than assumed.
fn edges(call_count: usize) -> Vec<(usize, usize)> {
    let count = word(OUT_BASE_OFF + 8) as usize;
    {
        (0..count)
            .map(|index| {
                let at = OUT_BASE_OFF + OUT_RECORDS_OFF + index * EDGE_BYTES;
                (word(at) as usize, word(at + 4) as usize)
            })
            .filter(|(from, to)| *from < call_count && *to < call_count)
            .collect()
    }
}

/// The direct feasibility check: durations hold, edges hold, and no capacity is
/// ever exceeded — evaluated tick by tick over the whole makespan, from the
/// schedule and the demands, without consulting any state the scheduler kept.
fn assert_feasible(ops: &[Op], capacities: &[u32], placed: &Placed) {
    let call_count = ops.len();
    assert_eq!(placed.status, OK);
    assert_eq!(placed.placed as usize, call_count, "every call is placed");

    for (index, entry) in ops.iter().enumerate() {
        let (start, finish) = placed.windows[index];
        assert_eq!(
            finish,
            start.saturating_add(entry.duration),
            "call {index} must span exactly its duration"
        );
        assert!(
            finish <= placed.makespan,
            "call {index} runs past the makespan"
        );
    }

    for (from, to) in edges(call_count) {
        assert!(
            placed.windows[to].0 >= placed.windows[from].1,
            "edge {from} -> {to}: start {} must not precede finish {}",
            placed.windows[to].0,
            placed.windows[from].1
        );
    }

    // The capacity condition, at every tick. Half-open: a call finishing at t
    // has already released, and one starting at t holds it.
    for tick in 0..placed.makespan {
        let mut live = vec![0u32; capacities.len()];
        for (index, entry) in ops.iter().enumerate() {
            let (start, finish) = placed.windows[index];
            if start <= tick && tick < finish {
                for (resource, amount) in &entry.demands {
                    live[*resource as usize] += *amount;
                }
            }
        }
        for (resource, used) in live.iter().enumerate() {
            assert!(
                *used <= capacities[resource],
                "tick {tick}: resource {resource} holds {used} of {}",
                capacities[resource]
            );
        }
    }
}

#[test]
fn independent_calls_sharing_one_exclusive_capacity_serialise() {
    let ops = [
        op(&[], &[(0, 1)], 3),
        op(&[], &[(0, 1)], 3),
        op(&[], &[(0, 1)], 3),
    ];
    let placed = place(&ops, &[1]);
    assert_feasible(&ops, &[1], &placed);
    assert_eq!(placed.makespan, 9, "one lane means nine ticks of work");
}

#[test]
fn capacity_exactly_matching_the_demand_allows_full_overlap() {
    let ops = [op(&[], &[(0, 4), (1, 2)], 5), op(&[], &[(0, 4), (1, 2)], 5)];
    let placed = place(&ops, &[8, 4]);
    assert_feasible(&ops, &[8, 4], &placed);
    assert_eq!(placed.makespan, 5, "both fit together on both resources");
    assert_eq!(placed.deferrals, 0, "nothing had to wait");
}

#[test]
fn a_dependency_edge_is_respected_even_when_capacity_is_free() {
    // A writes r1, B reads r1: the planner must edge A -> B, and the placement has
    // to honour it with a lane wide enough for both.
    let ops = [op(&[write(1)], &[(0, 1)], 4), op(&[read(1)], &[(0, 1)], 4)];
    let placed = place(&ops, &[2]);
    assert_feasible(&ops, &[2], &placed);
    assert_eq!(
        placed.makespan, 8,
        "the edge, not the capacity, serialises them"
    );
    assert_eq!(placed.windows[1].0, 4);
}

#[test]
fn an_over_capacity_demand_is_refused_before_anything_is_placed() {
    let ops = [op(&[], &[(0, 3)], 2)];
    let placed = place(&ops, &[2]);
    assert_eq!(placed.status, ERR_RESOURCE_CAPACITY);
    assert_eq!(
        placed.placed, 0,
        "a refused batch leaves no partial schedule"
    );
    assert_eq!(placed.windows[0], (0, 0));
    assert_eq!(placed.diagnostics[0][0], 1, "INSUFFICIENT_CAPACITY");
}

#[test]
fn simultaneous_completions_release_everything_at_the_same_tick() {
    // Three calls finish together; a fourth needs all of that capacity. The
    // half-open rule says it may start on that very tick.
    let ops = [
        op(&[], &[(0, 2)], 4),
        op(&[], &[(0, 2)], 4),
        op(&[], &[(0, 1)], 4),
        op(&[], &[(0, 5)], 2),
    ];
    let placed = place(&ops, &[5]);
    assert_feasible(&ops, &[5], &placed);
    assert_eq!(placed.makespan, 6, "release and acquisition share tick 4");
    assert_eq!(placed.windows[3].0, 4);
}

#[test]
fn a_long_call_blocks_short_ones_without_violating_anything() {
    let ops = [
        op(&[], &[(0, 3)], 10),
        op(&[], &[(0, 2)], 1),
        op(&[], &[(0, 2)], 1),
    ];
    let placed = place(&ops, &[4]);
    assert_feasible(&ops, &[4], &placed);
    // The long call takes 3 of 4; a short one joins it, the other must wait.
    assert!(placed.deferrals >= 1, "at least one call had to wait");
}

#[test]
fn a_call_demanding_every_resource_is_placed_when_all_fit() {
    let ops = [
        op(&[], &[(0, 1), (1, 1), (2, 1)], 3),
        op(&[], &[(0, 1), (1, 1), (2, 1)], 3),
    ];
    let placed = place(&ops, &[1, 1, 1]);
    assert_feasible(&ops, &[1, 1, 1], &placed);
    assert_eq!(placed.makespan, 6, "one lane across three resources");
}

#[test]
fn a_resource_nobody_uses_reports_zero_busy_and_peak() {
    let ops = [op(&[], &[(0, 1)], 4)];
    let placed = place(&ops, &[4, 7]);
    assert_feasible(&ops, &[4, 7], &placed);
    {
        assert_eq!(word(RESOURCE_STATS_BASE_OFF + ST_COUNT), 2);
        assert_eq!(word(RESOURCE_STATS_BASE_OFF + ST_ENTRIES + 16 + ST_PEAK), 0);
        assert_eq!(
            double_word(RESOURCE_STATS_BASE_OFF + ST_ENTRIES + 16 + ST_BUSY),
            0
        );
        assert_eq!(
            word(RESOURCE_STATS_BASE_OFF + ST_ENTRIES + 16 + ST_CAPACITY),
            7
        );
        assert_eq!(word(RESOURCE_STATS_BASE_OFF + ST_ENTRIES + ST_PEAK), 1);
        assert_eq!(
            double_word(RESOURCE_STATS_BASE_OFF + ST_ENTRIES + ST_BUSY),
            4
        );
    }
}

#[test]
fn reservation_is_atomic_a_call_that_cannot_take_all_holds_none() {
    // Two calls compete: the first holds all of resource 0, the second needs one
    // of 0 and all of 1. If the second reserved resource 1 before failing on 0,
    // resource 1 would be leaked and the check would see it later.
    let ops = [op(&[], &[(0, 2)], 3), op(&[], &[(0, 1), (1, 2)], 3)];
    let placed = place(&ops, &[2, 2]);
    assert_feasible(&ops, &[2, 2], &placed);
    assert_eq!(
        placed.makespan, 6,
        "the second call waits for the first entirely"
    );
    assert_eq!(placed.windows[1], (3, 6));
}

#[test]
fn unlimited_capacity_reproduces_the_dependency_only_earliest_starts() {
    // With demands all zero the placement has nothing to wait for, so it must
    // match the CPM earliest starts: the schedule and the timing region agree.
    let ops = [
        op(&[write(1)], &[], 2),
        op(&[read(1)], &[], 5),
        op(&[], &[], 3),
    ];
    let placed = place(&ops, &[1]);
    assert_feasible(&ops, &[1], &placed);
    let earliest = {
        (0..ops.len())
            .map(|index| word(TIMING_BASE_OFF + TIMING_HEADER_BYTES + index * 8))
            .collect::<Vec<u32>>()
    };
    for (index, start) in earliest.iter().enumerate() {
        assert_eq!(
            placed.windows[index].0, *start,
            "call {index}: greedy placement with free capacity must equal its earliest start"
        );
    }
}

#[test]
fn the_same_input_produces_the_same_bytes() {
    let ops = [
        op(&[write(1)], &[(0, 2)], 4),
        op(&[read(1)], &[(0, 1), (1, 3)], 3),
        op(&[], &[(1, 1)], 6),
        op(&[write(2)], &[(0, 1)], 2),
    ];
    let capacities = [3, 4];
    let first = place(&ops, &capacities);
    let second = place(&ops, &capacities);
    assert_eq!(first.status, OK);
    assert_eq!(first.windows, second.windows, "deterministic placement");
    assert_eq!(first.makespan, second.makespan);
    assert_eq!(first.deferrals, second.deferrals);
    assert_eq!(first.rounds, second.rounds);
}

#[test]
fn a_zero_duration_is_refused_by_the_scheduler_too_even_though_the_planner_would_catch_it() {
    // A zero duration never reaches placement through the normal pipeline: the
    // planning pass refuses the batch first, with the same code. The scheduler is
    // tested with one anyway, because it must not rely on that.
    let ops = [op(&[], &[(0, 1)], 2)];
    let placed = place(&ops, &[2]);
    assert_eq!(placed.status, OK);
    unsafe {
        let at = DURATIONS_BASE_OFF;
        arena_bytes()[at..at + 4].copy_from_slice(&0u32.to_le_bytes());
        let status = schedule_only(1, 1, 1);
        assert_eq!(status, ERR_DURATION_ZERO);
        assert_eq!(word(SCHEDULE_BASE_OFF + S_PLACED), 0);
        assert_eq!(word(SCHEDULE_BASE_OFF + S_REVISION), 0);
    }
}

#[test]
fn durations_that_would_overflow_the_tick_range_are_refused_not_wrapped() {
    let ops = [op(&[], &[(0, 1)], u32::MAX), op(&[], &[(0, 1)], u32::MAX)];
    let placed = place(&ops, &[1]);
    assert_eq!(placed.status, ERR_RESOURCE_RANGE);
    assert_eq!(placed.placed, 0, "no wrapped schedule is published");
    assert_eq!(
        placed.windows,
        vec![(0, 0), (0, 0)],
        "the region is cleared"
    );
    assert_eq!(placed.makespan, 0);
    assert_eq!(placed.schedule_status, 0);
}

#[test]
fn a_demand_naming_an_undeclared_resource_is_refused_before_placement() {
    // Two resources declared, two capacities supplied, and a demand that names a
    // third: undeclared rather than malformed, and caught before any placement.
    let ops = [op(&[], &[(4, 1)], 2)];
    let placed = place_with(&ops, &[2, 2], Some(2));
    assert_eq!(placed.status, ERR_RESOURCE_UNDECLARED);
    assert_eq!(placed.placed, 0);
}

#[test]
fn declaring_more_resources_than_there_are_capacities_is_malformed() {
    // The mirror case: a capacity table with a hole in it is not a resource that
    // simply went unused, and it must not be silently treated as capacity zero.
    let ops = [op(&[], &[(1, 1)], 2)];
    let placed = place_with(&ops, &[2], Some(2));
    assert_eq!(placed.status, ERR_RESOURCE_REGION);
    assert_eq!(placed.diagnostics[0][0], 3, "ZERO_CAPACITY");
}

#[test]
fn a_forged_graph_is_refused_rather_than_placed() {
    // Write a header that claims an edge from a call that does not exist, then
    // place without planning. The scheduler validates what it reads.
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    unsafe {
        let a = arena_bytes();
        for b in a.iter_mut() {
            *b = 0;
        }
        a[OUT_BASE_OFF..OUT_BASE_OFF + 4].copy_from_slice(&ABI_VERSION.to_le_bytes());
        a[OUT_BASE_OFF + 4..OUT_BASE_OFF + 8].copy_from_slice(&2u32.to_le_bytes());
        a[OUT_BASE_OFF + 8..OUT_BASE_OFF + 12].copy_from_slice(&1u32.to_le_bytes());
        let record = OUT_BASE_OFF + OUT_RECORDS_OFF;
        a[record..record + 4].copy_from_slice(&5u32.to_le_bytes()); // from, out of range
        a[record + 4..record + 8].copy_from_slice(&1u32.to_le_bytes());
        for index in 0..2 {
            let at = DURATIONS_BASE_OFF + index * 4;
            a[at..at + 4].copy_from_slice(&2u32.to_le_bytes());
        }
        a[CAPACITIES_BASE_OFF..CAPACITIES_BASE_OFF + 4].copy_from_slice(&2u32.to_le_bytes());
        let at = DEMAND_RECORDS_BASE_OFF;
        a[at..at + 4].copy_from_slice(&0u32.to_le_bytes());
        a[at + 4..at + 8].copy_from_slice(&1u32.to_le_bytes());
        let offsets = [0u32, 1, 1];
        for (index, value) in offsets.iter().enumerate() {
            let at = DEMAND_OFFSETS_BASE_OFF + index * 4;
            a[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        let base = arena_base() as u32;
        let status = effectplan_resource_schedule(
            ABI_VERSION,
            2,
            1,
            1,
            base + CAPACITIES_BASE_OFF as u32,
            CAPACITIES_REGION_LEN as u32,
            base + DEMAND_OFFSETS_BASE_OFF as u32,
            DEMAND_OFFSETS_REGION_LEN as u32,
            base + DEMAND_RECORDS_BASE_OFF as u32,
            DEMAND_RECORDS_REGION_LEN as u32,
            base + DURATIONS_BASE_OFF as u32,
            DURATIONS_REGION_LEN as u32,
            base + OUT_BASE_OFF as u32,
            OUTPUT_REGION_LEN as u32,
            base + TIMING_BASE_OFF as u32,
            TIMING_REGION_LEN as u32,
            base + SCHEDULE_BASE_OFF as u32,
            SCHEDULE_REGION_LEN as u32,
            base + RESOURCE_STATS_BASE_OFF as u32,
            RESOURCE_STATS_REGION_LEN as u32,
            base + RESOURCE_DIAGNOSTICS_BASE_OFF as u32,
            RESOURCE_DIAGNOSTICS_REGION_LEN as u32,
        );
        assert_eq!(
            status, ERR_OUTPUT_INVALID,
            "a graph out of range is refused"
        );
        assert_eq!(word(SCHEDULE_BASE_OFF + S_PLACED), 0);
        // The whole region is cleared on refusal — status included — so a host
        // reads "nothing here" rather than a schedule with an error attached.
        assert_eq!(word(SCHEDULE_BASE_OFF + S_STATUS), 0);
        // A refused run clears the header: revision 0 is how a host tells
        // "rejected" from "produced a short schedule".
        assert_eq!(word(SCHEDULE_BASE_OFF + S_REVISION), 0);
    }
}

#[test]
fn a_back_edge_in_an_otherwise_plausible_graph_is_refused() {
    // Every emitted edge points forward; a forged backward edge would let a
    // schedule place a call before its own predecessor. Same guard as the levels
    // and timing passes.
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    unsafe {
        let a = arena_bytes();
        for b in a.iter_mut() {
            *b = 0;
        }
        a[OUT_BASE_OFF..OUT_BASE_OFF + 4].copy_from_slice(&ABI_VERSION.to_le_bytes());
        a[OUT_BASE_OFF + 4..OUT_BASE_OFF + 8].copy_from_slice(&2u32.to_le_bytes());
        a[OUT_BASE_OFF + 8..OUT_BASE_OFF + 12].copy_from_slice(&1u32.to_le_bytes());
        let record = OUT_BASE_OFF + OUT_RECORDS_OFF;
        a[record..record + 4].copy_from_slice(&1u32.to_le_bytes()); // from
        a[record + 4..record + 8].copy_from_slice(&0u32.to_le_bytes()); // to: backwards
        for index in 0..2 {
            let at = DURATIONS_BASE_OFF + index * 4;
            a[at..at + 4].copy_from_slice(&1u32.to_le_bytes());
        }
        a[CAPACITIES_BASE_OFF..CAPACITIES_BASE_OFF + 4].copy_from_slice(&1u32.to_le_bytes());
        let offsets = [0u32, 0, 0];
        for (index, value) in offsets.iter().enumerate() {
            let at = DEMAND_OFFSETS_BASE_OFF + index * 4;
            a[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        let base = arena_base() as u32;
        let status = effectplan_resource_schedule(
            ABI_VERSION,
            2,
            1,
            0,
            base + CAPACITIES_BASE_OFF as u32,
            CAPACITIES_REGION_LEN as u32,
            base + DEMAND_OFFSETS_BASE_OFF as u32,
            DEMAND_OFFSETS_REGION_LEN as u32,
            base + DEMAND_RECORDS_BASE_OFF as u32,
            DEMAND_RECORDS_REGION_LEN as u32,
            base + DURATIONS_BASE_OFF as u32,
            DURATIONS_REGION_LEN as u32,
            base + OUT_BASE_OFF as u32,
            OUTPUT_REGION_LEN as u32,
            base + TIMING_BASE_OFF as u32,
            TIMING_REGION_LEN as u32,
            base + SCHEDULE_BASE_OFF as u32,
            SCHEDULE_REGION_LEN as u32,
            base + RESOURCE_STATS_BASE_OFF as u32,
            RESOURCE_STATS_REGION_LEN as u32,
            base + RESOURCE_DIAGNOSTICS_BASE_OFF as u32,
            RESOURCE_DIAGNOSTICS_REGION_LEN as u32,
        );
        assert_eq!(status, ERR_OUTPUT_INVALID);
    }
}

#[test]
fn placement_terminates_on_a_maximum_batch_instead_of_stalling() {
    // 128 calls, one exclusive lane: the loop must place all of them and stop,
    // inside the round budget, rather than discovering a livelock under load.
    let ops: Vec<Op> = (0..MAX_CALLS).map(|_| op(&[], &[(0, 1)], 1)).collect();
    let placed = place(&ops, &[1]);
    assert_feasible(&ops, &[1], &placed);
    assert_eq!(placed.makespan, MAX_CALLS as u32);
    assert!(
        (placed.rounds as usize) <= 4 * MAX_CALLS + 8,
        "rounds {} exceeded the budget",
        placed.rounds
    );
}

#[test]
fn a_scheduled_batch_leaves_the_dependency_regions_alone() {
    // A schedule that quietly edited the dependency answer would be worse than a
    // slow one, so every successful placement is checked against the planner's own
    // output, captured after planning and compared after placing.
    let ops = [
        op(&[write(1)], &[(0, 1)], 3),
        op(&[read(1)], &[(0, 1)], 3),
        op(&[], &[(0, 1)], 2),
    ];
    let placed = place(&ops, &[1]);
    assert_eq!(placed.status, OK);
    assert!(placed.graph_intact, "the placement wrote the graph region");
    assert!(
        placed.timing_intact,
        "the placement wrote the timing region"
    );
}
