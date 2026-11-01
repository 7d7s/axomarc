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
  const seen = new Set<string>();
  const interned = new Map<string, number>();
  const { callModes, offsets, resourceIds, accessModes } = input;

  let cursor = 0;
  let unknownCount = 0;
  let exclusiveCount = 0;
  offsets[0] = 0;

  for (let i = 0; i < n; i++) {
    const call = calls[i];
    if (typeof call !== "object" || call === null) {
      throw new EffectPlanError("EP_INVALID_INPUT", `call ${i} must be an object`);
    }
    const id = call.id;
    if (typeof id !== "string" || id.length === 0) {
      throw new EffectPlanError("EP_INVALID_INPUT", `call ${i} needs a non-empty string id`);
    }
    if (seen.has(id)) throw new EffectPlanError("EP_DUPLICATE_CALL_ID", `call index ${i}`);
    seen.add(id);

    const declared = call.effect;
    let mode: EffectMode = "unknown";
    let accesses: readonly ResourceAccess[] | undefined;
    if (declared !== undefined && declared !== null) {
      if (typeof declared !== "object") {
        throw new EffectPlanError("EP_INVALID_EFFECT", `call ${i} effect must be an object`);
      }
      if (declared.version !== 1) {
        throw new EffectPlanError("EP_INVALID_EFFECT", `call ${i} effect version`);
      }
      if (typeof declared.mode !== "string" || !(declared.mode in MODE_CODE)) {
        throw new EffectPlanError("EP_INVALID_EFFECT", `call ${i} effect mode`);
      }
      mode = declared.mode;
      accesses = declared.accesses;
    }
    if (accesses !== undefined && !Array.isArray(accesses)) {
      throw new EffectPlanError("EP_INVALID_EFFECT", `call ${i} accesses must be an array`);
    }
    if (mode === "pure" && accesses !== undefined && accesses.length > 0) {
      throw new EffectPlanError("EP_INVALID_EFFECT", `call ${i} is pure but declares accesses`);
    }
    if (mode === "resources" && (accesses === undefined || accesses.length === 0)) {
      throw new EffectPlanError("EP_INVALID_EFFECT", `call ${i} requires at least one access`);
    }
    if (mode === "unknown") {
      unknownCount++;
      if (policy === "reject") throw new EffectPlanError("EP_UNKNOWN_EFFECT_REJECTED", `call ${i}`);
    } else if (mode === "exclusive") {
      exclusiveCount++;
    }

    // Intern and normalise: one effective access per resource, write wins.
    const effective = new Map<number, number>();
    if (accesses !== undefined) {
      for (const access of accesses) {
        if (typeof access !== "object" || access === null) {
          throw new EffectPlanError("EP_INVALID_RESOURCE", `call ${i} access shape`);
        }
        const ref = access.resource;
        if (
          typeof ref !== "object" || ref === null ||
          typeof ref.namespace !== "string" || typeof ref.id !== "string" ||
          ref.namespace.length === 0 || ref.id.length === 0
        ) {
          throw new EffectPlanError("EP_INVALID_RESOURCE", `call ${i} resource shape`);
        }
        if (utf8Length(ref.namespace) > limits.maxNamespaceBytes) {
          throw new EffectPlanError("EP_LIMIT_EXCEEDED", `call ${i} namespace bytes`);
        }
        if (utf8Length(ref.id) > limits.maxResourceIdBytes) {
          throw new EffectPlanError("EP_LIMIT_EXCEEDED", `call ${i} resource id bytes`);
        }
        const accessMode = access.mode;
        if (accessMode !== "read" && accessMode !== "write") {
          throw new EffectPlanError("EP_INVALID_EFFECT", `call ${i} access mode`);
        }
        // Length prefixes keep "a:bc" and "ab:c" from colliding.
        const key = `${ref.namespace.length}:${ref.namespace}${ref.id.length}:${ref.id}`;
        let rid = interned.get(key);
        if (rid === undefined) {
          rid = interned.size + 1;
          if (rid > limits.maxResources) {
            throw new EffectPlanError("EP_LIMIT_EXCEEDED", `${rid} resources exceeds ${limits.maxResources}`);
          }
          interned.set(key, rid);
        }
        if (accessMode === "write" || !effective.has(rid)) {
          effective.set(rid, accessMode === "write" ? ACCESS_WRITE : 0);
        }
      }
    }

    const rids = [...effective.keys()];
    rids.sort((a, b) => a - b);
    for (const rid of rids) {
      if (cursor >= limits.maxAccesses) {
        throw new EffectPlanError("EP_LIMIT_EXCEEDED", `${cursor} accesses exceeds ${limits.maxAccesses}`);
      }
      resourceIds[cursor] = rid;
      accessModes[cursor] = effective.get(rid) === ACCESS_WRITE ? ACCESS_WRITE : 0;
      cursor++;
    }
    callModes[i] = MODE_CODE[mode];
    offsets[i + 1] = cursor;
  }

  return {
    callCount: n,
    accessCount: cursor,
    resourceCount: interned.size + 1,
    unknownEffectCount: unknownCount,
    exclusiveCount,
    pointers: {
      callModes: callModes.byteOffset,
      accessOffsets: offsets.byteOffset,
      resourceIds: resourceIds.byteOffset,
      accessModes: accessModes.byteOffset,
    },
  };
}
