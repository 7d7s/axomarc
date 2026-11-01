/**
 * Incremental planning: a session that keeps a plan alive as its workload
 * changes.
 *
 * A session owns the *identity* of a plan — its calls, their stable keys and the
 * resources they touch — and the engine owns the *analysis*. When a batch
 * changes, the session works out the earliest call the change can reach, restores
 * the checkpoint the engine committed at that boundary, replans the suffix and
 * publishes a new immutable snapshot.
 *
 * Three rules shape the code:
 *
 * * **The engine decides.** The session never computes a dependency, a level or
 *   a duration. It moves bytes, picks a boundary and reports what it did.
 * * **A wrong answer is worse than a slow one.** Every restored state is
 *   checked by the engine against the input prefix that is supposed to have
 *   produced it. If the check fails, the session falls back to planning the
 *   whole batch and says so in the diagnostics.
 * * **A failed update changes nothing.** The candidate is built and planned
 *   before anything is published, so a refused transaction leaves the previous
 *   snapshot exactly as it was.
 */

import {
  createInterner,
  normalizeCall,
  writePacked,
  type Interner,
  type NormalizedCall,
  type PackedInput,
  type ToolCall,
  type UnknownEffectPolicy,
} from "./encode.js";
import { EffectPlanError } from "./errors.js";
import type { LoadedEngine } from "./loader.js";
import type { EncodedBatch } from "./encode.js";
import type { EngineInfo } from "./loader.js";
import type { ExecutionPlan } from "./planner.js";

/** A call with the stable identity the session keeps for it. */
export interface IncrementalCall {
  /** Unique within the session, and never changed by a replacement. */
  readonly key: string;
  readonly call: ToolCall;
  /** Ticks, required when the session plans with timing. */
  readonly duration?: number | undefined;
}

/** One edit to a plan. */
export type PlanChange =
  | { readonly type: "append"; readonly key: string; readonly call: ToolCall; readonly duration?: number | undefined }
  | { readonly type: "replace"; readonly key: string; readonly call: ToolCall; readonly duration?: number | undefined }
  | { readonly type: "remove"; readonly key: string };

export interface ApplyChangesOptions {
  /** The version the caller believes it is editing. A mismatch is refused. */
  readonly expectedVersion: number;
  readonly changes: readonly PlanChange[];
}

/** What the session did, in the caller's terms. */
export interface UpdateDiagnostics {
  /**
   * `append` — only new calls were added, and the plan continued from the
   * previous end. `suffix-rebuild` — a prefix was reused and the rest replanned.
   * `full-rebuild` — nothing could be reused. `no-op` — nothing changed.
   */
  readonly strategy: "append" | "suffix-rebuild" | "full-rebuild" | "no-op";
  /** Calls whose dependencies were not recomputed. */
  readonly reusedPrefixCalls: number;
  /** Calls that were replanned. */
  readonly recomputedCalls: number;
  /** Why the strategy was chosen, when there is something to say. */
  readonly reason?: string | undefined;
}

/** An immutable result of one committed version. */
export interface PlanSnapshot {
  readonly version: number;
  /** The stable keys, in batch order. */
  readonly keys: readonly string[];
  readonly callCount: number;
  readonly edgeCount: number;
  /** The plan itself: the graph, the waves and, when asked for, the schedule. */
  readonly plan: ExecutionPlan;
  readonly update: UpdateDiagnostics;
}

export interface SessionOptions {
  /** Plan with timing. Every call then needs a duration. */
  readonly timing?: { readonly unit?: string };
  readonly unknownEffectPolicy?: UnknownEffectPolicy;
  /**
   * How many calls may share one checkpoint. The session stores a checkpoint
   * every this many calls, so a change late in the batch finds a nearby one.
   */
  readonly checkpointInterval?: number;
}

/** The engine's view of the world, injected so this module never imports the planner. */
export interface SessionDependencies {
  readonly engine: LoadedEngine;
  readonly input: PackedInput;
  readonly decode: (
    view: DataView,
    info: EngineInfo,
    encoded: EncodedBatch,
    timing?: { readonly durations: readonly number[]; readonly unit: string },
  ) => ExecutionPlan;
}

