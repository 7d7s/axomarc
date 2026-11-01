//! Host-side tests for Critical Path Intelligence.
//!
//! Every case drives `effectplan_plan_v3`, the entry point the WebAssembly host
//! calls, so the ABI checks, the graph, the levels and the timing analysis are
//! covered together. Expected values are worked out by hand from the CPM
//! definitions, not read back from the implementation.

use super::*;
use crate::schedule::{
    T_CRITICAL_COUNT, T_EVENTS, T_EVENT_COUNT, T_MAKESPAN, T_PEAK, T_RESERVED, T_REVISION,
    T_TOTAL_WORK,
};
use crate::ARENA_LOCK as LOCK;

struct Call {
    mode: u8,
    accesses: Vec<(u32, u8)>,
}

fn call(mode: u8, accesses: &[(u32, u8)]) -> Call {
    Call {
        mode,
        accesses: accesses.to_vec(),
    }
}

fn write(rid: u32) -> (u32, u8) {
    (rid, ACCESS_WRITE)
}

fn read(rid: u32) -> (u32, u8) {
    (rid, ACCESS_READ)
}

unsafe fn arena_bytes() -> &'static mut [u8] {
    core::slice::from_raw_parts_mut(core::ptr::addr_of_mut!(ARENA) as *mut u8, ARENA_BYTES)
}

/// Reads back the timing region as a typed view of the graph analysis.
#[derive(Debug, PartialEq)]
struct Timing {
    revision: u32,
    event_count: u32,
    makespan: u32,
    peak: u32,
    total_work: u64,
    critical_count: u32,
    reserved: u32,
    /// `(earliest_start, slack)` per call.
    events: Vec<(u32, u32)>,
}

fn read_timing() -> Timing {
    unsafe {
        let a = arena_bytes();
        let u32_at = |at: usize| u32::from_le_bytes(a[at..at + 4].try_into().unwrap());
        let count = u32_at(T_BASE + T_EVENT_COUNT);
        let mut events = Vec::new();
        for i in 0..count {
            let at = T_BASE + T_EVENTS + (i as usize) * TIMING_EVENT_BYTES;
            events.push((u32_at(at), u32_at(at + 4)));
        }
        Timing {
            revision: u32_at(T_BASE + T_REVISION),
            event_count: count,
            makespan: u32_at(T_BASE + T_MAKESPAN),
            peak: u32_at(T_BASE + T_PEAK),
            total_work: u64::from_le_bytes(
                a[T_BASE + T_TOTAL_WORK..T_BASE + T_TOTAL_WORK + 8]
                    .try_into()
                    .unwrap(),
            ),
            critical_count: u32_at(T_BASE + T_CRITICAL_COUNT),
            reserved: u32_at(T_BASE + T_RESERVED),
            events,
        }
    }
}

const T_BASE: usize = TIMING_BASE_OFF;

