/**
 * Parity against the independent O(N^2) reference planner. The engine emits a
 * sparse covering graph, the oracle emits every conflicting pair, so parity is
 * transitive-closure equality plus a soundness check that no engine edge is
 * spurious — and execution-level parity against the Kahn oracle in
 * benchmarks/reference-levels.ts.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { EffectPlan } from "../src/index.js";
import type { ToolCall } from "../src/index.js";
import { access, modeCall, wasmBytes } from "./support.js";
import { pairSet, referencePlan, transitiveClosure } from "../../../benchmarks/reference-planner.js";
import { referenceLevels } from "../../../benchmarks/reference-levels.js";
import { referenceCpm } from "../../../benchmarks/reference-cpm.js";

const open = (): Promise<EffectPlan> => EffectPlan.create({ wasmBytes: wasmBytes() });

/** A labelled batch: id per call plus the shape that produced it. */
function label(calls: readonly ToolCall[]): string {
  return calls.map((c, i) => {
    const effect = c.effect;
    if (!effect) return `${i}?`;
    if (effect.mode !== "resources") return `${i}${effect.mode[0]}`;
    const terms = (effect.accesses ?? [])
      .map((a) => `${a.resource.namespace}:${a.resource.id}${a.mode === "write" ? "W" : "R"}`)
      .join("+");
    return `${i}[${terms}]`;
  }).join(" ");
}

function assertParity(plan: EffectPlan, calls: readonly ToolCall[], context = ""): void {
  const result = plan.planBatch(calls);
  const engine = result.edges.map((e) => [e.from, e.to] as const);
  const oracle = referencePlan(calls);
  const where = `${label(calls)} ${context}`;

  // Soundness: the engine may only order pairs the oracle says must be ordered.
  const expected = pairSet(oracle);
  for (const [from, to] of engine) {
    assert.ok(expected.has(`${from}>${to}`), `spurious edge ${from}->${to} in ${where}`);
  }
  // Completeness: the sparse graph must reach everything the all-pairs graph does.
  const engineClosure = transitiveClosure(calls.length, engine);
  const oracleClosure = transitiveClosure(calls.length, oracle);
  assert.deepEqual(
    engineClosure.map(([f, t]) => `${f}>${t}`),
    oracleClosure.map(([f, t]) => `${f}>${t}`),
    `closure mismatch for ${where}`,
  );

  // Every record points forward and every successor is emitted in order.
  for (const edge of result.edges) {
    assert.ok(edge.from < edge.to, `backward edge in ${where}`);
    assert.ok(edge.resource >= 0 && edge.resource <= 1024, `resource index in ${where}`);
    assert.ok(edge.reason >= 1 && edge.reason <= 6, `reason code in ${where}`);
  }
  const successors = result.edges.map((e) => e.to);
  assert.deepEqual(successors, [...successors].sort((a, b) => a - b), `emission order in ${where}`);

  // Execution levels: the engine's single forward pass must agree, call for
  // call, with the independent Kahn traversal over the oracle's pair set.
  assert.deepEqual(
    [...result.execution.levelByCall],
    referenceLevels(calls.length, oracle),
    `levels disagree for ${where}`,
  );
  const partition = result.execution.waves.flat();
  assert.equal(partition.length, calls.length, `waves must partition the batch: ${where}`);

  // Timing: the same batch, with a duration per call, against the independent
  // Kahn-based reference. Deterministic durations so a failure is reproducible.
  const durations = calls.map((_, index) => 1 + ((index * 7 + calls.length * 3) % 11));
  const timed = plan.planBatch(calls, { timing: { durations } });
  const analysis = timed.timing!;
  const reference = referenceCpm(calls.length, oracle, durations);
  assert.deepEqual(
    [
      analysis.events.map((event) => event.earliestStart),
      analysis.events.map((event) => event.earliestFinish),
      analysis.events.map((event) => event.latestStart),
      analysis.events.map((event) => event.latestFinish),
      analysis.events.map((event) => event.slack),
      analysis.events.map((event) => event.critical),
      analysis.criticalPathDuration,
      analysis.totalWork,
      analysis.peakConcurrency,
    ],
    [
      reference.earliestStart,
      reference.earliestFinish,
      reference.latestStart,
      reference.latestFinish,
      reference.slack,
      reference.critical,
      reference.makespan,
      reference.totalWork,
      reference.peakConcurrency,
    ],
    `timing disagrees for ${where}`,
  );
}

