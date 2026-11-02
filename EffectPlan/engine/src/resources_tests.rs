//! Tests for capacity-resource validation (Phase 5, M3 gate).
//!
//! The claim under test is that the engine can tell a caller *before scheduling*
//! whether its declared demands are satisfiable at all — and that it refuses,
//! rather than guesses, when the declarations are malformed. Every test goes
//! through `effectplan_resource_check` with the real arena and real pointers,
//! because the interesting failures are pointer aliasing and hostile counts, not
//! Rust logic.
//!
//! Each fixture run holds `ARENA_LOCK` from the moment it writes its declarations
//! until it has read the diagnostics back. Tests run in parallel and share one
//! static arena, so releasing the lock between the call and the read would let
//! another test overwrite the very bytes being asserted.

use super::*;
use crate::resources::{
    CAPACITIES_REGION_LEN, DEMAND_OFFSETS_REGION_LEN, DEMAND_RECORDS_REGION_LEN,
    DEMAND_RECORD_BYTES, DIAGNOSTICS_REGION_LEN, DIAG_DUPLICATE_DEMAND, DIAG_INSUFFICIENT_CAPACITY,
    DIAG_RANGE, DIAG_RESOURCE_UNDECLARED, DIAG_ZERO_CAPACITY, DIAG_ZERO_DEMAND, D_CALL, D_KIND,
    D_RESOURCE, D_VALUE, H_COUNT, H_REVISION, H_STATUS, MAX_CAPACITY, MAX_CAPACITY_RESOURCES,
    MAX_DEMAND_RECORDS, MAX_RESOURCE_DIAGNOSTICS, RESOURCE_REVISION,
};
use crate::ARENA_LOCK as LOCK;
use crate::{
    CAPACITIES_BASE_OFF, DEMAND_OFFSETS_BASE_OFF, DEMAND_RECORDS_BASE_OFF,
    RESOURCE_DIAGNOSTICS_BASE_OFF,
};

/// The arena, reached the same way every other test module reaches it.
unsafe fn arena_bytes() -> &'static mut [u8] {
    core::slice::from_raw_parts_mut(core::ptr::addr_of_mut!(ARENA) as *mut u8, ARENA_BYTES)
}

struct Fixture {
    capacities: Vec<u32>,
    /// One entry per call: the `(resource, amount)` pairs it demands.
    demands: Vec<Vec<(u32, u32)>>,
}

/// Everything the engine published, captured while the lock is still held.
struct Outcome {
    status: i32,
    count: u32,
    records: Vec<[u32; 4]>,
    published: i32,
    revision: u32,
}

impl Fixture {
    fn new(capacities: &[u32], demands: &[&[(u32, u32)]]) -> Fixture {
        Fixture {
            capacities: capacities.to_vec(),
            demands: demands.iter().map(|d| d.to_vec()).collect(),
        }
    }

    fn dense(capacities: Vec<u32>, demands: Vec<Vec<(u32, u32)>>) -> Fixture {
        Fixture {
            capacities,
            demands,
        }
    }

    fn demand_count(&self) -> usize {
        self.demands.iter().map(|d| d.len()).sum()
    }

    fn run(&self) -> Outcome {
        let declared = self.capacities.len();
        self.run_with(declared, self.demand_count())
    }

    /// `resource_count` and `demand_count` are passed separately from the bytes so
    /// a test can name counts the arena does not support.
    fn run_with(&self, resource_count: usize, demand_count: usize) -> Outcome {
        let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        // The lock is not reentrant, so everything below this point assumes it is
        // already held rather than taking it again.
        // The lock is not reentrant, so everything below this point assumes it is
        // already held rather than taking it again.
        unsafe { self.run_locked(resource_count, demand_count) }
    }

