//! Native benchmark of the Rust core.
//!
//! It drives the same `plan_arena` / `plan_arena_v2` entry points the
//! WebAssembly build exports, with the same workload the TypeScript benchmark
//! generates, so the two numbers describe the same work. Results are written
//! to `artifacts/native-bench.json`; the TypeScript benchmark reads that file
//! and never guesses a native number.
//!
//! Workload (identical to `benchmarks/workload.ts`):
//!   calls `n`, `p` accesses per call, `r` resources in the pool.
//!   access j of call i touches resource `(i * p + j) % r`.
//!   write rules: alternate3 writes when `(i + j) % 3 != 0`; parity writes when
//!   `i % 2 == 0`. Barrier effects declare no accesses.
//!   Resources are interned in first-appearance order, exactly as the
//!   TypeScript encoder does, so both sides see identical resource ids.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use effectplan_engine::{
    levels::compute_execution_levels, plan_arena, plan_arena_v2, plan_arena_v3,
    schedule::compute_schedule, ACCESS_READ, ACCESS_WRITE, ARENA_BYTES, DURATIONS_BASE_OFF,
    DURATIONS_REGION_LEN, IN_ACCESS_MODES_OFF, IN_ACCESS_OFFSETS_OFF, IN_CALL_MODES_OFF,
    IN_RESOURCE_IDS_OFF, LEVELS_BASE_OFF, LEVELS_REGION_LEN, MODE_EXCLUSIVE, MODE_RESOURCES,
    MODE_UNKNOWN, OUTPUT_REGION_LEN, OUT_BASE_OFF, OUT_HEADER_BYTES, OUT_RECORDS_OFF,
    TIMING_BASE_OFF, TIMING_REGION_LEN, WORKSPACE_REGION_LEN, WS_BASE_OFF,
};

#[derive(Clone, Copy, PartialEq)]
enum Write {
    Alternate3,
    Parity,
}

#[derive(Clone, Copy, PartialEq)]
enum ModeRule {
    Resources,
    Barrier3,
    Mixed,
}

struct Dataset {
    name: &'static str,
    calls: usize,
    per_call: usize,
    resources: usize,
    write: Write,
    mode: ModeRule,
    iterations: usize,
}

const fn dataset(
    name: &'static str,
    calls: usize,
    per_call: usize,
    resources: usize,
    write: Write,
    mode: ModeRule,
    iterations: usize,
) -> Dataset {
    Dataset {
        name,
        calls,
        per_call,
        resources,
        write,
        mode,
        iterations,
    }
}

const DATASETS: &[Dataset] = &[
    // The four Phase 1 datasets keep their exact shape, for regression numbers.
    dataset(
        "tiny",
        8,
        4,
        4,
        Write::Alternate3,
        ModeRule::Resources,
        2_000,
    ),
    dataset(
        "small",
        32,
        4,
        16,
        Write::Alternate3,
        ModeRule::Resources,
        1_000,
    ),
    dataset(
        "normal",
        64,
        4,
        32,
        Write::Alternate3,
        ModeRule::Resources,
        500,
    ),
    dataset(
        "medium",
        128,
        8,
        64,
        Write::Alternate3,
        ModeRule::Resources,
        200,
    ),
    // The Phase 2 shapes, each stressing one property of the level algorithm.
    dataset(
        "wide",
        64,
        2,
        128,
        Write::Alternate3,
        ModeRule::Resources,
        500,
    ),
    dataset("deep", 64, 1, 1, Write::Parity, ModeRule::Resources, 500),
    dataset(
        "diamond",
        64,
        2,
        16,
        Write::Parity,
        ModeRule::Resources,
        500,
    ),
    dataset(
        "barrier",
        64,
        2,
        8,
        Write::Alternate3,
        ModeRule::Barrier3,
        500,
    ),
    dataset(
        "conflict",
        64,
        4,
        4,
        Write::Alternate3,
        ModeRule::Resources,
        500,
    ),
    dataset(
        "maxbound",
        128,
        8,
        128,
        Write::Alternate3,
        ModeRule::Resources,
        200,
    ),
    dataset("mixed", 96, 4, 24, Write::Alternate3, ModeRule::Mixed, 300),
];

