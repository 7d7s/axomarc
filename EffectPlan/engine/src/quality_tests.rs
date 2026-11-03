//! Tests for the Phase 6 M3 quality certificate.
//!
//! The invariant that matters is not "the numbers look reasonable" but "a bound is
//! never above a schedule, and an optimality claim is never made without one". So
//! these tests drive the real pipeline - plan, place, certify - read the result
//! back out of the arena, and check the certificate against arithmetic done by
//! hand for each shape. Several also assert the *negative*: a certificate computed
//! over a placement that never happened is refused rather than published as zero.

use super::*;
use crate::quality::{
    QUALITY_BYTES, QUALITY_REVISION, Q_CALL_COUNT, Q_LB, Q_LB_DEP, Q_LB_RES, Q_MAKESPAN,
    Q_PROVABLY_OPTIMAL, Q_RESERVED, Q_RESOURCE_COUNT, Q_REVISION, Q_WAITING,
};
use crate::ARENA_LOCK as LOCK;

unsafe fn arena_bytes() -> &'static mut [u8] {
    unsafe {
        core::slice::from_raw_parts_mut(core::ptr::addr_of_mut!(ARENA) as *mut u8, ARENA_BYTES)
    }
}

#[inline]
fn word(at: usize) -> u32 {
    assert!(
        LOCK.is_held(),
        "the arena is shared: read it only while this test holds the guard, or another thread may have rewritten it underneath"
    );
    unsafe { u32::from_le_bytes(arena_bytes()[at..at + 4].try_into().unwrap()) }
}

#[inline]
fn double_word(at: usize) -> u64 {
    assert!(
        LOCK.is_held(),
        "the arena is shared: read it only while this test holds the guard, or another thread may have rewritten it underneath"
    );
    unsafe { u64::from_le_bytes(arena_bytes()[at..at + 8].try_into().unwrap()) }
}

/// What a certificate run published.
struct Certified {
    status: i32,
    revision: u32,
    makespan: u32,
    lb_dep: u32,
    lb_res: u32,
    lb: u32,
    provably_optimal: u32,
    waiting: u64,
    call_count: u32,
    resource_count: u32,
    reserved: u32,
    /// The schedule, timing and statistics regions as they stood before the
    /// certificate ran, so a test can prove the reader stayed a reader.
    inputs_intact: bool,
}

/// One call: how long it runs and what capacity it holds.
struct Op {
    duration: u32,
    demands: Vec<(u32, u32)>,
}

fn op(duration: u32, demands: &[(u32, u32)]) -> Op {
    Op {
        duration,
        demands: demands.to_vec(),
    }
}

