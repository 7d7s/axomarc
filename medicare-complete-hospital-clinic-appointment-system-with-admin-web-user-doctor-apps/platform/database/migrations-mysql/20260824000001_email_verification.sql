-- MySQL 8 dialect twin of database/migrations/20260824000001_email_verification.sql.
-- B-23 email verification: record when a principal proved ownership of the
-- contact email. NULL = unverified. Additive column only (forward-only
-- migration discipline, G08); existing rows stay unverified until proven.
-- MySQL has no ADD COLUMN IF NOT EXISTS; plain ADD COLUMN (sqlx runs each
-- migration file exactly once).
ALTER TABLE principals ADD COLUMN email_verified_at  DATETIME(6);