fn mode_of(rule: ModeRule, i: usize) -> u8 {
    match rule {
        ModeRule::Resources => MODE_RESOURCES,
        ModeRule::Barrier3 => {
            if i % 3 == 2 {
                MODE_EXCLUSIVE
            } else {
                MODE_RESOURCES
            }
        }
        ModeRule::Mixed => {
            if i % 5 == 0 {
                MODE_EXCLUSIVE
            } else if i % 11 == 3 {
                MODE_UNKNOWN
            } else {
                MODE_RESOURCES
            }
        }
    }
}

/// Interns the workload and writes it into the arena's input region.
///
/// This is the native counterpart of the TypeScript encoder: both intern in
/// first-appearance order, both give barrier effects no accesses, and both
/// write the same four packed arrays in ascending resource order.
fn encode_into(arena: &mut [u8], dataset: &Dataset) -> (u32, u32) {
    let mut interned: HashMap<usize, u32> = HashMap::new();
    let mut cursor = 0usize;

    for i in 0..dataset.calls {
        let mode = mode_of(dataset.mode, i);
        let mut rids: Vec<(u32, u8)> = Vec::with_capacity(dataset.per_call);
        if mode == MODE_RESOURCES {
            for j in 0..dataset.per_call {
                let key = (i * dataset.per_call + j) % dataset.resources;
                let next = interned.len() as u32 + 1;
                let rid = *interned.entry(key).or_insert(next);
                let write = match dataset.write {
                    Write::Parity => i % 2 == 0,
                    Write::Alternate3 => (i + j) % 3 != 0,
                };
                rids.push((rid, if write { ACCESS_WRITE } else { ACCESS_READ }));
            }
        }
        // The encoder emits one access per resource, in ascending resource order.
        rids.sort_by_key(|entry| entry.0);
        for (rid, amode) in rids {
            let at = IN_RESOURCE_IDS_OFF + cursor * 4;
            arena[at..at + 4].copy_from_slice(&rid.to_le_bytes());
            arena[IN_ACCESS_MODES_OFF + cursor] = amode;
            cursor += 1;
        }
        arena[IN_CALL_MODES_OFF + i] = mode;
        let at = IN_ACCESS_OFFSETS_OFF + (i + 1) * 4;
        arena[at..at + 4].copy_from_slice(&(cursor as u32).to_le_bytes());
    }

    (cursor as u32, interned.len() as u32 + 1)
}

fn percentile(sorted: &[Duration], fraction: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let index = ((sorted.len() as f64 - 1.0) * fraction).round() as usize;
    sorted[index].as_nanos() as f64 / 1_000.0
}

fn measure<F: FnMut() -> u32>(iterations: usize, mut body: F) -> (Vec<Duration>, u32) {
    let mut samples = Vec::with_capacity(iterations);
    let mut last = 0;
    for _ in 0..iterations {
        let start = Instant::now();
        last = body();
        samples.push(start.elapsed());
    }
    samples.sort_unstable();
    (samples, last)
}

/// Reads the emitted edge count out of the output header.
fn edge_count(arena: &[u8]) -> u32 {
    u32::from_le_bytes([
        arena[OUT_BASE_OFF + 8],
        arena[OUT_BASE_OFF + 9],
        arena[OUT_BASE_OFF + 10],
        arena[OUT_BASE_OFF + 11],
    ])
}

/// Reads the wave count out of the levels region the last pass wrote.
fn wave_count(arena: &[u8], calls: usize) -> usize {
    let mut highest = 0u16;
    for i in 0..calls {
        let at = LEVELS_BASE_OFF + 4 + i * 2;
        let level = u16::from_le_bytes([arena[at], arena[at + 1]]);
        if level > highest {
            highest = level;
        }
    }
    highest as usize + 1
}

