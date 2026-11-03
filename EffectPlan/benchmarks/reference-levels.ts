/**
 * Independent execution-level oracle. Development only: never bundled.
 *
 * The production engine derives levels with a single forward pass over its
 * covering graph. This oracle derives them a deliberately different way: a
 * Kahn-style layered traversal over the full all-pairs conflict set produced
 * by reference-planner.ts. Two independent methods, one mathematical answer.
 */

/**
 * Longest-path levels over the given DAG edges, by Kahn's algorithm.
 *
 * `pairs` are `[from, to]` edges with `from < to`. The result is
 * `level[v] = 0` for a root, `1 + max(level[u])` for every other node — the
 * same definition the production engine implements, reached through a queue
 * instead of a forward scan.
 */
export function referenceLevels(
  nodeCount: number,
  pairs: ReadonlyArray<readonly [number, number]>,
): number[] {
  const successors: number[][] = Array.from({ length: nodeCount }, () => []);
  const indegree = new Array<number>(nodeCount).fill(0);
  for (const [from, to] of pairs) {
    if (!(from < to) || to >= nodeCount) {
      throw new Error(`oracle edge ${from}->${to} is not forward-pointing`);
    }
    successors[from]!.push(to);
    indegree[to] = indegree[to]! + 1;
  }

  const levels = new Array<number>(nodeCount).fill(0);
  // Roots first, in ascending order: the queue order affects nothing but the
  // speed, because a node is only released once every predecessor has relaxed
  // its level.
  const queue: number[] = [];
  for (let node = 0; node < nodeCount; node++) {
    if (indegree[node] === 0) queue.push(node);
  }
  let head = 0;
  let processed = 0;
  while (head < queue.length) {
    const node = queue[head++]!;
    processed++;
    for (const next of successors[node]!) {
      const candidate = levels[node]! + 1;
      if (candidate > levels[next]!) levels[next] = candidate;
      indegree[next] = indegree[next]! - 1;
      if (indegree[next] === 0) queue.push(next);
    }
  }
  if (processed !== nodeCount) {
    throw new Error("the oracle graph has a cycle");
  }
  return levels;
}

/** Group a level array into waves: ascending levels, input order inside each. */
export function referenceWaves(levels: ReadonlyArray<number>): number[][] {
  const highest = levels.reduce((max, level) => (level > max ? level : max), -1);
  const waves: number[][] = Array.from({ length: highest + 1 }, () => []);
  levels.forEach((level, node) => waves[level]!.push(node));
  return waves;
}
