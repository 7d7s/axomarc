# Incremental benchmarks

What an update costs against rebuilding the whole plan.

## 1. What is measured

`npm run benchmark:incremental` measures, in one process on one machine:

* a **full rebuild** — `planBatch(calls)`, which is what a caller without a
  session does;
* an **update** — one transaction on a session, applied to a session that was
  created for that sample and disposed right after it;
* the **strategy** the session chose and how many calls it reused;
* a **checkpoint interval sweep** — the same edit with the interval set to 16,
  32, 64 and 128.

Both sides are warmed for 15 rounds and then sampled 25 times, alternating: one
rebuild, one update of each case, repeat. Session creation and the plan that
fills it are outside every timed region. The report is written to
`artifacts/incremental-benchmarks.json`, including the gate floor and how many
cases it actually gated.

Measured 2026-11-02, Node v20.15.0, Intel i5-10300H, p50 microseconds.

## 2. The results

Percentages are the update's p50 as a share of the rebuild's p50. `*` marks a
case the gate enforces (§5).

| Workload | Rebuild (µs) | Append | Replace last | Remove last | Replace middle |
| --- | --- | --- | --- | --- | --- |
| tiny (8 calls) | 66 | 108% | 105% | 105% | 114% |
| small (32) | 167 | 69% | 93% | 90% | 90% |
| normal (64) | 231 | 56%\* | 69%\* | 65%\* | 72% |
| **medium (128, 1,259 edges)** | **673** | — | **56%\*** | **56%\*** | **56%** |
| wide (64, no edges) | 72 | 82% | 81% | 80% | 80% |
| deep (64, 64 waves) | 55 | 118% | 124% | 125% | 124% |
| diamond (64) | 73 | 79% | 83% | 87% | 83% |
| barrier (64) | 115 | 62% | 87% | 90% | 88% |
| conflict (64) | 195 | 45% | 71% | 71% | 70% |
| **maxbound (128, 1,152 edges)** | **733** | — | **49%\*** | **49%\*** | **49%** |
| mixed (96) | 214 | 43%\* | 55%\* | 58%\* | 75% |

An append on a full batch is skipped where the batch is already at the engine's
call ceiling — `medium` and `maxbound` are 128 calls.

Three runs of this harness, on the same build, at three different machine loads:
`medium` 50–57%, `maxbound` 49–64%, `mixed` 36–58%, `normal` 44–82%, `deep`
117–127%. The ratios repeat to within a few points; the absolute microseconds do
not — the quietest run measured `medium`'s rebuild at 673 µs and the busiest at
1,549 — and they are not the claim.

## 3. What the numbers say

**Where it pays.** The dense shapes, where the dependency pass is the expensive
part: `medium` and `maxbound` updates cost about half a rebuild, `mixed` two
fifths to three fifths, `normal` and `conflict` a half to four fifths. The
saving is the dependency pass the session did not repeat, and it grows with the
number of edges the prefix has already settled.

**Where it does not.** `deep` (one edge per level) and `tiny` cost *more* as an
update than as a rebuild, by 17% to 27%, and `wide`/`diamond` save little. When
planning is cheap, the fixed bookkeeping of an update — validating the candidate,
copying a checkpoint in, copying one out — is the same order as the whole rebuild.
That is the honest shape of the feature: incremental planning pays for itself
when planning is expensive, and it is not free when it is not. The session
reports the strategy and the reuse count on every update, so a caller can see
which case they are in rather than assume it.

**Why the strategy is what it is.** With the default interval of 64, a 64-call
batch has exactly one checkpoint — at its end — so an edit anywhere but the very
end cannot reuse anything and the update is a rebuild. `medium` and `maxbound`
have 128 calls, so they get a checkpoint in the middle and their suffix rebuilds
start there.

## 4. The checkpoint interval

The sweep, replacing a call in the middle of the batch (p50 µs, calls reused in
brackets):

| Interval | normal (64) | medium (128) | maxbound (128) | mixed (96) |
| --- | --- | --- | --- | --- |
| 16 | 297 (32) | 758 (64) | 717 (64) | 241 (48) |
| 32 | 120 (32) | 508 (64) | 498 (64) | 277 (32) |
| **64** | 261 (0) | **378 (64)** | **355 (64)** | **159 (0)** |
| 128 | 132 (0) | 500 (0) | 452 (0) | 160 (0) |

