//! Host-side unit tests for the planner core.
//!
//! Tests execute through the same `effectplan_plan` entry point the WebAssembly
//! host calls, so the ABI surface itself is covered, not just the algorithm.
//! `cargo test` runs cases on threads that would share `static mut ARENA`, so
//! every case goes through the `LOCK` guard below.

use super::*;
use crate::ARENA_LOCK as LOCK;

#[derive(Clone, Copy)]
struct Edge {
    pred: u32,
    succ: u32,
    res: u32,
    reason: u32,
}

impl Edge {
    fn t(self) -> (u32, u32, u32, u32) {
        (self.pred, self.succ, self.res, self.reason)
    }
}

fn e(pred: u32, succ: u32, res: u32, reason: u32) -> (u32, u32, u32, u32) {
    Edge {
        pred,
        succ,
        res,
        reason,
    }
    .t()
}

unsafe fn arena_bytes() -> &'static mut [u8] {
    core::slice::from_raw_parts_mut(core::ptr::addr_of_mut!(ARENA) as *mut u8, ARENA_BYTES)
}

/// Writes one batch into the canonical input regions, runs the ABI entry point
/// and returns the status plus the decoded records.
fn run_batch(
    modes: &[u8],
    offsets: &[u32],
    rids: &[u32],
    amodes: &[u8],
    resource_count: usize,
    output_capacity: Option<usize>,
) -> (i32, Vec<(u32, u32, u32, u32)>) {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let cap = output_capacity.unwrap_or(OUT_RECORDS_OFF + OUT_RECORDS_LEN);
    // Input writes and result reads are scoped so that no `&mut` of the arena
    // outlives the ABI call, which creates its own exclusive view.
    let status = unsafe {
        let a = arena_bytes();
        // Wipe the arena so a stale record can never masquerade as a result.
        for b in a.iter_mut() {
            *b = 0;
        }
        a[IN_CALL_MODES_OFF..IN_CALL_MODES_OFF + modes.len()].copy_from_slice(modes);
        for (i, v) in offsets.iter().enumerate() {
            a[IN_ACCESS_OFFSETS_OFF + i * 4..IN_ACCESS_OFFSETS_OFF + i * 4 + 4]
                .copy_from_slice(&v.to_le_bytes());
        }
        for (i, v) in rids.iter().enumerate() {
            a[IN_RESOURCE_IDS_OFF + i * 4..IN_RESOURCE_IDS_OFF + i * 4 + 4]
                .copy_from_slice(&v.to_le_bytes());
        }
        a[IN_ACCESS_MODES_OFF..IN_ACCESS_MODES_OFF + amodes.len()].copy_from_slice(amodes);
        let base = arena_base() as u32;
        effectplan_plan(
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
            cap as u32,
        )
    };
    if status != OK {
        return (status, Vec::new());
    }
    unsafe {
        let a = arena_bytes();
        let at = |i: usize| OUT_BASE_OFF + i * 4;
        let read = |i: usize| u32::from_le_bytes(a[at(i)..at(i) + 4].try_into().unwrap());
        // Header contract: abi echo, call count, edge count, reserved.
        assert_eq!(read(0), ABI_VERSION, "header must echo the ABI revision");
        assert_eq!(read(1), modes.len() as u32, "header call count");
        let edge_count = read(2);
        assert_eq!(read(3), 0, "header reserved word");
        let mut out = Vec::new();
        let mut i = 0u32;
        while i < edge_count {
            let at = OUT_BASE_OFF + OUT_RECORDS_OFF + (i as usize) * EDGE_BYTES;
            out.push((
                u32::from_le_bytes(a[at..at + 4].try_into().unwrap()),
                u32::from_le_bytes(a[at + 4..at + 8].try_into().unwrap()),
                u32::from_le_bytes(a[at + 8..at + 12].try_into().unwrap()),
                u32::from_le_bytes(a[at + 12..at + 16].try_into().unwrap()),
            ));
            i += 1;
        }
        (OK, out)
    }
}

/// Convenience for the common case: `resource_count` derived from the ids.
fn plan(modes: &[u8], offsets: &[u32], rids: &[u32], amodes: &[u8]) -> Vec<(u32, u32, u32, u32)> {
    let rc = rids.iter().copied().max().unwrap_or(0) as usize + 1;
    plan_rc(modes, offsets, rids, amodes, rc)
}

