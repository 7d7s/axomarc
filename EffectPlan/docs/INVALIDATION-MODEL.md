# Invalidation model

How EffectPlan decides which part of a plan a change invalidates.

## 1. The rule

**A change at position `p` can change every dependency from `p` onwards.**

That is not a conservative guess; it is what the conflict rules mean. Whether
call `k` depends on call `j` depends on what happened between them: a write
makes later readers wait for it, a later write makes earlier readers wait for
it, and the barrier resource makes every call wait for every exclusive one
before it. Change any call and the history it was part of is different.

So the invalidation boundary is the earliest call any change reaches, and
everything from there on is replanned.

## 2. What is *not* invalidated

The prefix before the boundary is not merely assumed to be valid — it is
*proved* to be valid, by the engine, against the input prefix:

* the counters in the restored state must be within what the prefix could have
  produced;
* every `last_writer` must name a call the prefix already contains;
* every reader link must point at an older node;
* the digest must match the prefix and the state together.

If any of that fails, the session does not trust the prefix: it discards the
checkpoint and plans the whole batch. The fallback is not an error — the result
is identical either way, only the cost differs, and the diagnostics say which
happened.

## 3. The three shapes of change

```
append at the end
  ├─ the prefix is the whole batch      → reuse everything, plan one call
  └─ the suffix is empty

replace or remove at position p
  ├─ the prefix is [0, p)                → reuse it, replan [p, n)
  └─ the suffix is everything after p

several changes
  └─ the boundary is the earliest of them
```

A removal renumbers the calls after it. That is not a problem for the reused
prefix, because the prefix's calls keep their positions — only the suffix moves
— and the engine's digest ties the prefix to the *inputs*, which are unchanged
in the prefix.

## 4. Where the checkpoints are

A session keeps a checkpoint every `checkpointInterval` calls (default 64). The
boundary an update resumes from is the newest checkpoint that is **at or before**
the invalidation boundary:

```
calls:      0    32    64    96   128
checkpoints:     ●     ●     ●     ●
change at 90 ──► the newest checkpoint at or before 90 is 64
                ⇒ reuse [0, 64), replan [64, 128)
```

A change at 95 and a change at 70 both reuse the same checkpoint, because
neither can be helped by a checkpoint that lies *after* it.

## 5. The adaptive question

The specification asks whether a suffix rebuild is worth it at all, and says the
answer must come from measurements rather than a percentage. The measurements
are in `docs/INCREMENTAL-BENCHMARKS.md`, and they say something blunt: on these
shapes the *host-side* work — validating, packing, decoding and copying the
checkpoint — costs about as much as the engine's dependency pass, so a suffix
rebuild saves the engine's share and little else.

That does not make the mechanism useless: it makes it worthwhile exactly where
the dependency pass is expensive (dense, wide batches with many conflicts) and
barely visible where it is cheap (a handful of calls). The session reports
which happened, per update, so a caller can see the difference rather than guess.

## 6. The fallback rules

| Situation | Strategy |
| --- | --- |
| only appends, and the previous end checkpoint is usable | `append` |
| a change at `p`, and a checkpoint exists at or before `p` | `suffix-rebuild` |
| no checkpoint at or before `p` | `full-rebuild` |
| a checkpoint was refused by the engine | `full-rebuild`, and the reason is recorded |
| the change is a reorder | not supported; `reset()` is the way |

## 7. Related documents

* `docs/CHECKPOINT-ARCHITECTURE.md` — what a checkpoint holds.
* `docs/INCREMENTAL-BENCHMARKS.md` — the measurements behind these rules.
* `docs/UPDATE-TRANSACTIONS.md` — atomicity.
