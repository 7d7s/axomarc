-- MySQL 8 dialect twin of database/migrations/20261001000002_job_batches.sql.
-- uuid: BINARY(16) (sqlx MySQL Uuid codec is binary)
-- Wave 6 P9 durable batch ledger (19-PARITY "Queues advanced" bullet 304;
-- B-32): one row per dispatched batch, written once at dispatch - the
-- membership is immutable, per-member state lives in jobs_queue, so
-- `batch.status` / `batch.cancel` / `batch.retry`, `queue:retry-batch` and
-- `queue:prune-batches` resolve live queue truth from any process and a
-- worker restart never loses a fan-out.
CREATE TABLE IF NOT EXISTS job_batches (
    id           BINARY(16) PRIMARY KEY,
    name         TEXT NOT NULL,
    members      JSON NOT NULL,
    on_complete  TEXT,
    created_at   DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    finished_at  DATETIME(6)
) DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;
-- `queue:prune-batches` ages terminal records out strictly-before this
-- stamp; open batches (finished_at IS NULL) are never pruned.
-- MySQL has no partial index: WHERE finished_at IS NOT NULL is dropped
-- (non-unique index, so only index size diverges; the prune query keeps its
-- explicit finished_at IS NOT NULL predicate).
CREATE INDEX ix_job_batches_finished_at
    ON job_batches (finished_at);
