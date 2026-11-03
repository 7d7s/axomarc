/**
 * Input encoding: the only place that touches caller objects. It validates the
 * public contract, interns resource identities into request-local integers,
 * normalises duplicate accesses and writes the packed arrays the ABI expects.
 * The conflict algorithm itself lives in Rust.
 */

import { EffectPlanError } from "./errors.js";
import type { EngineInfo, EngineLimits } from "./loader.js";
import type { TimingOptions } from "./timing.js";

export type EffectMode = "pure" | "resources" | "exclusive" | "unknown";
export type AccessMode = "read" | "write";
export type UnknownEffectPolicy = "serialize" | "reject";

export interface ResourceRef {
  readonly namespace: string;
  readonly id: string;
}

export interface ResourceAccess {
  readonly resource: ResourceRef;
  readonly mode: AccessMode;
}

export interface EffectContract {
  readonly version: 1;
  readonly mode: EffectMode;
  readonly accesses?: readonly ResourceAccess[];
}

export interface ToolCall {
  readonly id: string;
  readonly effect?: EffectContract;
}

export interface PlanOptions {
  readonly unknownEffectPolicy?: UnknownEffectPolicy;
  /**
   * Ask for duration-aware analysis: earliest and latest times, slack, the
   * critical set and the concurrency envelope. Omit it and the batch plans
   * exactly as it did before timing existed.
   */
  readonly timing?: TimingOptions;
}

/** Packed call-mode code, matching the Rust `EffectMode` repr. */
const MODE_CODE: Record<EffectMode, number> = { pure: 0, resources: 1, exclusive: 2, unknown: 3 };
/** Packed access-mode code, matching the Rust `AccessMode` repr. */
const ACCESS_WRITE = 1;

/** Absolute addresses of the packed regions, as the ABI wants them. */
export interface InputPointers {
  readonly callModes: number;
  readonly accessOffsets: number;
  readonly resourceIds: number;
  readonly accessModes: number;
}

/** What one batch contributed: counts for the ABI call, decode and statistics. */
export interface EncodedBatch {
  readonly callCount: number;
  readonly accessCount: number;
  readonly resourceCount: number;
  readonly unknownEffectCount: number;
  readonly exclusiveCount: number;
  readonly pointers: InputPointers;
}

/** Views over the engine's input region, sized to the capacity it declared. */
export interface PackedInput {
  readonly buffer: ArrayBufferLike;
  readonly view: DataView;
  readonly callModes: Uint8Array;
  readonly offsets: Uint32Array;
  readonly resourceIds: Uint32Array;
  readonly accessModes: Uint8Array;
  readonly durations: Uint32Array;
}

/**
 * Maps one live buffer. Growing the memory detaches the previous buffer and
 * every view on it, so the caller must re-map whenever the engine's buffer
 * identity changes; the views cannot reach past the region the engine declared.
 */
export function packInput(buffer: ArrayBufferLike, info: EngineInfo): PackedInput {
  const maxCalls = info.limits.maxCalls;
  const maxAccesses = info.limits.maxAccesses;
  const base = info.layout.arena.ptr;
  const offsetsAt = base + maxCalls;
  const idsAt = offsetsAt + (maxCalls + 1) * 4;
  const modesAt = idsAt + maxAccesses * 4;
  return {
    buffer,
    view: new DataView(buffer),
    callModes: new Uint8Array(buffer, base, maxCalls),
    offsets: new Uint32Array(buffer, offsetsAt, maxCalls + 1),
    resourceIds: new Uint32Array(buffer, idsAt, maxAccesses),
    accessModes: new Uint8Array(buffer, modesAt, maxAccesses),
    durations: new Uint32Array(buffer, info.layout.durations.ptr, maxCalls),
  };
}

/** UTF-8 byte length, computed without allocating an encoder for it. */
export function utf8Length(value: string): number {
  let bytes = 0;
  for (const ch of value) {
    const c = ch.codePointAt(0)!;
    bytes += c < 0x80 ? 1 : c < 0x800 ? 2 : c < 0x10000 ? 3 : 4;
  }
  return bytes;
}

