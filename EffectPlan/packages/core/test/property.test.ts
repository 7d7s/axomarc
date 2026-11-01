/**
 * Property tests over randomly generated batches. The oracle in
 * benchmarks/reference-planner.ts is the reference; nothing here re-implements
 * the conflict algorithm in a way production code depends on.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import * as fc from "fast-check";

import { EffectPlan } from "../src/index.js";
import type { EffectMode, ResourceAccess, ToolCall } from "../src/index.js";
import { access, call, wasmBytes } from "./support.js";
import { isAcyclic, referencePlan, transitiveClosure, pairSet } from "../../../benchmarks/reference-planner.js";
import { referenceLevels } from "../../../benchmarks/reference-levels.js";
import { referenceCpm } from "../../../benchmarks/reference-cpm.js";

const open = (): Promise<EffectPlan> => EffectPlan.create({ wasmBytes: wasmBytes() });

/** Fixed seed keeps a failing run reproducible; fast-check reports the shrink. */
const CONFIG = { numRuns: 400, seed: 20261031 };

const NAMESPACE = fc.constantFrom("fs", "db", "net", "http");
const RESOURCE_ID = fc.constantFrom("f1", "f2", "f3", "a:bc", "ab:c", "tenant/x");
const ACCESS_MODE = fc.constantFrom("read" as const, "write" as const);

const accessArb = fc
  .record({ resource: fc.record({ namespace: NAMESPACE, id: RESOURCE_ID }), mode: ACCESS_MODE })
  .map(({ resource, mode }): ResourceAccess => ({ resource, mode }));

const shapeArb = fc
  .record({ mode: fc.constantFrom("pure", "resources", "exclusive", "unknown"), accesses: fc.array(accessArb, { maxLength: 4 }) })
  .map(({ mode, accesses }): ToolCall => {
    // Keep the batch expressible: pure declares nothing, resources declares
    // at least one access. The placeholder id is replaced per batch position.
    if (mode === "pure") return { id: "", effect: { version: 1, mode, accesses: [] } };
    const list = accesses.length > 0 ? accesses : [{ resource: { namespace: "fs", id: "f1" }, mode: "read" as const }];
    return { id: "", effect: { version: 1, mode: mode as EffectMode, accesses: list } };
  });

/** Durations that keep a generated batch inside the tick range. */
const durationsArb = fc.array(fc.integer({ min: 1, max: 1000 }), { maxLength: 24 });

const batchArb = fc
  .array(shapeArb, { maxLength: 24 })
  .map((shapes) => shapes.map((shape, i) => ({ ...shape, id: `c${i}` })));

/** Resource keys as the reference planner sees them. */
function keysOf(call: ToolCall): Set<string> {
  const out = new Set<string>();
  for (const access of call.effect?.accesses ?? []) {
    out.add(`${access.resource.namespace.length}:${access.resource.namespace}${access.resource.id.length}:${access.resource.id}`);
  }
  return out;
}

