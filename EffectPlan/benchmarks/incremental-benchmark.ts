/**
 * The incremental benchmark: what an update costs against a full rebuild.
 *
 * The RFC's gate is deliberately qualitative - for append-only updates and
 * changes near the end of a batch, an incremental update should be
 * *demonstrably* cheaper than rebuilding the whole plan. This harness measures
 * both, interleaved, in one process on one machine, and fails when it is not.
 *
 * Both sides are warmed and both are sampled at length. An earlier version of
 * this file warmed three times and took the median of ten samples; the warm-up
 * transient outlasted that, so the rebuild baseline sat two to four times above
 * its own warmed value and the ratio flattered the update. The numbers quoted in
 * docs/INCREMENTAL-BENCHMARKS.md are from the corrected harness.
 *
 *   npm run benchmark:incremental
 */

import { writeFileSync, mkdirSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import { performance } from "node:perf_hooks";
import { fileURLToPath } from "node:url";

import type { PlanChange, PlanSession, ToolCall, UpdateDiagnostics } from "../packages/core/dist/index.js";
import { DATASETS, buildCalls } from "./workload.js";

// The published package, resolved at runtime so the benchmark measures what
// ships rather than the TypeScript sources.
const DIST = new URL("../../packages/core/dist/", import.meta.url);
const { EffectPlan } = (await import(new URL("index.js", DIST).href)) as typeof import("../packages/core/dist/index.js");

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const ARTIFACTS = path.join(ROOT, "artifacts");

// A rebuild and its matching update are timed in the same breath, so whatever
// the machine does mid-run lands on both sides of the comparison. With three
// warm-up iterations and a ten-sample median this harness reported a 128-call
// rebuild at 4,381 us; the same call warmed and measured at length is 740 us,
// and a median that far from the truth makes an update look five times cheaper
// than it is. Both numbers below are medians of enough samples to hold still.
const WARMUP = 15;
const ITERATIONS = 25;
const SWEEP_ITERATIONS = 7;

interface Stats {
  readonly p50: number;
  readonly p95: number;
  readonly min: number;
}

function stats(samples: readonly number[]): Stats {
  const sorted = [...samples].sort((a, b) => a - b);
  const at = (fraction: number): number =>
    sorted[Math.min(sorted.length - 1, Math.round((sorted.length - 1) * fraction))] ?? 0;
  return { p50: at(0.5), p95: at(0.95), min: sorted[0] ?? 0 };
}

function one(body: () => unknown): number {
  const start = performance.now();
  body();
  return (performance.now() - start) * 1_000;
}

interface Case {
  readonly name: string;
  readonly change: (calls: readonly ToolCall[]) => PlanChange;
  /** Which end of the batch the change lands on. */
  readonly where: "end" | "middle";
}

const CASES: readonly Case[] = [
  {
    name: "append one",
    where: "end",
    change: (calls) => ({
      type: "append",
      key: "extra",
      call: {
        id: "extra",
        effect: {
          version: 1,
          mode: "resources",
          accesses: [{ resource: { namespace: "fs", id: "f9" }, mode: "read" }],
        },
      },
    }),
  },
  {
    name: "replace last",
    where: "end",
    change: (calls) => ({
      type: "replace",
      key: `k${calls.length - 1}`,
      call: {
        id: "replaced",
        effect: {
          version: 1,
          mode: "resources",
          accesses: [{ resource: { namespace: "fs", id: "f0" }, mode: "write" }],
        },
      },
    }),
  },
  {
    name: "remove last",
    where: "end",
    change: (calls) => ({ type: "remove", key: `k${calls.length - 1}` }),
  },
  {
    name: "replace middle",
    where: "middle",
    change: (calls) => ({
      type: "replace",
      key: `k${Math.floor(calls.length / 2)}`,
      call: {
        id: "replaced",
        effect: {
          version: 1,
          mode: "resources",
          accesses: [{ resource: { namespace: "fs", id: "f1" }, mode: "write" }],
        },
      },
    }),
  },
];

const report: Record<string, unknown>[] = [];
let failures = 0;
let gatedCases = 0;

// What counts as "big enough to gate on" cannot be a number of microseconds: a
// busy machine inflates every absolute measurement, and a gate that quietly
// stops applying is worse than no gate. The eight-call workload carries almost
// no dependency work, so its rebuild is this run's floor, and three times that
// floor is where the effect has to be measurable. Recomputed every run.
let floorMicros = 0;
const MIN_GATED_CASES = 6;

interface Prepared {
  readonly testCase: Case;
  readonly change: PlanChange;
  readonly samples: number[];
  /** The diagnostics of the first measured update, for the report. */
  observed?: UpdateDiagnostics;
}

for (const workload of DATASETS) {
  const calls = buildCalls(workload);
  const planner = await EffectPlan.create();
  const entries = calls.map((call, index) => ({ key: `k${index}`, call }));
  const maxCalls = planner.getEngineInfo().limits.maxCalls;

  // One session per timed update: an update mutates the session it lands on, so
  // a second update on the same session would measure a different transaction.
  // Each is created the moment before it is needed and disposed the moment
  // after, because sessions left alive hold their checkpoint copies across the
  // whole run and slow the very rebuild they are being compared against.
  const prepared: Prepared[] = [];
  for (const testCase of CASES) {
    // An append would push the batch past the engine's call ceiling.
    if (testCase.name === "append one" && calls.length >= maxCalls) {
      process.stdout.write(
        `${workload.name.padEnd(9)} ${testCase.name.padEnd(15)} skipped: the batch is already at the call ceiling
`,
      );
      continue;
    }
    prepared.push({ testCase, change: testCase.change(calls), samples: [] });
  }

  const freshSession = (): PlanSession => planner.createSession(entries, { checkpointInterval: 64 });

  for (let i = 0; i < WARMUP; i++) {
    planner.planBatch(calls);
    for (const p of prepared) {
      const warm = freshSession();
      warm.applyChanges({ expectedVersion: 1, changes: [p.change] });
      warm.dispose();
    }
  }

  // The rebuild and the update alternate. Sampled back to back, drift in the
  // machine lands on one side only and the ratio inherits it. Creating the
  // session is not part of either measurement.
  const rebuildSamples: number[] = [];
  for (let round = 0; round < ITERATIONS; round++) {
    rebuildSamples.push(one(() => planner.planBatch(calls)));
    for (const p of prepared) {
      const session = freshSession();
      p.samples.push(one(() => session.applyChanges({ expectedVersion: 1, changes: [p.change] })));
      p.observed ??= session.current().update;
      session.dispose();
    }
  }
  const rebuild = stats(rebuildSamples);
  if (workload.name === "tiny") floorMicros = rebuild.p50 * 3;

  const rows: Record<string, unknown>[] = [];
  for (const p of prepared) {
    const testCase = p.testCase;
    const update = stats(p.samples);
    const observed = p.observed!;
    const strategy = observed.strategy;
    const reason = observed.reason ?? "";
    const reused = observed.reusedPrefixCalls;
    const ratio = (update.p50 / rebuild.p50) * 100;

    // The gate: an update near the end of the batch, or an append, must be
    // cheaper than rebuilding the whole thing - on a workload that carries
    // enough dependency work for the claim to mean anything. A batch of 96 calls
    // or more always qualifies. A 64-call batch qualifies only while it costs at
    // least three times this run's eight-call rebuild: below that the fixed
    // bookkeeping of an update is the same order as the whole rebuild, and
    // asserting either direction would be asserting the harness's own overhead.
    const dense = calls.length >= 96 || rebuild.p50 >= floorMicros;
    const gated = testCase.where === "end" && calls.length >= 64 && dense;
    const passed = gated ? ratio <= 100 : true;
    if (gated) gatedCases++;
    if (!passed) failures++;

    rows.push({
      name: testCase.name,
      strategy,
      reason,
      reusedPrefixCalls: reused,
      updateP50Micros: update.p50,
      updateP95Micros: update.p95,
      updateMinMicros: update.min,
      rebuildP50Micros: rebuild.p50,
      rebuildMinMicros: rebuild.min,
      ratioPercent: ratio,
      gated,
      passed,
    });
    process.stdout.write(
      `${workload.name.padEnd(9)} ${testCase.name.padEnd(15)} ${strategy.padEnd(14)} ` +
        `update ${update.p50.toFixed(1).padStart(8)} us  rebuild ${rebuild.p50.toFixed(1).padStart(8)} us  ` +
        `${ratio.toFixed(0).padStart(4)}%${gated ? (passed ? "  within the gate" : "  OVER THE GATE") : "  (not gated)"}\n`,
    );
  }
  // How often a checkpoint is worth its memory: a denser interval means a
  // shorter suffix to replan, and more state to keep.
  const intervals: Record<string, unknown>[] = [];
  if (workload.calls >= 64) {
    const target = Math.floor(workload.calls / 2);
    const change: PlanChange = {
      type: "replace",
      key: `k${target}`,
      call: {
        id: "replaced",
        effect: {
          version: 1,
          mode: "resources",
          accesses: [{ resource: { namespace: "fs", id: "f1" }, mode: "write" }],
        },
      },
    };
    for (const interval of [16, 32, 64, 128]) {
      const options = { checkpointInterval: interval };
      for (let i = 0; i < 3; i++) {
        const warm = planner.createSession(entries, options);
        warm.applyChanges({ expectedVersion: 1, changes: [change] });
        warm.dispose();
      }
      const samples: number[] = [];
      let reused = 0;
      for (let i = 0; i < SWEEP_ITERATIONS; i++) {
        const session = planner.createSession(entries, options);
        samples.push(one(() => session.applyChanges({ expectedVersion: 1, changes: [change] })));
        reused = session.current().update.reusedPrefixCalls;
        session.dispose();
      }
      const sweepStats = stats(samples);
      intervals.push({
        interval,
        reusedPrefixCalls: reused,
        updateP50Micros: sweepStats.p50,
        updateMinMicros: sweepStats.min,
      });
      process.stdout.write(
        `${workload.name.padEnd(9)} interval ${String(interval).padStart(4)} reused ` +
          `${String(reused).padStart(4)} calls  ` +
          `${sweepStats.p50.toFixed(1).padStart(8)} us
`,
      );
    }
  }
  report.push({ name: workload.name, calls: workload.calls, rebuildP50Micros: rebuild.p50, cases: rows, intervals });
}

mkdirSync(ARTIFACTS, { recursive: true });
writeFileSync(
  path.join(ARTIFACTS, "incremental-benchmarks.json"),
  `${JSON.stringify(
    {
      measuredAt: new Date().toISOString(),
      node: process.version,
      warmup: WARMUP,
      iterations: ITERATIONS,
      sweepIterations: SWEEP_ITERATIONS,
      gateFloorMicros: floorMicros,
      gatedCases,
      workloads: report,
    },
    null,
    2,
  )}\n`,
);

process.stdout.write(
  `gate floor ${floorMicros.toFixed(1)} us (three times the eight-call rebuild); ` +
    `${gatedCases} case(s) gated, ${failures} over the gate\n`,
);

// A gate that fired on nothing has proved nothing. If the machine was too busy
// or too fast for the dense shapes to clear the floor, say so and fail.
if (gatedCases < MIN_GATED_CASES) {
  process.stderr.write(
    `incremental-benchmark: only ${gatedCases} case(s) were gated, at least ${MIN_GATED_CASES} required\n`,
  );
  process.exit(1);
}
if (failures > 0) {
  process.stderr.write(
    `incremental-benchmark: ${failures} case(s) were not cheaper than a full rebuild\n`,
  );
  process.exit(1);
}
process.stdout.write(`wrote ${path.relative(ROOT, path.join(ARTIFACTS, "incremental-benchmarks.json"))}\n`);
