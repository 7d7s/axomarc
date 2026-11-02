//! Host-side tests for resumable planning (Phase 4, M3 gate).
//!
//! The claim under test is the one the whole phase rests on: resuming from a
//! checkpoint produces *exactly* the bytes a fresh full plan produces — graph,
//! levels and schedule — and anything that is not that checkpoint is refused
//! before it can extend the graph.

use super::*;
use crate::checkpoint::{
    CHECKPOINT_RECORD_BYTES, CHECKPOINT_REVISION, CP_CALLS, CP_DIGEST, CP_EDGES, CP_EVENTS,
    CP_FLAGS, CP_RESERVED, CP_REVISION,
};
use crate::ARENA_LOCK as LOCK;
use crate::{CHECKPOINT_BASE_OFF, CHECKPOINT_REGION_LEN};

#[derive(Clone)]
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

fn pack(calls: &[Call]) -> (Vec<u8>, Vec<u32>, Vec<u32>, Vec<u8>) {
    let mut modes = Vec::new();
    let mut offsets = vec![0u32];
    let mut rids = Vec::new();
    let mut amodes = Vec::new();
    for c in calls {
        modes.push(c.mode);
        for (rid, amode) in &c.accesses {
            rids.push(*rid);
            amodes.push(*amode);
        }
        offsets.push(rids.len() as u32);
    }
    (modes, offsets, rids, amodes)
}

unsafe fn arena_bytes() -> &'static mut [u8] {
    core::slice::from_raw_parts_mut(core::ptr::addr_of_mut!(ARENA) as *mut u8, ARENA_BYTES)
}

#[inline]
fn u32_at(at: usize) -> u32 {
    unsafe { u32::from_le_bytes(arena_bytes()[at..at + 4].try_into().unwrap()) }
}

/// The state that has to travel with a checkpoint: the conflict frontier and the
/// event nodes (the workspace region), the records emitted so far, and the
/// engine's own commitment about what they mean.
#[derive(Clone)]
struct Snapshot {
    workspace: Vec<u8>,
    records: Vec<u8>,
    record: Vec<u8>,
}

fn capture() -> Snapshot {
    let edges = u32_at(OUT_BASE_OFF + 8) as usize;
    unsafe {
        let a = arena_bytes();
        Snapshot {
            workspace: a[WS_BASE_OFF..WS_BASE_OFF + WORKSPACE_REGION_LEN].to_vec(),
            records: a[OUT_BASE_OFF + OUT_RECORDS_OFF
                ..OUT_BASE_OFF + OUT_RECORDS_OFF + edges * EDGE_BYTES]
                .to_vec(),
            record: a[CHECKPOINT_BASE_OFF..CHECKPOINT_BASE_OFF + CHECKPOINT_REGION_LEN].to_vec(),
        }
    }
}

/// The graph, the waves and the schedule, read back after a successful run.
#[derive(Debug, PartialEq)]
struct Plan {
    edges: Vec<(u32, u32, u32, u32)>,
    levels: Vec<u16>,
    makespan: u32,
    peak: u32,
    total_work: u64,
    critical: u32,
    events: Vec<(u32, u32)>,
    checkpoint: (u32, u32, u32, u32),
}

fn read_plan(durations: usize) -> Plan {
    unsafe {
        let a = arena_bytes();
        let word = |at: usize| u32::from_le_bytes(a[at..at + 4].try_into().unwrap());
        let edges = word(OUT_BASE_OFF + 8) as usize;
        let mut graph = Vec::with_capacity(edges);
        for i in 0..edges {
            let at = OUT_BASE_OFF + OUT_RECORDS_OFF + i * EDGE_BYTES;
            graph.push((word(at), word(at + 4), word(at + 8), word(at + 12)));
        }
        let level_count = word(LEVELS_BASE_OFF) as usize;
        let mut levels = Vec::with_capacity(level_count);
        for i in 0..level_count {
            let at = LEVELS_BASE_OFF + LEVELS_HEADER_BYTES + i * 2;
            levels.push(u16::from_le_bytes([a[at], a[at + 1]]));
        }
        let t = TIMING_BASE_OFF;
        let count = word(t + schedule::T_EVENT_COUNT) as usize;
        let mut events = Vec::with_capacity(count);
        for i in 0..count {
            let at = t + schedule::T_EVENTS + i * TIMING_EVENT_BYTES;
            events.push((word(at), word(at + 4)));
        }
        Plan {
            edges: graph,
            levels,
            makespan: word(t + schedule::T_MAKESPAN),
            peak: word(t + schedule::T_PEAK),
            total_work: u64::from_le_bytes(
                a[t + schedule::T_TOTAL_WORK..t + schedule::T_TOTAL_WORK + 8]
                    .try_into()
                    .unwrap(),
            ),
            critical: word(t + schedule::T_CRITICAL_COUNT),
            events,
            checkpoint: (
                word(CHECKPOINT_BASE_OFF + CP_REVISION),
                word(CHECKPOINT_BASE_OFF + CP_CALLS),
                word(CHECKPOINT_BASE_OFF + CP_EVENTS),
                word(CHECKPOINT_BASE_OFF + CP_EDGES),
            ),
        }
        .with_durations(durations)
    }
}

