/**
 * The public planner: one engine call per batch, re-validation of the graph that
 * comes back, and the immutable plan handle handed to the caller.
 */

import { EDGE_BYTES, EffectPlanError, engineError, HEADER_BYTES } from "./errors.js";
import type { EffectPlanErrorCode } from "./errors.js";
import { encodeBatch, packInput } from "./encode.js";
import type { EncodedBatch, PackedInput, PlanOptions, ToolCall } from "./encode.js";
import { decodeExecutionLevels } from "./execution-levels.js";
import type { ExecutionLevels } from "./execution-levels.js";
import { decodeTiming, encodeDurations } from "./timing.js";
import type { TimingAnalysis, TimingOptions } from "./timing.js";
import { loadEngine } from "./loader.js";
import type { EngineInfo, InitializationOptions, LoadedEngine } from "./loader.js";
import {
  decodeDiagnostics,
  decodePlacement,
  internCapacities,
  packResources,
  writeResources,
} from "./resources.js";
import type {
  CapacityTable,
  ResourceDiagnostic,
  ResourceOptions,
  ResourceResult,
  ResourceStatistic,
  ResourceWindow,
  ResourceViews,
} from "./resources.js";
import { createSession } from "./session.js";
import type { IncrementalCall, PlanSession, SessionOptions } from "./session.js";

export interface PlanStatistics {
  readonly callCount: number;
  readonly resourceAccessCount: number;
  readonly dependencyCount: number;
  readonly unknownEffectCount: number;
  readonly exclusiveCount: number;
}

/** Dependency record. `from` always precedes `to` in input order. */
export interface DependencyEdge {
  readonly from: number;
  readonly to: number;
  /** Request-local resource id; 0 marks the internal global barrier. */
  readonly resource: number;
  /** 1 write/read, 2 read/write, 3 write/write, 4..6 the same over the barrier. */
  readonly reason: number;
}

export interface ExecutionPlan {
  readonly version: 1;
  readonly callCount: number;
  readonly edgeCount: number;
  readonly statistics: PlanStatistics;
  readonly edges: readonly DependencyEdge[];
  /** Which calls may run in parallel. */
  readonly execution: ExecutionLevels;
  /**
   * Duration-aware analysis, or `undefined` when the batch was planned without
   * `options.timing`. The engine computed it from the graph above and the
   * durations the caller supplied.
   */
  readonly timing: TimingAnalysis | undefined;
}

/**
 * A finished plan. It owns a private copy of the graph and of the levels, so
 * the engine's output region can be reused by the next batch without
 * disturbing this handle.
 */
class PlanHandle implements ExecutionPlan {
  readonly version = 1 as const;
  readonly callCount: number;
  readonly edgeCount: number;
  readonly statistics: PlanStatistics;
  readonly execution: ExecutionLevels;
  readonly timing: TimingAnalysis | undefined;
  readonly #view: DataView;
  #edges: readonly DependencyEdge[] | undefined;

  constructor(
    callCount: number,
    graph: Int32Array,
    statistics: PlanStatistics,
    execution: ExecutionLevels,
    timing: TimingAnalysis | undefined,
  ) {
    this.callCount = callCount;
    this.edgeCount = graph.length / 4;
    this.statistics = statistics;
    this.execution = execution;
    this.timing = timing;
    this.#view = new DataView(graph.buffer, graph.byteOffset, graph.byteLength);
  }

  get edges(): readonly DependencyEdge[] {
    if (this.#edges === undefined) {
      const out: DependencyEdge[] = [];
      for (let at = 0; at < this.#view.byteLength; at += EDGE_BYTES) {
        out.push(Object.freeze({
          from: this.#view.getUint32(at, true),
          to: this.#view.getUint32(at + 4, true),
          resource: this.#view.getUint32(at + 8, true),
          reason: this.#view.getUint32(at + 12, true),
        }));
      }
      this.#edges = Object.freeze(out);
    }
    return this.#edges;
  }
}

/**
 * A plan and its resource verdict.
 *
 * The dependency plan and the capacity verdict are separate answers: a batch
 * can have a perfectly good dependency graph and still be impossible to run
 * because the capacity it asks for does not exist.
 */
