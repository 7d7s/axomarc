/**
 * Public API surface: initialisation, engine metadata, input validation and the
 * stable error mapping.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { EffectPlan, EffectPlanError } from "../src/index.js";
import type { ToolCall } from "../src/index.js";
import { access, call, fakeEngineBytes, modeCall, wasmBytes, wasmFileUrl } from "./support.js";

const open = (): Promise<EffectPlan> => EffectPlan.create({ wasmBytes: wasmBytes() });

/** Asserts a thrown EffectPlanError carries the expected code and phrasing. */
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
  assert.equal(error.name, "EffectPlanError");
  assert.ok(error instanceof Error);
  assert.match(error.message, new RegExp(`^${code}: `));
  if (phrase) assert.match(error.message, phrase);
  return error;
}

describe("EffectPlan.create", () => {
  it("loads the packaged wasm without a binding framework", async () => {
    const plan = await open();
    const info = plan.getEngineInfo();
    assert.equal(info.engineName, "effectplan-engine");
    assert.equal(info.engineVersion, "0.3.0");
    assert.equal(info.levelsRevision, 1);
    assert.equal(info.abiVersion, 1);
    assert.equal(info.wasmActive, true);
  });

  it("reports the prototype limits the engine declares", async () => {
    const { limits } = (await open()).getEngineInfo();
    assert.deepEqual(limits, {
      maxCalls: 128,
      maxAccesses: 1024,
      maxResources: 1024,
      maxEvents: 1152,
      maxEdges: 2304,
      maxNamespaceBytes: 256,
      maxResourceIdBytes: 1024,
      maxCapacityResources: 64,
      maxDemandRecords: 1024,
      maxResourceDiagnostics: 64,
      maxCapacity: 2147483647,
    });
  });

  it("describes one contiguous arena inside live memory", async () => {
    const plan = await open();
    const { layout, limits, memoryBytes } = plan.getEngineInfo();
    assert.equal(layout.input.ptr, layout.arena.ptr);
    assert.equal(layout.workspace.ptr, layout.input.ptr + layout.input.bytes);
    assert.equal(layout.output.ptr, layout.workspace.ptr + layout.workspace.bytes);
    assert.equal(layout.levels.ptr, layout.output.ptr + layout.output.bytes);
    assert.ok(layout.levels.ptr + layout.levels.bytes <= memoryBytes);
    assert.equal(memoryBytes % 65536, 0, "linear memory is a whole number of pages");
    for (const value of [layout.arena.ptr, layout.input.ptr, layout.workspace.ptr, layout.output.ptr, layout.levels.ptr]) {
      assert.equal(value % 4, 0, "every region base is four-byte aligned");
    }
    assert.equal(layout.output.bytes, 16 + 2304 * 16);
    assert.equal(layout.levels.bytes, 4 + limits.maxCalls * 2);
  });

  it("publishes the capacity-resource regions and sizes them from the ceilings", async () => {
    const plan = await open();
    const { layout, limits } = plan.getEngineInfo();
    assert.equal(layout.capacities.ptr, layout.checkpoint.ptr + layout.checkpoint.bytes);
    assert.equal(layout.demandOffsets.ptr, layout.capacities.ptr + layout.capacities.bytes);
    assert.equal(layout.demandRecords.ptr, layout.demandOffsets.ptr + layout.demandOffsets.bytes);
    // The engine's private scratch sits between the demand records and the
    // schedule; the host never names it.
    assert.ok(layout.schedule.ptr > layout.demandRecords.ptr + layout.demandRecords.bytes);
    assert.equal(layout.resourceStats.ptr, layout.schedule.ptr + layout.schedule.bytes);
    assert.equal(layout.resourceDiagnostics.ptr, layout.resourceStats.ptr + layout.resourceStats.bytes);
    assert.equal(layout.capacities.bytes, limits.maxCapacityResources * 4);
    assert.equal(layout.demandOffsets.bytes, (limits.maxCalls + 1) * 4);
    assert.equal(layout.demandRecords.bytes, limits.maxDemandRecords * 8);
    assert.equal(layout.schedule.bytes, 32 + limits.maxCalls * 8);
    assert.equal(layout.resourceStats.bytes, 32 + limits.maxCapacityResources * 16);
    assert.equal(layout.resourceDiagnostics.bytes, 32 + limits.maxResourceDiagnostics * 16);
  });

  it("hands out frozen metadata", async () => {
    const info = (await open()).getEngineInfo();
    for (const value of [info, info.limits, info.layout, info.layout.arena, info.layout.output, info.layout.levels]) {
      assert.ok(Object.isFrozen(value));
    }
  });

  it("accepts an explicit file URL for the wasm artifact", async () => {
    const plan = await EffectPlan.create({ wasmUrl: wasmFileUrl });
    assert.equal(plan.getEngineInfo().abiVersion, 1);
  });

  it("refuses bytes that are not a wasm module", async () => {
    await rejectsCode(
      () => EffectPlan.create({ wasmBytes: new Uint8Array([0x00, 0x61, 0x73, 0x6d, 0x99, 0x00, 0x00, 0x00]) }),
      "EP_ENGINE_UNAVAILABLE",
    );
  });

  it("refuses a missing artifact instead of falling back to JavaScript", async () => {
    await rejectsCode(
      () => EffectPlan.create({ wasmUrl: new URL("file:///effectplan-does-not-exist.wasm") }),
      "EP_ENGINE_UNAVAILABLE",
    );
  });

  it("refuses an engine whose exports are incomplete", async () => {
    await rejectsCode(
      () => EffectPlan.create({ wasmBytes: fakeEngineBytes({ omit: ["effectplan_info"] }) }),
      "EP_ENGINE_UNAVAILABLE",
      /required exports missing/,
    );
    await rejectsCode(
      () => EffectPlan.create({ wasmBytes: fakeEngineBytes({ omit: ["memory"] }) }),
      "EP_ENGINE_UNAVAILABLE",
      /required exports missing/,
    );
  });

  it("refuses an incompatible ABI revision", async () => {
    await rejectsCode(
      () => EffectPlan.create({ wasmBytes: fakeEngineBytes({ abi: 2 }) }),
      "EP_ENGINE_UNAVAILABLE",
      /abi 2 incompatible with 1/,
    );
  });

  it("refuses an engine whose record framing is not the framing it decodes", async () => {
    await rejectsCode(
      () => EffectPlan.create({ wasmBytes: fakeEngineBytes({ overrides: { 14: 8 } }) }),
      "EP_ENGINE_UNAVAILABLE",
      /record framing/,
    );
  });

  it("refuses an engine without the execution-levels entry point", async () => {
    await rejectsCode(
      () => EffectPlan.create({ wasmBytes: fakeEngineBytes({ omit: ["effectplan_plan_v2"] }) }),
      "EP_ENGINE_UNAVAILABLE",
      /does not support execution levels/,
    );
  });

  it("refuses an incompatible execution-levels revision", async () => {
    await rejectsCode(
      () => EffectPlan.create({ wasmBytes: fakeEngineBytes({ levelsRevision: 2 }) }),
      "EP_ENGINE_UNAVAILABLE",
      /levels revision 2 incompatible with 1/,
    );
  });

  it("refuses a levels region that is not the tail of the arena", async () => {
    await rejectsCode(
      () => EffectPlan.create({ wasmBytes: fakeEngineBytes({ overrides: { 22: 1132 } }) }),
      "EP_ENGINE_UNAVAILABLE",
      /non-contiguous levels region/,
    );
  });

  it("refuses a levels region that does not match the call ceiling", async () => {
    await rejectsCode(
      () => EffectPlan.create({ wasmBytes: fakeEngineBytes({ overrides: { 23: 10 } }) }),
      "EP_ENGINE_UNAVAILABLE",
      /levels region (does not match its declared capacity|publishes the wrong capacity)/,
    );
    await rejectsCode(
      () => EffectPlan.create({ wasmBytes: fakeEngineBytes({ overrides: { 24: 8 } }) }),
      "EP_ENGINE_UNAVAILABLE",
      /levels region (does not match its declared capacity|publishes the wrong capacity)/,
    );
  });

  it("refuses an engine without the capacity-resource entry points", async () => {
    await rejectsCode(
      () => EffectPlan.create({ wasmBytes: fakeEngineBytes({ omit: ["effectplan_resource_check"] }) }),
      "EP_ENGINE_UNAVAILABLE",
      /does not support capacity resources/,
    );
    await rejectsCode(
      () => EffectPlan.create({ wasmBytes: fakeEngineBytes({ omit: ["effectplan_resource_schedule"] }) }),
      "EP_ENGINE_UNAVAILABLE",
      /cannot schedule against capacity resources/,
    );
  });

  it("refuses an incompatible capacity-resource revision", async () => {
    await rejectsCode(
      () => EffectPlan.create({ wasmBytes: fakeEngineBytes({ overrides: { 35: 2 } }) }),
      "EP_ENGINE_UNAVAILABLE",
      /resource revision 2 incompatible with 1/,
    );
  });

  it("refuses a capacity-resource region it cannot audit", async () => {
    await rejectsCode(
      () => EffectPlan.create({ wasmBytes: fakeEngineBytes({ overrides: { 36: 1308 } }) }),
      "EP_ENGINE_UNAVAILABLE",
      /non-contiguous capacities region/,
    );
    await rejectsCode(
      () => EffectPlan.create({ wasmBytes: fakeEngineBytes({ overrides: { 37: 12 } }) }),
      "EP_ENGINE_UNAVAILABLE",
      /capacities region does not match/,
    );
    await rejectsCode(
      () => EffectPlan.create({ wasmBytes: fakeEngineBytes({ overrides: { 46: 1544 } }) }),
      "EP_ENGINE_UNAVAILABLE",
      /non-contiguous resource diagnostics region/,
    );
    // A schedule region that starts inside the demand records would overlap
    // them; the audit refuses an overlapping region as well as a misplaced one.
    await rejectsCode(
      () => EffectPlan.create({ wasmBytes: fakeEngineBytes({ overrides: { 42: 1344 } }) }),
      "EP_ENGINE_UNAVAILABLE",
      /overlapping schedule region/,
    );
  });

  it("refuses an arena, alignment or framing the loader cannot audit", async () => {
    await rejectsCode(
      () => EffectPlan.create({ wasmBytes: fakeEngineBytes({ overrides: { 2: 64513 } }) }),
      "EP_ENGINE_UNAVAILABLE",
      /arena outside linear memory/,
    );
    await rejectsCode(
      () => EffectPlan.create({ wasmBytes: fakeEngineBytes({ overrides: { 5: 1070 } }) }),
      "EP_ENGINE_UNAVAILABLE",
      /unaligned workspace region base/,
    );
    await rejectsCode(
      () => EffectPlan.create({ wasmBytes: fakeEngineBytes({ overrides: { 6: 24 } }) }),
      "EP_ENGINE_UNAVAILABLE",
      /non-contiguous output region/,
    );
    await rejectsCode(
      () => EffectPlan.create({ wasmBytes: fakeEngineBytes({ overrides: { 8: 32 } }) }),
      "EP_ENGINE_UNAVAILABLE",
      /output region does not match/,
    );
  });
});

