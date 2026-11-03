/**
 * Engine loading: obtains the WASM bytes, instantiates the module without any
 * binding framework, and proves that the memory the engine describes is the
 * memory the host is about to write into. Nothing here plans anything.
 */

import {
  EDGE_BYTES,
  EXPECTED_QUALITY_ABI,
  QUALITY_RECORD_BYTES,
  ENGINE_NAME,
  ENGINE_VERSION,
  EffectPlanError,
  EXPECTED_ABI,
  EXPECTED_LEVELS_ABI,
  EXPECTED_TIMING_ABI,
  EXPECTED_CHECKPOINT_ABI,
  EXPECTED_RESOURCE_ABI,
  CHECKPOINT_RECORD_BYTES,
  HEADER_BYTES,
  LEVELS_HEADER_BYTES,
  TIMING_HEADER_BYTES,
} from "./errors.js";

export interface InitializationOptions {
  /** Local WASM bytes. Required in browsers; Node reads the packaged file when absent. */
  readonly wasmBytes?: Uint8Array | ArrayBuffer;
  /** Explicit host-provided URL of the packaged WASM. Never a default. */
  readonly wasmUrl?: string | URL;
}

export interface EngineLimits {
  readonly maxCalls: number;
  readonly maxAccesses: number;
  readonly maxResources: number;
  readonly maxEvents: number;
  readonly maxEdges: number;
  readonly maxNamespaceBytes: number;
  readonly maxResourceIdBytes: number;
  /** Capacity resources one batch may declare. */
  readonly maxCapacityResources: number;
  /** Demand records one batch may carry. */
  readonly maxDemandRecords: number;
  /** Diagnostics one run may record. */
  readonly maxResourceDiagnostics: number;
  /** The largest capacity or demand the engine can represent. */
  readonly maxCapacity: number;
}

export interface EngineRegion {
  readonly ptr: number;
  readonly bytes: number;
}

export interface EngineLayout {
  readonly arena: EngineRegion;
  readonly input: EngineRegion;
  readonly workspace: EngineRegion;
  readonly output: EngineRegion;
  readonly levels: EngineRegion;
  /** Duration input, one `u32` per call slot. */
  readonly durations: EngineRegion;
  /** Scheduling output: header plus two `u32` per call. */
  readonly timing: EngineRegion;
  /** Resumable-planning record: eight words the engine commits and reads back. */
  readonly checkpoint: EngineRegion;
  /** The Phase 6 quality certificate: bounds, objective components, one claim. */
  readonly quality: EngineRegion;
  /** Declared capacity per capacity resource, one `u32` each. */
  readonly capacities: EngineRegion;
  /** Demand offsets: one `u32` per call, plus the terminator. */
  readonly demandOffsets: EngineRegion;
  /** Demand records: `(resource, amount)` pairs, flattened. */
  readonly demandRecords: EngineRegion;
  /** Placement output: a header plus `(start, finish)` per call. */
  readonly schedule: EngineRegion;
  /** Per-resource capacity, peak and busy time. */
  readonly resourceStats: EngineRegion;
  /** Why a demand could not be met, by index. */
  readonly resourceDiagnostics: EngineRegion;
}

export interface EngineInfo {
  readonly engineName: string;
  readonly engineVersion: string;
  readonly abiVersion: number;
  /** Revision of the execution-levels contract. */
  readonly levelsRevision: number;
  /** Revision of the scheduling contract. */
  readonly timingRevision: number;
  /** Revision of the resumable-planning contract. */
  readonly checkpointRevision: number;
  readonly wasmActive: boolean;
  readonly memoryBytes: number;
  readonly limits: EngineLimits;
  readonly layout: EngineLayout;
}

