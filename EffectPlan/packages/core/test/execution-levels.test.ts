/**
 * Execution levels through the public API: decoding, wave construction,
 * graph-to-level parity, immutability and the refusal of a malformed levels
 * region. The TypeScript layer only groups what the engine computed; every
 * dependency assertion here is checked against the engine's own graph.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { EffectPlan } from "../src/index.js";
import { decodeExecutionLevels } from "../src/execution-levels.js";
import type { ExecutionLevels } from "../src/execution-levels.js";
import { EffectPlanError } from "../src/errors.js";
import { encodeBatch, packInput } from "../src/encode.js";
import { loadEngine } from "../src/loader.js";
import type { EngineInfo } from "../src/loader.js";
import { access, call, modeCall, wasmBytes } from "./support.js";
import { referenceLevels } from "../../../benchmarks/reference-levels.js";
import { referencePlan } from "../../../benchmarks/reference-planner.js";

const open = (): Promise<EffectPlan> => EffectPlan.create({ wasmBytes: wasmBytes() });

/** The calls that must run before `index`, straight from the engine's graph. */
function predecessorsOf(plan: { edges: ReadonlyArray<{ from: number; to: number }> }, index: number): number[] {
  return plan.edges.filter((edge) => edge.to === index).map((edge) => edge.from);
}

