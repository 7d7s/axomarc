/**
 * The public planner: one engine call per batch, re-validation of the graph that
 * comes back, and the immutable plan handle handed to the caller.
 */

import { EDGE_BYTES, EffectPlanError, engineError, HEADER_BYTES } from "./errors.js";
import { encodeBatch, packInput } from "./encode.js";
import type { EncodedBatch, PackedInput, PlanOptions, ToolCall } from "./encode.js";
import { decodeExecutionLevels } from "./execution-levels.js";
import type { ExecutionLevels } from "./execution-levels.js";
import { decodeTiming, encodeDurations } from "./timing.js";
import type { TimingAnalysis } from "./timing.js";
import { loadEngine } from "./loader.js";
import type { EngineInfo, InitializationOptions, LoadedEngine } from "./loader.js";
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

export class EffectPlan {
  #engine: LoadedEngine;
  #input: PackedInput;

  private constructor(engine: LoadedEngine, input: PackedInput) {
    this.#engine = engine;
    this.#input = input;
  }

  static async create(options: InitializationOptions = {}): Promise<EffectPlan> {
    const engine = await loadEngine(options);
    return new EffectPlan(engine, packInput(engine.memory.buffer, engine.info));
  }

  getEngineInfo(): EngineInfo {
    const info = this.#engine.info;
    const bytes = this.#engine.memory.buffer.byteLength;
    return bytes === info.memoryBytes ? info : Object.freeze({ ...info, memoryBytes: bytes });
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
    let input = this.#input;
    const buffer = this.#engine.memory.buffer;
    if (input.buffer !== buffer) input = this.#input = packInput(buffer, this.#engine.info);
    return createSession(
      { engine: this.#engine, input, decode: decodePlan },
      calls,
      options,
    );
  }

  planBatch(calls: readonly ToolCall[], options: PlanOptions = {}): ExecutionPlan {
    const engine = this.#engine;
    // A host that grew the memory detached our previous buffer and every view
    // on it; re-map before writing anything, or the batch would land nowhere.
    let input = this.#input;
    const buffer = engine.memory.buffer;
    if (input.buffer !== buffer) input = this.#input = packInput(buffer, engine.info);

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
