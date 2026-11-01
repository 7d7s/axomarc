/**
 * Engine loading: obtains the WASM bytes, instantiates the module without any
 * binding framework, and proves that the memory the engine describes is the
 * memory the host is about to write into. Nothing here plans anything.
 */

import {
  EDGE_BYTES,
  ENGINE_NAME,
  ENGINE_VERSION,
  EffectPlanError,
  EXPECTED_ABI,
  EXPECTED_LEVELS_ABI,
  EXPECTED_TIMING_ABI,
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
}

export interface EngineInfo {
  readonly engineName: string;
  readonly engineVersion: string;
  readonly abiVersion: number;
  /** Revision of the execution-levels contract. */
  readonly levelsRevision: number;
  /** Revision of the scheduling contract. */
  readonly timingRevision: number;
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
  I_TIMING_PTR = 29, I_TIMING_LEN = 30, I_TIMING_MAX = 31;

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

/** An instantiated engine that has passed every host-side layout check. */
export interface LoadedEngine {
  readonly memory: WebAssembly.Memory;
  readonly plan: PlanFn;
  readonly planV2: PlanV2Fn;
  readonly planV3: PlanV3Fn;
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

  const base = read(I_BASE);
  const arenaBytes = read(I_ARENA_LEN);
  const inputPtr = read(I_IN_PTR), inputBytes = read(I_IN_LEN);
  const wsPtr = read(I_WS_PTR), wsBytes = read(I_WS_LEN);
  const outPtr = read(I_OUT_PTR), outBytes = read(I_OUT_LEN);
  const levelsPtr = read(I_LEVELS_PTR), levelsBytes = read(I_LEVELS_LEN);
  const levelsMax = read(I_LEVELS_MAX);
  const durationsPtr = read(I_DUR_PTR), durationsBytes = read(I_DUR_LEN), durationsMax = read(I_DUR_MAX);
  const timingPtr = read(I_TIMING_PTR), timingBytes = read(I_TIMING_LEN), timingMax = read(I_TIMING_MAX);
  const limits: EngineLimits = Object.freeze({
    maxCalls: read(I_MAX_CALLS),
    maxAccesses: read(I_MAX_ACCESS),
    maxResources: read(I_MAX_RES),
    maxEvents: read(I_MAX_EVENTS),
    maxEdges: read(I_MAX_EDGES),
    maxNamespaceBytes: read(I_MAX_NS),
    maxResourceIdBytes: read(I_MAX_ID),
  });
  // The host is about to hand the engine absolute addresses, so every region
  // must sit exactly where the engine said it does: four-byte aligned,
  // contiguous from the arena base, sized from the published ceilings, and
  // inside live memory. A region that fails this audit could let a batch reach
  // memory the engine does not own, so nothing is written until it passes.
  if (memory.buffer.byteLength < base + arenaBytes) unavailable("arena outside linear memory");
  const maxEdges = limits.maxEdges;
  const maxCalls = limits.maxCalls;
  // name, pointer, length, and the entry count the engine publishes for it.
  const regions: readonly (readonly [string, number, number, number])[] = [
    ["input", inputPtr, inputBytes, limits.maxAccesses],
    ["workspace", wsPtr, wsBytes, -1], // engine-private state, length audited only by position
    ["output", outPtr, outBytes, maxEdges],
    ["levels", levelsPtr, levelsBytes, read(I_LEVELS_MAX)],
    ["durations", durationsPtr, durationsBytes, read(I_DUR_MAX)],
    ["timing", timingPtr, timingBytes, read(I_TIMING_MAX)],
  ];
  // Each region's size is derived from a ceiling the engine also publishes, so
  // a binary that disagrees with itself is refused rather than trusted.
  // Per entry: input packs one byte plus one id and one mode per access, the
  // rest are the plain arrays their names describe.
  const widths = [5, 0, EDGE_BYTES, 2, 4, 8];
  const headers = [
    maxCalls + (maxCalls + 1) * 4, // call modes plus the CSR offsets
    wsBytes,
    HEADER_BYTES,
    LEVELS_HEADER_BYTES,
    0,
    TIMING_HEADER_BYTES,
  ];
  let cursor = base;
  for (let index = 0; index < regions.length; index++) {
    const [name, ptr, bytes, entries] = regions[index]!;
    if (ptr % 4 !== 0) unavailable(`unaligned ${name} region base`);
    if (ptr !== cursor) unavailable(`non-contiguous ${name} region`);
    if (entries !== -1) {
      const capacity = name === "input" ? limits.maxAccesses : name === "levels" || name === "durations" || name === "timing" ? maxCalls : maxEdges;
      if (entries !== capacity) unavailable(`${name} region publishes the wrong capacity`);
      if (bytes !== headers[index]! + entries * widths[index]!) {
        unavailable(`${name} region does not match its declared capacity`);
      }
    }
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
  });
  const info: EngineInfo = Object.freeze({
    engineName: ENGINE_NAME,
    engineVersion: ENGINE_VERSION,
    abiVersion: abi,
    levelsRevision,
    timingRevision,
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