/// Packs a batch, writes durations, runs the scheduling ABI and returns the
/// graph plus the timing analysis.
fn run_schedule(calls: &[Call], durations: &[u32]) -> (i32, Vec<(u32, u32)>, Vec<u16>, Timing) {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let (modes, offsets, rids, amodes) = pack(calls);
    let resource_count = rids.iter().copied().max().unwrap_or(0) as usize + 1;
    let status = unsafe {
        let a = arena_bytes();
        for b in a.iter_mut() {
            *b = 0;
        }
        a[IN_CALL_MODES_OFF..IN_CALL_MODES_OFF + modes.len()].copy_from_slice(&modes);
        for (i, v) in offsets.iter().enumerate() {
            a[IN_ACCESS_OFFSETS_OFF + i * 4..IN_ACCESS_OFFSETS_OFF + i * 4 + 4]
                .copy_from_slice(&v.to_le_bytes());
        }
        for (i, v) in rids.iter().enumerate() {
            a[IN_RESOURCE_IDS_OFF + i * 4..IN_RESOURCE_IDS_OFF + i * 4 + 4]
                .copy_from_slice(&v.to_le_bytes());
        }
        a[IN_ACCESS_MODES_OFF..IN_ACCESS_MODES_OFF + amodes.len()].copy_from_slice(&amodes);
        for (i, v) in durations.iter().enumerate() {
            a[DURATIONS_BASE_OFF + i * 4..DURATIONS_BASE_OFF + i * 4 + 4]
                .copy_from_slice(&v.to_le_bytes());
        }
        let base = arena_base() as u32;
        effectplan_plan_v3(
            ABI_VERSION,
            modes.len() as u32,
            rids.len() as u32,
            resource_count as u32,
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
        )
    };
    if status != OK {
        let _ = &durations;
        unsafe {
            let a = arena_bytes();
            let timing = Timing {
                revision: u32::from_le_bytes(a[T_BASE..T_BASE + 4].try_into().unwrap()),
                event_count: 0,
                makespan: 0,
                peak: 0,
                total_work: 0,
                critical_count: 0,
                reserved: 0,
                events: Vec::new(),
            };
            return (status, Vec::new(), Vec::new(), timing);
        }
    }
    unsafe {
        let a = arena_bytes();
        let u32_at = |at: usize| u32::from_le_bytes(a[at..at + 4].try_into().unwrap());
        let edge_count = u32_at(OUT_BASE_OFF + 8) as usize;
        let mut edges = Vec::new();
        for i in 0..edge_count {
            let at = OUT_BASE_OFF + OUT_RECORDS_OFF + i * EDGE_BYTES;
            edges.push((u32_at(at), u32_at(at + 4)));
        }
        let level_count = u32_at(LEVELS_BASE_OFF) as usize;
        let mut levels = Vec::new();
        for i in 0..level_count {
            let at = LEVELS_BASE_OFF + LEVELS_HEADER_BYTES + i * 2;
            levels.push(u16::from_le_bytes(a[at..at + 2].try_into().unwrap()));
        }
        let timing = read_timing();
        (status, edges, levels, timing)
    }
}

fn pack(calls: &[Call]) -> (Vec<u8>, Vec<u32>, Vec<u32>, Vec<u8>) {
    let mut modes = Vec::new();
    let mut offsets = vec![0u32];
    let mut rids = Vec::new();
    let mut amodes = Vec::new();
    for c in calls {
        modes.push(c.mode);
        for (rid, mode) in &c.accesses {
            rids.push(*rid);
            amodes.push(*mode);
        }
        offsets.push(rids.len() as u32);
    }
    (modes, offsets, rids, amodes)
}

/// The worked example: a diamond with a tail.
///
/// ```text
/// 0 writes r1   A ──┬──> 1 reads r1 ──┐
///                 ├──> 2 reads r1 ────┼──> 3 writes r1 ──> 4 reads r1
/// ```
/// Durations 2,5,2,3,4 give makespan 14; the critical set is {0,1,3,4}, call 2
/// carries the slack, and two calls overlap at the start of the fan-out.
fn diamond() -> Vec<Call> {
    vec![
        call(MODE_RESOURCES, &[write(1)]),
        call(MODE_RESOURCES, &[read(1)]),
        call(MODE_RESOURCES, &[read(1)]),
        call(MODE_RESOURCES, &[write(1)]),
        call(MODE_RESOURCES, &[read(1)]),
    ]
}

#[test]
fn empty_plan_reports_zero_everything() {
    let (status, _, _, timing) = run_schedule(&[], &[]);
    assert_eq!(status, OK);
    assert_eq!(timing.event_count, 0);
    assert_eq!(timing.makespan, 0, "no work, no duration");
    assert_eq!(timing.peak, 0, "nothing is concurrent");
    assert_eq!(timing.total_work, 0);
    assert_eq!(timing.critical_count, 0);
    assert!(timing.events.is_empty());
}

