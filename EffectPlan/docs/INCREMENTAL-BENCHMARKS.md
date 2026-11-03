# Incremental benchmarks

What an update costs against rebuilding the whole plan.

## 1. What is measured

`npm run benchmark:incremental` measures, in one process on one machine:

* an **update** — one transaction on a session, applied to a session created for
  that sample and disposed straight after it;
* **the rebuild that transaction replaces** — `planBatch(after)`, where `after` is
  the batch the update produces. Not a rebuild of the batch the update *started*
  from: a caller who is choosing between an update and a rebuild is choosing
  between two ways of planning the new batch, and comparing an append against a
  rebuild of the pre-append list compares two different amounts of work;
* the **strategy** the session chose, and how many calls it reused and recomputed;
* a **checkpoint interval sweep** — the same edit with the interval at 16, 32, 64
  and 128;
* **a hundred successive updates** on one session, each replacing the last call
  with a write to a rotating resource, timed per update and checked at the end
  against a fresh rebuild of the resulting batch. The check is not a timing: an
  update that disagreed with a rebuild would fail the run.

Both sides are warmed for 15 rounds and sampled 25 times, alternating — one
rebuild, one update, repeat — so machine drift lands on both sides of a
comparison instead of only one. Session creation and the plan that fills a
session are outside every timed region. The report goes to
`artifacts/incremental-benchmarks.json`, including the gate floor, how many cases
it gated, and the fallback parity of §3.

Measured 2026-11-02, Node v20.15.0, Intel i5-10300H, p50 microseconds.

## 2. The results

Percentages are the update's p50 as a share of the rebuild of the batch it
produces. `*` marks a case the gate enforces (§5).

| Workload | Rebuild (µs) | App 1 | App 10 | App 100 | Repl last | Rem last | Repl mid | Rem mid | Repl first | Multi | 100 updates (µs each) |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| tiny (8) | 31 | 169% | 140% | 237% | 178% | 162% | 160% | 167% | 159% | 173% | 44, no reuse |
| small (32) | 83 | 75% | 79% | — | 97% | 100% | 99% | 100% | 97% | 103% | 79, no reuse |
| normal (64) | 157 | 54%\* | 59%\* | — | 85% | 84% | 81% | 84% | 86% | 84% | 125, no reuse |
| **medium (128)** | **679** | — | — | — | **55%\*** | **56%\*** | 56% | 56% | 74% | — | **372, reusing** |
| wide (64) | 67 | 72% | 75% | — | 73% | 73% | 74% | 73% | 72% | 82% | 45, no reuse |
| deep (64) | 54 | 107% | 112% | — | 115% | 121% | 115% | 118% | 114% | 121% | 60, no reuse |
| diamond (64) | 72 | 75% | 77% | — | 83% | 85% | 79% | 84% | 81% | 86% | 57, no reuse |
| barrier (64) | 99 | 68%\* | 71%\* | — | 99% | 101% | 97% | 99% | 99% | 101% | 95, no reuse |
| conflict (64) | 170 | 52%\* | 54%\* | — | 82% | 83% | 81% | 83% | 81% | 83% | 134, no reuse |
| **maxbound (128)** | **652** | — | — | — | **55%\*** | **56%\*** | 56% | 56% | 70% | — | **357, reusing** |
| mixed (96) | 203 | 44%\* | 47%\* | — | **58%\*** | **58%\*** | 78% | 80% | 78% | 81% | **116, reusing** |

An append is skipped where the batch is already at a ceiling: `medium` and
`maxbound` are at 128 calls, and the 64-call shapes at 1,024 accesses cannot take
a hundred more. `multi-update` is one transaction that replaces a middle call,
removes the last and appends two.

## 3. What the numbers say

**Reuse pays, and pays most where planning is expensive.** On the 128-call
shapes an update is a little over half a rebuild (55–56%), on `mixed` two thirds,
on `normal` and `conflict` four fifths for a middle edit and a half for an
append. The saving is the dependency pass the session did not repeat, so it grows
with the edges the prefix has already settled — `medium` carries 1,259 edges and
`maxbound` 1,152.

