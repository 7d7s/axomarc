/**
 * Phase 6 M0: how much room is there for a Phase 6 optimizer to win at all?
 *
 * Phase 6 asks whether another valid schedule would be better than the Phase 5
 * baseline. That question has a precondition nobody had checked: whether the
 * baseline is ever *not* optimal. This harness measures it. Every instance is
 * solved twice — once by the shipped engine through the public API, once by
 * `optimalMakespan`, the exhaustive serial-GS search over precedence-feasible
 * permutations from `reference-resource-scheduler.ts`, which shares nothing with
 * the engine and whose result is a true lower bound on what any heuristic can
 * produce. The difference is the entire budget Phase 6 has to work with.
 *
 * It also measures the two lower bounds Phase 6 M3 would report, computed only
 * from integers the engine already publishes, and refuses to record a bound that
 * exceeds a verified makespan: a lower bound above a schedule is a defect, not a
 * number.
 *
 * Instance size is capped at 7 calls. That is the oracle's limit, not the
 * engine's — the exhaustive search is factorial — so nothing here is evidence
 * about a 128-call batch, and the artifact says so in its own metadata rather
 * than leaving the reader to notice.
 *
 *   npm run benchmark:headroom
 */

import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import { performance } from "node:perf_hooks";
import { fileURLToPath } from "node:url";

import {
  optimalMakespan,
  seeded,
  verifySchedule,
  type ResourceInstance,
} from "./reference-resource-scheduler.js";
import {
  buildInstance,
  familySeed,
  INSTANCES_PER_FAMILY,
  MAX_CALLS,
  MIN_CALLS,
  SPECS,
  type Spec,
} from "./headroom-corpus.js";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const DIST = new URL("../../packages/core/dist/", import.meta.url);
const { EffectPlan } = (await import(new URL("index.js", DIST).href)) as typeof import("../packages/core/dist/index.js");

/** One improvable instance, recorded in full so it becomes a test fixture. */
interface Fixture {
  readonly family: string;
  readonly instance: number;
  readonly calls: number;
  readonly durations: readonly number[];
  readonly capacities: readonly number[];
  readonly demands: readonly (readonly (readonly [number, number])[])[];
  readonly edges: readonly (readonly [number, number])[];
  readonly optimum: number;
  readonly baseline: number;
  readonly gap: number;
  readonly lowerBound: number;
}

interface FamilyReport {
  readonly name: string;
  readonly note: string;
  readonly instances: number;
  readonly optimalCount: number;
  readonly pctOptimal: number;
  readonly gapHistogram: Readonly<Record<string, number>>;
  readonly worstGapTicks: number;
  readonly worstGapPct: number;
  readonly meanOptimum: number;
  readonly lowerBoundTightCount: number;
  /** Bounds above a verified makespan. Any non-zero value fails the run. */
  readonly boundDefects: number;
  /** Instances the engine called feasible but the independent checker rejected. */
  readonly unverifiedBaselines: number;
  /** Batches the engine itself refused as infeasible, which are not measured. */
  readonly infeasibilityRefusals: number;
}

const started = performance.now();
const planner = await EffectPlan.create({
  wasmBytes: new Uint8Array(readFileSync(path.join(ROOT, "packages", "core", "dist", "effectplan.wasm"))),
});

const families: FamilyReport[] = [];
const fixtures: Fixture[] = [];
let totalInstances = 0;
let totalOptimal = 0;
let totalBoundDefects = 0;
let totalUnverified = 0;