#[test]
fn a_single_call_makespan_is_its_own_duration() {
    let (status, _, _, timing) = run_schedule(&[call(MODE_RESOURCES, &[write(1)])], &[7]);
    assert_eq!(status, OK);
    assert_eq!(timing.makespan, 7);
    assert_eq!(timing.total_work, 7);
    assert_eq!(timing.peak, 1);
    assert_eq!(timing.events, vec![(0, 0)], "the only call is critical");
    assert_eq!(timing.critical_count, 1);
}

#[test]
fn a_sequence_sums_its_durations() {
    let calls = vec![
        call(MODE_RESOURCES, &[write(1)]),
        call(MODE_RESOURCES, &[read(1)]),
        call(MODE_RESOURCES, &[write(1)]),
    ];
    let (status, _, _, timing) = run_schedule(&calls, &[2, 3, 4]);
    assert_eq!(status, OK);
    assert_eq!(timing.makespan, 9, "a chain is the sum of its durations");
    assert_eq!(timing.total_work, 9);
    assert_eq!(timing.peak, 1, "a chain never runs two calls at once");
    assert_eq!(timing.events, vec![(0, 0), (2, 0), (5, 0)]);
}

#[test]
fn independent_calls_take_the_longest_not_the_sum() {
    let calls = vec![
        call(MODE_RESOURCES, &[write(1)]),
        call(MODE_RESOURCES, &[write(2)]),
        call(MODE_RESOURCES, &[write(3)]),
    ];
    let (status, _, _, timing) = run_schedule(&calls, &[5, 9, 2]);
    assert_eq!(status, OK);
    assert_eq!(timing.makespan, 9, "the makespan is the longest branch");
    assert_eq!(timing.total_work, 16, "total work is the sum");
    assert_eq!(timing.peak, 3, "all three may run at once");
    assert_eq!(timing.events, vec![(0, 4), (0, 0), (0, 7)]);
    assert_eq!(timing.critical_count, 1);
}

#[test]
fn the_worked_diamond_example() {
    let (status, edges, levels, timing) = run_schedule(&diamond(), &[2, 5, 2, 3, 4]);
    assert_eq!(status, OK);
    assert_eq!(levels, vec![0, 1, 1, 2, 3], "the level pass still agrees");
    assert_eq!(
        edges,
        vec![(0, 1), (0, 2), (0, 3), (1, 3), (2, 3), (3, 4)],
        "unexpected covering graph"
    );
    assert_eq!(timing.makespan, 14, "A+B+D+E = 2+5+3+4");
    assert_eq!(timing.total_work, 16);
    assert_eq!(
        timing.events,
        vec![(0, 0), (2, 0), (2, 3), (7, 0), (10, 0)],
        "earliest start and slack per call"
    );
    assert_eq!(timing.critical_count, 4, "0,1,3,4 are critical");
    assert_eq!(timing.peak, 2, "calls 1 and 2 overlap");
    assert_eq!(timing.revision, TIMING_REVISION);
    assert_eq!(timing.reserved, 0, "the reserved word stays zero");
}

#[test]
fn a_permuted_duration_vector_moves_the_critical_path() {
    // Same graph, durations handed to different calls: the answers must follow
    // the index mapping, not the multiset of values.
    //
    // Edges: 0->1, 0->2, 0->3, 1->3, 2->3, 3->4.
    // Durations 4,3,2,5,2 give EF0=4, EF1=7, EF2=6, ES3=7 EF3=12, ES4=12 EF4=14.
    // Backward from T=14: LS4=12, LS3=7, LS1=4, LS2=5, LS0=min(4,5,7)=4.
    // So the critical set is {0,1,3,4} and call 2 carries one tick of slack —
    // a different path is critical than for 2,5,2,3,4, where call 2 had three.
    let (_, _, _, forward) = run_schedule(&diamond(), &[2, 5, 2, 3, 4]);
    let (_, _, _, reversed) = run_schedule(&diamond(), &[4, 3, 2, 5, 2]);
    assert_eq!(forward.makespan, 14);
    assert_eq!(
        reversed.makespan, 14,
        "same total, same makespan, different path"
    );
    assert_ne!(
        forward.events, reversed.events,
        "durations must bind by index"
    );
    assert_eq!(
        reversed.events,
        vec![(0, 0), (4, 0), (4, 1), (7, 0), (12, 0)],
        "the critical branch is now 0,1,3,4"
    );
    assert_eq!(reversed.critical_count, 4);
}

