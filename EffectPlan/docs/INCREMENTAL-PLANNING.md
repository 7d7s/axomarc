# Incremental planning

EffectPlan can keep a plan alive while the workload behind it changes.

## 1. What it is

A **session** holds a plan and the identity of the calls in it. The caller
applies **transactions** — appends, replacements and removals — and each
committed transaction publishes a new immutable **snapshot** with a **version**.

```ts
const session = planner.createSession([
  { key: "task-a", call: callA },
  { key: "task-b", call: callB },
]);

const first = session.current();                  // version 1
const second = session.applyChanges({
  expectedVersion: first.version,                 // refuse anything else
  changes: [{ type: "append", key: "task-c", call: callC }],
});                                                // version 2
```

The session decides *where* to resume and the engine decides *what* is true.
It never computes a dependency, a wave or a duration.

## 2. The update model

| Change | Meaning | Typical cost |
| --- | --- | --- |
| `append` | a new call at the end | the suffix is empty: one call is planned |
| `replace` | a different call in the same slot | the prefix before it is reused, the rest replanned |
| `remove` | a call disappears | the prefix before it is reused, the rest replanned and renumbered |

Reordering is **not** supported: it can change every conflict relationship in the
batch, so `reset()` is the way to express it.

## 3. What is guaranteed, and what is not

**Guaranteed.**

* An incremental plan equals a plan of the same batch built from scratch. This is
  the phase's central property and it is tested three ways: a fixed set of
  scenarios, a randomised differential test over 60 sequences of edits, and a
  comparison against the stateless API for every benchmark shape.
* A refused transaction changes nothing: the previous snapshot, its version and
  its keys are exactly as they were.
* A version is never reused. A transaction that names a version the session has
  moved past is refused.
* The engine's result is byte-identical to a fresh plan: the same graph, the same
  waves, the same schedule.
* A restored state that does not belong to the batch is refused by the engine
  before a single call is planned, and the session falls back to planning the
  whole batch and says so.

**Best-effort optimisation.**

* *Reusing the prefix.* Whether an update can reuse a checkpoint depends on
  where the change lands and how often checkpoints are taken. The session reports
  what it did: `update.strategy` is `append`, `suffix-rebuild` or `full-rebuild`,
  and `update.reusedPrefixCalls` says how much was reused.
* *Checkpoint density.* `checkpointInterval` (default 64) trades host memory for
  the length of the suffix that has to be replanned. A denser interval means more
  state and a shorter rebuild; `docs/INCREMENTAL-BENCHMARKS.md` has the
  measurements.

**Full-rebuild fallback.**

* When no checkpoint describes the unchanged prefix — the change is near the
  front, the interval is coarse, or a checkpoint was refused — the session plans
  the whole batch. The result is identical; only the cost differs.

**Not supported.**

* Reordering, inserting in the middle, moving a call.
* More than one session on one engine instance at a time (the session restores
  state into the engine's own regions).
* Sessions that outlive the engine instance, or that share state between processes.
* Persistence: a session lives in memory, in the host.

## 4. Stable keys

A call's identity in a session is its **key**, not its position. Keys are
caller-supplied strings, unique within a session, and they survive a removal:
removing `task-b` from `a, b, c, d` leaves `a, c, d` with their own keys, even
though their positions moved.

Keys never reach the engine. They live in the host, where they belong, and the
engine sees positions — which is why a removal renumbers the calls after it and
the plan is still correct.

## 5. Resource numbering

A session freezes its interning table: the first time a resource is seen it gets
a number, and that number never changes for the life of the session, even if the
call that introduced it is removed.

This matters more than it sounds. A fresh plan numbers resources by first
appearance, so removing an early call renumbers the resources of every later
call — and a plan that was going to be *reused* would suddenly describe different
resources while looking byte-identical. Freezing the table removes that class of
bug entirely. Gaps are legal: the engine only requires that a resource index be
below the count, and the count is the high-water mark.

The consequence for callers: an incremental plan and a fresh plan of the same
batch describe the same graph with possibly different resource *numbers*. Compare
the structure, not the numbers.

## 6. Where the state lives

The session's state is in the **host**, not in the arena. The engine stays
stateless between calls: a checkpoint is a snapshot of regions the engine already
owns — the conflict frontier, the event nodes, the emitted records — plus the
engine's own commitment about what they mean.

That keeps the WebAssembly artifact at 17 pages of linear memory, and it makes
the session's failure behaviour simple: a refused update cannot have touched the
committed state, because the committed state was never in the arena to begin
with. `docs/CHECKPOINT-ARCHITECTURE.md` has the details.

## 7. Cost

An update is never more expensive than a rebuild by much, and usually less: the
engine plans only the suffix, while the host does the same validation, packing
and decoding either way. `docs/INCREMENTAL-BENCHMARKS.md` has the measured
numbers, and the harness fails when an update near the end of a batch is *not*
cheaper than rebuilding the whole thing.

## 8. Related documents

* `docs/CHECKPOINT-ARCHITECTURE.md` — what a checkpoint holds and how it is restored.
* `docs/PLAN-VERSIONING.md` — versions, snapshots and the session lifecycle.
* `docs/UPDATE-TRANSACTIONS.md` — atomicity and what a failure leaves behind.
* `docs/INVALIDATION-MODEL.md` — how the affected suffix is chosen.
* `docs/INCREMENTAL-BENCHMARKS.md` — the measurements.
* `docs/INCREMENTAL-SECURITY.md` — what happens when the restored state is hostile.
