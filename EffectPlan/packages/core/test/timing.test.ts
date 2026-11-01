/**
 * Duration-aware scheduling through the public API: validation, decoding,
 * derived fields, backward compatibility, boundary semantics and agreement with
 * the independent reference implementation.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { EffectPlan, EffectPlanError } from "../src/index.js";
import type { ToolCall } from "../src/index.js";
import { access, call, modeCall, wasmBytes } from "./support.js";
import { loadEngine } from "../src/loader.js";
import { referenceCpm } from "../../../benchmarks/reference-cpm.js";
import { referencePlan } from "../../../benchmarks/reference-planner.js";

const open = (): Promise<EffectPlan> => EffectPlan.create({ wasmBytes: wasmBytes() });

async function rejectsCode(action: () => unknown, code: string, phrase?: RegExp): Promise<EffectPlanError> {
  let caught: unknown;
  try {
    await action();
  } catch (error) {
    caught = error;
  }
  assert.ok(caught instanceof EffectPlanError, `expected an EffectPlanError, got ${String(caught)}`);
  const error = caught as EffectPlanError;
  assert.equal(error.code, code, `unexpected code: ${error.message}`);
  assert.match(error.message, new RegExp(`^${code}: `));
  if (phrase) assert.match(error.message, phrase);
  return error;
}

/** The engine's own graph for this shape: a fan-out, a fan-in, then a tail. */
const diamond = (): ToolCall[] => [
  call("a", access("fs", "f1", "write")),
  call("b", access("fs", "f1", "read")),
  call("c", access("fs", "f1", "read")),
  call("d", access("fs", "f1", "write")),
  call("e", access("fs", "f1", "read")),
];

const times = (plan: ReturnType<EffectPlan["planBatch"]>["timing"]): Array<[number, number, number]> =>
  (plan?.events ?? []).map((event) => [event.earliestStart, event.latestStart, event.slack]);

describe("timing input validation", () => {
  it("requires one duration per call", async () => {
    const planner = await open();
    const calls = diamond();
    await rejectsCode(
      () => planner.planBatch(calls, { timing: { durations: [1, 2, 3] } }),
      "EP_INVALID_INPUT",
      /3 entries for 5 calls/,
    );
    await rejectsCode(
      () => planner.planBatch(calls, { timing: { durations: [1, 2, 3, 4, 5, 6] } }),
      "EP_INVALID_INPUT",
      /6 entries for 5 calls/,
    );
  });

  it("refuses durations that are not positive integers", async () => {
    const planner = await open();
    const bad: Array<[unknown, RegExp]> = [
      [0, /must be positive/],
      [-4, /must be positive/],
      [1.5, /must be an integer/],
      [Number.NaN, /must be an integer/],
      [Number.POSITIVE_INFINITY, /must be an integer/],
      [Number.MAX_SAFE_INTEGER * 2, /must be an integer/],
      ["4", /must be a number/],
      [null, /must be a number/],
      [undefined, /must be a number/],
      [true, /must be a number/],
    ];
    for (const [value, phrase] of bad) {
      await rejectsCode(
        () => planner.planBatch([call("a", access("fs", "f1", "write"))], { timing: { durations: [value] as unknown as number[] } }),
        "EP_INVALID_INPUT",
        phrase,
      );
    }
  });

  it("refuses a duration the engine cannot represent", async () => {
    const planner = await open();
    await rejectsCode(
      () => planner.planBatch([call("a", access("fs", "f1", "write"))], {
        timing: { durations: [0x1_0000_0000] },
      }),
      "EP_LIMIT_EXCEEDED",
      /exceeds 4294967295 ticks/,
    );
    // The largest representable duration is accepted.
    const plan = planner.planBatch([call("a", access("fs", "f1", "write"))], {
      timing: { durations: [0xffffffff] },
    });
    assert.equal(plan.timing?.criticalPathDuration, 0xffffffff);
  });

  it("refuses a bad unit instead of defaulting it", async () => {
    const planner = await open();
    await rejectsCode(
      () => planner.planBatch([call("a", access("fs", "f1", "write"))], {
        timing: { durations: [1], unit: "" },
      }),
      "EP_INVALID_INPUT",
      /timing\.unit/,
    );
    await rejectsCode(
      () => planner.planBatch([call("a", access("fs", "f1", "write"))], {
        timing: { durations: [1], unit: "x".repeat(33) },
      }),
      "EP_INVALID_INPUT",
      /timing\.unit/,
    );
    // A default unit is named so the result is never ambiguous.
    const defaulted = planner.planBatch([call("a", access("fs", "f1", "write"))], {
      timing: { durations: [1] },
    });
    assert.equal(defaulted.timing!.unit, "ticks");
  });

  it("rejects a duration array of the wrong shape", async () => {
    const planner = await open();
    await rejectsCode(
      () => planner.planBatch([call("a", access("fs", "f1", "write"))], { timing: { durations: "4" as unknown as number[] } }),
      "EP_INVALID_INPUT",
      /array-like/,
    );
    await rejectsCode(
      () => planner.planBatch([call("a", access("fs", "f1", "write"))], { timing: { durations: null as never } }),
      "EP_INVALID_INPUT",
      /array-like/,
    );
  });

  it("never lets an invalid duration become a free operation", async () => {
    const planner = await open();
    const calls = diamond();
    await rejectsCode(
      () => planner.planBatch(calls, { timing: { durations: [2, 5, 2, 0, 4] } }),
      "EP_INVALID_INPUT",
      /duration 3 must be positive/,
    );
    // The same batch without timing still plans: nothing was mutated in place.
    assert.equal(planner.planBatch(calls).edgeCount, 6);
  });

  it("does not echo a caller identifier in a timing failure", async () => {
    const planner = await open();
    const calls = [call("top-secret-call", access("tenant-a", "private-file", "write"))];
    const error = await rejectsCode(
      () => planner.planBatch(calls, { timing: { durations: [0] } }),
      "EP_INVALID_INPUT",
    );
    for (const secret of ["tenant-a", "private-file", "top-secret-call"]) {
      assert.ok(!error.message.includes(secret), `leaked ${secret}: ${error.message}`);
    }
  });
});