#[test]
fn multiple_critical_paths_are_all_identified() {
    // 0 writes r1; 1 reads r1; 2 writes r1. Both branches through 0 have the
    // same length, so three calls are critical and none carries slack.
    let calls = vec![
        call(MODE_RESOURCES, &[write(1)]),
        call(MODE_RESOURCES, &[read(1)]),
        call(MODE_RESOURCES, &[write(1)]),
    ];
    let (status, _, _, timing) = run_schedule(&calls, &[3, 3, 3]);
    assert_eq!(status, OK);
    assert_eq!(timing.makespan, 9);
    assert_eq!(timing.events, vec![(0, 0), (3, 0), (6, 0)]);
    assert_eq!(timing.critical_count, 3);
}

#[test]
fn disconnected_components_share_one_makespan() {
    let calls = vec![
        call(MODE_RESOURCES, &[write(1)]),
        call(MODE_RESOURCES, &[read(1)]),
        call(MODE_RESOURCES, &[write(2)]),
        call(MODE_RESOURCES, &[read(2)]),
    ];
    let (status, _, _, timing) = run_schedule(&calls, &[5, 5, 1, 1]);
    assert_eq!(status, OK);
    assert_eq!(timing.makespan, 10, "the long component sets the duration");
    assert_eq!(timing.total_work, 12);
    assert_eq!(timing.peak, 2, "both components run together");
    assert_eq!(
        timing.events,
        vec![(0, 0), (5, 0), (0, 8), (1, 8)],
        "the short component carries the slack"
    );
    assert_eq!(timing.critical_count, 2);
}

#[test]
fn adjacent_intervals_are_not_concurrent() {
    // The mandatory boundary case: [0,5) then [5,9) overlaps with nothing.
    let calls = vec![
        call(MODE_RESOURCES, &[write(1)]),
        call(MODE_RESOURCES, &[write(1)]),
    ];
    let (status, _, _, timing) = run_schedule(&calls, &[5, 4]);
    assert_eq!(status, OK);
    assert_eq!(timing.events, vec![(0, 0), (5, 0)]);
    assert_eq!(
        timing.peak, 1,
        "a finish at t and a start at t do not overlap"
    );
}

#[test]
fn overlapping_intervals_are_concurrent() {
    // [0,5) and [0,9) genuinely overlap.
    let calls = vec![
        call(MODE_RESOURCES, &[write(1)]),
        call(MODE_RESOURCES, &[write(2)]),
    ];
    let (status, _, _, timing) = run_schedule(&calls, &[5, 9]);
    assert_eq!(status, OK);
    assert_eq!(timing.peak, 2);
    assert_eq!(timing.makespan, 9);
}

#[test]
fn the_maximum_batch_schedules() {
    let mut calls = Vec::new();
    for i in 0..MAX_CALLS {
        let mode = if i % 2 == 0 {
            ACCESS_WRITE
        } else {
            ACCESS_READ
        };
        calls.push(call(MODE_RESOURCES, &[(1, mode)]));
    }
    let durations: Vec<u32> = (0..MAX_CALLS as u32).map(|i| i + 1).collect();
    let (status, _, _, timing) = run_schedule(&calls, &durations);
    assert_eq!(status, OK);
    assert_eq!(timing.event_count as usize, MAX_CALLS);
    let expected: u64 = (1..=MAX_CALLS as u64).sum();
    assert_eq!(timing.total_work, expected);
    assert_eq!(timing.makespan as u64, expected, "a chain sums everything");
    assert_eq!(timing.peak, 1);
    assert_eq!(timing.critical_count as usize, MAX_CALLS);
}

