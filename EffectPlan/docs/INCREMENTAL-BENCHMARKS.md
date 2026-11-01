# Incremental benchmarks

What an update costs against rebuilding the whole plan.

## 1. What is measured

`npm run benchmark:incremental` measures, in one process on one machine:

* a **full rebuild** — `planBatch(calls)`, which is what a caller without a
  session does;
* an **update** — one transaction on a session, with the session's creation and
  its first plan outside the timed region, so only the update is measured;
* the **strategy** the session chose and how many calls it reused;
* a **checkpoint interval sweep** — the same edit with the interval set to 16,
  32, 64 and 128.

The report is written to `artifacts/incremental-benchmarks.json`.

Measured 2026-11-01, Node v20.15.0, Intel i5-10300H, p50 microseconds.

## 2. The results

| Workload | Rebuild (µs) | Append | Replace last | Remove last | Replace middle |
| --- | --- | --- | --- | --- | --- |
| tiny (8 calls) | 151 | 91% | 63% | 63% | 68% |
| small (32) | 266 | 83% | 71% | 63% | 69% |
| normal (64) | 334 | 62% | 88% | 71% | 75% |
| **medium (128, 1,259 edges)** | **4,381** | — | **20%** | **17%** | **20%** |
| wide (64, no edges) | 138 | 98% | 117% | 81% | 118% |
| deep (64, 64 waves) | 160 | 97% | 85% | 91% | 95% |
| diamond (64) | 130 | 120% | 97% | 124% | 106% |
| barrier (64) | 300 | 68% | 72% | 72% | 76% |
| conflict (64) | 363 | 51% | 95% | 93% | 83% |
| **maxbound (128, 1,152 edges)** | **1,396** | — | **61%** | **58%** | **62%** |
| mixed (96) | 640 | 37% | 42% | 44% | 71% |

The percentages are the update's p50 as a share of the rebuild's p50. An append
on a full batch is skipped where the batch is already at the engine's call
ceiling.

## 3. What the numbers say

**Where it pays.** The dense shapes, where the dependency pass is the expensive
part: `medium` updates cost a fifth of a rebuild, `mixed` a third to a half,
`maxbound` three fifths. The saving is the dependency pass the session did not
repeat.

**Where it does not.** The sparse shapes — `wide` has no dependencies at all,
`deep` has one per level — rebuild in 130–160 µs, which is less than the
bookkeeping an update does: copying a checkpoint in, copying one out, and
rebuilding the candidate. An update there can cost *more* than a rebuild, and the
measurements show exactly that.

That is the honest shape of the feature: incremental planning pays for itself
when planning is expensive, and it is not free when it is not. The session
reports the strategy and the reuse count on every update, so a caller can see
which case they are in rather than assume.

**Why the strategy is what it is.** With the default interval of 64, a 64-call
batch has exactly one checkpoint — at its end — so an edit anywhere but the very
end cannot reuse anything and the update is a rebuild. `medium` and `maxbound`
have 128 calls, so they get a checkpoint in the middle and their suffix rebuilds
start there.

## 4. The checkpoint interval

The sweep, on `normal` (64 calls) and `mixed` (96), replacing a call in the
middle:

| Interval | Calls reused | Cost |
| --- | --- | --- |
| 16 | 32 | 402 µs |
| 32 | 32 | 247 µs |
| 64 | 0 | 316 µs |
| 128 | 0 | 287 µs |

A denser interval means a shorter suffix to replan and more state to keep: each
checkpoint holds the 17,416-byte workspace and the records emitted so far. The
first row of each sweep carries the cost of the first run through that path and
is not comparable with the rest.

The trade-off is real but modest: the difference between the best and the worst
interval is tens of microseconds, against a rebuild of hundreds. **64 is kept as
the default** because it keeps the number of checkpoints small, and because the
measurement does not show a dense interval buying anything an order of magnitude
larger than it costs.

## 5. The gate

The harness fails when an update near the end of a batch, or an append, is *not*
cheaper than rebuilding the whole thing — on a workload whose rebuild takes at
least 250 µs.

Below that, the run-to-run noise of this harness is larger than the effect
being measured: the same code measured 80% on one run and 116% on the next for
the same workload. Gating there would be gating on noise, so the small shapes are
reported and not gated. The gate is about the *representative* workloads, where
the effect is an order of magnitude above the noise.

## 6. What the measurement does not cover

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

## 7. Related documents

* `docs/INCREMENTAL-PLANNING.md` — the capability and its guarantees.
* `docs/INVALIDATION-MODEL.md` — how the boundary is chosen.
* `docs/CHECKPOINT-ARCHITECTURE.md` — what a checkpoint holds.
