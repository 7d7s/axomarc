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
import { internCapacities, placeResources } from "./resources.js";
import type {
  CapacityDemand,
  CapacityResource,
  CapacityTable,
  ResourceResult,
  ResourceViews,
} from "./resources.js";
import type { LoadedEngine } from "./loader.js";
import type { EncodedBatch } from "./encode.js";
import type { EngineInfo } from "./loader.js";
import type { ExecutionPlan } from "./planner.js";

/**
 * A caller key never reaches the engine, so this bound is the host's own. See
 * `docs/PLAN-VERSIONING.md` §3 for why it is counted in bytes.
 */
export const MAX_KEY_BYTES = 128;

/** Bytes a string takes in UTF-8, without allocating it. */
function utf8Bytes(text: string): number {
  let bytes = 0;
  for (let index = 0; index < text.length; index++) {
    const unit = text.charCodeAt(index);
    if (unit < 0x80) bytes += 1;
    else if (unit < 0x800) bytes += 2;
    else if (unit >= 0xd800 && unit < 0xdc00) {
      bytes += 4;
      index++; // a well-formed pair is one 4-byte scalar
    } else bytes += 3; // three-byte scalar, or a lone surrogate as U+FFFD
  }
  return bytes;
}

/** A call with the stable identity the session keeps for it. */
export interface IncrementalCall {
  /** Unique within the session, unchanged by a replacement, at most MAX_KEY_BYTES UTF-8 bytes. */
  readonly key: string;
  readonly call: ToolCall;
  /** Ticks, required when the session plans with timing. */
  readonly duration?: number | undefined;
  /**
   * What this call holds of the session's capacity resources, named by the
   * caller's own key. A demand never reaches the engine as a key: the host
   * interns the session's capacity table and passes an index.
   */
  readonly demands?: readonly CapacityDemand[] | undefined;
}

/** One edit to a plan. */
export type PlanChange =
  | {
      readonly type: "append";
      readonly key: string;
      readonly call: ToolCall;
      readonly duration?: number | undefined;
      readonly demands?: readonly CapacityDemand[] | undefined;
    }
  | {
      readonly type: "replace";
      readonly key: string;
      readonly call: ToolCall;
      readonly duration?: number | undefined;
      readonly demands?: readonly CapacityDemand[] | undefined;
    }
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
  /**
   * Which of the two invalidation domains the transaction touched.
   *
   * `dependency` means the graph itself could have changed: an access
   * declaration was rewritten, or a call was removed. `scheduling` means only
   * the clock and the demands moved, so the dependency graph was reused whole
   * and nothing about the conflict rules had to be recomputed. `none` means the
   * transaction added calls without touching anything that was already there.
   *
   * The session's word is an optimisation, not a guarantee: the engine checks
   * every restored state against the input prefix that is supposed to have
   * produced it, and refuses the restore if the two disagree.
   */
  readonly invalidated: "dependency" | "scheduling" | "none";
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
  /**
   * The placement against the session's capacity resources, or `undefined`
   * when the session declares none. A session that declares resources places
   * every batch it plans, so the snapshot carries the placement and the
   * certificate that goes with it.
   */
  readonly placement: ResourceResult | undefined;
  /** The version of the session's capacity-resource table. */
  readonly resourceVersion: number;
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
  /**
   * The capacity resources this session places against. The table belongs to
   * the session, not to a batch: it is declared once and holds until the
   * session is reset with a new one, because a capacity that changes under a
   * running plan is a new generation, not an edit.
   */
  readonly resources?: { readonly capacities: readonly CapacityResource[] } | undefined;
}

/** The engine's view of the world, injected so this module never imports the planner. */
export interface SessionDependencies {
  readonly engine: LoadedEngine;
  readonly input: PackedInput;
  /** The capacity-resource regions, mapped like the input regions. */
  readonly resources: ResourceViews;
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
  reset(calls: readonly IncrementalCall[], options?: SessionOptions): PlanSnapshot;
  dispose(): void;
}

interface Entry {
  readonly key: string;
  readonly normalized: NormalizedCall;
  readonly duration: number | undefined;
  /** What this call holds of the session's capacity resources. */
  readonly demands: readonly CapacityDemand[];
}

/**
 * Whether two calls declare the same dependencies, in the engine's terms.
 *
 * The comparison is over the normalised form — the effect mode and the pairs
 * of interned resource id and access mode — and not over the caller's text.
 * Two calls that name the same resource differently are the same dependency,
 * and two calls that name different resources are not, whatever the strings
 * look like.
 */