/**
 * The engine's capacity-resource statuses.
 *
 * Two of them are verdicts rather than faults — a demand that cannot be met
 * and a batch that cannot be sequenced — and the caller sees them as an
 * outcome. The rest are faults and become errors. These are implementation
 * details of this path, so they are named here rather than exported: a
 * published declaration costs every consumer bytes.
 */
const ERR_RESOURCE_CAPACITY = 133;
const ERR_RESOURCE_RANGE = 134;
const ERR_RESOURCE_UNDECLARED = 135;
const ERR_UNSCHEDULABLE = 137;

/** Maps a resource status that is a fault onto a stable public error. */
function resourceError(status: number): EffectPlanError {
  const code: EffectPlanErrorCode =
    status === ERR_RESOURCE_RANGE
      ? "EP_LIMIT_EXCEEDED"
      : status === ERR_RESOURCE_UNDECLARED
        ? "EP_INVALID_INPUT"
        : "EP_ENGINE_ERROR";
  return new EffectPlanError(code, `engine status ${status}`);
}

export interface ResourcePlan extends ResourceResult {
  /** The dependency plan: the graph, the levels and the timing analysis. */
  readonly plan: ExecutionPlan;
}

export class EffectPlan {
  #engine: LoadedEngine;
  #input: PackedInput;
  #resources: ResourceViews;

  private constructor(engine: LoadedEngine, input: PackedInput, resources: ResourceViews) {
    this.#engine = engine;
    this.#input = input;
    this.#resources = resources;
  }

  static async create(options: InitializationOptions = {}): Promise<EffectPlan> {
    const engine = await loadEngine(options);
    const buffer = engine.memory.buffer;
    return new EffectPlan(
      engine,
      packInput(buffer, engine.info),
      packResources(buffer, engine.info),
    );
  }

  getEngineInfo(): EngineInfo {
    const info = this.#engine.info;
    const bytes = this.#engine.memory.buffer.byteLength;
    return bytes === info.memoryBytes ? info : Object.freeze({ ...info, memoryBytes: bytes });
  }

  /**
   * Re-maps the views when the host grew the memory. Growth replaces the
   * buffer and detaches every view on it, so the old ones would write
   * nowhere; the caller's next batch would land in a buffer the engine no
   * longer reads.
   */
  #remap(): { readonly input: PackedInput; readonly resources: ResourceViews } {
    const engine = this.#engine;
    const buffer = engine.memory.buffer;
    if (this.#input.buffer !== buffer) {
      this.#input = packInput(buffer, engine.info);
      this.#resources = packResources(buffer, engine.info);
    }
    return { input: this.#input, resources: this.#resources };
  }