impl Plan {
    /// Fails loudly if a run reported success for a batch it did not schedule.
    fn with_durations(self, durations: usize) -> Self {
        assert_eq!(
            self.events.len(),
            durations,
            "the timing region must hold one row per call"
        );
        self
    }
}

/// Packs a batch, optionally restores a snapshot first, and runs the resumable
/// entry point. Returns the status and the plan the arena now describes.
fn run(calls: &[Call], durations: &[u32], restore: Option<&Snapshot>) -> (i32, Plan) {
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
        if let Some(snap) = restore {
            a[WS_BASE_OFF..WS_BASE_OFF + snap.workspace.len()].copy_from_slice(&snap.workspace);
            let at = OUT_BASE_OFF + OUT_RECORDS_OFF;
            a[at..at + snap.records.len()].copy_from_slice(&snap.records);
            a[CHECKPOINT_BASE_OFF..CHECKPOINT_BASE_OFF + snap.record.len()]
                .copy_from_slice(&snap.record);
        }
        let base = arena_base() as u32;
        effectplan_plan_v4(
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
            base + CHECKPOINT_BASE_OFF as u32,
            CHECKPOINT_REGION_LEN as u32,
        )
    };
    if status != OK {
        return (status, empty_plan());
    }
    let plan = read_plan(durations.len());
    assert_eq!(
        plan.checkpoint.0, CHECKPOINT_REVISION,
        "v4 must commit a record"
    );
    (status, plan)
}

fn empty_plan() -> Plan {
    Plan {
        edges: Vec::new(),
        levels: Vec::new(),
        makespan: 0,
        peak: 0,
        total_work: 0,
        critical: 0,
        events: Vec::new(),
        checkpoint: (0, 0, 0, 0),
    }
}

/// A batch with a mix of barrier effects, read/write conflicts and independent
/// resources — the shapes that exercise every frontier mutation.
fn workload() -> Vec<Call> {
    vec![
        call(MODE_RESOURCES, &[write(1)]),
        call(MODE_RESOURCES, &[read(1)]),
        call(MODE_RESOURCES, &[read(1), write(2)]),
        call(MODE_EXCLUSIVE, &[]),
        call(MODE_RESOURCES, &[read(2)]),
        call(MODE_UNKNOWN, &[]),
        call(MODE_RESOURCES, &[read(1), read(2), write(3)]),
        call(MODE_PURE, &[]),
        call(MODE_RESOURCES, &[write(3)]),
        call(MODE_RESOURCES, &[read(3)]),
    ]
}

fn ticks(n: usize) -> Vec<u32> {
    (0..n).map(|i| 1 + (i as u32 * 3) % 7).collect()
}