for (const [familyIndex, spec] of SPECS.entries()) {
  const random = seeded(familySeed(familyIndex));
  let optimalCount = 0;
  let infeasible = 0;
  let refusals = 0;
  let boundDefects = 0;
  let boundTight = 0;
  let worstGap = 0;
  let worstPct = 0;
  let sumOptimum = 0;
  const histogram = new Map<number, number>();
  let measured = 0;

  for (let instance = 0; instance < INSTANCES_PER_FAMILY; instance++) {
    const { durations, capacities, demands, calls, capacitiesForApi, demandsForApi } =
      buildInstance(spec, random);

    const result = planner.planResources(calls, {
      capacities: capacitiesForApi,
      demands: demandsForApi,
      durations,
    });
    if (result.outcome !== "feasible") {
      refusals += 1;
      continue;
    }
    const edges = result.plan.edges.map((e) => [e.from, e.to] as const);
    const oracleInstance: ResourceInstance = {
      calls: durations.map((d, i) => ({ duration: d, demands: demands[i]! })),
      edges,
      capacities,
    };

    // Self-check the premise: if the shipped baseline were not feasible, every
    // gap measured below would be meaningless.
    const violations = verifySchedule(oracleInstance, {
      windows: result.schedule.map((w) => [w.start, w.finish] as const),
      makespan: result.makespan,
    });
    if (violations.length > 0) infeasible += 1;

    const optimum = optimalMakespan(oracleInstance);
    const gap = result.makespan - optimum;
    measured += 1;
    sumOptimum += optimum;
    if (gap === 0) optimalCount += 1;
    histogram.set(gap, (histogram.get(gap) ?? 0) + 1);
    if (gap > worstGap) {
      worstGap = gap;
      worstPct = (100 * gap) / optimum;
    }

    // The two bounds Phase 6 would report, from published integers only:
    // LB_dep is Phase 3's critical-path duration at unlimited capacity, and
    // LB_res is Phase 5's per-resource busy time over its declared capacity.
    const lbDep = planner.planBatch(calls, { timing: { durations } }).timing!.criticalPathDuration;
    let lbRes = 0;
    for (const stat of result.statistics) {
      lbRes = Math.max(lbRes, Number((stat.busyTime + BigInt(stat.capacity) - 1n) / BigInt(stat.capacity)));
    }
    const lowerBound = Math.max(lbDep, lbRes);
    if (lowerBound > result.makespan) boundDefects += 1;
    if (lowerBound === optimum) boundTight += 1;

    if (gap > 0 && fixtures.length < 400) {
      fixtures.push({
        family: spec.name,
        instance,
        calls: durations.length,
        durations,
        capacities,
        demands,
        edges,
        optimum,
        baseline: result.makespan,
        gap,
        lowerBound,
      });
    }
  }

  totalInstances += measured;
  totalOptimal += optimalCount;
  totalBoundDefects += boundDefects;
  totalUnverified += infeasible;
  families.push({
    name: spec.name,
    note: spec.note,
    instances: measured,
    optimalCount,
    pctOptimal: measured === 0 ? 0 : (100 * optimalCount) / measured,
    gapHistogram: Object.fromEntries([...histogram.entries()].sort((a, b) => a[0] - b[0])),
    worstGapTicks: worstGap,
    worstGapPct: worstPct,
    meanOptimum: measured === 0 ? 0 : sumOptimum / measured,
    lowerBoundTightCount: boundTight,
    boundDefects,
    unverifiedBaselines: infeasible,
    infeasibilityRefusals: refusals,
  });
}

const elapsedMs = Math.round(performance.now() - started);
const artifact = {
  measuredAt: new Date().toISOString(),
  elapsedMs,
  node: process.version,
  oracle: "optimalMakespan: exhaustive serial SGS over precedence-feasible permutations",
  oracleLimit: `instances capped at ${MIN_CALLS}..${MAX_CALLS} calls because the oracle is factorial; NOT evidence about larger batches`,
  engine: planner.getEngineInfo().engineVersion,
  abi: planner.getEngineInfo().abiVersion,
  wasmBytes: readFileSync(path.join(ROOT, "packages", "core", "dist", "effectplan.wasm")).byteLength,
  baselineRanking: "Phase 5 policy: slack, then downstream dependents, then call index",
  totals: {
    instances: totalInstances,
    alreadyOptimal: totalOptimal,
    pctAlreadyOptimal: (100 * totalOptimal) / totalInstances,
    headroomInstances: totalInstances - totalOptimal,
    boundDefects: totalBoundDefects,
    unverifiedBaselines: totalUnverified,
  },
  families,
  fixtures,
};

mkdirSync(path.join(ROOT, "artifacts"), { recursive: true });
const target = path.join(ROOT, "artifacts", "optimization-headroom.json");
writeFileSync(target, `${JSON.stringify(artifact, null, 2)}\n`);

for (const f of families) {
  process.stdout.write(
    `${f.name.padEnd(17)} n=${String(f.instances).padEnd(4)} optimal ${f.pctOptimal.toFixed(1).padStart(5)}%  ` +
      `worst ${String(f.worstGapTicks).padStart(2)}t/${f.worstGapPct.toFixed(0).padStart(3)}%  ` +
      `LB==opt ${((100 * f.lowerBoundTightCount) / Math.max(1, f.instances)).toFixed(0).padStart(3)}%  ${f.note}\n`,
  );
}
process.stdout.write(
  `\n${totalInstances} instances, ${totalOptimal} already optimal ` +
    `(${((100 * totalOptimal) / totalInstances).toFixed(1)}%), ` +
    `${totalInstances - totalOptimal} with headroom; bound defects ${totalBoundDefects}, ` +
    `unverified baselines ${totalUnverified}, ${elapsedMs}ms\n`,
);
process.stdout.write(`wrote ${path.relative(ROOT, target)}\n`);

// Fail closed on the premises these numbers rest on. A bound above a verified
// schedule means the quality report would lie; a baseline the independent checker
// rejects means every gap here is fiction; and a corpus with no headroom at all
// could never satisfy the release gate that asks a benchmark family to show
// improvement, so saying so now beats letting that gate pass on an empty claim.
const failures: string[] = [];
if (totalBoundDefects > 0) failures.push(`${totalBoundDefects} lower bound(s) above a verified makespan`);
if (totalUnverified > 0) failures.push(`${totalUnverified} baseline(s) rejected by the independent checker`);
if (fixtures.length === 0) {
  failures.push("no instance in any family has headroom: nothing left for an optimizer to demonstrate");
}
if (families.some((f) => f.instances === 0)) failures.push("a family measured no instances");

for (const failure of failures) process.stderr.write(`headroom: ${failure}\n`);
if (failures.length > 0) process.exit(1);
