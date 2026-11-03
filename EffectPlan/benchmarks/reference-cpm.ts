/**
 * Independent reference implementation of Critical Path Intelligence.
 * Development only: never bundled into the published package.
 *
 * It deliberately shares nothing with the Rust engine's method:
 *
 *  - it builds explicit successor and predecessor adjacency lists,
 *  - it runs a Kahn topological order for the forward pass,
 *  - it walks the *reverse* topological order with a successor-minimum rule for
 *    the backward pass,
 *  - it computes peak concurrency with a per-tick counter map instead of a
 *    sorted endpoint sweep.
 *
 * Four different mechanisms, one expected answer. Agreement between them is
 * evidence about the mathematics, not about the code.
 */

export interface ReferenceTiming {
  readonly earliestStart: number[];
  readonly earliestFinish: number[];
  readonly latestStart: number[];
  readonly latestFinish: number[];
  readonly slack: number[];
  readonly critical: boolean[];
  readonly makespan: number;
  readonly totalWork: number;
  readonly peakConcurrency: number;
}

/**
 * `pairs` are the dependency edges, each `[from, to]` with `from < to`. The
 * engine emits a covering graph; the oracle emits every conflicting pair.
 * Longest-path timing is identical on both, because every conflicting pair has
 * a path between it in the covering graph — which is exactly what the Rust
 * forward pass relies on, so the parity check proves that claim.
 */
export function referenceCpm(
  nodeCount: number,
  pairs: ReadonlyArray<readonly [number, number]>,
  durations: ReadonlyArray<number>,
): ReferenceTiming {
  if (durations.length !== nodeCount) {
    throw new Error("every node needs a duration");
  }
  const successors: number[][] = Array.from({ length: nodeCount }, () => []);
  const predecessors: number[][] = Array.from({ length: nodeCount }, () => []);
  const indegree = new Array<number>(nodeCount).fill(0);
  for (const [from, to] of pairs) {
    if (!(from < to) || to >= nodeCount) throw new Error(`bad edge ${from}->${to}`);
    if (successors[from]!.includes(to)) continue; // adjacency is a set
    successors[from]!.push(to);
    predecessors[to]!.push(from);
    indegree[to] = indegree[to]! + 1;
  }

  const earliestStart = new Array<number>(nodeCount).fill(0);
  const earliestFinish = new Array<number>(nodeCount).fill(0);
  const latestStart = new Array<number>(nodeCount).fill(0);
  const latestFinish = new Array<number>(nodeCount).fill(0);

  // Forward pass over a Kahn order.
  const queue: number[] = [];
  for (let node = 0; node < nodeCount; node++) if (indegree[node] === 0) queue.push(node);
  const order: number[] = [];
  const pending = [...indegree];
  for (let head = 0; head < queue.length; head++) {
    const node = queue[head]!;
    order.push(node);
    earliestFinish[node] = earliestStart[node]! + durations[node]!;
    for (const next of successors[node]!) {
      earliestStart[next] = Math.max(earliestStart[next]!, earliestFinish[node]!);
      pending[next] = pending[next]! - 1;
      if (pending[next] === 0) queue.push(next);
    }
  }
  if (order.length !== nodeCount) throw new Error("the reference graph has a cycle");

  const makespan = nodeCount === 0 ? 0 : Math.max(...earliestFinish);
  const totalWork = durations.reduce((sum, value) => sum + value, 0);

  // Backward pass over the reverse topological order.
  for (let index = order.length - 1; index >= 0; index--) {
    const node = order[index]!;
    latestFinish[node] =
      successors[node]!.length === 0
        ? makespan
        : Math.min(...successors[node]!.map((next) => latestStart[next]!));
    latestStart[node] = latestFinish[node]! - durations[node]!;
  }

  const slack = earliestStart.map((start, node) => latestStart[node]! - start);
  const critical = slack.map((value) => value === 0);

  // Peak concurrency by ticking through the time axis with a counter map:
  // half-open intervals, so a finish at t and a start at t do not overlap.
  let peakConcurrency = 0;
  if (nodeCount > 0) {
    const change = new Map<number, number>();
    for (let node = 0; node < nodeCount; node++) {
      change.set(earliestStart[node]!, (change.get(earliestStart[node]!) ?? 0) + 1);
      change.set(earliestFinish[node]!, (change.get(earliestFinish[node]!) ?? 0) - 1);
    }
    let active = 0;
    for (const tick of [...change.keys()].sort((a, b) => a - b)) {
      active += change.get(tick)!;
      peakConcurrency = Math.max(peakConcurrency, active);
    }
  }

  return {
    earliestStart,
    earliestFinish,
    latestStart,
    latestFinish,
    slack,
    critical,
    makespan,
    totalWork,
    peakConcurrency,
  };
}
