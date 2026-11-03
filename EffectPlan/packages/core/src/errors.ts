/**
 * Stable public errors and the engine-status mapping. Nothing here leaks a
 * caller-supplied resource identifier: engine failures carry the numeric status
 * only.
 */

export type EffectPlanErrorCode =
  | "EP_INVALID_INPUT"
  | "EP_DUPLICATE_CALL_ID"
  | "EP_DUPLICATE_KEY"
  | "EP_UNKNOWN_KEY"
  | "EP_INVALID_CHANGESET"
  | "EP_STALE_VERSION"
  | "EP_SESSION_DISPOSED"
  | "EP_INVALID_EFFECT"
  | "EP_INVALID_RESOURCE"
  | "EP_UNKNOWN_EFFECT_REJECTED"
  | "EP_LIMIT_EXCEEDED"
  | "EP_INVALID_PLAN"
  | "EP_INVALID_CURSOR_STATE"
  | "EP_ENGINE_UNAVAILABLE"
  | "EP_ENGINE_ERROR";

export class EffectPlanError extends Error {
  readonly code: EffectPlanErrorCode;
  constructor(code: EffectPlanErrorCode, message: string) {
    super(`${code}: ${message}`);
    this.name = "EffectPlanError";
    this.code = code;
  }
}

/** ABI revision this wrapper was compiled against. */
export const EXPECTED_ABI = 1;
/** Revision of the execution-levels contract this wrapper was compiled against. */
export const EXPECTED_LEVELS_ABI = 1;
/** Revision of the scheduling contract this wrapper was compiled against. */
export const EXPECTED_TIMING_ABI = 1;
/** Revision of the resumable-planning contract this wrapper was compiled against. */
export const EXPECTED_CHECKPOINT_ABI = 1;
/** Revision of the capacity-resource contract this wrapper was compiled against. */
export const EXPECTED_RESOURCE_ABI = 1;
/** Revision of the schedule-quality certificate this wrapper was compiled against. */
export const EXPECTED_QUALITY_ABI = 1;
/** The certificate record: nine words, the last two reserved. */
export const QUALITY_RECORD_BYTES = 64;
export const ENGINE_VERSION = "0.3.0";
export const ENGINE_NAME = "effectplan-engine";
/** Output header and record framing, cross-checked against the engine. */
export const HEADER_BYTES = 16;
export const EDGE_BYTES = 16;
/** The levels region starts with a `u32` count, then one `u16` per call. */
export const LEVELS_HEADER_BYTES = 4;
/** Timing header: revision, count, makespan, peak, u64 work, critical count, reserved. */
export const TIMING_HEADER_BYTES = 32;
/** The checkpoint record: eight words, the last of which is the digest. */
export const CHECKPOINT_RECORD_BYTES = 32;

/** Maps a stable engine status onto a stable public error. */
export function engineError(status: number): EffectPlanError {
  const code: EffectPlanErrorCode =
    status === 101 || status === 102 || status === 103
      ? "EP_LIMIT_EXCEEDED"
      : status >= 110 && status <= 119
        ? "EP_INVALID_INPUT"
        : status === 121
          ? "EP_LIMIT_EXCEEDED"
          : "EP_ENGINE_ERROR";
  return new EffectPlanError(code, `engine status ${status}`);
}
