# Resource model

What a capacity resource is, what it is not, and what Phase 5 decides about it.

Phase 5 answers a question the first four phases could not: not *what must happen
before what* (Phase 1), nor *what may run together* (Phase 2), nor *how long that
takes* (Phase 3), nor *how to change it cheaply* (Phase 4) — but **what can happen
together given what is available.** An operation that is dependency-ready is not
therefore executable, and until now the planner had no way to know the difference.

This document is the M1 artifact. Its arithmetic and its claims about the existing
engine are checked against the source; the four open questions at the end are
decisions, not measurements, and are the gate this milestone closes on.

## 1. Two kinds of resource, one word too convenient

| | Dependency resource | Capacity resource |
| --- | --- | --- |
| Question it answers | who must wait for whom | how many may run at once |
| Purpose | correctness | feasibility |
| Mechanism | conflict detection on read/write | capacity accounting |
| Needs durations | no | yes |
| Changes the graph | yes | no |
| Lives in | `RES_SLOTS = MAX_RESOURCES + 1 = 1,025` lanes of `last_writer` and `reader_head`, indexed by the host's interned dependency id | its own index space `0..R`, its own arrays |
| Phase | 1 | 5 |

The separation is not a preference, it is structural: a capacity index and a
dependency index have no relationship to each other, and no mapping between them
exists in the engine. Merging the two models would mean inventing that mapping and
then answering questions like "does a write to record X contend for a GPU slot?" —
which has no answer, because they are different facts about different things. The
specification's warning about turning every infrastructure constraint into an
artificial dependency edge is exactly the failure this avoids: a fake edge says
"wait", a capacity constraint says "wait until something is free", and only one of
those is a property of the workload.

## 2. Capacity: one integer, and a unit that means nothing to the engine

Every capacity resource is a positive integer with a caller-chosen key and an
optional unit label. The engine performs integer arithmetic on capacities and
demands and never interprets a unit: `24576 MiB` and `24576 connections` are the
same number to it, and the same algorithm. Units are metadata that the host carries
and the artifact echoes back, nothing more. Specialised unit maths is not
implemented, deliberately: it would be a lot of surface area to get wrong for no
scheduling benefit.

Bounds proposed in `docs/RESOURCE-ABI.md`: 64 resources per batch, 1,024 demand
records, capacity up to 2³¹−1, all validated in Rust before anything is scheduled.

## 3. Access semantics, reduced to two mechanisms

The specification lists four modes — shared, exclusive, read, write — and
recommends against four scheduling algorithms, which is right. There are two
mechanisms:

```
SHARED    capacity = Q   demand = q        Q up to q operations may overlap
EXCLUSIVE capacity = 1   demand = 1        at most one operation at a time
```

Exclusive is not a special case of the accounting; it *is* the accounting with the
smallest possible numbers. That is worth stating because the tempting wrong
implementation is a second code path for locks, which would then need its own
bugs.

`read` and `write` in the capacity model are the *same mechanism* as shared and
exclusive, and they must not be confused with Phase 1's read/write **dependency**
modes, which mean something else entirely and stay where they are. An operation
can hold a dependency write on a record while sharing a connection slot; nothing
about one implies anything about the other.

**Occupancy is for the whole scheduled interval.** If an operation is scheduled
`[start, finish)`, it holds its demand for all of it. Short-lived acquisition
windows — holding a lock only across part of an operation — are deferred: they need
intra-operation phases, which the current operation model does not have, and they
would make the interval arithmetic a good deal more subtle for a benefit no caller
has asked for yet.

## 4. Intervals: inherited, not reinvented

Phase 3 measures concurrency over half-open intervals `[ES, EF)`: a call finishing
at tick 7 and a call starting at tick 7 are not concurrent. Phase 5's capacity check
sweeps the same kind of interval with the same convention, because otherwise the
two layers disagree about the same plan — the timing report would say two operations
do not overlap while the resource report says they contend for the same slot. The
engine already has this sweep working (`compute_peak_concurrency`, 2,048 bytes of
stack, heap-sorted endpoints); Phase 5 extends the same idea with amounts rather
than counts.