describe("timing analysis", () => {
  it("plans an empty batch with nothing to time", async () => {
    const planner = await open();
    const plan = planner.planBatch([], { timing: { durations: [] } });
    const timing = plan.timing!;
    assert.equal(timing.totalWork, 0);
    assert.equal(timing.criticalPathDuration, 0);
    assert.equal(timing.peakConcurrency, 0);
    assert.equal(timing.averageParallelism, null, "no duration, no ratio");
    assert.deepEqual(timing.events, []);
    assert.deepEqual(timing.criticalEvents, []);
    assert.deepEqual(plan.execution.waves, []);
  });

  it("reports a single call as its own critical path", async () => {
    const planner = await open();
    const plan = planner.planBatch([call("a", access("fs", "f1", "write"))], {
      timing: { durations: [7], unit: "hours" },
    });
    const timing = plan.timing!;
    assert.equal(timing.unit, "hours", "the caller's unit is echoed, never converted");
    assert.equal(timing.criticalPathDuration, 7);
    assert.equal(timing.totalWork, 7);
    assert.equal(timing.peakConcurrency, 1);
    assert.equal(timing.averageParallelism, 1, "one call of seven hours is exactly serial");
    assert.deepEqual(timing.events.map((e) => [e.callIndex, e.duration, e.slack, e.critical]), [[0, 7, 0, true]]);
  });

  it("sums a chain and never overlaps it", async () => {
    const planner = await open();
    const calls = [
      call("a", access("fs", "f1", "write")),
      call("b", access("fs", "f1", "read")),
      call("c", access("fs", "f1", "write")),
    ];
    const timing = planner.planBatch(calls, { timing: { durations: [2, 3, 4] } }).timing!;
    assert.equal(timing.criticalPathDuration, 9);
    assert.equal(timing.totalWork, 9);
    assert.equal(timing.peakConcurrency, 1);
    assert.equal(timing.averageParallelism, 1);
    assert.deepEqual(times(timing), [[0, 0, 0], [2, 2, 0], [5, 5, 0]]);
    assert.deepEqual(timing.criticalEvents, [0, 1, 2]);
  });

  it("takes the longest branch for independent calls", async () => {
    const planner = await open();
    const calls = [
      call("a", access("fs", "f1", "write")),
      call("b", access("fs", "f2", "write")),
      call("c", access("fs", "f3", "write")),
    ];
    const timing = planner.planBatch(calls, { timing: { durations: [5, 9, 2] } }).timing!;
    assert.equal(timing.criticalPathDuration, 9);
    assert.equal(timing.totalWork, 16);
    assert.equal(timing.peakConcurrency, 3);
    assert.equal(timing.averageParallelism, 16 / 9);
    // Three independent branches all start at zero; the 9-hour branch is
// the only one with nothing to spare.
    assert.deepEqual(times(timing), [[0, 4, 4], [0, 0, 0], [0, 7, 7]]);
    assert.deepEqual(timing.criticalEvents, [1]);
  });

  it("produces the worked example from the specification", async () => {
    const planner = await open();
    const timing = planner.planBatch(diamond(), { timing: { durations: [2, 5, 2, 3, 4] } }).timing!;
    assert.equal(timing.criticalPathDuration, 14, "2 + 5 + 3 + 4");
    assert.equal(timing.totalWork, 16);
    assert.equal(timing.peakConcurrency, 2);
    assert.equal(timing.averageParallelism, 16 / 14);
    assert.deepEqual(timing.events, [
      Object.freeze({
        callIndex: 0, duration: 2, earliestStart: 0, earliestFinish: 2,
        latestStart: 0, latestFinish: 2, slack: 0, critical: true,
      }),
      Object.freeze({
        callIndex: 1, duration: 5, earliestStart: 2, earliestFinish: 7,
        latestStart: 2, latestFinish: 7, slack: 0, critical: true,
      }),
      Object.freeze({
        callIndex: 2, duration: 2, earliestStart: 2, earliestFinish: 4,
        latestStart: 5, latestFinish: 7, slack: 3, critical: false,
      }),
      Object.freeze({
        callIndex: 3, duration: 3, earliestStart: 7, earliestFinish: 10,
        latestStart: 7, latestFinish: 10, slack: 0, critical: true,
      }),
      Object.freeze({
        callIndex: 4, duration: 4, earliestStart: 10, earliestFinish: 14,
        latestStart: 10, latestFinish: 14, slack: 0, critical: true,
      }),
    ]);
    assert.deepEqual(timing.criticalEvents, [0, 1, 3, 4]);
    // The waves from Phase 2 and the timing from Phase 3 describe one graph.
    assert.deepEqual([...planner.planBatch(diamond(), { timing: { durations: [2, 5, 2, 3, 4] } }).execution.levelByCall], [0, 1, 1, 2, 3]);
  });

  // The specification's prose gives this example a makespan of 10 and a critical
  // path A -> B -> D, which is only true if E is not downstream of D. Both
  // readings are correct CPM for their own graph, so both are executed here and
  // the difference is documented instead of quietly resolved: see
  // docs/SCHEDULING-MODEL.md section 7.
  it("produces the specification's example with E outside the chain", async () => {
    const planner = await open();
    const calls = [
      call("a", access("fs", "f1", "write")),
      call("b", access("fs", "f1", "read")),
      call("c", access("fs", "f1", "read")),
      call("d", access("fs", "f1", "write")),
      call("e", access("fs", "f2", "read")),
    ];
    const timing = planner.planBatch(calls, { timing: { durations: [2, 5, 2, 3, 4] } }).timing!;
    assert.equal(timing.criticalPathDuration, 10, "2 + 5 + 3, with E not downstream");
    assert.equal(timing.totalWork, 16);
    assert.equal(timing.peakConcurrency, 3, "b, c and e are all alive at tick 2");
    assert.equal(timing.averageParallelism, 16 / 10);
    assert.deepEqual(
      timing.events.map((event) => [event.earliestStart, event.latestStart, event.slack, event.critical]),
      [
        [0, 0, 0, true],
        [2, 2, 0, true],
        [2, 5, 3, false],
        [7, 7, 0, true],
        [0, 6, 6, false],
      ],
    );
    assert.deepEqual(timing.criticalEvents, [0, 1, 3]);
  });

  it("keeps Phase 2 levels and Phase 3 timing consistent with each other", async () => {
    const planner = await open();
    const calls = diamond();
    const plan = planner.planBatch(calls, { timing: { durations: [2, 5, 2, 3, 4] } });
    for (const [index, level] of plan.execution.levelByCall.entries()) {
      const timing = plan.timing!.events[index]!;
      assert.ok(timing.earliestStart >= level, `call ${index}: time must not precede its level`);
    }
    for (const edge of plan.edges) {
      assert.ok(plan.timing!.events[edge.from]!.earliestFinish <= plan.timing!.events[edge.to]!.earliestStart);
    }
  });

  it("agrees with the independent reference implementation", async () => {
    const planner = await open();
    const cases: Array<[ToolCall[], number[]]> = [
      [diamond(), [2, 5, 2, 3, 4]],
      [diamond(), [4, 3, 2, 5, 2]],
      [
        [
          call("a", access("fs", "f1", "write")),
          call("b", access("fs", "f1", "read")),
          call("c", access("fs", "f2", "write")),
          call("d", access("fs", "f2", "read")),
        ],
        [5, 5, 1, 1],
      ],
      [[modeCall("x", "unknown"), call("a", access("fs", "f1", "write")), modeCall("y", "exclusive")], [3, 4, 5]],
      [[], []],
    ];
    for (const [calls, durations] of cases) {
      const timing = planner.planBatch(calls, { timing: { durations } }).timing!;
      const reference = referenceCpm(calls.length, referencePlan(calls), durations);
      assert.deepEqual(timing.events.map((e) => e.earliestStart), reference.earliestStart, "earliest start");
      assert.deepEqual(timing.events.map((e) => e.earliestFinish), reference.earliestFinish, "earliest finish");
      assert.deepEqual(timing.events.map((e) => e.latestStart), reference.latestStart, "latest start");
      assert.deepEqual(timing.events.map((e) => e.latestFinish), reference.latestFinish, "latest finish");
      assert.deepEqual(timing.events.map((e) => e.slack), reference.slack, "slack");
      assert.deepEqual(timing.events.map((e) => e.critical), reference.critical, "criticality");
      assert.equal(timing.criticalPathDuration, reference.makespan, "makespan");
      assert.equal(timing.totalWork, reference.totalWork, "total work");
      assert.equal(timing.peakConcurrency, reference.peakConcurrency, "peak concurrency");
    }
  });

  it("refuses a makespan that would overflow the tick range", async () => {
    const planner = await open();
    const calls = [
      call("a", access("fs", "f1", "write")),
      call("b", access("fs", "f1", "read")),
    ];
    await rejectsCode(
      () => planner.planBatch(calls, { timing: { durations: [0xffff_ffff, 1] } }),
      "EP_LIMIT_EXCEEDED",
      /engine status 121/,
    );
    // The refused batch leaves the timing region unreadable rather than stale.
    await rejectsCode(
      () => planner.planBatch(calls, { timing: { durations: [0xffff_ffff, 1] } }),
      "EP_LIMIT_EXCEEDED",
    );
    // A following batch without timing is unaffected.
    assert.equal(planner.planBatch(calls).edgeCount, 1);
  });

  it("treats intervals as half-open", async () => {
    const planner = await open();
    // [0,5) then [5,9): one call at a time.
    const touching = planner.planBatch(
      [call("a", access("fs", "f1", "write")), call("b", access("fs", "f1", "write"))],
      { timing: { durations: [5, 4] } },
    );
    assert.equal(touching.timing!.peakConcurrency, 1, "a finish at t and a start at t do not overlap");

    // Two calls that genuinely overlap.
    const overlapping = planner.planBatch(
      [call("a", access("fs", "f1", "write")), call("b", access("fs", "f2", "write"))],
      { timing: { durations: [5, 9] } },
    );
    assert.equal(overlapping.timing!.peakConcurrency, 2);
  });

  it("is frozen, stable and independent of the previous batch", async () => {
    const planner = await open();
    const calls = diamond();
    const first = planner.planBatch(calls, { timing: { durations: [2, 5, 2, 3, 4] } });
    assert.ok(Object.isFrozen(first.timing));
    assert.ok(Object.isFrozen(first.timing!.events));
    for (const event of first.timing!.events) assert.ok(Object.isFrozen(event));
    // A different batch, then the same batch again: identical analysis.
    planner.planBatch([call("z", access("fs", "zz", "write"))], { timing: { durations: [100] } });
    const again = planner.planBatch(calls, { timing: { durations: [2, 5, 2, 3, 4] } });
    assert.deepEqual(again.timing, first.timing);
    // The earlier handle still reports its own numbers, not the later batch's.
    assert.equal(first.timing!.criticalPathDuration, 14);
    assert.equal(first.timing!.events.length, 5);
  });

  it("keeps working when the host grows the linear memory", async () => {
    const engine = await loadEngine({ wasmBytes: wasmBytes() });
    const planner = await open();
    const calls = diamond();
    const durations = [2, 5, 2, 3, 4];
    const before = planner.planBatch(calls, { timing: { durations } }).timing!;
    engine.memory.grow(1);
    const after = planner.planBatch(calls, { timing: { durations } }).timing!;
    assert.deepEqual(after, before, "the same batch must analyse the same way");
  });

  it("reports no timing at all when none was asked for", async () => {
    const planner = await open();
    const plan = planner.planBatch(diamond());
    assert.equal(plan.timing, undefined);
    assert.equal("criticalPathDuration" in (plan.timing ?? {}), false);
    // Levels still exist: Phase 2 behaviour is unchanged by Phase 3.
    assert.deepEqual(plan.execution.levelByCall, [0, 1, 1, 2, 3]);
  });
});

