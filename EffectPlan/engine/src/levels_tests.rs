//! Host-side unit tests for execution levels.
//!
//! Every case goes through `effectplan_plan_v2`, the entry point the
//! WebAssembly host calls, so the ABI, the region checks and the level
//! computation are covered together.

use super::*;
use crate::levels::MAX_LEVEL;
use crate::ARENA_LOCK as LOCK;

/// One call of a test batch: an effect mode and its accesses.
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

/// Packs calls into the four input arrays the engine reads.
fn pack(calls: &[Call]) -> (Vec<u8>, Vec<u32>, Vec<u32>, Vec<u8>) {
    let mut modes = Vec::with_capacity(calls.len());
    let mut offsets = Vec::with_capacity(calls.len() + 1);
    let mut rids = Vec::new();
    let mut amodes = Vec::new();
    offsets.push(0u32);
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

unsafe fn arena_bytes() -> &'static mut [u8] {
    core::slice::from_raw_parts_mut(core::ptr::addr_of_mut!(ARENA) as *mut u8, ARENA_BYTES)
}

/// Runs a batch through the extended ABI and returns the status, the edges and
/// the execution levels.
fn run_levels(calls: &[Call], resource_count: usize) -> (i32, Vec<(u32, u32, u32, u32)>, Vec<u16>) {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let (modes, offsets, rids, amodes) = pack(calls);
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
        let base = arena_base() as u32;
        effectplan_plan_v2(
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
        )
    };
    if status != OK {
        return (status, Vec::new(), Vec::new());
    }
    unsafe {
        let a = arena_bytes();
        let read_u32 = |at: usize| u32::from_le_bytes(a[at..at + 4].try_into().unwrap());
        let edge_count = read_u32(OUT_BASE_OFF + 8);
        let mut edges = Vec::new();
        let mut i = 0u32;
        while i < edge_count {
            let at = OUT_BASE_OFF + OUT_RECORDS_OFF + (i as usize) * EDGE_BYTES;
            edges.push((
                u32::from_le_bytes(a[at..at + 4].try_into().unwrap()),
                u32::from_le_bytes(a[at + 4..at + 8].try_into().unwrap()),
                u32::from_le_bytes(a[at + 8..at + 12].try_into().unwrap()),
                u32::from_le_bytes(a[at + 12..at + 16].try_into().unwrap()),
            ));
            i += 1;
        }
        let count = read_u32(LEVELS_BASE_OFF);
        let mut levels = Vec::new();
        let mut i = 0u32;
        while i < count {
            let at = LEVELS_BASE_OFF + LEVELS_HEADER_BYTES + (i as usize) * 2;
            levels.push(u16::from_le_bytes(a[at..at + 2].try_into().unwrap()));
            i += 1;
        }
        (OK, edges, levels)
    }
}

/// The waves the levels describe: call indices grouped by level, in input
/// order, with the waves in ascending level order.
fn waves(levels: &[u16]) -> Vec<Vec<u32>> {
    let mut waves: Vec<Vec<u32>> = vec![Vec::new(); levels.len()];
    for (call, level) in levels.iter().enumerate() {
        waves[*level as usize].push(call as u32);
    }
    waves.retain(|wave| !wave.is_empty());
    waves
}

fn assert_levels(calls: &[Call], resource_count: usize, expected: &[u16]) {
    let (status, edges, levels) = run_levels(calls, resource_count);
    assert_eq!(status, OK, "the engine refused the batch");
    assert_eq!(levels, expected, "unexpected execution levels");
    // Every edge must point from a lower level to a higher one.
    for (pred, succ, _, _) in &edges {
        assert!(
            levels[*pred as usize] < levels[*succ as usize],
            "edge {pred}->{succ} does not respect the levels: {levels:?}"
        );
    }
    // The waves must partition the calls.
    let grouped: usize = waves(&levels).iter().map(|wave| wave.len()).sum();
    assert_eq!(grouped, levels.len(), "the waves must cover every call");
}

