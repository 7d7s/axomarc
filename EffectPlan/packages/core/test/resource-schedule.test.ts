/**
 * Phase 5 cross-check: the engine's resource schedules against an independent
 * reference.
 *
 * The engine's Rust tests already check feasibility directly, but they were
 * written against the same reading of the model by the same author, which is how a
 * shared mistake survives a test suite. This file drives the shipped artifact
 * through the raw ABI and checks each schedule with
 * `benchmarks/reference-resource-scheduler.ts`: a brute-force tick-by-tick
 * capacity checker, and an exhaustive search over precedence-feasible sequences
 * whose makespan is a true lower bound.
 *
 * What is asserted: every schedule is feasible, every edge holds, no capacity is
 * exceeded, the makespan never *beats* the optimum (that would mean a constraint
 * was ignored somewhere) and never exceeds the serial upper bound of total
 * duration, and the same input always yields the same bytes. What is only
 * reported is the gap to the optimum: a heuristic is allowed to be second-best,
 * and the day it claims otherwise is the day to stop believing it.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { wasmBytes } from "./support.js";
import {
  optimalMakespan,
  seeded,
  verifySchedule,
  type ResourceInstance,
  type Schedule,
} from "../../../benchmarks/reference-resource-scheduler.js";

interface Fixture {
  readonly memory: WebAssembly.Memory;
  readonly abi: number;
  readonly base: number;
  readonly arenaLen: number;
  readonly maxCalls: number;
  readonly maxAccess: number;
  readonly edgeBytes: number;
  readonly input: number;
  readonly ws: number;
  readonly wsLen: number;
  readonly out: number;
  readonly outLen: number;
  readonly levels: number;
  readonly levelsLen: number;
  readonly durations: number;
  readonly durationsLen: number;
  readonly timing: number;
  readonly timingLen: number;
  readonly checkpoint: number;
  readonly checkpointLen: number;
  readonly capacities: number;
  readonly capacitiesLen: number;
  readonly demandOffsets: number;
  readonly demandOffsetsLen: number;
  readonly demandRecords: number;
  readonly demandRecordsLen: number;
  readonly scheduleOut: number;
  readonly scheduleLen: number;
  readonly stats: number;
  readonly statsLen: number;
  readonly diagnostics: number;
  readonly diagnosticsLen: number;
  readonly planV4: (...args: number[]) => number;
  readonly place: (...args: number[]) => number;
}

function fixture(): Fixture {
  const module = new WebAssembly.Module(wasmBytes());
  const instance = new WebAssembly.Instance(module, {}) as unknown as {
    exports: Record<string, unknown>;
  };
  const info = instance.exports.effectplan_info as (kind: number) => number;
  return {
    memory: instance.exports.memory as WebAssembly.Memory,
    abi: info(0),
    base: info(1),
    arenaLen: info(2),
    input: info(3),
    ws: info(5),
    wsLen: info(6),
    out: info(7),
    outLen: info(8),
    maxCalls: info(9),
    maxAccess: info(10),
    edgeBytes: info(14),
    levels: info(22),
    levelsLen: info(23),
    durations: info(26),
    durationsLen: info(27),
    timing: info(29),
    timingLen: info(30),
    checkpoint: info(33),
    checkpointLen: info(34),
    capacities: info(36),
    capacitiesLen: info(37),
    demandOffsets: info(38),
    demandOffsetsLen: info(39),
    demandRecords: info(40),
    demandRecordsLen: info(41),
    scheduleOut: info(42),
    scheduleLen: info(43),
    stats: info(44),
    statsLen: info(45),
    diagnostics: info(46),
    diagnosticsLen: info(47),
    planV4: instance.exports.effectplan_plan_v4 as (...args: number[]) => number,
    place: instance.exports.effectplan_resource_schedule as (...args: number[]) => number,
  };
}

/**
 * Sub-arrays inside the input region: call modes, access offsets, resource ids,
 * access modes. Derived from the published counts, so the test cannot disagree
 * with the layout the engine reports.
 */
function inputs(f: Fixture): { modes: number; offsets: number; ids: number; accessModes: number } {
  const modes = f.input;
  const offsets = modes + f.maxCalls;
  const ids = offsets + 4 * (f.maxCalls + 1);
  const accessModes = ids + 4 * f.maxAccess;
  return { modes, offsets, ids, accessModes };
}

interface Op {
  /** Dependency resources touched: `(resource, 0 read | 1 write)`. Id 0 is the barrier, so tests start at 1. */
  readonly accesses: readonly (readonly [number, 0 | 1])[];
  /** Capacity resources held for the whole call: `(resource, amount)`. */
  readonly demands: readonly (readonly [number, number])[];
  readonly duration: number;
}

function op(
  accesses: readonly (readonly [number, 0 | 1])[],
  demands: readonly (readonly [number, number])[],
  duration: number,
): Op {
  return { accesses, demands, duration };
}

