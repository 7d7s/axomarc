/**
 * The Phase 6 workload corpus, shared by everything that measures headroom.
 *
 * It lives here rather than inside one script because two consumers must agree
 * instance-for-instance: `optimization-headroom.ts` measures what the shipped
 * engine achieves, and a strategy prototype measures what a different priority
 * rule could achieve. Two private copies of a generator drift the moment one is
 * adjusted, and the drift would show up as a difference in results rather than as
 * a bug — which is the worst way to discover it.
 *
 * Generation is deterministic from the family index alone: each family seeds a
 * xorshift32 and draws in a fixed order, so the 1,200th instance is the same
 * instance on every machine and in every run.
 */

import type { ToolCall } from "../packages/core/dist/index.js";
import { seeded } from "./reference-resource-scheduler.js";

/** One dependency predecessor set per call: index i lists the calls that must finish first. */
export type EdgeRule = "none" | "chain" | "fanout" | "fanin" | "sparse";

export interface Spec {
  readonly name: string;
  readonly edges: EdgeRule;
  readonly lanes: number;
  /** Capacity of each lane. */
  readonly capacity: (lane: number, pick: () => number) => number;
  /** Whether a call holds anything of this lane. */
  readonly wants: (lane: number, pick: () => number) => boolean;
  /** How many units a call holds while it runs. */
  readonly amount: (capacity: number, pick: () => number) => number;
  readonly duration: (pick: () => number) => number;
  readonly note: string;
}

export const SPECS: readonly Spec[] = [
  {
    name: "independent",
    edges: "none",
    lanes: 1,
    capacity: () => 8,
    wants: (_, pick) => pick() < 0.8,
    amount: () => 1,
    duration: (pick) => 1 + Math.floor(pick() * 5),
    note: "no ordering and generous capacity: parallelism is already free",
  },
  {
    name: "chain",
    edges: "chain",
    lanes: 1,
    capacity: () => 4,
    wants: (_, pick) => pick() < 0.8,
    amount: () => 1,
    duration: (pick) => 1 + Math.floor(pick() * 5),
    note: "the order is forced by dependencies, so no schedule can reorder it",
  },
  {
    name: "fanout",
    edges: "fanout",
    lanes: 1,
    capacity: () => 3,
    wants: (_, pick) => pick() < 0.8,
    amount: () => 1,
    duration: (pick) => 1 + Math.floor(pick() * 5),
    note: "one root releases many ready calls: ready-set ordering is the only lever",
  },
  {
    name: "fanin",
    edges: "fanin",
    lanes: 1,
    capacity: () => 3,
    wants: (_, pick) => pick() < 0.8,
    amount: () => 1,
    duration: (pick) => 1 + Math.floor(pick() * 5),
    note: "many calls converge on one: the dependency bottleneck dominates",
  },
  {
    name: "exclusive",
    edges: "none",
    lanes: 1,
    capacity: () => 1,
    wants: () => true,
    amount: () => 1,
    duration: (pick) => 1 + Math.floor(pick() * 5),
    note: "one unit of capacity: every schedule is serial, so makespan is total work whatever the order",
  },
  {
    name: "shared",
    edges: "sparse",
    lanes: 2,
    capacity: () => 2,
    wants: (_, pick) => pick() < 0.7,
    amount: () => 1,
    duration: (pick) => 1 + Math.floor(pick() * 5),
    note: "two half-open lanes with freedom of order",
  },
  {
    name: "multiresource",
    edges: "sparse",
    lanes: 3,
    capacity: () => 3,
    wants: (_, pick) => pick() < 0.8,
    amount: (cap, pick) => 1 + Math.floor(pick() * cap),
    duration: (pick) => 1 + Math.floor(pick() * 5),
    note: "calls straddle several scarce lanes at once: placement is the only lever",
  },
  {
    name: "mixeddurations",
    edges: "sparse",
    lanes: 2,
    capacity: () => 2,
    wants: (_, pick) => pick() < 0.7,
    amount: () => 1,
    duration: (pick) => (pick() < 0.3 ? 6 + Math.floor(pick() * 4) : 1),
    note: "a few long calls among many short ones: priority can matter",
  },
  {
    name: "highutilization",
    edges: "none",
    lanes: 1,
    capacity: () => 2,
    wants: () => true,
    amount: () => 2,
    duration: (pick) => 1 + Math.floor(pick() * 4),
    note: "every call takes the whole lane: saturated and serial by construction",
  },
  {
    name: "sparseuse",
    edges: "none",
    lanes: 4,
    capacity: () => 4,
    wants: (_, pick) => pick() < 0.15,
    amount: () => 1,
    duration: (pick) => 1 + Math.floor(pick() * 5),
    note: "capacity rarely binds: measures optimization overhead against an already-free schedule",
  },
];