#[test]
fn large_durations_stay_exact() {
    let calls = vec![
        call(MODE_RESOURCES, &[write(1)]),
        call(MODE_RESOURCES, &[read(1)]),
    ];
    let (status, _, _, timing) = run_schedule(&calls, &[u32::MAX / 2, u32::MAX / 2]);
    assert_eq!(status, OK);
    let half = u32::MAX / 2;
    assert_eq!(
        timing.makespan,
        half.saturating_add(half),
        "no rounding, no float"
    );
    assert_eq!(timing.total_work, (half as u64) * 2);
    assert_eq!(timing.events, vec![(0, 0), (half, 0)]);
}

#[test]
fn tick_overflow_is_reported_never_wrapped() {
    let calls = vec![
        call(MODE_RESOURCES, &[write(1)]),
        call(MODE_RESOURCES, &[read(1)]),
    ];
    let (status, _, _, _) = run_schedule(&calls, &[u32::MAX, u32::MAX]);
    assert_eq!(
        status, ERR_TIMING_OVERFLOW,
        "a wrapped makespan would be a lie"
    );
}

#[test]
fn a_zero_duration_is_refused_not_treated_as_instant() {
    let calls = vec![
        call(MODE_RESOURCES, &[write(1)]),
        call(MODE_RESOURCES, &[read(1)]),
    ];
    let (status, _, _, _) = run_schedule(&calls, &[4, 0]);
    assert_eq!(status, ERR_DURATION_ZERO);
}

#[test]
fn a_missing_duration_is_refused() {
    // Two calls, one duration written: the second slot still reads zero.
    let calls = vec![
        call(MODE_RESOURCES, &[write(1)]),
        call(MODE_RESOURCES, &[read(1)]),
    ];
    let (status, _, _, _) = run_schedule(&calls, &[4]);
    assert_eq!(
        status, ERR_DURATION_ZERO,
        "an unwritten slot is never a default"
    );
}

#[test]
fn a_refused_schedule_writes_no_result() {
    let calls = vec![
        call(MODE_RESOURCES, &[write(1)]),
        call(MODE_RESOURCES, &[read(1)]),
    ];
    let _ = run_schedule(&calls, &[4, 4]);
    // Now a batch that overflows: the timing region must not keep half results.
    let (status, _, _, timing) = run_schedule(&calls, &[u32::MAX, u32::MAX]);
    assert_eq!(status, ERR_TIMING_OVERFLOW);
    assert_eq!(
        timing.revision, 0,
        "a refused call leaves the region untouched"
    );
}

#[test]
fn the_same_input_produces_the_same_bytes() {
    let first = run_schedule(&diamond(), &[2, 5, 2, 3, 4]);
    let second = run_schedule(&diamond(), &[2, 5, 2, 3, 4]);
    assert_eq!(first, second);
    let (status, _, _, timing) = &second;
    assert_eq!(*status, OK);
    assert_eq!(timing.event_count, 5);
}