**Append is the cheapest thing to do to a plan.** Every append is at or below
80% of a rebuild wherever there is a prefix to reuse, and appending ten calls
costs about the same as appending one (52→54%, 68→71%): the transaction is priced
by the suffix it replans, not by how much arrived.

**When nothing is reused, an update is roughly a rebuild — sometimes cheaper.**
Across the 54 cases where the session could not reuse anything, the update cost a
median 85% of the rebuild it fell back to (min 70%, p95 162%). Cheaper, because
the session already holds an interned resource table and normalised calls, so the
fallback skips part of the host-side work a fresh `planBatch` does. That is the
worst case of this feature and it is close to neutral, which is the point of
falling back rather than refusing.

**It does not pay on a plan that was never expensive.** `deep` — 64 calls, one
edge per level — lands at 107–121% everywhere, and `tiny` at 140–237%. With
almost nothing to reuse, the fixed bookkeeping of an update is larger than the
whole rebuild. `wide` and `diamond`, sparse but wider, land at 72–86%: a rebuild
there is so cheap that the session's saving is invisible.

**A hundred updates in a row stay flat.** On the shapes that reuse, the per-update
p50 across the sequence equals the single-update cost (`medium` 372 µs against
374 for an isolated update, `maxbound` 357 against 356): no drift, no growing
bookkeeping, and the plan after the hundredth update is byte-equal in structure to
a fresh rebuild of the same batch. `medium` and `maxbound` also keep reusing
`64` calls at update 100 — reuse does not decay with history (it used to; §6).

**Why the strategy is what it is.** With the default interval of 64, a 64-call
batch has exactly one checkpoint — at its end — so an edit anywhere but the very
end cannot reuse anything and the update is a rebuild. That is why the
"no reuse" column above says 64 calls: those shapes are paying the documented
consequence of the interval, and §4 shows what changing it buys.

## 4. The checkpoint interval

The sweep, replacing a call in the middle (p50 µs, calls reused in brackets):

| Interval | normal (64) | medium (128) | maxbound (128) | mixed (96) |
| --- | --- | --- | --- | --- |
| 16 | 166 (32) | 754 (64) | 712 (64) | 238 (48) |
| 32 | 115 (32) | 510 (64) | 490 (64) | 197 (32) |
| **64** | 126 (0) | **384 (64)** | **366 (64)** | **160 (0)** |
| 128 | 127 (0) | 496 (0) | 460 (0) | 160 (0) |

On the dense shapes the interval is worth choosing, and the ordering is the same
in every run: for `medium`, 64 is half the cost of 16 and 23% better than 128.
Two effects pull against each other. A denser interval has more checkpoints to
*capture* — each one is a copy of the 17,416-byte workspace plus the records
emitted so far, and an update that replans a suffix captures along it. A coarser
interval carries less state but leaves a longer suffix, and at 128 on a 128-call
batch the only checkpoint sits at the end, so a middle edit reuses nothing.

On the 64-call shapes the interval is not the variable that matters: `normal`
ranged 115–166 with no consistent ordering, which is this harness's noise on a
shape whose entire rebuild is 157 µs, not a property of the interval.

**64 stays the default.** It is the best measured point on the shapes where the
choice matters, and it keeps the checkpoint count — and the host memory behind it
— low.

## 5. The gate

One claim is enforced: **an update that reuses a prefix must cost no more than
planning the resulting batch from scratch.** Four conditions, per case:

* the transaction lands at the end of the batch (an append, or a replace/remove
  of the last call);
* the batch has at least 64 calls;
* it has 96 calls or more — where the dependency pass dominates — or its rebuild
  costs at least three times the eight-call rebuild *measured in the same run*;
* and the update actually reused something.