#[test]
fn empty_batch_has_no_levels_and_no_waves() {
    let (status, edges, levels) = run_levels(&[], 1);
    assert_eq!(status, OK);
    assert!(edges.is_empty());
    assert!(levels.is_empty());
    assert!(waves(&levels).is_empty());
}

#[test]
fn one_call_is_one_wave() {
    assert_levels(&[call(MODE_RESOURCES, &[write(1)])], 2, &[0]);
}

#[test]
fn independent_calls_share_one_wave() {
    let calls: Vec<Call> = (1..=8).map(|i| call(MODE_RESOURCES, &[write(i)])).collect();
    assert_levels(&calls, 9, &[0, 0, 0, 0, 0, 0, 0, 0]);
    let (_, _, levels) = run_levels(&calls, 9);
    assert_eq!(waves(&levels), vec![vec![0, 1, 2, 3, 4, 5, 6, 7]]);
}

#[test]
fn a_sequential_chain_gives_one_call_per_wave() {
    let calls = vec![
        call(MODE_RESOURCES, &[write(1)]),
        call(MODE_RESOURCES, &[read(1)]),
        call(MODE_RESOURCES, &[write(1)]),
        call(MODE_RESOURCES, &[read(1)]),
    ];
    assert_levels(&calls, 2, &[0, 1, 2, 3]);
}

#[test]
fn independent_branches_share_a_wave() {
    // 0 writes r1; 1 and 2 read r1; 3 writes r1.
    let calls = vec![
        call(MODE_RESOURCES, &[write(1)]),
        call(MODE_RESOURCES, &[read(1)]),
        call(MODE_RESOURCES, &[read(1)]),
        call(MODE_RESOURCES, &[write(1)]),
    ];
    assert_levels(&calls, 2, &[0, 1, 1, 2]);
    let (_, _, levels) = run_levels(&calls, 2);
    assert_eq!(waves(&levels), vec![vec![0], vec![1, 2], vec![3]]);
}

#[test]
fn a_diamond_is_layered_by_depth() {
    let calls = vec![
        call(MODE_RESOURCES, &[write(1)]),
        call(MODE_RESOURCES, &[read(1)]),
        call(MODE_RESOURCES, &[read(1)]),
        call(MODE_RESOURCES, &[write(1)]),
    ];
    let (_, edges, levels) = run_levels(&calls, 2);
    assert_eq!(levels, vec![0, 1, 1, 2]);
    // The two middle calls are independent of each other.
    assert!(
        !edges.iter().any(|(p, s, _, _)| *p == 1 && *s == 2),
        "read/read must stay independent: {edges:?}"
    );
}

#[test]
fn a_level_is_one_more_than_its_deepest_predecessor() {
    // 0 writes r1; 1 reads r1; 2 writes r2; 3 writes r1; 4 reads r1 and writes
    // r2. Call 4 has two predecessors: call 3 (level 2) and call 2 (level 0).
    // Its level must be 3, the deepest predecessor plus one, not 1.
    let calls = vec![
        call(MODE_RESOURCES, &[write(1)]),
        call(MODE_RESOURCES, &[read(1)]),
        call(MODE_RESOURCES, &[write(2)]),
        call(MODE_RESOURCES, &[write(1)]),
        call(MODE_RESOURCES, &[read(1), write(2)]),
    ];
    let (_, edges, levels) = run_levels(&calls, 3);
    assert_eq!(levels, vec![0, 1, 0, 2, 3]);
    for (pred, succ, _, _) in &edges {
        assert!(levels[*pred as usize] + 1 <= levels[*succ as usize]);
    }
}

#[test]
fn disconnected_components_are_layered_independently() {
    let calls = vec![
        call(MODE_RESOURCES, &[write(1)]),
        call(MODE_RESOURCES, &[read(1)]),
        call(MODE_RESOURCES, &[write(2)]),
        call(MODE_RESOURCES, &[read(2)]),
    ];
    assert_levels(&calls, 3, &[0, 1, 0, 1]);
    let (_, _, levels) = run_levels(&calls, 3);
    assert_eq!(waves(&levels), vec![vec![0, 2], vec![1, 3]]);
}

