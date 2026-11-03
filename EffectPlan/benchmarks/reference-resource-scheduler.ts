/**
 * An independent reference for Phase 5 resource scheduling.
 *
 * Deliberately written to share nothing with the engine: no event queue, no
 * incremental usage lanes, no half-open endpoint sort, no reuse of a ranking. The
 * checker walks every tick and recomputes every usage from the declared demands,
 * which is O(makespan x calls x demands) and embarrassingly slow, and is exactly
 * why it can be trusted to disagree with a clever implementation. The solver is a
 * serial schedule generation scheme run over every precedence-feasible permutation
 * — the standard exhaustive result that an optimal RCPSP schedule is reachable by
 * some such sequence — so its makespan is a true lower bound on what any heuristic
 * can produce, not another guess to compare guesses against.
 *
 * This file is not shipped. It exists so `packages/core/test/resource-schedule.test.ts`
 * can answer a question the engine cannot answer about itself: is the schedule it
 * returns actually feasible, and how far from optimal is it?
 */

export interface ResourceInstance {
  /** One entry per call. `demands` are (capacity resource index, amount). */
  readonly calls: readonly {
    readonly duration: number;
    readonly demands: readonly (readonly [number, number])[];
  }[];
  /** Dependency edges as (predecessor, successor), predecessor first. */
  readonly edges: readonly (readonly [number, number])[];
  readonly capacities: readonly number[];
}

export interface Schedule {
  /** Per call: start and finish, in ticks. */
  readonly windows: readonly (readonly [number, number])[];
  readonly makespan: number;
}

export interface Violation {
  readonly kind: "duration" | "edge" | "capacity" | "shape" | "unplaced" | "negative" | "stats";
  readonly detail: string;
}

/**
 * Check a schedule against an instance, from first principles.
 *
 * Returns the empty list only when every duration holds, every edge holds, and
 * the summed demand of the calls alive at each tick fits each capacity. Ticks are
 * integers here, which is what the engine's own model is: durations and starts are
 * integer ticks, so walking them one by one is exact rather than an approximation
 * of a continuous interval.
 */
export function verifySchedule(instance: ResourceInstance, schedule: Schedule): Violation[] {
  const problems: Violation[] = [];
  const callCount = instance.calls.length;
  if (schedule.windows.length !== callCount) {
    problems.push({ kind: "shape", detail: `${schedule.windows.length} windows for ${callCount} calls` });
    return problems;
  }

  for (let index = 0; index < callCount; index++) {
    const [start, finish] = schedule.windows[index]!;
    const duration = instance.calls[index]!.duration;
    if (start < 0 || finish < 0) {
      problems.push({ kind: "negative", detail: `call ${index} starts at ${start}` });
      continue;
    }
    if (finish - start !== duration) {
      problems.push({
        kind: "duration",
        detail: `call ${index}: [${start}, ${finish}) spans ${finish - start}, duration is ${duration}`,
      });
    }
    if (duration > 0 && finish === start) {
      problems.push({ kind: "unplaced", detail: `call ${index} is unplaced` });
    }
  }

  for (const [from, to] of instance.edges) {
    const predecessor = schedule.windows[from];
    const successor = schedule.windows[to];
    if (!predecessor || !successor) continue;
    if (successor[0] < predecessor[1]) {
      problems.push({
        kind: "edge",
        detail: `edge ${from} -> ${to}: successor starts ${successor[0]}, predecessor finishes ${predecessor[1]}`,
      });
    }
  }

  // The capacity condition, tick by tick, computed from the declarations each
  // time. No interval arithmetic, no sorting, no shared state.
  for (let tick = 0; tick < schedule.makespan; tick++) {
    const live = instance.capacities.map(() => 0);
    for (let index = 0; index < callCount; index++) {
      const window = schedule.windows[index]!;
      if (window[0] <= tick && tick < window[1]) {
        for (const [resource, amount] of instance.calls[index]!.demands) {
          live[resource] = (live[resource] ?? 0) + amount;
        }
      }
    }
    for (let resource = 0; resource < instance.capacities.length; resource++) {
      if ((live[resource] ?? 0) > instance.capacities[resource]!) {
        problems.push({
          kind: "capacity",
          detail: `tick ${tick}: resource ${resource} holds ${live[resource]} of ${instance.capacities[resource]}`,
        });
      }
    }
  }

  const observedSpan = schedule.windows.reduce((max, [, finish]) => Math.max(max, finish), 0);
  if (observedSpan !== schedule.makespan) {
    problems.push({
      kind: "shape",
      detail: `makespan ${schedule.makespan} but the last call ends at ${observedSpan}`,
    });
  }
  return problems;
}