describe("execution levels", () => {
  it("reports an empty batch as zero waves", async () => {
    const plan = await open();
    const result = plan.planBatch([]);
    assert.deepEqual(result.execution.levelByCall, []);
    assert.deepEqual(result.execution.waves, []);
    assert.equal(result.execution.waveCount, 0);
  });

  it("puts a single call in a single wave", async () => {
    const plan = await open();
    const result = plan.planBatch([call("a", access("fs", "f1", "write"))]);
    assert.deepEqual(result.execution.levelByCall, [0]);
    assert.deepEqual(result.execution.waves, [[0]]);
    assert.equal(result.execution.waveCount, 1);
  });

  it("serializes a chain into one call per wave", async () => {
    const plan = await open();
    const result = plan.planBatch([
      call("a", access("fs", "f1", "write")),
      call("b", access("fs", "f1", "read")),
      call("c", access("fs", "f1", "write")),
    ]);
    assert.deepEqual(result.execution.levelByCall, [0, 1, 2]);
    assert.deepEqual(result.execution.waves, [[0], [1], [2]]);
    assert.equal(result.execution.waveCount, 3);
  });

  it("layers the documented diamond example", async () => {
    const plan = await open();
    // 0 writes f1; 1 and 2 read f1; 3 writes f1.
    const result = plan.planBatch([
      call("a", access("fs", "f1", "write")),
      call("b", access("fs", "f1", "read")),
      call("c", access("fs", "f1", "read")),
      call("d", access("fs", "f1", "write")),
    ]);
    assert.deepEqual(result.execution.levelByCall, [0, 1, 1, 2]);
    assert.deepEqual(result.execution.waves, [[0], [1, 2], [3]]);
    assert.equal(result.execution.waveCount, 3);
  });

  it("keeps independent resources in the first wave", async () => {
    const plan = await open();
    const result = plan.planBatch([
      call("a", access("fs", "f1", "write")),
      call("b", access("fs", "f2", "write")),
      call("c", access("fs", "f3", "read")),
    ]);
    assert.deepEqual(result.execution.levelByCall, [0, 0, 0]);
    assert.deepEqual(result.execution.waves, [[0, 1, 2]]);
  });

  it("separates anything a barrier touches", async () => {
    const plan = await open();
    const withUnknown = plan.planBatch([
      call("a", access("fs", "f1", "write")),
      modeCall("u", "unknown"),
      call("b", access("fs", "f2", "write")),
    ]);
    // The unknown call is a barrier: before it, after it, nothing shares.
    assert.deepEqual(withUnknown.execution.levelByCall, [0, 1, 2]);
    assert.deepEqual(withUnknown.execution.waves, [[0], [1], [2]]);

    const withExclusive = plan.planBatch([
      call("a", access("fs", "f1", "write")),
      modeCall("x", "exclusive"),
      call("b", access("fs", "f1", "read")),
    ]);
    assert.deepEqual(withExclusive.execution.levelByCall, [0, 1, 2]);
  });

  it("indexes levels by the batch position, not by any internal id", async () => {
    const plan = await open();
    const result = plan.planBatch([
      call("call-that-runs-last", access("fs", "f1", "read")),
      call("call-that-runs-first", access("fs", "f1", "write")),
    ]);
    // Index 1 writes, so index 0 must depend on it? No: dependencies only
    // order an *earlier* call before a *later* one. The read at index 0 has no
    // predecessor; the write at index 1 waits for the read.
    assert.deepEqual(result.execution.levelByCall, [0, 1]);
    assert.deepEqual(result.execution.waves, [[0], [1]]);
  });

  it("matches the independent Kahn oracle on every curated shape", async () => {
    const plan = await open();
    const batches = [
      [],
      [call("a", access("fs", "f1", "write"))],
      [
        call("a", access("fs", "f1", "write")),
        call("b", access("fs", "f1", "read")),
        call("c", access("fs", "f1", "write")),
      ],
      [
        call("a", access("fs", "f1", "write")),
        call("b", access("fs", "f2", "write")),
        call("c", access("fs", "f1", "read")),
        call("d", access("fs", "f2", "read")),
        call("e", access("fs", "f1", "write")),
      ],
      [
        modeCall("x", "exclusive"),
        call("a", access("fs", "f1", "read")),
        modeCall("u", "unknown"),
        call("b", access("fs", "f1", "write")),
      ],
    ];
    for (const calls of batches) {
      const result = plan.planBatch(calls);
      const oracle = referenceLevels(calls.length, referencePlan(calls));
      assert.deepEqual([...result.execution.levelByCall], oracle, `oracle disagrees on ${JSON.stringify(calls)}`);
      const partition = result.execution.waves.flat();
      assert.equal(partition.length, calls.length);
      assert.deepEqual([...partition].sort((a, b) => a - b), [...partition], "waves keep input order");
    }
  });

  it("plans the maximum batch with levels that stay inside the batch", async () => {
    const plan = await open();
    const calls = Array.from({ length: 128 }, (_, i) =>
      call(`c${i}`, access("fs", `f${i % 8}`, i % 2 === 0 ? "write" : "read")));
    const result = plan.planBatch(calls);
    assert.equal(result.execution.levelByCall.length, 128);
    for (const [index, level] of result.execution.levelByCall.entries()) {
      assert.ok(level >= 0 && level < 128, `level ${level} for call ${index}`);
    }
    for (const edge of result.edges) {
      assert.ok(result.execution.levelByCall[edge.from]! < result.execution.levelByCall[edge.to]!);
    }
    assert.equal(result.execution.waves.flat().length, 128);
    assert.equal(result.execution.waveCount, Math.max(...result.execution.levelByCall) + 1);
  });

  it("hands out frozen, stable levels", async () => {
    const plan = await open();
    const result = plan.planBatch([
      call("a", access("fs", "f1", "write")),
      call("b", access("fs", "f1", "read")),
    ]);
    const execution = result.execution;
    assert.ok(Object.isFrozen(execution));
    assert.ok(Object.isFrozen(execution.waves));
    assert.ok(Object.isFrozen(execution.levelByCall));
    for (const wave of execution.waves) assert.ok(Object.isFrozen(wave));
    assert.equal(execution.waves, result.execution.waves, "the same handle every time");
  });

  it("gives identical batches identical levels, batch after batch", async () => {
    const plan = await open();
    const calls = [
      call("a", access("fs", "f1", "write")),
      call("b", access("fs", "f2", "read")),
      modeCall("u", "unknown"),
      call("c", access("fs", "f1", "read")),
    ];
    const shapes = new Set<string>();
    for (let round = 0; round < 25; round++) {
      const result = plan.planBatch(calls);
      shapes.add(JSON.stringify([result.execution.levelByCall, result.execution.waves]));
      // A different batch between replays must not disturb the next one.
      plan.planBatch([call("z", access("fs", "zz", "write"))]);
    }
    assert.equal(shapes.size, 1, "levels must be a pure function of the batch");
  });

  it("keeps the Phase 1 result shape for existing consumers", async () => {
    const plan = await open();
    const result = plan.planBatch([
      call("a", access("fs", "f1", "write")),
      call("b", access("fs", "f1", "read")),
    ]);
    assert.equal(result.version, 1);
    assert.equal(result.callCount, 2);
    assert.deepEqual(result.edges.map((edge) => [edge.from, edge.to, edge.resource, edge.reason]), [[0, 1, 1, 1]]);
    assert.equal(result.statistics.dependencyCount, 1);
    assert.equal(result.execution.levelByCall.join(","), "0,1");
  });
});