#[test]
fn an_empty_record_plans_the_batch_and_commits_a_checkpoint() {
    let calls = workload();
    let durations = ticks(calls.len());
    let (status, plan) = run(&calls, &durations, None);
    assert_eq!(status, OK);
    assert_eq!(plan.checkpoint.0, CHECKPOINT_REVISION);
    assert_eq!(plan.checkpoint.1, calls.len() as u32);
    assert_eq!(plan.checkpoint.3 as usize, plan.edges.len());
    assert!(plan.checkpoint.2 > 0, "the run must have appended events");
    assert!(!plan.edges.is_empty(), "the workload must conflict");
    // The same batch through the Phase 3 entry point must produce the same graph,
    // levels and schedule: v4 adds a commitment, not a different plan.
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let v3 = unsafe {
        let a = arena_bytes();
        for b in a.iter_mut() {
            *b = 0;
        }
        let (modes, offsets, rids, amodes) = pack(&calls);
        let resource_count = rids.iter().copied().max().unwrap_or(0) as usize + 1;
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
        let status = effectplan_plan_v3(
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
        );
        assert_eq!(status, OK);
        // v3 must not write the checkpoint region: read it as all-zero.
        let ckpt = &a[CHECKPOINT_BASE_OFF..CHECKPOINT_BASE_OFF + CHECKPOINT_REGION_LEN];
        assert!(
            ckpt.iter().all(|byte| *byte == 0),
            "v3 wrote the checkpoint"
        );
        read_plan(durations.len())
    };
    // The plan is identical; only the commitment differs, because v3 has no
    // checkpoint to commit.
    let mut comparable = plan;
    comparable.checkpoint = v3.checkpoint;
    assert_eq!(v3, comparable);
}

#[test]
fn a_run_may_decline_the_scheduling_pass_and_still_commit() {
    let calls = workload();
    // Zero durations would be refused by the scheduling pass; asked for no
    // scheduling, they are simply not read.
    let (status, graph, levels, timing_revision) = run_without_timing(&calls, None);
    assert_eq!(status, OK, "an unscheduled run must not need durations");
    assert!(!graph.is_empty());
    assert_eq!(levels.len(), calls.len());
    assert_eq!(
        timing_revision, 0xdead_beef,
        "the timing region was touched"
    );

    // The flag survives the commit, so a resume inherits the mode it was given.
    // Read it before any other run overwrites the record.
    let kept = u32_at(CHECKPOINT_BASE_OFF + CP_FLAGS);
    assert_eq!(kept, crate::checkpoint::FLAG_WITHOUT_TIMING);

    // And the graph it produces is the graph a normal run produces.
    let (status, plan) = run(&calls, &ticks(calls.len()), None);
    assert_eq!(status, OK);
    assert_eq!(graph, plan.edges);
    assert_eq!(levels, plan.levels);

    // Resuming without timing keeps working from that checkpoint.
    let snapshot = capture();
    let (status, _, _, _) = run_without_timing(&calls, Some(&snapshot));
    assert_eq!(status, OK);
}

#[test]
fn an_unknown_flag_is_refused() {
    let calls = workload();
    let durations = ticks(calls.len());
    run(&calls[..4], &durations[..4], None);
    let snapshot = capture();
    let mut unknown = snapshot.clone();
    unknown.record[CP_FLAGS..CP_FLAGS + 4].copy_from_slice(&2u32.to_le_bytes());
    let (status, _) = run(&calls, &durations, Some(&unknown));
    assert_eq!(status, ERR_CHECKPOINT_INVALID, "flags are not a free field");
}

/// The same batch with the scheduling pass declined. A sentinel in the timing
/// region proves the engine left it alone.
fn run_without_timing(
    calls: &[Call],
    restore: Option<&Snapshot>,
) -> (i32, Vec<(u32, u32, u32, u32)>, Vec<u16>, u32) {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let (modes, offsets, rids, amodes) = pack(calls);
    let resource_count = rids.iter().copied().max().unwrap_or(0) as usize + 1;
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
        if let Some(snap) = restore {
            a[WS_BASE_OFF..WS_BASE_OFF + snap.workspace.len()].copy_from_slice(&snap.workspace);
            let at = OUT_BASE_OFF + OUT_RECORDS_OFF;
            a[at..at + snap.records.len()].copy_from_slice(&snap.records);
            a[CHECKPOINT_BASE_OFF..CHECKPOINT_BASE_OFF + snap.record.len()]
                .copy_from_slice(&snap.record);
        }
        a[TIMING_BASE_OFF..TIMING_BASE_OFF + 4].copy_from_slice(&0xdead_beefu32.to_le_bytes());
        a[CHECKPOINT_BASE_OFF + CP_FLAGS..CHECKPOINT_BASE_OFF + CP_FLAGS + 4]
            .copy_from_slice(&crate::checkpoint::FLAG_WITHOUT_TIMING.to_le_bytes());
        let base = arena_base() as u32;
        let status = effectplan_plan_v4(
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
            base + CHECKPOINT_BASE_OFF as u32,
            CHECKPOINT_REGION_LEN as u32,
        );
        let u32_here = |at: usize| u32::from_le_bytes(a[at..at + 4].try_into().unwrap());
        let emitted = u32_here(OUT_BASE_OFF + 8) as usize;
        let mut graph = Vec::with_capacity(emitted);
        for i in 0..emitted {
            let at = OUT_BASE_OFF + OUT_RECORDS_OFF + i * EDGE_BYTES;
            graph.push((
                u32_here(at),
                u32_here(at + 4),
                u32_here(at + 8),
                u32_here(at + 12),
            ));
        }
        let count = u32_here(LEVELS_BASE_OFF) as usize;
        let mut levels = Vec::with_capacity(count);
        for i in 0..count {
            let at = LEVELS_BASE_OFF + LEVELS_HEADER_BYTES + i * 2;
            levels.push(u16::from_le_bytes([a[at], a[at + 1]]));
        }
        let timing_revision = u32_here(TIMING_BASE_OFF);
        (status, graph, levels, timing_revision)
    }
}