/**
 * Per-resource statistics as the engine publishes them: the declared capacity,
 * the highest simultaneous demand observed, and the busy time.
 */
export interface ResourceStats {
  readonly capacity: number;
  readonly peak: number;
  readonly busy: bigint;
}

/**
 * Check the published statistics against the instance and the schedule.
 *
 * The peak is recomputed by walking the ticks and counting what is alive, and
 * the busy time by summing `amount × duration` over the calls that hold the
 * resource — the same two definitions the engine documents, derived here from
 * the declarations rather than from anything the engine computed. Busy time is
 * compared as a `bigint` because the engine stores it as `u64` and a `number`
 * would quietly lose precision above 2^53.
 */
export function verifyStats(
  instance: ResourceInstance,
  schedule: Schedule,
  published: readonly ResourceStats[],
): Violation[] {
  const problems: Violation[] = [];
  if (published.length !== instance.capacities.length) {
    problems.push({
      kind: "shape",
      detail: `${published.length} stat rows for ${instance.capacities.length} resources`,
    });
    return problems;
  }

  for (let resource = 0; resource < instance.capacities.length; resource++) {
    const row = published[resource]!;
    const capacity = instance.capacities[resource]!;
    if (row.capacity !== capacity) {
      problems.push({
        kind: "stats",
        detail: `resource ${resource}: capacity published ${row.capacity}, declared ${capacity}`,
      });
    }

    let peak = 0;
    for (let tick = 0; tick < schedule.makespan; tick++) {
      let live = 0;
      for (let index = 0; index < instance.calls.length; index++) {
        const window = schedule.windows[index]!;
        if (window[0] <= tick && tick < window[1]) {
          for (const [held, amount] of instance.calls[index]!.demands) {
            if (held === resource) live += amount;
          }
        }
      }
      if (live > peak) peak = live;
    }
    if (row.peak !== peak) {
      problems.push({
        kind: "stats",
        detail: `resource ${resource}: peak published ${row.peak}, observed ${peak}`,
      });
    }

    let busy = 0n;
    for (let index = 0; index < instance.calls.length; index++) {
      const entry = instance.calls[index]!;
      for (const [held, amount] of entry.demands) {
        if (held === resource) busy += BigInt(amount) * BigInt(entry.duration);
      }
    }
    if (row.busy !== busy) {
      problems.push({
        kind: "stats",
        detail: `resource ${resource}: busy published ${row.busy}, expected ${busy}`,
      });
    }
  }
  return problems;
}

/**
 * The optimal makespan, by exhaustive search over precedence-feasible sequences.
 *
 * Exponential: keep the instances small. The sequence is applied through a serial
 * schedule generation scheme — take the calls in the given order, start each one
 * at the earliest tick where its predecessors have finished *and* every resource
 * it needs is free for its whole length. That yields a feasible schedule for each
 * order, and the best over all orders is optimal.
 */
export function optimalMakespan(instance: ResourceInstance): number {
  const callCount = instance.calls.length;
  const predecessors: number[][] = instance.calls.map(() => []);
  for (const [from, to] of instance.edges) predecessors[to]!.push(from);

  let best = Number.POSITIVE_INFINITY;
  const placed: (number | undefined)[] = new Array<number | undefined>(callCount);

  /** Earliest finish for a call at `start`, or undefined if it cannot fit there. */
  const fits = (index: number, start: number): number | undefined => {
    const entry = instance.calls[index]!;
    const finish = start + entry.duration;
    for (const [resource, amount] of entry.demands) {
      const held = usageAt(instance, placed, index, resource, start, finish);
      if (held + amount > instance.capacities[resource]!) return undefined;
    }
    return finish;
  };

  const search = (remaining: number[]): void => {
    if (remaining.length === 0) {
      let span = 0;
      for (const window of placed) span = Math.max(span, window ?? 0);
      if (span < best) best = span;
      return;
    }
    for (let position = 0; position < remaining.length; position++) {
      const index = remaining[position]!;
      let earliest = 0;
      for (const predecessor of predecessors[index]!) {
        const finish = placed[predecessor];
        if (finish === undefined) {
          earliest = Number.POSITIVE_INFINITY;
          break;
        }
        earliest = Math.max(earliest, finish);
      }
      if (earliest === Number.POSITIVE_INFINITY) continue; // predecessor not placed yet
      if (earliest >= best) continue; // this call cannot improve on what we have

      let start = earliest;
      for (;;) {
        const finish = fits(index, start);
        if (finish !== undefined) {
          placed[index] = finish;
          search([...remaining.slice(0, position), ...remaining.slice(position + 1)]);
          placed[index] = undefined;
          break;
        }
        start += 1;
        if (start >= best) break;
      }
    }
  };

  search(Array.from({ length: callCount }, (_, index) => index));
  return best === Number.POSITIVE_INFINITY ? 0 : best;
}

