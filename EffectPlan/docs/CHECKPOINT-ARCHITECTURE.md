# Checkpoint architecture

What a checkpoint holds, where it lives, and why it is safe to restore.

## 1. The state a plan carries

The engine's planner is a streaming pass over the calls, and its whole state is
two arrays plus two counters:

| State | Size | Meaning |
| --- | --- | --- |
| `last_writer` | `1,025 × u32` | the last call that wrote each resource, `u32::MAX` for none |
| `reader_head` | `1,025 × u32` | the front of each resource's pending-reader list, `0` for empty |
| `event_op`, `event_next` | `2 × 1,152 × u32` | the event nodes: which call owns a node, and the next node of its list |
| `events` | counter | how many event nodes exist |
| `edge_count` | counter | how many records have been emitted |

That is the complete state. There is no other live structure: the dependency
graph is the output region, and the input arrays are the caller's.

## 2. Why a snapshot of the heads is enough

A reader list is an intrusive chain: `event_next[i]` points at the next node of
the same resource's list, and a read pushes a node onto the front. The links are
written **once**, when the node is appended, and never rewritten afterwards —
`process_write` reads the chain and clears the head, but it does not touch a
node's link.

So the links of a prefix stay valid forever. Restoring the frontier means
restoring the two head arrays and pointing at the node array that is already
there: **no list is ever copied**. This was the open question in the design
("can the reader-list links remain valid in unchanged prefix records?") and the
answer is yes, because the engine never rewrites them.

## 3. What a checkpoint is

A checkpoint is three byte ranges and one commitment:

| Part | Size | Source |
| --- | --- | --- |
| the workspace region | 17,416 B | `last_writer`, `reader_head`, `event_next`, `event_op` |
| the emitted records | `edges × 16 B` | the output region |
| the record | 32 B | the engine's own commitment (below) |

The **record** is eight words:

| Word | Meaning |
| --- | --- |
| `revision` | `1`; `0` means "no checkpoint, plan the whole batch" |
| `calls` | how many calls the state covers |
| `events` | how many event nodes the state holds |
| `edges` | how many records the state holds |
| `digest` | the commitment (below) |
| `flags` | `1` when the run declines the scheduling pass |
| two reserved words | always zero |

The engine writes the record *last*, after the state it describes. A reader that
finds `revision == 0` knows no checkpoint was published, which is what makes a
refused run safe: the record is cleared before the run starts, and only written
again when the run succeeds.

## 4. The digest

The digest is what makes a restored state *provably* the state it claims to be.
It covers both sides of the claim:

* the **input prefix** — the call modes, the CSR offsets, the resource ids and
  the access modes of the first `calls` calls; and
* the **state** — the frontier, the event nodes, the records and the counters.

Both are folded into one 32-bit value with FNV-1a over 32-bit lanes. Change one
byte of the prefix and the digest disagrees; change one byte of the frontier and
it disagrees; hand it a snapshot from a different batch and it disagrees. In all
three cases the engine refuses with `131` **before** it plans anything.

Thirty-two bits is a commitment, not a proof against a deliberate adversary — a
collision is possible. It is the same trade the engine makes everywhere else
(checked pointers, canonical regions): the threat model is a buggy or confused
host, not an attacker who can choose bytes. A hostile caller who can write the
arena directly is already inside the trust boundary; `docs/INCREMENTAL-SECURITY.md`
says what the engine still checks in that case.

## 5. Where checkpoints live

In the **host**, as plain byte arrays. Not in the arena.

The arena is 75,336 bytes and holds the engine's regions; linear memory is 18
pages and the last 54,352 bytes of the address space are the slack Phase 5's
resource regions left behind. A single
full snapshot of the frontier is 8,200 bytes and a journal of every frontier
mutation would be 9,216 more — neither fits, and a checkpoint every 32 calls
would need hundreds of kilobytes.

Keeping the state in the host instead means:

* the WebAssembly artifact does not grow by a session's worth of memory;
* a failed update cannot have corrupted the committed state, because the
  committed state was never in the arena;
* the session's memory is the host's to account for, and it is bounded and
  measurable.

The cost is a copy in and a copy out per update — measured at 0.5 µs for the
frontier and 1.5 µs for a full 36,880-byte record region — against a rebuild
that costs 67–344 µs for the same shapes.

## 6. Taking checkpoints

A session plans a batch in **segments**. The first segment plans from nothing;
each later segment resumes from the previous segment's checkpoint. The segments
are not extra work: the batch is planned once, and the checkpoints fall out of
the way.

```
calls:   0        32        64        96       128
         |---------|---------|---------|---------|
         segment 1  segment 2  segment 3  segment 4
         └── checkpoint ──┘ └── checkpoint ──┘ ...
```

`checkpointInterval` (default 64) is the segment length. Each boundary keeps one
snapshot in the host: the workspace region, the records emitted so far, and the
record.

**Which snapshots survive an update.** A run that resumes from boundary *b* leaves
everything at or below *b* exactly as it found it, so the snapshots at or below *b*
stay valid and are kept; those above *b* describe state that was just replanned and
are dropped. Getting this wrong is the difference between a session that can reuse
a prefix and one that can only ever do it once: if a replanning run replaces the
whole snapshot set with only what it captured itself, the next update finds nothing
to resume from and rebuilds — correctly, and slowly, with no error to show for it.
That is what the first implementation did, and the 100-successive-update stress in
`docs/INCREMENTAL-BENCHMARKS.md` §6 is what found it.

After a fallback, when a restored snapshot failed the engine's check, nothing old
is trusted: the rejected snapshot is dropped and only what the whole-batch run
captured is kept.

## 7. Restoring

Restoring is three writes and one call:

1. copy the workspace region back,
2. copy the records back,
3. copy the record back and set the flags word,
4. call the engine, which validates the restored state before planning.

The engine's validation is: the counters are within their ceilings and within
what the prefix could have produced; every `last_writer` names a call the prefix
already contains; every `reader_head` names an event the prefix already has; every
event's owner is a call of the prefix; every reader link points at an *older*
node, so a list cannot run forward; and the digest matches. Anything else is
`131` or `132`, and the session falls back to planning the whole batch.

## 8. The one thing the engine changed for this

The reader drain in `process_write` walked a list until it ended. That was safe
because the engine wrote the list and a list cannot contain a cycle. A restored
list is host data, so the drain is now bounded by the number of events, and the
validator refuses a chain that does not strictly recede.

## 9. Related documents

* `docs/INCREMENTAL-PLANNING.md` — the capability and its guarantees.
* `docs/INVALIDATION-MODEL.md` — which checkpoint an update resumes from.
* `docs/INCREMENTAL-SECURITY.md` — the validation in detail.
* `docs/ABI.md` — the record's bytes in the ABI.