function sameDependencies(a: NormalizedCall, b: NormalizedCall): boolean {
  if (a.mode !== b.mode) return false;
  if (a.rids.length !== b.rids.length) return false;
  for (let i = 0; i < a.rids.length; i++) {
    if (a.rids[i] !== b.rids[i]) return false;
    if (a.accessModes[i] !== b.accessModes[i]) return false;
  }
  return true;
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
  /** The session's capacity-resource table. Empty when it declares none. */
  #table: CapacityTable;
  /** How many times a capacity table has been installed. */
  #resourceVersion = 0;

  constructor(deps: SessionDependencies, calls: readonly IncrementalCall[], options: SessionOptions) {
    this.#deps = deps;
    this.#intern = createInterner(deps.engine.info.limits);
    this.#timing = options.timing !== undefined;
    this.#unit = options.timing?.unit ?? "ticks";
    this.#policy = options.unknownEffectPolicy ?? "serialize";
    this.#interval = Math.max(1, options.checkpointInterval ?? DEFAULT_INTERVAL);
    this.#table = this.#installTable(options.resources?.capacities);
    this.#entries = this.#normalize(calls);
    this.#publish(
      this.#plan(this.#entries, 0, "full-rebuild", "the first plan of a session has no prefix to reuse"),
    );
  }

  /**
   * Interns a capacity table for the session.
   *
   * The table belongs to the session rather than to a batch: it is declared
   * once, and a different set of capacities is a new generation rather than an
   * edit, because a capacity that changes under a running plan invalidates
   * every placement the session has published.
   */
  #installTable(capacities: readonly CapacityResource[] | undefined): CapacityTable {
    this.#resourceVersion += 1;
    return internCapacities(capacities ?? [], this.#deps.engine.info.limits);
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
          invalidated: "none" as const,
        }),
      });
    }

    const { entries: candidate, invalidated } = this.#buildCandidate(changes);
    const earliest = this.#earliestTouched(changes);
    const boundary = this.#boundaryFor(earliest, invalidated);
    const strategy = this.#strategyFor(changes, boundary);
    const snapshot = this.#plan(candidate, boundary, strategy, undefined, invalidated);
    // Nothing above touched the published state: only now does the session
    // become the new plan.
    this.#entries = candidate;
    this.#publish(snapshot);
    return snapshot;
  }

  reset(calls: readonly IncrementalCall[], options: SessionOptions = {}): PlanSnapshot {
    this.#alive();
    if (!Array.isArray(calls)) {
      throw new EffectPlanError("EP_INVALID_CHANGESET", "reset needs an array of calls");
    }
    // A new generation: the interning table and every checkpoint belong to the
    // old batch and cannot be carried over. So does the capacity table, if the
    // caller is installing a different one: a capacity that changes under a
    // running plan invalidates every placement the session published.
    this.#intern = createInterner(this.#deps.engine.info.limits);
    this.#checkpoints.clear();
    if (options.resources !== undefined) {
      this.#table = this.#installTable(options.resources.capacities);
    }
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
      if (utf8Bytes(key) > MAX_KEY_BYTES) {
        throw new EffectPlanError(
          "EP_INVALID_CHANGESET",
          `call ${index} has a key over ${MAX_KEY_BYTES} UTF-8 bytes`,
        );
      }
      if (seen.has(key)) {
        throw new EffectPlanError("EP_DUPLICATE_KEY", `key at position ${index} is already used`);
      }
      seen.add(key);
      const normalized = normalizeCall(entry.call, index, limits, this.#policy, this.#intern);
      entries.push({
        key,
        normalized,
        duration: this.#duration(entry.duration, index),
        demands: this.#demands(entry.demands, index),
      });
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
   * Validates a call's demands against the session's capacity table.
   *
   * The table is the session's, so a demand that names a resource nobody
   * declared is refused here rather than by the engine: the error would
   * otherwise be about an index the caller never chose.
   */
  #demands(value: readonly CapacityDemand[] | undefined, index: number): readonly CapacityDemand[] {
    if (value === undefined) return [];
    if (!Array.isArray(value)) {
      throw new EffectPlanError("EP_INVALID_CHANGESET", `call ${index} demands must be an array`);
    }
    for (const demand of value) {
      if (typeof demand !== "object" || demand === null) {
        throw new EffectPlanError("EP_INVALID_CHANGESET", `call ${index} demand must be an object`);
      }
      if (typeof demand.resource !== "string" || demand.resource.length === 0) {
        throw new EffectPlanError("EP_INVALID_CHANGESET", `call ${index} demand needs a resource key`);
      }
      if (!this.#table.index.has(demand.resource)) {
        throw new EffectPlanError(
          "EP_INVALID_CHANGESET",
          `call ${index} demands a resource this session did not declare`,
        );
      }
      if (typeof demand.amount !== "number" || !Number.isInteger(demand.amount) || demand.amount <= 0) {
        throw new EffectPlanError("EP_INVALID_CHANGESET", `call ${index} demand amount must be positive`);
      }
    }
    return value;
  }

  /**
   * Applies the changes to a copy of the entries. The session's own list is
   * untouched until the candidate has been planned successfully.
   *
   * The transaction is classified as it is applied, because the classification
   * decides how much of the previous plan can be reused: a change that leaves
   * the dependencies alone leaves the graph alone too.
   */
  #buildCandidate(changes: readonly PlanChange[]): { readonly entries: Entry[]; readonly invalidated: UpdateDiagnostics["invalidated"] } {
    const candidate = [...this.#entries];
    let invalidated: UpdateDiagnostics["invalidated"] = "none";
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
      if (utf8Bytes(key) > MAX_KEY_BYTES) {
        throw new EffectPlanError(
          "EP_INVALID_CHANGESET",
          `a change has a key over ${MAX_KEY_BYTES} UTF-8 bytes`,
        );
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
          demands: this.#demands(change.demands, candidate.length),
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
        // A replacement that declares the same dependencies changes nothing
        // the conflict rules can see. Only the clock and the demands moved, so
        // the graph is still the graph.
        if (!sameDependencies(candidate[at]!.normalized, normalized)) {
          invalidated = "dependency";
        } else if (invalidated === "none") {
          invalidated = "scheduling";
        }
        candidate[at] = {
          key,
          normalized,
          duration: this.#duration(change.duration, at),
          demands: this.#demands(change.demands, at),
        };
        continue;
      }
      if (change.type === "remove") {
        // A removal renumbers the calls after it, so the edges that were
        // published for them no longer describe them. The graph has to be
        // recomputed from here.
        invalidated = "dependency";
        candidate.splice(at, 1);
        indexOf.clear();
        candidate.forEach((entry, index) => indexOf.set(entry.key, index));
        continue;
      }
      throw new EffectPlanError("EP_INVALID_CHANGESET", "a change has an unknown type");
    }
    return { entries: candidate, invalidated };
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

  /**
   * The boundary the session resumes from.
   *
   * The usual rule is the newest checkpoint that still describes the unchanged
   * prefix. The exception is a transaction that only moved the clock or the
   * demands: the dependencies did not change, so the whole dependency pass
   * is reusable and the checkpoint at the end of the batch still describes it.
   * Resuming there means the engine plans no calls at all — it re-runs the
   * level and timing passes over the graph it already has, which is where the
   * new durations and the new demands take effect.
   *
   * The engine has the last word either way: it checks the restored state
   * against the input prefix, and refuses it if the two disagree.
   */
  #boundaryFor(earliest: number, invalidated: UpdateDiagnostics["invalidated"]): number {
    if (invalidated === "scheduling") {
      const end = this.#entries.length;
      if (this.#checkpoints.has(end)) return end;
    }
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
    invalidated: UpdateDiagnostics["invalidated"] = "none",
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

    // A run that resumed from a boundary left everything at or before it exactly
    // as it found it, so those checkpoints still describe the prefix and are kept
    // — dropping them is what made the *second* update on a session fall back to a
    // rebuild, because nothing was left to resume from. Past the resume point the
    // state was replanned, so this run's captures replace what was there, and after
    // a fallback nothing old can be trusted at all.
    if (!fellBack) {
      for (const [keptBoundary, stored] of this.#checkpoints) {
        if (keptBoundary <= boundary) captured.set(keptBoundary, stored);
      }
    }
    this.#checkpoints.clear();
    for (const [atBoundary, stored] of captured) this.#checkpoints.set(atBoundary, stored);

    // The placement is a whole-batch pass: the capacities and the demands are
    // the session's, and the schedule is computed from the graph and the
    // durations this run published.
    const placement = this.#table.count
      ? placeResources(
          this.#deps.engine,
          this.#deps.resources,
          this.#table,
          entries.map((entry) => entry.demands),
          total,
          entries.map((entry) => entry.normalized.id),
        )
      : undefined;

    return Object.freeze({
      version: this.#version + 1,
      keys: Object.freeze(entries.map((entry) => entry.key)),
      callCount: total,
      edgeCount: plan.edgeCount,
      plan,
      placement,
      resourceVersion: this.#resourceVersion,
      update: Object.freeze({
        strategy: fallback ? "full-rebuild" : strategy,
        reusedPrefixCalls: fallback ? 0 : reused,
        recomputedCalls: fallback ? total : total - reused,
        reason: fallback || reason,
        invalidated: fallback ? "dependency" : invalidated,
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