describe("planner properties", () => {
  it("always produces the same plan for the same input", async () => {
    const plan = await open();
    fc.assert(
      fc.property(batchArb, (calls) => {
        const first = JSON.stringify(plan.planBatch(calls).edges);
        const second = JSON.stringify(plan.planBatch(calls).edges);
        assert.equal(first, second, "planning the same batch twice must be byte-identical");
      }),
      CONFIG,
    );
  });

  it("produces an acyclic, forward-pointing graph", async () => {
    const plan = await open();
    fc.assert(
      fc.property(batchArb, (calls) => {
        const result = plan.planBatch(calls);
        const pairs = result.edges.map((e) => [e.from, e.to] as const);
        assert.ok(result.edges.every((e) => e.from < e.to), "every record points forward");
        assert.ok(isAcyclic(calls.length, pairs), "the graph must be acyclic");
        assert.ok(
          pairs.length <= 2 * (result.statistics.callCount + result.statistics.resourceAccessCount),
          "the 2*(N+A) event bound must hold",
        );
        assert.equal(result.edgeCount, pairs.length);
        assert.equal(result.statistics.dependencyCount, pairs.length);
      }),
      CONFIG,
    );
  });

  it("matches the reference planner exactly, pair for pair", async () => {
    const plan = await open();
    fc.assert(
      fc.property(batchArb, (calls) => {
        const result = plan.planBatch(calls);
        const engine = result.edges.map((e) => [e.from, e.to] as const);
        const oracle = referencePlan(calls);
        const engineClosure = transitiveClosure(calls.length, engine).map(([f, t]) => `${f}>${t}`);
        const oracleClosure = transitiveClosure(calls.length, oracle).map(([f, t]) => `${f}>${t}`);
        assert.deepEqual(engineClosure, oracleClosure, "the covering graph must equal the closure");
      }),
      CONFIG,
    );
  });

  it("misses no conflict: every all-pairs conflict is reachable", async () => {
    const plan = await open();
    fc.assert(
      fc.property(batchArb, (calls) => {
        const result = plan.planBatch(calls);
        const reachable = new Set(
          transitiveClosure(calls.length, result.edges.map((e) => [e.from, e.to] as const)).map(([f, t]) => `${f}>${t}`),
        );
        for (const [from, to] of referencePlan(calls)) {
          assert.ok(reachable.has(`${from}>${to}`), `missed conflict ${from}->${to}`);
        }
      }),
      CONFIG,
    );
  });

  it("never orders two reads of the same resource, with no barriers present", async () => {
    const plan = await open();
    const readOnly = batchArb.map((calls) =>
      calls
        .filter((c) => c.effect?.mode === "resources")
        .map((c, i) => ({
          id: `r${i}`,
          effect: {
            version: 1 as const,
            mode: "resources" as const,
            accesses: (c.effect?.accesses ?? []).map((a) => ({ resource: a.resource, mode: "read" as const })),
          },
        }))
        .filter((c) => (c.effect?.accesses?.length ?? 0) > 0),
    );
    fc.assert(
      fc.property(readOnly, (calls) => {
        const result = plan.planBatch(calls);
        assert.equal(result.edgeCount, 0, `read-only batch produced ${result.edgeCount} edges`);
      }),
      { ...CONFIG, numRuns: 200 },
    );
  });

  it("keeps calls over disjoint resources unordered", async () => {
    const plan = await open();
    fc.assert(
      fc.property(batchArb, (calls) => {
        const result = plan.planBatch(calls);
        const keys = calls.map(keysOf);
        const global = calls.map((c) => c.effect?.mode === "unknown" || c.effect?.mode === "exclusive");
        for (const edge of result.edges) {
          if (global[edge.from]! || global[edge.to]!) continue;
          if (edge.resource === 0) assert.fail(`barrier record between ordinary calls ${edge.from}->${edge.to}`);
          const shared = [...keys[edge.from]!].some((k) => keys[edge.to]!.has(k));
          assert.ok(shared, `edge ${edge.from}->${edge.to} joins disjoint resources`);
        }
      }),
      CONFIG,
    );
  });

  it("treats unknown and exclusive effects as global barriers", async () => {
    const plan = await open();
    fc.assert(
      fc.property(batchArb, (calls) => {
        const result = plan.planBatch(calls);
        const reachable = pairSet(transitiveClosure(calls.length, result.edges.map((e) => [e.from, e.to] as const)));
        calls.forEach((call, index) => {
          const mode = call.effect?.mode;
          if (mode !== "unknown" && mode !== "exclusive") return;
          for (let earlier = 0; earlier < index; earlier++) {
            assert.ok(reachable.has(`${earlier}>${index}`), `barrier ${index} does not wait for ${earlier}`);
          }
          for (let later = index + 1; later < calls.length; later++) {
            assert.ok(reachable.has(`${index}>${later}`), `call ${later} does not wait behind barrier ${index}`);
          }
        });
      }),
      CONFIG,
    );
  });

  it("keeps independent resource identities independent", async () => {
    const plan = await open();
    // Two disjoint resource pools, no barrier calls at all.
    const poolArb = fc.integer({ min: 2, max: 20 }).chain((n) =>
      fc.array(
        fc.record({
          pool: fc.constantFrom("left", "right"),
          slot: fc.integer({ min: 0, max: 5 }),
          mode: ACCESS_MODE,
        }),
        { minLength: n, maxLength: n },
      ),
    );
    fc.assert(
      fc.property(poolArb, (specs) => {
        const calls: ToolCall[] = specs.map((spec, i) => ({
          id: `p${i}`,
          effect: {
            version: 1,
            mode: "resources",
            accesses: [{ resource: { namespace: spec.pool, id: `r${spec.slot}` }, mode: spec.mode }],
          },
        }));
        const result = plan.planBatch(calls);
        for (const edge of result.edges) {
          assert.equal(
            calls[edge.from]!.effect?.accesses![0]!.resource.namespace,
            calls[edge.to]!.effect?.accesses![0]!.resource.namespace,
            `edge ${edge.from}->${edge.to} crosses the resource pools`,
          );
        }
      }),
      CONFIG,
    );
  });
});