    /// Write the declarations, call the entry point and read the published
    /// diagnostics — all while `ARENA_LOCK` is held by the caller.
    ///
    /// # Safety
    /// The caller must hold `ARENA_LOCK` for the whole span of this call, and no
    /// other test may be touching the arena.
    unsafe fn run_locked(&self, resource_count: usize, demand_count: usize) -> Outcome {
        let a = arena_bytes();
        for byte in a.iter_mut() {
            *byte = 0;
        }
        for (index, capacity) in self.capacities.iter().enumerate() {
            let at = CAPACITIES_BASE_OFF + index * 4;
            a[at..at + 4].copy_from_slice(&capacity.to_le_bytes());
        }
        let mut offsets = vec![0u32];
        let mut records: Vec<(u32, u32)> = Vec::new();
        for demand in &self.demands {
            for pair in demand {
                records.push(*pair);
            }
            offsets.push(records.len() as u32);
        }
        for (index, offset) in offsets.iter().enumerate() {
            let at = DEMAND_OFFSETS_BASE_OFF + index * 4;
            a[at..at + 4].copy_from_slice(&offset.to_le_bytes());
        }
        for (index, (resource, amount)) in records.iter().enumerate() {
            let at = DEMAND_RECORDS_BASE_OFF + index * DEMAND_RECORD_BYTES;
            a[at..at + 4].copy_from_slice(&resource.to_le_bytes());
            a[at + 4..at + 8].copy_from_slice(&amount.to_le_bytes());
        }
        let base = arena_base() as u32;
        let status = effectplan_resource_check(
            ABI_VERSION,
            self.demands.len() as u32,
            resource_count as u32,
            demand_count as u32,
            base + CAPACITIES_BASE_OFF as u32,
            CAPACITIES_REGION_LEN as u32,
            base + DEMAND_OFFSETS_BASE_OFF as u32,
            DEMAND_OFFSETS_REGION_LEN as u32,
            base + DEMAND_RECORDS_BASE_OFF as u32,
            DEMAND_RECORDS_REGION_LEN as u32,
            base + RESOURCE_DIAGNOSTICS_BASE_OFF as u32,
            DIAGNOSTICS_REGION_LEN as u32,
        );
        let count = rd_u32(arena_bytes(), RESOURCE_DIAGNOSTICS_BASE_OFF + H_COUNT);
        let published = rd_u32(arena_bytes(), RESOURCE_DIAGNOSTICS_BASE_OFF + H_STATUS) as i32;
        let revision = rd_u32(arena_bytes(), RESOURCE_DIAGNOSTICS_BASE_OFF + H_REVISION);
        let mut read_back = Vec::new();
        for index in 0..count as usize {
            let at = RESOURCE_DIAGNOSTICS_BASE_OFF + 32 + index * 16;
            let b = arena_bytes();
            read_back.push([
                rd_u32(b, at + D_KIND),
                rd_u32(b, at + D_RESOURCE),
                rd_u32(b, at + D_CALL),
                rd_u32(b, at + D_VALUE),
            ]);
        }
        Outcome {
            status,
            count,
            records: read_back,
            published,
            revision,
        }
    }
}

impl Outcome {
    fn kind(&self, index: usize) -> u32 {
        self.records[index][0]
    }
}

#[test]
fn a_batch_with_no_capacity_resources_is_accepted() {
    let fixture = Fixture::new(&[], &[&[], &[]]);
    let out = fixture.run();
    assert_eq!(out.status, OK);
    assert_eq!(out.count, 0);
    assert_eq!(out.revision, RESOURCE_REVISION);
}

#[test]
fn demands_within_capacity_are_accepted() {
    let fixture = Fixture::new(&[10, 4], &[&[(0, 6)], &[(1, 4)]]);
    assert_eq!(fixture.run().status, OK);
}

#[test]
fn a_demand_exactly_equal_to_capacity_fits() {
    let fixture = Fixture::new(&[2], &[&[(0, 2)]]);
    assert_eq!(fixture.run().status, OK);
}