/**
 * Turns resource identities into the dense integers the engine reads. The table
 * is separate from the batch loop because a session keeps one alive across
 * updates: renumbering the resources of a plan that is being reused would
 * silently relabel every dependency in the preserved prefix.
 */
export interface Interner {
  id(namespace: string, id: string): number;
  /** The `resource_count` to pass, one past the highest id allocated. */
  readonly count: number;
}

export function createInterner(limits: EngineLimits): Interner {
  const table = new Map<string, number>();
  return {
    id(namespace: string, id: string): number {
      // Length prefixes keep "a:bc" and "ab:c" from colliding.
      const key = `${namespace.length}:${namespace}${id.length}:${id}`;
      let rid = table.get(key);
      if (rid === undefined) {
        rid = table.size + 1;
        if (rid > limits.maxResources) {
          throw new EffectPlanError(
            "EP_LIMIT_EXCEEDED",
            `${rid} resources exceeds ${limits.maxResources}`,
          );
        }
        table.set(key, rid);
      }
      return rid;
    },
    get count(): number {
      return table.size + 1;
    },
  };
}

/** One validated call, normalised for packing: one access per resource, write wins. */
export interface NormalizedCall {
  /** The caller's identifier, validated for shape but not for uniqueness. */
  readonly id: string;
  readonly mode: number;
  readonly rids: readonly number[];
  readonly accessModes: readonly number[];
}

/** Validates a single public call against the contract and the engine's limits. */
export function normalizeCall(
  call: ToolCall,
  index: number,
  limits: EngineLimits,
  policy: UnknownEffectPolicy,
  intern: Interner,
): NormalizedCall {
  const where = `call ${index}`;
  if (typeof call !== "object" || call === null) {
    throw new EffectPlanError("EP_INVALID_INPUT", `${where} must be an object`);
  }
  const id = call.id;
  if (typeof id !== "string" || id.length === 0) {
    throw new EffectPlanError("EP_INVALID_INPUT", `${where} needs a non-empty string id`);
  }

  const declared = call.effect;
  let mode: EffectMode = "unknown";
  let accesses: readonly ResourceAccess[] | undefined;
  if (declared !== undefined && declared !== null) {
    if (typeof declared !== "object") {
      throw new EffectPlanError("EP_INVALID_EFFECT", `${where} effect must be an object`);
    }
    if (declared.version !== 1) {
      throw new EffectPlanError("EP_INVALID_EFFECT", `${where} effect version`);
    }
    if (typeof declared.mode !== "string" || !(declared.mode in MODE_CODE)) {
      throw new EffectPlanError("EP_INVALID_EFFECT", `${where} effect mode`);
    }
    mode = declared.mode;
    accesses = declared.accesses;
  }
  if (accesses !== undefined && !Array.isArray(accesses)) {
    throw new EffectPlanError("EP_INVALID_EFFECT", `${where} accesses must be an array`);
  }
  if (mode === "pure" && accesses !== undefined && accesses.length > 0) {
    throw new EffectPlanError("EP_INVALID_EFFECT", `${where} is pure but declares accesses`);
  }
  if (mode === "resources" && (accesses === undefined || accesses.length === 0)) {
    throw new EffectPlanError("EP_INVALID_EFFECT", `${where} requires at least one access`);
  }
  if (mode === "unknown" && policy === "reject") {
    throw new EffectPlanError("EP_UNKNOWN_EFFECT_REJECTED", where);
  }

  const effective = new Map<number, number>();
  if (accesses !== undefined) {
    for (const access of accesses) {
      if (typeof access !== "object" || access === null) {
        throw new EffectPlanError("EP_INVALID_RESOURCE", `${where} access shape`);
      }
      const ref = access.resource;
      if (
        typeof ref !== "object" || ref === null ||
        typeof ref.namespace !== "string" || typeof ref.id !== "string" ||
        ref.namespace.length === 0 || ref.id.length === 0
      ) {
        throw new EffectPlanError("EP_INVALID_RESOURCE", `${where} resource shape`);
      }
      if (utf8Length(ref.namespace) > limits.maxNamespaceBytes) {
        throw new EffectPlanError("EP_LIMIT_EXCEEDED", `${where} namespace bytes`);
      }
      if (utf8Length(ref.id) > limits.maxResourceIdBytes) {
        throw new EffectPlanError("EP_LIMIT_EXCEEDED", `${where} resource id bytes`);
      }
      const accessMode = access.mode;
      if (accessMode !== "read" && accessMode !== "write") {
        throw new EffectPlanError("EP_INVALID_EFFECT", `${where} access mode`);
      }
      const rid = intern.id(ref.namespace, ref.id);
      if (accessMode === "write" || !effective.has(rid)) {
        effective.set(rid, accessMode === "write" ? ACCESS_WRITE : 0);
      }
    }
  }

  const rids = [...effective.keys()];
  rids.sort((a, b) => a - b);
  return {
    id,
    mode: MODE_CODE[mode],
    rids,
    accessModes: rids.map((rid) => (effective.get(rid) === ACCESS_WRITE ? ACCESS_WRITE : 0)),
  };
}

