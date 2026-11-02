# Resource ABI

The memory and byte-level budget for Phase 5, measured before any of it is built.

This is the M2 artifact, and as of M3 it is the layout the engine ships: the
regions below are in `engine/src/lib.rs`, the total is pinned by a compile-time
assert, and `verify:wasm` checks the published offsets against the artifact. Its
numbers come from the shipped engine and from a build that was made to measure the
page cost before the decision was taken.

## 1. Pages are a consequence, not a setting

The engine's arena is a static `no_std` array. LLD sizes linear memory as
`data + stack` rounded up to a page, so the only way to change the page count is
to change the arena. The engine puts its arena at the top of the data segment, so any host that needs
absolute addresses asks `info(1)` rather than assuming them — which is why adding a
region moved the base (1,049,928 before Phase 5, 1,049,960 after) and why no
document or test should hardcode it. Before Phase 5 landed:

| Quantity | Value |
| --- | --- |
| arena base | 1,049,928 |
| arena length | 61,920 |
| arena end | 1,111,848 |
| linear memory | 1,114,112 B = 17 pages |
| **spare address space** | **2,264 B** |

To test what Phase 5 would cost, the arena was temporarily extended by 26,216
bytes — the largest candidate configuration in §3 — and the module was rebuilt.
The declared minimum memory moved to **18 pages (1,179,648 B)** and the artifact
size did not change at all (13,499 B, because the arena is BSS, not code). That
measurement is what the 18th page was approved on; the regions then landed for
real, and the artifact now reports:

| Quantity | Value |
| --- | --- |
| arena base | 1,050,016 |
| arena length | 75,848 |
| arena end | 1,125,864 |
| linear memory | 1,179,648 B = 18 pages |
| spare address space | 53,784 B |
| wasm code | 19,794 B, 20,206 under budget |

The base is not fixed either: it has moved twice as the data segment grew, which is
why nothing in the host, the tests or these documents addresses the arena by a
literal address. The numbers in this table are this artifact's.

`verify:wasm` reads the expected page count from `scripts/limits.mjs`, and
`EXPECTED_PAGES` moved in the same commit that grew the arena — a gate that permits
growth in advance is not a gate.

The consequence worth stating plainly: **Phase 5 does not fit in 2,264 bytes at
any useful size.** The smallest configuration that can express the specification's
own examples — 8 capacity resources, 64 demand records, a diagnostics buffer —
needs 4,904 bytes, and before a single resource is declared the per-call
structures alone (demand offsets 516 B, schedule output 1,056 B, and the three
per-call working arrays 1,536 B) cost 3,076 — already more than the 2,264 spare. So
the 18th page is not a performance convenience, it is a precondition. With one page the arena gains
65,536 B, of which 2,264 was already spare: **67,800 bytes of new address
space.**

## 2. Region budget

With `V = MAX_CALLS = 128` (fixed since Phase 1), `R` capacity resources, `K`
demand records and `N` diagnostics, the new regions cost:

| Region | Bytes | Written by | Read by |
| --- | --- | --- | --- |
| capacities, `4R` | 4R | host | engine |
| demand offsets, `4(V+1)` | 516 | host | engine |
| demand records, `8K` (resource index `u32`, amount `u32`) | 8K | host | engine |
| live usage during placement, `4R` | 4R | engine | engine |
| predecessors each call waits on, `4V` | 512 | engine | engine |
| earliest tick each call may start, `4V` | 512 | engine | engine |
| downstream dependents per call, `4V` | 512 | engine | engine |
| schedule, `32 + 8V` (start, finish per call) | 1,056 | engine | host |
| per-resource stats, `32 + 16R` (capacity, peak `u32`; busy time `u64`) | 32 + 16R | engine | host |
| diagnostics, `32 + 16N` | 32 + 16N | engine | host |

Total: **24R + 8K + 4,196 bytes**, eight-byte aligned — 13,924 rounded to 13,928
at the shipped configuration.

Three working arrays were budgeted at M2 and the scheduler needed a fourth. Ranking by
"more downstream dependents" — rule 3 of the priority the specification proposes —
has nowhere to live otherwise; the per-call finish time does not need one, because
it is already in the published schedule region. The extra 512 bytes came out of the
same page, which is the point of measuring it first.

## 3. Candidate configurations

The specification asked for 64, 128 and 256 capacity resources to be benchmarked
before choosing a production limit. Measured against the 67,800 bytes that one
page buys:

| R | K | new arena bytes | arena length | pages | address space left |
| --- | --- | --- | --- | --- | --- |
| 8 | 64 | 4,904 | 66,824 | 18 | 62,808 |
| **64** | **1,024** | **13,928** | **75,848** | **18** | **53,784** |
| 128 | 1,024 | 15,464 | 77,384 | 18 | 52,248 |
| 256 | 1,024 | 18,536 | 80,456 | 18 | 49,176 |
| 256 | 2,048 | 26,728 | 88,648 | 18 | 40,984 |
| 256 | 8,000 | 74,344 | 136,264 | **19** | — |