fn plan_rc(
    modes: &[u8],
    offsets: &[u32],
    rids: &[u32],
    amodes: &[u8],
    resource_count: usize,
) -> Vec<(u32, u32, u32, u32)> {
    let (status, edges) = run_batch(modes, offsets, rids, amodes, resource_count, None);
    assert_eq!(status, OK, "unexpected engine status");
    edges
}

// ---------------------------------------------------------------------------
// 1. Read / read
// ---------------------------------------------------------------------------

#[test]
fn read_read_is_independent() {
    let edges = plan(
        &[MODE_RESOURCES, MODE_RESOURCES],
        &[0, 1, 2],
        &[1, 1],
        &[ACCESS_READ, ACCESS_READ],
    );
    assert!(
        edges.is_empty(),
        "read/read must not create a dependency: {edges:?}"
    );
}

#[test]
fn many_readers_of_one_resource_stay_independent() {
    let modes = [MODE_RESOURCES; 6];
    let offsets: Vec<u32> = (0..7).map(|i| i as u32).collect();
    let rids = vec![1; 6];
    let amodes = vec![ACCESS_READ; 6];
    assert!(plan(&modes, &offsets, &rids, &amodes).is_empty());
}

// ---------------------------------------------------------------------------
// 2. Read / write
// ---------------------------------------------------------------------------

#[test]
fn read_then_write_same_resource_orders() {
    let edges = plan(
        &[MODE_RESOURCES, MODE_RESOURCES],
        &[0, 1, 2],
        &[1, 1],
        &[ACCESS_READ, ACCESS_WRITE],
    );
    assert_eq!(edges, vec![e(0, 1, 1, REASON_READ_WRITE)]);
}

#[test]
fn write_drains_every_pending_reader() {
    let edges = plan(
        &[MODE_RESOURCES; 3],
        &[0, 1, 2, 3],
        &[1, 1, 1],
        &[ACCESS_READ, ACCESS_READ, ACCESS_WRITE],
    );
    assert_eq!(
        edges,
        vec![e(0, 2, 1, REASON_READ_WRITE), e(1, 2, 1, REASON_READ_WRITE)]
    );
}

// ---------------------------------------------------------------------------
// 3. Write / read
// ---------------------------------------------------------------------------

#[test]
fn write_then_read_same_resource_orders() {
    let edges = plan(
        &[MODE_RESOURCES, MODE_RESOURCES],
        &[0, 1, 2],
        &[1, 1],
        &[ACCESS_WRITE, ACCESS_READ],
    );
    assert_eq!(edges, vec![e(0, 1, 1, REASON_WRITE_READ)]);
}

// ---------------------------------------------------------------------------
// 4. Write / write
// ---------------------------------------------------------------------------

#[test]
fn write_then_write_same_resource_orders() {
    let edges = plan(
        &[MODE_RESOURCES, MODE_RESOURCES],
        &[0, 1, 2],
        &[1, 1],
        &[ACCESS_WRITE, ACCESS_WRITE],
    );
    assert_eq!(edges, vec![e(0, 1, 1, REASON_WRITE_WRITE)]);
}

#[test]
fn write_write_chain_is_transitively_ordered() {
    let edges = plan(
        &[MODE_RESOURCES; 3],
        &[0, 1, 2, 3],
        &[1, 1, 1],
        &[ACCESS_WRITE, ACCESS_WRITE, ACCESS_WRITE],
    );
    assert_eq!(
        edges,
        vec![
            e(0, 1, 1, REASON_WRITE_WRITE),
            e(1, 2, 1, REASON_WRITE_WRITE)
        ]
    );
}

#[test]
fn reader_set_clears_after_a_write() {
    // read, write, write: the second write must depend on the first writer only,
    // not on the reader that was already drained.
    let edges = plan(
        &[MODE_RESOURCES; 3],
        &[0, 1, 2, 3],
        &[1, 1, 1],
        &[ACCESS_READ, ACCESS_WRITE, ACCESS_WRITE],
    );
    assert_eq!(
        edges,
        vec![
            e(0, 1, 1, REASON_READ_WRITE),
            e(1, 2, 1, REASON_WRITE_WRITE)
        ]
    );
}