/**
 * Writes a normalised sequence into the engine's input region and returns how
 * many accesses it occupies. Ascending resource order within a call and
 * contiguous CSR offsets are the framing `validate_offsets` expects.
 */
export function writePacked(
  input: PackedInput,
  limits: EngineLimits,
  calls: readonly NormalizedCall[],
): number {
  const { callModes, offsets, resourceIds, accessModes } = input;
  let cursor = 0;
  offsets[0] = 0;
  for (let i = 0; i < calls.length; i++) {
    const call = calls[i]!;
    for (let a = 0; a < call.rids.length; a++) {
      if (cursor >= limits.maxAccesses) {
        throw new EffectPlanError(
          "EP_LIMIT_EXCEEDED",
          `${cursor} accesses exceeds ${limits.maxAccesses}`,
        );
      }
      resourceIds[cursor] = call.rids[a]!;
      accessModes[cursor] = call.accessModes[a]!;
      cursor++;
    }
    callModes[i] = call.mode;
    offsets[i + 1] = cursor;
  }
  return cursor;
}

/**
 * Validates and packs one batch. The interning table is local, so resource
 * numbering depends only on the batch content and the order within it.
 */
export function encodeBatch(
  input: PackedInput,
  limits: EngineLimits,
  calls: readonly ToolCall[],
  options: PlanOptions,
): EncodedBatch {
  if (!Array.isArray(calls)) {
    throw new EffectPlanError("EP_INVALID_INPUT", "batch must be an array");
  }
  const n = calls.length;
  if (n > limits.maxCalls) {
    throw new EffectPlanError("EP_LIMIT_EXCEEDED", `${n} calls exceeds ${limits.maxCalls}`);
  }
  const policy = options.unknownEffectPolicy ?? "serialize";
  const intern = createInterner(limits);
  const seen = new Set<string>();
  const normalized: NormalizedCall[] = [];
  let unknownEffectCount = 0;
  let exclusiveCount = 0;

  for (let i = 0; i < n; i++) {
    const entry = normalizeCall(calls[i]!, i, limits, policy, intern);
    if (seen.has(entry.id)) throw new EffectPlanError("EP_DUPLICATE_CALL_ID", `call index ${i}`);
    seen.add(entry.id);
    if (entry.mode === MODE_CODE.unknown) unknownEffectCount++;
    if (entry.mode === MODE_CODE.exclusive) exclusiveCount++;
    normalized.push(entry);
  }

  const cursor = writePacked(input, limits, normalized);

  return {
    callCount: n,
    accessCount: cursor,
    resourceCount: intern.count,
    unknownEffectCount,
    exclusiveCount,
    pointers: {
      callModes: input.callModes.byteOffset,
      accessOffsets: input.offsets.byteOffset,
      resourceIds: input.resourceIds.byteOffset,
      accessModes: input.accessModes.byteOffset,
    },
  };
}
