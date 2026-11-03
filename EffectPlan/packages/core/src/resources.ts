/**
 * Capacity resources: declarations, demands, and the two engine calls that
 * check and schedule them.
 *
 * Dependency resources and capacity resources stay separate models. A call
 * names both — the dependency resources it touches and the capacity it holds —
 * and this module interns the capacity keys into the engine's own index space,
 * exactly as `encode.ts` interns dependency identities. No caller string
 * crosses the ABI: the engine sees integers, and the wrapper translates the
 * engine's numeric diagnostics back into the caller's own keys.
 *
 * The wrapper validates shape and leaves feasibility to the engine. A demand
 * that asks for more than a resource holds is not a malformed request, it is
 * the answer "this cannot run as asked", and it comes back as an outcome with
 * diagnostics rather than as an exception.
 */

import { EffectPlanError, EXPECTED_QUALITY_ABI } from "./errors.js";
import type { UnknownEffectPolicy } from "./encode.js";
import type { EngineInfo, EngineLayout, EngineLimits, LoadedEngine } from "./loader.js";

/** A capacity resource: the caller's key and the number of units it holds. */
export interface CapacityResource {
  readonly key: string;
  /** Positive integer. The engine does not know what a unit means. */
  readonly capacity: number;
}

/** What one call holds, for the whole of its execution. */
export interface CapacityDemand {
  /** The key of a declared capacity resource. */
  readonly resource: string;
  /** Positive integer: how many units the call holds while it runs. */
  readonly amount: number;
}

export interface ResourceOptions {
  /** The capacity resources this batch runs against. */
  readonly capacities: readonly CapacityResource[];
  /**
   * One entry per call, in batch order: what that call holds. An absent entry
   * means the call holds nothing. The list must have one entry per call.
   */
  readonly demands?: readonly (readonly CapacityDemand[])[];
  /**
   * Durations, one per call, in batch order. Required to schedule; ignored by
   * a check, which has no clock to place anything on.
   */
  readonly durations?: ArrayLike<number>;
  /** The caller's unit, carried through to the result. Metadata only. */
  readonly unit?: string;
  /** What to do with a call whose effect is unknown. */
  readonly unknownEffectPolicy?: UnknownEffectPolicy;
}

/**
 * Why the engine refused, in the engine's own numbering. A plain record rather
 * than a literal union: the published types cost bytes in every consumer's
 * declaration file, and the numbers are the contract, not the type.
 */
export const RESOURCE_DIAGNOSTIC: Readonly<Record<string, number>> = {
  insufficientCapacity: 1,
  undeclaredResource: 2,
  zeroCapacity: 3,
  zeroDemand: 4,
  duplicateDemand: 5,
  range: 6,
  contention: 7,
  unschedulable: 8,
};

export interface ResourceDiagnostic {
  /** One of `RESOURCE_DIAGNOSTIC`. */
  readonly kind: number;
  /** The capacity resource, by the caller's key. Undefined if the engine named an index it never declared. */
  readonly resource: string | undefined;
  /** The capacity resource index the engine used. */
  readonly resourceIndex: number;
  /** The call, by the caller's id. Undefined if the engine named no call. */
  readonly call: string | undefined;
  /** The call index the engine used. */
  readonly callIndex: number;
  /** The number the engine attached: an amount, a capacity or a count. */
  readonly value: number;
}

/** One call's place in the schedule. */
export interface ResourceWindow {
  readonly callIndex: number;
  readonly callId: string;
  readonly start: number;
  readonly finish: number;
}

/** What one resource did, as the engine measured it. */
export interface ResourceStatistic {
  readonly resourceIndex: number;
  readonly key: string;
  /** The declared capacity. */
  readonly capacity: number;
  /** The highest simultaneous demand observed. Never above the capacity. */
  readonly peak: number;
  /**
   * `amount × duration` summed over the calls that held the resource, in the
   * caller's unit. A `bigint`, because the engine stores it as `u64` and a
   * `number` would lose precision above 2^53 without saying so.
   */
  readonly busyTime: bigint;
}

export interface ResourceResult {
  /** `feasible` when a schedule was produced, `infeasible` when it could not be. */
  readonly outcome: "feasible" | "infeasible";
  /** Every diagnostic the engine recorded, in engine order. */
  readonly diagnostics: readonly ResourceDiagnostic[];
  /** The placement, one row per call, in batch order. Empty when infeasible. */
  readonly schedule: readonly ResourceWindow[];
  /** The length of the schedule in the caller's unit. */
  readonly makespan: number;
  /** How many times a call had to wait for capacity. */
  readonly deferrals: number;
  /** One row per declared resource, in declaration order. */
  readonly statistics: readonly ResourceStatistic[];
  /**
   * The quality certificate for this placement, or `undefined` when nothing was
   * placed — a check asks whether a batch can run and has no schedule to
   * measure. Computed by the engine over the regions the placement published.
   */
  readonly quality: ScheduleQuality | undefined;
}