// ---------------------------------------------------------------------------
// 5. Different resources
// ---------------------------------------------------------------------------

#[test]
fn distinct_resources_do_not_interfere() {
    let edges = plan(
        &[MODE_RESOURCES, MODE_RESOURCES],
        &[0, 1, 2],
        &[1, 2],
        &[ACCESS_WRITE, ACCESS_READ],
    );
    assert!(
        edges.is_empty(),
        "different resources must stay independent: {edges:?}"
    );
}

#[test]
fn write_read_on_different_resources_is_independent() {
    let edges = plan(
        &[MODE_RESOURCES, MODE_RESOURCES],
        &[0, 1, 2],
        &[1, 2],
        &[ACCESS_WRITE, ACCESS_WRITE],
    );
    assert!(edges.is_empty());
}

// ---------------------------------------------------------------------------
// 6. Multiple resources
// ---------------------------------------------------------------------------

#[test]
fn multiple_resources_produce_multiple_edges() {
    // 0: read R1   1: write R1   2: read R2   3: write R2
    let edges = plan(
        &[MODE_RESOURCES; 4],
        &[0, 1, 2, 3, 4],
        &[1, 1, 2, 2],
        &[ACCESS_READ, ACCESS_WRITE, ACCESS_READ, ACCESS_WRITE],
    );
    assert_eq!(
        edges,
        vec![e(0, 1, 1, REASON_READ_WRITE), e(2, 3, 2, REASON_READ_WRITE)]
    );
}

#[test]
fn one_call_touching_two_resources_keeps_both_conflicts() {
    // 0: write R1        1: read R1 + read R2        2: write R2
    let edges = plan(
        &[MODE_RESOURCES; 3],
        &[0, 1, 3, 4],
        &[1, 1, 2, 2],
        &[ACCESS_WRITE, ACCESS_READ, ACCESS_READ, ACCESS_WRITE],
    );
    assert_eq!(
        edges,
        vec![e(0, 1, 1, REASON_WRITE_READ), e(1, 2, 2, REASON_READ_WRITE)]
    );
}

#[test]
fn edges_are_sorted_by_predecessor_within_a_successor() {
    // 0: write R2, 1: write R1, 2: read R1 + read R2 -> successor 2 has preds 0 and 1
    let edges = plan(
        &[MODE_RESOURCES; 3],
        &[0, 1, 2, 4],
        &[2, 1, 1, 2],
        &[ACCESS_WRITE, ACCESS_WRITE, ACCESS_READ, ACCESS_READ],
    );
    assert_eq!(
        edges,
        vec![e(0, 2, 2, REASON_WRITE_READ), e(1, 2, 1, REASON_WRITE_READ)]
    );
}

// ---------------------------------------------------------------------------
// 7. Empty input
// ---------------------------------------------------------------------------

#[test]
fn empty_batch_is_ok_with_no_edges() {
    let (status, edges) = run_batch(&[], &[0], &[], &[], 1, None);
    assert_eq!(status, OK);
    assert!(edges.is_empty());
}

#[test]
fn pure_operations_are_mutually_independent() {
    let edges = plan(&[MODE_PURE, MODE_PURE, MODE_PURE], &[0, 0, 0, 0], &[], &[]);
    assert!(edges.is_empty(), "pure calls declare nothing: {edges:?}");
}

#[test]
fn accesses_declared_by_a_pure_call_are_still_honoured() {
    // The engine takes the packed contract at face value; the TypeScript layer
    // is what forbids pure calls from declaring accesses.
    let edges = plan(
        &[MODE_PURE, MODE_PURE],
        &[0, 1, 2],
        &[1, 1],
        &[ACCESS_WRITE, ACCESS_READ],
    );
    assert_eq!(edges, vec![e(0, 1, 1, REASON_WRITE_READ)]);
}

// ---------------------------------------------------------------------------
// 8. Invalid modes
// ---------------------------------------------------------------------------

#[test]
fn rejects_unknown_call_mode() {
    let (status, _) = run_batch(&[4], &[0, 0], &[], &[], 1, None);
    assert_eq!(status, ERR_CALL_MODE);
}