#[test]
fn a_demand_above_capacity_is_infeasible_and_says_which() {
    let fixture = Fixture::new(&[2], &[&[(0, 3)]]);
    let out = fixture.run();
    assert_eq!(out.status, ERR_RESOURCE_CAPACITY);
    assert_eq!(out.count, 1);
    assert_eq!(out.kind(0), DIAG_INSUFFICIENT_CAPACITY);
    assert_eq!(out.records[0][D_RESOURCE / 4], 0, "resource index");
    assert_eq!(out.records[0][D_CALL / 4], 0, "call index");
    assert_eq!(
        out.records[0][D_VALUE / 4],
        3,
        "the demand that cannot be met"
    );
    assert_eq!(
        out.published, out.status,
        "the header echoes the returned status"
    );
}

#[test]
fn zero_capacity_is_refused_as_malformed() {
    let fixture = Fixture::new(&[0], &[&[]]);
    let out = fixture.run();
    assert_eq!(out.status, ERR_RESOURCE_REGION);
    assert_eq!(out.kind(0), DIAG_ZERO_CAPACITY);
}

#[test]
fn zero_demand_is_refused_rather_than_treated_as_free() {
    // An amount of zero would slip through every capacity check and then occupy
    // nothing: the caller means something else, so it is asked again.
    let fixture = Fixture::new(&[4], &[&[(0, 0)]]);
    let out = fixture.run();
    assert_eq!(out.status, ERR_RESOURCE_REGION);
    assert_eq!(out.kind(0), DIAG_ZERO_DEMAND);
}

#[test]
fn a_requirement_naming_an_undeclared_resource_is_refused() {
    let fixture = Fixture::new(&[4], &[&[(5, 1)]]);
    let out = fixture.run();
    assert_eq!(out.status, ERR_RESOURCE_UNDECLARED);
    assert_eq!(out.kind(0), DIAG_RESOURCE_UNDECLARED);
    assert_eq!(
        out.records[0][D_RESOURCE / 4],
        5,
        "the index the caller named"
    );
}

#[test]
fn one_call_cannot_name_the_same_resource_twice() {
    // Two records for one resource in one call would be summed by some readers and
    // maxed by others. The model has one demand per resource per call.
    let fixture = Fixture::new(&[8], &[&[(0, 3), (0, 4)]]);
    let out = fixture.run();
    assert_eq!(out.status, ERR_RESOURCE_REGION);
    assert_eq!(out.kind(0), DIAG_DUPLICATE_DEMAND);
}

#[test]
fn the_same_resource_in_two_calls_is_not_a_duplicate() {
    let fixture = Fixture::new(&[8], &[&[(0, 3)], &[(0, 4)]]);
    assert_eq!(fixture.run().status, OK);
}

#[test]
fn counts_over_the_ceiling_are_refused_before_any_read() {
    let fixture = Fixture::new(&[4], &[&[(0, 1)]]);
    assert_eq!(
        fixture.run_with(MAX_CAPACITY_RESOURCES + 1, 1).status,
        ERR_RESOURCE_REGION
    );
    assert_eq!(
        fixture.run_with(1, MAX_DEMAND_RECORDS + 1).status,
        ERR_RESOURCE_REGION
    );
}

#[test]
fn a_call_count_over_the_ceiling_is_refused_by_the_entry_point() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let base = arena_base() as u32;
    let status = unsafe {
        effectplan_resource_check(
            ABI_VERSION,
            (MAX_CALLS + 1) as u32,
            1,
            1,
            base + CAPACITIES_BASE_OFF as u32,
            CAPACITIES_REGION_LEN as u32,
            base + DEMAND_OFFSETS_BASE_OFF as u32,
            DEMAND_OFFSETS_REGION_LEN as u32,
            base + DEMAND_RECORDS_BASE_OFF as u32,
            DEMAND_RECORDS_REGION_LEN as u32,
            base + RESOURCE_DIAGNOSTICS_BASE_OFF as u32,
            DIAGNOSTICS_REGION_LEN as u32,
        )
    };
    assert_eq!(status, ERR_CALL_LIMIT);
}