/** `effectplan_info` selectors; must stay in step with the Rust engine. */
const I_ABI = 0, I_BASE = 1, I_ARENA_LEN = 2, I_IN_PTR = 3, I_IN_LEN = 4,
  I_WS_PTR = 5, I_WS_LEN = 6, I_OUT_PTR = 7, I_OUT_LEN = 8, I_MAX_CALLS = 9,
  I_MAX_ACCESS = 10, I_MAX_RES = 11, I_MAX_EVENTS = 12, I_MAX_EDGES = 13,
  I_EDGE_BYTES = 14, I_MAX_NS = 15, I_MAX_ID = 16, I_HDR_BYTES = 17,
  I_LEVELS_REV = 21, I_LEVELS_PTR = 22, I_LEVELS_LEN = 23, I_LEVELS_MAX = 24,
  I_TIMING_REV = 25, I_DUR_PTR = 26, I_DUR_LEN = 27, I_DUR_MAX = 28,
  I_TIMING_PTR = 29, I_TIMING_LEN = 30, I_TIMING_MAX = 31,
  I_CKPT_REV = 32, I_CKPT_PTR = 33, I_CKPT_LEN = 34,
  I_RES_REV = 35, I_CAPS_PTR = 36, I_CAPS_LEN = 37,
  I_OFFS_PTR = 38, I_OFFS_LEN = 39, I_RECS_PTR = 40, I_RECS_LEN = 41,
  I_SCHED_PTR = 42, I_SCHED_LEN = 43, I_STATS_PTR = 44, I_STATS_LEN = 45,
  I_DIAG_PTR = 46, I_DIAG_LEN = 47,
  I_MAX_CAPS = 48, I_MAX_RECORDS = 49, I_MAX_DIAGS = 50, I_MAX_CAPACITY = 51,
  I_QUALITY_REV = 52, I_QUALITY_PTR = 53, I_QUALITY_LEN = 54;

/** The raw engine entry point as exported by the wasm module. */
export type PlanFn = (
  abi: number, calls: number, accesses: number, resources: number,
  callModes: number, accessOffsets: number, resourceIds: number, accessModes: number,
  workspace: number, workspaceBytes: number, output: number, outputBytes: number,
) => number;

/** The extended entry point: the graph, and the execution levels. */
export type PlanV2Fn = (
  abi: number, calls: number, accesses: number, resources: number,
  callModes: number, accessOffsets: number, resourceIds: number, accessModes: number,
  workspace: number, workspaceBytes: number, output: number, outputBytes: number,
  levels: number, levelsBytes: number,
) => number;

/** The scheduling entry point: levels plus duration-aware analysis. */
export type PlanV3Fn = (
  abi: number, calls: number, accesses: number, resources: number,
  callModes: number, accessOffsets: number, resourceIds: number, accessModes: number,
  workspace: number, workspaceBytes: number, output: number, outputBytes: number,
  levels: number, levelsBytes: number,
  durations: number, durationsBytes: number, timing: number, timingBytes: number,
) => number;

/**
 * The resumable entry point: the same pipeline as version three, plus the
 * checkpoint region. A host that leaves the record empty plans the whole batch;
 * a host that writes one back resumes from it.
 */
export type PlanV4Fn = (
  abi: number, calls: number, accesses: number, resources: number,
  callModes: number, accessOffsets: number, resourceIds: number, accessModes: number,
  workspace: number, workspaceBytes: number, output: number, outputBytes: number,
  levels: number, levelsBytes: number,
  durations: number, durationsBytes: number, timing: number, timingBytes: number,
  checkpoint: number, checkpointBytes: number,
) => number;

/**
 * The capacity check: declarations only. No planning region appears among its
 * arguments, so a check cannot disturb the graph.
 */
export type ResourceCheckFn = (
  abi: number, calls: number, resources: number, demands: number,
  capacities: number, capacitiesBytes: number,
  offsets: number, offsetsBytes: number,
  records: number, recordsBytes: number,
  diagnostics: number, diagnosticsBytes: number,
) => number;

/**
 * The placement: the same declarations, plus the graph and the timings it
 * schedules against, and the two regions it writes.
 */
