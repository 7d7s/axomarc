/**
 * The public capacity-resource surface: declaring capacity, demanding it, and
 * reading back the placement, the diagnostics and the statistics.
 *
 * These tests drive the shipped artifact through the package's own API, so
 * they cover the whole path a caller takes: the wrapper's validation, the
 * encoding of the declarations, the engine's two resource entry points, and
 * the decoding of what they wrote.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { EffectPlan, EffectPlanError, RESOURCE_DIAGNOSTIC } from "../src/index.js";
import type { EffectPlanErrorCode, ResourceOptions } from "../src/index.js";
import { modeCall, wasmBytes } from "./support.js";

const open = (): Promise<EffectPlan> => EffectPlan.create({ wasmBytes: wasmBytes() });

/** Asserts a thrown EffectPlanError carries the expected code. */
function rejectsCode(action: () => unknown, code: EffectPlanErrorCode): EffectPlanError {
  let caught: unknown;
  try {
    action();
  } catch (error) {
    caught = error;
  }
  assert.ok(caught instanceof EffectPlanError, `expected an EffectPlanError, got ${String(caught)}`);
  const error = caught as EffectPlanError;
  assert.equal(error.code, code, `unexpected code: ${error.message}`);
  assert.match(error.message, new RegExp(`^${code}: `));
  return error;
}

/** A batch of pure calls, so the only thing that can order them is capacity. */
function calls(...ids: string[]) {
  return ids.map((id) => modeCall(id, "pure"));
}

const lane = (capacity: number): ResourceOptions["capacities"] => [{ key: "lane", capacity }];

