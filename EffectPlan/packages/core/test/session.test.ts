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
