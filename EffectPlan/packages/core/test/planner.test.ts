/**
 * Planning semantics through the public API: the conflict matrix, effect-mode
 * barriers, encoding normalisation, output decoding and plan immutability.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { EffectPlan } from "../src/index.js";
import type { ExecutionPlan } from "../src/index.js";
import { access, call, modeCall, wasmBytes } from "./support.js";

const open = (): Promise<EffectPlan> => EffectPlan.create({ wasmBytes: wasmBytes() });

const read = (id: string) => access("fs", id, "read");
const write = (id: string) => access("fs", id, "write");

describe("conflict matrix", () => {
  it("leaves read/read independent", async () => {
    const plan = await open();
    const result = plan.planBatch([call("a", read("f1")), call("b", read("f1"))]);
    assert.deepEqual([...result.edges], []);
  });

  it("orders a writer behind every pending reader", async () => {
    const plan = await open();
    const result = plan.planBatch([
      call("a", read("f1")),
      call("b", read("f1")),
      call("c", write("f1")),
    ]);
    assert.deepEqual(result.edges.map((e) => [e.from, e.to, e.resource, e.reason]), [
      [0, 2, 1, 2],
      [1, 2, 1, 2],
    ]);
  });

  it("orders a reader behind the last writer", async () => {
    const plan = await open();
    const result = plan.planBatch([call("a", write("f1")), call("b", read("f1"))]);
    assert.deepEqual(result.edges.map((e) => [e.from, e.to, e.reason]), [[0, 1, 1]]);
  });

  it("chains write/write and clears the drained readers", async () => {
    const plan = await open();
    const result = plan.planBatch([
      call("a", read("f1")),
      call("b", write("f1")),
      call("c", write("f1")),
    ]);
    assert.deepEqual(result.edges.map((e) => [e.from, e.to, e.reason]), [
      [0, 1, 2],
      [1, 2, 3],
    ]);
  });

  it("keeps disjoint resources independent", async () => {
    const plan = await open();
    const result = plan.planBatch([
      call("a", write("f1")),
      call("b", write("f2")),
      call("c", read("f3")),
    ]);
    assert.equal(result.edgeCount, 0);
  });

  it("emits one edge per conflicting resource of a multi-resource call", async () => {
    const plan = await open();
    const result = plan.planBatch([
      call("a", write("f1"), write("f2")),
      call("b", read("f1"), read("f2")),
      call("c", write("f3")),
    ]);
    assert.deepEqual(result.edges.map((e) => [e.from, e.to, e.resource, e.reason]), [
      [0, 1, 1, 1],
      [0, 1, 2, 1],
    ]);
  });

  it("orders every successor's predecessors ascending", async () => {
    const plan = await open();
    const result = plan.planBatch([
      call("a", write("f2")),
      call("b", write("f1")),
      call("c", read("f1"), read("f2")),
    ]);
    // Resource numbers follow first appearance, so f2 is 1 and f1 is 2.
    assert.deepEqual(result.edges.map((e) => [e.from, e.to, e.resource]), [
      [0, 2, 1],
      [1, 2, 2],
    ]);
  });
});

describe("effect modes", () => {
  it("treats pure calls as declaring nothing", async () => {
    const plan = await open();
    const result = plan.planBatch([
      modeCall("a", "pure"),
      modeCall("b", "pure"),
      call("c", write("f1")),
    ]);
    assert.equal(result.edgeCount, 0);
  });

  it("serialises an exclusive call behind earlier work and ahead of later work", async () => {
    const plan = await open();
    const result = plan.planBatch([
      call("a", write("f1")),
      modeCall("b", "exclusive"),
      call("c", write("f2")),
    ]);
    // Distinct resources would be independent; the barrier forbids that.
    // 5 = read/write over the barrier, 4 = write/read over the barrier.
    assert.deepEqual(result.edges.map((e) => [e.from, e.to, e.resource, e.reason]), [
      [0, 1, 0, 5],
      [1, 2, 0, 4],
    ]);
  });

  it("serialises consecutive exclusive calls", async () => {
    const plan = await open();
    const result = plan.planBatch([modeCall("a", "exclusive"), modeCall("b", "exclusive")]);
    assert.deepEqual(result.edges.map((e) => [e.from, e.to, e.resource, e.reason]), [[0, 1, 0, 6]]);
  });

  it("treats a missing contract as an unknown effect, never as pure", async () => {
    const plan = await open();
    // Both calls are barriers; the second is a barrier write behind a barrier
    // write, so the reason is write/write over the barrier (6).
    const result = plan.planBatch([modeCall("a", "unknown"), { id: "b" }]);
    assert.deepEqual(result.edges.map((e) => [e.from, e.to, e.resource, e.reason]), [[0, 1, 0, 6]]);
    assert.equal(result.statistics.unknownEffectCount, 2);
    // An unknown followed by a pure call is a barrier write then a barrier read.
    const mixed = plan.planBatch([modeCall("a", "unknown"), modeCall("b", "pure")]);
    assert.deepEqual(mixed.edges.map((e) => [e.from, e.to, e.resource, e.reason]), [[0, 1, 0, 4]]);
  });

  it("keeps ordinary reads of one resource parallel across an unrelated barrier", async () => {
    const plan = await open();
    const result = plan.planBatch([
      call("a", read("f1")),
      call("b", read("f1")),
      modeCall("c", "exclusive"),
    ]);
    assert.deepEqual(result.edges.map((e) => [e.from, e.to]), [[0, 2], [1, 2]]);
  });
});

describe("encoding normalisation", () => {
  it("collapses a read and a write of one resource into a write", async () => {
    const plan = await open();
    const result = plan.planBatch([
      call("a", read("f1"), write("f1")),
      call("b", read("f1")),
    ]);
    assert.deepEqual(result.edges.map((e) => [e.from, e.to, e.reason]), [[0, 1, 1]]);
    assert.equal(result.statistics.resourceAccessCount, 2);
  });

  it("deduplicates repeated identical accesses", async () => {
    const plan = await open();
    const result = plan.planBatch([
      call("a", read("f1"), read("f1"), read("f1")),
      call("b", write("f1")),
    ]);
    assert.equal(result.statistics.resourceAccessCount, 2);
    assert.deepEqual(result.edges.map((e) => [e.from, e.to]), [[0, 1]]);
  });

  it("keys resources by namespace and id, never by their concatenation", async () => {
    const plan = await open();
    const result = plan.planBatch([
      call("a", access("fs", "ab:c", "write")),
      call("b", access("fs", "a:bc", "read")),  // different id
      call("c", access("db", "ab:c", "read")),  // different namespace
      call("d", access("a", "bc", "write")),    // concatenates to "abc"
      call("e", access("ab", "c", "read")),     // also concatenates to "abc"
      call("f", access("fs", "ab:c", "read")),  // the only genuine conflict
    ]);
    assert.deepEqual(result.edges.map((e) => [e.from, e.to, e.resource, e.reason]), [[0, 5, 1, 1]]);
  });

  it("numbers resources per batch, not per instance", async () => {
    const plan = await open();
    const first = plan.planBatch([
      call("a", write("x"), write("y")),
      call("b", read("x"), read("y")),
    ]);
    assert.deepEqual(first.edges.map((e) => e.resource), [1, 2]);
    const second = plan.planBatch([
      call("a", write("y")),
      call("b", read("y")),
    ]);
    assert.deepEqual(second.edges.map((e) => e.resource), [1], "the interning table must be reset");
  });

  it("never orders a call against itself", async () => {
    const plan = await open();
    const result = plan.planBatch([
      call("a", read("f1"), write("f1")),
      modeCall("b", "unknown"),
    ]);
    assert.ok(result.edges.every((e) => e.from < e.to), JSON.stringify([...result.edges]));
    assert.ok(!result.edges.some((e) => e.from === e.to));
  });
});

describe("output decoding", () => {
  it("reports statistics that agree with the decoded graph", async () => {
    const plan = await open();
    const result = plan.planBatch([
      call("a", write("f1")),
      call("b", read("f1")),
      modeCall("c", "exclusive"),
      call("d", write("f2")),
      call("e", read("f2")),
    ]);
    assert.equal(result.version, 1);
    assert.equal(result.callCount, 5);
    assert.equal(result.edgeCount, result.edges.length);
    assert.deepEqual(result.statistics, {
      callCount: 5,
      resourceAccessCount: 4,
      dependencyCount: result.edges.length,
      unknownEffectCount: 0,
      exclusiveCount: 1,
    });
    assert.ok(result.edges.every((e) => e.reason >= 1 && e.reason <= 6));
  });

  it("handles an empty batch", async () => {
    const plan = await open();
    const result = plan.planBatch([]);
    assert.equal(result.edgeCount, 0);
    assert.deepEqual(result.statistics, {
      callCount: 0,
      resourceAccessCount: 0,
      dependencyCount: 0,
      unknownEffectCount: 0,
      exclusiveCount: 0,
    });
  });

  it("plans the full prototype ceiling through the public API", async () => {
    const plan = await open();
    const calls = Array.from({ length: 128 }, (_, i) =>
      call(`c${i}`, ...Array.from({ length: 8 }, (_, k) => access("fs", `r${(i * 8 + k) % 256}`, i % 2 ? "write" : "read"))));
    const result = plan.planBatch(calls);
    assert.equal(result.statistics.resourceAccessCount, 1024);
    assert.equal(result.callCount, 128);
    assert.ok(result.edgeCount > 0);
    assert.ok(result.edgeCount <= 2304, `edge bound violated: ${result.edgeCount}`);
    assert.ok(result.edges.every((e) => e.from < e.to && e.to < 128));
  });

  it("freezes the graph and keeps it stable across later batches", async () => {
    const plan = await open();
    const first: ExecutionPlan = plan.planBatch([
      call("a", write("f1")),
      call("b", read("f1")),
    ]);
    const edges = first.edges;
    assert.ok(Object.isFrozen(edges));
    assert.ok(edges.every((edge) => Object.isFrozen(edge)));
    assert.ok(Object.isFrozen(first.statistics));
    assert.equal(first.edges, edges, "the getter must not rebuild on every read");

    // A second, larger batch overwrites the engine's output region; the earlier
    // plan must be unaffected.
    const second = plan.planBatch(Array.from({ length: 40 }, (_, i) =>
      call(`x${i}`, ...Array.from({ length: 4 }, (_, k) => access("fs", `q${k}`, i % 2 ? "read" : "write")))));
    assert.ok(second.edgeCount > first.edgeCount);
    assert.deepEqual([...first.edges], [{ from: 0, to: 1, resource: 1, reason: 1 }]);
  });

  it("is deterministic across repeated calls and fresh instances", async () => {
    const calls = [
      call("a", write("f1"), read("f2")),
      modeCall("b", "unknown"),
      call("c", read("f1"), write("f2")),
      call("d", write("f3")),
      modeCall("e", "exclusive"),
      call("f", read("f3")),
    ];
    const plan = await open();
    const a = JSON.stringify(plan.planBatch(calls).edges);
    const b = JSON.stringify(plan.planBatch(calls).edges);
    const other = await open();
    const c = JSON.stringify(other.planBatch(calls).edges);
    assert.equal(a, b);
    assert.equal(a, c);
  });
});
