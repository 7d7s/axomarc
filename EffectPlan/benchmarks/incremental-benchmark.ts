/**
 * The incremental benchmark: what an update costs against a full rebuild.
 *
 * The RFC's gate is deliberately qualitative - for append-only updates and
 * changes near the end of a batch, an incremental update should be
 * *demonstrably* cheaper than rebuilding the whole plan. This harness measures
 * both in one process on the same machine and fails when it is not.
 *
 *   npm run benchmark:incremental
 */

import { writeFileSync, mkdirSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import { performance } from "node:perf_hooks";
import { fileURLToPath } from "node:url";

import type { PlanChange, ToolCall } from "../packages/core/dist/index.js";
import { DATASETS, buildCalls } from "./workload.js";

// The published package, resolved at runtime so the benchmark measures what
// ships rather than the TypeScript sources.
const DIST = new URL("../../packages/core/dist/", import.meta.url);
const { EffectPlan } = (await import(new URL("index.js", DIST).href)) as typeof import("../packages/core/dist/index.js");

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const ARTIFACTS = path.join(ROOT, "artifacts");

const WARMUP = 3;
const ITERATIONS = 10;

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

function timed(iterations: number, body: (index: number) => unknown): number[] {
  const samples: number[] = [];
  for (let i = 0; i < iterations; i++) {
    const start = performance.now();
    body(i);
    samples.push((performance.now() - start) * 1_000);
  }
  return samples;
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

for (const workload of DATASETS) {
  const calls = buildCalls(workload);
  const planner = await EffectPlan.create();
  const entries = calls.map((call, index) => ({ key: `k${index}`, call }));

  // The full rebuild: what a caller would do without a session.
  for (let i = 0; i < WARMUP; i++) planner.planBatch(calls);
  const rebuild = stats(timed(ITERATIONS, () => planner.planBatch(calls)));

  const rows: Record<string, unknown>[] = [];
  for (const testCase of CASES) {
    // An append would push the batch past the engine's call ceiling.
    if (testCase.name === "append one" && calls.length >= planner.getEngineInfo().limits.maxCalls) {
      process.stdout.write(
        `${workload.name.padEnd(9)} ${testCase.name.padEnd(15)} skipped: the batch is already at the call ceiling
`,
      );
      continue;
    }
    const session = planner.createSession(entries, { checkpointInterval: 64 });
    const change = testCase.change(calls);
    // Warm the session's own checkpoints, then measure the update itself.
    for (let i = 0; i < WARMUP; i++) {
      const warm = planner.createSession(entries, { checkpointInterval: 64 });
      warm.applyChanges({ expectedVersion: 1, changes: [change] });
    }
    const sessions = Array.from({ length: ITERATIONS }, () => planner.createSession(entries, { checkpointInterval: 64 }));
    const versions = sessions.map(() => 1);
    const samples = timed(ITERATIONS, (index) => {
      const session = sessions[index]!;
      session.applyChanges({ expectedVersion: versions[index]!, changes: [change] });
      versions[index] = session.current().version;
    });
    const update = stats(samples);
    const strategy = sessions[0]!.current().update.strategy;
    const reason = sessions[0]!.current().update.reason ?? "";
    const reused = sessions[0]!.current().update.reusedPrefixCalls;
    const ratio = (update.p50 / rebuild.p50) * 100;

    // The gate: an update near the end of the batch, or an append, must be
    // cheaper than rebuilding the whole thing - on a workload big enough for
    // the measurement to mean something. Below roughly a quarter of a
    // millisecond of rebuild, the run-to-run noise of this harness is larger
    // than the effect being measured, and gating on it would be gating on
    // noise.
    const gated = testCase.where === "end" && rebuild.p50 >= 250;
    const passed = gated ? ratio <= 100 : true;
    if (!passed) failures++;

    rows.push({
      name: testCase.name,
      strategy,
      reason,
      reusedPrefixCalls: reused,
      updateP50Micros: update.p50,
      updateP95Micros: update.p95,
      rebuildP50Micros: rebuild.p50,
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
    for (const interval of [16, 32, 64, 128]) {
      // Warm the shape first: the first interval measured would otherwise carry
      // the cost of the first run through this code path.
      const warm = planner.createSession(entries, { checkpointInterval: interval });
      warm.applyChanges({
        expectedVersion: 1,
        changes: [
          {
            type: "replace",
            key: `k${Math.floor(workload.calls / 2)}`,
            call: {
              id: "warmed",
              effect: {
                version: 1,
                mode: "resources",
                accesses: [{ resource: { namespace: "fs", id: "f1" }, mode: "write" }],
              },
            },
          },
        ],
      });
      const session = planner.createSession(entries, { checkpointInterval: interval });
      const target = Math.floor(workload.calls / 2);
      const change = {
        type: "replace" as const,
        key: `k${target}`,
        call: {
          id: "replaced",
          effect: {
            version: 1 as const,
            mode: "resources" as const,
            accesses: [{ resource: { namespace: "fs", id: "f1" }, mode: "write" as const }],
          },
        },
      };
      const start = performance.now();
      session.applyChanges({ expectedVersion: 1, changes: [change] });
      const elapsed = performance.now() - start;
      intervals.push({
        interval,
        reusedPrefixCalls: session.current().update.reusedPrefixCalls,
        micros: elapsed * 1_000,
      });
      process.stdout.write(
        `${workload.name.padEnd(9)} interval ${String(interval).padStart(4)} reused ` +
          `${String(session.current().update.reusedPrefixCalls).padStart(4)} calls  ` +
          `${(elapsed * 1_000).toFixed(1).padStart(8)} us
`,
      );
    }
  }
  report.push({ name: workload.name, calls: workload.calls, rebuildP50Micros: rebuild.p50, cases: rows, intervals });
}

mkdirSync(ARTIFACTS, { recursive: true });
writeFileSync(
  path.join(ARTIFACTS, "incremental-benchmarks.json"),
  `${JSON.stringify({ measuredAt: new Date().toISOString(), node: process.version, workloads: report }, null, 2)}\n`,
);

if (failures > 0) {
  process.stderr.write(
    `incremental-benchmark: ${failures} case(s) were not cheaper than a full rebuild\n`,
  );
  process.exit(1);
}
process.stdout.write(`wrote ${path.relative(ROOT, path.join(ARTIFACTS, "incremental-benchmarks.json"))}\n`);