describe("planBatch input validation", () => {
  const bad = (value: unknown): ToolCall[] => [value as ToolCall];

  it("requires an array batch", async () => {
    const plan = await open();
    await rejectsCode(() => plan.planBatch(null as unknown as ToolCall[]), "EP_INVALID_INPUT", /must be an array/);
  });

  it("rejects a batch above the call ceiling", async () => {
    const planner = await open();
    const calls = Array.from({ length: 129 }, (_, i) => modeCall(`c${i}`, "pure"));
    await rejectsCode(() => planner.planBatch(calls), "EP_LIMIT_EXCEEDED", /129 calls exceeds 128/);
  });

  it("rejects calls that are not objects", async () => {
    const plan = await open();
    await rejectsCode(() => plan.planBatch(bad(undefined)), "EP_INVALID_INPUT", /must be an object/);
    await rejectsCode(() => plan.planBatch(bad("read-a")), "EP_INVALID_INPUT", /must be an object/);
  });

  it("rejects missing or empty call ids", async () => {
    const plan = await open();
    await rejectsCode(() => plan.planBatch(bad({})), "EP_INVALID_INPUT", /non-empty string id/);
    await rejectsCode(() => plan.planBatch(bad({ id: "" })), "EP_INVALID_INPUT", /non-empty string id/);
    await rejectsCode(() => plan.planBatch(bad({ id: 7 })), "EP_INVALID_INPUT", /non-empty string id/);
  });

  it("rejects duplicate call ids without echoing the id itself", async () => {
    const plan = await open();
    await rejectsCode(
      () => plan.planBatch([modeCall("same-secret-id", "pure"), modeCall("same-secret-id", "pure")]),
      "EP_DUPLICATE_CALL_ID",
      /call index 1/,
    );
  });

  it("rejects an effect version other than 1", async () => {
    const plan = await open();
    await rejectsCode(
      () => plan.planBatch(bad({ id: "a", effect: { version: 2, mode: "pure" } })),
      "EP_INVALID_EFFECT",
      /effect version/,
    );
  });

  it("rejects an unknown effect mode", async () => {
    const plan = await open();
    await rejectsCode(
      () => plan.planBatch(bad({ id: "a", effect: { version: 1, mode: "sometimes" } })),
      "EP_INVALID_EFFECT",
      /effect mode/,
    );
  });

  it("rejects a non-object effect", async () => {
    const plan = await open();
    await rejectsCode(
      () => plan.planBatch(bad({ id: "a", effect: "resources" })),
      "EP_INVALID_EFFECT",
      /effect must be an object/,
    );
  });

  it("rejects a non-array accesses list", async () => {
    const plan = await open();
    await rejectsCode(
      () => plan.planBatch(bad({ id: "a", effect: { version: 1, mode: "resources", accesses: "fs:f1" } })),
      "EP_INVALID_EFFECT",
      /accesses must be an array/,
    );
  });

  it("rejects a pure call that declares accesses", async () => {
    const plan = await open();
    await rejectsCode(
      () => plan.planBatch(bad({
        id: "a",
        effect: { version: 1, mode: "pure", accesses: [access("fs", "f1", "read")] },
      })),
      "EP_INVALID_EFFECT",
      /pure but declares accesses/,
    );
  });

  it("rejects a resources call that declares nothing", async () => {
    const plan = await open();
    await rejectsCode(
      () => plan.planBatch(bad({ id: "a", effect: { version: 1, mode: "resources", accesses: [] } })),
      "EP_INVALID_EFFECT",
      /at least one access/,
    );
  });

  it("rejects an access mode that is neither read nor write", async () => {
    const plan = await open();
    await rejectsCode(
      () => plan.planBatch(bad({
        id: "a",
        effect: { version: 1, mode: "resources", accesses: [{ resource: { namespace: "fs", id: "f1" }, mode: "append" }] },
      })),
      "EP_INVALID_EFFECT",
      /access mode/,
    );
  });

  it("rejects malformed resource references", async () => {
    const plan = await open();
    const withResource = (resource: unknown): ToolCall[] =>
      bad({ id: "a", effect: { version: 1, mode: "resources", accesses: [{ resource, mode: "read" }] } });
    await rejectsCode(() => plan.planBatch(withResource(undefined)), "EP_INVALID_RESOURCE", /resource shape/);
    await rejectsCode(() => plan.planBatch(withResource({ namespace: "", id: "f1" })), "EP_INVALID_RESOURCE", /resource shape/);
    await rejectsCode(() => plan.planBatch(withResource({ namespace: "fs", id: 42 })), "EP_INVALID_RESOURCE", /resource shape/);
    await rejectsCode(
      () => plan.planBatch(bad({ id: "a", effect: { version: 1, mode: "resources", accesses: [null] } })),
      "EP_INVALID_RESOURCE",
      /access shape/,
    );
  });

  it("enforces the namespace and resource id byte limits", async () => {
    const plan = await open();
    await rejectsCode(
      () => plan.planBatch([call("a", access("n".repeat(257), "f1", "read"))]),
      "EP_LIMIT_EXCEEDED",
      /namespace bytes/,
    );
    await rejectsCode(
      () => plan.planBatch([call("a", access("fs", "i".repeat(1025), "read"))]),
      "EP_LIMIT_EXCEEDED",
      /resource id bytes/,
    );
    // Exactly at the limit is accepted.
    const result = plan.planBatch([call("a", access("n".repeat(256), "i".repeat(1024), "read"))]);
    assert.equal(result.edgeCount, 0);
  });

  it("counts multibyte identifiers in UTF-8 bytes, not code units", async () => {
    const plan = await open();
    const long = "é".repeat(129); // 258 UTF-8 bytes, 129 characters
    await rejectsCode(() => plan.planBatch([call("a", access(long, "f1", "read"))]), "EP_LIMIT_EXCEEDED");
  });

  it("enforces the access ceiling", async () => {
    const plan = await open();
    // Nine shared resources, re-touched by every call: the access count is what
    // overflows, not the resource table.
    const shared = Array.from({ length: 9 }, (_, k) => access("fs", `s${k}`, "read"));
    const calls = Array.from({ length: 128 }, (_, i) => call(`c${i}`, ...shared));
    await rejectsCode(() => plan.planBatch(calls), "EP_LIMIT_EXCEEDED", /accesses exceeds 1024/);
  });

  it("enforces the resource ceiling", async () => {
    const plan = await open();
    const calls = Array.from({ length: 128 }, (_, i) =>
      call(`c${i}`, ...Array.from({ length: 9 }, (_, k) => access("fs", `u${i}-${k}`, "read"))));
    await rejectsCode(() => plan.planBatch(calls), "EP_LIMIT_EXCEEDED", /resources exceeds 1024/);
  });

  it("keeps an unknown effect unless the caller forbids it", async () => {
    const plan = await open();
    const calls = [modeCall("a", "unknown")];
    assert.equal(plan.planBatch(calls).statistics.unknownEffectCount, 1);
    await rejectsCode(
      () => plan.planBatch(calls, { unknownEffectPolicy: "reject" }),
      "EP_UNKNOWN_EFFECT_REJECTED",
    );
  });

  it("never echoes a caller resource identifier in an error", async () => {
    const plan = await open();
    const calls = [
      call("top-secret-call", access("tenant-a", "private-file", "read")),
      {
        id: "other",
        effect: {
          version: 1,
          mode: "resources",
          accesses: [{ resource: { namespace: "tenant-a", id: "private-file" }, mode: "sideways" }],
        },
      } as unknown as ToolCall,
    ];
    const error = await rejectsCode(() => plan.planBatch(calls), "EP_INVALID_EFFECT");
    for (const secret of ["tenant-a", "private-file", "top-secret-call"]) {
      assert.ok(!error.message.includes(secret), `leaked ${secret}: ${error.message}`);
    }
  });
});