const WRITE = 1;
const READ = 0;

interface Outcome {
  readonly status: number;
  readonly windows: readonly (readonly [number, number])[];
  readonly edges: readonly (readonly [number, number])[];
  readonly makespan: number;
  readonly deferrals: number;
}

/** Plan the batch, then place it. The graph is read back, never assumed. */
function planAndPlace(f: Fixture, ops: readonly Op[], capacities: readonly number[]): Outcome {
  const view = new DataView(f.memory.buffer);
  // Clear the arena only. The module keeps read-only data below the arena base
  // — jump tables and panic strings — and erasing it changes what the engine
  // computes rather than raising an error.
  new Uint8Array(f.memory.buffer).fill(0, f.base, f.base + f.arenaLen);
  const region = inputs(f);

  const callModes: number[] = [];
  const accessOffsets: number[] = [0];
  const ids: number[] = [];
  const accessModes: number[] = [];
  for (const entry of ops) {
    callModes.push(entry.accesses.length === 0 ? 0 : 1);
    for (const [resource, mode] of entry.accesses) {
      ids.push(resource);
      accessModes.push(mode);
    }
    accessOffsets.push(ids.length);
  }
  const dependencyResources = ids.length === 0 ? 1 : Math.max(...ids) + 1;

  callModes.forEach((value, index) => view.setUint8(region.modes + index, value));
  accessModes.forEach((value, index) => view.setUint8(region.accessModes + index, value));
  accessOffsets.forEach((value, index) => view.setUint32(region.offsets + index * 4, value, true));
  ids.forEach((value, index) => view.setUint32(region.ids + index * 4, value, true));
  ops.forEach((entry, index) => view.setUint32(f.durations + index * 4, entry.duration, true));

  const planStatus = f.planV4(
    f.abi,
    ops.length,
    ids.length,
    dependencyResources,
    region.modes,
    region.offsets,
    region.ids,
    region.accessModes,
    f.ws,
    f.wsLen,
    f.out,
    f.outLen,
    f.levels,
    f.levelsLen,
    f.durations,
    f.durationsLen,
    f.timing,
    f.timingLen,
    f.checkpoint,
    f.checkpointLen,
  );
  assert.equal(planStatus, 0, "the dependency batch must plan before it can be placed");

  const edgeCount = view.getUint32(f.out + 8, true);
  assert.equal(view.getUint32(f.out, true), f.abi, "graph header names this abi");
  assert.equal(view.getUint32(f.out + 4, true), ops.length, "graph header names this batch");
  const edges: [number, number][] = [];
  for (let index = 0; index < edgeCount; index++) {
    const at = f.out + 16 + index * f.edgeBytes;
    edges.push([view.getUint32(at, true), view.getUint32(at + 4, true)]);
  }

  capacities.forEach((value, index) => view.setUint32(f.capacities + index * 4, value, true));
  const demandOffsets: number[] = [0];
  const records: (readonly [number, number])[] = [];
  for (const entry of ops) {
    for (const pair of entry.demands) records.push(pair);
    demandOffsets.push(records.length);
  }
  demandOffsets.forEach((value, index) => view.setUint32(f.demandOffsets + index * 4, value, true));
  records.forEach(([resource, amount], index) => {
    const at = f.demandRecords + index * 8;
    view.setUint32(at, resource, true);
    view.setUint32(at + 4, amount, true);
  });

  const status = f.place(
    f.abi,
    ops.length,
    capacities.length,
    records.length,
    f.capacities,
    f.capacitiesLen,
    f.demandOffsets,
    f.demandOffsetsLen,
    f.demandRecords,
    f.demandRecordsLen,
    f.durations,
    f.durationsLen,
    f.out,
    f.outLen,
    f.timing,
    f.timingLen,
    f.scheduleOut,
    f.scheduleLen,
    f.stats,
    f.statsLen,
    f.diagnostics,
    f.diagnosticsLen,
  );

  const windows: [number, number][] = [];
  for (let index = 0; index < ops.length; index++) {
    const at = f.scheduleOut + 32 + index * 8;
    windows.push([view.getUint32(at, true), view.getUint32(at + 4, true)]);
  }
  return {
    status,
    windows,
    edges,
    makespan: view.getUint32(f.scheduleOut + 8, true),
    deferrals: view.getUint32(f.scheduleOut + 16, true),
  };
}

/** The instance as the reference sees it, from the same declarations. */
function referenceInstance(
  ops: readonly Op[],
  edges: readonly (readonly [number, number])[],
  capacities: readonly number[],
): ResourceInstance {
  return {
    calls: ops.map((entry) => ({ duration: entry.duration, demands: entry.demands })),
    edges,
    capacities,
  };
}

