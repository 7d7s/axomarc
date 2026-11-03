/**
 * Session tests: an incremental plan must equal a plan of the same batch built
 * from scratch.
 *
 * The comparison is on the dependency *structure* — which call must follow
 * which — plus the waves and the schedule. The resource index on an edge is not
 * compared, and deliberately so: a session freezes its interning table so that
 * a preserved prefix keeps meaning the same resources after a removal, while a
 * fresh plan numbers resources by first appearance. Two plans can therefore
 * describe the same graph with different resource numbers, and the resource
 * numbers are checked separately for stability.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { EffectPlan } from "../src/index.js";
import type { PlanChange, PlanSession, ToolCall } from "../src/index.js";
import { wasmBytes } from "./support.js";

/** The published artifact, not a development build. */
const open = (): Promise<EffectPlan> => EffectPlan.create({ wasmBytes: wasmBytes() });

const call = (id: string, resource: string, mode: "read" | "write") => ({
  id,
  effect: {
    version: 1 as const,
    mode: "resources" as const,
    accesses: [{ resource: { namespace: "fs", id: resource }, mode }],
  },
});

/** A batch with a chain, a branch and an independent tail. */
function batch(count: number) {
  const calls = [];
  for (let i = 0; i < count; i++) {
    calls.push(call(`c${i}`, `f${i % 3}`, i % 2 === 0 ? "write" : "read"));
  }
  return calls;
}


/** The same edits, applied to a plain list, so the expected plan is derived. */
interface Model {
  readonly entries: Array<{ key: string; call: ToolCall; duration?: number | undefined }>;
}

function applyTo(model: Model, change: PlanChange): void {
  if (change.type === "append") {
    model.entries.push({
      key: change.key,
      call: change.call,
      duration: change.duration,
    });
    return;
  }
  const at = model.entries.findIndex((entry) => entry.key === change.key);
  if (change.type === "remove") {
    model.entries.splice(at, 1);
    return;
  }
  model.entries[at] = { key: change.key, call: change.call, duration: change.duration };
}

interface Shape {
  readonly edges: readonly (readonly [number, number])[];
  readonly levels: readonly number[];
  readonly makespan: number;
  readonly peak: number;
  readonly work: number;
  readonly critical: readonly number[];
}

function shape(plan: ReturnType<EffectPlan["planBatch"]>): Shape {
  return {
    edges: plan.edges.map((edge) => [edge.from, edge.to] as const),
    levels: plan.execution.levelByCall,
    makespan: plan.timing?.criticalPathDuration ?? -1,
    peak: plan.timing?.peakConcurrency ?? -1,
    work: plan.timing?.totalWork ?? -1,
    critical: plan.timing?.criticalEvents ?? [],
  };
}

/** The same batch, planned from scratch by the stateless API. */
async function fresh(calls: readonly ToolCall[], durations?: number[]) {
  const planner = await open();
  const plan = planner.planBatch(
    calls,
    durations === undefined ? {} : { timing: { durations } },
  );
  return shape(plan);
}

