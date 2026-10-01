-- Wave 6 P9 durable batch ledger (19-PARITY "Queues advanced" bullet 304;
-- B-32): one row per dispatched batch, written once at dispatch — the
-- membership is immutable, per-member state lives in jobs_queue, so
-- `batch.status` / `batch.cancel` / `batch.retry`, `queue:retry-batch` and
-- `queue:prune-batches` resolve live queue truth from any process and a
-- worker restart never loses a fan-out.
CREATE TABLE IF NOT EXISTS job_batches (
    id           uuid PRIMARY KEY,
    name         text NOT NULL,
    members      jsonb NOT NULL,
    on_complete  text,
    created_at   timestamptz NOT NULL DEFAULT now(),
    finished_at  timestamptz
);
-- `queue:prune-batches` ages terminal records out strictly-before this
-- stamp; open batches (finished_at IS NULL) are never pruned.
CREATE INDEX IF NOT EXISTS ix_job_batches_finished_at
    ON job_batches (finished_at) WHERE finished_at IS NOT NULL;