describe("execution level properties", () => {
  it("every edge rises: L(from) < L(to)", async () => {
    const plan = await open();
    fc.assert(
      fc.property(batchArb, (calls) => {
        const { edges, execution } = plan.planBatch(calls);
        for (const edge of edges) {
          assert.ok(
            execution.levelByCall[edge.from]! < execution.levelByCall[edge.to]!,
            `edge ${edge.from}->${edge.to} does not rise: ${execution.levelByCall}`,
          );
        }
      }),
      CONFIG,
    );
  });

  it("roots sit at zero and non-roots at one over their deepest predecessor", async () => {
    const plan = await open();
    fc.assert(
      fc.property(batchArb, (calls) => {
        const result = plan.planBatch(calls);
        const levels = result.execution.levelByCall;
        for (let call = 0; call < calls.length; call++) {
          const incoming = result.edges.filter((edge) => edge.to === call).map((edge) => edge.from);
          if (incoming.length === 0) {
            assert.equal(levels[call], 0, `root call ${call} must sit at level 0`);
          } else {
            const expected = 1 + Math.max(...incoming.map((pred) => levels[pred]!));
            assert.equal(levels[call], expected, `call ${call}: expected ${expected}`);
          }
        }
      }),
      CONFIG,
    );
  });

  it("the waves partition the batch exactly once", async () => {
    const plan = await open();
    fc.assert(
      fc.property(batchArb, (calls) => {
        const { execution } = plan.planBatch(calls);
        const seen = new Set<number>();
        let total = 0;
        execution.waves.forEach((wave, level) => {
          for (const node of wave) {
            assert.ok(!seen.has(node), `call ${node} appears in two waves`);
            seen.add(node);
            total++;
            assert.equal(execution.levelByCall[node], level, "the wave index is the level");
          }
          assert.ok(wave.every((node, i) => i === 0 || wave[i - 1]! < node), "input order inside a wave");
        });
        assert.equal(total, calls.length, `sum of wave sizes must equal ${calls.length}`);
        assert.equal(execution.waveCount, execution.waves.length);
      }),
      CONFIG,
    );
  });

  it("levels stay inside the batch", async () => {
    const plan = await open();
    fc.assert(
      fc.property(batchArb, (calls) => {
        const { execution } = plan.planBatch(calls);
        for (const level of execution.levelByCall) {
          assert.ok(level >= 0 && level < Math.max(calls.length, 1), `level ${level} out of range`);
        }
        assert.ok(execution.waveCount <= calls.length, "one wave per level, at most one per call");
      }),
      CONFIG,
    );
  });

  it("identical input gives byte-identical levels and waves", async () => {
    const plan = await open();
    fc.assert(
      fc.property(batchArb, (calls) => {
        const first = JSON.stringify(plan.planBatch(calls).execution);
        const second = JSON.stringify(plan.planBatch(calls).execution);
        assert.equal(first, second);
      }),
      CONFIG,
    );
  });

  it("the single-pass levels equal the independent Kahn levels", async () => {
    const plan = await open();
    fc.assert(
      fc.property(batchArb, (calls) => {
        const result = plan.planBatch(calls);
        const oracle = referenceLevels(calls.length, referencePlan(calls));
        assert.deepEqual([...result.execution.levelByCall], oracle);
      }),
      CONFIG,
    );
  });

  it("barriers keep their wave to themselves", async () => {
    const plan = await open();
    fc.assert(
      fc.property(batchArb, (calls) => {
        const result = plan.planBatch(calls);
        const barrier = calls.map((c) => c.effect?.mode === "unknown" || c.effect?.mode === "exclusive");
        result.execution.waves.forEach((wave) => {
          if (wave.some((node) => barrier[node]!)) {
            assert.equal(wave.length, 1, `a barrier shares wave ${JSON.stringify(wave)}`);
          }
        });
      }),
      CONFIG,
    );
  });
});

