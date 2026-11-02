/**
 * WebAssembly integration: the artifact loads on its own, the raw ABI behaves as
 * documented, the packed typed-array encoding is what the engine reads, hostile
 * arguments are refused with status codes instead of traps, execution levels
 * come back through the versioned ABI, and memory growth is survivable.
 */

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";

import { EffectPlan } from "../src/index.js";
import { encodeBatch, packInput } from "../src/encode.js";
import { decodeExecutionLevels } from "../src/execution-levels.js";
import { loadEngine } from "../src/loader.js";
import { access, call, modeCall, wasmBytes, wasmFileUrl } from "./support.js";

const MODE = { pure: 0, resources: 1, exclusive: 2, unknown: 3 } as const;
const READ = 0;
const WRITE = 1;

const SELECTOR = {
  abi: 0, base: 1, arenaLen: 2, inPtr: 3, inLen: 4, wsPtr: 5, wsLen: 6, outPtr: 7, outLen: 8,
  maxCalls: 9, maxAccess: 10, maxRes: 11, maxEvents: 12, maxEdges: 13, edgeBytes: 14,
  maxNs: 15, maxId: 16, hdrBytes: 17, levelsRev: 21, levelsPtr: 22, levelsLen: 23, levelsMax: 24,
  timingRev: 25, durPtr: 26, durLen: 27, durMax: 28, timingPtr: 29, timingLen: 30, timingMax: 31,
  ckptRev: 32, ckptPtr: 33, ckptLen: 34,
  resRev: 35, capPtr: 36, capLen: 37, offsPtr: 38, offsLen: 39, recPtr: 40, recLen: 41,
  schedPtr: 42, schedLen: 43, statsPtr: 44, statsLen: 45, diagPtr: 46, diagLen: 47,
  maxCapRes: 48, maxDemands: 49, maxDiags: 50, maxCapacity: 51,
} as const;

/** The staged artifact plus the region map it publishes. */
function fixture() {
  const module = new WebAssembly.Module(wasmBytes());
  const instance = new WebAssembly.Instance(module, {});
  const info = instance.exports.effectplan_info as (kind: number) => number;
  const plan = instance.exports.effectplan_plan as (...args: number[]) => number;
  const planV2 = instance.exports.effectplan_plan_v2 as (...args: number[]) => number;
  const planV3 = instance.exports.effectplan_plan_v3 as (...args: number[]) => number;
  const planV4 = instance.exports.effectplan_plan_v4 as (...args: number[]) => number;
  const memory = instance.exports.memory as WebAssembly.Memory;
  const layout = {
    base: info(SELECTOR.base),
    arena: info(SELECTOR.arenaLen),
    in: info(SELECTOR.inPtr),
    inLen: info(SELECTOR.inLen),
    ws: info(SELECTOR.wsPtr),
    wsLen: info(SELECTOR.wsLen),
    out: info(SELECTOR.outPtr),
    outLen: info(SELECTOR.outLen),
    levels: info(SELECTOR.levelsPtr),
    levelsLen: info(SELECTOR.levelsLen),
    levelsRev: info(SELECTOR.levelsRev),
    maxLevels: info(SELECTOR.levelsMax),
    timingRev: info(SELECTOR.timingRev),
    durations: info(SELECTOR.durPtr),
    durationsLen: info(SELECTOR.durLen),
    durationsMax: info(SELECTOR.durMax),
    timing: info(SELECTOR.timingPtr),
    timingLen: info(SELECTOR.timingLen),
    timingMax: info(SELECTOR.timingMax),
    maxCalls: info(SELECTOR.maxCalls),
    maxAccess: info(SELECTOR.maxAccess),
    maxRes: info(SELECTOR.maxRes),
    ckptRev: info(SELECTOR.ckptRev),
    ckpt: info(SELECTOR.ckptPtr),
    ckptLen: info(SELECTOR.ckptLen),
    resRev: info(SELECTOR.resRev),
    capacities: info(SELECTOR.capPtr),
    capacitiesLen: info(SELECTOR.capLen),
    demandOffsets: info(SELECTOR.offsPtr),
    demandOffsetsLen: info(SELECTOR.offsLen),
    demandRecords: info(SELECTOR.recPtr),
    demandRecordsLen: info(SELECTOR.recLen),
    schedule: info(SELECTOR.schedPtr),
    scheduleLen: info(SELECTOR.schedLen),
    stats: info(SELECTOR.statsPtr),
    statsLen: info(SELECTOR.statsLen),
    diagnostics: info(SELECTOR.diagPtr),
    diagnosticsLen: info(SELECTOR.diagLen),
    maxCapacityResources: info(SELECTOR.maxCapRes),
    maxDemandRecords: info(SELECTOR.maxDemands),
    maxDiagnostics: info(SELECTOR.maxDiags),
    maxCapacity: info(SELECTOR.maxCapacity),
    maxEdges: info(SELECTOR.maxEdges),
    abi: info(SELECTOR.abi),
  };
  const view = () => new DataView(memory.buffer);
  const edges = (): number[][] => {
    const buffer = view();
    const count = buffer.getUint32(layout.out + 8, true);
    const out: number[][] = [];
    for (let i = 0; i < count; i++) {
      const at = layout.out + 16 + i * 16;
      out.push([
        buffer.getUint32(at, true),
        buffer.getUint32(at + 4, true),
        buffer.getUint32(at + 8, true),
        buffer.getUint32(at + 12, true),
      ]);
    }
    return out;
  };
  /** The count header plus one u16 level per call, as the engine wrote them. */
  const levels = (): { count: number; entries: number[] } => {
    const buffer = view();
    const count = buffer.getUint32(layout.levels, true);
    const entries: number[] = [];
    for (let i = 0; i < count; i++) {
      entries.push(buffer.getUint16(layout.levels + 4 + i * 2, true));
    }
    return { count, entries };
  };
  /** The timing region: header plus (earliestStart, slack) per call. */
  const timing = () => {
    const buffer = view();
    const revision = buffer.getUint32(layout.timing, true);
    const count = buffer.getUint32(layout.timing + 4, true);
    const events: Array<[number, number]> = [];
    for (let i = 0; i < count; i++) {
      const at = layout.timing + 32 + i * 8;
      events.push([buffer.getUint32(at, true), buffer.getUint32(at + 4, true)]);
    }
    return {
      revision,
      count,
      makespan: buffer.getUint32(layout.timing + 8, true),
      peak: buffer.getUint32(layout.timing + 12, true),
      totalWork: buffer.getUint32(layout.timing + 16, true) + buffer.getUint32(layout.timing + 20, true) * 2 ** 32,
      criticalCount: buffer.getUint32(layout.timing + 24, true),
      reserved: buffer.getUint32(layout.timing + 28, true),
      events,
    };
  };
  const checkpoint = () => {
    const buffer = view();
    const at = layout.ckpt;
    return {
      revision: buffer.getUint32(at, true),
      calls: buffer.getUint32(at + 4, true),
      events: buffer.getUint32(at + 8, true),
      edges: buffer.getUint32(at + 12, true),
      digest: buffer.getUint32(at + 16, true),
      reserved: [
        buffer.getUint32(at + 20, true),
        buffer.getUint32(at + 24, true),
        buffer.getUint32(at + 28, true),
      ],
    };
  };
  /**
   * The state a caller must keep to resume: the conflict frontier and event
   * nodes, the records emitted so far, and the engine's own commitment.
   */
  const snapshot = () => {
    const bytes = new Uint8Array(memory.buffer);
    const emitted = view().getUint32(layout.out + 8, true);
    return {
      workspace: bytes.slice(layout.ws, layout.ws + layout.wsLen),
      records: bytes.slice(layout.out + 16, layout.out + 16 + emitted * 16),
      record: bytes.slice(layout.ckpt, layout.ckpt + layout.ckptLen),
    };
  };
  /** A copy of a known good checkpoint, so a test can damage one field only. */
  const corrupt = (snap: ReturnType<typeof snapshot>) => ({
    workspace: snap.workspace.slice(),
    records: snap.records.slice(),
    record: snap.record.slice(),
  });
  const writeDurations = (values: number[]) => {
    const buffer = new DataView(memory.buffer);
    for (let i = 0; i < layout.durationsMax; i++) {
      buffer.setUint32(layout.durations + i * 4, values[i] ?? 0, true);
    }
  };
  return { module, instance, info, plan, planV2, planV3, planV4, memory, layout, view, edges, levels, timing, writeDurations, checkpoint, snapshot, corrupt };
}

