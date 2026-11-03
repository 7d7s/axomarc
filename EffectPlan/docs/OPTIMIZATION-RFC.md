# Optimization RFC

Phase 6 M1: the objective, the tie-breaks, the status semantics and the
candidate-verification contract, frozen before any optimizer is written.

**What is settled here** is the objective and its ordering, the lower bounds and
how they may be described, the statuses and their precedence, and the verification
contract. **What stays with the owner** is listed in §11 — including whether
Phase 6 happens at all, which the M0 measurement makes a live question rather than
a formality.

## 1. What Phase 6 is for, given the evidence

Phase 6 asks whether another valid schedule would be better than the Phase 5 one.
`docs/OPTIMIZATION-BENCHMARKS.md` measured the answer: across 1,200 generated
batches solved against an exhaustive oracle, **1,187 (98.9%) already reach the
optimum**, all headroom lives in three of ten workload families, and seven families
are structurally incapable of improvement.

That changes what Phase 6 should be, and pretending otherwise would be the failure
mode its own review warns about. The defensible product is:

1. a **quality certificate** — lower bounds, gap, and an explicit "provably optimal"
   where the bound closes — computed from integers the engine already publishes;
2. **one** additional strategy aimed at the shape that has headroom (calls straddling
   several scarce resources), accepted only if it beats the baseline on the recorded
   fixtures without regressing anything else;
3. a bounded local search over admission order, if and only if (2) shows the corpus
   can be improved at all.

Five strategies and a search are not justified by the measurement, and "we built
them anyway" is not a defense that has ever worked on a budget.

## 2. The objective, frozen

A schedule is a window assignment `(S_i, F_i)` for every call `i`. The comparison is
a lexicographic tuple:

```
Score(S) = (T, W, C)
```

| Component | Definition | Type | Schedule-dependent? |
| --- | --- | --- | --- |
| `T` — makespan | `max_i F_i` over all calls | `u32` | yes |
| `W` — resource waiting | `Σ_i (S_i − ES_i)`, `ES_i` from Phase 3's timing region | `u64` | yes |
| `C` — canonical order | the window list in **call-index order**, compared elementwise by `start` then `finish` as unsigned integers; the first difference decides | — | yes |

**Fewer is better in every component.** `T` is the primary objective. `W` breaks
makespan ties by preferring the schedule that spent less time blocked on capacity,
which is the delay a caller would notice as contention rather than as work. `C` is
not an objective at all: it is the determinism safety net that makes the ordering
total when two schedules are genuinely equivalent, and it exists so that "the same
input always gives the same bytes" cannot depend on which of two identical candidates
was generated first.

Two properties follow, and both are theorems rather than tests:

- `S_i ≥ ES_i` for every dependency-feasible schedule, because `ES_i` is the earliest
  topologically possible start. So `W ≥ 0` without needing a clamp, and `W = 0`
  exactly when resources cost nothing.
- **The baseline is always in the candidate set**, so the optimizer's result can never
  be worse than the baseline under this ordering. That is stated as a structural rule
  (§6), not left to a test to discover.

### 2.1 Idle capacity is removed from the ordering, with proof

The Phase 6 planning document's ordering was `(T, W, I, C)`, where `I` is total idle
capacity over the horizon. `I` cannot break a tie, because **it is not a function of
the schedule**. Per resource:

```
I_r = Q_r × T − busy_r        where busy_r = Σ_i q_ir · D_i
```

`Q_r` is a declared capacity and `busy_r` is a sum over demands and durations —
neither depends on when anything runs. Every call executes exactly once for its full
duration in every candidate, so `busy_r` is constant across candidates, and among
candidates that tie on `T`, `I_r` is identical for every `r`. The aggregate
`Σ_r I_r = T · Σ_r Q_r − Σ_r busy_r` is therefore a strictly increasing function of
`T` alone: it carries no information that `T` does not already carry, and once `T`
ties it is constant. A tie-break that is constant on the tie set is not a tie-break.

This is not an argument from taste; it is visible in the shipped engine.
`finish_up` in `engine/src/scheduling.rs` accumulates `busy` by iterating demand
records and durations and never reads a window — the implementation already knows
what the algebra says.

`I_r` stays in the result as a **diagnostic**, reported per resource next to the
`busy_r` it is derived from. What is removed is its role in the ordering, where it
would have been a decoration that looked like a criterion.

### 2.2 The aggregate cannot be represented anyway

