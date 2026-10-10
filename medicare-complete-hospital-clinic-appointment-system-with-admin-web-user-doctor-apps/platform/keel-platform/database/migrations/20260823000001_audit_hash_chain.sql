-- Sentinel (S5): audit hash-chain columns (D-74, STAGE-SENTINEL-SPEC).
-- Per-(tenant, day) chain segments; chain computation happens inside the
-- engine transaction via kelson-integrity (no side channels). Old rows keep
-- NULL columns: pre-chain history is not backfilled, chains start fresh.

ALTER TABLE audit_records ADD COLUMN IF NOT EXISTS chain_id   uuid;
ALTER TABLE audit_records ADD COLUMN IF NOT EXISTS prev_hash  bytea;
ALTER TABLE audit_records ADD COLUMN IF NOT EXISTS chain_hash bytea;

CREATE INDEX IF NOT EXISTS ix_audit_chain
    ON audit_records (tenant_id, chain_id, at);