type Fixture = ReturnType<typeof fixture>;

/** Region addresses for the packed tables, exactly as the engine publishes them. */
function regions(f: Fixture) {
  const callModes = f.layout.in;
  const offsets = callModes + f.layout.maxCalls;
  const ids = offsets + (f.layout.maxCalls + 1) * 4;
  const modes = ids + f.layout.maxAccess * 4;
  return { callModes, offsets, ids, modes };
}

/** Writes one batch by hand, through plain typed arrays. */
function pack(f: Fixture, callModes: number[], offsets: number[], ids: number[], accessModes: number[]) {
  const r = regions(f);
  const bytes = new Uint8Array(f.memory.buffer, f.layout.in, f.layout.inLen);
  const words = new Uint32Array(f.memory.buffer, r.offsets, f.layout.maxCalls + 1);
  const idWords = new Uint32Array(f.memory.buffer, r.ids, f.layout.maxAccess);
  bytes.fill(0);
  words.fill(0);
  idWords.fill(0);
  callModes.forEach((value, i) => { bytes[r.callModes - f.layout.in + i] = value; });
  accessModes.forEach((value, i) => { bytes[r.modes - f.layout.in + i] = value; });
  offsets.forEach((value, i) => { words[i] = value; });
  ids.forEach((value, i) => { idWords[i] = value; });
  return {
    r,
    args: [
      f.layout.abi, callModes.length, ids.length, (ids.length ? Math.max(...ids) : 0) + 1,
      r.callModes, r.offsets, r.ids, r.modes, f.layout.ws, f.layout.wsLen, f.layout.out, f.layout.outLen,
    ],
  };
}

/** Version one: writes one batch, calls the ABI, returns the status. */
function encode(f: Fixture, callModes: number[], offsets: number[], ids: number[], accessModes: number[]) {
  return f.plan(...pack(f, callModes, offsets, ids, accessModes).args);
}

/** Version two: the same batch, plus the canonical levels region. */
function encodeV2(f: Fixture, callModes: number[], offsets: number[], ids: number[], accessModes: number[]) {
  const { args } = pack(f, callModes, offsets, ids, accessModes);
  return f.planV2(...args, f.layout.levels, f.layout.levelsLen);
}

/** Version three: levels plus the duration-aware scheduling analysis. */
function encodeV3(
  f: Fixture,
  callModes: number[],
  offsets: number[],
  ids: number[],
  accessModes: number[],
  durations: number[],
) {
  const { args } = pack(f, callModes, offsets, ids, accessModes);
  f.writeDurations(durations);
  return f.planV3(
    ...args,
    f.layout.levels, f.layout.levelsLen,
    f.layout.durations, f.layout.durationsLen,
    f.layout.timing, f.layout.timingLen,
  );
}


/** A batch shaped like the specification's worked diamond, plus its durations. */
interface ResumeBatch {
  readonly modes: number[];
  readonly offsets: number[];
  readonly ids: number[];
  readonly accessModes: number[];
  readonly ticks: number[];
}

/**
 * Version four: the same pipeline as version three, plus the checkpoint region.
 * `boundary` plans only the first calls, `snap` restores a previous run's state
 * before planning the suffix, and `grown` extends the batch for the append case.
 */
function encodeV4(
  f: Fixture,
  batch: ResumeBatch,
  boundary = batch.modes.length,
  snap: ReturnType<Fixture["snapshot"]> | null = null,
) {
  const accesses = boundary === 0 ? 0 : batch.offsets[boundary];
  const { args } = pack(
    f,
    batch.modes.slice(0, boundary),
    boundary === 0 ? [0] : batch.offsets.slice(0, boundary + 1),
    batch.ids.slice(0, accesses),
    batch.accessModes.slice(0, accesses),
  );
  f.writeDurations(batch.ticks.slice(0, boundary));
  const bytes = new Uint8Array(f.memory.buffer);
  if (snap) {
    bytes.set(snap.workspace, f.layout.ws);
    bytes.set(snap.records, f.layout.out + 16);
    bytes.set(snap.record, f.layout.ckpt);
  } else {
    bytes.fill(0, f.layout.ckpt, f.layout.ckpt + f.layout.ckptLen);
  }
  return f.planV4(
    ...args,
    f.layout.levels, f.layout.levelsLen,
    f.layout.durations, f.layout.durationsLen,
    f.layout.timing, f.layout.timingLen,
    f.layout.ckpt, f.layout.ckptLen,
  );
}

/** Everything a caller may compare between two runs of the same batch. */
function shape(f: Fixture): string {
  return JSON.stringify({ edges: f.edges(), levels: f.levels(), timing: f.timing() });
}