## 5. Four outcomes, and why they are not three

| Status | Meaning |
| --- | --- |
| `FEASIBLE` | a schedule was produced that respects every dependency and every capacity. It is not claimed to be optimal |
| `INFEASIBLE` | the declared demands cannot be met by the declared capacities — e.g. an operation asking for 3 of a resource whose capacity is 2 |
| `INVALID` | the input is malformed: unknown resource, duplicate key, zero capacity or demand, non-integer amount |
| `UNSUPPORTED` | the request is well-formed but outside the model — a semantics Phase 5 does not implement, said plainly instead of approximated |

Feasibility and optimality are different claims and must never be conflated. A
greedy scheduler that returns a valid 30-tick plan has not produced *the* 30-tick
plan, and nothing in the output should imply that it did. Conversely an infeasible
declaration is not "the schedule is slow" — it is "this cannot be run as asked",
and it is detected before scheduling rather than discovered as a mysterious delay.

## 6. Determinism, and the priority rules it constrains

The scheduler is a deterministic list scheduler: dependency readiness, then
resource availability, then a fixed ranking, then event-driven time advancement.
The ranking the specification proposes, in order:

1. greater remaining downstream path length
2. smaller slack
3. more downstream dependents
4. lower call index

Rules 2, 3 and 4 are computable from what the engine already publishes — Phase 3's
timing region carries `earliest_start` and `slack` per call, and the edge records
give dependents. Rule 1 is **not**: the longest path from a call to the end of the
project is a different quantity from CPM's latest-start times, which assume
unlimited capacity, and the engine does not currently compute it. It would need a
reverse pass over the graph and one more `u32` array of `MAX_CALLS` (512 bytes).
That is affordable and it is a real decision rather than an oversight, so it is
open as D3 below — and the specification's own hard question 3 ("how much better is
critical-path priority than FIFO?") can only be answered by measuring both.

Tie-breaking by original call index is what makes the whole thing reproducible
byte-for-byte, which is the property every phase has shipped with since Phase 1.

### What the heuristic costs, measured

The ranking that ships today is rules 2, 3 and 4; rule 1 is D3 and is not
implemented. To say anything at all about its quality, an independent reference
computes the optimal makespan by exhaustive search over precedence-feasible
permutations. That is the standard result — an optimal RCPSP schedule is
reachable by some serial schedule generation sequence — so the number is a lower
bound on what any heuristic can reach rather than a second guess to compare
guesses against. The reference shares nothing with the engine: it recomputes
usage tick by tick from the declarations, and it walks the graph itself.

Over 32 seeded instances of 3–8 calls and 1–3 capacity resources, every
schedule the engine produced was feasible, every dependency edge held, no
capacity was exceeded, and the same declarations always produced the same
bytes. 30 of the 32 matched the optimum exactly. Two were longer: by one tick,
and by three on an eight-call instance whose second lane had capacity one.

That is a bounded claim and it is worth stating precisely: *over that family,
the current ranking costs at most three ticks.* It is not a claim about larger
or differently shaped instances, and it is emphatically not a claim of
optimality. The gap is the thing to watch — it is the number that would move
first if the ranking changed.

The reference lives in `benchmarks/reference-resource-scheduler.ts` and the
comparison in `packages/core/test/resource-schedule.test.ts`. Neither is
shipped.

## 7. Requirements mapping

Requirements are given **per call, in the caller's own call order** — positional,
exactly like Phase 3's `durations` array — for `planBatch`. For a session, the same
demands are addressed by the caller's stable key, so a transaction that replaces a
call can change its demand without the caller recomputing a position. The rule the
specification states and this design follows: mapping is to *input call indexes*,
never to edge positions or topological positions, because those are outputs of the
planner and the caller does not know them in advance.

