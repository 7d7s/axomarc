/**
 * @effectplan/core — deterministic resource-aware planning for AI tool
 * operations. Public wrapper over the Rust/WASM engine. Phase 1 surface:
 * `EffectPlan.create()`, `planBatch()` and `getEngineInfo()`.
 *
 * Responsibilities are separated across the module graph: `errors.ts` holds the
 * stable error surface, `loader.ts` instantiates and audits the engine memory,
 * `encode.ts` turns caller objects into packed integers, `planner.ts` performs
 * one ABI call per batch and validates the graph it got back.
 */

export { EffectPlan } from "./planner.js";
export { EffectPlanError } from "./errors.js";
export type { EffectPlanErrorCode } from "./errors.js";
export type {
  AccessMode,
  EffectContract,
  EffectMode,
  PlanOptions,
  ResourceAccess,
  ResourceRef,
  ToolCall,
  UnknownEffectPolicy,
} from "./encode.js";
export type {
  EngineInfo,
  EngineLayout,
  EngineLimits,
  EngineRegion,
  InitializationOptions,
} from "./loader.js";
export type { ExecutionLevels } from "./execution-levels.js";
export type { EventTiming, TimingAnalysis, TimingOptions } from "./timing.js";
export type { DependencyEdge, ExecutionPlan, PlanStatistics } from "./planner.js";