#[test]
fn rejects_call_mode_255() {
    let (status, _) = run_batch(&[255], &[0, 0], &[], &[], 1, None);
    assert_eq!(status, ERR_CALL_MODE);
}

#[test]
fn rejects_invalid_access_mode() {
    let (status, _) = run_batch(
        &[MODE_RESOURCES, MODE_RESOURCES],
        &[0, 1, 2],
        &[1, 1],
        &[ACCESS_READ, 7],
        2,
        None,
    );
    assert_eq!(status, ERR_ACCESS_MODE);
}

// ---------------------------------------------------------------------------
// 9. Invalid offsets
// ---------------------------------------------------------------------------

#[test]
fn rejects_non_monotonic_offsets() {
    let (status, _) = run_batch(
        &[MODE_RESOURCES; 2],
        &[0, 2, 1],
        &[1, 1],
        &[ACCESS_READ, ACCESS_READ],
        2,
        None,
    );
    assert_eq!(status, ERR_OFFSET_MONOTONIC);
}

#[test]
fn rejects_non_zero_initial_offset() {
    let (status, _) = run_batch(&[MODE_RESOURCES], &[1, 1], &[1], &[ACCESS_READ], 2, None);
    assert_eq!(status, ERR_OFFSET_INITIAL);
}

#[test]
fn rejects_offset_terminator_mismatch() {
    // One declared access, but the offsets array never reaches it.
    let (status, _) = run_batch(&[MODE_RESOURCES], &[0, 0], &[1], &[ACCESS_READ], 2, None);
    assert_eq!(status, ERR_OFFSET_TERMINAL);
}

#[test]
fn rejects_offsets_beyond_access_count() {
    let (status, _) = run_batch(
        &[MODE_RESOURCES; 2],
        &[0, 5, 9],
        &[1, 1],
        &[ACCESS_READ, ACCESS_READ],
        2,
        None,
    );
    assert_eq!(status, ERR_OFFSET_MONOTONIC);
}

#[test]
fn rejects_dangling_accesses_with_zero_calls() {
    let (status, _) = run_batch(&[], &[0], &[1], &[ACCESS_READ], 2, None);
    assert_eq!(status, ERR_OFFSET_TERMINAL);
}

// ---------------------------------------------------------------------------
// 10. Invalid resource indices
// ---------------------------------------------------------------------------

#[test]
fn rejects_resource_index_zero() {
    // Index 0 is the internal barrier and must never be host-supplied.
    let (status, _) = run_batch(&[MODE_RESOURCES], &[0, 1], &[0], &[ACCESS_READ], 2, None);
    assert_eq!(status, ERR_RESOURCE_INDEX);
}

#[test]
fn rejects_resource_index_at_or_above_count() {
    let (status, _) = run_batch(&[MODE_RESOURCES], &[0, 1], &[3], &[ACCESS_READ], 3, None);
    assert_eq!(status, ERR_RESOURCE_INDEX);
}

#[test]
fn rejects_out_of_range_slots_and_bad_abi() {
    let (status, _) = run_batch(&[MODE_RESOURCES], &[0, 1], &[1], &[ACCESS_READ], 0, None);
    assert_eq!(status, ERR_RESOURCE_LIMIT);
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    unsafe {
        let base = arena_base() as u32;
        let a = arena_bytes();
        for b in a.iter_mut() {
            *b = 0;
        }
        let s = effectplan_plan(
            ABI_VERSION + 1,
            0,
            0,
            1,
            base + IN_CALL_MODES_OFF as u32,
            base + IN_ACCESS_OFFSETS_OFF as u32,
            base + IN_RESOURCE_IDS_OFF as u32,
            base + IN_ACCESS_MODES_OFF as u32,
            base + WS_BASE_OFF as u32,
            WORKSPACE_REGION_LEN as u32,
            base + OUT_BASE_OFF as u32,
            OUTPUT_REGION_LEN as u32,
        );
        assert_eq!(s, ERR_ABI_VERSION);
    }
}

