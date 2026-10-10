-- MySQL 8 dialect twin of database/migrations/20260823000001_audit_hash_chain.sql.
-- uuid: BINARY(16) (sqlx MySQL Uuid codec is binary)
-- Sentinel (S5): audit hash-chain columns (D-74, STAGE-SENTINEL-SPEC).
-- Per-(tenant, day) chain segments; chain computation happens inside the
-- engine transaction via kelson-integrity (no side channels). Old rows keep
-- NULL columns: pre-chain history is not backfilled, chains start fresh.
-- MySQL has no ADD COLUMN IF NOT EXISTS; plain ADD COLUMN (sqlx runs each
-- migration file exactly once).

ALTER TABLE audit_records ADD COLUMN chain_id  BINARY(16);
ALTER TABLE audit_records ADD COLUMN prev_hash  BLOB;
ALTER TABLE audit_records ADD COLUMN chain_hash  BLOB;

CREATE INDEX ix_audit_chain ON audit_records (tenant_id, chain_id, at);
