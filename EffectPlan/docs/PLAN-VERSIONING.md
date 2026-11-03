# Plan versioning

Versions, snapshots and the life of a session.

## 1. The version rule

| Situation | Version |
| --- | --- |
| a session is opened | 1 |
| a transaction is committed | the previous version + 1 |
| a transaction is refused | unchanged |
| a transaction names a version the session has passed | refused |
| a transaction carries no changes | unchanged |
| the session is reset | the previous version + 1 |

The version is a **monotonic counter**, not a state identifier. It never goes
backwards, it is never reused, and a reset does not restart it: a caller holding
a snapshot from before the reset still cannot apply it, because the session has
moved on.

## 2. Why versions at all

Two consumers can hold the same plan. If one of them updates it, the other's
copy is stale — and applying an update computed from the stale copy would
silently discard the first update.

The version makes that impossible to do by accident:

```ts
const snapshot = session.current();          // version 7
// ... somewhere else ...
session.applyChanges({ expectedVersion: 7, changes: [...] });   // commits, version 8
// ... the other consumer, still holding version 7 ...
session.applyChanges({ expectedVersion: 7, changes: [...] });   // refused: EP_STALE_VERSION
```

The error is `EP_STALE_VERSION`, and nothing about the session changes when it
is thrown.

## 3. Snapshots

A snapshot is what `current()` returns and what `applyChanges()` returns:

| Field | Meaning |
| --- | --- |
| `version` | the version this plan was committed at |
| `keys` | the stable keys, in batch order |
| `callCount`, `edgeCount` | the shape of the plan |
| `plan` | the plan itself: edges, waves, and the schedule when timings were asked for |
| `update` | what the session did to get here |

A snapshot is **immutable and self-contained**. It holds a private copy of the
graph, so the engine's arena can be reused immediately afterwards — the same
property `planBatch()` has had since Phase 1. A snapshot stays valid whatever
happens to the session afterwards: the plan it holds was true when it was
committed, and the session's later versions do not change it.

### The key contract

A key is the caller's name for a call, and the session's handle on it across
transactions. Three rules, all enforced while the transaction is still
hypothetical:

* **at most 128 UTF-8 bytes**, counted in bytes rather than code units, so a key
  of 65 two-byte characters is refused where 64 are allowed. A key never reaches
  the engine, so this bound is the host's own: it keeps the session's per-update
  bookkeeping from becoming the expensive part of an update;
* **unique within the session**, and unique within one changeset — two changes
  naming the same key is refused rather than resolved by order;
* **never echoed**. A refusal about a key names its position and the rule, never
  the key, and a snapshot's `keys` are only ever the caller's own strings.

A replacement keeps the key and changes the call; the key is the identity, the
call is the content. That is why `remove` then `append` of the same key is a
different transaction from `replace`, and why a resource id that a call reuses
across a replacement cannot renumber the prefix it shares.

The session's diagnostics are part of the snapshot, so a caller can always ask
"how much of this was reused?" without keeping a log.

## 4. The session lifecycle

```
open ──▶ current() ◀── applyChanges() ──▶ current() ──▶ …
  │                                                      │
  └──────────────── reset() ────────────────────────────┘
  │
  └──────────────── dispose() ──▶ every later call throws EP_SESSION_DISPOSED
```

* **open** — `createSession(calls, options)` validates and normalises the calls,
  plans the batch and publishes version 1.
* **applyChanges** — validates, plans and publishes.
* **reset** — starts a new generation: a fresh interning table, no checkpoints,
  a fresh plan. The version keeps counting up, so a snapshot from before the
  reset cannot be applied to the session afterwards.
* **dispose** — drops the calls, the checkpoints and the snapshot. Any later use
  throws `EP_SESSION_DISPOSED`. Disposing is explicit because a session holds
  host memory — the checkpoints are tens of kilobytes each — and a forgotten
  session would keep them alive.

## 5. Concurrency

A session is **not** thread-safe and does not pretend to be. Two concurrent
`applyChanges` calls on one session would race on the version check and on the
arena. The contract is: one session, one caller, serialised updates.

Independent sessions are independent — but they need independent engines, because
a session restores state into the engine's regions. Two sessions on one engine
instance would overwrite each other's state. The wrapper does not stop you: it
is the same class of mistake as planning a batch on an engine whose session is
live, and the failure is a wrong plan rather than a crash.

The safe pattern is one engine per session, and `EffectPlan.create()` is cheap
enough (about half a millisecond) for that to be the default.

## 6. Errors a session can raise

| Code | When |
| --- | --- |
| `EP_STALE_VERSION` | the transaction names a version the session has passed |
| `EP_DUPLICATE_KEY` | two changes name the same key, or an append reuses a key |
| `EP_UNKNOWN_KEY` | a replacement or removal names a key the session does not have |
| `EP_INVALID_CHANGESET` | the changeset is not an array of known changes, a key is empty, or a key is over 128 UTF-8 bytes |
| `EP_SESSION_DISPOSED` | the session has been disposed |
| `EP_INVALID_INPUT` | a call, a duration or a mode violates the public contract |
| `EP_LIMIT_EXCEEDED` | the batch would exceed the engine's call, access or resource ceilings |
| `EP_ENGINE_ERROR` | the engine refused the batch for a reason the host cannot name |

None of them carries a caller's key or resource identifier in its message.

## 7. Related documents

* `docs/UPDATE-TRANSACTIONS.md` — what a refused transaction leaves behind.
* `docs/INCREMENTAL-PLANNING.md` — the capability and its guarantees.
* `docs/INVALIDATION-MODEL.md` — how the affected suffix is chosen.