export type ResourceScheduleFn = (
  abi: number, calls: number, resources: number, demands: number,
  capacities: number, capacitiesBytes: number,
  offsets: number, offsetsBytes: number,
  records: number, recordsBytes: number,
  durations: number, durationsBytes: number,
  graph: number, graphBytes: number,
  timing: number, timingBytes: number,
  schedule: number, scheduleBytes: number,
  stats: number, statsBytes: number,
  diagnostics: number, diagnosticsBytes: number,
) => number;

/**
 * The Phase 6 certificate: reads a placement already made, writes only its own
 * region, and can be re-run over the same placement without disturbing it.
 */
export type ResourceQualityFn = (
  abi: number, calls: number, resources: number,
  schedule: number, scheduleBytes: number,
  timing: number, timingBytes: number,
  stats: number, statsBytes: number,
  capacities: number, capacitiesBytes: number,
  quality: number, qualityBytes: number,
) => number;

/** An instantiated engine that has passed every host-side layout check. */
export interface LoadedEngine {
  readonly memory: WebAssembly.Memory;
  readonly plan: PlanFn;
  readonly planV2: PlanV2Fn;
  readonly planV3: PlanV3Fn;
  readonly planV4: PlanV4Fn;
  readonly resourceCheck: ResourceCheckFn;
  readonly resourceSchedule: ResourceScheduleFn;
  readonly resourceQuality: ResourceQualityFn;
  readonly info: EngineInfo;
}

type InfoReader = (kind: number) => number;

function unavailable(why: string): never {
  throw new EffectPlanError("EP_ENGINE_UNAVAILABLE", why);
}