/** Region header offsets; must match `engine/src/resources.rs`. */
const DIAG = {
  revision: 0,
  count: 4,
  status: 8,
  entries: 32,
  entryBytes: 16,
} as const;

/** Schedule header offsets; must match `engine/src/scheduling.rs`. */
const SCHEDULE = {
  revision: 0,
  placed: 4,
  makespan: 8,
  status: 12,
  deferrals: 16,
  rounds: 20,
  entries: 32,
  entryBytes: 8,
} as const;

/** Statistics header offsets; must match `engine/src/scheduling.rs`. */
const STATS = {
  revision: 0,
  count: 4,
  makespan: 8,
  entries: 32,
  entryBytes: 16,
} as const;

const EXPECTED_RESOURCE_REVISION = 1;

/** The capacity keys, in declaration order, and the index each one got. */
export interface CapacityTable {
  readonly keys: readonly string[];
  readonly index: ReadonlyMap<string, number>;
  /** The declared capacity of each resource, in the same order. */
  readonly capacities: readonly number[];
  readonly count: number;
}

/**
 * Validates the declarations and interns the keys. The order of declaration is
 * the order of the indexes, so the mapping is reproducible from the input
 * alone — the same discipline `encode.ts` applies to dependency resources.
 */
export function internCapacities(
  capacities: readonly CapacityResource[],
  limits: EngineLimits,
): CapacityTable {
  if (!Array.isArray(capacities)) {
    throw new EffectPlanError("EP_INVALID_INPUT", "capacities must be an array");
  }
  if (capacities.length > limits.maxCapacityResources) {
    throw new EffectPlanError(
      "EP_LIMIT_EXCEEDED",
      `${capacities.length} capacity resources exceeds ${limits.maxCapacityResources}`,
    );
  }
  const keys: string[] = [];
  const values: number[] = [];
  const index = new Map<string, number>();
  for (let position = 0; position < capacities.length; position++) {
    const entry = capacities[position]!;
    if (typeof entry !== "object" || entry === null) {
      throw new EffectPlanError("EP_INVALID_INPUT", `capacity ${position} must be an object`);
    }
    if (typeof entry.key !== "string" || entry.key.length === 0) {
      throw new EffectPlanError("EP_INVALID_INPUT", `capacity ${position} needs a key`);
    }
    if (index.has(entry.key)) {
      throw new EffectPlanError("EP_DUPLICATE_KEY", `capacity ${position} repeats a key`);
    }
    const capacity = entry.capacity;
    if (typeof capacity !== "number" || !Number.isInteger(capacity) || capacity <= 0) {
      throw new EffectPlanError("EP_INVALID_INPUT", `capacity ${position} must be a positive integer`);
    }
    if (capacity > limits.maxCapacity) {
      throw new EffectPlanError("EP_LIMIT_EXCEEDED", `capacity ${position} is too large`);
    }
    index.set(entry.key, keys.length);
    keys.push(entry.key);
    values.push(capacity);
  }
  return { keys, index, capacities: values, count: keys.length };
}

/** Views over the engine's capacity-resource regions. */
export interface ResourceViews {
  readonly buffer: ArrayBufferLike;
  readonly capacities: Uint32Array;
  readonly offsets: Uint32Array;
  /** Two `u32` per record: the resource index and the amount. */
  readonly records: Uint32Array;
}

/**
 * Maps the capacity-resource regions once. Growing the memory detaches the
 * previous buffer and every view on it, so the caller must re-map whenever the
 * engine's buffer identity changes.
 */
export function packResources(buffer: ArrayBufferLike, info: EngineInfo): ResourceViews {
  const layout = info.layout;
  return {
    buffer,
    capacities: new Uint32Array(buffer, layout.capacities.ptr, info.limits.maxCapacityResources),
    offsets: new Uint32Array(buffer, layout.demandOffsets.ptr, info.limits.maxCalls + 1),
    records: new Uint32Array(buffer, layout.demandRecords.ptr, info.limits.maxDemandRecords * 2),
  };
}

