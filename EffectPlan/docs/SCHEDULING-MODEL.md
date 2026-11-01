# Scheduling model

What EffectPlan's timing numbers mean, and what they deliberately do not mean.
This is the mathematical contract. The engine that implements it is
[CRITICAL-PATH.md](CRITICAL-PATH.md), the byte layout is
[ABI.md](ABI.md).

## 1. What is being modelled

A batch of tool calls, each with a declared effect and a caller-supplied
duration, scheduled against unlimited capacity. The output is a *planning
estimate*: it answers "how long will this take if everything that can run, runs"
and "which calls decide that answer". It is not an execution engine, not a
resource allocator and not a forecast with confidence intervals.

The model is classical Critical Path Method (CPM) on a directed acyclic graph,
with integer arithmetic and no rounding anywhere.

## 2. Nodes and edges

A node is a **call in the batch**, identified by its index in the array the
caller passed. Node count equals call count, bounded by `maxCalls = 128`.

An edge `from -> to` is a *required ordering*: call `from` must finish before
call `to` may start. Edges come from resource conflicts the dependency pass
found, and the graph is a **covering graph**: it contains enough edges to imply
every required ordering, not every conflicting pair. If `0 -> 1 -> 2` is present,
the ordering `0` before `2` is implied and no direct edge is emitted.

Two properties are load-bearing for the scheduling passes and are established in
`docs/EXECUTION-LEVELS.md` and the tests, not assumed here:

* every edge points forward: `from < to`;
* records are emitted grouped by successor, in ascending successor order.

Together they make the graph acyclic by construction and let both CPM passes walk
one array without adjacency lists.

## 3. Durations

* A duration is a count of **integer ticks**. There is no floating point and no
  unit conversion: the caller's `unit` string is metadata carried to the result so
  the numbers can be read back, and the engine never looks at it.
* A duration must be a positive integer, `1 ≤ d ≤ 4,294,967,295`. Zero is refused
  (`EP_INVALID_INPUT`, engine status 118). A missing entry is refused, never
  treated as an instant operation.
* Exactly one duration per call, in batch order. An array of the wrong length is
  refused before anything is encoded.
* Sums are checked. If a chain's ticks would pass `2^32 − 1` the engine refuses the
  schedule (status 121) instead of wrapping into a plausible smaller number.

## 4. The four times and slack

For a call `i` with duration `d(i)`, predecessors `pred(i)` and successors
`succ(i)`:

```
earliestStart(i)  = 0                                if pred(i) is empty
                = max over p in pred(i) of  ES(p) + d(p)

earliestFinish(i) = earliestStart(i) + d(i)

latestFinish(i)   = makespan                         if succ(i) is empty
                = min over s in succ(i) of  LS(s)

latestStart(i)    = latestFinish(i) - d(i)

slack(i)          = latestStart(i) - earliestStart(i)
```

`makespan = max over all i of EF(i)`.

The engine stores two numbers per call — `earliestStart` and `slack` — and the
host derives `EF = ES + d`, `LS = ES + slack`, `LF = LS + d` from the durations it
already supplied. Two whole arrays stay out of the arena this way, and the
derived values cannot disagree with the stored ones because they are arithmetic on
them, not a second measurement.

Because `EF(p) ≤ ES(s)` holds for every edge, `LS(i) ≥ ES(i)` always: **slack is
never negative**. A schedule in which it was is refused rather than reported.

## 5. Criticality

A call is critical when `slack(i) == 0`, meaning any delay to it delays the whole
plan. Critical calls are reported as the complete zero-slack set, in batch order.

Paths are never enumerated. There can be exponentially many critical paths; there
is always exactly one critical *set*, and it is what a caller acts on. A plan can
have several disjoint critical paths — all of their calls appear.

Two corollaries worth stating because they surprise people:

* The critical set is not necessarily a chain. Two critical calls can be
  independent of each other and still both decide the makespan.
* Every sink with the maximum finish time is critical, and a plan whose only
  critical call is a single long operation is normal, not an error.

## 6. Work and concurrency