fn main() {
    let mut results: Vec<String> = Vec::new();

    for dataset in DATASETS {
        let mut arena = vec![0u8; ARENA_BYTES];
        let (access_count, resource_count) = encode_into(&mut arena, dataset);
        for i in 0..dataset.calls {
            let ticks = 1u32 + ((i as u32 * 13) % 17);
            let at = DURATIONS_BASE_OFF + i * 4;
            arena[at..at + 4].copy_from_slice(&ticks.to_le_bytes());
        }
        let warmup = dataset.iterations / 10 + 1;

        let plan_v1 = |arena: &mut [u8]| -> i32 {
            unsafe {
                plan_arena(
                    arena,
                    1,
                    dataset.calls as u32,
                    access_count,
                    resource_count,
                    IN_CALL_MODES_OFF as u32,
                    IN_ACCESS_OFFSETS_OFF as u32,
                    IN_RESOURCE_IDS_OFF as u32,
                    IN_ACCESS_MODES_OFF as u32,
                    WS_BASE_OFF as u32,
                    WORKSPACE_REGION_LEN as u32,
                    OUT_BASE_OFF as u32,
                    OUTPUT_REGION_LEN as u32,
                )
            }
        };
        let plan_v2 = |arena: &mut [u8]| -> i32 {
            unsafe {
                plan_arena_v2(
                    arena,
                    1,
                    dataset.calls as u32,
                    access_count,
                    resource_count,
                    IN_CALL_MODES_OFF as u32,
                    IN_ACCESS_OFFSETS_OFF as u32,
                    IN_RESOURCE_IDS_OFF as u32,
                    IN_ACCESS_MODES_OFF as u32,
                    WS_BASE_OFF as u32,
                    WORKSPACE_REGION_LEN as u32,
                    OUT_BASE_OFF as u32,
                    OUTPUT_REGION_LEN as u32,
                    LEVELS_BASE_OFF as u32,
                    LEVELS_REGION_LEN as u32,
                )
            }
        };

        for _ in 0..warmup {
            assert_eq!(
                plan_v2(&mut arena),
                0,
                "the benchmark workload must be valid"
            );
        }

        // Encode: interning and writing the packed arrays.
        let (encode_samples, _) = measure(warmup.max(dataset.iterations / 4), || {
            encode_into(&mut arena, dataset).0
        });

        // Execute: the dependency pass alone, the version one entry point.
        let (exec_samples, edges) = measure(dataset.iterations, || {
            assert_eq!(
                plan_v1(&mut arena),
                0,
                "the benchmark workload must be valid"
            );
            edge_count(&arena)
        });

        // The extended pass: planning plus the level computation.
        let (v2_samples, _) = measure(dataset.iterations, || {
            assert_eq!(
                plan_v2(&mut arena),
                0,
                "the benchmark workload must be valid"
            );
            edge_count(&arena)
        });

        // The scheduling pass: plan, layer, then the critical path analysis.
        let plan_v3 = |arena: &mut [u8]| -> i32 {
            unsafe {
                plan_arena_v3(
                    arena,
                    1,
                    dataset.calls as u32,
                    access_count,
                    resource_count,
                    IN_CALL_MODES_OFF as u32,
                    IN_ACCESS_OFFSETS_OFF as u32,
                    IN_RESOURCE_IDS_OFF as u32,
                    IN_ACCESS_MODES_OFF as u32,
                    WS_BASE_OFF as u32,
                    WORKSPACE_REGION_LEN as u32,
                    OUT_BASE_OFF as u32,
                    OUTPUT_REGION_LEN as u32,
                    LEVELS_BASE_OFF as u32,
                    LEVELS_REGION_LEN as u32,
                    DURATIONS_BASE_OFF as u32,
                    DURATIONS_REGION_LEN as u32,
                    TIMING_BASE_OFF as u32,
                    TIMING_REGION_LEN as u32,
                )
            }
        };
        let (v3_samples, _) = measure(dataset.iterations, || {
            assert_eq!(
                plan_v3(&mut arena),
                0,
                "the benchmark workload must be valid"
            );
            edge_count(&arena)
        });

        // The level pass on its own, over a graph planned once up front.
        assert_eq!(plan_v1(&mut arena), 0);
        let edges_for_levels = edge_count(&arena) as usize;
        let mut levels_only = vec![0u8; LEVELS_REGION_LEN];
        let (levels_samples, _) = measure(warmup.max(dataset.iterations / 4), || {
            let records = &arena[OUT_BASE_OFF + OUT_HEADER_BYTES..OUT_BASE_OFF + OUTPUT_REGION_LEN];
            compute_execution_levels(records, edges_for_levels, &mut levels_only, dataset.calls)
                as u32
        });

        // The scheduling pass on its own, over the same graph the planner wrote,
        // so the critical path cost is separated from the arena plumbing.
        let durations_only =
            arena[DURATIONS_BASE_OFF..DURATIONS_BASE_OFF + DURATIONS_REGION_LEN].to_vec();
        let mut timing_only = vec![0u8; TIMING_REGION_LEN];
        let (schedule_samples, _) = measure(warmup.max(dataset.iterations / 4), || {
            let records = &arena[OUT_BASE_OFF + OUT_HEADER_BYTES..OUT_BASE_OFF + OUTPUT_REGION_LEN];
            compute_schedule(
                records,
                edges_for_levels,
                &durations_only,
                &mut timing_only,
                dataset.calls,
            ) as u32
        });

        // Decode: reading the emitted records back.
        let (decode_samples, _) = measure(warmup.max(dataset.iterations / 4), || {
            let count = edge_count(&arena);
            let base = OUT_BASE_OFF + OUT_RECORDS_OFF;
            let mut sum = 0u32;
            for e in 0..count as usize {
                let at = base + e * 16;
                sum = sum.wrapping_add(u32::from_le_bytes([
                    arena[at],
                    arena[at + 1],
                    arena[at + 2],
                    arena[at + 3],
                ]));
            }
            sum
        });

        assert_eq!(plan_v2(&mut arena), 0);
        let waves = wave_count(&arena, dataset.calls);
        assert_eq!(plan_v3(&mut arena), 0);
        let u32_at = |at: usize| u32::from_le_bytes(arena[at..at + 4].try_into().unwrap());
        let makespan = u32_at(TIMING_BASE_OFF + 8);
        let peak = u32_at(TIMING_BASE_OFF + 12);
        let critical = u32_at(TIMING_BASE_OFF + 24);

        results.push(format!(
            concat!(
                "    {{\n",
                "      \"name\": \"{}\",\n",
                "      \"calls\": {},\n",
                "      \"accesses\": {},\n",
                "      \"resources\": {},\n",
                "      \"edges\": {},\n",
                "      \"waves\": {},\n",
                "      \"makespan\": {},\n",
                "      \"peakConcurrency\": {},\n",
                "      \"criticalEvents\": {},\n",
                "      \"iterations\": {},\n",
                "      \"encodeP50Micros\": {:.3},\n",
                "      \"execP50Micros\": {:.3},\n",
                "      \"execP95Micros\": {:.3},\n",
                "      \"execP99Micros\": {:.3},\n",
                "      \"execMinMicros\": {:.3},\n",
                "      \"execV2P50Micros\": {:.3},\n",
                "      \"execV2P95Micros\": {:.3},\n",
                "      \"execV2P99Micros\": {:.3},\n",
                "      \"execV3P50Micros\": {:.3},\n",
                "      \"execV3P95Micros\": {:.3},\n",
                "      \"execV3P99Micros\": {:.3},\n",
                "      \"levelsOnlyP50Micros\": {:.3},\n",
                "      \"scheduleOnlyP50Micros\": {:.3},\n",
                "      \"decodeP50Micros\": {:.3}\n",
                "    }}"
            ),
            dataset.name,
            dataset.calls,
            access_count,
            resource_count - 1,
            edges,
            waves,
            makespan,
            peak,
            critical,
            dataset.iterations,
            percentile(&encode_samples, 0.5),
            percentile(&exec_samples, 0.5),
            percentile(&exec_samples, 0.95),
            percentile(&exec_samples, 0.99),
            percentile(&exec_samples, 0.0),
            percentile(&v2_samples, 0.5),
            percentile(&v2_samples, 0.95),
            percentile(&v2_samples, 0.99),
            percentile(&v3_samples, 0.5),
            percentile(&v3_samples, 0.95),
            percentile(&v3_samples, 0.99),
            percentile(&levels_samples, 0.5),
            percentile(&schedule_samples, 0.5),
            percentile(&decode_samples, 0.5),
        ));
        println!(
            "{:>9}: plan {:>8.3} us | +levels {:>8.3} us | +timing {:>8.3} us | edges {:>5} waves {:>3} makespan {:>6} peak {:>3}",
            dataset.name,
            percentile(&exec_samples, 0.5),
            percentile(&v2_samples, 0.5),
            percentile(&v3_samples, 0.5),
            edges,
            waves,
            makespan,
            peak,
        );
    }

    let json = format!(
        concat!(
            "{{\n",
            "  \"source\": \"cargo bench -p effectplan-engine --bench native\",\n",
            "  \"note\": \"the same plan_arena, plan_arena_v2 and plan_arena_v3 entry points the wasm build exports\",\n",
            "  \"datasets\": [\n{}\n  ]\n",
            "}}\n"
        ),
        results.join(",\n")
    );

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let path = root.join("artifacts").join("native-bench.json");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&path, json).expect("the native benchmark must be able to write its report");
    println!("wrote {}", path.display());
}
