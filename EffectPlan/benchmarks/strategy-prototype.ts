/**
 * Phase 6 strategy prototype — can an alternative priority rule beat the shipped
 * engine on any instance that has headroom?
 *
 * This is a decision instrument, not a candidate implementation. Everything here
 * is a serial schedule generation scheme written in TypeScript, dev-only, exactly
 * like `optimalMakespan` in `reference-resource-scheduler.ts` which already
 * generates schedules here for the same reason: to measure, not to ship. Nothing
 * in this file may become the production algorithm, and what it says about a
 * priority rule's *potential* says nothing about a wasm implementation's cost.
 *
 * Three numbers per instance, from three things that share no code:
 *
 * - the engine's own makespan, from `planResources` on the shipped artifact;
 * - the exhaustive optimum, from the Phase 5 oracle — a true lower bound on any
 *   heuristic, and the only reason a win here means anything;
 * - each priority rule's serial-GS makespan, verified feasible before it counts.
 *
 * A strategy wins only against the ENGINE, never against another prototype, and
 * the engine's own schedule always stays in the portfolio — so the best-case
 * result can never be worse than what Phase 6 would ship without the baseline
 * being a candidate.
 *
 *   npm run prototype:strategies
 */

import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import { performance } from "node:perf_hooks";
import { fileURLToPath } from "node:url";

import {
  optimalMakespan,
  verifySchedule,
  type ResourceInstance,
} from "./reference-resource-scheduler.js";
import { eachInstance } from "./headroom-corpus.js";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const DIST = new URL("../../packages/core/dist/", import.meta.url);
const { EffectPlan } = (await import(new URL("index.js", DIST).href)) as typeof import("../packages/core/dist/index.js");

interface Graph {
  readonly instance: ResourceInstance;
  readonly successors: number[][];
  readonly predecessors: number[][];
}

function toGraph(
  durations: readonly number[],
  demands: readonly (readonly (readonly [number, number])[])[],
  capacities: readonly number[],
  edges: readonly (readonly [number, number])[],
): Graph {
  const count = durations.length;
  const successors: number[][] = Array.from({ length: count }, () => []);
  const predecessors: number[][] = Array.from({ length: count }, () => []);
  for (const [from, to] of edges) {
    successors[from]!.push(to);
    predecessors[to]!.push(from);
  }
  return {
    instance: { calls: durations.map((d, i) => ({ duration: d, demands: demands[i]! })), edges, capacities },
    successors,
    predecessors,
  };
}

/**
 * CPM at unlimited capacity, computed here rather than read from the engine:
 * the prototype must be able to disagree with the engine and be believed.
 * Edges in this corpus always point forward, so index order is a valid
 * topological order and relaxation converges in `count` passes.
 */
function cpm(graph: Graph): { es: number[]; slack: number[]; cp: number[] } {
  const calls = graph.instance.calls;
  const count = calls.length;
  const es = new Array<number>(count).fill(0);
  for (let pass = 0; pass < count; pass++) {
    for (let i = 0; i < count; i++) {
      for (const s of graph.successors[i]!) {
        es[s] = Math.max(es[s]!, es[i]! + calls[i]!.duration);
      }
    }
  }
  const makespan = Math.max(...es.map((e, i) => e + calls[i]!.duration));
  // Latest start is bounded by the SUCCESSORS, not the predecessors: a call may
  // not start so late that something waiting on it misses its own latest start.
  // Getting this round produces a slack that looks plausible and is wrong, which
  // is why the counts below are compared against the engine's own published slack.
  // A sink's latest START is T - D, not T. Initialising it to T is the mistake
  // this file's self-check exists for: it leaves every sink slack of D and the
  // error propagates backwards through the whole graph, quietly redefining which
  // calls look critical.
  const ls = calls.map((c) => makespan - c.duration);
  for (let pass = 0; pass < count; pass++) {
    for (let i = 0; i < count; i++) {
      for (const succ of graph.successors[i]!) {
        ls[i] = Math.min(ls[i]!, ls[succ]! - calls[i]!.duration);
      }
    }
  }
  const cp = new Array<number>(count).fill(0);
  for (let pass = 0; pass < count; pass++) {
    for (let i = count - 1; i >= 0; i--) {
      const tail = graph.successors[i]!.length === 0 ? 0 : Math.max(...graph.successors[i]!.map((s) => cp[s]!));
      cp[i] = calls[i]!.duration + tail;
    }
  }
  return { es, slack: es.map((e, i) => ls[i]! - e), cp };
}