describe("wasm artifact", () => {
  it("loads in Node.js through the packaged entry point with no options", async () => {
    // Resolved at runtime from the emitted test file, so this exercises the built
    // artifact rather than the TypeScript sources.
    const distEntry = new URL("../../../../packages/core/dist/index.js", import.meta.url).href;
    const dist = (await import(distEntry)) as typeof import("../src/index.js");
    const planner = await dist.EffectPlan.create();
    const result = planner.planBatch([
      call("a", access("fs", "f1", "write")),
      call("b", access("fs", "f1", "read")),
    ]);
    assert.deepEqual(result.edges.map((e) => [e.from, e.to, e.reason]), [[0, 1, 1]]);
  });

  it("has no imports and exports only the raw ABI", () => {
    const module = new WebAssembly.Module(wasmBytes());
    assert.deepEqual(WebAssembly.Module.imports(module), [], "no host glue, no WASI, no JS helpers");
    const functions = WebAssembly.Module.exports(module)
      .filter((entry) => entry.kind === "function")
      .map((entry) => entry.name)
      .sort();
    assert.deepEqual(functions, [
      "effectplan_info",
      "effectplan_plan",
      "effectplan_plan_v2",
      "effectplan_plan_v3",
      "effectplan_plan_v4",
      "effectplan_resource_check",
      "effectplan_resource_schedule",
    ]);
    assert.equal(WebAssembly.Module.exports(module).filter((e) => e.kind === "memory").length, 1);
  });

  it("carries no allocator, binding framework or panic formatting", () => {
    const bytes = Buffer.from(wasmBytes());
    for (const needle of ["wasm_bindgen", "__wbindgen", "__rust_alloc", "panicked at", "capacity overflow"]) {
      assert.ok(!bytes.includes(Buffer.from(needle, "utf8")), `found ${needle}`);
    }
  });

  it("executes through the raw ABI", () => {
    const f = fixture();
    assert.equal(encode(f, [MODE.resources, MODE.resources], [0, 1, 2], [1, 1], [WRITE, READ]), 0);
    assert.deepEqual(f.edges(), [[0, 1, 1, 1]]);
    assert.equal(encode(f, [MODE.resources, MODE.resources], [0, 1, 2], [1, 1], [READ, READ]), 0);
    assert.deepEqual(f.edges(), []);
  });

  it("reads exactly the packed typed arrays the wrapper writes", async () => {
    const planner = await EffectPlan.create({ wasmBytes: wasmBytes() });
    const info = planner.getEngineInfo();
    // A second, independent instance: hand-written typed arrays, same bytes.
    const f = fixture();
    const input = packInput(f.memory.buffer, info);
    const batches = [
      [
        call("a", access("fs", "f1", "read")),
        call("b", access("fs", "f1", "write")),
        call("c", access("fs", "f2", "read")),
      ],
      [modeCall("a", "unknown"), call("b", access("fs", "f1", "read")), modeCall("c", "exclusive")],
      [
        call("a", access("fs", "f1", "read"), access("db", "t1", "write")),
        call("b", access("db", "t1", "read")),
        call("c", access("fs", "f1", "read")),
      ],
    ];
    for (const calls of batches) {
      const viaApi = planner.planBatch(calls).edges.map((e) => [e.from, e.to, e.resource, e.reason]);
      const encoded = encodeBatch(input, info.limits, calls, {});
      const layout = f.layout;
      const status = f.plan(
        layout.abi, encoded.callCount, encoded.accessCount, encoded.resourceCount,
        encoded.pointers.callModes, encoded.pointers.accessOffsets, encoded.pointers.resourceIds, encoded.pointers.accessModes,
        layout.ws, layout.wsLen, layout.out, layout.outLen,
      );
      assert.equal(status, 0);
      assert.deepEqual(f.edges(), viaApi, "raw ABI and public API must agree record for record");
    }
  });

  it("refuses an ABI revision it did not produce", () => {
    const f = fixture();
    const r = regions(f);
    assert.equal(
      f.plan(f.layout.abi + 1, 0, 0, 1, r.callModes, r.offsets, r.ids, r.modes, f.layout.ws, f.layout.wsLen, f.layout.out, f.layout.outLen),
      100,
    );
  });

  it("refuses malformed offsets", () => {
    const f = fixture();
    assert.equal(encode(f, [MODE.resources, MODE.resources], [0, 2, 1], [1, 1], [READ, READ]), 111);
    assert.equal(encode(f, [MODE.resources], [1, 1], [1], [READ]), 112);
    assert.equal(encode(f, [MODE.resources], [0, 0], [1], [READ]), 113);
    assert.equal(encode(f, [MODE.resources], [0, 7], [1], [READ]), 111, "offset past the access count");
  });

  it("refuses out-of-bounds and non-canonical pointers without trapping", () => {
    const f = fixture();
    const r = regions(f);
    const arenaEnd = f.layout.base + f.layout.arena;
    const call12 = (over: Partial<ReturnType<typeof regions> & { ws: number; wsLen: number; out: number; outLen: number }> = {}) => {
      const a = { ...r, ws: f.layout.ws, wsLen: f.layout.wsLen, out: f.layout.out, outLen: f.layout.outLen, ...over };
      return f.plan(f.layout.abi, 0, 0, 1, a.callModes, a.offsets, a.ids, a.modes, a.ws, a.wsLen, a.out, a.outLen);
    };
    assert.equal(call12({ callModes: 0 }), 104, "below the arena");
    assert.equal(call12({ ids: arenaEnd + 4 }), 108, "past the arena is not a canonical region base");
    assert.equal(call12({ ids: 0x7fff_fff0 }), 108, "a wild u32 pointer is refused");
    assert.equal(call12({ modes: r.modes + 1 }), 108, "unaligned region base");
    assert.equal(call12({ ws: f.layout.ws + 4 }), 108, "workspace base shifted into itself");
    assert.equal(call12({ wsLen: 16 }), 106, "workspace shorter than the layout needs");
    assert.equal(call12({ outLen: 8 }), 107, "output cannot hold the header");
    assert.equal(call12({ outLen: f.layout.outLen + 16 }), 107, "output beyond the canonical region");
    assert.deepEqual(f.edges(), [], "a refused call leaves no stale records behind");
  });

  it("survives a host that grows the linear memory", () => {
    const f = fixture();
    const before = f.memory.buffer.byteLength;
    assert.equal(encode(f, [MODE.resources, MODE.resources], [0, 1, 2], [1, 1], [WRITE, READ]), 0);
    const first = JSON.stringify(f.edges());

    const pages = f.memory.grow(1);
    assert.equal(pages, before / 65536, "grow reports the previous page count");
    assert.ok(f.memory.buffer.byteLength > before, "growth appends; the arena addresses are unchanged");
    assert.equal(encode(f, [MODE.resources, MODE.resources], [0, 1, 2], [1, 1], [WRITE, READ]), 0);
    assert.equal(JSON.stringify(f.edges()), first, "identical input still produces identical output");
  });

  it("re-maps its views after growth instead of writing into a detached buffer", async () => {
    // planBatch() takes this path: growth detaches the old buffer, so the packed
    // views must be rebuilt from the live one before the next write.
    const engine = await loadEngine({ wasmBytes: wasmBytes() });
    const info = engine.info;
    const batch = [call("a", access("fs", "f1", "write")), call("b", access("fs", "f1", "read"))];
    const run = (input: ReturnType<typeof packInput>, encoded: ReturnType<typeof encodeBatch>) =>
      engine.plan(
        info.abiVersion, encoded.callCount, encoded.accessCount, encoded.resourceCount,
        encoded.pointers.callModes, encoded.pointers.accessOffsets, encoded.pointers.resourceIds, encoded.pointers.accessModes,
        info.layout.workspace.ptr, info.layout.workspace.bytes, info.layout.output.ptr, info.layout.output.bytes,
      ) === 0
        ? new DataView(input.buffer).getUint32(info.layout.output.ptr + 16, true)
        : -1;

    const first = packInput(engine.memory.buffer, info);
    assert.equal(run(first, encodeBatch(first, info.limits, batch, {})), 0);

    const detached = engine.memory.buffer;
    engine.memory.grow(1);
    assert.notEqual(engine.memory.buffer, detached, "growth replaces the buffer");
    assert.equal(detached.byteLength, 0, "the previous buffer reports no length");

    // The hazard this guards: a write through a stale view is silently dropped,
    // so the engine would plan a batch it never received.
    first.callModes[0] = MODE.unknown;
    const live = new Uint8Array(engine.memory.buffer);
    assert.notEqual(live[info.layout.input.ptr], MODE.unknown, "stale writes never reach live memory");
    assert.throws(() => new DataView(detached).getUint32(0, true), TypeError,
      "reading a detached buffer fails loudly instead of returning zeros");

    const second = packInput(engine.memory.buffer, info);
    assert.equal(run(second, encodeBatch(second, info.limits, batch, {})), 0);
    assert.equal(
      new DataView(second.buffer).getUint32(info.layout.output.ptr + 20, true),
      1,
      "the decoded successor is call 1",
    );
  });

  it("never traps on arbitrary hostile arguments", () => {
    const f = fixture();
    let state = 0x2f6e2b1;
    const next = () => {
      // xorshift32: a deterministic pseudo-random stream, no Math.random().
      state ^= state << 13; state >>>= 0;
      state ^= state >>> 17; state >>>= 0;
      state ^= state << 5; state >>>= 0;
      return state >>> 0;
    };
    const r = regions(f);
    const arenaEnd = f.layout.base + f.layout.arena;

    // One well-formed batch, packed through the typed arrays the engine reads.
    encode(f, [MODE.resources, MODE.resources], [0, 1, 2], [1, 2], [WRITE, READ]);
    const valid = {
      abi: f.layout.abi, calls: 2, accesses: 2, resources: 3,
      callModes: r.callModes, offsets: r.offsets, ids: r.ids, modes: r.modes,
      ws: f.layout.ws, wsLen: f.layout.wsLen, out: f.layout.out, outLen: f.layout.outLen,
      levels: f.layout.levels, levelsLen: f.layout.levelsLen,
    };
    const canonical = [
      valid.abi, valid.calls, valid.accesses, valid.resources,
      valid.callModes, valid.offsets, valid.ids, valid.modes,
      valid.ws, valid.wsLen, valid.out, valid.outLen,
    ];
    const canonical14 = [...canonical, valid.levels, valid.levelsLen];
    assert.equal(f.plan(...canonical), 0);
    const expected = JSON.stringify(f.edges());
    assert.equal(f.planV2(...canonical14), 0);
    assert.deepEqual(f.levels(), { count: 2, entries: [0, 0] }, "independent calls share a wave");

    const statuses = new Map<number, number>();
    const record = (status: number): number => {
      assert.ok(Number.isInteger(status) && status >= 0, `status must be a non-negative code, got ${status}`);
      statuses.set(status, (statuses.get(status) ?? 0) + 1);
      return status;
    };
    const call = (args: readonly number[]): number => {
      try {
        return record(f.plan(...args));
      } catch (cause) {
        assert.fail(`trap on arguments ${args.join(",")}: ${String(cause)}`);
      }
    };
    const callV2 = (args: readonly number[]): number => {
      try {
        return record(f.planV2(...args));
      } catch (cause) {
        assert.fail(`trap on v2 arguments ${args.join(",")}: ${String(cause)}`);
      }
    };

    // Sweep: one slot poisoned at a time, the rest left exactly right. The v2
    // entry point adds the levels pointer and its length to the twelve slots
    // the version one entry point takes.
    const hostile = [0, 1, 3, r.ids + 1, f.layout.base - 4, f.layout.in - 4, arenaEnd, arenaEnd + 4, 0x7fff_fff0, 0x8000_0000, -1];
    for (let slot = 0; slot < canonical14.length; slot++) {
      for (const value of hostile) {
        const args = canonical14.slice();
        args[slot] = value;
        callV2(args);
        if (slot < canonical.length) call(args.slice(0, 12));
      }
    }

    // Randomised: each slot independently keeps its canonical value or takes a
    // hostile one, so wholly-valid combinations still occur. Both entry points
    // get every argument set.
    const roll = () => next() % 4;
    const inside = () => next() % (arenaEnd + 32);
    const keep = (value: number) => (roll() < 3 ? value : inside());
    const small = (value: number, span: number) => (roll() === 0 ? next() % span : value);
    const randomArgs = (): number[] => [
      valid.abi,
      small(valid.calls, 400),
      small(valid.accesses, 400),
      small(valid.resources, 400),
      keep(valid.callModes),
      keep(valid.offsets),
      keep(valid.ids),
      keep(valid.modes),
      keep(valid.ws),
      small(valid.wsLen, 4000),
      keep(valid.out),
      small(valid.outLen, 70000),
      keep(valid.levels),
      small(valid.levelsLen, 4000),
    ];
    for (let i = 0; i < 50000; i++) {
      const args = randomArgs();
      call(args.slice(0, 12));
      callV2(args);
    }

    assert.ok(statuses.size >= 4, `only ${statuses.size} distinct outcomes`);
    assert.ok(statuses.has(0), "some argument combinations are valid and must plan successfully");
    assert.ok(statuses.has(116), "the levels-capacity code must be reachable");
    for (const code of statuses.keys()) {
      assert.ok(code === 0 || (code >= 100 && code < 200), `status ${code} is not a documented error code`);
    }
    // Fail-closed: tens of thousands of refused calls left no residue behind.
    assert.equal(f.plan(...canonical), 0);
    assert.equal(JSON.stringify(f.edges()), expected, "a refused call must not corrupt the next one");
    assert.equal(f.planV2(...canonical14), 0);
    assert.deepEqual(f.levels(), { count: 2, entries: [0, 0] });
  });

  it("keeps its documented arena inside a fixed memory footprint", () => {
    const f = fixture();
    const bytes = Buffer.from(wasmBytes());
    assert.ok(bytes.length <= 40000, `wasm artifact is ${bytes.length} bytes`);
    assert.equal(f.layout.maxEdges, 2304);
    assert.ok(f.memory.buffer.byteLength <= 2 * 1024 * 1024, `linear memory is ${f.memory.buffer.byteLength} bytes`);
    assert.ok(f.layout.base + f.layout.arena <= f.memory.buffer.byteLength);
    // The execution-levels region is the tail of the arena, inside live memory.
    assert.equal(f.layout.levels, f.layout.out + f.layout.outLen);
    assert.ok(f.layout.levels + f.layout.levelsLen <= f.layout.base + f.layout.arena);
    assert.equal(f.memory.buffer.byteLength % 65536, 0);
    assert.equal(readFileSync(wasmFileUrl).length, bytes.length);
  });
});

