/**
 * The benchmark workload, shared by the TypeScript harness and (by formula)
 * the native Rust benchmark.
 *
 *   access j of call i touches resource `(i * perCall + j) % resources`
 *   write rules:
 *     alternate3 — write when `(i + j) % 3 != 0`, otherwise read
 *     parity     — write when `i % 2 == 0`, otherwise read
 *   mode rules:
 *     resources  — every call is a resources effect
 *     barrier3   — every third call (`i % 3 == 2`) is exclusive, declaring nothing
 *     mixed      — `i % 5 == 0` exclusive, `i % 11 == 3` unknown, else resources
 *
 * `resources` is never smaller than `perCall`, so one call never names the same
 * resource twice: the TypeScript encoder dedupes intra-call duplicates while a raw
 * packer would not, and both sides must describe the identical batch.
 * The resource names are stable strings, so both sides intern them in
 * first-appearance order and end up with identical resource ids.
 */

import type { ResourceAccess, ToolCall } from "../packages/core/src/index.js";

export type WriteRule = "alternate3" | "parity";
export type ModeRule = "resources" | "barrier3" | "mixed";

export interface Workload {
  readonly name: string;
  readonly calls: number;
  readonly perCall: number;
  readonly resources: number;
  readonly writeRule: WriteRule;
  readonly modeRule: ModeRule;
  readonly iterations: number;
}

/** The four Phase 1 datasets keep their exact shape, for regression numbers. */
export const DATASETS: readonly Workload[] = [
  { name: "tiny", calls: 8, perCall: 4, resources: 4, writeRule: "alternate3", modeRule: "resources", iterations: 2_000 },
  { name: "small", calls: 32, perCall: 4, resources: 16, writeRule: "alternate3", modeRule: "resources", iterations: 1_000 },
  { name: "normal", calls: 64, perCall: 4, resources: 32, writeRule: "alternate3", modeRule: "resources", iterations: 500 },
  { name: "medium", calls: 128, perCall: 8, resources: 64, writeRule: "alternate3", modeRule: "resources", iterations: 200 },
  // The Phase 2 shapes, each stressing one property of the level algorithm.
  { name: "wide", calls: 64, perCall: 2, resources: 128, writeRule: "alternate3", modeRule: "resources", iterations: 500 },
  { name: "deep", calls: 64, perCall: 1, resources: 1, writeRule: "parity", modeRule: "resources", iterations: 500 },
  { name: "diamond", calls: 64, perCall: 2, resources: 16, writeRule: "parity", modeRule: "resources", iterations: 500 },
  { name: "barrier", calls: 64, perCall: 2, resources: 8, writeRule: "alternate3", modeRule: "barrier3", iterations: 500 },
  { name: "conflict", calls: 64, perCall: 4, resources: 4, writeRule: "alternate3", modeRule: "resources", iterations: 500 },
  { name: "maxbound", calls: 128, perCall: 8, resources: 128, writeRule: "alternate3", modeRule: "resources", iterations: 200 },
  { name: "mixed", calls: 96, perCall: 4, resources: 24, writeRule: "alternate3", modeRule: "mixed", iterations: 300 },
];

function modeOf(workload: Workload, i: number): "resources" | "exclusive" | "unknown" {
  if (workload.modeRule === "barrier3") return i % 3 === 2 ? "exclusive" : "resources";
  if (workload.modeRule === "mixed") {
    if (i % 5 === 0) return "exclusive";
    if (i % 11 === 3) return "unknown";
    return "resources";
  }
  return "resources";
}

/**
 * Deterministic durations, identical to the Rust benchmark's formula:
 * `1 + ((i * 13) % 17)` ticks for call `i`.
 */
export function buildDurations(workload: Workload): number[] {
  return Array.from({ length: workload.calls }, (_, i) => 1 + ((i * 13) % 17));
}

export function buildCalls(workload: Workload): ToolCall[] {
  const calls: ToolCall[] = [];
  for (let i = 0; i < workload.calls; i++) {
    const mode = modeOf(workload, i);
    const accesses: ResourceAccess[] = [];
    // Barrier effects declare nothing; resources effects touch the pool.
    if (mode === "resources") {
      for (let j = 0; j < workload.perCall; j++) {
        const key = (i * workload.perCall + j) % workload.resources;
        const write = workload.writeRule === "parity" ? i % 2 === 0 : (i + j) % 3 !== 0;
        accesses.push({
          resource: { namespace: "ns", id: `r${key}` },
          mode: write ? "write" : "read",
        });
      }
    }
    calls.push({ id: `c${i}`, effect: { version: 1, mode, accesses } });
  }
  return calls;
}