export async function loadEngine(options: InitializationOptions = {}): Promise<LoadedEngine> {
  const bytes = await loadWasmBytes(options);
  const instance = await instantiate(bytes);
  const exports = instance.exports;
  const infoFn = exports["effectplan_info"];
  const planFn = exports["effectplan_plan"];
  const planV2Fn = exports["effectplan_plan_v2"];
  const memory = exports["memory"];
  if (typeof infoFn !== "function" || typeof planFn !== "function" || !(memory instanceof WebAssembly.Memory)) {
    unavailable("required exports missing");
  }
  if (typeof planV2Fn !== "function") {
    unavailable("engine does not support execution levels");
  }
  const planV3Fn = exports["effectplan_plan_v3"];
  if (typeof planV3Fn !== "function") {
    unavailable("engine does not support scheduling analysis");
  }
  const planV4Fn = exports["effectplan_plan_v4"];
  if (typeof planV4Fn !== "function") {
    unavailable("engine does not support resumable planning");
  }
  const resourceCheckFn = exports["effectplan_resource_check"];
  if (typeof resourceCheckFn !== "function") {
    unavailable("engine does not support capacity resources");
  }
  const resourceScheduleFn = exports["effectplan_resource_schedule"];
  if (typeof resourceScheduleFn !== "function") {
    unavailable("engine cannot schedule against capacity resources");
  }
  const resourceQualityFn = exports["effectplan_resource_quality"];
  if (typeof resourceQualityFn !== "function") {
    unavailable("engine cannot certify schedule quality");
  }
  const read = infoFn as InfoReader;
  const abi = read(I_ABI);
  if (abi !== EXPECTED_ABI) unavailable(`abi ${abi} incompatible with ${EXPECTED_ABI}`);
  if (read(I_EDGE_BYTES) !== EDGE_BYTES || read(I_HDR_BYTES) !== HEADER_BYTES) {
    unavailable("unexpected record framing");
  }
  const levelsRevision = read(I_LEVELS_REV);
  if (levelsRevision !== EXPECTED_LEVELS_ABI) {
    unavailable(`execution levels revision ${levelsRevision} incompatible with ${EXPECTED_LEVELS_ABI}`);
  }
  const timingRevision = read(I_TIMING_REV);
  if (timingRevision !== EXPECTED_TIMING_ABI) {
    unavailable(`timing revision ${timingRevision} incompatible with ${EXPECTED_TIMING_ABI}`);
  }
  const checkpointRevision = read(I_CKPT_REV);
  if (checkpointRevision !== EXPECTED_CHECKPOINT_ABI) {
    unavailable(
      `checkpoint revision ${checkpointRevision} incompatible with ${EXPECTED_CHECKPOINT_ABI}`,
    );
  }

  const base = read(I_BASE);
  const arenaBytes = read(I_ARENA_LEN);
  const inputPtr = read(I_IN_PTR), inputBytes = read(I_IN_LEN);
  const wsPtr = read(I_WS_PTR), wsBytes = read(I_WS_LEN);
  const outPtr = read(I_OUT_PTR), outBytes = read(I_OUT_LEN);
  const levelsPtr = read(I_LEVELS_PTR), levelsBytes = read(I_LEVELS_LEN);
  const levelsMax = read(I_LEVELS_MAX);
  const durationsPtr = read(I_DUR_PTR), durationsBytes = read(I_DUR_LEN), durationsMax = read(I_DUR_MAX);
  const timingPtr = read(I_TIMING_PTR), timingBytes = read(I_TIMING_LEN), timingMax = read(I_TIMING_MAX);
  const ckptPtr = read(I_CKPT_PTR), ckptBytes = read(I_CKPT_LEN);
  if (ckptBytes !== CHECKPOINT_RECORD_BYTES) {
    unavailable(`checkpoint region is ${ckptBytes} bytes, expected ${CHECKPOINT_RECORD_BYTES}`);
  }
  const resourceRevision = read(I_RES_REV);
  if (resourceRevision !== EXPECTED_RESOURCE_ABI) {
    unavailable(`resource revision ${resourceRevision} incompatible with ${EXPECTED_RESOURCE_ABI}`);
  }
  const capsPtr = read(I_CAPS_PTR), capsBytes = read(I_CAPS_LEN);
  const offsPtr = read(I_OFFS_PTR), offsBytes = read(I_OFFS_LEN);
  const recsPtr = read(I_RECS_PTR), recsBytes = read(I_RECS_LEN);
  const schedPtr = read(I_SCHED_PTR), schedBytes = read(I_SCHED_LEN);
  const statsPtr = read(I_STATS_PTR), statsBytes = read(I_STATS_LEN);
  const diagPtr = read(I_DIAG_PTR), diagBytes = read(I_DIAG_LEN);
  const qualityPtr = read(I_QUALITY_PTR), qualityBytes = read(I_QUALITY_LEN);
  const qualityRevision = read(I_QUALITY_REV);
  if (qualityRevision !== EXPECTED_QUALITY_ABI) {
    unavailable(`quality revision ${qualityRevision} incompatible with ${EXPECTED_QUALITY_ABI}`);
  }
  if (qualityBytes !== QUALITY_RECORD_BYTES) {
    unavailable(`quality region is ${qualityBytes} bytes, expected ${QUALITY_RECORD_BYTES}`);
  }
  const maxCapacityResources = read(I_MAX_CAPS);
  const maxDemandRecords = read(I_MAX_RECORDS);
  const maxResourceDiagnostics = read(I_MAX_DIAGS);
  const maxCapacity = read(I_MAX_CAPACITY);
  const limits: EngineLimits = Object.freeze({
    maxCalls: read(I_MAX_CALLS),
    maxAccesses: read(I_MAX_ACCESS),
    maxResources: read(I_MAX_RES),
    maxEvents: read(I_MAX_EVENTS),
    maxEdges: read(I_MAX_EDGES),
    maxNamespaceBytes: read(I_MAX_NS),
    maxResourceIdBytes: read(I_MAX_ID),
    maxCapacityResources,
    maxDemandRecords,
    maxResourceDiagnostics,
    maxCapacity,
  });
  // The host is about to hand the engine absolute addresses, so every region
  // must sit exactly where the engine said it does: four-byte aligned,
  // contiguous from the arena base, sized from the published ceilings, and
  // inside live memory. A region that fails this audit could let a batch reach
  // memory the engine does not own, so nothing is written until it passes.
  if (memory.buffer.byteLength < base + arenaBytes) unavailable("arena outside linear memory");
  const maxEdges = limits.maxEdges;
  const maxCalls = limits.maxCalls;
  // name, pointer, length, entry count, header bytes, entry width. A count of
  // -1 means the region is audited by position only. The last field marks the
  // one place where the engine keeps scratch of its own between two published
  // regions: it is not published, so the host neither names it nor writes it.
  const regions: readonly (readonly [string, number, number, number, number, number, number])[] = [
    ["input", inputPtr, inputBytes, limits.maxAccesses, maxCalls + (maxCalls + 1) * 4, 5, 0],
    ["workspace", wsPtr, wsBytes, -1, wsBytes, 0, 0],
    ["output", outPtr, outBytes, maxEdges, HEADER_BYTES, EDGE_BYTES, 0],
    ["levels", levelsPtr, levelsBytes, levelsMax, LEVELS_HEADER_BYTES, 2, 0],
    ["durations", durationsPtr, durationsBytes, durationsMax, 0, 4, 0],
    ["timing", timingPtr, timingBytes, timingMax, TIMING_HEADER_BYTES, 8, 0],
    ["checkpoint", ckptPtr, ckptBytes, -1, CHECKPOINT_RECORD_BYTES, 0, 0],
    ["capacities", capsPtr, capsBytes, maxCapacityResources, 0, 4, 0],
    ["demand offsets", offsPtr, offsBytes, maxCalls + 1, 0, 4, 0],
    ["demand records", recsPtr, recsBytes, maxDemandRecords, 0, 8, 0],
    ["schedule", schedPtr, schedBytes, maxCalls, 32, 8, 1],
    ["resource stats", statsPtr, statsBytes, maxCapacityResources, 32, 16, 0],
    ["resource diagnostics", diagPtr, diagBytes, maxResourceDiagnostics, 32, 16, 0],
    ["quality", qualityPtr, qualityBytes, -1, QUALITY_RECORD_BYTES, 0, 0],
  ];
  let cursor = base;
  // The three regions that publish their own capacity must publish the one the
  // limits describe, or the host would size its writes from a number the engine
  // and the host disagree about.
  if (levelsMax !== maxCalls) unavailable("levels region publishes the wrong capacity");
  if (durationsMax !== maxCalls) unavailable("durations region publishes the wrong capacity");
  if (timingMax !== maxCalls) unavailable("timing region publishes the wrong capacity");
  for (const [name, ptr, bytes, entries, header, width, gap] of regions) {
    if (ptr % 4 !== 0) unavailable(`unaligned ${name} region base`);
    if (gap === 1) {
      if (ptr < cursor) unavailable(`overlapping ${name} region`);
    } else if (ptr !== cursor) {
      unavailable(`non-contiguous ${name} region`);
    }
    if (entries !== -1 && bytes !== header + entries * width) {
      unavailable(`${name} region does not match its declared capacity`);
    }
    if (ptr + bytes > base + arenaBytes) unavailable(`${name} region runs past the arena`);
    cursor = ptr + bytes;
  }

  const layout: EngineLayout = Object.freeze({
    arena: Object.freeze({ ptr: base, bytes: arenaBytes }),
    input: Object.freeze({ ptr: inputPtr, bytes: inputBytes }),
    workspace: Object.freeze({ ptr: wsPtr, bytes: wsBytes }),
    output: Object.freeze({ ptr: outPtr, bytes: outBytes }),
    levels: Object.freeze({ ptr: levelsPtr, bytes: levelsBytes }),
    durations: Object.freeze({ ptr: durationsPtr, bytes: durationsBytes }),
    timing: Object.freeze({ ptr: timingPtr, bytes: timingBytes }),
    checkpoint: Object.freeze({ ptr: ckptPtr, bytes: ckptBytes }),
    capacities: Object.freeze({ ptr: capsPtr, bytes: capsBytes }),
    demandOffsets: Object.freeze({ ptr: offsPtr, bytes: offsBytes }),
    demandRecords: Object.freeze({ ptr: recsPtr, bytes: recsBytes }),
    schedule: Object.freeze({ ptr: schedPtr, bytes: schedBytes }),
    resourceStats: Object.freeze({ ptr: statsPtr, bytes: statsBytes }),
    resourceDiagnostics: Object.freeze({ ptr: diagPtr, bytes: diagBytes }),
    quality: Object.freeze({ ptr: qualityPtr, bytes: qualityBytes }),
  });
  const info: EngineInfo = Object.freeze({
    engineName: ENGINE_NAME,
    engineVersion: ENGINE_VERSION,
    abiVersion: abi,
    levelsRevision,
    timingRevision,
    checkpointRevision,
    wasmActive: true,
    memoryBytes: memory.buffer.byteLength,
    limits,
    layout,
  });
  return {
    memory,
    plan: planFn as unknown as PlanFn,
    planV2: planV2Fn as unknown as PlanV2Fn,
    planV3: planV3Fn as unknown as PlanV3Fn,
    planV4: planV4Fn as unknown as PlanV4Fn,
    resourceCheck: resourceCheckFn as unknown as ResourceCheckFn,
    resourceSchedule: resourceScheduleFn as unknown as ResourceScheduleFn,
    resourceQuality: resourceQualityFn as unknown as ResourceQualityFn,
    info,
  };
}