/** Lexicographic tuple comparison. Packed arithmetic keys were tried first and
 *  are a bug generator: mismatched magnitudes silently invert a rule's order. */
function compareKeys(a: readonly number[], b: readonly number[]): number {
  for (let i = 0; i < a.length; i++) {
    if (a[i] !== b[i]) return a[i]! < b[i]! ? -1 : 1;
  }
  return 0;
}

type StrategyName = "baseline-rule" | "critical-path" | "resource-aware" | "hybrid" | "shortest-first";

const STRATEGIES: readonly StrategyName[] = ["baseline-rule", "critical-path", "resource-aware", "hybrid", "shortest-first"];

/**
 * Priority keys, smaller runs first. Every rule ends with the same tail the
 * engine uses — more dependents, then lower index — so the strategies differ only
 * in the head of the ranking, which is the thing being tested.
 */
function keyFor(
  name: StrategyName,
  i: number,
  graph: Graph,
  slack: number[],
  cp: number[],
): number[] {
  const dependents = -graph.successors[i]!.length;
  const call = graph.instance.calls[i]!;
  let bottleneck = 0;
  for (const [lane, amount] of call.demands) {
    bottleneck = Math.max(bottleneck, amount / graph.instance.capacities[lane]!);
  }
  switch (name) {
    case "baseline-rule":
      return [slack[i]!, dependents, i];
    case "critical-path":
      return [-cp[i]!, slack[i]!, dependents, i];
    case "resource-aware":
      return [-bottleneck, slack[i]!, dependents, i];
    case "hybrid":
      return [-cp[i]!, -bottleneck, slack[i]!, dependents, i];
    case "shortest-first":
      return [call.duration, slack[i]!, dependents, i];
  }
}

/**
 * Serial schedule generation: take the highest-priority call whose predecessors
 * are all placed, and put it at the earliest tick where capacity holds for its
 * whole window. Feasible by construction, verified anyway, because an unverified
 * "win" is exactly the defect Phase 6 exists to avoid.
 */
function serialSchedule(
  graph: Graph,
  key: (i: number) => number[],
): { windows: readonly (readonly [number, number])[]; makespan: number } | null {
  const count = graph.instance.calls.length;
  const finish = new Array<number | null>(count).fill(null);
  const start = new Array<number>(count).fill(0);
  const remaining = [...Array(count).keys()];
  const horizon = graph.instance.calls.reduce((s, c) => s + c.duration, 0) + 1;

  const holds = (index: number, from: number, to: number): boolean => {
    for (let tick = from; tick < to; tick++) {
      const live = new Map<number, number>();
      for (const [lane, amount] of graph.instance.calls[index]!.demands) {
        live.set(lane, (live.get(lane) ?? 0) + amount);
      }
      for (let other = 0; other < count; other++) {
        if (other === index || finish[other] === null) continue;
        if (start[other]! <= tick && tick < finish[other]!) {
          for (const [lane, amount] of graph.instance.calls[other]!.demands) {
            live.set(lane, (live.get(lane) ?? 0) + amount);
          }
        }
      }
      for (const [lane, total] of live) {
        if (total > graph.instance.capacities[lane]!) return false;
      }
    }
    return true;
  };

  while (remaining.length > 0) {
    const ready = remaining.filter((i) => graph.predecessors[i]!.every((p) => finish[p] !== null));
    if (ready.length === 0) return null;
    const pick = ready.reduce((best, i) => (compareKeys(key(i), key(best)) < 0 ? i : best));
    const duration = graph.instance.calls[pick]!.duration;
    let from = graph.predecessors[pick]!.reduce((max, p) => Math.max(max, finish[p]!), 0);
    while (!holds(pick, from, from + duration)) {
      from += 1;
      if (from > horizon) return null;
    }
    start[pick] = from;
    finish[pick] = from + duration;
    remaining.splice(remaining.indexOf(pick), 1);
  }
  return { windows: start.map((s, i) => [s, finish[i]!] as const), makespan: Math.max(...finish.map((f) => f!)) };
}

