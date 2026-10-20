# Pair migrations (rollback-capable)

Forward-only single-file migrations live in [`../migrations/`](../migrations/)
— sqlx-owned, recorded in `_sqlx_migrations`, and **never roll back**.

Every rollback-capable migration ships here as a pair:

```text
<ts>_<name>.up.sql     applied by `kelson migrate` (filename order)
<ts>_<name>.down.sql   required; applied by `kelson migrate:rollback`
```

Applied pairs are recorded in the `keel_migrations` bookkeeping table
(`migration`, `batch`) — one batch number per `kelson migrate` run that applied
at least one pair (Laravel semantics).

| Command | Effect |
| --- | --- |
| `kelson migrate:install` | create `keel_migrations` without running any files |
| `kelson migrate:rollback [--step n]` | revert the last `n` batches (default 1), newest batch first, reverse filename order inside a batch |
| `kelson migrate:reset` | revert every applied pair in reverse order (dev/test only) |
| `kelson migrate:refresh [--step n]` | rollback `n` batches, then migrate (dev/test only) |
| `kelson migrate:status` | shows singles and pairs with their applied/pending state |

Rules (fail-closed):

- A `.up.sql` without its `.down.sql` — or an orphan `.down.sql` — refuses
  the scan before any DDL runs. A pair that cannot roll back never applies.
- The base name must be `<ts>_<name>` with a numeric timestamp prefix.
- A row recorded as applied whose files are gone refuses the rollback.

Documented divergence: within a run, legacy singles apply before pairs
(interleaving the two formats by timestamp is not supported), and rollback
applies only to pairs — legacy single-file migrations are forward-only.

## Rollback granularity, and no-downtime changes

`kelson migrate:rollback` reverts whole **batches** (one batch number per
`kelson migrate` run that applied at least one pair — Laravel's granularity).
A single migration inside a batch cannot be reverted on its own; ship it in
its own run if it may need reverting alone.

Destructive rollback cannot be promised universally: a `.down.sql` that drops
a column destroys the data in it. For a **no-downtime** change, use the
expand/contract sequence and keep every step reversible on its own:

1. **Expand** — add the new column/table/index. Nothing reads it yet, so the
   old code is unaffected. A pair's `.down.sql` drops it again.
2. **Backfill** — copy the old data into the new shape, in batches. A
   forward-only single is fine here (it is additive and idempotent); make the
   backfill re-runnable so a retry is safe.
3. **Switch** — read from the new shape, write to both. Deploys are now
   compatible with the previous release *and* the next one, so a rollback of
   the application does not need a schema rollback.
4. **Contract** — drop the old column/table, in a later release, once no
   running version reads it.

Rule of thumb: a change that must be rolled back with the application belongs
in a **pair** (it has a `.down.sql`); a change that only ever moves forward
belongs in the forward-only singles.