describe("critical path properties", () => {
  /** One generated batch plus a duration for every call in it. */
  const timedArb = fc.tuple(batchArb, fc.integer({ min: 1, max: 1000 })).map(
    ([calls, seed]) => [calls, calls.map((_, index) => 1 + ((index * 37 + seed * 13) % 900))] as const,
  );

  it("every edge respects both timing passes", async () => {
    const plan = await open();
    fc.assert(
      fc.property(timedArb, ([calls, durations]) => {
        const result = plan.planBatch(calls, { timing: { durations } });
        const events = result.timing!.events;
        for (const edge of result.edges) {
          assert.ok(
            events[edge.from]!.earliestFinish <= events[edge.to]!.earliestStart,
            `edge ${edge.from}->${edge.to} breaks earliest-time precedence`,
          );
          assert.ok(
            events[edge.from]!.latestFinish <= events[edge.to]!.latestStart,
            `edge ${edge.from}->${edge.to} breaks latest-time precedence`,
          );
        }
      }),
      CONFIG,
    );
  });

  it("roots start at zero and every other call after its latest predecessor", async () => {
    const plan = await open();
    fc.assert(
      fc.property(timedArb, ([calls, durations]) => {
        const result = plan.planBatch(calls, { timing: { durations } });
        const events = result.timing!.events;
        for (let index = 0; index < calls.length; index++) {
          const incoming = result.edges.filter((edge) => edge.to === index).map((edge) => edge.from);
          if (incoming.length === 0) {
            assert.equal(events[index]!.earliestStart, 0, `root call ${index}`);
          } else {
            const expected = Math.max(...incoming.map((pred) => events[pred]!.earliestFinish));
            assert.equal(events[index]!.earliestStart, expected, `call ${index} earliest start`);
          }
        }
      }),
      CONFIG,
    );
  });

  it("the makespan is the last finish and every sink ends at it", async () => {
    const plan = await open();
    fc.assert(
      fc.property(timedArb, ([calls, durations]) => {
        const result = plan.planBatch(calls, { timing: { durations } });
        const timing = result.timing!;
        assert.equal(
          timing.criticalPathDuration,
          Math.max(0, ...timing.events.map((event) => event.earliestFinish)),
        );
        for (const event of timing.events) {
          const isSink = !result.edges.some((edge) => edge.from === event.callIndex);
          if (isSink) assert.equal(event.latestFinish, timing.criticalPathDuration, `sink ${event.callIndex}`);
        }
      }),
      CONFIG,
    );
  });

  it("slack is non-negative and the derived fields hold", async () => {
    const plan = await open();
    fc.assert(
      fc.property(timedArb, ([calls, durations]) => {
        const timing = plan.planBatch(calls, { timing: { durations } }).timing!;
        for (const event of timing.events) {
          assert.ok(event.slack >= 0, `negative slack on call ${event.callIndex}`);
          assert.equal(event.latestStart, event.earliestStart + event.slack);
          assert.equal(event.earliestFinish, event.earliestStart + event.duration);
          assert.equal(event.latestFinish, event.latestStart + event.duration);
          assert.equal(event.critical, event.slack === 0);
        }
        assert.deepEqual(
          timing.criticalEvents,
          timing.events.filter((event) => event.critical).map((event) => event.callIndex),
        );
        if (calls.length > 0) assert.ok(timing.criticalEvents.length > 0, "a plan has a critical call");
      }),
      CONFIG,
    );
  });

  it("the concurrency envelope is consistent with the schedule", async () => {
    const plan = await open();
    fc.assert(
      fc.property(timedArb, ([calls, durations]) => {
        const timing = plan.planBatch(calls, { timing: { durations } }).timing!;
        assert.equal(timing.totalWork, durations.reduce((sum, value) => sum + value, 0));
        assert.ok(timing.peakConcurrency <= calls.length);
        assert.ok(timing.peakConcurrency >= (calls.length === 0 ? 0 : 1));
        if (timing.criticalPathDuration > 0) {
          assert.equal(timing.averageParallelism, timing.totalWork / timing.criticalPathDuration);
          assert.ok(timing.averageParallelism! <= timing.peakConcurrency + 1e-9, "average cannot exceed peak");
        } else {
          assert.equal(timing.averageParallelism, null);
        }
      }),
      CONFIG,
    );
  });

  it("timing is deterministic and independent of the batch before it", async () => {
    const plan = await open();
    fc.assert(
      fc.property(timedArb, ([calls, durations]) => {
        const first = JSON.stringify(plan.planBatch(calls, { timing: { durations } }).timing);
        plan.planBatch([call("between", access("fs", "between", "write"))], { timing: { durations: [999] } });
        const second = JSON.stringify(plan.planBatch(calls, { timing: { durations } }).timing);
        assert.equal(first, second, "the same batch must analyse identically");
      }),
      CONFIG,
    );
  });

  it("the reference CPM agrees on every number, per call", async () => {
    const plan = await open();
    fc.assert(
      fc.property(timedArb, ([calls, durations]) => {
        const timing = plan.planBatch(calls, { timing: { durations } }).timing!;
        const reference = referenceCpm(calls.length, referencePlan(calls), durations);
        assert.deepEqual(timing.events.map((event) => event.earliestStart), reference.earliestStart);
        assert.deepEqual(timing.events.map((event) => event.earliestFinish), reference.earliestFinish);
        assert.deepEqual(timing.events.map((event) => event.latestStart), reference.latestStart);
        assert.deepEqual(timing.events.map((event) => event.latestFinish), reference.latestFinish);
        assert.deepEqual(timing.events.map((event) => event.slack), reference.slack);
        assert.deepEqual(timing.events.map((event) => event.critical), reference.critical);
        assert.equal(timing.criticalPathDuration, reference.makespan);
        assert.equal(timing.totalWork, reference.totalWork);
        assert.equal(timing.peakConcurrency, reference.peakConcurrency);
      }),
      CONFIG,
    );
  });
});