Independent of the above: an aggregate over lanes is not representable at the
ranges the engine permits. `Σ_r I_r` reaches 5.9×10^20 at `capacity ≤ 2^31−1`,
`ticks ≤ 2^32−1`, 64 lanes — 32× past `u64`, and far beyond the exact-integer range
of a double, so declaring it as `number` in the public API (as the planning document
does) would lose precision silently rather than fail loudly. Per-resource `u64`,
crossing the boundary as `bigint`, is the same treatment Phase 5 already gives
`busyTime`. See `docs/OPTIMIZATION-BUDGETS.md`.

## 3. Lower bounds and how they may be described

```
LB_dep = max_i CP_i                    = published TimingAnalysis.criticalPathDuration
LB_res = max_r ceil(busy_r / Q_r)      = published statistics (busyTime, capacity)
LB     = max(LB_dep, LB_res)
Gap_LB = (T − LB) / LB                 , LB == 0  ->  reported as not applicable
```

Both bounds are arithmetic over integers the engine already emits — which is why the
evaluator is cheap in package bytes, and cheap is the whole point given §1.
`LB == 0` cannot arise for a non-empty batch (durations are positive), but the empty
batch is defined behaviour, so it is handled explicitly and **never** divided.

The honest description, and the one the API must use: **`Gap_LB` is a distance from a
bound, not the optimality gap.** M0 measured why: the bound equals the optimum on
100% of the free families but only 15% of `multiresource` — the family where
headroom actually exists. So the certificate is most confident where nothing is at
stake and weakest where something is. Shipping a number called "optimality gap" on
that evidence would be exactly the dishonesty the Phase 6 document forbids.

One further reportable case, also from M0: 1,187 of 1,200 instances have `T == optimum`
while `LB` is often below `T`. A candidate whose `T` equals a *proven* optimum (from
the offline solver, in tests only) is optimal; a candidate whose `T` equals `LB` is
also optimal, and that is the only optimality the production engine can ever certify
by itself. The distinction belongs in the diagnostics vocabulary.

## 4. Status semantics

Five statuses, and the first thing to say about them is that they describe **why the
search ended**, not what it found — `improvementCount` and `budgetExhausted` are
reported alongside, and a run can legitimately be both `BUDGET_EXHAUSTED` and
improved.

| Status | Meaning | Incumbent returned | Precondition |
| --- | --- | --- | --- |
| `OPTIMIZED` | some verified candidate strictly beat the baseline under §2 | that candidate | ≥ 1 candidate accepted |
| `UNCHANGED` | the search completed and nothing beat the baseline | the baseline | budget not exhausted |
| `BUDGET_EXHAUSTED` | the search stopped on a limit, not on completion | best verified candidate, possibly the baseline | always reported with counts |
| `UNSUPPORTED` | well-formed but outside the model — randomized search, a batch above the ceilings, missing durations or demands | nothing; caller keeps the Phase 5 result | before any search |
| `INVALID` | a candidate the engine generated failed verification | never returned to the caller as a result | see below |

**`INVALID` is split, and the split matters.** The planning document lists `INVALID`
as a status while also requiring that invalid options return structured errors —
which is two different mechanisms for one word. Resolution: malformed *caller input*
remains an `EffectPlanError` thrown before the engine is asked anything, exactly like
Phases 1–5; the `INVALID` status survives only to mean *an engine-generated candidate
failed verification*, which is a bug-grade event, never a caller's fault, and must
count toward `candidatesRejected` rather than surface as a result. If `INVALID` ever
reaches a caller as a top-level outcome, something is wrong in the engine and the
counters should show it.

`UNSUPPORTED` precedes everything: a request the model cannot serve does not get a
partial search that returns a confusing `UNCHANGED`.

## 5. The candidate-verification contract

A candidate may replace the incumbent only after passing, **in this order**:

1. **Completeness** — every call appears exactly once: 128 slots, no duplicates, no
   omissions, no uninitialised window.
2. **Duration** — `F_i = S_i + D_i` for every call, checked against the durations the
   caller supplied, not against anything the strategy believes.
3. **Dependency** — `S_j ≥ F_i` for every edge in the graph the *planner* emitted, read
   from the output region rather than from a strategy's private copy.
4. **Capacity** — no tick at which summed demand exceeds capacity. Computed by event
   sweep over interval endpoints (`2V` events, the existing `heap_sort`), never by a
   per-tick array: makespan reaches 2^32−1, so a tick-domain buffer is unbounded and
   cannot exist in this arena.
5. **Objective integrity** — `T`, `W` and every `busy_r`/`I_r` computed with
   `checked_add`/`checked_mul`; an overflow refuses the candidate rather than
   saturating it.
6. **Comparison** — strictly better than the incumbent under §2, tie broken by `C`.

Three rules make this a contract rather than a checklist:

