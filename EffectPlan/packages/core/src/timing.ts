/**
 * Duration-aware scheduling: the caller's timing request, the engine's answer,
 * and the checks that keep one from being mistaken for the other.
 *
 * The Critical Path Method lives in Rust (`engine/src/schedule.rs`). Nothing in
 * this module computes an earliest or latest time: it validates what the caller
 * supplied, writes the durations into the canonical region the engine reads, and
 * decodes what the engine wrote into its canonical region — deriving the values
 * the contract defines as derivable (`LS = ES + S`, `EF = ES + D`, `LF = LS + D`)
 * and refusing anything the graph does not support.
 */

import { EffectPlanError, TIMING_HEADER_BYTES } from "./errors.js";
import type { EngineInfo } from "./loader.js";
import type { PackedInput } from "./encode.js";

/** The engine's largest representable duration, in ticks. */
export const MAX_DURATION_TICKS = 0xffffffff;

export interface TimingOptions {
  /**
   * One duration per call, in the same order as the calls. Every value must be
   * a positive integer no greater than 4,294,967,295 ticks. A missing entry is
   * an error, never an implied zero.
   */
  readonly durations: ArrayLike<number>;

  /**
   * The caller's unit, carried through to the result so the numbers can be read
   * back. The engine does not convert and does not interpret it.
   */
  readonly unit?: string;
}

export interface EventTiming {
  /** The call this row describes, by its position in the batch. */
  readonly callIndex: number;
  readonly duration: number;

  readonly earliestStart: number;
  readonly earliestFinish: number;

  readonly latestStart: number;
  readonly latestFinish: number;

  /** How far this call can slip without delaying the plan. */
  readonly slack: number;
  /** `slack === 0`. Derived from the engine's own slack, not re-computed. */
  readonly critical: boolean;
}

export interface TimingAnalysis {
  /** The unit the caller named, or `"ticks"`. Metadata only. */
  readonly unit: string;

  /** Sum of every duration: the work in the plan, in the caller's unit. */
  readonly totalWork: number;
  /**
   * The shortest possible completion time under the declared dependencies and
   * unlimited capacity. Also the makespan of the ASAP schedule.
   */
  readonly criticalPathDuration: number;

  /** `totalWork / criticalPathDuration`, or `null` when nothing can be timed. */
  readonly averageParallelism: number | null;
  /** Most calls alive at one tick in the earliest-start schedule. */
  readonly peakConcurrency: number;

  /** Every call, in batch order. */
  readonly events: readonly EventTiming[];
  /** The zero-slack calls, in batch order. */
  readonly criticalEvents: readonly number[];
}

/** Offsets inside the timing region; must match `engine/src/schedule.rs`. */
const FIELD = {
  revision: 0,
  eventCount: 4,
  makespan: 8,
  peak: 12,
  totalWork: 16,
  criticalCount: 24,
  reserved: 28,
  events: TIMING_HEADER_BYTES,
  eventBytes: 8,
} as const;

const EXPECTED_REVISION = 1;

/**
 * Validates the caller's durations and writes them into the canonical region.
 *
 * Every rule here exists because a wrong duration produces a plausible schedule:
 * the engine cannot tell a typo from an estimate. So nothing is defaulted,
 * clamped or rounded.
 */
export function encodeDurations(
  input: PackedInput,
  callCount: number,
  options: TimingOptions,
): readonly number[] {
  const durations = options.durations;
  if (durations === null || durations === undefined || typeof durations === "string") {
    throw new EffectPlanError("EP_INVALID_INPUT", "timing.durations must be an array-like of numbers");
  }
  const length = Number(durations.length);
  if (!Number.isInteger(length) || length < 0) {
    throw new EffectPlanError("EP_INVALID_INPUT", "timing.durations needs a valid length");
  }
  if (length !== callCount) {
    throw new EffectPlanError(
      "EP_INVALID_INPUT",
      `timing.durations has ${length} entries for ${callCount} calls`,
    );
  }
  const unit = options.unit ?? "ticks";
  if (typeof unit !== "string" || unit.length === 0 || unit.length > 32) {
    throw new EffectPlanError("EP_INVALID_INPUT", "timing.unit must be a string of 1 to 32 characters");
  }

  const view = input.durations;
  const written: number[] = [];
  for (let index = 0; index < callCount; index++) {
    const raw = durations[index];
    if (typeof raw !== "number") {
      throw new EffectPlanError(
        "EP_INVALID_INPUT",
        `duration ${index} must be a number, not ${typeof raw}`,
      );
    }
    const value = raw;
    if (!Number.isSafeInteger(value)) {
      throw new EffectPlanError(
        "EP_INVALID_INPUT",
        `duration ${index} must be an integer, not ${String(durations[index])}`,
      );
    }
    if (value <= 0) {
      throw new EffectPlanError("EP_INVALID_INPUT", `duration ${index} must be positive`);
    }
    if (value > MAX_DURATION_TICKS) {
      throw new EffectPlanError(
        "EP_LIMIT_EXCEEDED",
        `duration ${index} exceeds ${MAX_DURATION_TICKS} ticks`,
      );
    }
    view[index] = value;
    written.push(value);
  }
  return written;
}

/**
 * Reads the timing region the engine just committed and checks it against the
 * durations and the graph the same batch produced.
 *
 * `graph` is the engine's own edge buffer, four words per edge: from, to,
 * resource, reason. It is passed as the buffer rather than as edge objects so
 * that asking for timing does not cost an allocation per dependency.
 */