#[test]
fn resuming_at_every_boundary_reproduces_the_full_plan() {
    let calls = workload();
    let durations = ticks(calls.len());
    let (_, full) = run(&calls, &durations, None);
    for boundary in 0..=calls.len() {
        let (_, _) = run(&calls[..boundary], &durations[..boundary], None);
        let snapshot = capture();
        let (status, resumed) = run(&calls, &durations, Some(&snapshot));
        assert_eq!(status, OK, "resume at {boundary} was refused");
        assert_eq!(
            resumed, full,
            "resume at boundary {boundary} disagrees with a full plan"
        );
        assert_eq!(
            resumed.checkpoint.1,
            calls.len() as u32,
            "the committed boundary must be the end of the batch"
        );
    }
}

#[test]
fn appending_after_a_committed_batch_matches_a_fresh_batch() {
    let base = workload();
    let mut extended = base.clone();
    extended.push(call(MODE_RESOURCES, &[read(1), write(4)]));
    let durations = ticks(extended.len());
    let (status, _) = run(&base, &durations[..base.len()], None);
    assert_eq!(status, OK);
    let snapshot = capture();
    let (status, appended) = run(&extended, &durations, Some(&snapshot));
    assert_eq!(status, OK);
    let (_, fresh) = run(&extended, &durations, None);
    assert_eq!(appended, fresh);
    // The append must have reused the whole prefix and added exactly the edges of
    // the new call: the checkpoint says where the engine stopped before it.
    assert_eq!(
        u32::from_le_bytes(snapshot.record[CP_CALLS..CP_CALLS + 4].try_into().unwrap()),
        base.len() as u32
    );
}

#[test]
fn removing_a_call_reuses_the_prefix_in_front_of_it() {
    let calls = workload();
    let drop_at = 3; // the exclusive barrier call
    let mut after = calls.clone();
    after.remove(drop_at);
    let durations = ticks(after.len());
    // A checkpoint of the calls before the removal.
    let (status, _) = run(&calls[..drop_at], &ticks(drop_at), None);
    assert_eq!(status, OK);
    let snapshot = capture();
    let (status, resumed) = run(&after, &durations, Some(&snapshot));
    assert_eq!(status, OK);
    let (_, fresh) = run(&after, &durations, None);
    assert_eq!(resumed, fresh, "removal must agree with a fresh plan");
}

#[test]
fn editing_inside_the_prefix_refuses_the_checkpoint() {
    let calls = workload();
    let durations = ticks(calls.len());
    let (status, _) = run(&calls, &durations, None);
    assert_eq!(status, OK);
    let snapshot = capture();
    // Now the same batch with one access flipped inside the preserved prefix.
    let mut edited = calls.clone();
    edited[1] = call(MODE_RESOURCES, &[write(1)]);
    let (status, plan) = run(&edited, &durations, Some(&snapshot));
    assert_eq!(status, ERR_CHECKPOINT_INVALID);
    assert_eq!(plan, empty_plan());
    // The refused run must not leave a resumable record behind.
    assert_eq!(u32_at(CHECKPOINT_BASE_OFF + CP_REVISION), 0);
}