#[test]
fn a_barrier_never_shares_a_wave_with_anything() {
    // An exclusive call is a global barrier: nothing may share its wave.
    let calls = vec![
        call(MODE_RESOURCES, &[write(1)]),
        call(MODE_EXCLUSIVE, &[]),
        call(MODE_RESOURCES, &[read(1)]),
    ];
    let (_, _, levels) = run_levels(&calls, 2);
    assert_eq!(levels, vec![0, 1, 2]);
    for wave in waves(&levels) {
        assert!(wave.len() == 1, "a barrier wave holds one call: {wave:?}");
    }
}

#[test]
fn an_unknown_effect_keeps_its_barrier_semantics() {
    let calls = vec![
        call(MODE_RESOURCES, &[write(1)]),
        call(MODE_UNKNOWN, &[]),
        call(MODE_RESOURCES, &[read(1)]),
    ];
    let (_, _, levels) = run_levels(&calls, 2);
    assert_eq!(levels, vec![0, 1, 2]);
}

#[test]
fn duplicate_accesses_do_not_change_the_levels() {
    let once = vec![
        call(MODE_RESOURCES, &[write(1)]),
        call(MODE_RESOURCES, &[read(1)]),
    ];
    let twice = vec![
        call(MODE_RESOURCES, &[write(1), write(1)]),
        call(MODE_RESOURCES, &[read(1), read(1)]),
    ];
    assert_levels(&once, 2, &[0, 1]);
    assert_levels(&twice, 2, &[0, 1]);
}

#[test]
fn the_maximum_batch_is_layered() {
    let mut calls: Vec<Call> = Vec::new();
    for i in 0..MAX_CALLS {
        let rid = (i % 64) as u32 + 1;
        calls.push(call(MODE_RESOURCES, &[write(rid), read(rid)]));
    }
    let (status, edges, levels) = run_levels(&calls, 65);
    assert_eq!(status, OK);
    assert_eq!(levels.len(), MAX_CALLS);
    assert!(edges.len() <= MAX_EDGES, "the edge bound must hold");
    for (pred, succ, _, _) in &edges {
        assert!(levels[*pred as usize] < levels[*succ as usize]);
    }
    // Every level is strictly below the call count.
    for level in &levels {
        assert!((*level as usize) < MAX_CALLS);
    }
}

#[test]
fn the_deepest_chain_reaches_the_maximum_level() {
    let mut calls: Vec<Call> = Vec::new();
    for i in 0..MAX_CALLS {
        let mode = if i % 2 == 0 {
            ACCESS_WRITE
        } else {
            ACCESS_READ
        };
        calls.push(call(MODE_RESOURCES, &[(1, mode)]));
    }
    let (_, _, levels) = run_levels(&calls, 2);
    assert_eq!(levels.len(), MAX_CALLS);
    assert_eq!(levels[MAX_CALLS - 1], MAX_LEVEL as u16);
    for (i, level) in levels.iter().enumerate() {
        assert_eq!(*level as usize, i, "a chain has one call per level");
    }
}

#[test]
fn every_edge_points_forward() {
    // The invariant the level computation relies on, over a mixed workload.
    let mut calls: Vec<Call> = Vec::new();
    for i in 0..64 {
        let mode = if i % 5 == 0 {
            MODE_EXCLUSIVE
        } else {
            MODE_RESOURCES
        };
        let accesses = if mode == MODE_EXCLUSIVE {
            Vec::new()
        } else {
            vec![write((i % 7) as u32 + 1), read(((i * 3) % 7) as u32 + 1)]
        };
        calls.push(call(mode, &accesses));
    }
    let (status, edges, _) = run_levels(&calls, 8);
    assert_eq!(status, OK);
    for (pred, succ, _, _) in &edges {
        assert!(pred < succ, "edge {pred}->{succ} does not point forward");
    }
}