The last column uses the base this build reports (1,050,016), so the rows other than
the shipped one are computed rather than measured — enough to show that the page
holds, not a figure to gate on.

So within one page the engine can support up to 256 capacity resources and 7,171
demand records — more than 55 demands per call at the maximum batch size. **Memory stops being the binding constraint
anywhere near the sizes Phase 5 needs.** The limits are therefore taken from
function and not from byte pressure, and these are the values the engine now
publishes:

| Limit | Proposed | Why this number |
| --- | --- | --- |
| `MAX_CAPACITY_RESOURCES` | 64 | 24 concurrent resource types is already more than the worked examples in the specification use; the other 40 cost 2,560 B and buy the "one operation touches many resources" case its headroom |
| `MAX_DEMAND_RECORDS` | 1,024 | 8 demands per call at `MAX_CALLS = 128`, mirroring `MAX_ACCESS = 1,024` so the input budget stays symmetric with the dependency model |
| `MAX_RESOURCE_DIAGNOSTICS` | 64 | one per call plus slack; beyond that a caller has more problems than the buffer can name |
| `MAX_CAPACITY` (per resource) | 2³¹ − 1 | keeps `demand × duration` sums inside a checked `u64` with room to spare (§5) |

Raising any of these later costs bytes, not architecture: `R=256, K=2,048` still
fits page 18 with 41,584 B spare.

## 4. Index spaces, and why they cannot be merged

Dependency resources are interned by the host into `0..resourceCount` and the
engine indexes `last_writer` and `reader_head` with them — `RES_SLOTS =
MAX_RESOURCES + 1 = 1,025` `u32` lanes each. Capacity resources get their own
index space, `0..R`, into a separate table.

This is not just a semantic preference. A capacity index and a dependency index
have no relationship to each other, so merging the models would mean inventing a
mapping that does not exist, and every conflict rule would then have to answer
"does a write to record X contend for a GPU slot?" — which it cannot, because
those are different questions about different things. The layout enforces the
separation: the arrays are different sizes, indexed differently, and live in
different regions.

## 5. Arithmetic rules

Phase 3 set the precedent this follows: values are checked, ranges are refused,
and nothing wraps.

| Rule | Behaviour |
| --- | --- |
| demand must not exceed capacity | refused before scheduling, as `INFEASIBLE` with the resource and call indexes — an operation that can never fit is not a scheduling delay |
| capacity, demand | positive integers, `0` refused in both |
| usage during placement | `u32`, cannot exceed capacity by construction because reservation is atomic and checked against the lane |
| busy time `Σ q·D` per resource | accumulated in `u64` with `checked_add`; overflow is refused, never saturated |
| makespan | the Phase 3 bound stands: durations are positive integers of at most 4,294,967,295 ticks |
| utilisation ratio | **not computed in the engine.** It emits `busy_time` and `capacity × makespan` as two integers; the division is presentation and belongs to the host |
| time comparison | the Phase 3 half-open convention `[start, finish)` is inherited verbatim, so a release at tick *t* and an acquisition at tick *t* are not an overlap — if the two layers used different conventions they would disagree about the same schedule |

New engine status codes follow the Phase 4 numbering (131/132 are the checkpoint
pair): `133 ERR_RESOURCE_CAPACITY` for a demand that cannot be met, `134
ERR_RESOURCE_RANGE` for arithmetic outside the supported range, `135
ERR_RESOURCE_UNDECLARED` for a requirement naming a resource that was never
declared. Diagnostics carry numeric indexes only, never a caller's resource key —
the same rule the error surface has followed since Phase 1.

## 6. What is settled, and what is still a decision

The budget is settled and in force. So are two architectural choices, adopted when
the owner said to proceed and implemented in M3:

- **the resource check is its own export, not a twenty-first parameter on `v4`** —
  chosen because one call that always validates, plans, layers, times and schedules
  cannot express the specification's own §13.1 rule that a scheduling-only change
  leave the dependency graph alone. A Rust test asserts the graph, levels,
  durations, timing and checkpoint regions are all still zero after a resource call.
- **capacity keys are interned by the host into `0..R`**, so no caller string
  crosses the ABI boundary and a diagnostic cannot leak one.

Two remain genuinely open, and neither should be settled by whoever writes the next
module: how a session carries resource declarations across versions and tracks the
two invalidation domains (D2), and whether the engine computes longest-remaining-
path priority, which it does not have today (D3). Both are written up in
`docs/RESOURCE-MODEL.md` §9, which is the M1 gate.

## 7. Related documents

* `docs/ABI.md` — the Phase 1–4 layout this extends.
* `docs/RESOURCE-MODEL.md` — the semantics and the open decisions.
* `docs/SIZE-REPORT.md` — the package and memory budgets, and how they are reviewed.