#[test]
fn rejects_non_canonical_pointers_and_short_buffers() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    unsafe {
        let base = arena_base() as u32;
        // A pointer one element into the input region is not the canonical base.
        let s = effectplan_plan(
            ABI_VERSION,
            0,
            0,
            1,
            base + IN_CALL_MODES_OFF as u32,
            base + IN_ACCESS_OFFSETS_OFF as u32,
            base + IN_RESOURCE_IDS_OFF as u32,
            base + IN_ACCESS_MODES_OFF as u32 + 4,
            base + WS_BASE_OFF as u32,
            WORKSPACE_REGION_LEN as u32,
            base + OUT_BASE_OFF as u32,
            OUTPUT_REGION_LEN as u32,
        );
        assert_eq!(s, ERR_LAYOUT);

        // Workspace too small for the declared layout.
        let s = effectplan_plan(
            ABI_VERSION,
            0,
            0,
            1,
            base + IN_CALL_MODES_OFF as u32,
            base + IN_ACCESS_OFFSETS_OFF as u32,
            base + IN_RESOURCE_IDS_OFF as u32,
            base + IN_ACCESS_MODES_OFF as u32,
            base + WS_BASE_OFF as u32,
            16,
            base + OUT_BASE_OFF as u32,
            OUTPUT_REGION_LEN as u32,
        );
        assert_eq!(s, ERR_WORKSPACE_CAP);

        // Output capacity beyond the canonical region would alias nothing, but
        // is still refused because the arena cannot hold it.
        let s = effectplan_plan(
            ABI_VERSION,
            0,
            0,
            1,
            base + IN_CALL_MODES_OFF as u32,
            base + IN_ACCESS_OFFSETS_OFF as u32,
            base + IN_RESOURCE_IDS_OFF as u32,
            base + IN_ACCESS_MODES_OFF as u32,
            base + WS_BASE_OFF as u32,
            WORKSPACE_REGION_LEN as u32,
            base + OUT_BASE_OFF as u32,
            OUTPUT_REGION_LEN as u32 + 16,
        );
        assert_eq!(s, ERR_OUTPUT_CAP);

        // A pointer far outside the arena. Off-target the arena base is 0, so a
        // below-base address (which yields ERR_RANGE through the checked
        // subtraction) cannot be represented; the wasm integration suite covers
        // that branch with the real base. Here the value converts but is not a
        // canonical region base, so it must still be refused.
        let s = effectplan_plan(
            ABI_VERSION,
            0,
            0,
            1,
            base + IN_CALL_MODES_OFF as u32,
            base + IN_ACCESS_OFFSETS_OFF as u32,
            u32::MAX,
            base + IN_ACCESS_MODES_OFF as u32,
            base + WS_BASE_OFF as u32,
            WORKSPACE_REGION_LEN as u32,
            base + OUT_BASE_OFF as u32,
            OUTPUT_REGION_LEN as u32,
        );
        assert_eq!(s, ERR_LAYOUT);
    }
}

// ---------------------------------------------------------------------------
// 11. Exclusive operations
// ---------------------------------------------------------------------------

#[test]
fn exclusive_waits_for_earlier_ordinary_operations() {
    let edges = plan_rc(
        &[MODE_RESOURCES, MODE_EXCLUSIVE],
        &[0, 1, 1],
        &[1],
        &[ACCESS_READ],
        2,
    );
    assert_eq!(
        edges,
        vec![e(0, 1, 0, REASON_READ_WRITE + REASON_BARRIER_OFFSET)]
    );
}

#[test]
fn ordinary_operation_waits_for_an_earlier_exclusive_one() {
    let edges = plan_rc(
        &[MODE_EXCLUSIVE, MODE_RESOURCES],
        &[0, 0, 1],
        &[1],
        &[ACCESS_READ],
        2,
    );
    assert_eq!(
        edges,
        vec![e(0, 1, 0, REASON_WRITE_READ + REASON_BARRIER_OFFSET)]
    );
}

#[test]
fn exclusive_operations_are_ordered_against_each_other() {
    let edges = plan_rc(&[MODE_EXCLUSIVE, MODE_EXCLUSIVE], &[0, 0, 0], &[], &[], 1);
    assert_eq!(
        edges,
        vec![e(0, 1, 0, REASON_WRITE_WRITE + REASON_BARRIER_OFFSET)]
    );
}