#[test]
fn a_frontier_that_names_a_call_that_has_not_run_is_refused() {
    let calls = workload();
    let durations = ticks(calls.len());
    run(&calls[..4], &durations[..4], None);
    let snapshot = capture();
    let mut tampered = snapshot;
    // Point a resource's last writer at a call beyond the checkpoint.
    tampered.workspace[WS_LAST_WRITER_OFF + 4..WS_LAST_WRITER_OFF + 8]
        .copy_from_slice(&9u32.to_le_bytes());
    let (status, _) = run(&calls, &durations, Some(&tampered));
    assert_eq!(status, ERR_CHECKPOINT_INVALID);
}

#[test]
fn a_reader_chain_that_does_not_recede_is_refused() {
    let calls = workload();
    let durations = ticks(calls.len());
    run(&calls[..4], &durations[..4], None);
    let snapshot = capture();
    let mut tampered = snapshot;
    // The first node's link may be empty or point backwards, never forwards.
    tampered.workspace[WS_EVENT_NEXT_OFF..WS_EVENT_NEXT_OFF + 4]
        .copy_from_slice(&6u32.to_le_bytes());
    let (status, _) = run(&calls, &durations, Some(&tampered));
    assert_eq!(status, ERR_CHECKPOINT_INVALID);
}

#[test]
fn counters_beyond_what_the_prefix_could_produce_are_refused() {
    let calls = workload();
    let durations = ticks(calls.len());
    run(&calls[..4], &durations[..4], None);
    let snapshot = capture();
    let mut inflated = snapshot;
    // Six records cannot come out of four calls and two accesses.
    inflated.record[CP_EDGES..CP_EDGES + 4].copy_from_slice(&9_000u32.to_le_bytes());
    let (status, _) = run(&calls, &durations, Some(&inflated));
    assert_eq!(status, ERR_CHECKPOINT_RANGE);
}

#[test]
fn a_digest_that_does_not_match_the_state_is_refused() {
    let calls = workload();
    let durations = ticks(calls.len());
    run(&calls[..4], &durations[..4], None);
    let snapshot = capture();
    let mut tampered = snapshot;
    tampered.record[CP_DIGEST..CP_DIGEST + 4].copy_from_slice(&0xdead_beefu32.to_le_bytes());
    let (status, _) = run(&calls, &durations, Some(&tampered));
    assert_eq!(status, ERR_CHECKPOINT_INVALID);
}

#[test]
fn an_unknown_checkpoint_revision_is_refused() {
    let calls = workload();
    let durations = ticks(calls.len());
    run(&calls[..4], &durations[..4], None);
    let snapshot = capture();
    let mut tampered = snapshot;
    tampered.record[CP_REVISION..CP_REVISION + 4].copy_from_slice(&7u32.to_le_bytes());
    let (status, _) = run(&calls, &durations, Some(&tampered));
    assert_eq!(status, ERR_CHECKPOINT_INVALID);
}

#[test]
fn a_zero_revision_with_a_dirty_record_is_refused() {
    let calls = workload();
    let durations = ticks(calls.len());
    let mut tampered = vec![0u8; CHECKPOINT_RECORD_BYTES];
    tampered[CP_CALLS..CP_CALLS + 4].copy_from_slice(&3u32.to_le_bytes());
    let snapshot = Snapshot {
        workspace: vec![0u8; WORKSPACE_REGION_LEN],
        records: Vec::new(),
        record: tampered,
    };
    let (status, _) = run(&calls, &durations, Some(&snapshot));
    assert_eq!(status, ERR_CHECKPOINT_INVALID);
}

#[test]
fn a_non_canonical_checkpoint_pointer_is_refused() {
    let calls = workload();
    let durations = ticks(calls.len());
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let (modes, offsets, rids, amodes) = pack(&calls);
    let resource_count = rids.iter().copied().max().unwrap_or(0) as usize + 1;
    let alias = unsafe {
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
        // Point the checkpoint at the output region: an alias, not a region.
        let status = effectplan_plan_v4(
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
            base + OUT_BASE_OFF as u32,
            CHECKPOINT_REGION_LEN as u32,
        );
        assert_eq!(status, ERR_LAYOUT);
        // A wrong length is its own refusal, before any state is read.
        effectplan_plan_v4(
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
            base + CHECKPOINT_BASE_OFF as u32,
            (CHECKPOINT_RECORD_BYTES + 8) as u32,
        )
    };
    assert_eq!(alias, ERR_CHECKPOINT_RANGE);
}