/** Assert one placement is everything it claims to be. */
function assertSound(
  label: string,
  ops: readonly Op[],
  capacities: readonly number[],
  outcome: Outcome,
): { optimum: number; gap: number } {
  assert.equal(outcome.status, 0, `${label}: placement refused`);
  const instance = referenceInstance(ops, outcome.edges, capacities);
  const schedule: Schedule = { windows: outcome.windows, makespan: outcome.makespan };
  const problems = verifySchedule(instance, schedule);
  assert.deepEqual(problems, [], `${label}: the reference found violations`);

  const optimum = optimalMakespan(instance);
  const totalWork = ops.reduce((sum, entry) => sum + entry.duration, 0);
  assert.ok(
    outcome.makespan >= optimum,
    `${label}: ${outcome.makespan} beats the exhaustive optimum ${optimum} — a constraint was ignored`,
  );
  assert.ok(
    outcome.makespan <= totalWork,
    `${label}: ${outcome.makespan} exceeds the serial bound ${totalWork}`,
  );
  return { optimum, gap: outcome.makespan - optimum };
}

describe("resource schedules against an independent reference", () => {
  it("a chain that also contends for a lane matches the optimum", () => {
    const f = fixture();
    // 0 writes r1, 1 reads r1 (so 0 -> 1), and both hold one unit of a single lane.
    const ops = [
      op([[1, WRITE]], [[0, 1]], 3),
      op([[1, READ]], [[0, 1]], 4),
    ];
    const outcome = planAndPlace(f, ops, [1]);
    assert.ok(outcome.edges.length >= 1, "the dependency edge should exist");
    const { gap } = assertSound("chain", ops, [1], outcome);
    assert.equal(gap, 0);
    assert.deepEqual(outcome.windows, [[0, 3], [3, 7]]);
  });

  it("contention alone serialises calls that have no dependency between them", () => {
    const f = fixture();
    const ops = [
      op([], [[0, 1]], 2),
      op([], [[0, 1]], 2),
      op([], [[0, 1]], 2),
    ];
    const outcome = planAndPlace(f, ops, [1]);
    assert.deepEqual(outcome.edges, [], "no pair conflicts on a dependency resource");
    const { optimum, gap } = assertSound("lane", ops, [1], outcome);
    assert.equal(outcome.makespan, 6);
    assert.equal(gap, optimum === 6 ? 0 : 1);
    assert.ok(outcome.deferrals >= 2, "the waits were real deferrals");
  });

  it("refuses to publish a schedule when a demand cannot be met", () => {
    const f = fixture();
    const ops = [op([], [[0, 5]], 3), op([], [[0, 1]], 3)];
    const outcome = planAndPlace(f, ops, [2]);
    assert.equal(outcome.status, 133, "a demand above capacity is infeasible, not merely unschedulable");
    assert.deepEqual(outcome.windows, [[0, 0], [0, 0]], "no schedule is published");
    assert.equal(outcome.makespan, 0);
  });

  it("randomised instances are all feasible, and the gap to optimal is reported", () => {
    const f = fixture();
    const random = seeded(0x5eed_2026);
    let worstGap = 0;
    let worstLabel = "";
    let checked = 0;

    for (let caseNumber = 0; caseNumber < 24; caseNumber++) {
      const callCount = 3 + Math.floor(random() * 4); // 3..6, small enough to solve exactly
      const capacityCount = 1 + Math.floor(random() * 2);
      const capacities = Array.from(
        { length: capacityCount },
        () => 1 + Math.floor(random() * 3),
      );
      const ops: Op[] = [];
      for (let index = 0; index < callCount; index++) {
        const accesses: [number, 0 | 1][] = [];
        const shape = random();
        if (shape < 0.6) {
          accesses.push([1 + Math.floor(random() * 2), random() < 0.5 ? WRITE : READ]);
        }
        const demands: [number, number][] = [];
        for (let resource = 0; resource < capacityCount; resource++) {
          if (random() < 0.5) demands.push([resource, 1 + Math.floor(random() * capacities[resource]!)]);
        }
        ops.push(op(accesses, demands, 1 + Math.floor(random() * 4)));
      }

      const outcome = planAndPlace(f, ops, capacities);
      const { gap } = assertSound(`case ${caseNumber}`, ops, capacities, outcome);
      checked += 1;
      if (gap > worstGap) {
        worstGap = gap;
        worstLabel = `case ${caseNumber} (${callCount} calls, ${capacities.join("/")})`;
      }

      // Determinism, byte for byte, from the same declarations.
      const again = planAndPlace(f, ops, capacities);
      assert.deepEqual(again.windows, outcome.windows, `case ${caseNumber} is not deterministic`);
      assert.equal(again.makespan, outcome.makespan);
    }

    assert.equal(checked, 24);
    process.stdout.write(
      `      ${checked} randomised placements verified against the exhaustive optimum; ` +
        `worst gap ${worstGap} ticks (${worstLabel})\n`,
    );
  });
});