/**
 * Writes the capacity table, the per-call demand offsets and the demand
 * records into the engine's regions.
 *
 * The offsets are CSR: the first is zero, each next one is the end of the
 * previous call's demands, and the last is the total number of records. The
 * engine refuses anything else, so the terminator is written explicitly rather
 * than left to whatever was in the arena.
 */
export function writeResources(
  views: ResourceViews,
  info: EngineInfo,
  table: CapacityTable,
  demands: readonly (readonly CapacityDemand[])[] | undefined,
  callCount: number,
): number {
  const { capacities, offsets, records } = views;
  for (let position = 0; position < table.count; position++) {
    capacities[position] = table.capacities[position]!;
  }

  let cursor = 0;
  offsets[0] = 0;
  if (demands !== undefined && demands.length !== callCount) {
    throw new EffectPlanError(
      "EP_INVALID_INPUT",
      `${demands.length} demand lists for ${callCount} calls`,
    );
  }
  for (let call = 0; call < callCount; call++) {
    const list = demands?.[call] ?? [];
    if (!Array.isArray(list)) {
      throw new EffectPlanError("EP_INVALID_INPUT", `call ${call} demands must be an array`);
    }
    for (const demand of list) {
      if (typeof demand !== "object" || demand === null) {
        throw new EffectPlanError("EP_INVALID_INPUT", `call ${call} demand must be an object`);
      }
      if (typeof demand.amount !== "number" || !Number.isInteger(demand.amount) || demand.amount <= 0) {
        throw new EffectPlanError("EP_INVALID_INPUT", `call ${call} demand amount must be positive`);
      }
      if (demand.amount > info.limits.maxCapacity) {
        throw new EffectPlanError("EP_LIMIT_EXCEEDED", `call ${call} demand amount is too large`);
      }
      if (cursor >= info.limits.maxDemandRecords) {
        throw new EffectPlanError(
          "EP_LIMIT_EXCEEDED",
          `${cursor + 1} demand records exceeds ${info.limits.maxDemandRecords}`,
        );
      }
      const at = table.index.get(demand.resource);
      if (at === undefined) {
        throw new EffectPlanError(
          "EP_INVALID_INPUT",
          `call ${call} demands an undeclared resource`,
        );
      }
      records[cursor * 2] = at;
      records[cursor * 2 + 1] = demand.amount;
      cursor += 1;
    }
    offsets[call + 1] = cursor;
  }
  return cursor;
}

/**
 * The quality certificate for a completed placement.
 *
 * The bounds are integers the engine derived; the ratio is not among them,
 * because a quotient is presentation and this project keeps presentation in the
 * host. `lowerBoundGap` is therefore computed here, and it is a distance from a
 * bound - never an optimality gap. `LB == 0` has no gap, and dividing by it
 * would be an invention rather than a measurement.
 */
export interface ScheduleQuality {
  /** `T`, the schedule's makespan, in the caller's unit. */
  readonly makespan: number;
  /** Critical-path duration at unlimited capacity: a lower bound on `T`. */
  readonly dependencyLowerBound: number;
  /** `max_r ceil(busy_r / capacity_r)`: another lower bound on `T`. */
  readonly resourceLowerBound: number;
  /** The stronger of the two bounds. */
  readonly lowerBound: number;
  /**
   * `(makespan - lowerBound) / lowerBound`, or `null` when the bound is zero.
   * A distance from a bound: a schedule can sit above this number and still be
   * optimal, because the bound does not capture every dependency/capacity
   * interaction.
   */
  readonly lowerBoundGap: number | null;
  /**
   * `true` only when the makespan equals the bound, which no valid schedule can
   * beat. `false` means "not proven", not "suboptimal".
   */
  readonly provablyOptimal: boolean;
  /** `Σ_i (start_i - earliestStart_i)`: aggregate time lost waiting for capacity. */
  readonly totalResourceWaiting: number;
  readonly callCount: number;
  readonly resourceCount: number;
}

/** Certificate field offsets; must match `engine/src/quality.rs`. */
const QUALITY = {
  revision: 0,
  callCount: 4,
  makespan: 8,
  lbDep: 12,
  lbRes: 16,
  lb: 20,
  provablyOptimal: 24,
  resourceCount: 28,
  waiting: 32,
} as const;