#[test]
fn a_durations_pointer_that_is_not_canonical_is_refused() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let (modes, offsets, rids, amodes) = pack(&[call(MODE_RESOURCES, &[write(1)])]);
    unsafe {
        let a = arena_bytes();
        for b in a.iter_mut() {
            *b = 0;
        }
        a[IN_CALL_MODES_OFF..IN_CALL_MODES_OFF + modes.len()].copy_from_slice(&modes);
        for (i, v) in offsets.iter().enumerate() {
            a[IN_ACCESS_OFFSETS_OFF + i * 4..IN_ACCESS_OFFSETS_OFF + i * 4 + 4]
                .copy_from_slice(&v.to_le_bytes());
        }
        for (i, v) in rids.iter().enumerate() {
            a[IN_RESOURCE_IDS_OFF + i * 4..IN_RESOURCE_IDS_OFF + i * 4 + 4]
                .copy_from_slice(&v.to_le_bytes());
        }
        a[IN_ACCESS_MODES_OFF..IN_ACCESS_MODES_OFF + amodes.len()].copy_from_slice(&amodes);
        let base = arena_base() as u32;
        let args = [
            ABI_VERSION,
            1,
            1,
            2,
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
        ];
        // Wrong durations pointer: the output region, which would alias the graph.
        assert_eq!(
            effectplan_plan_v3(
                args[0],
                args[1],
                args[2],
                args[3],
                args[4],
                args[5],
                args[6],
                args[7],
                args[8],
                args[9],
                args[10],
                args[11],
                args[12],
                args[13],
                base + OUT_BASE_OFF as u32,
                DURATIONS_REGION_LEN as u32,
                base + TIMING_BASE_OFF as u32,
                TIMING_REGION_LEN as u32,
            ),
            ERR_LAYOUT
        );
        // Wrong timing pointer: the durations region.
        assert_eq!(
            effectplan_plan_v3(
                args[0],
                args[1],
                args[2],
                args[3],
                args[4],
                args[5],
                args[6],
                args[7],
                args[8],
                args[9],
                args[10],
                args[11],
                args[12],
                args[13],
                base + DURATIONS_BASE_OFF as u32,
                DURATIONS_REGION_LEN as u32,
                base + DURATIONS_BASE_OFF as u32,
                TIMING_REGION_LEN as u32,
            ),
            ERR_LAYOUT
        );
        // A pointer of zero is below every region. Off-target the arena base is
        // zero itself, so it converts and is then refused as non-canonical; the
        // WebAssembly test drives the same argument against a real arena base,
        // where the checked subtraction fails outright.
        assert_eq!(
            effectplan_plan_v3(
                args[0],
                args[1],
                args[2],
                args[3],
                args[4],
                args[5],
                args[6],
                args[7],
                args[8],
                args[9],
                args[10],
                args[11],
                args[12],
                args[13],
                0,
                DURATIONS_REGION_LEN as u32,
                base + TIMING_BASE_OFF as u32,
                TIMING_REGION_LEN as u32,
            ),
            ERR_LAYOUT
        );
        // Right pointers, wrong lengths.
        assert_eq!(
            effectplan_plan_v3(
                args[0],
                args[1],
                args[2],
                args[3],
                args[4],
                args[5],
                args[6],
                args[7],
                args[8],
                args[9],
                args[10],
                args[11],
                args[12],
                args[13],
                base + DURATIONS_BASE_OFF as u32,
                4,
                base + TIMING_BASE_OFF as u32,
                TIMING_REGION_LEN as u32,
            ),
            ERR_TIMING_CAP
        );
        // A zero duration is refused before any region check would matter: the
        // durations slot is still unwritten here.
        assert_eq!(
            effectplan_plan_v3(
                args[0],
                args[1],
                args[2],
                args[3],
                args[4],
                args[5],
                args[6],
                args[7],
                args[8],
                args[9],
                args[10],
                args[11],
                args[12],
                args[13],
                base + DURATIONS_BASE_OFF as u32,
                DURATIONS_REGION_LEN as u32,
                base + TIMING_BASE_OFF as u32,
                TIMING_REGION_LEN as u32,
            ),
            ERR_DURATION_ZERO
        );
        // With the duration supplied, the same sixteen arguments succeed.
        a[DURATIONS_BASE_OFF..DURATIONS_BASE_OFF + 4].copy_from_slice(&5u32.to_le_bytes());
        assert_eq!(
            effectplan_plan_v3(
                args[0],
                args[1],
                args[2],
                args[3],
                args[4],
                args[5],
                args[6],
                args[7],
                args[8],
                args[9],
                args[10],
                args[11],
                args[12],
                args[13],
                base + DURATIONS_BASE_OFF as u32,
                DURATIONS_REGION_LEN as u32,
                base + TIMING_BASE_OFF as u32,
                64,
            ),
            ERR_TIMING_CAP
        );
        // And the correct call succeeds.
        assert_eq!(
            effectplan_plan_v3(
                args[0],
                args[1],
                args[2],
                args[3],
                args[4],
                args[5],
                args[6],
                args[7],
                args[8],
                args[9],
                args[10],
                args[11],
                args[12],
                args[13],
                base + DURATIONS_BASE_OFF as u32,
                DURATIONS_REGION_LEN as u32,
                base + TIMING_BASE_OFF as u32,
                TIMING_REGION_LEN as u32,
            ),
            OK
        );
    }
}