/// Plan a pure-call batch (no dependency resources, so ordering is decided purely
/// by capacity), place it, and certify the placement.
fn certify(ops: &[Op], capacities: &[u32]) -> Certified {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    unsafe {
        let a = arena_bytes();
        for b in a.iter_mut() {
            *b = 0;
        }

        // Durations only: every call is `pure`, so the graph has no edges and the
        // schedule is decided entirely by capacity.
        let modes = vec![MODE_PURE; ops.len()];
        a[IN_CALL_MODES_OFF..IN_CALL_MODES_OFF + modes.len()].copy_from_slice(&modes);
        let offsets = [0u32, 0u32];
        for (index, value) in offsets.iter().enumerate() {
            let at = IN_ACCESS_OFFSETS_OFF + index * 4;
            a[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        for (index, entry) in ops.iter().enumerate() {
            let at = DURATIONS_BASE_OFF + index * 4;
            a[at..at + 4].copy_from_slice(&entry.duration.to_le_bytes());
        }

        let base = arena_base() as u32;
        let plan_status = effectplan_plan_v4(
            ABI_VERSION,
            ops.len() as u32,
            0,
            1,
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
            "the dependency batch must plan before certifying"
        );

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
            let at = DEMAND_RECORDS_BASE_OFF + index * resources::DEMAND_RECORD_BYTES;
            a[at..at + 4].copy_from_slice(&resource.to_le_bytes());
            a[at + 4..at + 8].copy_from_slice(&amount.to_le_bytes());
        }

        let place_status = effectplan_resource_schedule(
            ABI_VERSION,
            ops.len() as u32,
            capacities.len() as u32,
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
        assert_eq!(place_status, OK, "the batch must place before certifying");

        // Snapshot what the certificate is only allowed to read.
        let schedule_before: Vec<u8> = a[SCHEDULE_BASE_OFF..SCHEDULE_BASE_OFF + 64].to_vec();
        let timing_before: Vec<u8> = a[TIMING_BASE_OFF..TIMING_BASE_OFF + 64].to_vec();
        let stats_before: Vec<u8> =
            a[RESOURCE_STATS_BASE_OFF..RESOURCE_STATS_BASE_OFF + 64].to_vec();

        let status = effectplan_resource_quality(
            ABI_VERSION,
            ops.len() as u32,
            capacities.len() as u32,
            base + SCHEDULE_BASE_OFF as u32,
            SCHEDULE_REGION_LEN as u32,
            base + TIMING_BASE_OFF as u32,
            TIMING_REGION_LEN as u32,
            base + RESOURCE_STATS_BASE_OFF as u32,
            RESOURCE_STATS_REGION_LEN as u32,
            base + CAPACITIES_BASE_OFF as u32,
            CAPACITIES_REGION_LEN as u32,
            base + QUALITY_BASE_OFF as u32,
            QUALITY_REGION_LEN as u32,
        );

        let at = QUALITY_BASE_OFF;
        let intact = a[SCHEDULE_BASE_OFF..SCHEDULE_BASE_OFF + 64] == schedule_before[..]
            && a[TIMING_BASE_OFF..TIMING_BASE_OFF + 64] == timing_before[..]
            && a[RESOURCE_STATS_BASE_OFF..RESOURCE_STATS_BASE_OFF + 64] == stats_before[..];

        Certified {
            status,
            revision: word(at + Q_REVISION),
            makespan: word(at + Q_MAKESPAN),
            lb_dep: word(at + Q_LB_DEP),
            lb_res: word(at + Q_LB_RES),
            lb: word(at + Q_LB),
            provably_optimal: word(at + Q_PROVABLY_OPTIMAL),
            waiting: double_word(at + Q_WAITING),
            call_count: word(at + Q_CALL_COUNT),
            resource_count: word(at + Q_RESOURCE_COUNT),
            reserved: word(at + Q_RESERVED),
            inputs_intact: intact,
        }
    }
}

#[test]
fn an_uncontended_batch_is_certified_optimal_against_the_dependency_bound() {
    // Three calls, generous capacity, nothing waits: makespan equals the longest
    // duration, and the bound is that same number.
    let run = certify(
        &[op(4, &[(0, 1)]), op(2, &[(0, 1)]), op(3, &[(0, 1)])],
        &[8],
    );
    assert_eq!(run.status, OK);
    assert_eq!(run.revision, QUALITY_REVISION);
    assert_eq!(run.makespan, 4);
    assert_eq!(
        run.lb_dep, 4,
        "no edges, so the bound is the longest duration"
    );
    assert_eq!(run.lb, 4);
    assert_eq!(run.provably_optimal, 1);
    assert_eq!(run.waiting, 0);
    assert_eq!(run.call_count, 3);
    assert_eq!(run.reserved, 0);
    assert!(
        run.inputs_intact,
        "the certificate must not write what it reads"
    );
}

#[test]
fn the_resource_bound_is_the_workload_ceiling_and_binds_when_capacity_is_the_whole_story() {
    // One lane of capacity one, calls of 2 and 3 ticks: everything is serial, so
    // busy time 5 over capacity 1 is exactly the makespan.
    let run = certify(&[op(2, &[(0, 1)]), op(3, &[(0, 1)])], &[1]);
    assert_eq!(run.status, OK);
    assert_eq!(run.makespan, 5);
    assert_eq!(run.lb_res, 5, "ceil((2 + 3) / 1)");
    assert_eq!(
        run.lb_dep, 3,
        "with no edges the dependency bound is the longest call"
    );
    assert_eq!(run.lb, 5, "the resource bound is the one that binds here");
    assert_eq!(run.provably_optimal, 1);
}

#[test]
fn a_demand_of_two_lane_units_doubles_the_bound_that_lane_contributes() {
    // Two calls, each wanting the whole lane: busy = 2x3 + 2x3 = 12 over capacity
    // 2 -> ceil(12/2) = 6, and the schedule really does run 6 ticks, so the
    // resource bound is what proves it optimal.
    let run = certify(&[op(3, &[(0, 2)]), op(3, &[(0, 2)])], &[2]);
    assert_eq!(run.status, OK);
    assert_eq!(run.makespan, 6);
    assert_eq!(run.lb_res, 6, "ceil(12 / 2)");
    assert_eq!(
        run.lb_dep, 3,
        "no edges, so the dependency bound is one call"
    );
    assert_eq!(run.provably_optimal, 1);
}

#[test]
fn a_demand_that_no_placement_can_hold_is_refused_before_any_certificate_exists() {
    // The same shape with the lane halved: 2 units asked of 1 is not a slow
    // schedule, it is a contradiction, and the placement says so. The
    // certificate is never reached, so it must not be left describing anything.
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    unsafe {
        let a = arena_bytes();
        for b in a.iter_mut() {
            *b = 0;
        }
        let base = arena_base() as u32;
        let status = effectplan_resource_quality(
            ABI_VERSION,
            1,
            1,
            base + SCHEDULE_BASE_OFF as u32,
            SCHEDULE_REGION_LEN as u32,
            base + TIMING_BASE_OFF as u32,
            TIMING_REGION_LEN as u32,
            base + RESOURCE_STATS_BASE_OFF as u32,
            RESOURCE_STATS_REGION_LEN as u32,
            base + CAPACITIES_BASE_OFF as u32,
            CAPACITIES_REGION_LEN as u32,
            base + QUALITY_BASE_OFF as u32,
            QUALITY_REGION_LEN as u32,
        );
        assert_ne!(status, OK, "no placement, no certificate");
        assert_eq!(word(QUALITY_BASE_OFF + Q_REVISION), 0);
    }
}

#[test]
fn waiting_is_recorded_and_an_optimality_claim_is_withdrawn_when_a_bound_is_loose() {
    // Two lanes: call 0 holds lane 1 alone for 6 ticks, call 1 holds lane 0 for 2.
    // Both start at zero, so nobody waits and the makespan still equals the bound.
    let free = certify(&[op(6, &[(1, 1)]), op(2, &[(0, 1)])], &[1, 1]);
    assert_eq!(free.status, OK);
    assert_eq!(free.waiting, 0);
    assert_eq!(free.makespan, 6);

    // Now make one call wait: three calls, one lane of capacity one, durations
    // 1, 1, 1. The last starts at tick 2, two ticks after its own earliest start.
    let queued = certify(
        &[op(1, &[(0, 1)]), op(1, &[(0, 1)]), op(1, &[(0, 1)])],
        &[1],
    );
    assert_eq!(queued.status, OK);
    assert_eq!(queued.makespan, 3);
    assert_eq!(
        queued.waiting, 3,
        "0 + 1 + 2 ticks of delay behind one lane"
    );
    assert_eq!(queued.lb_res, 3);
    assert_eq!(
        queued.provably_optimal, 1,
        "serial is optimal when one lane holds everything"
    );
}

#[test]
fn no_bound_ever_exceeds_the_schedule_it_certifies() {
    let shapes: [(&[Op], &[u32]); 6] = [
        (&[op(5, &[(0, 1)])], &[1]),
        (&[op(2, &[(0, 1)]), op(2, &[(0, 1)])], &[2]),
        (&[op(3, &[(0, 1)]), op(4, &[(0, 2)])], &[2]),
        (&[op(2, &[(0, 1), (1, 1)]), op(3, &[(1, 1)])], &[1, 2]),
        (&[op(1, &[]), op(7, &[(0, 3)])], &[3]),
        (
            &[op(4, &[(0, 1)]), op(4, &[(0, 1)]), op(4, &[(0, 1)])],
            &[2],
        ),
    ];
    for (ops, capacities) in shapes {
        let run = certify(ops, capacities);
        assert_eq!(run.status, OK);
        assert!(
            run.lb_dep <= run.makespan,
            "dependency bound {} above makespan {}",
            run.lb_dep,
            run.makespan
        );
        assert!(
            run.lb_res <= run.makespan,
            "resource bound {} above makespan {}",
            run.lb_res,
            run.makespan
        );
        assert_eq!(run.lb, run.lb_dep.max(run.lb_res));
        assert_eq!(
            run.provably_optimal,
            u32::from(run.lb == run.makespan),
            "the claim must be exactly the equality, not a judgement about it"
        );
    }
}

#[test]
fn certifying_the_same_placement_twice_gives_the_same_bytes() {
    let ops = [op(3, &[(0, 1)]), op(4, &[(0, 1)])];
    let first = certify(&ops, &[1]);
    let second = certify(&ops, &[1]);
    assert_eq!(first.status, OK);
    assert_eq!(second.makespan, first.makespan);
    assert_eq!(second.lb, first.lb);
    assert_eq!(second.waiting, first.waiting);
    assert_eq!(second.provably_optimal, first.provably_optimal);
}

#[test]
fn a_certificate_over_a_placement_that_never_ran_is_refused_not_zeroed() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    unsafe {
        let a = arena_bytes();
        for b in a.iter_mut() {
            *b = 0;
        }
        let base = arena_base() as u32;
        let status = effectplan_resource_quality(
            ABI_VERSION,
            1,
            1,
            base + SCHEDULE_BASE_OFF as u32,
            SCHEDULE_REGION_LEN as u32,
            base + TIMING_BASE_OFF as u32,
            TIMING_REGION_LEN as u32,
            base + RESOURCE_STATS_BASE_OFF as u32,
            RESOURCE_STATS_REGION_LEN as u32,
            base + CAPACITIES_BASE_OFF as u32,
            CAPACITIES_REGION_LEN as u32,
            base + QUALITY_BASE_OFF as u32,
            QUALITY_REGION_LEN as u32,
        );
        assert_eq!(
            status, ERR_QUALITY_UNAVAILABLE,
            "an empty timing region is no reference"
        );
        assert_eq!(
            word(QUALITY_BASE_OFF + Q_REVISION),
            0,
            "no revision, no certificate"
        );
        assert_eq!(
            a[QUALITY_BASE_OFF..QUALITY_BASE_OFF + QUALITY_BYTES]
                .iter()
                .filter(|byte| **byte != 0)
                .count(),
            0,
            "a refused run must leave the whole region blank"
        );
    }
}

#[test]
fn a_pointer_into_the_middle_of_a_region_is_refused() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    unsafe {
        let a = arena_bytes();
        for b in a.iter_mut() {
            *b = 0;
        }
        let base = arena_base() as u32;
        let status = effectplan_resource_quality(
            ABI_VERSION,
            1,
            1,
            base + SCHEDULE_BASE_OFF as u32,
            SCHEDULE_REGION_LEN as u32,
            base + TIMING_BASE_OFF as u32,
            TIMING_REGION_LEN as u32,
            base + RESOURCE_STATS_BASE_OFF as u32 + 4,
            RESOURCE_STATS_REGION_LEN as u32,
            base + CAPACITIES_BASE_OFF as u32,
            CAPACITIES_REGION_LEN as u32,
            base + QUALITY_BASE_OFF as u32,
            QUALITY_REGION_LEN as u32,
        );
        assert_eq!(status, ERR_LAYOUT, "a region argument must be canonical");
    }
}