#[test]
fn the_levels_are_deterministic() {
    let calls = vec![
        call(MODE_RESOURCES, &[write(1)]),
        call(MODE_RESOURCES, &[read(1)]),
        call(MODE_UNKNOWN, &[]),
        call(MODE_RESOURCES, &[write(2)]),
    ];
    let (_, _, first) = run_levels(&calls, 3);
    let (_, _, second) = run_levels(&calls, 3);
    assert_eq!(first, second);
}

#[test]
fn a_levels_pointer_that_is_not_canonical_is_refused() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let (modes, offsets, rids, amodes) = pack(&[call(MODE_RESOURCES, &[write(1)])]);
    let status = unsafe {
        let a = arena_bytes();
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
        effectplan_plan_v2(
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
            base + OUT_BASE_OFF as u32, // not the levels region
            LEVELS_REGION_LEN as u32,
        )
    };
    assert_eq!(status, ERR_LAYOUT);
}

#[test]
fn a_wrong_levels_length_is_refused() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let (modes, offsets, rids, amodes) = pack(&[call(MODE_RESOURCES, &[write(1)])]);
    let status = unsafe {
        let a = arena_bytes();
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
        effectplan_plan_v2(
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
            LEVELS_REGION_LEN as u32 - 2,
        )
    };
    assert_eq!(status, ERR_LEVELS_CAP);
}

#[test]
fn the_version_one_entry_point_is_unchanged() {
    // The original ABI keeps its meaning, its regions and its behaviour.
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let (modes, offsets, rids, amodes) = pack(&[
        call(MODE_RESOURCES, &[write(1)]),
        call(MODE_RESOURCES, &[read(1)]),
    ]);
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
        let base = arena_base() as u32;
        effectplan_plan(
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
        )
    };
    assert_eq!(status, OK);
    unsafe {
        let a = arena_bytes();
        let edge_count =
            u32::from_le_bytes(a[OUT_BASE_OFF + 8..OUT_BASE_OFF + 12].try_into().unwrap());
        assert_eq!(edge_count, 1);
        // The version one call leaves the levels region untouched.
        let count = u32::from_le_bytes(a[LEVELS_BASE_OFF..LEVELS_BASE_OFF + 4].try_into().unwrap());
        assert_eq!(count, 0, "the version one entry point writes no levels");
    }
}

#[test]
fn a_record_that_does_not_point_forward_is_refused() {
    // The level computation must not plan on a graph that breaks the
    // invariant, even if such a graph were somehow produced.
    let mut records = vec![0u8; EDGE_BYTES * 2];
    // Edge 0: 2 -> 1, which points backwards.
    records[0..4].copy_from_slice(&2u32.to_le_bytes());
    records[4..8].copy_from_slice(&1u32.to_le_bytes());
    let mut levels = vec![0u8; LEVELS_REGION_LEN];
    assert_eq!(
        levels::compute_execution_levels(&records, 1, &mut levels, 3),
        ERR_OUTPUT_INVALID
    );
}

#[test]
fn a_level_above_the_call_count_is_refused() {
    // Two calls, but a record that claims a successor beyond the batch.
    let mut records = vec![0u8; EDGE_BYTES];
    records[0..4].copy_from_slice(&0u32.to_le_bytes());
    records[4..8].copy_from_slice(&5u32.to_le_bytes());
    let mut levels = vec![0u8; LEVELS_REGION_LEN];
    assert_eq!(
        levels::compute_execution_levels(&records, 1, &mut levels, 2),
        ERR_OUTPUT_INVALID
    );
}

#[test]
fn the_levels_region_starts_with_the_number_of_entries() {
    let (_, _, levels) = run_levels(&[call(MODE_RESOURCES, &[write(1)])], 2);
    assert_eq!(levels.len(), 1);
    unsafe {
        let a = arena_bytes();
        let count = u32::from_le_bytes(a[LEVELS_BASE_OFF..LEVELS_BASE_OFF + 4].try_into().unwrap());
        assert_eq!(count, 1, "the region publishes how many entries are valid");
    }
}