The floor is relative on purpose: an absolute threshold in microseconds moves with
machine load, and a gate that quietly stops applying on a fast machine — or starts
applying to `tiny` on a busy one — is worse than no gate. The shape rule keeps the
claim alive where a ratio between a 64-call and a 128-call rebuild narrows. The
reuse condition is not an escape hatch: a transaction that falls back to a full
rebuild *is* a rebuild plus the cost of discovering that, so "cheaper than a
rebuild" is false for it by construction — its parity is measured and reported
instead (§3). A run that gates fewer than six cases fails rather than passing
quietly. This run gated 14, none over.

`deep` and `tiny` are excluded by these rules and reported precisely because they
are where the effect is negative; asserting either direction there would be
asserting the harness's own overhead.

## 6. Defects that measurement found

Three of them, and each was invisible to the tests. The engine's and session's
correctness never depended on any of these — `Incremental(G,Δ) = FullRebuild(G+Δ)`
is proved by tests, and every number below was re-checked after each fix — but all
three were in what this repository *reports*.

**A cold baseline flattered the update.** The harness first warmed three times and
took the median of ten samples, and measured every rebuild before any update. The
warm-up transient outlasted the samples: the same `planBatch` call, in the same
process, measured 828 µs over ten samples and 184 µs over two hundred on `normal`.
`medium`'s rebuild was reported at 4,381 µs where a warmed run gives 740. What that
run called "a fifth of a rebuild" is about a half (§2). Warmed, sampled 25 times,
alternated: fixed.

**An update that grew the batch was compared against a rebuild that did not.**
With one per-workload baseline, `tiny`'s append of a hundred calls reported 500% —
the cost of planning 108 calls against the cost of planning 8. Each case now
carries its own `planBatch(after)` baseline, and the same number is 237%.

**Reuse died after the first update.** The 100-successive-update stress — added
because the specification asked for repeated updates, not because anything
suggested it was broken — showed a session falling back to a full rebuild on
every update after the first, with no reason reported and every result still
correct. A replanning run replaced the session's whole checkpoint set with only
what that run had captured, discarding the prefix checkpoint the *next* update
needed. Nothing in 180 tests caught it: the session tests apply one update, the
property tests compare plans against a rebuild and never look at the strategy, and
the fallback was silently indistinguishable from a correct slow path. Fixed by
keeping the checkpoints at or below the boundary a run resumed from (they still
describe the prefix, and the engine's digest check still refuses one that does
not), and pinned by a test that updates the same session three times and asserts
it reuses 64 calls each time. `maxbound` went from 439 µs to 357 µs per update,
and the successive row from `full-rebuild` to `suffix-rebuild`.

The pattern is worth keeping: the two defects that mattered were found by the
stress cases the specification asked for, and one of them was only visible because
the harness reported *which strategy it had chosen*, not just how long it took.

## 7. What the measurement does not cover

* **The host-side cost dominates the small shapes.** Validating, packing and
  decoding the plan is the same work in an update and in a rebuild, so the
  engine's saving is only part of the difference. This is why ratios approach 100%
  on cheap batches rather than approaching zero.
* **The first plan of a session is a full rebuild**, plus the checkpoints it
  takes. It is outside the timed region here, and it is the price of opening a
  session.
* **A session does not help a caller who plans one batch and throws it away.**
* **The numbers are from one machine.** The ratios are the durable part; the
  absolute microseconds are not — `medium`'s rebuild measured 673 µs to 1,549 µs
  across runs of the same build at different loads, while its ratio stayed at
  50–57%.
* **Sessions holding more than a hundred updates.** The stress stops at 100; the
  property tests run 60 randomised edit sequences and check the plan, not the cost.

## 8. Related documents

* `docs/INCREMENTAL-PLANNING.md` — the capability and its guarantees.
* `docs/INVALIDATION-MODEL.md` — how the boundary is chosen.
* `docs/CHECKPOINT-ARCHITECTURE.md` — what a checkpoint holds.