interface FamilyRow {
  readonly name: string;
  readonly instances: number;
  readonly engineOptimal: number;
  readonly headroomInstances: number;
  readonly closedByPortfolio: number;
  readonly engineSum: number;
  readonly prototypeSum: number;
  readonly optimumSum: number;
  readonly instancesWherePrototypeWorseThanEngine: number;
  readonly winsByStrategy: Readonly<Record<string, number>>;
  /** Instances where this rule ALONE is worse than the engine - the measured case for keeping the baseline a candidate. */
  readonly regressionsByStrategy: Readonly<Record<string, number>>;
}

let cpmMismatches = 0;
const started = performance.now();
const planner = await EffectPlan.create({
  wasmBytes: new Uint8Array(readFileSync(path.join(ROOT, "packages", "core", "dist", "effectplan.wasm"))),
});

const perFamily = new Map<string, FamilyRow>();
const wins: { family: string; instance: number; strategy: string; engine: number; optimum: number; achieved: number }[] = [];
let processed = 0;
let unverified = 0;
let refusals = 0;

for (const { family, index, instance } of eachInstance()) {
  const result = planner.planResources(instance.calls, {
    capacities: instance.capacitiesForApi,
    demands: instance.demandsForApi,
    durations: instance.durations,
  });
  if (result.outcome !== "feasible") {
    refusals += 1;
    continue;
  }

  const edges = result.plan.edges.map((e) => [e.from, e.to] as const);
  const graph = toGraph(instance.durations, instance.demands, instance.capacities, edges);
  const cpmOut = cpm(graph);
  const { slack, cp } = cpmOut;
  // Self-check: the prototype's own CPM must agree with the engine's published
  // earliest starts and slack, or every priority derived from them is fiction.
  for (const event of result.plan.timing!.events) {
    if (event.earliestStart !== cpmOut.es[event.callIndex] || event.slack !== slack[event.callIndex]) {
      cpmMismatches += 1;
    }
  }
  const engine = result.makespan;
  const optimum = optimalMakespan(graph.instance);

  // The portfolio always contains the engine's own schedule, so the best case
  // here is a bound on what an optimizer could claim, never a regression risk.
  let best = engine;
  const winsHere: Record<string, number> = {};
  const regressionsHere: Record<string, number> = {};
  let anyPrototypeWorse = false;
  for (const name of STRATEGIES) {
    winsHere[name] = 0;
    regressionsHere[name] = 0;
    const candidate = serialSchedule(graph, (i) => keyFor(name, i, graph, slack, cp));
    if (candidate === null) continue;
    const problems = verifySchedule(graph.instance, { windows: candidate.windows, makespan: candidate.makespan });
    if (problems.length > 0) {
      unverified += 1;
      continue;
    }
    if (candidate.makespan > engine) {
      anyPrototypeWorse = true;
      regressionsHere[name] = 1;
    }
    if (candidate.makespan < best) {
      best = candidate.makespan;
      winsHere[name] = 1;
      wins.push({ family: family.name, instance: index, strategy: name, engine, optimum, achieved: candidate.makespan });
    }
  }

  if (!perFamily.has(family.name)) {
    perFamily.set(family.name, {
      name: family.name,
      instances: 0,
      engineOptimal: 0,
      headroomInstances: 0,
      closedByPortfolio: 0,
      engineSum: 0,
      prototypeSum: 0,
      optimumSum: 0,
      instancesWherePrototypeWorseThanEngine: 0,
      winsByStrategy: {},
      regressionsByStrategy: {},
    });
  }
  const row = perFamily.get(family.name)!;
  perFamily.set(family.name, {
    ...row,
    instances: row.instances + 1,
    engineOptimal: row.engineOptimal + (engine === optimum ? 1 : 0),
    headroomInstances: row.headroomInstances + (engine > optimum ? 1 : 0),
    closedByPortfolio: row.closedByPortfolio + (engine > optimum && best === optimum ? 1 : 0),
    engineSum: row.engineSum + engine,
    prototypeSum: row.prototypeSum + best,
    optimumSum: row.optimumSum + optimum,
    instancesWherePrototypeWorseThanEngine: row.instancesWherePrototypeWorseThanEngine + (anyPrototypeWorse ? 1 : 0),
    winsByStrategy: Object.fromEntries(STRATEGIES.map((s) => [s, (row.winsByStrategy[s] ?? 0) + winsHere[s]!])),
    regressionsByStrategy: Object.fromEntries(
      STRATEGIES.map((s) => [s, (row.regressionsByStrategy[s] ?? 0) + regressionsHere[s]!]),
    ),
  });
  processed += 1;
}

