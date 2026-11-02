# Update transactions

Atomicity, and what a failure leaves behind.

## 1. The rule

**A transaction either changes the plan completely or not at all.**

There is no state in which half the changes are applied, no state in which the
version moved but the plan did not, and no state in which the session's
committed plan and the arena disagree about what was planned.

## 2. How it is achieved

An update is built in four steps, and only the last one is visible:

1. **Validate the transaction.** The version, the shape of the changeset, the
   keys — empty, duplicated, or over 128 UTF-8 bytes — and the calls themselves,
   all of it before anything is planned. A duplicate key, an unknown key, an
   over-long key or an invalid call fails here, with the session untouched.
2. **Build a candidate.** A copy of the session's entries with the changes
   applied. The session's own list is not touched.
3. **Plan the candidate.** The engine plans it into the arena. The engine may
   refuse; the refusal is a status code, not a partially written plan.
4. **Publish.** The candidate becomes the session's entries, the version moves,
   and the new snapshot is returned.

The only assignment to the session's state happens in step 4. Everything that can
fail happens before it.

## 3. What "the previous state remains usable" means here

The session's committed state — the calls, the checkpoints, the snapshot — lives
in the **host**, not in the arena. So a failed update cannot have corrupted it:
the arena may hold a half-written plan, but the committed state was never there.

The next update rewrites the arena from the committed state before it plans
anything, so a failed update costs nothing but the time it took.

This is also why the engine's own commit record matters: the engine clears the
checkpoint record *before* a run and writes it *after*, so a refused run leaves a
record that says "there is no checkpoint here" rather than one that looks
resumable.

## 4. The failure modes, and what each leaves

| Failure | Detected | What remains |
| --- | --- | --- |
| stale version | before anything is built | the previous snapshot, version and keys, exactly |
| duplicate key | while validating the changeset | as above |
| unknown key | while validating the changeset | as above |
| a key over 128 UTF-8 bytes | while validating the changeset, before the key is compared with any other | as above |
| an invalid call, effect or duration | while normalising the candidate | as above |
| the batch would exceed a ceiling | while packing the candidate | as above |
| the engine refuses the batch | while planning | as above, and the arena holds a refused plan that the next update overwrites |
| a restored checkpoint does not match | while planning | as above, and the session falls back to a full rebuild |
| the schedule overflows the tick range | while planning | as above |

In every case the caller gets an exception, the session's version does not move,
and the session can apply another transaction immediately. The tests assert this
sequence explicitly: a valid version 1, a deliberately invalid transaction, the
version still 1, the result still the same, and the next valid transaction
succeeding.

## 5. What is *not* atomic

* **The arena.** A refused run can leave a partially written plan in the engine's
  output region. It is scratch space, it is overwritten by the next run, and
  nothing reads it in between — but a caller that reached into the engine
  directly would see it. The public API never does.
* **The host's heap.** A refused transaction may have allocated candidate
  objects. They are garbage collected.
* **Other consumers of the same engine.** A session and a stateless `planBatch`
  on the same engine instance are not coordinated: the stateless call will
  overwrite the arena the session uses. Use one engine per session.

## 6. Multi-change transactions

A transaction may carry several changes, and they are applied **in the order
given**. Two changes naming the same key are refused, so a transaction cannot
append and then remove the same key — it is either one edit per key or an error.

The invalidation boundary is the earliest call any change reaches, so a
transaction that edits a late call and appends a new one still reuses the prefix
before the late edit.

## 7. Related documents

* `docs/PLAN-VERSIONING.md` — versions and the session lifecycle.
* `docs/INVALIDATION-MODEL.md` — how the affected suffix is chosen.
* `docs/INCREMENTAL-SECURITY.md` — what a hostile restored state is checked against.
