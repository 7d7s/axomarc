# Resource scheduling

How a batch is placed against declared capacity, and what comes back.

This is the Phase 5 placement surface: the engine's `effectplan_resource_schedule`
export, the public TypeScript methods that call it, and the promises both of them
make. The memory layout is in `docs/RESOURCE-ABI.md`; the semantics — what a
capacity resource is, how access modes are reduced, why the two resource models
are never merged — are in `docs/RESOURCE-MODEL.md`.

## 1. The shape of the call

A capacity resource is a number: how many units of it may be held at once. The
engine does not know what a unit means. A call declares what it holds, and for
how long: a call that holds two units of a lane with capacity four takes a
quarter of the lane for the whole of its execution, and gives it back at the
end — not at the start, and not in pieces.

```ts
const result = plan.planResources(calls, {
  capacities: [{ key: "gpu", capacity: 2 }],
  demands: [
    [{ resource: "gpu", amount: 1 }],
    [{ resource: "gpu", amount: 2 }],
    [],
  ],
  durations: [1200, 900, 300],
  unit: "ms",
});

result.outcome;      // "feasible" | "infeasible"
result.schedule;     // one row per call, in batch order: start, finish
result.statistics;   // per resource: capacity, peak, busy time
result.diagnostics;  // why anything was refused, by call and by resource
result.plan;         // the dependency plan: edges, levels, timing
```

The demands are positional, exactly like the Phase 3 `durations` array: one list
per call, in the caller's own call order. Capacity keys never cross the ABI.
The host interns them into `0..R` — the same discipline dependency resources
have followed since Phase 1 — so the engine sees integers and a diagnostic can
never leak a caller's identifier. The wrapper resolves the integers back to the
caller's own keys, because that is the caller's own data.

## 2. The four outcomes, and where they surface

| Outcome | What it means | How it appears |
| --- | --- | --- |
| feasible | a schedule was produced that respects every dependency and every capacity | `outcome: "feasible"` |
| infeasible | the demands cannot be met by the declared capacities | `outcome: "infeasible"` with diagnostics |
| invalid | the input is malformed — a zero capacity, a demand naming a resource nobody declared, a non-integer amount | `EffectPlanError` |
| unsupported | well-formed but outside the model | `EffectPlanError` with `EP_LIMIT_EXCEEDED` |

An infeasible batch is an *answer*, not a fault: the wrapper returns it rather
than throwing, because "this cannot run as asked" is the most useful thing a
planner can say. An invalid declaration is the caller's mistake and is raised
as an error, before the engine is asked anything.

The two are also different in kind. An infeasible demand is detected *before*
scheduling — a call that can never fit is not a scheduling delay, and pretending
otherwise would turn a contradiction into a mysterious wait. The engine's check
runs first, and only its verdict may reach the placement.

## 3. What the scheduler does

Event-driven list scheduling, with the deterministic ranking the model
prescribes:

1. greater remaining downstream path length — **not implemented** (D3, open)
2. smaller slack
3. more downstream dependents
4. lower call index

The rule the engine applies today is therefore rules 2, 3 and 4, in that order,
with the call index as the final tie-break. Tie-breaking by index is what makes
the whole thing reproducible byte-for-byte.

The loop is: take the calls whose dependencies have finished, rank them, and try
to place each one at the current tick. A call that fits is placed and holds its
capacity until it finishes. A call that does not fit is deferred, and the clock
advances to the next event — a completion, or the earliest tick at which
something else could start. Reservation is atomic: every demand of a call is
checked before any of them is applied, so there is no half-held capacity to
leak and nothing to unwind.

The half-open interval convention is inherited verbatim from Phase 3: a call
finishing at tick 7 and a call starting at tick 7 do not overlap. The two layers
use the same convention on purpose — if they disagreed, the timing report and
the resource report would contradict each other about the same plan.

Termination is structural rather than hoped for. Every round places a call or
advances the clock to a completion; if neither is possible the batch is reported
unschedulable with the blocked call named by index. A round budget of `4V + 8`
backs that argument against a reasoning error: the loop ends either way.

## 4. What comes back

**The schedule** is one row per call, in batch order: the start tick and the
finish tick. The rows are in batch order rather than in placement order, so the
placement order is read off the start times.

**The statistics** are per declared resource: the capacity, the highest
simultaneous demand observed (`peak`), and the busy time. Busy time is
`amount × duration` summed over the calls that held the resource, in the
caller's unit — the weighted measure, which stays inside `[0, capacity ×
makespan]` where a wall-clock measure would not. It is a `bigint`, because the
engine stores it as `u64` and a `number` would lose precision above 2^53
without saying so. The engine does not divide: the utilisation ratio is
presentation and belongs to the host.

**The diagnostics** are the engine's own records, translated back into the
caller's keys: an insufficient capacity, an undeclared resource, a zero
capacity or demand, a duplicate demand, an out-of-range value, a contention
event — the call that had to wait, the resource it waited for and the tick it
waited at — and an unschedulable batch.

What is *not* reported: a per-resource count of how many calls waited for it
(the contention record names the waiting call, not a tally per resource), and any
claim of optimality. The gap to the optimum is measured in
`docs/RESOURCE-MODEL.md` §6 and it is not zero in general.

## 5. The separation from planning

`effectplan_resource_schedule` reads the planner's regions and writes none of
them. That separation is the point: a change to a duration, a capacity or a
demand can be re-placed without rebuilding the dependency graph, which is what
the specification's scheduling-invalidation rule requires and what a single
all-in-one call cannot express. A test captures the graph and the timing bytes
after planning and compares them after placing.

The public `planResources` method calls the planner and then the placement, in
that order, so ordinary callers see one method. `checkResources` calls the check
export alone — no graph, no clock — for the question "can this batch run at
all?".

## 6. The limits

| Quantity | Limit | Where it comes from |
| --- | --- | --- |
| calls per batch | 128 | fixed since Phase 1 |
| capacity resources | 64 | the M2 budget, measured against one memory page |
| demand records | 1,024 | the M2 budget |
| diagnostics per run | 64 | past the ceiling the list stops growing and the status still says what was wrong |
| capacity or demand amount | 2,147,483,647 | the engine's arithmetic |

Exceeding the resource ceilings is `EP_LIMIT_EXCEEDED`, not a truncation: a
batch that does not fit is refused rather than quietly fitted.

## 7. What it does not do

It plans. It does not acquire, lock, monitor or execute; it does not discover
infrastructure, detect GPUs, assign workers, autoscale, or run anything. The
schedule it produces is a proposal under the declared model, and something else
owns the infrastructure it names.