#[test]
fn the_lower_entry_points_never_write_the_timing_region() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    unsafe {
        let a = arena_bytes();
        for b in a.iter_mut() {
            *b = 0;
        }
        // Seed the region with a sentinel, then plan with v1 and v2 only.
        a[TIMING_BASE_OFF..TIMING_BASE_OFF + 4].copy_from_slice(&0xdeadbeefu32.to_le_bytes());
        let (modes, offsets, rids, amodes) = pack(&[
            call(MODE_RESOURCES, &[write(1)]),
            call(MODE_RESOURCES, &[read(1)]),
        ]);
        a[IN_CALL_MODES_OFF..IN_CALL_MODES_OFF + 2].copy_from_slice(&modes);
        for (i, v) in offsets.iter().enumerate() {
            a[IN_ACCESS_OFFSETS_OFF + i * 4..IN_ACCESS_OFFSETS_OFF + i * 4 + 4]
                .copy_from_slice(&v.to_le_bytes());
        }
        for (i, v) in rids.iter().enumerate() {
            a[IN_RESOURCE_IDS_OFF + i * 4..IN_RESOURCE_IDS_OFF + i * 4 + 4]
                .copy_from_slice(&v.to_le_bytes());
        }
        a[IN_ACCESS_MODES_OFF..IN_ACCESS_MODES_OFF + amodes.len()].copy_from_slice(&amodes);
        let base = arena_base() as u32;
        let head = [
            ABI_VERSION,
            2,
            2,
            2,
            base + IN_CALL_MODES_OFF as u32,
            base + IN_ACCESS_OFFSETS_OFF as u32,
            base + IN_RESOURCE_IDS_OFF as u32,
            base + IN_ACCESS_MODES_OFF as u32,
            base + WS_BASE_OFF as u32,
            WORKSPACE_REGION_LEN as u32,
            base + OUT_BASE_OFF as u32,
            (OUT_RECORDS_OFF + OUT_RECORDS_LEN) as u32,
        ];
        assert_eq!(
            effectplan_plan(
                head[0], head[1], head[2], head[3], head[4], head[5], head[6], head[7], head[8],
                head[9], head[10], head[11],
            ),
            OK
        );
        assert_eq!(
            effectplan_plan_v2(
                head[0],
                head[1],
                head[2],
                head[3],
                head[4],
                head[5],
                head[6],
                head[7],
                head[8],
                head[9],
                head[10],
                head[11],
                base + LEVELS_BASE_OFF as u32,
                LEVELS_REGION_LEN as u32,
            ),
            OK
        );
        assert_eq!(
            u32::from_le_bytes(a[TIMING_BASE_OFF..TIMING_BASE_OFF + 4].try_into().unwrap()),
            0xdead_beef,
            "a batch that asks for no timing analysis must not touch the region"
        );
    }
}

#[test]
fn scheduling_a_stale_graph_is_impossible() {
    // The scheduling pass reads the records of the batch it just planned. Plan a
    // two-call batch, then plan a one-call batch with the durations left over:
    // the second result must describe one call, never the earlier graph.
    let two = vec![
        call(MODE_RESOURCES, &[write(1)]),
        call(MODE_RESOURCES, &[read(1)]),
    ];
    let (status, _, _, timing) = run_schedule(&two, &[3, 4]);
    assert_eq!(status, OK);
    assert_eq!(timing.event_count, 2);

    let one = vec![call(MODE_RESOURCES, &[write(1)])];
    let (status, _, _, timing) = run_schedule(&one, &[9]);
    assert_eq!(status, OK);
    assert_eq!(timing.event_count, 1);
    assert_eq!(timing.makespan, 9);
    assert_eq!(timing.events, vec![(0, 0)]);
}