#[test]
fn the_checkpoint_region_publishes_itself_through_info() {
    let base = arena_base() as u32;
    assert_eq!(effectplan_info(32), CHECKPOINT_REVISION);
    assert_eq!(effectplan_info(33), base + CHECKPOINT_BASE_OFF as u32);
    assert_eq!(effectplan_info(34), CHECKPOINT_REGION_LEN as u32);
    // Phase 5 appended the resource table after this one. The sentinel guard
    // belonged to the Phase 4 boundary; it moves to the new end of the table so
    // that the next extension still fails a test rather than passing quietly.
    assert_eq!(effectplan_info(35), crate::resources::RESOURCE_REVISION);
    assert_eq!(effectplan_info(52), SENTINEL);
    assert_eq!(effectplan_info(u32::MAX), SENTINEL);
}

#[test]
fn a_checkpoint_survives_a_refused_suffix_and_can_be_retried() {
    let calls = workload();
    let durations = ticks(calls.len());
    run(&calls[..4], &durations[..4], None);
    let snapshot = capture();
    // A suffix the scheduler refuses: one zero duration past the checkpoint.
    let mut broken = durations.clone();
    broken[6] = 0;
    let (status, _) = run(&calls, &broken, Some(&snapshot));
    assert_eq!(status, ERR_DURATION_ZERO);
    assert_eq!(
        u32_at(CHECKPOINT_BASE_OFF + CP_REVISION),
        0,
        "record survived a refused run"
    );
    assert_eq!(u32_at(TIMING_BASE_OFF + schedule::T_REVISION), 0);
    // The same checkpoint is still usable for a batch that does satisfy it.
    let (status, plan) = run(&calls, &durations, Some(&snapshot));
    assert_eq!(status, OK);
    let (_, fresh) = run(&calls, &durations, None);
    assert_eq!(plan, fresh);
}

#[test]
fn reserved_checkpoint_words_must_be_zero_on_commit() {
    let calls = workload();
    let durations = ticks(calls.len());
    let (status, _) = run(&calls, &durations, None);
    assert_eq!(status, OK);
    for offset in [CP_RESERVED, CP_RESERVED + 4] {
        assert_eq!(u32_at(CHECKPOINT_BASE_OFF + offset), 0);
    }
}

#[test]
fn a_long_chain_resumes_from_every_checkpoint_of_a_dense_interval() {
    // Sixty-four calls all on one resource: the deepest graph the workload has,
    // checked at every 8-call boundary rather than only the ones the host would
    // naturally take.
    let writes = [write(1)];
    let reads = [read(1)];
    let calls: Vec<Call> = (0..64)
        .map(|i| {
            call(
                MODE_RESOURCES,
                if i % 2 == 0 { &writes[..] } else { &reads[..] },
            )
        })
        .collect();
    let durations = ticks(calls.len());
    let (_, full) = run(&calls, &durations, None);
    let mut boundary = 0usize;
    loop {
        run(&calls[..boundary], &durations[..boundary], None);
        let snapshot = capture();
        let (status, resumed) = run(&calls, &durations, Some(&snapshot));
        assert_eq!(status, OK, "boundary {boundary} refused");
        assert_eq!(resumed, full, "boundary {boundary} disagrees");
        if boundary >= calls.len() {
            break;
        }
        boundary += 8;
    }
}

#[test]
fn restoring_state_without_a_record_plans_the_whole_batch() {
    // A host that pushes the frontier but forgets the commitment gets a full run,
    // not a resumed one: the prefix it restored is unproven, so it is discarded.
    let calls = workload();
    let durations = ticks(calls.len());
    run(&calls[..4], &durations[..4], None);
    let snapshot = capture();
    let mut without_record = snapshot;
    without_record.record = vec![0u8; CHECKPOINT_RECORD_BYTES];
    let (status, plan) = run(&calls, &durations, Some(&without_record));
    assert_eq!(status, OK);
    let (_, fresh) = run(&calls, &durations, None);
    assert_eq!(plan, fresh);
}