- **Verification is independent of production.** The checks re-derive everything from
  the declarations — edges from the output region, durations from the durations
  region, demands from the demand regions — and call no function that the placement
  code uses to construct the candidate. A strategy that miscalculates and a verifier
  that agrees with its miscalculation is the failure this exists to prevent, and it is
  the same reason Phase 5's oracle shares no code with the engine.
- **The incumbent is never in the candidate's write path.** Candidates are generated
  into one reusable working buffer, verified there, and copied into the incumbent
  only on an accepted comparison. A failed or aborted candidate therefore cannot
  corrupt the result — "optimization failure never corrupts the Phase 5 baseline"
  is implemented as a memory-discipline rule, not as a hope.
- **No candidate is accepted because an internal strategy produced it.** Every one
  passes the same six checks, including the baseline itself, which is verified on the
  way in so that a Phase 6 result never silently inherits a Phase 5 bug.

The exhaustive oracle stays outside the package, in `benchmarks/`, exactly as in
Phase 5: a production verifier that is exponential is not a verifier, it is an outage.

## 6. Strategies in scope

| Name | Priority rule | Measured on the 1,200-instance corpus |
| --- | --- | --- |
| `baseline` | Phase 5: slack, then dependents, then index | **always in the candidate set** |
| `critical-path` | remaining path `CP_i`, then slack, then dependents, then index | 4 wins, **9 regressions** |
| `slack` | equals the baseline rule today | **duplicate — delete** (a serial-GS run of it wins 1 and loses 11) |
| `resource-aware` | bottleneck pressure over scarce lanes | 2 wins, **15 regressions** |
| `hybrid` | `CP_i`, then bottleneck, then slack | **0 wins in 1,200 — delete** |

Wins and regressions are counted against the **engine's** makespan, and the
portfolio that keeps the baseline closes 6 of the 13 improvable instances for 17
of the 28 recoverable ticks. Full table and reasoning:
`docs/OPTIMIZATION-STRATEGIES.md`.

The measured result sharpens the rule this section started from. **No priority
studied here beats the baseline more often than it loses to it** — every rule
regresses more instances than it wins. So a Phase 6 that *switched* ranking would
be a net loss, and generate-then-select with the baseline retained is not a
precaution but the only construction with a positive measured value.

The baseline's permanent presence is what makes "optimized output is never worse than
the retained baseline" a structural guarantee: the worst possible Phase 6 outcome is
the schedule Phase 5 already produced, reported as `UNCHANGED`.

## 7. The Rust / TypeScript boundary

Unchanged from Phases 1–5, and Phase 6 does not erode it:

- Objective evaluation, comparison, candidate generation, local search and
  verification are **Rust**. The engine is the only authoritative implementation of
  dependency and timing algorithms; an optimizer that decides which schedule wins
  is an algorithm about both.
- The host **presents**. Division for display, formatting, mapping numeric indexes to
  the caller's own keys — the same rule that already keeps the utilisation ratio out
  of the engine.
- The exhaustive oracle and the headroom corpus are dev-only, never packed, never
  imported by `packages/core/src`.
- Revalidation inside Rust stays mandatory: TypeScript checks shape so callers get a
  good error early; the engine re-checks everything because a hostile caller does not
  run TypeScript.

## 8. ABI sketch, for M6 to finalize

Nothing here is a frozen ABI; it is the shape that keeps Phase 6 out of the
Phase 5 contract:

- **A new export**, `effectplan_resource_optimize`, rather than more parameters on
  `effectplan_resource_schedule`. The rule the project has followed since Phase 3: a
  change to what an export writes requires a new name, not new behaviour.
- **New regions**, sized from `docs/OPTIMIZATION-BUDGETS.md` (3,848 bytes total,
  7.2% of the spare in page 18). **No page growth is requested or needed.**
- **New info selectors** continuing the published pointer/length/max pattern after 51,
  with unknown selectors still returning `u32::MAX` so an older host notices.
- **A new status band, 150–159**, deliberately skipping 138 and 139 so the Phase 5
  resource family 133–137 stays a contiguous block, and leaving 140 (`ERR_INTERNAL`)
  alone. Stable identifiers that callers may branch on, never reused.
- The canonical-pointer rule inherited without amendment: every argument names its
  region base exactly, or the call is refused.

## 9. Budget defaults

| Parameter | Proposed | Counter | Behaviour at the limit |
| --- | --- | --- | --- |
| strategies | 4 (see §6) | fixed table | n/a |
| local-search rounds | 8 | `u16` | stop, keep incumbent, report `BUDGET_EXHAUSTED` |
| candidate evaluations | 64 | `u16` | as above |
| randomized search | disabled | — | `UNSUPPORTED` |
| unbounded search | forbidden | — | does not exist |

