# Optimization benchmarks

What Phase 6 could win, measured before Phase 6 builds anything.

The Phase 6 premise is that among the valid schedules the Phase 5 engine can
construct, some are better than the one it constructs. That is a testable claim,
and it was untested until this document existed. The harness is
`benchmarks/optimization-headroom.ts`, run with `npm run benchmark:headroom`, and
its output is `artifacts/optimization-headroom.json`.

**The answer: on this corpus, 98.9% of instances have nothing to win.** Of 1,200
generated batches, the shipped baseline already reaches the exhaustively computed
optimum in 1,187. Thirteen instances have headroom, totalling 28 ticks, and all
thirteen live in three of the ten workload families.

## Why this is measured first

An optimizer is not free. It costs package bytes, arena bytes, and latency, and
the RFC's own §20 names the failure mode: "build six scheduling algorithms and
declare success without showing that they produce better schedules". The only
defense against that is to establish, before writing them, how much better a
schedule could be — because if the answer is "usually not at all", the honest
deliverable is a quality certificate, not a search.

## Method

Each instance is solved twice, by things that do not share any code:

- **The baseline** comes from the shipped engine through the public API
  (`planResources`), so it measures what a caller actually gets.
- **The optimum** comes from `optimalMakespan` in
  `benchmarks/reference-resource-scheduler.ts`, an exhaustive serial schedule
  generation scheme over every precedence-feasible permutation. By the standard
  result, some such permutation yields an optimal RCPSP schedule, so this is a
  true lower bound on what *any* heuristic can achieve — not a second guess to
  compare guesses against.
- **Every baseline is independently verified first.** `verifySchedule` walks the
  ticks and recomputes usage from the declarations; 0 of 1,200 were rejected.
  Without that, a gap could just mean the engine produced nonsense cheaply.

The two lower bounds Phase 6 would report are computed only from integers the
engine already publishes — `criticalPathDuration` for the dependency bound, and
`ceil(busyTime / capacity)` per resource for the workload bound — and are checked
against the verified makespan. **0 bound defects across 1,200 instances.**

## The one constraint that matters: instance size

Calls per instance are capped at **4 to 7**, because the oracle is factorial.
This is the harness's limit, not the engine's, and it is recorded in the artifact
metadata rather than left for the reader to discover:

> These numbers are **not** evidence about a 128-call batch. Whether headroom
> grows, shrinks or vanishes at scale is unmeasured, and the argument below is
> only as wide as the corpus.

## Results

| Family | n | % already optimal | gap histogram | worst gap | LB == optimum | Why it looks like that |
| --- | --- | --- | --- | --- | --- | --- |
| `independent` | 120 | 100.0% | 0:120 | 0 t (0%) | 100% | no ordering and generous capacity: parallelism is already free |
| `chain` | 120 | 100.0% | 0:120 | 0 t (0%) | 100% | the order is forced by dependencies, so no schedule can reorder it |
| `fanout` | 120 | 100.0% | 0:120 | 0 t (0%) | 78% | one root releases many ready calls: ready-set ordering is the only lever |
| `fanin` | 120 | 100.0% | 0:120 | 0 t (0%) | 79% | many calls converge on one: the dependency bottleneck dominates |
| `exclusive` | 120 | 100.0% | 0:120 | 0 t (0%) | 100% | one unit of capacity: every schedule is serial, so makespan is total work whatever the order |
| `shared` | 120 | 99.2% | 0:119 2:1 | 2 t (29%) | 97% | two half-open lanes with freedom of order |
| `multiresource` | 120 | 92.5% | 0:111 1:3 2:3 3:1 5:2 | 5 t (38%) | 15% | calls straddle several scarce lanes at once: placement is the only lever |
| `mixeddurations` | 120 | 97.5% | 0:117 1:2 2:1 | 2 t (12%) | 97% | a few long calls among many short ones: priority can matter |
| `highutilization` | 120 | 100.0% | 0:120 | 0 t (0%) | 100% | every call takes the whole lane: saturated and serial by construction |
| `sparseuse` | 120 | 100.0% | 0:120 | 0 t (0%) | 100% | capacity rarely binds: measures optimization overhead against an already-free schedule |

Totals: **1,200 instances, 1,187 already optimal (98.9%), 13 with headroom, 28
ticks of headroom all told, 0 bound defects, 0 unverified baselines**, measured in
about 1.9 s against a 21,295-byte artifact on 18 pages of linear memory. The
measurement was re-taken after the Phase 6 certificate was added and the totals
are unchanged to the byte: the certificate costs 1,486 bytes of wasm and moves
no schedule.