/** Decodes the certificate the engine published for the placement just made. */
export function decodeQuality(view: DataView, at: number): ScheduleQuality {
  const revision = view.getUint32(at + QUALITY.revision, true);
  if (revision !== EXPECTED_QUALITY_ABI) {
    throw new EffectPlanError("EP_INVALID_PLAN", `quality certificate revision ${revision}`);
  }
  const makespan = view.getUint32(at + QUALITY.makespan, true);
  const lowerBound = view.getUint32(at + QUALITY.lb, true);
  if (lowerBound > makespan) {
    // The engine refuses to publish this; if it ever slips through, the host
    // refuses too rather than reporting a schedule that beats a bound.
    throw new EffectPlanError("EP_INVALID_PLAN", `bound ${lowerBound} above makespan ${makespan}`);
  }
  return {
    makespan,
    dependencyLowerBound: view.getUint32(at + QUALITY.lbDep, true),
    resourceLowerBound: view.getUint32(at + QUALITY.lbRes, true),
    lowerBound,
    lowerBoundGap: lowerBound === 0 ? null : (makespan - lowerBound) / lowerBound,
    provablyOptimal: view.getUint32(at + QUALITY.provablyOptimal, true) === 1,
    totalResourceWaiting: Number(view.getBigUint64(at + QUALITY.waiting, true)),
    callCount: view.getUint32(at + QUALITY.callCount, true),
    resourceCount: view.getUint32(at + QUALITY.resourceCount, true),
  };
}

/** Decodes the diagnostics the engine recorded. */
export function decodeDiagnostics(
  view: DataView,
  at: number,
  table: CapacityTable,
  callIds: readonly string[],
): readonly ResourceDiagnostic[] {
  const revision = view.getUint32(at + DIAG.revision, true);
  if (revision !== EXPECTED_RESOURCE_REVISION) {
    throw new EffectPlanError("EP_INVALID_PLAN", `resource diagnostics revision ${revision}`);
  }
  const count = view.getUint32(at + DIAG.count, true);
  const out: ResourceDiagnostic[] = [];
  for (let index = 0; index < count; index++) {
    const record = at + DIAG.entries + index * DIAG.entryBytes;
    const kind = view.getUint32(record, true);
    const resourceIndex = view.getUint32(record + 4, true);
    const callIndex = view.getUint32(record + 8, true);
    const value = view.getUint32(record + 12, true);
    out.push({
      kind,
      resource: table.keys[resourceIndex],
      resourceIndex,
      call: callIds[callIndex],
      callIndex,
      value,
    });
  }
  return Object.freeze(out);
}

/** Decodes the placement: the schedule header, the windows and the statistics. */
export function decodePlacement(
  view: DataView,
  layout: EngineLayout,
  table: CapacityTable,
  callIds: readonly string[],
): {
  readonly schedule: readonly ResourceWindow[];
  readonly makespan: number;
  readonly deferrals: number;
  readonly statistics: readonly ResourceStatistic[];
} {
  const revision = view.getUint32(layout.schedule.ptr + SCHEDULE.revision, true);
  if (revision !== EXPECTED_RESOURCE_REVISION) {
    throw new EffectPlanError("EP_INVALID_PLAN", `resource schedule revision ${revision}`);
  }
  const placed = view.getUint32(layout.schedule.ptr + SCHEDULE.placed, true);
  const makespan = view.getUint32(layout.schedule.ptr + SCHEDULE.makespan, true);
  const deferrals = view.getUint32(layout.schedule.ptr + SCHEDULE.deferrals, true);
  const windows: ResourceWindow[] = [];
  for (let call = 0; call < placed; call++) {
    const entry = layout.schedule.ptr + SCHEDULE.entries + call * SCHEDULE.entryBytes;
    windows.push({
      callIndex: call,
      callId: callIds[call] ?? "",
      start: view.getUint32(entry, true),
      finish: view.getUint32(entry + 4, true),
    });
  }

  const statsRevision = view.getUint32(layout.resourceStats.ptr + STATS.revision, true);
  if (statsRevision !== EXPECTED_RESOURCE_REVISION) {
    throw new EffectPlanError("EP_INVALID_PLAN", `resource statistics revision ${statsRevision}`);
  }
  const count = view.getUint32(layout.resourceStats.ptr + STATS.count, true);
  const statistics: ResourceStatistic[] = [];
  for (let index = 0; index < count; index++) {
    const entry = layout.resourceStats.ptr + STATS.entries + index * STATS.entryBytes;
    statistics.push({
      resourceIndex: index,
      key: table.keys[index] ?? "",
      capacity: view.getUint32(entry, true),
      peak: view.getUint32(entry + 4, true),
      busyTime: view.getBigUint64(entry + 8, true),
    });
  }
  return { schedule: Object.freeze(windows), makespan, deferrals, statistics: Object.freeze(statistics) };
}