export interface PlanSession {
  /** The last committed snapshot. */
  current(): PlanSnapshot;
  applyChanges(options: ApplyChangesOptions): PlanSnapshot;
  /** Start over with a different batch. The version keeps counting up. */
  reset(calls: readonly IncrementalCall[]): PlanSnapshot;
  dispose(): void;
}

interface Entry {
  readonly key: string;
  readonly normalized: NormalizedCall;
  readonly duration: number | undefined;
}

/** The state the engine committed at a call boundary, kept by the host. */
interface Stored {
  readonly boundary: number;
  readonly workspace: Uint8Array;
  readonly records: Uint8Array;
  readonly record: Uint8Array;
}

const DEFAULT_INTERVAL = 64;
/** The record's flags word: the run does not want the scheduling pass. */
const FLAG_WITHOUT_TIMING = 1;
/** Offsets inside the checkpoint record. */
const CP_REVISION = 0, CP_CALLS = 4, CP_EVENTS = 8, CP_EDGES = 12, CP_DIGEST = 16, CP_FLAGS = 20;

class Session implements PlanSession {
  #deps: SessionDependencies;
  #entries: Entry[] = [];
  #intern: Interner;
  #checkpoints = new Map<number, Stored>();
  #version = 0;
  #snapshot: PlanSnapshot | undefined;
  #disposed = false;
  #timing: boolean;
  #unit: string;
  #policy: UnknownEffectPolicy;
  #interval: number;