#[test]
fn a_wrong_abi_version_is_refused() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let base = arena_base() as u32;
    let status = unsafe {
        effectplan_resource_check(
            ABI_VERSION + 1,
            1,
            1,
            1,
            base + CAPACITIES_BASE_OFF as u32,
            CAPACITIES_REGION_LEN as u32,
            base + DEMAND_OFFSETS_BASE_OFF as u32,
            DEMAND_OFFSETS_REGION_LEN as u32,
            base + DEMAND_RECORDS_BASE_OFF as u32,
            DEMAND_RECORDS_REGION_LEN as u32,
            base + RESOURCE_DIAGNOSTICS_BASE_OFF as u32,
            DIAGNOSTICS_REGION_LEN as u32,
        )
    };
    assert_eq!(status, ERR_ABI_VERSION);
}

#[test]
fn a_pointer_that_only_lands_inside_the_arena_is_refused() {
    // One byte into the capacities region would alias it; each argument must name
    // its region's base exactly, which is the rule every phase has held to.
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let base = arena_base() as u32;
    let status = unsafe {
        effectplan_resource_check(
            ABI_VERSION,
            1,
            1,
            1,
            base + CAPACITIES_BASE_OFF as u32 + 1,
            CAPACITIES_REGION_LEN as u32,
            base + DEMAND_OFFSETS_BASE_OFF as u32,
            DEMAND_OFFSETS_REGION_LEN as u32,
            base + DEMAND_RECORDS_BASE_OFF as u32,
            DEMAND_RECORDS_REGION_LEN as u32,
            base + RESOURCE_DIAGNOSTICS_BASE_OFF as u32,
            DIAGNOSTICS_REGION_LEN as u32,
        )
    };
    assert_eq!(status, ERR_LAYOUT);
}

#[test]
fn a_region_length_that_is_not_the_published_one_is_refused() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let base = arena_base() as u32;
    let status = unsafe {
        effectplan_resource_check(
            ABI_VERSION,
            1,
            1,
            1,
            base + CAPACITIES_BASE_OFF as u32,
            (CAPACITIES_REGION_LEN - 4) as u32,
            base + DEMAND_OFFSETS_BASE_OFF as u32,
            DEMAND_OFFSETS_REGION_LEN as u32,
            base + DEMAND_RECORDS_BASE_OFF as u32,
            DEMAND_RECORDS_REGION_LEN as u32,
            base + RESOURCE_DIAGNOSTICS_BASE_OFF as u32,
            DIAGNOSTICS_REGION_LEN as u32,
        )
    };
    assert_eq!(status, ERR_LAYOUT);
}

#[test]
fn a_capacity_beyond_the_supported_range_is_reported_as_range_not_malformed() {
    let fixture = Fixture::new(&[MAX_CAPACITY + 1], &[&[]]);
    let out = fixture.run();
    assert_eq!(out.status, ERR_RESOURCE_RANGE);
    assert_eq!(out.kind(0), DIAG_RANGE);
}

#[test]
fn a_malformed_offset_vector_is_refused_rather_than_walked() {
    // A terminal bound disagreeing with the declared record count means the scan
    // cannot know where a call's demands end, so it stops rather than guesses.
    let fixture = Fixture::new(&[4], &[&[(0, 1)]]);
    assert_eq!(fixture.run_with(1, 7).status, ERR_RESOURCE_REGION);
}

#[test]
fn the_same_declarations_produce_the_same_result_every_time() {
    let fixture = Fixture::new(&[2, 8], &[&[(0, 3)], &[(1, 4), (0, 1)], &[]]);
    let first = fixture.run();
    let second = fixture.run();
    assert_eq!(first.status, second.status);
    assert_eq!(first.count, second.count);
    assert_eq!(
        first.records, second.records,
        "diagnostics are deterministic"
    );
    assert_eq!(first.published, second.published);
}