/** The engine's "this batch does not fit" statuses, in its own numbering. */
const ERR_RESOURCE_CAPACITY = 133;
const ERR_UNSCHEDULABLE = 137;

function resourceError(status: number): EffectPlanError {
  const known = status === ERR_RESOURCE_CAPACITY ? "the demands exceed the capacity" : "the batch cannot be scheduled";
  return new EffectPlanError("EP_ENGINE_ERROR", `the engine refused the placement with status ${status} (${known})`);
}

/**
 * Asks the engine to certify the placement it just published.
 *
 * A refusal here is not a caller's problem: every region involved belongs to the
 * batch that was placed moments ago, so the only way this fails is if the
 * engine's own outputs disagree about the batch they describe. That is reported
 * as an invalid plan rather than swallowed, because a schedule nobody can
 * certify is a schedule nobody should be told is optimal.
 */
export function certifyPlacement(
  engine: LoadedEngine,
  callCount: number,
  resourceCount: number,
  placedMakespan: number,
): ScheduleQuality {
  const layout = engine.info.layout;
  const status = engine.resourceQuality(
    engine.info.abiVersion,
    callCount,
    resourceCount,
    layout.schedule.ptr,
    layout.schedule.bytes,
    layout.timing.ptr,
    layout.timing.bytes,
    layout.resourceStats.ptr,
    layout.resourceStats.bytes,
    layout.capacities.ptr,
    layout.capacities.bytes,
    layout.quality.ptr,
    layout.quality.bytes,
  );
  if (status !== 0) {
    throw new EffectPlanError(
      "EP_INVALID_PLAN",
      `the engine could not certify its own placement (status ${status})`,
    );
  }
  const quality = decodeQuality(new DataView(engine.memory.buffer), layout.quality.ptr);
  if (quality.makespan !== placedMakespan) {
    // Two reads of the same placement, computed by two paths, disagreeing. The
    // certificate recomputes the makespan from the windows rather than copying
    // the header, so this is a real cross-check and not a tautology.
    throw new EffectPlanError(
      "EP_INVALID_PLAN",
      `certificate makespan ${quality.makespan} disagrees with the placement ${placedMakespan}`,
    );
  }
  return quality;
}

/**
 * Places a planned batch against a capacity table and decodes the result.
 *
 * The call sequence lives here so that a stateless plan and an incremental
 * session place a batch the same way. There is one authoritative path from
 * declarations to a placement; a second copy of it would be a second answer to
 * the same question, and the two would drift.
 *
 * The batch must already have been planned: this reads the graph, the durations
 * and the timing the planner published, and writes only the placement.
 */
export function placeResources(
  engine: LoadedEngine,
  views: ResourceViews,
  table: CapacityTable,
  demands: readonly (readonly CapacityDemand[])[] | undefined,
  callCount: number,
  callIds: readonly string[],
): ResourceResult {
  const info = engine.info;
  const layout = info.layout;
  const demandCount = writeResources(views, info, table, demands, callCount);
  const status = engine.resourceSchedule(
    info.abiVersion,
    callCount,
    table.count,
    demandCount,
    layout.capacities.ptr,
    layout.capacities.bytes,
    layout.demandOffsets.ptr,
    layout.demandOffsets.bytes,
    layout.demandRecords.ptr,
    layout.demandRecords.bytes,
    layout.durations.ptr,
    layout.durations.bytes,
    layout.output.ptr,
    layout.output.bytes,
    layout.timing.ptr,
    layout.timing.bytes,
    layout.schedule.ptr,
    layout.schedule.bytes,
    layout.resourceStats.ptr,
    layout.resourceStats.bytes,
    layout.resourceDiagnostics.ptr,
    layout.resourceDiagnostics.bytes,
  );
  const view = new DataView(engine.memory.buffer);
  const diagnostics = decodeDiagnostics(view, layout.resourceDiagnostics.ptr, table, callIds);
  if (status === 0) {
    const placed = decodePlacement(view, layout, table, callIds);
    return {
      outcome: "feasible",
      diagnostics,
      schedule: placed.schedule,
      makespan: placed.makespan,
      deferrals: placed.deferrals,
      statistics: placed.statistics,
      quality: certifyPlacement(engine, callCount, table.count, placed.makespan),
    };
  }
  if (status === ERR_RESOURCE_CAPACITY || status === ERR_UNSCHEDULABLE) {
    return {
      outcome: "infeasible",
      diagnostics,
      schedule: [],
      makespan: 0,
      deferrals: 0,
      statistics: [],
      quality: undefined,
    };
  }
  throw resourceError(status);
}