#[test]
fn exclusive_blocks_a_conflict_free_resource_pair() {
    // Distinct resources would normally be independent; the barrier changes that.
    // 0 writes R1, 1 is exclusive, 2 writes R2: 1 waits for 0, and 2 waits for 1.
    let edges = plan_rc(
        &[MODE_RESOURCES, MODE_EXCLUSIVE, MODE_RESOURCES],
        &[0, 1, 1, 2],
        &[1, 2],
        &[ACCESS_WRITE, ACCESS_WRITE],
        3,
    );
    assert_eq!(
        edges,
        vec![
            e(0, 1, 0, REASON_READ_WRITE + REASON_BARRIER_OFFSET),
            e(1, 2, 0, REASON_WRITE_READ + REASON_BARRIER_OFFSET),
        ]
    );
}

// ---------------------------------------------------------------------------
// 12. Unknown effects
// ---------------------------------------------------------------------------

#[test]
fn unknown_effect_behaves_as_a_global_barrier_write() {
    let edges = plan_rc(
        &[MODE_RESOURCES, MODE_UNKNOWN],
        &[0, 1, 2],
        &[1, 1],
        &[ACCESS_READ, ACCESS_READ],
        2,
    );
    // Barrier: 0 is a pending reader of 1. R1 conflict is folded into the same
    // predecessor, and deduplication keeps one record per (pred,res,reason).
    assert!(
        edges.contains(&e(0, 1, 0, REASON_READ_WRITE + REASON_BARRIER_OFFSET)),
        "unknown must be ordered after earlier ordinary work: {edges:?}"
    );
}

#[test]
fn unknown_effect_is_never_treated_as_pure() {
    let edges = plan_rc(&[MODE_UNKNOWN, MODE_PURE], &[0, 0, 0], &[], &[], 1);
    assert_eq!(
        edges.len(),
        1,
        "unknown must not be silently parallel: {edges:?}"
    );
    assert_eq!(
        edges[0],
        e(0, 1, 0, REASON_WRITE_READ + REASON_BARRIER_OFFSET)
    );
}

// ---------------------------------------------------------------------------
// 13. Global barriers
// ---------------------------------------------------------------------------

#[test]
fn barrier_orders_a_resource_conflict_across_an_unknown_call() {
    // 0: read R1, 1: unknown, 2: write R1
    let edges = plan_rc(
        &[MODE_RESOURCES, MODE_UNKNOWN, MODE_RESOURCES],
        &[0, 1, 1, 2],
        &[1, 1],
        &[ACCESS_READ, ACCESS_WRITE],
        2,
    );
    assert_eq!(
        edges,
        vec![
            e(0, 1, 0, REASON_READ_WRITE + REASON_BARRIER_OFFSET),
            e(0, 2, 1, REASON_READ_WRITE),
            e(1, 2, 0, REASON_WRITE_READ + REASON_BARRIER_OFFSET),
        ],
        "records are ordered by successor, then predecessor"
    );
}

#[test]
fn barrier_does_not_order_ordinary_reads_of_the_same_resource() {
    let edges = plan_rc(
        &[MODE_RESOURCES, MODE_RESOURCES, MODE_RESOURCES],
        &[0, 1, 2, 3],
        &[1, 1, 1],
        &[ACCESS_READ, ACCESS_READ, ACCESS_READ],
        2,
    );
    assert!(edges.is_empty());
}

// ---------------------------------------------------------------------------
// 14. Self-dependency prevention
// ---------------------------------------------------------------------------

#[test]
fn never_emits_a_self_dependency_for_read_then_write() {
    // One call declaring both read and write on R1 (the TypeScript layer would
    // normalise this; the engine must still refuse to order a call against itself).
    let edges = plan_rc(
        &[MODE_RESOURCES],
        &[0, 2],
        &[1, 1],
        &[ACCESS_READ, ACCESS_WRITE],
        2,
    );
    assert!(edges.is_empty(), "self-dependency emitted: {edges:?}");
}

#[test]
fn never_emits_a_self_dependency_for_write_then_read() {
    let edges = plan_rc(
        &[MODE_RESOURCES],
        &[0, 2],
        &[1, 1],
        &[ACCESS_WRITE, ACCESS_READ],
        2,
    );
    assert!(edges.is_empty(), "self-dependency emitted: {edges:?}");
}