describe("execution levels through the raw ABI", () => {
  it("publishes the levels contract through the info export", () => {
    const f = fixture();
    assert.equal(f.layout.levelsRev, 1);
    assert.equal(f.layout.maxLevels, f.layout.maxCalls);
    assert.equal(f.layout.levelsLen, 4 + f.layout.maxCalls * 2);
  });

  it("writes no levels through the version one entry point", () => {
    const f = fixture();
    assert.equal(encode(f, [MODE.resources, MODE.resources], [0, 1, 2], [1, 1], [WRITE, READ]), 0);
    assert.deepEqual(f.levels(), { count: 0, entries: [] });
  });

  it("layers a chain through the versioned entry point", () => {
    const f = fixture();
    // 0 writes r1; 1 reads r1; 2 writes r1; 3 reads r1: a four-deep chain.
    assert.equal(
      encodeV2(f, [MODE.resources, MODE.resources, MODE.resources, MODE.resources], [0, 1, 2, 3, 4], [1, 1, 1, 1], [WRITE, READ, WRITE, READ]),
      0,
    );
    assert.deepEqual(f.levels(), { count: 4, entries: [0, 1, 2, 3] });
  });

  it("puts independent calls in one wave", () => {
    const f = fixture();
    assert.equal(encodeV2(f, [MODE.resources, MODE.resources], [0, 1, 2], [1, 2], [WRITE, WRITE]), 0);
    assert.deepEqual(f.levels(), { count: 2, entries: [0, 0] });
  });

  it("layers a diamond by its deepest predecessor", () => {
    // 0 writes r1; 1 and 2 read r1; 3 writes r1. Levels 0 / 1 / 2, with 1 and 2
    // sharing a wave, exactly the example the public API documents.
    const f = fixture();
    assert.equal(
      encodeV2(f, [MODE.resources, MODE.resources, MODE.resources, MODE.resources], [0, 1, 2, 3, 4], [1, 1, 1, 1], [WRITE, READ, READ, WRITE]),
      0,
    );
    const { count, entries } = f.levels();
    assert.equal(count, 4);
    assert.deepEqual(entries, [0, 1, 1, 2]);
    for (const edge of f.edges()) {
      assert.ok(entries[edge[0] as number]! < entries[edge[1] as number]!, "every edge must rise");
    }
  });

  it("treats an unknown effect as a barrier in the levels", () => {
    const f = fixture();
    assert.equal(encodeV2(f, [MODE.resources, MODE.unknown, MODE.resources], [0, 1, 1, 1], [1], [READ]), 0);
    assert.deepEqual(f.levels(), { count: 3, entries: [0, 1, 2] });
  });

  it("gives an empty batch an empty levels region", () => {
    const f = fixture();
    assert.equal(encodeV2(f, [], [0], [], []), 0);
    assert.deepEqual(f.levels(), { count: 0, entries: [] });
  });

  it("re-computes levels for every batch size, repeatedly", () => {
    const f = fixture();
    const shapes = [
      { modes: [MODE.resources], offsets: [0, 1], ids: [1], amodes: [WRITE] },
      { modes: [MODE.resources, MODE.resources], offsets: [0, 1, 2], ids: [1, 1], amodes: [WRITE, READ] },
      { modes: [MODE.resources, MODE.resources, MODE.resources], offsets: [0, 1, 2, 3], ids: [1, 1, 1], amodes: [WRITE, READ, WRITE] },
    ];
    for (const round of [0, 1, 2]) {
      for (const [index, shape] of shapes.entries()) {
        const expected = Array.from({ length: index + 1 }, (_, call) => call);
        assert.equal(encodeV2(f, shape.modes, shape.offsets, shape.ids, shape.amodes), 0, `round ${round} shape ${index}`);
        assert.deepEqual(f.levels().entries, expected, `round ${round} shape ${index}`);
      }
    }
  });

  it("refuses a levels pointer that is not the canonical region", () => {
    const f = fixture();
    const { args } = pack(f, [MODE.resources], [0, 1], [1], [WRITE]);
    const inArena = [f.layout.out, f.layout.in, f.layout.ws, f.layout.levels + 4];
    for (const ptr of inArena) {
      assert.equal(f.planV2(...args, ptr, f.layout.levelsLen), 108, `levels pointer ${ptr}`);
    }
    // Below the arena never converts to an offset at all.
    assert.equal(f.planV2(...args, f.layout.base - 4, f.layout.levelsLen), 104);
    assert.equal(f.planV2(...args, 0, f.layout.levelsLen), 104);
  });

  it("refuses a levels length that is not the canonical region", () => {
    const f = fixture();
    const { args } = pack(f, [MODE.resources], [0, 1], [1], [WRITE]);
    for (const len of [0, 4, 256, f.layout.levelsLen - 2, f.layout.levelsLen + 2, 1 << 20]) {
      assert.equal(f.planV2(...args, f.layout.levels, len), 116, `levels length ${len}`);
    }
  });

  it("keeps version one answering exactly what it answered before", () => {
    const f = fixture();
    const batch = [MODE.resources, MODE.resources] as const;
    assert.equal(encode(f, [...batch], [0, 1, 2], [1, 1], [WRITE, READ]), 0);
    const first = JSON.stringify(f.edges());
    assert.equal(encodeV2(f, [...batch], [0, 1, 2], [1, 1], [WRITE, READ]), 0);
    assert.equal(encode(f, [...batch], [0, 1, 2], [1, 1], [WRITE, READ]), 0);
    assert.equal(JSON.stringify(f.edges()), first, "the two entry points must agree on the graph");
  });

  it("maps every status the extended ABI can return", () => {
    const f = fixture();
    const { args } = pack(f, [MODE.resources, MODE.resources], [0, 1, 2], [1, 1], [WRITE, READ]);
    // 0: a valid batch, 100: wrong abi, 116: wrong levels length,
    // 108: wrong levels pointer.
    assert.equal(f.planV2(...args, f.layout.levels, f.layout.levelsLen), 0);
    assert.equal(f.planV2(f.layout.abi + 1, ...args.slice(1), f.layout.levels, f.layout.levelsLen), 100);
    assert.equal(f.planV2(...args, f.layout.levels, 7), 116);
    assert.equal(f.planV2(...args, f.layout.levels + 4, f.layout.levelsLen), 108);
  });

  it("keeps execution levels correct after the host grows the memory", async () => {
    // The levels region lives in the arena, whose addresses survive growth;
    // the host's *views* do not, so the wrapper must re-map them before the
    // next write. planBatch takes exactly this path.
    const engine = await loadEngine({ wasmBytes: wasmBytes() });
    const info = engine.info;
    const calls = [
      call("a", access("fs", "f1", "write")),
      call("b", access("fs", "f1", "read")),
      call("c", access("fs", "f2", "write")),
    ];
    const runV2 = (input: ReturnType<typeof packInput>): number[] => {
      const encoded = encodeBatch(input, info.limits, calls, {});
      const p = encoded.pointers;
      const { layout } = info;
      const status = engine.planV2(
        info.abiVersion, encoded.callCount, encoded.accessCount, encoded.resourceCount,
        p.callModes, p.accessOffsets, p.resourceIds, p.accessModes,
        layout.workspace.ptr, layout.workspace.bytes, layout.output.ptr, layout.output.bytes,
        layout.levels.ptr, layout.levels.bytes,
      );
      assert.equal(status, 0);
      return decodeExecutionLevels(input.view, info, encoded.callCount).levelByCall as number[];
    };

    const before = runV2(packInput(engine.memory.buffer, info));
    assert.deepEqual(before, [0, 1, 0]);
    engine.memory.grow(1);
    const after = runV2(packInput(engine.memory.buffer, info));
    assert.deepEqual(after, before, "identical input, identical levels, across growth");
  });
});

