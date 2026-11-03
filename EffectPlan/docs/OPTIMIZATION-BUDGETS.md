# Optimization budgets

Phase 6 M2: the worst-case memory and overflow envelope, computed **before** any
optimization code exists.

The M2 exit gate is explicit about this order — a worst-case budget calculated
first, because the alternative is discovering after six strategies are written
that the arena cannot hold them. Every number below comes from the shipped layout
constants and arithmetic, reproduced by the snippet at the end of this document,
not from a table anyone typed.

## The unit being optimized is the call, not the event

The engine's graph has 128 call slots (`MAX_CALLS`) and its planner internally uses
1,152 event slots (`MAX_EVENTS = MAX_CALLS + MAX_ACCESS`). A schedule is a
`(start, finish)` pair **per call**, which is what `effectplan_resource_schedule`
emits and what a caller acts on. Optimizing at event granularity would triple the
working set to buy precision nobody consumes:

| Buffer set | Per unit | Units | Cost |
| --- | --- | --- | --- |
| windows at call granularity (chosen) | 8 B | 128 | 1,024 B |
| windows at event granularity (rejected) | 8 B | 1,152 | 9,216 B |

## Worst-case working set

With the incumbent-plus-single-working-buffer scheme §10 of the Phase 6 planning
document requires:

| Bytes | Region |
| --- | --- |
| 1,056 | baseline copy, kept immutable so a failed optimization cannot corrupt it |
| 1,056 | incumbent windows |
| 1,056 | one reusable working candidate |
| 512 | remaining critical path `CP_i` — this is exactly decision D3 |
| 20 | strategy table, 5 entries |
| 20 | objective accumulators (makespan, waiting, idle) |
| 12 | lower bounds: `LB_dep`, `LB_res`, combined |
| 32 | optimization header and counters |
| 80 | per-strategy report, 5 rows |
| **3,848** | **aligned total** |

Against the address space that is actually free:

| Quantity | Bytes |
| --- | --- |
| linear memory, 18 pages | 1,179,648 |
| arena end | 1,125,928 |
| **spare address space** | **53,720** |
| Phase 6 worst-case working set | 3,848 |
| **share of spare used** | **7.2%** |

**Phase 6 fits in the 18th page.** No page growth is requested here, and none may
be taken without a measurement and explicit owner approval — the same rule that
governed Phase 5's page, which is why that approval is recorded in
`docs/RESOURCE-ABI.md` §1 rather than assumed.

## The candidate-storage rule is load-bearing, not stylistic

Storing all evaluated candidates — the obvious first implementation — costs
`64 × 1,056 = 67,584` bytes against 53,720 of spare. **It does not fit.** The
incumbent-plus-one-working-buffer scheme is therefore not a performance
preference; it is the difference between a design that can be shipped inside the
memory the artifact already reserves and one that cannot be shipped at all. That
is the concrete content of §10's `O(C × V) → O(V)` argument, and it is worth
noting that the RFC was right about it for a reason it did not state.

An edge-adjacency table for faster local search would cost `2,304 × 4 = 9,216`
bytes and still fit (13,064 total, 24% of spare). It is not recommended: 64
candidates scanning 2,304 edges is about 147k edge visits, which is cheap, so this
is a case where buying CPU with bytes solves a problem the engine does not have.
The budget is stated anyway so that if measurement later says otherwise, the number
is already known.

## Tick-domain arrays are forbidden

Makespan can reach 4,294,967,295 ticks, so **any** per-tick structure is unbounded
by construction — an array of occupancy per tick is not a fixed-capacity buffer and
cannot appear in the production core. Every Phase 6 metric must therefore be
derivable in closed form or by event sweep:

```
waiting   = Σ_i (S_i − ES_i)                          over calls, not ticks
idle_r    = Q_r × T − busy_r                          per resource, closed form
LB_dep    = max_i CP_i                                = published criticalPathDuration
LB_res    = max_r ceil(busy_r / Q_r)                  = published statistics
```

`busy_r` is already the Phase 5 statistics region's `Σ q·D`, and `ES_i` is already
the Phase 3 timing region. Phase 6 M3's evaluator is arithmetic over data that is
already published — which is why it is nearly free in bytes.

## Where the objective can overflow, and one place it must not be summed

Computed at the engine's declared maxima (`capacity ≤ 2^31−1`, ticks `≤ 2^32−1`,
64 lanes, 128 calls):

| Quantity | Maximum value | Fits `u64`? | Exact as a JS `number` (< 2^53)? |
| --- | --- | --- | --- |
| per-lane `Q_r × T` | 9,223,372,030,412,324,865 | yes, 2.0× margin | no |
| **`idleCapacityTime` summed over 64 lanes** | 590,295,809,946,388,791,360 | **no, 32× over** | **no** |
| `totalResourceWaiting` summed over 128 calls | 549,755,813,760 | yes | yes |

