/**
 * Independent correctness oracle. Development only: never bundled into the
 * published package.
 *
 * It deliberately uses the slow all-pairs formulation from the documented
 * conflict matrix, so it shares no algorithmic structure with the streaming
 * Rust engine. The engine emits a sparse covering graph; this oracle emits every
 * conflicting pair, which is the transitive closure of that graph. Parity is
 * therefore checked on closure equality plus a subset check, not raw edge sets.
 */

import type { EffectMode, ToolCall } from "../packages/core/src/index.js";

export type AccessMode = "read" | "write";

export interface NormalizedCall {
  readonly mode: EffectMode;
  /** Interned resource key to effective access mode; write wins over read. */
  readonly resources: ReadonlyMap<string, AccessMode>;
}

const GLOBAL_MODES: ReadonlySet<EffectMode> = /* @__PURE__ */ new Set(["exclusive", "unknown"]);

export function resourceKey(namespace: string, id: string): string {
  return `${namespace.length}:${namespace}${id.length}:${id}`;
}

/** Applies the documented normalisation: missing contract is unknown, and a call
 * declaring both read and write of one resource effectively writes it. */
export function normalize(calls: readonly ToolCall[]): NormalizedCall[] {
  return calls.map((call) => {
    const effect = call.effect;
    const mode: EffectMode = effect === undefined || effect === null ? "unknown" : effect.mode;
    const resources = new Map<string, AccessMode>();
    for (const access of effect?.accesses ?? []) {
      const key = resourceKey(access.resource.namespace, access.resource.id);
      const write = access.mode === "write";
      if (write) resources.set(key, "write");
      else if (!resources.has(key)) resources.set(key, "read");
    }
    return { mode, resources };
  });
}

/**
 * Every pair (i, j) with i < j that the conflict matrix says must be ordered.
 * O(N^2) pair enumeration plus O(A) set intersection per pair, with no
 * resource-indexed state anywhere.
 */
export function referencePlan(calls: readonly ToolCall[]): Array<readonly [number, number]> {
  const list = normalize(calls);
  const pairs: Array<readonly [number, number]> = [];
  for (let i = 0; i < list.length; i++) {
    for (let j = i + 1; j < list.length; j++) {
      if (conflicts(list[i]!, list[j]!)) pairs.push([i, j]);
    }
  }
  return pairs;
}

function conflicts(a: NormalizedCall, b: NormalizedCall): boolean {
  // Unknown and exclusive operations are global barriers against everything.
  if (GLOBAL_MODES.has(a.mode) || GLOBAL_MODES.has(b.mode)) return true;
  for (const [key, modeA] of a.resources) {
    const modeB = b.resources.get(key);
    if (modeB === undefined) continue;
    // Read/read is the only compatible combination.
    if (modeA === "write" || modeB === "write") return true;
  }
  return false;
}

/** Reachability over the forward-only edge set, as a sorted pair list. */
export function transitiveClosure(
  callCount: number,
  edges: ReadonlyArray<readonly [number, number]>,
): Array<readonly [number, number]> {
  const reach: boolean[][] = Array.from({ length: callCount }, () => new Array(callCount).fill(false));
  for (const [from, to] of edges) reach[from]![to] = true;
  // Warshall, applied in ascending intermediate order; edges only point forward.
  for (let k = 0; k < callCount; k++) {
    for (let i = 0; i < callCount; i++) {
      if (!reach[i]![k]) continue;
      for (let j = k + 1; j < callCount; j++) {
        if (reach[k]![j]) reach[i]![j] = true;
      }
    }
  }
  const out: Array<readonly [number, number]> = [];
  for (let i = 0; i < callCount; i++) {
    for (let j = 0; j < callCount; j++) if (reach[i]![j]) out.push([i, j]);
  }
  return out;
}

export function pairSet(pairs: ReadonlyArray<readonly [number, number]>): Set<string> {
  return new Set(pairs.map(([a, b]) => `${a}>${b}`));
}

/** True when the graph has no cycle. Forward-only edges make this cheap. */
export function isAcyclic(callCount: number, edges: ReadonlyArray<readonly [number, number]>): boolean {
  if (edges.some(([from, to]) => !(from < to))) return false;
  const outgoing = Array.from({ length: callCount }, () => new Set<number>());
  for (const [from, to] of edges) outgoing[from]!.add(to);
  const state = new Array<number>(callCount).fill(0);
  const visit = (node: number): boolean => {
    if (state[node] === 1) return false;
    if (state[node] === 2) return true;
    state[node] = 1;
    for (const next of outgoing[node]!) if (!visit(next)) return false;
    state[node] = 2;
    return true;
  };
  for (let i = 0; i < callCount; i++) if (!visit(i)) return false;
  return true;
}

/** Immediate predecessors of a node, ascending. */
export function predecessors(
  callCount: number,
  edges: ReadonlyArray<readonly [number, number]>,
): number[][] {
  const out: number[][] = Array.from({ length: callCount }, () => []);
  for (const [from, to] of edges) out[to]!.push(from);
  for (const list of out) list.sort((a, b) => a - b);
  return out;
}