describe("plan sessions", () => {
  it("plans the first batch exactly as the stateless API does", async () => {
    const planner = await open();
    const calls = batch(6);
    const durations = [2, 5, 3, 1, 4, 2];
    const session = planner.createSession(
      calls.map((c, index) => ({ key: `k${index}`, call: c, duration: durations[index] })),
      { timing: { unit: "ms" } },
    );
    const snapshot = session.current();
    assert.equal(snapshot.version, 1);
    assert.deepEqual(snapshot.keys, ["k0", "k1", "k2", "k3", "k4", "k5"]);
    assert.equal(snapshot.callCount, 6);
    const expected = await fresh(calls, durations);
    assert.deepEqual(shape(snapshot.plan).edges, expected.edges);
    assert.deepEqual(shape(snapshot.plan).levels, expected.levels);
  });

  it("appends a call and matches a fresh plan of the longer batch", async () => {
    const planner = await open();
    const calls = batch(6);
    const durations = [2, 5, 3, 1, 4, 2];
    const session = planner.createSession(
      calls.map((c, index) => ({ key: `k${index}`, call: c, duration: durations[index] })),
      { timing: { unit: "ms" } },
    );
    const added = call("c6", "f0", "read");
    const next = session.applyChanges({
      expectedVersion: 1,
      changes: [{ type: "append", key: "k6", call: added, duration: 7 }],
    });
    assert.equal(next.version, 2);
    assert.equal(next.update.strategy, "append");
    assert.equal(next.update.reusedPrefixCalls, 6);
    assert.equal(next.update.recomputedCalls, 1);
    const expected = await fresh([...calls, added], [...durations, 7]);
    assert.deepEqual(shape(next.plan).edges, expected.edges);
    assert.deepEqual(shape(next.plan).levels, expected.levels);
    assert.equal(next.plan.timing!.criticalPathDuration, expected.makespan);
    assert.equal(next.plan.timing!.peakConcurrency, expected.peak);
    assert.deepEqual(next.plan.timing!.criticalEvents, expected.critical);
  });

  it("replaces a call and matches a fresh plan of the changed batch", async () => {
    const planner = await open();
    const calls = batch(6);
    const durations = [2, 5, 3, 1, 4, 2];
    const session = planner.createSession(
      calls.map((c, index) => ({ key: `k${index}`, call: c, duration: durations[index] })),
      { timing: { unit: "ms" } },
    );
    const replacement = call("c2", "f1", "write");
    const changed = [...calls];
    changed[2] = replacement;
    const next = session.applyChanges({
      expectedVersion: 1,
      changes: [{ type: "replace", key: "k2", call: replacement, duration: 3 }],
    });
    assert.equal(next.version, 2);
    const expected = await fresh(changed, durations);
    assert.deepEqual(shape(next.plan).edges, expected.edges);
    assert.deepEqual(shape(next.plan).levels, expected.levels);
    assert.equal(next.plan.timing!.criticalPathDuration, expected.makespan);
  });

  it("removes a call and matches a fresh plan of the shorter batch", async () => {
    const planner = await open();
    const calls = batch(6);
    const durations = [2, 5, 3, 1, 4, 2];
    const session = planner.createSession(
      calls.map((c, index) => ({ key: `k${index}`, call: c, duration: durations[index] })),
      { timing: { unit: "ms" } },
    );
    const shorter = calls.filter((_, index) => index !== 2);
    const next = session.applyChanges({
      expectedVersion: 1,
      changes: [{ type: "remove", key: "k2" }],
    });
    assert.equal(next.callCount, 5);
    assert.deepEqual(next.keys, ["k0", "k1", "k3", "k4", "k5"], "keys keep their identity");
    const expected = await fresh(shorter, [2, 5, 1, 4, 2]);
    assert.deepEqual(shape(next.plan).edges, expected.edges);
    assert.deepEqual(shape(next.plan).levels, expected.levels);
    assert.equal(next.plan.timing!.criticalPathDuration, expected.makespan);
  });

  it("keeps the resource numbers of a prefix stable when a call is removed", async () => {
    const planner = await open();
    // The first call touches a resource nothing else does.
    const calls = [call("a", "only-a", "write"), call("b", "f", "read"), call("c", "f", "write")];
    const session = planner.createSession(
      calls.map((c, index) => ({ key: `k${index}`, call: c })),
      { checkpointInterval: 1 },
    );
    const before = session.current().plan.edges.map((edge) => edge.resource);
    const next = session.applyChanges({
      expectedVersion: 1,
      changes: [{ type: "remove", key: "k0" }],
    });
    const after = next.plan.edges.map((edge) => edge.resource);
    // The removed call's resource is still numbered, and the others keep their
    // numbers: a fresh plan would renumber them, which is exactly what the
    // frozen table exists to prevent.
    assert.ok(after.every((resource) => before.includes(resource)), "resource numbers moved");
    const expected = await fresh(calls.slice(1));
    assert.deepEqual(shape(next.plan).edges, expected.edges);
  });

  it("reuses the prefix when a change is near the end", async () => {
    const planner = await open();
    const calls = batch(20);
    const session = planner.createSession(
      calls.map((c, index) => ({ key: `k${index}`, call: c })),
      { checkpointInterval: 4 },
    );
    const replacement = call("c19", "f0", "read");
    const next = session.applyChanges({
      expectedVersion: 1,
      changes: [{ type: "replace", key: "k19", call: replacement }],
    });
    assert.equal(next.update.strategy, "suffix-rebuild");
    assert.ok(next.update.reusedPrefixCalls >= 16, "the prefix should have been reused");
    assert.equal(next.update.recomputedCalls, 20 - next.update.reusedPrefixCalls);
    const changed = [...calls];
    changed[19] = replacement;
    const expected = await fresh(changed);
    assert.deepEqual(shape(next.plan).edges, expected.edges);
  });

  it("reuses the prefix again on the update after the update", async () => {
    const planner = await open();
    const calls = batch(96);
    const session = planner.createSession(
      calls.map((c, index) => ({ key: `k${index}`, call: c })),
      { checkpointInterval: 64 },
    );
    const seen: Array<[string, number]> = [];
    for (let round = 0; round < 3; round++) {
      session.applyChanges({
        expectedVersion: session.current().version,
        changes: [{ type: "replace", key: "k95", call: call(`late${round}`, "zz", "write") }],
      });
      const update = session.current().update;
      seen.push([update.strategy, update.reusedPrefixCalls]);
    }
    // What this pins: a replanned run used to replace the whole checkpoint set
    // with only what it captured, so the second update found nothing to resume
    // from and rebuilt the batch - correctly, but at full cost, every time after
    // the first. Reuse has to survive a history of updates, not just one.
    //
    // The first update rewrites the call's dependencies, so it invalidates the
    // dependency domain and the prefix is reused only as far as the last
    // checkpoint before it. The second and third rewrite the *same*
    // dependencies - "zz", written - with a different id, so nothing the
    // conflict rules can see has changed: the dependency graph is reused whole
    // and only the clock and the demands are recomputed.
    assert.deepEqual(seen, [
      ["suffix-rebuild", 64],
      ["suffix-rebuild", 96],
      ["suffix-rebuild", 96],
    ]);
  });

  it("reuses the whole graph when only the clock moved, and equals a fresh rebuild", async () => {
    const planner = await open();
    const calls = batch(20);
    const durations = calls.map((_, index) => index + 1);
    const session = planner.createSession(
      calls.map((c, index) => ({ key: `k${index}`, call: c, duration: durations[index] })),
      { timing: { unit: "ms" }, checkpointInterval: 4 },
    );
    // The replacement declares exactly the dependencies the call already had -
    // "f1", read - so the graph cannot have changed. Only the duration moved.
    const replacement = call("c19", "f1", "read");
    const next = session.applyChanges({
      expectedVersion: 1,
      changes: [{ type: "replace", key: "k19", call: replacement, duration: 99 }],
    });
    assert.equal(next.update.invalidated, "scheduling", "a duration change is not a dependency change");
    assert.equal(
      next.update.reusedPrefixCalls,
      20,
      "nothing about the graph moved, so the whole graph was reused",
    );
    assert.equal(next.update.recomputedCalls, 0);
    // The reuse is only allowed if it produces the same answer as a rebuild.
    const changed = [...calls];
    changed[19] = replacement;
    const changedDurations = [...durations];
    changedDurations[19] = 99;
    const expected = await fresh(changed, changedDurations);
    assert.deepEqual(shape(next.plan).edges, expected.edges, "the reused graph differs from a rebuild");
    assert.deepEqual(shape(next.plan).levels, expected.levels);
    assert.equal(next.plan.timing!.criticalPathDuration, expected.makespan, "the clock differs");
    assert.equal(next.plan.timing!.totalWork, expected.work);
  });

  it("places every batch against the capacity resources the session declares", async () => {
    const planner = await open();
    // A second engine for the stateless comparison: a session and a stateless
    // batch on the same instance would fight over the arena.
    const stateless = await open();
    const calls = batch(6);
    const durations = calls.map((_, index) => index + 1);
    const session = planner.createSession(
      calls.map((c, index) => ({
        key: `k${index}`,
        call: c,
        duration: durations[index],
        demands: [{ resource: "lane", amount: 1 }],
      })),
      {
        timing: { unit: "ms" },
        resources: { capacities: [{ key: "lane", capacity: 2 }] },
      },
    );
    const snapshot = session.current();
    assert.ok(snapshot.placement, "a session that declares resources places its batches");
    assert.equal(snapshot.placement.outcome, "feasible");
    assert.equal(snapshot.resourceVersion, 1);
    // The placement has to be the same one the stateless API produces for the
    // same batch: the session must not have its own idea of how to place.
    const expected = stateless.planResources(calls, {
      capacities: [{ key: "lane", capacity: 2 }],
      demands: calls.map(() => [{ resource: "lane", amount: 1 }]),
      durations,
    });
    assert.deepEqual(snapshot.placement.schedule, expected.schedule);
    assert.equal(snapshot.placement.makespan, expected.makespan);
    assert.deepEqual(snapshot.placement.statistics, expected.statistics);
    assert.equal(snapshot.placement.quality?.provablyOptimal, expected.quality?.provablyOptimal);
  });

  it("moves the placement when a demand moves, and keeps the graph", async () => {
    const planner = await open();
    const calls = batch(4);
    const session = planner.createSession(
      calls.map((c, index) => ({
        key: `k${index}`,
        call: c,
        duration: index + 1,
        demands: [{ resource: "lane", amount: 1 }],
      })),
      {
        timing: { unit: "ms" },
        resources: { capacities: [{ key: "lane", capacity: 1 }] },
      },
    );
    const before = session.current();
    assert.ok(before.placement);
    // The same dependencies, a different demand: the scheduling domain.
    const next = session.applyChanges({
      expectedVersion: before.version,
      changes: [
        {
          type: "replace",
          key: "k3",
          call: calls[3]!,
          duration: 4,
          demands: [{ resource: "lane", amount: 1 }],
        },
      ],
    });
    assert.equal(next.update.invalidated, "scheduling");
    assert.ok(next.placement, "the placement follows the change");
    assert.equal(next.placement.makespan, before.placement.makespan);
    // The graph did not move.
    assert.deepEqual(shape(next.plan).edges, shape(before.plan).edges);
  });

  it("refuses a demand that names a resource the session did not declare", async () => {
    const planner = await open();
    const calls = batch(3);
    assert.throws(
      () =>
        planner.createSession(
          calls.map((c, index) => ({
            key: `k${index}`,
            call: c,
            duration: index + 1,
            demands: [{ resource: "nowhere", amount: 1 }],
          })),
          { timing: { unit: "ms" }, resources: { capacities: [{ key: "lane", capacity: 1 }] } },
        ),
      /EP_INVALID_CHANGESET/,
    );
  });

  it("reports the dependency domain when the access declaration moved", async () => {
    const planner = await open();
    const calls = batch(20);
    const session = planner.createSession(
      calls.map((c, index) => ({ key: `k${index}`, call: c })),
      { checkpointInterval: 4 },
    );
    const next = session.applyChanges({
      expectedVersion: 1,
      changes: [{ type: "replace", key: "k19", call: call("c19", "somewhere-else", "write") }],
    });
    assert.equal(next.update.invalidated, "dependency", "a different resource is a different dependency");
    assert.ok(
      next.update.reusedPrefixCalls < 20,
      "a dependency change cannot reuse the whole graph",
    );
  });

  it("reports the dependency domain when a call is removed", async () => {
    const planner = await open();
    const calls = batch(20);
    const session = planner.createSession(
      calls.map((c, index) => ({ key: `k${index}`, call: c })),
      { checkpointInterval: 4 },
    );
    const next = session.applyChanges({
      expectedVersion: 1,
      changes: [{ type: "remove", key: "k19" }],
    });
    assert.equal(next.update.invalidated, "dependency", "a removal renumbers the calls after it");
  });

  it("refuses a stale version and leaves the plan alone", async () => {
    const planner = await open();
    const calls = batch(4);
    const session = planner.createSession(calls.map((c, index) => ({ key: `k${index}`, call: c })));
    const before = session.current();
    assert.throws(
      () =>
        session.applyChanges({
          expectedVersion: 99,
          changes: [{ type: "append", key: "k9", call: call("x", "f", "read") }],
        }),
      /EP_STALE_VERSION/,
    );
    assert.equal(session.current().version, before.version);
    assert.deepEqual(session.current().keys, before.keys);
  });

  it("refuses a duplicate key and an unknown key", async () => {
    const planner = await open();
    const calls = batch(4);
    const session = planner.createSession(calls.map((c, index) => ({ key: `k${index}`, call: c })));
    assert.throws(
      () =>
        session.applyChanges({
          expectedVersion: 1,
          changes: [{ type: "append", key: "k0", call: call("x", "f", "read") }],
        }),
      /EP_DUPLICATE_KEY/,
    );
    assert.throws(
      () =>
        session.applyChanges({
          expectedVersion: 1,
          changes: [{ type: "remove", key: "nope" }],
        }),
      /EP_UNKNOWN_KEY/,
    );
    assert.throws(
      () =>
        session.applyChanges({
          expectedVersion: 1,
          changes: [{ type: "replace", key: "k0", call: call("x", "f", "read") }, { type: "remove", key: "k0" }],
        }),
      /EP_DUPLICATE_KEY/,
    );
  });

  it("caps a caller key at 128 UTF-8 bytes and never quotes one", async () => {
    const planner = await open();
    const ascii = "a".repeat(128);
    const twoByte = "é".repeat(64); // 128 bytes, 64 code units
    const over = "é".repeat(65); // 130 bytes
    const calls = batch(2);
    const session = planner.createSession([
      { key: ascii, call: calls[0]! },
      { key: twoByte, call: calls[1]! },
    ]);
    assert.deepEqual(session.current().keys, [ascii, twoByte]);

    // The same rule at both doors: the first plan and every changeset.
    assert.throws(
      () => planner.createSession([{ key: over, call: calls[0]! }]),
      /EP_INVALID_CHANGESET/,
    );
    let message = "";
    try {
      session.applyChanges({ expectedVersion: 1, changes: [{ type: "remove", key: over }] });
    } catch (error) {
      message = (error as Error).message;
    }
    assert.match(message, /EP_INVALID_CHANGESET/);
    assert.ok(!message.includes(over), `the refusal quoted the key: ${message}`);
    assert.ok(!message.includes(twoByte), "the refusal quoted an accepted key either");
    assert.equal(session.current().version, 1);
  });

  it("leaves the previous snapshot untouched when a change is refused", async () => {
    const planner = await open();
    const calls = batch(4);
    const durations = [2, 3, 4, 5];
    const session = planner.createSession(
      calls.map((c, index) => ({ key: `k${index}`, call: c, duration: durations[index] })),
      { timing: { unit: "ms" } },
    );
    const before = session.current();
    assert.throws(
      () =>
        session.applyChanges({
          expectedVersion: 1,
          changes: [{ type: "append", key: "k9", call: call("x", "f", "read"), duration: 0 }],
        }),
      /EP_INVALID_INPUT/,
    );
    assert.equal(session.current().version, before.version);
    assert.deepEqual(session.current().keys, before.keys);
    assert.deepEqual(shape(session.current().plan).edges, shape(before.plan).edges);
  });

  it("refuses a change that would pass the access ceiling and keeps the plan", async () => {
    const planner = await open();
    const limits = planner.getEngineInfo().limits;
    const perCall = Math.floor(limits.maxAccesses / limits.maxCalls);
    const wide = (index: number) => ({
      id: `c${index}`,
      effect: {
        version: 1 as const,
        mode: "resources" as const,
        accesses: Array.from({ length: perCall }, (_, slot) => ({
          resource: { namespace: "fs", id: `f${index}-${slot}` },
          mode: "read" as const,
        })),
      },
    });
    // Exactly at both ceilings: 128 calls, 1,024 accesses, 1,024 resources.
    const session = planner.createSession(
      Array.from({ length: limits.maxCalls }, (_, index) => ({ key: `k${index}`, call: wide(index) })),
    );
    const before = session.current();
    assert.equal(before.callCount, limits.maxCalls);
    assert.equal(before.keys.length, limits.maxCalls);
    assert.throws(
      () =>
        session.applyChanges({
          expectedVersion: before.version,
          changes: [{ type: "append", key: "extra", call: call("extra", "f", "read") }],
        }),
      /EP_LIMIT_EXCEEDED/,
    );
    // Refused before anything was planned, so the session is exactly as it was.
    assert.equal(session.current().version, before.version);
    assert.deepEqual(session.current().keys, before.keys);
    assert.deepEqual(session.current().plan.edges.length, before.plan.edges.length);
  });

  it("treats an empty transaction as a no-op and keeps the version", async () => {
    const planner = await open();
    const calls = batch(3);
    const session = planner.createSession(calls.map((c, index) => ({ key: `k${index}`, call: c })));
    const next = session.applyChanges({ expectedVersion: 1, changes: [] });
    assert.equal(next.update.strategy, "no-op");
    assert.equal(next.version, 1);
  });

  it("resets into a new generation and keeps counting versions", async () => {
    const planner = await open();
    const calls = batch(3);
    const session = planner.createSession(calls.map((c, index) => ({ key: `k${index}`, call: c })));
    const next = session.reset([{ key: "fresh", call: call("z", "g", "write") }]);
    assert.equal(next.version, 2);
    assert.deepEqual(next.keys, ["fresh"]);
    assert.equal(next.callCount, 1);
    const expected = await fresh([call("z", "g", "write")]);
    assert.deepEqual(shape(next.plan).edges, expected.edges);
  });

  it("refuses to work after being disposed", async () => {
    const planner = await open();
    const calls = batch(2);
    const session = planner.createSession(calls.map((c, index) => ({ key: `k${index}`, call: c })));
    session.dispose();
    assert.throws(() => session.current(), /EP_SESSION_DISPOSED/);
    assert.throws(
      () => session.applyChanges({ expectedVersion: 1, changes: [] }),
      /EP_SESSION_DISPOSED/,
    );
  });

  it("plans an empty session as nothing", async () => {
    const planner = await open();
    const session = planner.createSession([]);
    const snapshot = session.current();
    assert.equal(snapshot.callCount, 0);
    assert.equal(snapshot.edgeCount, 0);
    assert.deepEqual(snapshot.plan.edges, []);
    assert.deepEqual(snapshot.plan.execution.waves, []);
  });

  it("matches a fresh plan after a long chain of edits", async () => {
    const planner = await open();
    const calls = batch(8);
    const session: PlanSession = planner.createSession(
      calls.map((c, index) => ({ key: `k${index}`, call: c, duration: index + 1 })),
      { timing: { unit: "ms" }, checkpointInterval: 2 },
    );
    const model: Model = {
      entries: calls.map((c, index) => ({
        key: `k${index}`,
        call: c,
        duration: index + 1,
      })),
    };
    let version = 1;
    const changes: PlanChange[] = [
      { type: "append", key: "k8", call: call("c8", "f1", "read"), duration: 9 },
    ];
    let next = session.applyChanges({ expectedVersion: version, changes });
    version = next.version;
    assert.equal(next.callCount, 9);
    applyTo(model, changes[0]!);

    changes.length = 0;
    changes.push({ type: "replace", key: "k3", call: call("c3", "f2", "write"), duration: 4 });
    applyTo(model, changes[0]!);
    next = session.applyChanges({ expectedVersion: version, changes });
    version = next.version;

    changes.length = 0;
    changes.push({ type: "remove", key: "k0" });
    applyTo(model, changes[0]!);
    next = session.applyChanges({ expectedVersion: version, changes });
    version = next.version;

    changes.length = 0;
    changes.push({ type: "append", key: "k10", call: call("c10", "f0", "write"), duration: 2 });
    applyTo(model, changes[0]!);
    next = session.applyChanges({ expectedVersion: version, changes });
    version = next.version;

    const finalCalls = model.entries.map((entry) => entry.call);
    const finalDurations = model.entries.map((entry) => entry.duration ?? 1);
    const expected = await fresh(finalCalls, finalDurations);
    assert.deepEqual(shape(next.plan).edges, expected.edges);
    assert.deepEqual(shape(next.plan).levels, expected.levels);
    assert.equal(next.plan.timing!.criticalPathDuration, expected.makespan);
    assert.equal(next.plan.timing!.totalWork, expected.work);
    assert.deepEqual(next.plan.timing!.criticalEvents, expected.critical);
  });
});