That middle row is a defect in the Phase 6 planning document's proposed public API,
found before implementing rather than after: `ScheduleObjective.idleCapacityTime`
is declared as a single `number`. An aggregate of idle capacity over all lanes is
not representable in `u64`, let alone in an IEEE double, at the ranges the engine
itself permits. Reporting it as a `number` would not overflow loudly — it would
silently lose precision, which is the failure mode this project has refused since
Phase 1 by refusing to saturate.

Resolution carried into `OPTIMIZATION-RFC.md`: idle capacity is **per resource**, as
`u64` in the arena and `bigint` across the boundary, matching how Phase 5 already
publishes `busyTime`. If an aggregate is ever wanted it must be a `bigint` with the
range stated, not a `number`. `totalResourceWaiting` needs no such treatment: at
5.5×10^11 it is exact as a double, and stating that is the difference between a
bound and a guess.

## Budget counters and their overflow policy

| Parameter | Initial limit | Counter | On exhaustion |
| --- | --- | --- | --- |
| candidate strategies | 5 | fixed table | not exhaustible |
| local-search rounds | 8 | `u16` | stop, keep incumbent |
| candidate evaluations | 64 | `u16` | stop, keep incumbent |
| operations per candidate | `O(V+E+K)` = 128+2,304+1,024 | no counter needed | — |
| randomized search | disabled | — | refused as `UNSUPPORTED` |

`u16` is ample for limits this small and the checked rule is the existing one: a
counter never wraps, exhaustion clamps, the outcome says `BUDGET_EXHAUSTED`, and
the baseline survives. Roughly 221k elementary operations at the full budget —
which is an arithmetic ceiling, not a latency measurement. Latency gets measured
in M8, not asserted here.

## The cheapest thing on this list is also the most contested

D3 — longest remaining path as priority rule 1 — costs **512 bytes**: one `u32`
per call slot. It is the only decision on this page whose entire memory cost is a
single line, and Phase 5 has already deferred it twice. The M0 corpus says the
heuristic is beaten in 13 of 1,200 instances and specifically in the
`multiresource` family, so the answer to "is 512 bytes worth it" is now measurable
rather than debatable.

## What this document does not decide

It budgets **arena bytes and overflow**, nothing else. It does not measure wasm code
size, which the same planning document caps at 40 KB with 18,705 bytes currently
free — that has to be re-measured after every milestone, because unlike the arena
it is not knowable from constants. It does not measure runtime, it does not
establish that any strategy improves anything (M0 bounds how much improvement
exists: 13 instances in 1,200), and it does not decide the objective's tie-break
order, which is `OPTIMIZATION-RFC.md`.

## Artifacts versus predictions, after M3

The budget above is the optimization working set. The certificate that M3 actually
shipped is a different thing, and the difference is worth recording:

| Quantity | Predicted or budgeted | Measured | Note |
| --- | --- | --- | --- |
| certificate arena region | 64 B (the record) | 64 B | `QUALITY_BYTES`, asserted at compile time |
| arena length | 75,848 B (Phase 5) | 75,912 B | +64 B, no page change |
| spare address space | 53,784 B | 53,720 B | −64 B, still 7.2% of a 3,848 B working set |
| wasm code | not budgeted here | **+1,486 B** (19,809 → 21,295) | the code, not the arena |
| wasm headroom | 20,191 B free | 18,705 B free | still 47% of the 40 KB cap |

The last two rows are the honest part of this table. M2 budgeted *arena* bytes
and said so; it did not budget code, because code size is not knowable from the
constants the arena is built from. The certificate's code cost 1,486 bytes —
twenty-three times its arena cost — and it is the reason the wasm budget has to
be re-measured after every milestone instead of derived once.

The 19,809 is measured, not remembered: the engine at `HEAD` without the
certificate was rebuilt and measured (24,746 B of Rust, 19,809 B after
`wasm-opt`), then the certificate was restored and rebuilt (26,304 B of Rust,
21,295 B after `wasm-opt`). The 19,809 is 15 bytes above the 19,794 quoted
earlier in `docs/RESOURCE-ABI.md`, because the Phase 5 diagnostics-revision fix
cost those 15 bytes and that document still carried the number from before it.

## Reproducing these numbers

```js
const U64 = 2n**64n - 1n, F53 = 2n**53n - 1n;
const Qmax = 2n**31n - 1n, Tmax = 2n**32n - 1n, R = 64n, V = 128n;
const perLane = Qmax * Tmax;              // fits u64 with ~2x margin
const allLanes = perLane * R;             // does not fit: 32x over
const waiting = Tmax * V;                 // fits u64 and a double
```

The buffer table is `32 + 128*8` per window region and `128*4` for the critical
path, summed and rounded up to an 8-byte boundary. Spare space is
`pages*65536 − arena_end`, both read from the shipped artifact via
`node scripts/dump-layout.mjs`, not from this document.

## Related documents

* `docs/OPTIMIZATION-RFC.md` — the objective, statuses and verification contract.
* `docs/OPTIMIZATION-BENCHMARKS.md` — how much there is to win (98.9% is nothing).
* `docs/RESOURCE-ABI.md` — where these regions sit and how the page count got approved.