/** The exhaustive alphabet: one call shape out of the documented effect modes. */
const SHAPES: Array<() => ToolCall> = [
  () => modeCall("s", "pure"),
  () => ({ id: "s" }),
  () => modeCall("s", "exclusive"),
  () => modeCall("s", "unknown"),
  () => ({ id: "s", effect: { version: 1, mode: "resources", accesses: [access("fs", "r1", "read")] } }),
  () => ({ id: "s", effect: { version: 1, mode: "resources", accesses: [access("fs", "r1", "write")] } }),
  () => ({ id: "s", effect: { version: 1, mode: "resources", accesses: [access("fs", "r2", "read")] } }),
  () => ({ id: "s", effect: { version: 1, mode: "resources", accesses: [access("fs", "r2", "write")] } }),
  () => ({
    id: "s",
    effect: {
      version: 1,
      mode: "resources",
      accesses: [access("fs", "r1", "read"), access("fs", "r2", "write"), access("fs", "r1", "write")],
    },
  }),
];

/** Every batch of exactly `depth` calls over the shape alphabet. */
function* batches(depth: number, prefix: ToolCall[] = []): Generator<ToolCall[]> {
  if (prefix.length === depth) {
    yield prefix.map((c, i) => ({ ...c, id: `c${i}` }));
    return;
  }
  for (const shape of SHAPES) yield* batches(depth, [...prefix, shape()]);
}

describe("reference parity", () => {
  it("agrees on the documented conflict cases", async () => {
    const plan = await open();
    const cases: ToolCall[][] = [
      [],
      [modeCall("a", "pure")],
      [{ id: "a" }],
      [{ id: "a", effect: { version: 1, mode: "resources", accesses: [access("fs", "f", "read")] } }],
      [
        { id: "a", effect: { version: 1, mode: "resources", accesses: [access("fs", "f1", "read")] } },
        { id: "b", effect: { version: 1, mode: "resources", accesses: [access("fs", "f1", "read")] } },
      ],
      [
        { id: "a", effect: { version: 1, mode: "resources", accesses: [access("fs", "f1", "write")] } },
        { id: "b", effect: { version: 1, mode: "resources", accesses: [access("fs", "f1", "read")] } },
        { id: "c", effect: { version: 1, mode: "resources", accesses: [access("fs", "f1", "write")] } },
      ],
      [
        { id: "a", effect: { version: 1, mode: "resources", accesses: [access("fs", "f1", "read"), access("db", "t", "write")] } },
        modeCall("b", "exclusive"),
        { id: "c", effect: { version: 1, mode: "resources", accesses: [access("db", "t", "read")] } },
      ],
      [
        modeCall("a", "unknown"),
        { id: "b", effect: { version: 1, mode: "resources", accesses: [access("fs", "f9", "read")] } },
        modeCall("c", "unknown"),
        { id: "d", effect: { version: 1, mode: "resources", accesses: [access("fs", "f9", "write")] } },
      ],
    ];
    for (const calls of cases) assertParity(plan, calls, "(curated)");
  });

  it("agrees on every batch of up to three calls over the full shape alphabet", async () => {
    const plan = await open();
    let checked = 0;
    for (const depth of [0, 1, 2, 3]) {
      for (const calls of batches(depth)) {
        assertParity(plan, calls, `(exhaustive n=${depth})`);
        checked++;
      }
    }
    assert.equal(checked, 1 + 9 + 9 ** 2 + 9 ** 3);
  });

  it("agrees on every batch of four calls over the full shape alphabet", async () => {
    const plan = await open();
    let checked = 0;
    for (const calls of batches(4)) {
      assertParity(plan, calls, "(exhaustive n=4)");
      checked++;
    }
    assert.equal(checked, 9 ** 4);
  });

  it("orders every pair of calls with an unknown effect between them", async () => {
    const plan = await open();
    const calls: ToolCall[] = [
      { id: "a", effect: { version: 1, mode: "resources", accesses: [access("fs", "x", "write")] } },
      { id: "b", effect: { version: 1, mode: "resources", accesses: [access("fs", "y", "read")] } },
      modeCall("c", "unknown"),
      { id: "d", effect: { version: 1, mode: "resources", accesses: [access("fs", "z", "write")] } },
    ];
    assertParity(plan, calls, "(barrier sandwich)");
    const closure = new Set(transitiveClosure(4, plan.planBatch(calls).edges.map((e) => [e.from, e.to] as const)).map(([f, t]) => `${f}>${t}`));
    // The unknown call is reachable from everything before it and reaches everything after it.
    assert.ok(closure.has("0>2") && closure.has("1>2"), "barrier drains earlier work");
    assert.ok(closure.has("2>3"), "later work waits behind the barrier");
    assert.ok(!closure.has("0>1"), "disjoint resources x and y stay unordered");
  });
});