/**
 * Compiles and instantiates without any binding framework. `compile` plus
 * `new Instance` is used rather than the buffer overload of `instantiate`,
 * whose return type differs between the ECMAScript and DOM library definitions.
 */
async function instantiate(source: Uint8Array): Promise<WebAssembly.Instance> {
  try {
    // The copy normalises any view (offset-backed or shared-backed) into a plain
    // ArrayBuffer buffer, which is what the compile signature demands.
    const bytes = new Uint8Array(source);
    const module = await WebAssembly.compile(bytes);
    return new WebAssembly.Instance(module, {});
  } catch (cause) {
    unavailable(`wasm instantiate failed: ${String(cause)}`);
  }
}

async function loadWasmBytes(options: InitializationOptions): Promise<Uint8Array> {
  const provided = options.wasmBytes;
  if (provided !== undefined) {
    return provided instanceof Uint8Array ? provided : new Uint8Array(provided);
  }
  if (options.wasmUrl !== undefined) {
    const url = options.wasmUrl;
    if (typeof url === "string" && /^https?:/i.test(url)) {
      // Only for an explicit host-provided URL; the package never fetches by default.
      const response = await fetch(url);
      if (!response.ok) {
        throw new EffectPlanError("EP_ENGINE_UNAVAILABLE", `wasm fetch status ${response.status}`);
      }
      return new Uint8Array(await response.arrayBuffer());
    }
    return readFileAsBytes(typeof url === "string" ? new URL(url) : url);
  }
  return readFileAsBytes(new URL("./effectplan.wasm", import.meta.url));
}

async function readFileAsBytes(url: URL): Promise<Uint8Array> {
  try {
    // Dynamic import keeps the browser bundle free of any Node-only specifier.
    const fs = await import("node:fs/promises");
    return new Uint8Array(await fs.readFile(url));
  } catch (cause) {
    throw new EffectPlanError(
      "EP_ENGINE_UNAVAILABLE",
      `wasm bytes unavailable, pass InitializationOptions.wasmBytes in a browser: ${String(cause)}`,
    );
  }
}