/** Total demand already held on one resource across a call's whole interval. */
function usageAt(
  instance: ResourceInstance,
  placed: (number | undefined)[],
  except: number,
  resource: number,
  start: number,
  finish: number,
): number {
  let total = 0;
  for (let index = 0; index < instance.calls.length; index++) {
    if (index === except) continue;
    const window = placed[index];
    if (window === undefined) continue;
    // The serial scheme places calls in a sequence and never moves them, so a
    // call's start is not retained here; recomputing it is what makes this
    // reference slow, and deliberately so.
    const entry = instance.calls[index]!;
    const begin = window - entry.duration;
    if (begin < finish && start < window) {
      for (const [heldResource, amount] of entry.demands) {
        if (heldResource === resource) total += amount;
      }
    }
  }
  return total;
}

/**
 * What a schedule is worth, computed from the declarations alone.
 *
 * Deliberately a second implementation of the same three numbers the engine
 * publishes: earliest starts derived by a longest-path pass over the edges, the
 * workload bound derived by summing `amount x duration` per resource, and
 * waiting derived by comparing the schedule against those earliest starts. The
 * engine has its own versions in `engine/src/quality.rs`; the point of two
 * independent computations is that agreement is evidence and a shared mistake is
 * not.
 */
export interface ExpectedQuality {
  readonly makespan: number;
  readonly dependencyLowerBound: number;
  readonly resourceLowerBound: number;
  readonly lowerBound: number;
  readonly provablyOptimal: boolean;
  readonly totalResourceWaiting: number;
}

export function expectedQuality(
  instance: ResourceInstance,
  schedule: Schedule,
): ExpectedQuality {
  const count = instance.calls.length;
  const predecessors: number[][] = Array.from({ length: count }, () => []);
  for (const [from, to] of instance.edges) predecessors[to]!.push(from);

  // Earliest start by longest path. Index order is a valid topological order for
  // this corpus because every edge points forward; iterating `count` times makes
  // that assumption unnecessary rather than merely true.
  const es = new Array<number>(count).fill(0);
  for (let pass = 0; pass < count; pass++) {
    for (let i = 0; i < count; i++) {
      for (const p of predecessors[i]!) {
        es[i] = Math.max(es[i]!, es[p]! + instance.calls[p]!.duration);
      }
    }
  }
  const dependencyLowerBound = es.reduce(
    (max, start, i) => Math.max(max, start + instance.calls[i]!.duration),
    0,
  );

  const load = instance.capacities.map(() => 0);
  for (let i = 0; i < count; i++) {
    for (const [resource, amount] of instance.calls[i]!.demands) {
      load[resource] = (load[resource] ?? 0) + amount * instance.calls[i]!.duration;
    }
  }
  let resourceLowerBound = 0;
  instance.capacities.forEach((capacity, resource) => {
    if (capacity <= 0) return;
    const needed = Math.ceil((load[resource] ?? 0) / capacity);
    if (needed > resourceLowerBound) resourceLowerBound = needed;
  });

  let waiting = 0;
  for (let i = 0; i < count; i++) {
    const window = schedule.windows[i];
    if (window === undefined) continue;
    waiting += Math.max(0, window[0] - es[i]!);
  }

  const lowerBound = Math.max(dependencyLowerBound, resourceLowerBound);
  return {
    makespan: schedule.makespan,
    dependencyLowerBound,
    resourceLowerBound,
    lowerBound,
    provablyOptimal: schedule.makespan === lowerBound,
    totalResourceWaiting: waiting,
  };
}

/**
 * A small deterministic pseudo-random generator, so a randomised test is
 * reproducible from its seed rather than from the machine's entropy.
 */
export function seeded(seed: number): () => number {
  let state = seed >>> 0 || 1;
  return () => {
    // xorshift32: 32 bits of state, no allocation, same output on every platform.
    state ^= state << 13;
    state >>>= 0;
    state ^= state >>> 17;
    state ^= state << 5;
    state >>>= 0;
    return state / 0x1_0000_0000;
  };
}