## Seven families cannot show improvement, and that is a finding

`independent`, `chain`, `exclusive` and `highutilization` are not "the baseline
happens to be good" — they are **structurally unimprovable**. With one capacity-1
lane every schedule is serial, so makespan equals total work regardless of order;
with no dependencies and generous capacity everything already runs at once.
Tuning those families to produce a percentage would require changing what they
mean.

This matters because the Phase 6 planning document lists "Exclusive resource
contention" and "High resource utilization" among the twelve benchmark workloads
a release must demonstrate improvement on. On this evidence those two families
can never demonstrate makespan improvement, and a gate written against them would
be satisfied only by redefining the workload — which is precisely the
benchmark-specific hack the same document warns against.

## Headroom concentrates where multiple scarce resources interact

All thirteen improvable instances are in `multiresource` (9), `mixeddurations`
(3) and `shared` (1). The worst is 5 ticks on a 13-tick optimum (38%). That is
the target: calls holding several scarce lanes at once, where the order of
admission changes which lane blocks which.

## The lower bound is weakest exactly where optimization matters

`LB == optimum` in 100% of the free families, but only **15%** of
`multiresource`. Read that the right way round: on the instances where an
optimizer could win, the bound usually sits *below* the true optimum, so the
reported `Gap_LB` overstates the gap and a schedule that is already optimal will
often fail to be certified as such.

Two consequences, both awkward and both worth stating:

1. `Gap_LB` is not the optimality gap and must never be labelled as one.
2. The quality certificate — Phase 6's cheapest genuine deliverable — is
   confident precisely where no optimizer is needed, and timid precisely where one
   is. It is still worth shipping, because "provably optimal" on 100% of the
   `chain`/`exclusive`/`sparseuse` corpus is a real answer, and on the rest the
   bound is an honest ceiling on what anyone could claim.

## The fixtures

The artifact embeds all 13 improvable instances, fully specified — durations,
capacities, per-call demands, the engine's own edges, the optimum, the baseline
and the bound. They are the acceptance instrument for any Phase 6 candidate:

- an optimizer that cannot improve **any** of the 13 has not optimized anything;
- an optimizer that regresses **any** of the 1,187 already-optimal instances is
  worse than the baseline it replaced;
- and because 7 of 10 families are structurally incapable of improvement, a
  claimed "average % improvement" across the whole corpus would mostly measure
  noise. Report improvement on the three families that have headroom, and report
  *no regression* elsewhere.

## What the harness refuses to tolerate

It exits non-zero rather than printing a pretty table if any premise breaks: a
lower bound above a verified makespan, a baseline the independent checker rejects,
a family that measured nothing, or a corpus with zero headroom. The last is
deliberate: if nothing in the corpus can be improved, the release gate that asks a
family to demonstrate improvement cannot honestly pass, and silence would be a
lie by omission. The guards were negative-tested by doctored copies of the
compiled output — a gate that has never failed is a decoration.

Determinism is verified rather than asserted: two consecutive runs produce
byte-identical `families`, `fixtures` and `totals` sections; only `measuredAt`,
`elapsedMs` and `wasmBytes` differ. Seeds are fixed per family and recorded
implicitly by family index. `wasmBytes` is the size of the artifact the corpus
was measured against, so it moves when the engine changes — the Phase 6
certificate moved it from 19,809 to 21,295 and moved nothing else: the
`families`, `fixtures` and `totals` sections are identical between the two
measurements, which is the evidence that the certificate changed no schedule.

## What this does not measure

Runtime and memory cost of optimization (there is none yet), peak arena use,
artifact size after any Phase 6 code, incremental re-optimization, or anything at
all above 7 calls. The maximum-graph families the RFC asks for (1,152 events,
2,304 edges) are unmeasured here, and cannot be measured against an exhaustive
oracle — they need a bound-based comparison, which is `OPTIMIZATION-LOWER-BOUNDS.md`
territory and does not exist yet.

## Related documents

* `docs/RESOURCE-MODEL.md` §6 — the ranking the baseline actually applies, and D3.
* `docs/RESOURCE-SCHEDULING.md` — what the baseline emits, and what it does not claim.
* `docs/RESOURCE-ABI.md` — where the bytes are, before Phase 6 asks for more.