On the dense shapes the interval is worth choosing, and the ordering is the same
in every run: for `medium`, 64 is half the cost of 16 and 24% better than 128;
for `maxbound`, 355 against 717 and 452. Two effects pull in opposite directions.
A denser interval has more checkpoints to *capture* — each one is a copy of the
17,416-byte workspace plus the records emitted so far, and an update that replans
a suffix captures along it. A coarser interval carries less state but leaves a
longer suffix, and at 128 on a 128-call batch the only checkpoint sits at the end,
so a middle edit reuses nothing and the update is a rebuild.

On the 64-call shapes the interval is not the variable that matters: `normal`
ranged 120–297 with no consistent ordering between the intervals, which is the
harness's own noise on a shape whose whole rebuild is 231 µs, not a property of the
interval.

**64 stays the default.** It is the best measured point on the shapes where the
choice matters, and it keeps the checkpoint count — and the host memory behind
it — low.

## 5. The gate

The harness enforces one claim: **an update at the end of a batch, or an append,
must not cost more than rebuilding the whole plan** — on the workloads big enough
for the claim to mean something. Three conditions decide that, per run:

* the batch has at least 64 calls;
* it has 96 calls or more, which is where the dependency pass dominates, or its
  rebuild costs at least three times the eight-call rebuild *measured in the same
  run*;
* and the run as a whole must gate at least six cases, or it fails.

The floor is relative on purpose. An absolute threshold in microseconds moves with
machine load, and a gate that quietly stops applying on a fast machine — or starts
applying to `tiny` on a busy one — is worse than no gate. The shape rule keeps the
claim alive on machines where the ratio between a 64-call and a 128-call rebuild
narrows, and the count assertion turns "the gate measured nothing" into a failure
instead of a green tick. This run gated 10 cases and none went over; the run on
the quietest machine gated 13.

`deep` and `tiny` are excluded by that rule and are reported precisely because
they are the cases where the effect is negative: asserting either direction there
would be asserting the harness's own overhead.

## 6. A measurement defect, found and fixed

The numbers in §2 are not the ones this harness first reported, and the reason
matters more than the table.

The original harness warmed three times and took the median of ten samples,
measured every rebuild *before* any update, and held each session it meant to
measure alive for the whole workload. In that shape:

* the warm-up transient outlasted the samples. The same `planBatch` call, in the
  same process, measured 828 µs over ten samples and 184 µs over two hundred on
  `normal`; the committed artifact reported `medium`'s rebuild at 4,381 µs where
  a warmed long run gives 740 µs.
* the rebuild side was therefore inflated two to four times, and the ratio
  inherited it. What that run called "a fifth of a rebuild" for `medium` is, when
  both sides are warmed and measured together, about a half.
* sessions left alive hold their checkpoint copies across the run, which slows
  the rebuild they are being compared against. Creating each session the moment
  before its own sample and disposing it after removed that too.

Nothing about the engine changed in fixing this, and no equivalence result
depends on these timings — `Incremental(G,Δ) = FullRebuild(G+Δ)` is proved by
tests, not by measurements. What changed is that the reported speed-up went down
by roughly half and became reproducible to a few points. The old result is on
record here because the defect was real: a cold baseline flatters whatever it is
compared against, and in this case it flattered the feature this repository is
building.

## 7. What the measurement does not cover

* **The host-side cost dominates the small shapes.** Validating, packing and
  decoding the plan is the same work in an update and in a rebuild, so the
  engine's saving is only ever part of the difference. This is why the ratios
  approach 100% on cheap batches rather than approaching zero.
* **The first plan of a session is a full rebuild**, plus the checkpoints it
  takes. It is outside the timed region here, and it is the price of opening a
  session.
* **A session does not help a caller who plans one batch and throws it away.**
  The whole point is the second update.
* **The numbers are from one machine.** The ratios are the durable part; the
  absolute microseconds are not.
* **One update per session.** Successive updates on one session re-enter the same
  path with more history; repeated updates are covered by the property tests, not
  by these timings.

## 8. Related documents

* `docs/INCREMENTAL-PLANNING.md` — the capability and its guarantees.
* `docs/INVALIDATION-MODEL.md` — how the boundary is chosen.
* `docs/CHECKPOINT-ARCHITECTURE.md` — what a checkpoint holds.