describe("execution levels decoding", () => {
  /** Runs one batch and hands back the engine metadata and a live view. */
  async function engineRun(calls: Parameters<EffectPlan["planBatch"]>[0]) {
    const engine = await loadEngine({ wasmBytes: wasmBytes() });
    const input = packInput(engine.memory.buffer, engine.info);
    const encoded = encodeBatch(input, engine.info.limits, calls, {});
    const p = encoded.pointers;
    const { layout } = engine.info;
    const status = engine.planV2(
      engine.info.abiVersion, encoded.callCount, encoded.accessCount, encoded.resourceCount,
      p.callModes, p.accessOffsets, p.resourceIds, p.accessModes,
      layout.workspace.ptr, layout.workspace.bytes, layout.output.ptr, layout.output.bytes,
      layout.levels.ptr, layout.levels.bytes,
    );
    assert.equal(status, 0);
    return { info: engine.info, view: input.view };
  }

  async function decodeWith(calls: Parameters<EffectPlan["planBatch"]>[0], corrupt?: (view: DataView, info: EngineInfo) => void): Promise<ExecutionLevels> {
    const { info, view } = await engineRun(calls);
    if (corrupt) corrupt(view, info);
    return decodeExecutionLevels(view, info, calls.length);
  }

  const batch = [
    call("a", access("fs", "f1", "write")),
    call("b", access("fs", "f1", "read")),
  ];

  it("decodes the engine's own region without complaint", async () => {
    const execution = await decodeWith(batch);
    assert.deepEqual(execution.levelByCall, [0, 1]);
    assert.deepEqual(execution.waves, [[0], [1]]);
  });

  it("refuses a levels count that is not the batch size", async () => {
    const execution = decodeWith(batch, (view, info) => {
      view.setUint32(info.layout.levels.ptr, 7, true);
    });
    await assert.rejects(() => execution, (error: unknown) => {
      assert.ok(error instanceof EffectPlanError);
      assert.equal(error.code, "EP_INVALID_PLAN");
      assert.match(error.message, /levels region holds 7/);
      return true;
    });
  });

  it("refuses a level that reaches outside the batch", async () => {
    const execution = decodeWith(batch, (view, info) => {
      view.setUint16(info.layout.levels.ptr + 4 + 2, 99, true);
    });
    await assert.rejects(() => execution, (error: unknown) => {
      assert.ok(error instanceof EffectPlanError);
      assert.equal(error.code, "EP_INVALID_PLAN");
      assert.match(error.message, /out of range/);
      return true;
    });
  });

  it("refuses a level assignment that leaves an empty wave", async () => {
    const three = [...batch, call("c", access("fs", "f3", "read"))];
    const execution = decodeWith(three, (view, info) => {
      // levels [0, 0, 2]: no call sits in wave 1.
      view.setUint16(info.layout.levels.ptr + 4 + 2, 0, true);
      view.setUint16(info.layout.levels.ptr + 4 + 4, 2, true);
    });
    await assert.rejects(() => execution, (error: unknown) => {
      assert.ok(error instanceof EffectPlanError);
      assert.equal(error.code, "EP_INVALID_PLAN");
      assert.match(error.message, /not contiguous/);
      return true;
    });
  });

  it("never echoes a caller identifier in a decoding failure", async () => {
    const withSecrets = [
      call("top-secret-call", access("tenant-a", "private-file", "write")),
      call("other", access("tenant-a", "private-file", "read")),
    ];
    const execution = decodeWith(withSecrets, (view, info) => {
      view.setUint32(info.layout.levels.ptr, 42, true);
    });
    const error = (await execution.catch((cause: unknown) => cause)) as EffectPlanError;
    assert.ok(error instanceof EffectPlanError);
    for (const secret of ["tenant-a", "private-file", "top-secret-call"]) {
      assert.ok(!error.message.includes(secret), `leaked ${secret}: ${error.message}`);
    }
  });
});
