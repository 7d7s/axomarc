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
  readonly changes: (calls: readonly ToolCall[]) => readonly PlanChange[];
  /** How many calls the transaction adds, if any - what the ceilings are checked against. */
  readonly adds?: number;
  /** Where the transaction lands: `end` is gateable, `middle` and `mixed` are reported. */
  readonly where: "end" | "middle" | "mixed";
}

function write(id: string, resource: string): ToolCall {
  return {
    id,
    effect: {
      version: 1,
      mode: "resources",
      accesses: [{ resource: { namespace: "fs", id: resource }, mode: "write" }],
    },
  };
}

function appended(count: number): PlanChange[] {
  return Array.from({ length: count }, (_, index) => ({
    type: "append" as const,
    key: `extra${index}`,
    call: write(`extra${index}`, `f${index % 8}`),
  }));
}

const CASES: readonly Case[] = [
  { name: "append one", where: "end", adds: 1, changes: () => appended(1) },
  { name: "append ten", where: "end", adds: 10, changes: () => appended(10) },
  { name: "append one hundred", where: "end", adds: 100, changes: () => appended(100) },
  { name: "replace last", where: "end", changes: (calls) => [{ type: "replace", key: `k${calls.length - 1}`, call: write("replaced", "f0") }] },
  { name: "remove last", where: "end", changes: (calls) => [{ type: "remove", key: `k${calls.length - 1}` }] },
  {
    name: "replace middle",
    where: "middle",
    changes: (calls) => [{ type: "replace", key: `k${Math.floor(calls.length / 2)}`, call: write("replaced", "f1") }],
  },
  {
    name: "remove middle",
    where: "middle",
    changes: (calls) => [{ type: "remove", key: `k${Math.floor(calls.length / 2)}` }],
  },
  {
    name: "replace first",
    where: "middle",
    // The shape that cannot reuse anything: the boundary is the very first call.
    changes: (calls) => [{ type: "replace", key: "k0", call: write("replaced", "f2") }],
  },
  {
    // One transaction, three kinds of edit, the earliest touched call in the
    // middle - what a caller batching a small refactor actually sends.
    name: "multi-update",
    where: "mixed",
    adds: 2,
    changes: (calls) => [
      { type: "replace", key: `k${Math.floor(calls.length / 2)}`, call: write("replaced", "f3") },
      { type: "remove", key: `k${calls.length - 1}` },
      ...appended(2),
    ],
  },
];

const report: Record<string, unknown>[] = [];
let failures = 0;
let gatedCases = 0;
/** How a no-reuse update costs against the rebuild it fell back to, per case. */
const fallbackParity: number[] = [];

// What counts as "big enough to gate on" cannot be a number of microseconds: a
// busy machine inflates every absolute measurement, and a gate that quietly
// stops applying is worse than no gate. The eight-call workload carries almost
// no dependency work, so its rebuild is this run's floor, and three times that
// floor is where the effect has to be measurable. Recomputed every run.
let floorMicros = 0;
const MIN_GATED_CASES = 6;

interface Prepared {
  readonly testCase: Case;
  readonly changes: readonly PlanChange[];
  readonly samples: number[];
  /** The batch this transaction produces - what a rebuild has to plan from scratch. */
  readonly after: ToolCall[];
  readonly rebuildSamples: number[];
  /** The diagnostics of the first measured update, for the report. */
  observed?: UpdateDiagnostics;
}

/** The list of calls a transaction leaves behind, computed on the host. */
function applyToCalls(calls: readonly ToolCall[], changes: readonly PlanChange[]): ToolCall[] {
  const after = [...calls];
  for (const change of changes) {
    if (change.type === "append") {
      after.push(change.call);
      continue;
    }
    const at = Number(change.key.slice(1)); // entries are keyed k0, k1, ... in batch order
    if (change.type === "remove") after.splice(at, 1);
    else after[at] = change.call;
  }
  return after;
}