These are **limits, not guarantees**. The arithmetic ceiling is roughly 64 candidates
at `O(V+E+K)` ≈ 3.5k operations each, about 221k elementary operations, which says
nothing about milliseconds: latency is measured in M8 against the corpus, and the
release gate will be the measurement rather than this table.

## 10. Acceptance criteria Phase 6 can honestly meet

The planning document's release table needs three edits, all of them consequences of
M0/M2 rather than lowering of standards:

1. ~~"At least one benchmark family demonstrates measurable makespan improvement"~~
   becomes **improvement on the recorded fixtures, and zero regression on the other
   1,187 instances** — with `exclusive` and `highutilization` explicitly marked
   incapable of demonstrating makespan gains, so nobody is tempted to "pass" them by
   redefining them.
2. ~~"npm unpacked ≤ 50 KB"~~ is retired as the superseded pre-Phase-4 ceiling; the
   live gate is the frozen Phase 5 baseline plus the Phase 6 envelope
   (94,565 + 12,000 = 106,565), owner review 2026-11-03.
3. A new one, because it is the only honest summary of §3: **the quality report never
   labels a bound-derived distance as an optimality gap, and never certifies
   optimality it cannot prove.**

Everything else in that table — zero dependency violations, zero capacity violations,
no missing or duplicated operations, 100% determinism, 100% candidate verification,
budget enforcement, zero heap allocation, zero runtime dependencies, 18 pages — stands
exactly as written and is testable as written.

## 11. Decisions, for the owner

P6-D1, P6-D4 and D3 are settled by measurement or by adoption. P6-D2 and P6-D3
had a stated gate, and the gate has now been evaluated — the recommendation
column below has been updated with what the evidence says. The decisions
themselves remain the owner's.

| # | Decision | Recommendation | Cost of deciding wrong |
| --- | --- | --- | --- |
| P6-D1 | Drop idle capacity from the objective ordering | **Adopted** — it is provably constant on the tie set and unrepresentable in aggregate; the certificate has no idle metric | a tie-break that never fires, plus a silent precision loss |
| P6-D2 | Proceed with evaluator + certificate + one strategy, not five | **The gate is not met.** It was conditional on `resource-aware` beating the fixtures; it wins 2 and loses 15. **Ship the certificate, do not build the optimizer** | six algorithms and no measured win — the failure §20 of the planning document names |
| P6-D3 | Is Phase 6 worth doing at all for makespan? | **For makespan, no** — the ceiling is 6 of 13 improvable instances and 17 of 28 recoverable ticks, and only via a bounded local search whose entire prize is 11 ticks. The certificate is worth shipping on its own terms | 1.1% of instances can absorb a lot of engineering |
| D3 (carried from Phase 5) | `CP_i` priority rule | **No** — every rule studied regresses more instances than it wins, so the 512 bytes are not earned. `docs/RESOURCE-MODEL.md` §9 records it | deferred twice already; the corpus answers it |
| P6-D4 | New export vs extending `resource_schedule` | **Adopted** — a new export | an existing contract changing meaning |
| P6-D5 | Candidate/round limits | **Moot if P6-D2 is not** — keep 64/8 until M8 measures; then freeze from data | a budget that is a wish |
| D2 (carried from Phase 5) | Session resource table and the two invalidation domains | Blocks **M7 only**; M1–M6 are independent of it | stale schedules presented as current |

The Phase 6 scope question therefore has an answer with a number attached: the
certificate ships, and the optimizer does not. That is not "Phase 6 failed" —
the certificate is a real deliverable, it is what makes a schedule's quality
*knowable* rather than asserted, and it cost 1,486 bytes of wasm and 64 bytes
of arena. It is that the *makespan* half of Phase 6 has a ceiling too small to
justify an export, a region, a verification contract, a CI job and a permanent
compatibility obligation.

## 12. What this document does not decide

It does not implement anything, does not fix the final region offsets (M6 will, and
they must be read from the artifact rather than typed here), and does not measure
latency, wasm bytes or arena use at runtime — the first two are M8 work, the third
is bounded by `OPTIMIZATION-BUDGETS.md` and needs no page. It does not settle
whether Phase 6 ships at all, which is P6-D3 and is the owner's call — but the
evidence for that call is now in `docs/OPTIMIZATION-STRATEGIES.md`, and the
recommendation it supports is above.

## Related documents

* `docs/OPTIMIZATION-BENCHMARKS.md` — the measurement this document is built on.
* `docs/OPTIMIZATION-BUDGETS.md` — the memory and overflow envelope.
* `docs/RESOURCE-SCHEDULING.md` — the baseline being optimized.
* `docs/RESOURCE-MODEL.md` — the ranking rules, and D3.
* `docs/RESOURCE-ABI.md` — where a new region would sit.