  /**
   * Opens a session that keeps a plan alive as its workload changes.
   *
   * The session is bound to this engine instance: it restores state into the
   * engine's own regions, so a session and a stateless `planBatch` on the same
   * instance would fight over them. Use one engine per session, or let the
   * session finish before planning a batch here.
   */
  createSession(calls: readonly IncrementalCall[], options: SessionOptions = {}): PlanSession {
    const { input } = this.#remap();
    return createSession(
      { engine: this.#engine, input, decode: decodePlan },
      calls,
      options,
    );
  }

  planBatch(calls: readonly ToolCall[], options: PlanOptions = {}): ExecutionPlan {
    const engine = this.#engine;
    const { input } = this.#remap();

    const encoded = encodeBatch(input, engine.info.limits, calls, options);
    const { layout } = engine.info;
    const info = engine.info;
    // With durations, one call plans, layers and analyses. Without them, the
    // batch takes exactly the path it took before timing existed.
    const timing = options.timing;
    const durations =
      timing === undefined ? undefined : encodeDurations(input, encoded.callCount, timing);
    const p = encoded.pointers;
    // A tuple, so one list serves both entry points at their exact arity.
    const args: [number, number, number, number, number, number, number, number, number, number, number, number, number, number] = [
      info.abiVersion,
      encoded.callCount,
      encoded.accessCount,
      encoded.resourceCount,
      p.callModes,
      p.accessOffsets,
      p.resourceIds,
      p.accessModes,
      layout.workspace.ptr,
      layout.workspace.bytes,
      layout.output.ptr,
      layout.output.bytes,
      layout.levels.ptr,
      layout.levels.bytes,
    ];
    const status =
      durations === undefined
        ? engine.planV2(...args)
        : engine.planV3(
            ...args,
            layout.durations.ptr,
            layout.durations.bytes,
            layout.timing.ptr,
            layout.timing.bytes,
          );
    if (status !== 0) throw engineError(status);
    return decodePlan(
      input.view,
      engine.info,
      encoded,
      durations === undefined
        ? undefined
        : { durations, unit: timing?.unit ?? "ticks" },
    );
  }

  /**
   * Checks a batch against declared capacity resources.
   *
   * No graph is built and no clock is involved: the engine is asked whether
   * the demands can be met at all. The answer is an outcome and the
   * diagnostics behind it — a demand that cannot be met is a verdict about
   * the batch, not a fault in the call.
   */
  checkResources(calls: readonly ToolCall[], options: ResourceOptions): ResourceResult {
    const engine = this.#engine;
    const info = engine.info;
    const layout = info.layout;
    const { input, resources } = this.#remap();
    const table = internCapacities(options.capacities, info.limits);
    const demandCount = writeResources(resources, info, table, options.demands, calls.length);
    const status = engine.resourceCheck(
      info.abiVersion,
      calls.length,
      table.count,
      demandCount,
      layout.capacities.ptr,
      layout.capacities.bytes,
      layout.demandOffsets.ptr,
      layout.demandOffsets.bytes,
      layout.demandRecords.ptr,
      layout.demandRecords.bytes,
      layout.resourceDiagnostics.ptr,
      layout.resourceDiagnostics.bytes,
    );
    const diagnostics = decodeDiagnostics(
      input.view,
      layout.resourceDiagnostics.ptr,
      table,
      calls.map((call) => call.id),
    );
    if (status === 0) {
      return {
        outcome: "feasible",
        diagnostics,
        schedule: [],
        makespan: 0,
        deferrals: 0,
        statistics: [],
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
      };
    }
    throw resourceError(status);
  }

  /**
   * Plans the batch and places it against the declared capacity resources.
   *
   * The dependency plan and the capacity verdict are separate answers, and both
   * come back: a batch can have a perfectly good graph and still be impossible
   * to run. Durations are required — a placement without them has no clock to
   * sit on.
   */
  planResources(calls: readonly ToolCall[], options: ResourceOptions): ResourcePlan {
    const engine = this.#engine;
    const info = engine.info;
    const layout = info.layout;
    const { input, resources } = this.#remap();
    if (options.durations === undefined) {
      throw new EffectPlanError("EP_INVALID_INPUT", "durations are required to place a batch");
    }
    const planOptions: PlanOptions = options.unknownEffectPolicy === undefined
      ? {}
      : { unknownEffectPolicy: options.unknownEffectPolicy };
    const encoded = encodeBatch(input, info.limits, calls, planOptions);
    const timingOptions: TimingOptions = options.unit === undefined
      ? { durations: options.durations }
      : { durations: options.durations, unit: options.unit };
    const durations = encodeDurations(input, encoded.callCount, timingOptions);
    const p = encoded.pointers;
    // Version three, not four: a stateless call must not inherit the checkpoint
    // the previous call committed. v4 resumes from the checkpoint region, and a
    // record left by a different batch is correctly refused as stale (131/132) —
    // which would make the second of two unrelated batches fail. Placement needs
    // the graph, the durations and the timing, and v3 publishes all three.
    const planStatus = engine.planV3(
      info.abiVersion,
      encoded.callCount,
      encoded.accessCount,
      encoded.resourceCount,
      p.callModes,
      p.accessOffsets,
      p.resourceIds,
      p.accessModes,
      layout.workspace.ptr,
      layout.workspace.bytes,
      layout.output.ptr,
      layout.output.bytes,
      layout.levels.ptr,
      layout.levels.bytes,
      layout.durations.ptr,
      layout.durations.bytes,
      layout.timing.ptr,
      layout.timing.bytes,
    );
    if (planStatus !== 0) throw engineError(planStatus);

    const table = internCapacities(options.capacities, info.limits);
    const demandCount = writeResources(resources, info, table, options.demands, encoded.callCount);
    const placeStatus = engine.resourceSchedule(
      info.abiVersion,
      encoded.callCount,
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
    const view = input.view;
    const callIds = calls.map((call) => call.id);
    const diagnostics = decodeDiagnostics(view, layout.resourceDiagnostics.ptr, table, callIds);
    const plan = decodePlan(input.view, info, encoded, { durations, unit: options.unit ?? "ticks" });
    if (placeStatus === 0) {
      const placed = decodePlacement(view, layout, table, callIds);
      return {
        outcome: "feasible",
        diagnostics,
        schedule: placed.schedule,
        makespan: placed.makespan,
        deferrals: placed.deferrals,
        statistics: placed.statistics,
        plan,
      };
    }
    if (placeStatus === ERR_RESOURCE_CAPACITY || placeStatus === ERR_UNSCHEDULABLE) {
      return {
        outcome: "infeasible",
        diagnostics,
        schedule: [],
        makespan: 0,
        deferrals: 0,
        statistics: [],
        plan,
      };
    }
    throw resourceError(placeStatus);
  }
}

/**
 * Re-validates the engine's output before handing it to a caller.
 *
 * Exported for the benchmark harness, which measures the decode stage on its
 * own. It is not part of the public surface: `index.ts` does not re-export it.
 */
export function decodePlan(
  view: DataView,
  info: EngineInfo,
  encoded: EncodedBatch,
  timing?: { readonly durations: readonly number[]; readonly unit: string },
): ExecutionPlan {
  const out = info.layout.output.ptr;
  if (view.getUint32(out, true) !== info.abiVersion) {
    throw new EffectPlanError("EP_INVALID_PLAN", "output header abi mismatch");
  }
  if (view.getUint32(out + 4, true) !== encoded.callCount) {
    throw new EffectPlanError("EP_INVALID_PLAN", "output header call count mismatch");
  }
  if (view.getUint32(out + 12, true) !== 0) {
    throw new EffectPlanError("EP_INVALID_PLAN", "output header reserved word set");
  }
  const edgeCount = view.getUint32(out + 8, true);
  if (edgeCount > info.limits.maxEdges) {
    throw new EffectPlanError("EP_INVALID_PLAN", `edge count ${edgeCount}`);
  }
  const graph = new Int32Array(edgeCount * 4);
  const records = new DataView(graph.buffer);
  let previousTo = -1;
  for (let e = 0; e < edgeCount; e++) {
    const at = out + HEADER_BYTES + e * EDGE_BYTES;
    const from = view.getUint32(at, true);
    const to = view.getUint32(at + 4, true);
    const resource = view.getUint32(at + 8, true);
    const reason = view.getUint32(at + 12, true);
    if (!(from < to && to < encoded.callCount)) {
      throw new EffectPlanError("EP_INVALID_PLAN", `edge ${e} is not forward-pointing`);
    }
    if (!(resource < encoded.resourceCount)) {
      throw new EffectPlanError("EP_INVALID_PLAN", `edge ${e} resource index`);
    }
    if (!(reason >= 1 && reason <= 6)) {
      throw new EffectPlanError("EP_INVALID_PLAN", `edge ${e} reason code`);
    }
    if (to < previousTo) {
      throw new EffectPlanError("EP_INVALID_PLAN", `edge ${e} out of order`);
    }
    previousTo = to;
    records.setUint32(e * 16, from, true);
    records.setUint32(e * 16 + 4, to, true);
    records.setUint32(e * 16 + 8, resource, true);
    records.setUint32(e * 16 + 12, reason, true);
  }
  return new PlanHandle(
    encoded.callCount,
    graph,
    Object.freeze({
      callCount: encoded.callCount,
      resourceAccessCount: encoded.accessCount,
      dependencyCount: edgeCount,
      unknownEffectCount: encoded.unknownEffectCount,
      exclusiveCount: encoded.exclusiveCount,
    }),
    decodeExecutionLevels(view, info, encoded.callCount),
    timing === undefined
      ? undefined
      : decodeTiming(view, info, timing.durations, graph, encoded.callCount, timing.unit),
  );
}