  constructor(deps: SessionDependencies, calls: readonly IncrementalCall[], options: SessionOptions) {
    this.#deps = deps;
    this.#intern = createInterner(deps.engine.info.limits);
    this.#timing = options.timing !== undefined;
    this.#unit = options.timing?.unit ?? "ticks";
    this.#policy = options.unknownEffectPolicy ?? "serialize";
    this.#interval = Math.max(1, options.checkpointInterval ?? DEFAULT_INTERVAL);
    this.#entries = this.#normalize(calls);
    this.#publish(
      this.#plan(this.#entries, 0, "full-rebuild", "the first plan of a session has no prefix to reuse"),
    );
  }

  current(): PlanSnapshot {
    this.#alive();
    return this.#snapshot!;
  }

  applyChanges(options: ApplyChangesOptions): PlanSnapshot {
    this.#alive();
    if (!options || typeof options !== "object") {
      throw new EffectPlanError("EP_INVALID_CHANGESET", "applyChanges needs an options object");
    }
    if (options.expectedVersion !== this.#version) {
      throw new EffectPlanError(
        "EP_STALE_VERSION",
        `this session is at version ${this.#version}`,
      );
    }
    const changes = options.changes;
    if (!Array.isArray(changes)) {
      throw new EffectPlanError("EP_INVALID_CHANGESET", "changes must be an array");
    }
    if (changes.length === 0) {
      return Object.freeze({
        ...this.#snapshot!,
        update: Object.freeze({
          strategy: "no-op" as const,
          reusedPrefixCalls: this.#entries.length,
          recomputedCalls: 0,
          reason: "the transaction carried no changes",
        }),
      });
    }

    const candidate = this.#buildCandidate(changes);
    const earliest = this.#earliestTouched(changes);
    const boundary = this.#boundaryFor(earliest);
    const strategy = this.#strategyFor(changes, boundary);
    const snapshot = this.#plan(candidate, boundary, strategy);
    // Nothing above touched the published state: only now does the session
    // become the new plan.
    this.#entries = candidate;
    this.#publish(snapshot);
    return snapshot;
  }

  reset(calls: readonly IncrementalCall[]): PlanSnapshot {
    this.#alive();
    if (!Array.isArray(calls)) {
      throw new EffectPlanError("EP_INVALID_CHANGESET", "reset needs an array of calls");
    }
    // A new generation: the interning table and every checkpoint belong to the
    // old batch and cannot be carried over.
    this.#intern = createInterner(this.#deps.engine.info.limits);
    this.#checkpoints.clear();
    this.#entries = this.#normalize(calls);
    const snapshot = this.#plan(this.#entries, 0, "full-rebuild", "a reset starts a new generation");
    this.#publish(snapshot);
    return snapshot;
  }

  dispose(): void {
    this.#entries = [];
    this.#checkpoints.clear();
    this.#snapshot = undefined;
    this.#disposed = true;
  }

  #alive(): void {
    if (this.#disposed) {
      throw new EffectPlanError("EP_SESSION_DISPOSED", "this session has been disposed");
    }
  }

  /** Validates a list of calls and normalises them against the session's table. */
  #normalize(calls: readonly IncrementalCall[]): Entry[] {
    const limits = this.#deps.engine.info.limits;
    const seen = new Set<string>();
    const entries: Entry[] = [];
    for (let index = 0; index < calls.length; index++) {
      const entry = calls[index]!;
      if (typeof entry !== "object" || entry === null) {
        throw new EffectPlanError("EP_INVALID_CHANGESET", `call ${index} must be an object`);
      }
      const key = entry.key;
      if (typeof key !== "string" || key.length === 0) {
        throw new EffectPlanError("EP_INVALID_CHANGESET", `call ${index} needs a non-empty key`);
      }
      if (seen.has(key)) {
        throw new EffectPlanError("EP_DUPLICATE_KEY", `key at position ${index} is already used`);
      }
      seen.add(key);
      const normalized = normalizeCall(entry.call, index, limits, this.#policy, this.#intern);
      entries.push({ key, normalized, duration: this.#duration(entry.duration, index) });
    }
    if (entries.length > limits.maxCalls) {
      throw new EffectPlanError(
        "EP_LIMIT_EXCEEDED",
        `${entries.length} calls exceeds ${limits.maxCalls}`,
      );
    }
    return entries;
  }

  #duration(value: number | undefined, index: number): number | undefined {
    if (!this.#timing) {
      if (value !== undefined) {
        throw new EffectPlanError(
          "EP_INVALID_INPUT",
          `call ${index} carries a duration but the session plans without timing`,
        );
      }
      return undefined;
    }
    if (typeof value !== "number" || !Number.isSafeInteger(value) || value <= 0) {
      throw new EffectPlanError(
        "EP_INVALID_INPUT",
        `call ${index} needs a positive integer duration`,
      );
    }
    return value;
  }

  /**
   * Applies the changes to a copy of the entries. The session's own list is
   * untouched until the candidate has been planned successfully.
   */
  #buildCandidate(changes: readonly PlanChange[]): Entry[] {
    const candidate = [...this.#entries];
    const indexOf = new Map(this.#entries.map((entry, index) => [entry.key, index]));
    const touched = new Set<string>();
    for (const change of changes) {
      if (typeof change !== "object" || change === null) {
        throw new EffectPlanError("EP_INVALID_CHANGESET", "a change must be an object");
      }
      const key = change.key;
      if (typeof key !== "string" || key.length === 0) {
        throw new EffectPlanError("EP_INVALID_CHANGESET", "a change needs a non-empty key");
      }
      if (touched.has(key)) {
        throw new EffectPlanError("EP_DUPLICATE_KEY", "two changes name the same key");
      }
      touched.add(key);
      if (change.type === "append") {
        if (indexOf.has(key)) {
          throw new EffectPlanError("EP_DUPLICATE_KEY", "an appended key already exists");
        }
        const normalized = normalizeCall(
          change.call,
          candidate.length,
          this.#deps.engine.info.limits,
          this.#policy,
          this.#intern,
        );
        candidate.push({
          key,
          normalized,
          duration: this.#duration(change.duration, candidate.length),
        });
        indexOf.set(key, candidate.length - 1);
        continue;
      }
      const at = indexOf.get(key);
      if (at === undefined) {
        throw new EffectPlanError("EP_UNKNOWN_KEY", "the change names a key this session does not have");
      }
      if (change.type === "replace") {
        const normalized = normalizeCall(
          change.call,
          at,
          this.#deps.engine.info.limits,
          this.#policy,
          this.#intern,
        );
        candidate[at] = {
          key,
          normalized,
          duration: this.#duration(change.duration, at),
        };
        continue;
      }
      if (change.type === "remove") {
        candidate.splice(at, 1);
        indexOf.clear();
        candidate.forEach((entry, index) => indexOf.set(entry.key, index));
        continue;
      }
      throw new EffectPlanError("EP_INVALID_CHANGESET", "a change has an unknown type");
    }
    return candidate;
  }

  /** The earliest call a change can reach: everything from there is replanned. */
  #earliestTouched(changes: readonly PlanChange[]): number {
    const indexOf = new Map(this.#entries.map((entry, index) => [entry.key, index]));
    let earliest = this.#entries.length;
    for (const change of changes) {
      if (change.type === "append") {
        earliest = Math.min(earliest, this.#entries.length);
        continue;
      }
      const at = indexOf.get(change.key);
      if (at !== undefined) earliest = Math.min(earliest, at);
    }
    return earliest;
  }

  /** The newest checkpoint that still describes the unchanged prefix. */
  #boundaryFor(earliest: number): number {
    let best = 0;
    for (const boundary of this.#checkpoints.keys()) {
      if (boundary <= earliest && boundary > best) best = boundary;
    }
    return best;
  }

  #strategyFor(changes: readonly PlanChange[], boundary: number): UpdateDiagnostics["strategy"] {
    const onlyAppends = changes.every((change) => change.type === "append");
    if (onlyAppends && boundary === this.#entries.length) return "append";
    return boundary === 0 ? "full-rebuild" : "suffix-rebuild";
  }

  /**
   * Plans the batch, reusing the prefix at `boundary` and capturing a
   * checkpoint every `interval` calls on the way.
   *
   * The segments are not extra work: each one resumes from the previous
   * segment's checkpoint, so the whole batch is planned once, and the
   * checkpoints fall out of it.
   */
  #plan(
    entries: readonly Entry[],
    boundary: number,
    strategy: UpdateDiagnostics["strategy"],
    reason?: string,
  ): PlanSnapshot {
    const { engine, input } = this.#deps;
    const info = engine.info;
    const limits = info.limits;

    // The packed arrays always describe the whole candidate batch; a segment
    // only tells the engine how much of it to look at.
    const accessCount = writePacked(input, limits, entries.map((entry) => entry.normalized));
    const durations = entries.map((entry) => entry.duration ?? 0);
    if (this.#timing) {
      for (let i = 0; i < entries.length; i++) {
        input.durations[i] = durations[i]!;
      }
    }

    const flags = this.#timing ? 0 : FLAG_WITHOUT_TIMING;
    const total = entries.length;
    let at = boundary;
    let carry = this.#checkpoints.get(boundary);
    const captured = new Map<number, Stored>();
    let status = 0;
    let reused = boundary;
    let fallback = "";
    let fellBack = false;
    // Nothing to reuse means there is nothing to resume: the batch is planned in
    // one pass, with no checkpoint copied in and no intermediate checkpoint
    // taken. The first plan of a session is the exception - it segments, so the
    // session ends up holding checkpoints along the batch for later updates to
    // reuse. A fallback update does not segment: it would rebuild the same
    // checkpoints it already has.
    const segmented = boundary > 0 || this.#checkpoints.size === 0;

    for (;;) {
      // After a fallback the whole batch is planned in one go: there is no
      // point trying to resume again, and a second failure must end the loop
      // rather than repeat it.
      const target =
        !segmented || fellBack ? total : Math.min(at + this.#interval, total);
      const segmentAccesses = input.offsets[target] ?? accessCount;
      this.#restore(carry, flags);
      status = engine.planV4(
        info.abiVersion,
        target,
        segmentAccesses,
        this.#intern.count,
        input.callModes.byteOffset,
        input.offsets.byteOffset,
        input.resourceIds.byteOffset,
        input.accessModes.byteOffset,
        info.layout.workspace.ptr,
        info.layout.workspace.bytes,
        info.layout.output.ptr,
        info.layout.output.bytes,
        info.layout.levels.ptr,
        info.layout.levels.bytes,
        info.layout.durations.ptr,
        info.layout.durations.bytes,
        info.layout.timing.ptr,
        info.layout.timing.bytes,
        info.layout.checkpoint.ptr,
        info.layout.checkpoint.bytes,
      );
      if (status === 131 || status === 132) {
        // The restored state does not belong to this prefix. Fall back to
        // planning from nothing rather than trusting it - once.
        if (fellBack || carry === undefined || carry.boundary === 0) break;
        fellBack = true;
        fallback = "a restored checkpoint did not match the prefix, so the batch was planned whole";
        this.#checkpoints.delete(carry.boundary);
        carry = undefined;
        at = 0;
        reused = 0;
        continue;
      }
      if (status !== 0) break;
      const stored = this.#capture(target);
      captured.set(target, stored);
      carry = stored;
      at = target;
      if (at >= total) break;
    }

    if (status !== 0) {
      throw new EffectPlanError(
        status === 118 || status === 121 ? "EP_INVALID_INPUT" : "EP_ENGINE_ERROR",
        `the engine refused the batch with status ${status}`,
      );
    }

    const encoded: EncodedBatch = {
      callCount: total,
      accessCount,
      resourceCount: this.#intern.count,
      unknownEffectCount: entries.filter((entry) => entry.normalized.mode === 3).length,
      exclusiveCount: entries.filter((entry) => entry.normalized.mode === 2).length,
      pointers: {
        callModes: input.callModes.byteOffset,
        accessOffsets: input.offsets.byteOffset,
        resourceIds: input.resourceIds.byteOffset,
        accessModes: input.accessModes.byteOffset,
      },
    };
    const plan = this.#deps.decode(
      input.view,
      info,
      encoded,
      this.#timing ? { durations, unit: this.#unit } : undefined,
    );

    // The checkpoints this run produced are the ones worth keeping.
    this.#checkpoints.clear();
    for (const [atBoundary, stored] of captured) this.#checkpoints.set(atBoundary, stored);

    return Object.freeze({
      version: this.#version + 1,
      keys: Object.freeze(entries.map((entry) => entry.key)),
      callCount: total,
      edgeCount: plan.edgeCount,
      plan,
      update: Object.freeze({
        strategy: fallback ? "full-rebuild" : strategy,
        reusedPrefixCalls: fallback ? 0 : reused,
        recomputedCalls: fallback ? total : total - reused,
        reason: fallback || reason,
      }),
    });
  }

  /** Copies the state the engine committed at a boundary into the host. */
  #capture(boundary: number): Stored {
    const { engine, input } = this.#deps;
    const info = engine.info;
    const bytes = new Uint8Array(engine.memory.buffer);
    const emitted = new DataView(engine.memory.buffer).getUint32(info.layout.output.ptr + 8, true);
    return {
      boundary,
      workspace: bytes.slice(info.layout.workspace.ptr, info.layout.workspace.ptr + info.layout.workspace.bytes),
      records: bytes.slice(
        info.layout.output.ptr + 16,
        info.layout.output.ptr + 16 + emitted * 16,
      ),
      record: bytes.slice(info.layout.checkpoint.ptr, info.layout.checkpoint.ptr + info.layout.checkpoint.bytes),
    };
  }

  /** Puts a stored state back into the engine's regions, ready to be resumed. */
  #restore(stored: Stored | undefined, flags: number): void {
    const { engine } = this.#deps;
    const info = engine.info;
    const bytes = new Uint8Array(engine.memory.buffer);
    const record = info.layout.checkpoint;
    if (stored === undefined) {
      bytes.fill(0, record.ptr, record.ptr + record.bytes);
    } else {
      bytes.set(stored.workspace, info.layout.workspace.ptr);
      bytes.set(stored.records, info.layout.output.ptr + 16);
      bytes.set(stored.record, record.ptr);
    }
    // The flags word says what this run should do, and it survives the engine's
    // own clearing of the record.
    new DataView(engine.memory.buffer).setUint32(record.ptr + CP_FLAGS, flags, true);
  }

  #publish(snapshot: PlanSnapshot): void {
    this.#version = snapshot.version;
    this.#snapshot = Object.freeze({
      ...snapshot,
      version: this.#version,
    });
  }
}

/** Creates a session over a batch of calls with stable keys. */
export function createSession(
  deps: SessionDependencies,
  calls: readonly IncrementalCall[],
  options: SessionOptions = {},
): PlanSession {
  return new Session(deps, calls, options);
}
