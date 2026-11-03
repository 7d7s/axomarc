# Optimization strategies

Phase 6 M4, measured before it is built: what each priority rule is actually worth
on a corpus with known optima.

`benchmarks/strategy-prototype.ts` (`npm run prototype:strategies`, artifact
`artifacts/strategy-prototype.json`) runs each rule as a serial schedule
generation scheme in TypeScript over the 1,200-instance corpus from
`docs/OPTIMIZATION-BENCHMARKS.md`, and compares every result against two things
that are not prototypes: the makespan the shipped engine actually produced, and
the exhaustive optimum. Candidates that the independent checker rejects are
counted and fail the run; the prototype's own CPM is checked against the engine's
published earliest starts and slack and fails if a single value disagrees.

**This file is not a description of what the engine does.** It is evidence about
what a Phase 6 could select, and the distinction is why the numbers mean anything:
the prototype shares no code with the engine, so if the two disagree the disagreement is
information rather than a tautology.

## Results

Wins and regressions are counted against **the engine's makespan**, not against
each other. 1,200 instances, 13 of which have any headroom at all.

| Rule | Priority key (every rule ends with the same tail: more dependents first, then lower call index) | Wins vs engine | Regresses vs engine |
| --- | --- | --- | --- |
| `baseline-rule` | slack, dependents, index — the shipped ranking, run as serial GS | 1 | **11** |
| `critical-path` | longest remaining path `CP_i`, then slack, dependents, index | 4 | 9 |
| `resource-aware` | bottleneck pressure `max(q/Q)`, then slack, dependents, index | 2 | 15 |
| `hybrid` | `CP_i`, then bottleneck, then slack, dependents, index | **0** | 9 |
| `shortest-first` | duration, then slack, dependents, index | 1 | **200** |

Portfolio (engine schedule always retained, best verified candidate wins): closes
**6 of the 13** improvable instances and recovers **17 of the 28** ticks that
exist to be recovered.

## What this says, in the order it matters

**1. Every single rule regresses more often than it wins.** `critical-path` wins 4
and loses 9; `resource-aware` wins 2 and loses 15. So "adopt a better priority
rule" is a **net loss** on this corpus. The only construction that is a net gain
is generate-then-select with the baseline permanently in the candidate set —
which Phase 6 M1 states as an invariant, and which this table converts from a
precaution into the entire justification for the design. A Phase 6 that replaced
the Phase 5 ranking rather than competing with it would have made 9–200 instances
worse to fix 1–4.

**2. The engine's own ranking, run as a serial scheme, loses 11 and wins 1.** The
priority is not the only variable: Phase 5 uses *parallel* list scheduling and
serial GS is a different generator. So the baseline that must be retained is the
engine's actual output, not "the same priority done differently" — and this is the
measured reason §5's verification contract verifies the baseline itself on the way
in, rather than assuming a Phase 6 result inherits a correct starting point.

**3. `hybrid` contributes nothing.** Zero wins across 1,200 instances. M1 guessed
the five-strategy portfolio was really four; the measurement is harsher — it is
really three plus a baseline. Delete `hybrid` before it costs wasm bytes and a
diagnostics row that can never fire.

**4. The realistic ceiling is small and specific.** Best case on this corpus: 6
instances improved, 17 ticks recovered, and never a regression — because the
portfolio keeps the baseline. Whether that is worth an export, a region, a
verification contract, a CI job and a permanent compatibility obligation is a
judgment call with a number attached to it now, which is more than Phase 6
started with.

**5. 11 ticks stay on the table.** The portfolio closes 6 of 13 improvable
instances. The remaining 7 need something that reorders *within* a schedule
rather than choosing a different priority — M5's bounded local search — and the
prize for that whole additional mechanism is 11 ticks across 1,200 instances.
That is the cheapest Phase 6 has ever looked, and also the most clearly bounded.

## What the prototype cannot tell you

- **Nothing about a 128-call batch.** The oracle is factorial, so instances are
  4–7 calls. Headroom may scale differently, and the argument must not be read as
  if it did.
- **Nothing about cost.** These are TypeScript serial passes. The engine's DELS
  placement, arena use and wasm bytes are unmeasured here;
  `docs/OPTIMIZATION-BUDGETS.md` covers memory, and latency is M8 work.
- **No optimality claim from the bound.** The bound equals the optimum on only 15%
  of `multiresource`, so on the family that matters most, `Gap_LB` overstates how
  far from optimal a schedule is. A win here is against a real optimum, but a
  *non-win* in production would not be detectable from the bound alone.
- Serial GS with a given priority is not the same generator as the engine's
  parallel GS, which is why row 1 of the table beats row 2 of people's
  expectations.

## Consequences carried into the other documents

`docs/OPTIMIZATION-RFC.md` §6 records `critical-path` and `resource-aware` as the
only two non-baseline rules with measured value, `hybrid` as measured-worthless,
and the baseline's permanent candidacy as empirically load-bearing. §11's P6-D2
and P6-D3 are now decisions with evidence attached rather than open questions.

## Related documents

* `docs/OPTIMIZATION-BENCHMARKS.md` — the corpus, and how much headroom exists.
* `docs/OPTIMIZATION-RFC.md` — the objective a strategy is scored under.
* `docs/OPTIMIZATION-BUDGETS.md` — what a candidate costs in bytes.
* `docs/RESOURCE-SCHEDULING.md` — the baseline being competed against.