describe("scheduling ABI", () => {
  /** Same shape as DIAMOND, with the durations this phase plans with. */
  const resumeBatch = (): ResumeBatch => ({
    modes: [MODE.resources, MODE.resources, MODE.resources, MODE.resources, MODE.resources],
    offsets: [0, 1, 2, 3, 4, 5],
    ids: [1, 1, 1, 1, 1],
    accessModes: [WRITE, READ, READ, WRITE, READ],
    ticks: [2, 5, 2, 3, 4],
  });

  const DIAMOND = {
    modes: [MODE.resources, MODE.resources, MODE.resources, MODE.resources, MODE.resources],
    offsets: [0, 1, 2, 3, 4, 5],
    ids: [1, 1, 1, 1, 1],
    accessModes: [WRITE, READ, READ, WRITE, READ],
  };

  it("publishes the scheduling contract through the info export", () => {
    const f = fixture();
    assert.equal(f.layout.timingRev, 1);
    assert.equal(f.layout.durationsLen, 4 * f.layout.maxCalls);
    assert.equal(f.layout.durationsMax, f.layout.maxCalls);
    assert.equal(f.layout.timingLen, 32 + 8 * f.layout.maxCalls);
    assert.equal(f.layout.timingMax, f.layout.maxCalls);
    assert.equal(f.layout.durations, f.layout.levels + f.layout.levelsLen, "durations follow levels");
    assert.equal(f.layout.timing, f.layout.durations + f.layout.durationsLen, "timing follows durations");
    assert.ok(f.layout.timing + f.layout.timingLen <= f.layout.base + f.layout.arena);
    assert.equal(
      f.layout.arena,
      75848,
      "the arena is the Phase 4 layout plus the resource regions, exactly as docs/RESOURCE-ABI.md derives",
    );
    assert.equal(f.memory.buffer.byteLength, 1179648, "linear memory stays at 18 pages");
  });

  it("publishes the checkpoint contract through the info export", () => {
    const f = fixture();
    assert.equal(f.layout.ckptRev, 1);
    assert.equal(f.layout.ckptLen, 32, "eight words, nothing more");
    assert.equal(f.layout.ckpt, f.layout.timing + f.layout.timingLen, "checkpoint follows timing");
    assert.ok(f.layout.ckpt + f.layout.ckptLen <= f.layout.base + f.layout.arena);
  });

  it("publishes the resource contract and tiles the arena to its end", () => {
    const f = fixture();
    assert.equal(f.layout.resRev, 1, "resource revision");
    assert.equal(f.layout.maxCapacityResources, 64);
    assert.equal(f.layout.maxDemandRecords, 1024);
    assert.equal(f.layout.maxDiagnostics, 64);
    assert.equal(f.layout.maxCapacity, 0x7fff_ffff);

    // The published chain must be contiguous, in the order the budget document
    // derives it, and the last region must end exactly at the arena's end: a
    // layout that drifts fails here rather than producing a wrong schedule.
    assert.equal(f.layout.capacities, f.layout.ckpt + f.layout.ckptLen, "capacities follow checkpoint");
    assert.equal(f.layout.demandOffsets, f.layout.capacities + f.layout.capacitiesLen);
    assert.equal(
      f.layout.demandRecords,
      f.layout.demandOffsets + f.layout.demandOffsetsLen,
      "records follow the offset vector",
    );
    assert.equal(f.layout.demandOffsetsLen, 4 * (f.layout.maxCalls + 1));
    assert.equal(f.layout.demandRecordsLen, 8 * f.layout.maxDemandRecords);
    assert.ok(f.layout.schedule > f.layout.demandRecords, "the schedule sits after the input regions");
    assert.equal(f.layout.stats + f.layout.statsLen, f.layout.diagnostics, "stats precede diagnostics");
    assert.ok(
      f.layout.base + f.layout.arena - (f.layout.diagnostics + f.layout.diagnosticsLen) < 8,
      "the arena ends at the diagnostics region, plus at most the eight-byte rounding",
    );

    // The sentinel boundary moved with the table; the next extension has to fail
    // a test rather than silently reuse a selector.
    assert.equal(f.info(52), -1, "an unknown selector still fails closed");
    assert.equal(f.info(0xffff_ffff), -1);
  });

  it("writes no timing through the lower entry points", () => {
    const f = fixture();
    assert.equal(encodeV2(f, DIAMOND.modes, DIAMOND.offsets, DIAMOND.ids, DIAMOND.accessModes), 0);
    assert.deepEqual(f.timing(), {
      revision: 0, count: 0, makespan: 0, peak: 0, totalWork: 0, criticalCount: 0, reserved: 0, events: [],
    });
  });

  it("analyses the worked diamond through the raw ABI", () => {
    const f = fixture();
    assert.equal(encodeV3(f, DIAMOND.modes, DIAMOND.offsets, DIAMOND.ids, DIAMOND.accessModes, [2, 5, 2, 3, 4]), 0);
    const timing = f.timing();
    assert.equal(timing.revision, 1);
    assert.equal(timing.count, 5);
    assert.equal(timing.makespan, 14);
    assert.equal(timing.totalWork, 16);
    assert.equal(timing.peak, 2);
    assert.equal(timing.criticalCount, 4);
    assert.equal(timing.reserved, 0);
    assert.deepEqual(timing.events, [[0, 0], [2, 0], [2, 3], [7, 0], [10, 0]]);
    assert.deepEqual(f.levels().entries, [0, 1, 1, 2, 3], "levels and timing must describe one graph");
  });

  it("refuses a zero duration through the raw ABI", () => {
    const f = fixture();
    assert.equal(encodeV3(f, DIAMOND.modes, DIAMOND.offsets, DIAMOND.ids, DIAMOND.accessModes, [2, 0, 2, 3, 4]), 118);
    assert.equal(f.timing().revision, 0, "a refused schedule leaves no readable header");
  });

  it("refuses a tick total that will not fit", () => {
    const f = fixture();
    assert.equal(encodeV3(f, DIAMOND.modes, DIAMOND.offsets, DIAMOND.ids, DIAMOND.accessModes, [2, 5, 2, 3, 0xffffffff]), 121);
    assert.equal(f.timing().revision, 0);
  });


  it("plans the whole batch and commits a checkpoint when handed an empty record", () => {
    const f = fixture();
    const batch = resumeBatch();
    assert.equal(encodeV4(f, batch), 0);
    const record = f.checkpoint();
    assert.equal(record.revision, 1);
    assert.equal(record.calls, batch.modes.length);
    assert.equal(record.edges, f.edges().length);
    assert.deepEqual(record.reserved, [0, 0, 0]);
    assert.ok(record.events > 0 && record.digest !== 0);
    const throughV4 = shape(f);
    encodeV3(f, batch.modes, batch.offsets, batch.ids, batch.accessModes, batch.ticks);
    assert.equal(shape(f), throughV4, "v4 must produce exactly the v3 plan");
  });

  it("reproduces the whole plan from every checkpoint boundary", () => {
    const f = fixture();
    const batch = resumeBatch();
    assert.equal(encodeV4(f, batch), 0);
    const whole = shape(f);
    for (let boundary = 0; boundary <= batch.modes.length; boundary++) {
      assert.equal(encodeV4(f, batch, boundary), 0, `planning the first ${boundary} calls failed`);
      const snap = f.snapshot();
      assert.equal(encodeV4(f, batch, batch.modes.length, snap), 0, `resume at ${boundary} failed`);
      assert.equal(shape(f), whole, `resume at ${boundary} disagrees with the whole run`);
    }
  });

  it("appends after a checkpoint exactly as a fresh longer batch would", () => {
    const f = fixture();
    const batch = resumeBatch();
    assert.equal(encodeV4(f, batch), 0);
    const before = f.snapshot();
    const grown: ResumeBatch = {
      modes: [...batch.modes, MODE.resources],
      offsets: [...batch.offsets, (batch.offsets.at(-1) ?? 0) + 1],
      ids: [...batch.ids, 1],
      accessModes: [...batch.accessModes, READ],
      ticks: [...batch.ticks, 6],
    };
    assert.equal(encodeV4(f, grown, grown.modes.length, before), 0);
    const appended = shape(f);
    assert.equal(encodeV4(f, grown), 0);
    assert.equal(shape(f), appended, "an appended call must not depend on how it arrived");
    assert.equal(f.checkpoint().calls, grown.modes.length);
  });

  it("refuses a checkpoint whose prefix is not this prefix", () => {
    const f = fixture();
    const batch = resumeBatch();
    assert.equal(encodeV4(f, batch, 3), 0);
    const stale = f.snapshot();
    const edited: ResumeBatch = { ...batch, accessModes: [WRITE, WRITE, READ, WRITE, READ] };
    assert.equal(encodeV4(f, edited, 5, stale), 131, "an edited prefix cannot extend the graph");
    assert.equal(f.checkpoint().revision, 0, "a refused resume leaves nothing resumable behind");
  });

  it("refuses a restored frontier or reader chain that could never have existed", () => {
    const f = fixture();
    const batch = resumeBatch();
    assert.equal(encodeV4(f, batch, 3), 0);
    const good = f.snapshot();

    const frontier = f.corrupt(good);
    new DataView(frontier.workspace.buffer).setUint32(4, 9, true);
    assert.equal(encodeV4(f, batch, 5, frontier), 131, "a last writer beyond the checkpoint is refused");

    const chain = f.corrupt(good);
    new DataView(chain.workspace.buffer).setUint32(f.layout.maxRes * 8 + 8, 40, true);
    assert.equal(encodeV4(f, batch, 5, chain), 131, "a reader link that runs forward is refused");

    const counters = f.corrupt(good);
    new DataView(counters.record.buffer).setUint32(12, 9000, true);
    assert.equal(encodeV4(f, batch, 5, counters), 132, "a record count the prefix cannot produce is refused");

    const digest = f.corrupt(good);
    new DataView(digest.record.buffer).setUint32(16, 0xdeadbeef, true);
    assert.equal(encodeV4(f, batch, 5, digest), 131, "the commitment is checked, not trusted");
  });

  it("refuses a non-canonical checkpoint region and never writes one from a lower version", () => {
    const f = fixture();
    const batch = resumeBatch();
    const { args } = pack(f, batch.modes, batch.offsets, batch.ids, batch.accessModes);
    const tail = [
      f.layout.levels, f.layout.levelsLen,
      f.layout.durations, f.layout.durationsLen,
      f.layout.timing, f.layout.timingLen,
    ];
    assert.equal(f.planV4(...args, ...tail, f.layout.out, f.layout.ckptLen), 108, "an aliasing pointer is refused");
    assert.equal(f.planV4(...args, ...tail, f.layout.ckpt, 8), 132, "a short region is refused");

    const bytes = new Uint8Array(f.memory.buffer);
    bytes.fill(0xff, f.layout.ckpt, f.layout.ckpt + f.layout.ckptLen);
    encodeV3(f, batch.modes, batch.offsets, batch.ids, batch.accessModes, batch.ticks);
    assert.ok(
      bytes.slice(f.layout.ckpt, f.layout.ckpt + f.layout.ckptLen).every((byte) => byte === 0xff),
      "the scheduling entry point must not touch the checkpoint region",
    );
  });

  it("refuses non-canonical scheduling regions", () => {
    const f = fixture();
    const { args } = pack(f, [MODE.resources], [0, 1], [1], [WRITE]);
    const head = args.slice(12); // levels pointer and length, already appended by args
    void head;
    const twelve = args.slice(0, 12);
    const L = f.layout;
    const call = (durations: number, durationsLen: number, timing: number, timingLen: number) =>
      f.planV3(...twelve, L.levels, L.levelsLen, durations, durationsLen, timing, timingLen);
    f.writeDurations([1]);
    assert.equal(call(L.durations, L.durationsLen, L.timing, L.timingLen), 0);
    assert.equal(call(L.out, L.durationsLen, L.timing, L.timingLen), 108, "durations must be the canonical region");
    assert.equal(call(L.durations, L.durationsLen, L.levels, L.timingLen), 108, "timing must be the canonical region");
    assert.equal(call(L.durations + 4, L.durationsLen, L.timing, L.timingLen), 108, "a shifted durations base");
    assert.equal(call(L.durations, 4, L.timing, L.timingLen), 119, "a short durations region");
    assert.equal(call(L.durations, L.durationsLen, L.timing, 8), 119, "a short timing region");
    assert.equal(call(L.base - 4, L.durationsLen, L.timing, L.timingLen), 104, "below the arena");
  });

  it("keeps planning and scheduling identical across repeated batches", () => {
    const f = fixture();
    // One chain, a two-call chain, and the diamond: makespans 7, 5 and 14.
    const shapes: Array<[number[], number[], number[], number[], number[], number]> = [
      [[MODE.resources], [0, 1], [1], [WRITE], [7], 7],
      [[MODE.resources, MODE.resources], [0, 1, 2], [1, 1], [WRITE, READ], [2, 3], 5],
      [DIAMOND.modes, DIAMOND.offsets, DIAMOND.ids, DIAMOND.accessModes, [2, 5, 2, 3, 4], 14],
    ];
    const seen: string[] = [];
    for (let round = 0; round < 3; round++) {
      for (const shape of shapes) {
        const [modes, offsets, ids, accessModes, durations, makespan] = shape;
        assert.equal(encodeV3(f, modes, offsets, ids, accessModes, durations), 0, `round ${round}`);
        const timing = f.timing();
        assert.equal(timing.count, modes.length);
        assert.equal(timing.revision, 1);
        assert.equal(timing.makespan, makespan);
        seen.push(JSON.stringify(timing));
      }
    }
    assert.equal(new Set(seen).size, 3, "distinct shapes must produce distinct analyses");
    assert.equal(seen.filter((row) => row === seen[0]).length, 3, "each shape repeats identically");
  });

  it("never traps on hostile scheduling arguments", () => {
    const f = fixture();
    let state = 0x51ed2701;
    const next = () => {
      state ^= state << 13; state >>>= 0;
      state ^= state >>> 17; state >>>= 0;
      state ^= state << 5; state >>>= 0;
      return state >>> 0;
    };
    const { args } = pack(f, [MODE.resources, MODE.resources], [0, 1, 2], [1, 1], [WRITE, READ]);
    const twelve = args.slice(0, 12);
    f.writeDurations([3, 4]);
    const beyond = f.layout.base + f.layout.arena;
    const statuses = new Map<number, number>();
    const roll = () => next() % 4;
    const maybe = (canonical: number, span: number) => (roll() < 3 ? canonical : next() % span);
    for (let i = 0; i < 20000; i++) {
      const calls = [
        ...twelve.slice(0, 3),
        maybe(twelve[3]!, beyond),
        ...twelve.slice(4),
        f.layout.levels,
        maybe(f.layout.levelsLen, 900),
        maybe(f.layout.durations, beyond),
        maybe(f.layout.durationsLen, 900),
        maybe(f.layout.timing, beyond),
        maybe(f.layout.timingLen, 4000),
      ];
      let status: number;
      try {
        status = f.planV3(...calls);
      } catch (cause) {
        assert.fail(`trap on scheduling arguments ${calls.join(",")}: ${String(cause)}`);
      }
      assert.ok(Number.isInteger(status) && status >= 0, `status must be a code, got ${status}`);
      assert.ok(status === 0 || (status >= 100 && status < 200), `undocumented status ${status}`);
      statuses.set(status, (statuses.get(status) ?? 0) + 1);
    }
    assert.ok(statuses.has(0), "valid scheduling combinations must still succeed");
    // The codes the randomised arguments can produce. 118 and 121 need
    // well-formed regions with hostile durations, so the sweep and the
    // dedicated tests below prove those deterministically instead.
    for (const code of [103, 104, 108, 116, 119]) {
      assert.ok(statuses.has(code), `the fuzz never reached status ${code}`);
    }
    // A sweep is the honest way to prove each slot is checked: one hostile
    // value at a time, everything else exactly right.
    const L2 = f.layout;
    const sweep: Array<[number[], number, string]> = [
      [[...twelve, L2.levels, L2.levelsLen, L2.durations, L2.durationsLen, L2.timing, 8], 119, "short timing region"],
      [[...twelve, L2.levels, L2.levelsLen, L2.durations, 12, L2.timing, L2.timingLen], 119, "short durations region"],
      [[...twelve, L2.levels, 12, L2.durations, L2.durationsLen, L2.timing, L2.timingLen], 116, "short levels region"],
      [[...twelve, L2.levels, L2.levelsLen, L2.out, L2.durationsLen, L2.timing, L2.timingLen], 108, "aliased durations"],
      [[...twelve, L2.levels, L2.levelsLen, L2.durations, L2.durationsLen, L2.durations, L2.timingLen], 108, "aliased timing"],
    ];
    for (const [calls, expected, why] of sweep) {
      f.writeDurations([3, 4]);
      assert.equal(f.planV3(...calls), expected, why);
    }
    const canonical = [
      ...twelve, f.layout.levels, f.layout.levelsLen,
      f.layout.durations, f.layout.durationsLen, f.layout.timing, f.layout.timingLen,
    ];
    f.writeDurations([3, 0]);
    assert.equal(f.planV3(...canonical), 118, "a zero duration is refused");
    // 1 + 4294967294 lands exactly on u32::MAX: the largest chain that fits.
    f.writeDurations([1, 0xffff_fffe]);
    assert.equal(f.planV3(...canonical), 0, "the largest legal chain must be accepted");
    assert.equal(f.timing().makespan, 0xffff_ffff, "and report its exact makespan");
    f.writeDurations([0xffff_ffff, 1]);
    assert.equal(f.planV3(...canonical), 121, "a chained makespan must not wrap");
    // Fail-closed: the region is only ever committed by a finished pass. The
    // durations go back to a batch that fits before that pass is re-run.
    f.writeDurations([3, 4]);
    assert.equal(
      f.planV3(
        ...twelve, f.layout.levels, f.layout.levelsLen,
        f.layout.durations, f.layout.durationsLen, f.layout.timing, f.layout.timingLen,
      ),
      0,
    );
    assert.deepEqual(f.timing().events, [[0, 0], [3, 0]]);
  });
});