Resource keys never enter the arena. The host interns them into `0..R` (the same
discipline `encode.ts` already applies to dependency resources), the engine sees
integers, and every diagnostic carries a numeric index. Error messages name an
index and a rule, never a caller's identifier — the contract since Phase 1.

## 8. What Phase 5 does not do

It plans. It does not acquire, lock, monitor or execute. No dynamic infrastructure
discovery, no GPU detection, no distributed locking, no worker assignment, no
autoscaling, no runtime execution, no AI scheduling, and no global-optimal RCPSP
solver — the last because it is not achievable in a bounded-memory deterministic
kernel, and pretending otherwise would be the one way to make this library
untrustworthy. EffectPlan hands a plan to something else; that something else owns
the infrastructure.

## 9. Decisions

D1 and D4 were adopted when the owner said to proceed, and are implemented in M3.
D2 and D3 remain genuinely open: they are not measurements, and each changes what
a later module has to be. Recording an adopted decision is not the same as a
reviewed one — if D1 or D4 is wrong, the correction belongs here, and the engine
change that follows it.

**D1 — a new export, adopted and implemented.**
A `effectplan_plan_v5` would have taken the current twenty parameters plus
capacities, offsets and demands, in one call that always validates, plans the
graph, layers, times and schedules. The reason that is wrong is not tidiness:
§13.1 requires that a **scheduling-only** change — new durations, new capacities,
new demands, no change to any operation's access declarations — must not rebuild
the dependency graph, and one call that always does both cannot express that rule
at all. `effectplan_resource_check` therefore takes twelve parameters, none of them
a planning region, and a test asserts it leaves the graph, levels, durations,
timing and checkpoint bytes untouched. The stateless API will call both in sequence
so ordinary callers still see one method (M6).

**D4 — capacity keys interned by the host, adopted.**
Keys never enter the arena; the host maps them to `0..R`, the same discipline
`encode.ts` has applied to dependency resources since Phase 1. Requirements are
positional per call for `planBatch` — exactly like `durations` — and keyed by
caller key inside a session.

**D2 — open. How a session carries resource declarations across versions.**
A Phase 4 snapshot is immutable and self-contained. Resource declarations belong to
the session, not the snapshot, but the schedule inside the snapshot was computed
from them. Two invalidation domains then have to be tracked independently —
dependency invalidation (access declarations change, checkpointed prefix may be
invalid) and scheduling invalidation (durations/capacities/demands change, graph
untouched, checkpoint fully valid) — and a change can belong to both. Recommendation:
session-level resource table with its own version, scheduling-only transactions
skipping the dependency pass entirely, and the differential test extended so
scheduling-only edits are compared against a full rebuild like everything else.

**D3 — open. Does the engine compute longest remaining path (rule 1 of §6)?**
Cost: one reverse pass and 512 bytes. Benefit: unknown until measured — it is the
difference between "prioritise what the critical path needs" and "prioritise what
has the least slack", which are not the same ranking, and the specification's own
benchmark question depends on which is better. Recommendation: implement slack-based
ranking first (the data already exists), benchmark it against FIFO and against the
path-based variant, and let the measurement decide — the same way the Phase 4
interval default got settled.

D2 has to be answered before M7, because it decides whether the session grows a
second table or reuses the checkpoint machinery. D3 can wait for the benchmark that
answers it, which is the right order: the ranking rule is a performance question
and should be settled by a measurement, not by an opinion about schedulers.

## 10. Related documents

* `docs/RESOURCE-ABI.md` — the measured memory budget and region layout.
* `docs/RESOURCE-SCHEDULING.md` — the scheduler once it exists.
* `docs/INCREMENTAL-PLANNING.md` — the session the resource table joins.
* `docs/CRITICAL-PATH.md` — where slack and the half-open interval convention come from.