#[test]
fn never_emits_a_self_dependency_for_duplicate_reads() {
    let edges = plan_rc(
        &[MODE_RESOURCES, MODE_RESOURCES],
        &[0, 2, 3],
        &[1, 1, 1],
        &[ACCESS_READ, ACCESS_READ, ACCESS_WRITE],
        2,
    );
    assert!(
        edges.iter().all(|(pred, succ, _, _)| pred < succ),
        "no record may reference itself: {edges:?}"
    );
    assert_eq!(
        edges,
        vec![e(0, 1, 1, REASON_READ_WRITE)],
        "the two identical reader edges must be deduplicated"
    );
}

#[test]
fn no_record_ever_references_itself() {
    let edges = plan_rc(
        &[MODE_RESOURCES; 4],
        &[0, 2, 4, 5, 6],
        &[1, 1, 2, 2, 1, 1],
        &[
            ACCESS_WRITE,
            ACCESS_READ,
            ACCESS_WRITE,
            ACCESS_READ,
            ACCESS_WRITE,
            ACCESS_READ,
        ],
        3,
    );
    for (pred, succ, _, _) in edges {
        assert!(pred < succ, "edge must point forward: {pred} -> {succ}");
    }
}

// ---------------------------------------------------------------------------
// 15. Output capacity overflow
// ---------------------------------------------------------------------------

#[test]
fn reports_capacity_overflow_instead_of_truncating() {
    // Two edges are required; only one record of capacity is offered.
    let (status, edges) = run_batch(
        &[MODE_RESOURCES; 3],
        &[0, 1, 2, 3],
        &[1, 1, 1],
        &[ACCESS_WRITE, ACCESS_WRITE, ACCESS_WRITE],
        2,
        Some(OUT_HEADER_BYTES + EDGE_BYTES),
    );
    assert_eq!(status, ERR_OUTPUT_CAP, "edges: {edges:?}");
    assert!(edges.is_empty());
}

#[test]
fn accepts_output_capacity_that_fits_exactly() {
    let (status, edges) = run_batch(
        &[MODE_RESOURCES; 2],
        &[0, 1, 2],
        &[1, 1],
        &[ACCESS_WRITE, ACCESS_WRITE],
        2,
        Some(OUT_HEADER_BYTES + 2 * EDGE_BYTES),
    );
    assert_eq!(status, OK);
    assert_eq!(edges.len(), 1);
}

#[test]
fn header_and_record_count_agree() {
    let (status, edges) = run_batch(
        &[MODE_RESOURCES; 4],
        &[0, 1, 2, 3, 4],
        &[1, 1, 1, 1],
        &[ACCESS_WRITE, ACCESS_READ, ACCESS_WRITE, ACCESS_READ],
        2,
        None,
    );
    assert_eq!(status, OK);
    assert_eq!(
        edges,
        vec![
            e(0, 1, 1, REASON_WRITE_READ),
            e(0, 2, 1, REASON_WRITE_WRITE),
            e(1, 2, 1, REASON_READ_WRITE),
            e(2, 3, 1, REASON_WRITE_READ),
        ],
        "header count and decoded records must agree"
    );
}

// ---------------------------------------------------------------------------
// 16. Input size overflow
// ---------------------------------------------------------------------------

#[test]
fn rejects_call_count_above_the_limit() {
    let modes = vec![MODE_PURE; MAX_CALLS + 1];
    let offsets = vec![0u32; MAX_CALLS + 2];
    let (status, _) = run_batch(&modes, &offsets, &[], &[], 1, None);
    assert_eq!(status, ERR_CALL_LIMIT);
}

#[test]
fn rejects_access_count_above_the_limit() {
    let offsets = vec![0u32, MAX_ACCESS as u32 + 1];
    let modes = [MODE_RESOURCES];
    let rids = vec![1; MAX_ACCESS + 1];
    let amodes = vec![ACCESS_READ; MAX_ACCESS + 1];
    let (status, _) = run_batch(&modes, &offsets, &rids, &amodes, 2, None);
    assert_eq!(status, ERR_ACCESS_LIMIT);
}

#[test]
fn rejects_resource_count_above_the_slot_table() {
    let (status, _) = run_batch(
        &[MODE_RESOURCES],
        &[0, 1],
        &[1],
        &[ACCESS_READ],
        RES_SLOTS + 1,
        None,
    );
    assert_eq!(status, ERR_RESOURCE_LIMIT);
}