#[test]
fn an_infeasible_run_never_leaves_its_diagnostics_for_the_next_caller() {
    let broken = Fixture::new(&[1], &[&[(0, 9)], &[(0, 9)]]);
    let out = broken.run();
    assert_eq!(out.status, ERR_RESOURCE_CAPACITY);
    assert_eq!(out.count, 2, "both problems are reported");

    let clean = Fixture::new(&[9], &[&[(0, 9)]]);
    let out = clean.run();
    assert_eq!(out.status, OK);
    assert_eq!(
        out.count, 0,
        "the region was cleared, not left from the previous run"
    );
    assert!(out.records.is_empty());
    assert_eq!(out.published, OK);
}

#[test]
fn the_diagnostic_list_stops_growing_but_the_status_still_reports() {
    let demands: Vec<Vec<(u32, u32)>> = (0..MAX_CALLS)
        .map(|call| vec![(call as u32 % MAX_CAPACITY_RESOURCES as u32, 9u32)])
        .collect();
    let fixture = Fixture::dense(vec![4u32; MAX_CAPACITY_RESOURCES], demands);
    let out = fixture.run();
    assert_eq!(out.status, ERR_RESOURCE_CAPACITY);
    assert_eq!(
        out.count as usize, MAX_RESOURCE_DIAGNOSTICS,
        "capped, not empty"
    );
    assert_eq!(out.records.len(), MAX_RESOURCE_DIAGNOSTICS);
    assert_eq!(out.published, ERR_RESOURCE_CAPACITY);
}

#[test]
fn every_call_demanding_several_resources_at_the_ceiling_is_accepted() {
    // 128 calls × 8 resources = 1,024 records, exactly MAX_DEMAND_RECORDS.
    let demands: Vec<Vec<(u32, u32)>> = (0..MAX_CALLS)
        .map(|_| (0..8u32).map(|r| (r, 1u32)).collect())
        .collect();
    let fixture = Fixture::dense(vec![MAX_CAPACITY; MAX_CAPACITY_RESOURCES], demands);
    assert_eq!(fixture.demand_count(), MAX_DEMAND_RECORDS);
    let out = fixture.run();
    assert_eq!(out.status, OK);
    assert_eq!(out.count, 0);
}

#[test]
fn hostile_counts_cannot_reach_past_the_regions() {
    // u32::MAX everywhere: refused, never a trap and never a read outside a region.
    let fixture = Fixture::new(&[4], &[&[(0, 1)]]);
    assert_eq!(
        fixture
            .run_with(u32::MAX as usize, u32::MAX as usize)
            .status,
        ERR_RESOURCE_REGION
    );
}

#[test]
fn a_single_demand_does_not_disturb_the_planning_regions_it_does_not_use() {
    // The resource call must not write the graph, levels, timing or the checkpoint
    // record: it answers a different question from the planner's, and keeping the
    // two apart is the architectural point of Phase 5. One guard for the whole
    // sequence, because the arena is shared with every other test.
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let fixture = Fixture::new(&[4], &[&[(0, 2)]]);
    let out = unsafe { fixture.run_locked(1, 1) };
    assert_eq!(out.status, OK);
    unsafe {
        // Graph records, levels, durations, timing and the checkpoint record:
        // every region the planner owns, up to where the resource regions start.
        let a = arena_bytes();
        for at in (OUT_BASE_OFF..CAPACITIES_BASE_OFF).step_by(4) {
            assert_eq!(rd_u32(a, at), 0, "planner region dirty at {at}");
        }
    }
}

#[test]
fn a_later_call_that_fits_does_not_excuse_an_earlier_one_that_cannot() {
    // Two calls, one impossible lane demand: the well-formed second call must not
    // hide the first. This is the ordering the single-call case cannot catch.
    let fixture = Fixture::new(&[2], &[&[(0, 5)], &[(0, 1)]]);
    let out = fixture.run();
    assert_eq!(out.status, ERR_RESOURCE_CAPACITY);
    assert_eq!(out.count, 1, "only the impossible demand is reported");
    assert_eq!(out.records[0][0], crate::resources::DIAG_INSUFFICIENT_CAPACITY);
    assert_eq!(out.records[0][2], 0, "the offending call is the first");
    assert_eq!(out.published, ERR_RESOURCE_CAPACITY);
}