for (const workload of DATASETS) {
  const calls = buildCalls(workload);
  const planner = await EffectPlan.create();
  const entries = calls.map((call, index) => ({ key: `k${index}`, call }));
  const limits = planner.getEngineInfo().limits;
  const maxCalls = limits.maxCalls;

  // One session per timed update: an update mutates the session it lands on, so
  // a second update on the same session would measure a different transaction.
  // Each is created the moment before it is needed and disposed the moment
  // after, because sessions left alive hold their checkpoint copies across the
  // whole run and slow the very rebuild they are being compared against.
  const prepared: Prepared[] = [];
  const accessCount = calls.reduce((total, call) => total + (call.effect?.accesses?.length ?? 0), 0);
  for (const testCase of CASES) {
    // The same preflight the session itself makes, run here so an inapplicable
    // case is reported rather than thrown: a batch at either ceiling cannot grow.
    const adds = testCase.adds ?? 0;
    if (calls.length + adds > maxCalls || accessCount + adds > limits.maxAccesses) {
      process.stdout.write(
        `${workload.name.padEnd(9)} ${testCase.name.padEnd(20)} skipped: the batch is already at a ceiling
`,
      );
      continue;
    }
    const changes = testCase.changes(calls);
    prepared.push({
      testCase,
      changes,
      after: applyToCalls(calls, changes),
      samples: [],
      rebuildSamples: [],
    });
  }

  const freshSession = (): PlanSession => planner.createSession(entries, { checkpointInterval: 64 });

  for (let i = 0; i < WARMUP; i++) {
    planner.planBatch(calls);
    for (const p of prepared) {
      planner.planBatch(p.after);
      const warm = freshSession();
      warm.applyChanges({ expectedVersion: 1, changes: p.changes });
      warm.dispose();
    }
  }

  // The rebuild and the update alternate, and the rebuild is of the batch the
  // transaction *produces*: a caller choosing a rebuild over an update plans the
  // new batch, not the old one, so comparing an append against a rebuild of the
  // pre-append list compares two different amounts of work.
  const rebuildSamples: number[] = [];
  for (let round = 0; round < ITERATIONS; round++) {
    rebuildSamples.push(one(() => planner.planBatch(calls)));
    for (const p of prepared) {
      p.rebuildSamples.push(one(() => planner.planBatch(p.after)));
      const session = freshSession();
      p.samples.push(one(() => session.applyChanges({ expectedVersion: 1, changes: p.changes })));
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
    const rebuilt = stats(p.rebuildSamples);
    const observed = p.observed!;
    const strategy = observed.strategy;
    const reason = observed.reason ?? "";
    const reused = observed.reusedPrefixCalls;
    const ratio = (update.p50 / rebuilt.p50) * 100;

    // The gate: when a session *does* reuse a prefix, the update must be cheaper
    // than planning the resulting batch from scratch. A batch of 96 calls or more
    // always qualifies; a 64-call batch qualifies only while it costs at least
    // three times this run's eight-call rebuild, because below that the fixed
    // bookkeeping of an update is the same order as the whole rebuild and
    // asserting either direction would be asserting the harness's own overhead.
    //
    // Cases that reused nothing are excluded on principle, not to make the gate
    // pass: a transaction that falls back to a full rebuild *is* a rebuild, plus
    // the cost of discovering that, so "cheaper than a rebuild" is false for it by
    // construction. Their parity is measured and reported instead.
    const dense = calls.length >= 96 || rebuild.p50 >= floorMicros;
    const reusedSomething = reused > 0;
    const gated = testCase.where === "end" && calls.length >= 64 && dense && reusedSomething;
    const passed = gated ? ratio <= 100 : true;
    if (gated) gatedCases++;
    if (!passed) failures++;
    if (!reusedSomething && rebuilt.p50 > 0) fallbackParity.push(ratio);

    rows.push({
      name: testCase.name,
      strategy,
      reason,
      reusedPrefixCalls: reused,
      recomputedCalls: observed.recomputedCalls,
      callsAfter: p.after.length,
      updateP50Micros: update.p50,
      updateP95Micros: update.p95,
      updateMinMicros: update.min,
      rebuildBeforeP50Micros: rebuild.p50,
      rebuildP50Micros: rebuilt.p50,
      rebuildMinMicros: rebuilt.min,
      ratioPercent: ratio,
      gated,
      passed,
    });
    process.stdout.write(
      `${workload.name.padEnd(9)} ${testCase.name.padEnd(20)} ${strategy.padEnd(14)} ` +
        `update ${update.p50.toFixed(1).padStart(8)} us  rebuild ${rebuilt.p50.toFixed(1).padStart(8)} us  ` +
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
  // A hundred updates in a row on one session - the repeated-update stress the
  // specification asks for, measured and checked in the same breath. Each
  // transaction replaces the last call with a write to a rotating resource, so
  // the graph really is different every time, and the plan the session ends on
  // has to equal a fresh rebuild of the same batch.
  let successive: Record<string, unknown> | undefined;
  {
    const session = planner.createSession(entries, { checkpointInterval: 64 });
    const model: ToolCall[] = [...calls];
    const perUpdate: number[] = [];
    for (let i = 0; i < 100; i++) {
      const call = write(`replaced${i}`, `f${i % 8}`);
      perUpdate.push(
        one(() =>
          session.applyChanges({
            expectedVersion: i + 1,
            changes: [{ type: "replace", key: `k${model.length - 1}`, call }],
          }),
        ),
      );
      model[model.length - 1] = call;
    }
    const reference = planner.planBatch(model);
    const snapshot = session.current();
    const matches =
      JSON.stringify(snapshot.plan.edges) === JSON.stringify(reference.edges) &&
      JSON.stringify(snapshot.plan.execution.waves) === JSON.stringify(reference.execution.waves);
    const updateStats = stats(perUpdate);
    successive = {
      updates: perUpdate.length,
      strategy: snapshot.update.strategy,
      perUpdateP50Micros: updateStats.p50,
      perUpdateMinMicros: updateStats.min,
      totalMicros: perUpdate.reduce((total, sample) => total + sample, 0),
      matchesRebuild: matches,
    };
    session.dispose();
    if (!matches) {
      failures++;
      process.stdout.write(`${workload.name} 100 successive updates DID NOT match a fresh rebuild\n`);
    } else {
      process.stdout.write(
        `${workload.name.padEnd(9)} 100 successive    ${String(snapshot.update.strategy).padEnd(14)} ` +
          `per update ${updateStats.p50.toFixed(1).padStart(8)} us  equal to a rebuild\n`,
      );
    }
  }
  report.push({ name: workload.name, calls: workload.calls, rebuildP50Micros: rebuild.p50, cases: rows, intervals, successive });
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
      fallbackParityPercent: stats(fallbackParity),
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
process.stdout.write(
  `an update that reuses nothing costs ${stats(fallbackParity).p50.toFixed(0)}% of the rebuild it fell back to ` +
    `(min ${stats(fallbackParity).min.toFixed(0)}%, p95 ${stats(fallbackParity).p95.toFixed(0)}%, ` +
    `${fallbackParity.length} cases)\n`,
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