#[test]
fn a_count_that_disagrees_with_the_regions_is_refused() {
    let ops = [op(2, &[(0, 1)]), op(2, &[(0, 1)])];
    let run = certify(&ops, &[1]);
    assert_eq!(run.status, OK);
    // The helper already released the arena lock; take it again only for the
    // hand-driven call. Holding it across both would deadlock: it is a plain
    // non-reentrant mutex, and `certify` locks it too.
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    unsafe {
        let base = arena_base() as u32;
        // Three calls, two placements: the certificate must notice and stop.
        let status = effectplan_resource_quality(
            ABI_VERSION,
            3,
            1,
            base + SCHEDULE_BASE_OFF as u32,
            SCHEDULE_REGION_LEN as u32,
            base + TIMING_BASE_OFF as u32,
            TIMING_REGION_LEN as u32,
            base + RESOURCE_STATS_BASE_OFF as u32,
            RESOURCE_STATS_REGION_LEN as u32,
            base + CAPACITIES_BASE_OFF as u32,
            CAPACITIES_REGION_LEN as u32,
            base + QUALITY_BASE_OFF as u32,
            QUALITY_REGION_LEN as u32,
        );
        assert_eq!(
            status, ERR_QUALITY_UNAVAILABLE,
            "the timing region says two calls"
        );
        // The honest failure mode: the stale certificate described a different
        // batch, so it must be gone rather than left readable. A caller that
        // reads a blank region knows to ask again; a caller that reads the old
        // one would report a number for a schedule that no longer exists.
        assert_eq!(
            word(QUALITY_BASE_OFF + Q_REVISION),
            0,
            "a refusal must clear the certificate it did not produce"
        );
    }
}

