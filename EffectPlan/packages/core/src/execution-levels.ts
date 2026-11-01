/**
 * Execution levels: which calls of a batch may run in parallel.
 *
 * The Rust engine owns the mathematics. It computes one level per call from
 * the dependency graph it produced, and the levels arrive in the canonical
 * levels region: a `u32` count followed by one `u16` level per call.
 *
 * This module only presents them. It groups the calls by level and refuses
 * anything that does not describe a partition of the batch. It never derives a
 * dependency from the levels, and it never computes a level of its own.
 */

import { EffectPlanError, LEVELS_HEADER_BYTES } from "./errors.js";
import type { EngineInfo } from "./loader.js";

export interface ExecutionLevels {
  /**
   * The level of every call, indexed by the call's position in the batch.
   * Two calls in the same wave have the same level.
   */
  readonly levelByCall: readonly number[];
  /**
   * The waves: call indices grouped by level. The waves are in ascending
   * level order, and the calls inside a wave keep their input order.
   */
  readonly waves: readonly (readonly number[])[];
  /** The number of waves, which is the highest level plus one. */
  readonly waveCount: number;
}

/**
 * Reads the levels the engine wrote and groups them into waves.
 *
 * The count in the region header must be the call count of the batch, so a
 * stale region from an earlier batch can never be mistaken for this one.
 */
export function decodeExecutionLevels(
  view: DataView,
  info: EngineInfo,
  callCount: number,
): ExecutionLevels {
  const base = info.layout.levels.ptr;
  const count = view.getUint32(base, true);
  if (count !== callCount) {
    throw new EffectPlanError(
      "EP_INVALID_PLAN",
      `levels region holds ${count} entries, the batch has ${callCount} calls`,
    );
  }

  const levels: number[] = [];
  let highest = -1;
  for (let call = 0; call < count; call++) {
    const level = view.getUint16(base + LEVELS_HEADER_BYTES + call * 2, true);
    if (level >= callCount) {
      throw new EffectPlanError("EP_INVALID_PLAN", `level ${level} is out of range`);
    }
    levels.push(level);
    if (level > highest) highest = level;
  }

  const waves: number[][] = [];
  for (let level = 0; level <= highest; level++) waves.push([]);
  for (let call = 0; call < count; call++) waves[levels[call]!]!.push(call);
  // A level can only exist if some call at the level below exists, so the
  // waves must be contiguous. A hole would mean the levels do not describe a
  // partition of the batch.
  for (const wave of waves) {
    if (wave.length === 0) {
      throw new EffectPlanError("EP_INVALID_PLAN", "the waves are not contiguous");
    }
  }

  return Object.freeze({
    levelByCall: Object.freeze(levels),
    waves: Object.freeze(waves.map((wave) => Object.freeze(wave))),
    waveCount: waves.length,
  });
}