#[test]
fn accepts_the_maximum_declared_batch() {
    // Full prototype ceiling: 128 calls, 1024 accesses, 1024 resources. Each
    // resource is re-touched every 256 accesses so conflicts span calls.
    let modes = vec![MODE_RESOURCES; MAX_CALLS];
    let mut offsets = Vec::with_capacity(MAX_CALLS + 1);
    let mut rids = Vec::with_capacity(MAX_ACCESS);
    let mut amodes = Vec::with_capacity(MAX_ACCESS);
    for i in 0..=MAX_CALLS {
        offsets.push((i * MAX_ACCESS / MAX_CALLS) as u32);
    }
    for k in 0..MAX_ACCESS {
        rids.push((k % 256) as u32 + 1);
        amodes.push(if k % 2 == 0 {
            ACCESS_READ
        } else {
            ACCESS_WRITE
        });
    }
    let (status, edges) = run_batch(&modes, &offsets, &rids, &amodes, MAX_RESOURCES + 1, None);
    assert_eq!(status, OK);
    assert!(!edges.is_empty(), "a dense batch must produce ordering");
    assert!(
        edges.len() <= MAX_EDGES,
        "edge bound violated: {} > {}",
        edges.len(),
        MAX_EDGES
    );
    for (pred, succ, _, _) in &edges {
        assert!(pred < succ);
    }
}

// ---------------------------------------------------------------------------
// Determinism and layout introspection
// ---------------------------------------------------------------------------

#[test]
fn identical_input_produces_identical_bytes() {
    let modes = [
        MODE_RESOURCES,
        MODE_UNKNOWN,
        MODE_RESOURCES,
        MODE_EXCLUSIVE,
        MODE_RESOURCES,
    ];
    let offsets = [0u32, 2, 2, 3, 5, 6];
    let rids = [1u32, 2, 1, 2, 3, 3];
    let amodes = [
        ACCESS_WRITE,
        ACCESS_READ,
        ACCESS_READ,
        ACCESS_WRITE,
        ACCESS_WRITE,
        ACCESS_READ,
    ];
    let first = plan(&modes, &offsets, &rids, &amodes);
    let second = plan(&modes, &offsets, &rids, &amodes);
    assert_eq!(first, second);
    // Encoding order within a successor is ascending by predecessor.
    let mut succs: Vec<u32> = first.iter().map(|(_, s, _, _)| *s).collect();
    succs.dedup();
    let mut sorted = succs.clone();
    sorted.sort_unstable();
    assert_eq!(succs, sorted);
}

#[test]
fn edge_bound_holds_for_a_barrier_heavy_batch() {
    // Alternating exclusive and ordinary calls maximises barrier edges.
    let mut modes = Vec::new();
    for i in 0..64 {
        modes.push(if i % 2 == 0 {
            MODE_EXCLUSIVE
        } else {
            MODE_RESOURCES
        });
    }
    let offsets = vec![0u32; 65];
    let (status, edges) = run_batch(&modes, &offsets, &[], &[], 1, None);
    assert_eq!(status, OK);
    let t = modes.len();
    assert!(
        edges.len() <= 2 * t,
        "2*T bound violated: {} > {}",
        edges.len(),
        2 * t
    );
}

#[test]
fn info_reports_the_live_layout() {
    assert_eq!(effectplan_info(0), ABI_VERSION);
    assert_eq!(effectplan_info(2), ARENA_BYTES as u32);
    assert_eq!(effectplan_info(4), INPUT_REGION_LEN as u32);
    assert_eq!(effectplan_info(6), WORKSPACE_REGION_LEN as u32);
    assert_eq!(effectplan_info(8), OUTPUT_REGION_LEN as u32);
    assert_eq!(effectplan_info(9), MAX_CALLS as u32);
    assert_eq!(effectplan_info(13), MAX_EDGES as u32);
    assert_eq!(effectplan_info(100), SENTINEL);
    // Regions tile the arena exactly, modulo final padding.
    assert_eq!(
        effectplan_info(7) - effectplan_info(5),
        WORKSPACE_REGION_LEN as u32
    );
    assert!(ARENA_BYTES >= OUT_BASE_OFF + OUTPUT_REGION_LEN);
}