export function decodeTiming(
  view: DataView,
  info: EngineInfo,
  durations: readonly number[],
  graph: Int32Array,
  callCount: number,
  unit: string,
): TimingAnalysis {
  const base = info.layout.timing.ptr;
  const revision = view.getUint32(base + FIELD.revision, true);
  if (revision !== EXPECTED_REVISION) {
    throw new EffectPlanError("EP_INVALID_PLAN", `timing region revision ${revision}`);
  }
  const eventCount = view.getUint32(base + FIELD.eventCount, true);
  if (eventCount !== callCount) {
    throw new EffectPlanError(
      "EP_INVALID_PLAN",
      `timing region holds ${eventCount} events, the batch has ${callCount} calls`,
    );
  }
  if (view.getUint32(base + FIELD.reserved, true) !== 0) {
    throw new EffectPlanError("EP_INVALID_PLAN", "timing header reserved word set");
  }
  const makespan = view.getUint32(base + FIELD.makespan, true);
  const peak = view.getUint32(base + FIELD.peak, true);
  const totalLow = view.getUint32(base + FIELD.totalWork, true);
  const totalHigh = view.getUint32(base + FIELD.totalWork + 4, true);
  const criticalCount = view.getUint32(base + FIELD.criticalCount, true);
  if (totalHigh !== 0 || totalLow > Number.MAX_SAFE_INTEGER) {
    throw new EffectPlanError("EP_INVALID_PLAN", "total work is outside the representable range");
  }
  const totalWork = totalLow;
  if (peak > callCount) {
    throw new EffectPlanError("EP_INVALID_PLAN", `peak concurrency ${peak} for ${callCount} calls`);
  }

  const events: EventTiming[] = [];
  const criticalEvents: number[] = [];
  for (let index = 0; index < callCount; index++) {
    const at = base + FIELD.events + index * FIELD.eventBytes;
    const earliestStart = view.getUint32(at, true);
    const slack = view.getUint32(at + 4, true);
    const duration = durations[index]!;
    if (duration === undefined || duration <= 0) {
      throw new EffectPlanError("EP_INVALID_PLAN", `duration missing for call ${index}`);
    }
    // The derived values must stay inside the contract's own range; a schedule
    // that walks past u32 was never a legal answer.
    const earliestFinish = earliestStart + duration;
    const latestStart = earliestStart + slack;
    const latestFinish = latestStart + duration;
    if (earliestFinish > 0xffffffff || latestFinish > 0xffffffff || latestStart > makespan) {
      throw new EffectPlanError("EP_INVALID_PLAN", `timing for call ${index} is out of range`);
    }
    if (earliestStart > latestStart) {
      throw new EffectPlanError("EP_INVALID_PLAN", `negative slack for call ${index}`);
    }
    const isCritical = slack === 0;
    if (isCritical) criticalEvents.push(index);
    events.push(
      Object.freeze({
        callIndex: index,
        duration,
        earliestStart,
        earliestFinish,
        latestStart,
        latestFinish,
        slack,
        critical: isCritical,
      }),
    );
  }
  if (criticalEvents.length !== criticalCount) {
    throw new EffectPlanError(
      "EP_INVALID_PLAN",
      `engine reported ${criticalCount} critical events, the rows show ${criticalEvents.length}`,
    );
  }

  // Consistency against the graph the engine itself published: for every edge
  // the predecessor must finish no later than this call may start, and no call
  // may start later than its own deadline allows. Checking, not re-deriving.
  if (graph.length % 4 !== 0) {
    throw new EffectPlanError("EP_INVALID_PLAN", "the edge buffer is not a whole number of records");
  }
  for (let at = 0; at < graph.length; at += 4) {
    const from = graph[at]!;
    const to = graph[at + 1]!;
    if (!(from >= 0 && to >= 0 && from < callCount && to < callCount)) {
      throw new EffectPlanError("EP_INVALID_PLAN", `edge record ${at / 4} names call ${from}->${to}`);
    }
    const head = events[from]!;
    const tail = events[to]!;
    if (head.earliestFinish > tail.earliestStart) {
      throw new EffectPlanError(
        "EP_INVALID_PLAN",
        `edge ${from}->${to} violates earliest-time precedence`,
      );
    }
    if (head.latestFinish > tail.latestStart) {
      throw new EffectPlanError(
        "EP_INVALID_PLAN",
        `edge ${from}->${to} violates latest-time precedence`,
      );
    }
  }
  let latestFinishObserved = 0;
  for (const event of events) latestFinishObserved = Math.max(latestFinishObserved, event.earliestFinish);
  if (latestFinishObserved > makespan) {
    throw new EffectPlanError("EP_INVALID_PLAN", "the makespan is shorter than a scheduled finish");
  }
  if (callCount > 0) {
    const sum = durations.reduce((total, value) => total + value, 0);
    if (sum !== totalWork) {
      throw new EffectPlanError("EP_INVALID_PLAN", `total work ${totalWork} disagrees with ${sum} ticks of durations`);
    }
  } else if (totalWork !== 0 || makespan !== 0 || peak !== 0) {
    throw new EffectPlanError("EP_INVALID_PLAN", "an empty batch cannot carry timing");
  }

  return Object.freeze({
    unit,
    totalWork,
    criticalPathDuration: makespan,
    averageParallelism: callCount === 0 || makespan === 0 ? null : totalWork / makespan,
    peakConcurrency: peak,
    events: Object.freeze(events),
    criticalEvents: Object.freeze(criticalEvents),
  });
}