export const INSTANCES_PER_FAMILY = 120;
export const MIN_CALLS = 4;
export const MAX_CALLS = 7;

/** The seed for one family. Changing it changes the corpus, so it is stated here. */
export function familySeed(familyIndex: number): number {
  return 0x6e0_0000 + familyIndex * 104729;
}

export interface Instance {
  readonly durations: readonly number[];
  readonly capacities: readonly number[];
  /** `[lane, amount]` per call, in lane order. */
  readonly demands: readonly (readonly (readonly [number, number])[])[];
  readonly predecessors: readonly (readonly number[])[];
  /** Calls as the public API takes them: call i owns token d_i, a successor reads its parents'. */
  readonly calls: readonly ToolCall[];
  readonly capacitiesForApi: readonly { readonly key: string; readonly capacity: number }[];
  readonly demandsForApi: readonly (readonly { readonly resource: string; readonly amount: number }[])[];
}

export function predecessorsFor(rule: EdgeRule, callCount: number, pick: () => number): number[][] {
  const list: number[][] = [];
  for (let i = 0; i < callCount; i++) {
    if (rule === "none") list.push([]);
    else if (rule === "chain") list.push(i === 0 ? [] : [i - 1]);
    else if (rule === "fanout") list.push(i === 0 ? [] : [0]);
    else if (rule === "fanin") list.push(i === callCount - 1 ? [...Array(callCount - 1).keys()] : []);
    else list.push(i === 0 ? [] : [...Array(i).keys()].filter(() => pick() < 0.4));
  }
  return list;
}

/**
 * Draw one instance. The order of `pick()` calls below is part of the corpus
 * definition: durations, then capacities, then demands, then edges. Reordering
 * the draws silently renumbers every instance, which is why it is written once.
 */
export function buildInstance(spec: Spec, random: () => number): Instance {
  const callCount = MIN_CALLS + Math.floor(random() * (MAX_CALLS - MIN_CALLS + 1));
  const durations = Array.from({ length: callCount }, () => spec.duration(random));
  const capacities = Array.from({ length: spec.lanes }, (_, lane) => spec.capacity(lane, random));
  const demands = Array.from({ length: callCount }, () => {
    const held: [number, number][] = [];
    for (let lane = 0; lane < spec.lanes; lane++) {
      if (spec.wants(lane, random)) held.push([lane, spec.amount(capacities[lane]!, random)]);
    }
    // A call that wants nothing is still a call; leave it empty rather than
    // inventing a demand to fill the slot.
    return held;
  });
  const predecessors = predecessorsFor(spec.edges, callCount, random);

  // Dependency resources: call i writes token d_i, and a successor additionally
  // reads the token of everything it waits for. A call writing and reading its
  // own token would create no edge at all, which is the mistake this shape
  // exists to avoid.
  const calls = predecessors.map((parents, i) => ({
    id: `c${i}`,
    effect: {
      version: 1 as const,
      mode: "resources" as const,
      accesses: [
        { resource: { namespace: "headroom", id: `d${i}` }, mode: "write" as const },
        ...parents.map((parent) => ({
          resource: { namespace: "headroom", id: `d${parent}` },
          mode: "read" as const,
        })),
      ],
    },
  }));

  return {
    durations,
    capacities,
    demands,
    predecessors,
    calls,
    capacitiesForApi: capacities.map((q, lane) => ({ key: `r${lane}`, capacity: q })),
    demandsForApi: demands.map((held) => held.map(([lane, amount]) => ({ resource: `r${lane}`, amount }))),
  };
}

/**
 * Walk the corpus family by family, instance by instance. Each family gets its
 * own seed derived from its index, so a family can be reproduced alone and
 * adding a family at the end does not renumber the ones before it.
 */
export function* eachInstance(): Generator<{
  readonly family: Spec;
  readonly familyIndex: number;
  readonly index: number;
  readonly instance: Instance;
}> {
  for (const [familyIndex, spec] of SPECS.entries()) {
    const random = seeded(familySeed(familyIndex));
    for (let index = 0; index < INSTANCES_PER_FAMILY; index++) {
      yield { family: spec, familyIndex, index, instance: buildInstance(spec, random) };
    }
  }
}
