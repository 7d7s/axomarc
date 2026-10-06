-- Target-arch §5.2 (Phase 3: event idempotency, KEEL-006/KEEL-V10):
-- outbox rows carry the append-time idempotency key
-- (SHA256(event + tenant + correlation_id + payload_hash), computed in the
-- commit that writes the row). NULL = pre-migration rows AND any writer
-- that predates the column — the pump maps NULL/empty to the empty key and
-- dispatches those rows WITHOUT dedup (back-compat, zero behavior change).
-- No UNIQUE constraint on purpose: duplicates are SKIPPED by the pump via
-- the completion gate, not rejected at write time (a rejected write would
-- roll back the business transaction the event belongs to).
-- MySQL twin of migrations/20261005000002_outbox_idempotency_key.sql.
ALTER TABLE outbox ADD COLUMN idempotency_key TEXT NULL;

CREATE INDEX ix_outbox_idempotency_key ON outbox (idempotency_key(64));
