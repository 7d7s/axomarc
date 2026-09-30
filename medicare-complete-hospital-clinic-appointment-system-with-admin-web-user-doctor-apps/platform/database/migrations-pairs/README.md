# Pair migrations (rollback-capable)

Forward-only single-file migrations live in [`../migrations/`](../migrations/)
— sqlx-owned, recorded in `_sqlx_migrations`, and **never roll back**.

Every rollback-capable migration ships here as a pair:

```text
<ts>_<name>.up.sql     applied by `keel migrate` (filename order)
<ts>_<name>.down.sql   required; applied by `keel migrate:rollback`
```

Applied pairs are recorded in the `keel_migrations` bookkeeping table
(`migration`, `batch`) — one batch number per `keel migrate` run that applied
at least one pair (Laravel semantics).

| Command | Effect |
| --- | --- |
| `keel migrate:install` | create `keel_migrations` without running any files |
| `keel migrate:rollback [--step n]` | revert the last `n` batches (default 1), newest batch first, reverse filename order inside a batch |
| `keel migrate:reset` | revert every applied pair in reverse order (dev/test only) |
| `keel migrate:refresh [--step n]` | rollback `n` batches, then migrate (dev/test only) |
| `keel migrate:status` | shows singles and pairs with their applied/pending state |

Rules (fail-closed):

- A `.up.sql` without its `.down.sql` — or an orphan `.down.sql` — refuses
  the scan before any DDL runs. A pair that cannot roll back never applies.
- The base name must be `<ts>_<name>` with a numeric timestamp prefix.
- A row recorded as applied whose files are gone refuses the rollback.

Documented divergence: within a run, legacy singles apply before pairs
(interleaving the two formats by timestamp is not supported), and rollback
applies only to pairs — legacy single-file migrations are forward-only.