describe("planResources", () => {
  it("places calls that contend for a lane, and says how long each ran", async () => {
    const plan = await open();
    const result = plan.planResources(calls("first", "second"), {
      capacities: lane(1),
      demands: [[{ resource: "lane", amount: 1 }], [{ resource: "lane", amount: 1 }]],
      durations: [3, 4],
      unit: "ms",
    });

    assert.equal(result.outcome, "feasible");
    assert.deepEqual(
      result.schedule.map((entry) => [entry.callId, entry.start, entry.finish]),
      // One unit of capacity means one call at a time, and the longer call
      // goes first: its slack is zero, the other's is one. The rows stay in
      // batch order, so the placement order is read off the start times.
      [["first", 4, 7], ["second", 0, 4]],
    );
    assert.equal(result.makespan, 7);
    assert.equal(result.deferrals, 1, "the second call had to wait");
    assert.equal(result.statistics.length, 1);
    assert.equal(result.statistics[0]?.key, "lane");
    assert.equal(result.statistics[0]?.capacity, 1);
    assert.equal(result.statistics[0]?.peak, 1);
    assert.equal(result.statistics[0]?.busyTime, 7n, "4 ticks plus 3, each holding one unit");
  });

  it("names the call that had to wait, and the tick it waited at", async () => {
    const plan = await open();
    const result = plan.planResources(calls("first", "second"), {
      capacities: lane(1),
      demands: [[{ resource: "lane", amount: 1 }], [{ resource: "lane", amount: 1 }]],
      durations: [3, 4],
    });
    assert.equal(result.diagnostics.length, 1);
    const diagnostic = result.diagnostics[0]!;
    assert.equal(diagnostic.kind, RESOURCE_DIAGNOSTIC.contention);
    assert.equal(diagnostic.call, "first", "the call with the slack was the one that waited");
    assert.equal(diagnostic.resource, "lane");
    assert.equal(diagnostic.value, 0, "it waited at tick zero");
  });

  it("runs calls in parallel when the capacity allows it", async () => {
    const plan = await open();
    const result = plan.planResources(calls("a", "b"), {
      capacities: lane(2),
      demands: [[{ resource: "lane", amount: 1 }], [{ resource: "lane", amount: 1 }]],
      durations: [5, 5],
    });
    assert.equal(result.outcome, "feasible");
    assert.deepEqual(
      result.schedule.map((entry) => [entry.start, entry.finish]),
      [[0, 5], [0, 5]],
      "two units of capacity, two calls at once",
    );
    assert.equal(result.makespan, 5);
    assert.equal(result.deferrals, 0);
    assert.equal(result.statistics[0]?.peak, 2);
    assert.equal(result.statistics[0]?.busyTime, 10n);
  });

  it("reports an impossible demand as infeasible, with the call and the resource named", async () => {
    const plan = await open();
    const result = plan.planResources(calls("train"), {
      capacities: lane(2),
      demands: [[{ resource: "lane", amount: 5 }]],
      durations: [3],
    });

    assert.equal(result.outcome, "infeasible");
    assert.deepEqual(result.schedule, [], "nothing is placed when the batch cannot run");
    assert.equal(result.makespan, 0);
    assert.equal(result.diagnostics.length, 1);
    const diagnostic = result.diagnostics[0]!;
    assert.equal(diagnostic.kind, RESOURCE_DIAGNOSTIC.insufficientCapacity);
    assert.equal(diagnostic.call, "train");
    assert.equal(diagnostic.callIndex, 0);
    assert.equal(diagnostic.resource, "lane");
    assert.equal(diagnostic.value, 5);
  });

  it("still returns the dependency plan when the capacity verdict is infeasible", async () => {
    const plan = await open();
    const result = plan.planResources(calls("a", "b"), {
      capacities: lane(1),
      demands: [[{ resource: "lane", amount: 1 }], [{ resource: "lane", amount: 9 }]],
      durations: [1, 1],
    });
    assert.equal(result.outcome, "infeasible");
    assert.equal(result.plan.callCount, 2);
    assert.equal(result.plan.timing?.criticalPathDuration, 1);
  });

  it("plans two unrelated batches back to back on one planner", async () => {
    // The regression this pins: the stateless path used to plan through the
    // resumable entry point, so the second batch inherited the first one's
    // checkpoint and was refused as stale. Two calls with the *same* batch
    // passed, which is why the determinism test never noticed.
    const plan = await open();
    const first = plan.planResources(calls("a", "b", "c"), {
      capacities: lane(2),
      demands: [[{ resource: "lane", amount: 1 }], [], [{ resource: "lane", amount: 2 }]],
      durations: [3, 2, 4],
    });
    const second = plan.planResources(calls("x", "y"), {
      capacities: lane(1),
      demands: [[{ resource: "lane", amount: 1 }], [{ resource: "lane", amount: 1 }]],
      durations: [2, 5],
    });
    assert.equal(first.outcome, "feasible");
    assert.equal(second.outcome, "feasible");
    assert.equal(second.makespan, 7, "the second batch is planned whole, not resumed");
    assert.deepEqual(
      // Batch order, not placement order: y (duration 5, slack 0) goes first and
      // x waits for the lane. The rows stay where the calls were declared.
      second.schedule.map((entry) => [entry.callId, entry.start, entry.finish]),
      [["x", 5, 7], ["y", 0, 5]],
      "the longer call goes first and the shorter one waits for the lane",
    );
  });

  it("interleaves checks and placements without disturbing either", async () => {
    const plan = await open();
    const options: ResourceOptions = {
      capacities: lane(1),
      demands: [[{ resource: "lane", amount: 1 }], [{ resource: "lane", amount: 1 }]],
      durations: [2, 2],
    };
    assert.equal(plan.checkResources(calls("a", "b"), options).outcome, "feasible");
    assert.equal(plan.planResources(calls("a", "b"), options).outcome, "feasible");
    assert.equal(plan.checkResources(calls("a", "b"), options).outcome, "feasible");
    const refused = plan.planResources(calls("a", "b"), {
      ...options,
      demands: [[{ resource: "lane", amount: 4 }], []],
      durations: [2, 2],
    });
    assert.equal(refused.outcome, "infeasible");
    assert.equal(refused.diagnostics[0]?.kind, RESOURCE_DIAGNOSTIC.insufficientCapacity);
  });

  it("gives the same answer every time", async () => {
    const plan = await open();
    const options: ResourceOptions = {
      capacities: [{ key: "gpu", capacity: 2 }],
      demands: [[{ resource: "gpu", amount: 1 }], [{ resource: "gpu", amount: 2 }], []],
      durations: [4, 4, 4],
    };
    const first = plan.planResources(calls("a", "b", "c"), options);
    const second = plan.planResources(calls("a", "b", "c"), options);
    assert.deepEqual(second.schedule, first.schedule);
    assert.equal(second.makespan, first.makespan);
    assert.deepEqual(second.statistics, first.statistics);
  });

  it("plans the same dependency graph as a batch without capacity", async () => {
    const plan = await open();
    const batch = calls("a", "b", "c");
    const plain = plan.planBatch(batch);
    const withResources = plan.planResources(batch, {
      capacities: lane(4),
      durations: [1, 1, 1],
    });
    assert.deepEqual(
      withResources.plan.edges.map((edge) => [edge.from, edge.to]),
      plain.edges.map((edge) => [edge.from, edge.to]),
      "the capacity verdict must not change the graph",
    );
  });

  it("refuses declarations the engine would refuse anyway", async () => {
    const plan = await open();
    rejectsCode(
      () => plan.planResources(calls("a"), { capacities: [{ key: "lane", capacity: 0 }], durations: [1] }),
      "EP_INVALID_INPUT",
    );
    rejectsCode(
      () => plan.planResources(calls("a"), { capacities: [{ key: "lane", capacity: 1.5 }], durations: [1] }),
      "EP_INVALID_INPUT",
    );
    rejectsCode(
      () =>
        plan.planResources(calls("a"), {
          capacities: [],
          demands: [[{ resource: "lane", amount: 1 }]],
          durations: [1],
        }),
      "EP_INVALID_INPUT",
    );
    rejectsCode(
      () =>
        plan.planResources(calls("a"), {
          capacities: [{ key: "lane", capacity: 1 }, { key: "lane", capacity: 2 }],
          durations: [1],
        }),
      "EP_DUPLICATE_KEY",
    );
    rejectsCode(
      () =>
        plan.planResources(calls("a"), {
          capacities: lane(1),
          demands: [[{ resource: "other", amount: 1 }]],
          durations: [1],
        }),
      "EP_INVALID_INPUT",
    );
    rejectsCode(
      () =>
        plan.planResources(calls("a"), {
          capacities: lane(1),
          demands: [[{ resource: "lane", amount: 0 }]],
          durations: [1],
        }),
      "EP_INVALID_INPUT",
    );
    rejectsCode(
      () => plan.planResources(calls("a", "b"), { capacities: lane(1), demands: [[]], durations: [1, 1] }),
      "EP_INVALID_INPUT",
    );
    rejectsCode(
      () => plan.planResources(calls("a"), { capacities: lane(1) }),
      "EP_INVALID_INPUT",
    );
  });

  it("refuses more capacity resources than the engine can hold", async () => {
    const plan = await open();
    const many = Array.from({ length: 65 }, (_, index) => ({ key: `r${index}`, capacity: 1 }));
    rejectsCode(
      () => plan.planResources(calls("a"), { capacities: many, durations: [1] }),
      "EP_LIMIT_EXCEEDED",
    );
  });
});

describe("checkResources", () => {
  it("answers whether the demands can be met, without a clock", async () => {
    const plan = await open();
    const ok = plan.checkResources(calls("a", "b"), {
      capacities: lane(1),
      demands: [[{ resource: "lane", amount: 1 }], [{ resource: "lane", amount: 1 }]],
    });
    assert.equal(ok.outcome, "feasible", "one unit each, one unit of capacity: runnable, one after the other");
    assert.deepEqual(ok.schedule, []);
    assert.deepEqual(ok.diagnostics, []);

    const refused = plan.checkResources(calls("a"), {
      capacities: lane(2),
      demands: [[{ resource: "lane", amount: 5 }]],
    });
    assert.equal(refused.outcome, "infeasible");
    assert.equal(refused.diagnostics[0]?.kind, RESOURCE_DIAGNOSTIC.insufficientCapacity);
    assert.equal(refused.diagnostics[0]?.call, "a");
  });

  it("refuses a demand that names a resource nobody declared", async () => {
    const plan = await open();
    rejectsCode(
      () =>
        plan.checkResources(calls("a"), {
          capacities: lane(1),
          demands: [[{ resource: "other", amount: 1 }]],
        }),
      "EP_INVALID_INPUT",
    );
  });
});