const rows = [...perFamily.values()];
const byStrategy = (field: "winsByStrategy" | "regressionsByStrategy") =>
  STRATEGIES.reduce((acc, name) => {
    acc[name] = rows.reduce((s, r) => s + (r[field][name] ?? 0), 0);
    return acc;
  }, {} as Record<string, number>);
const sum = (pick: (r: FamilyRow) => number) => rows.reduce((s, r) => s + pick(r), 0);
const totals = {
  instances: processed,
  infeasibilityRefusals: refusals,
  engineOptimal: sum((r) => r.engineOptimal),
  headroomInstances: sum((r) => r.headroomInstances),
  closedByPortfolio: sum((r) => r.closedByPortfolio),
  ticksAvailable: sum((r) => r.engineSum - r.optimumSum),
  ticksSaved: sum((r) => r.engineSum - r.prototypeSum),
  instancesWhereSomePrototypeIsWorseThanEngine: sum((r) => r.instancesWherePrototypeWorseThanEngine),
  unverifiedPrototypeCandidates: unverified,
  cpmMismatchesAgainstEngine: cpmMismatches,
};

const artifact = {
  purpose: "decision instrument: does an alternative priority beat the engine where headroom exists?",
  caveat: "TypeScript serial-GS prototypes. They measure a rule's potential, NOT a wasm implementation's cost, and they are not the production algorithm.",
  strategies: STRATEGIES,
  rankingTail: "every rule shares the engine's tail: more dependents, then lower index",
  elapsedMs: Math.round(performance.now() - started),
  totals,
  regressionsByStrategy: byStrategy("regressionsByStrategy"),
  winsByStrategy: STRATEGIES.reduce((acc, name) => {
    acc[name] = sum((r) => r.winsByStrategy[name] ?? 0);
    return acc;
  }, {} as Record<string, number>),
  families: rows,
  wins: wins.slice(0, 80),
};

mkdirSync(path.join(ROOT, "artifacts"), { recursive: true });
writeFileSync(path.join(ROOT, "artifacts", "strategy-prototype.json"), `${JSON.stringify(artifact, null, 2)}\n`);

process.stdout.write(
  `${"family".padEnd(17)} ${"n".padStart(4)} ${"opt".padStart(5)} ${"room".padStart(6)} ${"closed".padStart(7)} ${"saved".padStart(6)} ${"avail".padStart(6)}\n`,
);
for (const r of rows) {
  process.stdout.write(
    `${r.name.padEnd(17)} ${String(r.instances).padStart(4)} ${String(r.engineOptimal).padStart(5)} ` +
      `${String(r.headroomInstances).padStart(6)} ${String(r.closedByPortfolio).padStart(7)} ` +
      `${String(r.engineSum - r.prototypeSum).padStart(6)} ${String(r.engineSum - r.optimumSum).padStart(6)}\n`,
  );
}
process.stdout.write(
  `\n${totals.instances} instances: engine optimal ${totals.engineOptimal}, headroom ${totals.headroomInstances}, ` +
    `closed by portfolio ${totals.closedByPortfolio}; ${totals.ticksSaved} of ${totals.ticksAvailable} ticks saved\n`,
);
process.stdout.write(`regressions by strategy: ${JSON.stringify(byStrategy("regressionsByStrategy"))}
`);
process.stdout.write(`wins by strategy: ${JSON.stringify(artifact.winsByStrategy)}\n`);
process.stdout.write(`instances where some prototype rule is worse than the engine: ${totals.instancesWhereSomePrototypeIsWorseThanEngine}\n`);

if (cpmMismatches > 0) {
  process.stderr.write(`strategy-prototype: ${cpmMismatches} CPM value(s) disagree with the engine's own timing region
`);
  process.exit(1);
}
if (unverified > 0) {
  process.stderr.write(`strategy-prototype: ${unverified} candidate(s) failed the independent checker\n`);
  process.exit(1);
}
process.stdout.write(`wrote artifacts/strategy-prototype.json in ${artifact.elapsedMs}ms\n`);