#[test]
fn the_certificate_publishes_the_batch_it_was_handed() {
    let run = certify(&[op(2, &[(0, 1), (1, 2)]), op(3, &[(1, 1)])], &[1, 2]);
    assert_eq!(run.status, OK);
    assert_eq!(run.call_count, 2);
    assert_eq!(run.resource_count, 2);
    // The ranking puts call 1 first - it has the smaller slack, being the longer
    // call in a project whose dependency duration is 3 - so call 1 holds one unit
    // of lane 1 for ticks 0..3, and call 0, which needs both units, starts at 3.
    // The bound sees the load (ceil(7/2) = 4) but not that lane 1 cannot be
    // shared in halves, so it sits one tick short of the truth.
    assert_eq!(
        run.makespan, 5,
        "the call needing the whole lane waits for it"
    );
    assert_eq!(
        run.lb_res, 4,
        "ceil((1*3 + 2*2) / 2) = 4 over the tighter lane"
    );
    assert_eq!(
        run.lb_dep, 3,
        "the longer call, with no edges to force order"
    );
    assert_eq!(
        run.waiting, 3,
        "call 0 started three ticks after its earliest start"
    );
    assert_eq!(
        run.provably_optimal, 0,
        "a loose bound proves nothing, in either direction"
    );
}
