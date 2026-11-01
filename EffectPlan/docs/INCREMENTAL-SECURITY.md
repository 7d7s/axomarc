# Incremental security

What happens when the state a session restores is hostile, and what the engine
still checks when it is.

## 1. The trust boundary

Phase 1's rule was that the engine never trusts the host: pointers must name
canonical regions, lengths must be exact, counts must be inside their ceilings,
and every byte access is checked. Phase 4 adds a second surface — the host can
now hand the engine *state*, not just arguments — and that state is treated the
same way.

The threat model is a buggy or confused host, not an attacker who can write the
linear memory directly. Someone who can write the arena can already forge
anything; the checks below are about catching mistakes, and about making sure a
mistake produces a refusal rather than a wrong plan.

## 2. What a restored state must satisfy

Before a single call is planned, the engine checks:

| Check | Refusal |
| --- | --- |
| the record's revision is `0` (plan everything) or `1` (resume) | `131` |
| the flags word holds only defined bits | `131` |
| the two reserved words are zero | `131` |
| the calls, events and edges counters are inside their ceilings | `132` |
| the events counter is within what the prefix could have produced | `132` |
| the edges counter is within `2 × events` | `132` |
| every `last_writer` is `u32::MAX` or names a call the prefix contains | `131` |
| every `reader_head` names an event the state holds | `131` |
| every event's owner is a call the prefix contains | `131` |
| every reader link names an *older* node | `131` |
| the digest of the prefix and the state matches the committed digest | `131` |

The last one is the important one: it ties the state to the *input* that
produced it. Without it, a state from a different batch — with different
resources, or a different call order — would pass the range checks and produce
a graph that is structurally valid and semantically wrong. With it, the only way
through is a 32-bit collision.

## 3. Why a reader link must recede

A read pushes a node onto the front of its resource's list, and the node's link
points at the node that was there before — which is always older. The engine's
own lists therefore decrease, and the drain in `process_write` terminates.

A restored list is host data, so the property has to be re-established: the
validator refuses a link that points forwards or at itself. Without that check, a
host that pushed a cycle would hang the engine in the drain loop — the Phase 1
code had no bound there, because the engine wrote every list it ever walked.

The drain is now bounded by the event count as well, so even a state that slipped
past the validator cannot make the engine walk forever.

## 4. What the host does with a refusal

A refusal is not an error the caller has to handle: the session catches it,
discards the checkpoint, plans the whole batch and records the reason in
`update.reason`. The plan is correct either way; only the cost differs.

## 5. The keys stay in the host

Stable keys are caller-supplied strings and they never cross into the engine.
The engine sees positions. Two consequences:

* a key cannot be a vector for a memory-safety problem — the engine never
  parses it;
* an error message cannot leak a key. The session's messages name positions and
  error codes, never the caller's strings.

## 6. The frozen interning table

A session allocates a resource number the first time it sees a resource and never
recycles it. This is not a security measure but it has a security-adjacent
property: it removes the possibility that a *reused* plan describes different
resources than it did before. Without it, removing an early call would renumber
the resources of every later call, and the preserved prefix would quietly refer
to the wrong things — a graph that is internally consistent and wrong.

## 7. The hostile-ABI surface the session adds

| Vector | What happens |
| --- | --- |
| a checkpoint record with an unknown revision | `131`, the session falls back |
| a record with a flag bit that does not exist | `131` |
| counters that exceed the prefix's capacity | `132` |
| a frontier that names a call that has not happened | `131` |
| a reader list that runs forward | `131` |
| a digest that does not match the state | `131` |
| a checkpoint region of the wrong size | `132` |
| a checkpoint pointer that is not the canonical region | `108` |

The last two are checked by the host's loader before any session exists: the
region must be the canonical one and exactly the right size, like every other
region in this ABI.

## 8. What is *not* protected against

* A host that can write the linear memory directly can forge a state and a
  digest together. The digest is a commitment against mistakes, not against an
  adversary with that access.
* A host that lies about its own calls: the engine plans what it is given. The
  dependency graph is a function of the declared effects, and declared effects
  are the caller's claim.
* Sessions that outlive their engine instance, or that share an engine with a
  stateless batch, are a misuse the API documents rather than prevents.

## 9. Related documents

* `docs/CHECKPOINT-ARCHITECTURE.md` — what a checkpoint holds.
* `docs/ABI.md` — the record's bytes.
* `docs/ARCHITECTURE.md` — the Phase 1–3 safety properties that still hold.