```
totalWork           = Σ d(i)                       (u64; 128 × 2^32-1 fits)
averageParallelism  = totalWork / makespan         (null when makespan is 0)
peakConcurrency     = the most calls alive at one tick, in the earliest-start
                      schedule
```

Intervals are **half-open**: `[ES(i), EF(i))`. A call finishing at tick 7 and a
call starting at tick 7 do not overlap and do not count as concurrent. This is the
usual convention for tasks that release their capacity at completion, and it is
the one the engine implements:

```
A: [0, 2)   B: [2, 5)   ->  peak 1     A: [0, 2)   B: [1, 5)   ->  peak 2
```

`averageParallelism` is a ratio, so it is fractional and is left as a division the
caller can format; `peakConcurrency` is an integer count of calls, never a
fraction, and is bounded by the call count.

`peakConcurrency` is measured on the earliest-start schedule. The latest-start
schedule can have a different envelope; the model reports the ASAP one because
that is the schedule a caller would run if it had capacity for everything.

## 7. Worked example

The specification's example is five calls with durations `A = 2`, `B = 5`,
`C = 2`, `D = 3`, `E = 4`, where `A` writes a resource, `B` and `C` read it, `D`
writes it again and `E` reads it. That ordering is the diamond `A -> {B, C} -> D`
plus `D -> E`:

| call | d | ES | EF | LS | LF | slack | critical |
| --- | --- | --- | --- | --- | --- | --- | --- |
| A | 2 | 0 | 2 | 0 | 2 | 0 | yes |
| B | 5 | 2 | 7 | 2 | 7 | 0 | yes |
| C | 2 | 2 | 4 | 5 | 7 | 3 | no |
| D | 3 | 7 | 10 | 7 | 10 | 0 | yes |
| E | 4 | 10 | 14 | 10 | 14 | 0 | yes |

Makespan 14, total work 16, average parallelism 8/7, peak concurrency 2, critical
set `{A, B, D, E}` — the path `A -> B -> D -> E`, while `C` may slip three ticks.

The specification's prose states this example's makespan as 10 with the critical
path `A -> B -> D`. That is the answer for the same durations on a graph where
`E` is *not* downstream of `D` — `E` touching nothing:

| call | d | ES | EF | LS | LF | slack | critical |
| --- | --- | --- | --- | --- | --- | --- | --- |
| A | 2 | 0 | 2 | 0 | 2 | 0 | yes |
| B | 5 | 2 | 7 | 2 | 7 | 0 | yes |
| C | 2 | 2 | 4 | 5 | 7 | 3 | no |
| D | 3 | 7 | 10 | 7 | 10 | 0 | yes |
| E | 4 | 0 | 4 | 6 | 10 | 6 | no |

Makespan 10, total work 16, peak concurrency 3 (`B`, `C` and `E` are all alive at
tick 2), average parallelism 1.6, critical set `{A, B, D}`.

Both tables are correct CPM for their own graph; the prose names the durations and
the path but not `E`'s edges, so the reading is ambiguous rather than wrong. The
engine implements the model, not the arithmetic of a sentence, and both shapes are
executed as tests — `produces the worked example from the specification` and
`produces the specification's example with E outside the chain` in
`packages/core/test/timing.test.ts`. This document records the difference instead
of picking a favourite.

## 8. What is deliberately not modelled

| Not modelled | Why not | Where it belongs |
| --- | --- | --- |
| Resource capacity and contention | Durations are per call; the engine has no machine pool, and a capacity-constrained schedule is a different algorithm (and a different can of worms) | Phase 5 |
| Calendars, working hours, holidays | A planning kernel should not encode a business calendar | caller |
| Ramp-up, interruption, preemption | Each call is one uninterrupted block of work | caller |
| Lag and lead on an edge (`FS+n`) | The dependency graph carries no lag today; adding it changes the dependency pass first | future, if proven necessary |
| Probabilistic durations (PERT) | Needs three durations per call and a distribution assumption; it is a forecast, not a schedule | out of scope |
| Machine assignment or ordering within a wave | EffectPlan says what may run together, not what will | Phase 4+ |

The honest summary: these numbers are correct **given the dependency graph and the
durations**. Both are the caller's claims. A plan whose durations are guesses
produces a makespan that is a guess, propagated exactly.
