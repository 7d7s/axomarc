-- F15 (Step 15): relax the `UNIQUE (app_id, key)` constraint on
-- the `secret` table so that the V0.1.0 → V0.5 master-key
-- migration can mark the old row `retired` and insert a new
-- active row with the same `key` in the same app.
--
-- The previous full-table UNIQUE was technically over-constrained
-- even before this migration: a `rotated` workflow (F7) was
-- supposed to retire the old row, then put a new active row
-- with the same key, and the full unique index blocked that.
-- Step 15 just makes that real.
--
-- V0.5 is a single-tenant deployment; the partial unique index
-- keeps the same guarantee that "an app has at most one active
-- secret per key" while allowing the audit trail of retired
-- versions to live alongside.
--
-- SQLite doesn't allow dropping a UNIQUE constraint in place;
-- the canonical pattern is to rename → recreate → copy → drop.
-- We do that here, preserving every row and every column.
--
-- `sqlx::migrate!` wraps each migration in a transaction; do
-- not add BEGIN/COMMIT here.

PRAGMA foreign_keys=OFF;

CREATE TABLE secret_new (
    id            BLOB PRIMARY KEY,
    app_id        BLOB NOT NULL REFERENCES app(id),
    key           TEXT NOT NULL,
    ciphertext    BLOB NOT NULL,
    status        TEXT NOT NULL CHECK (status IN ('active','rotating','retired')) DEFAULT 'active',
    created_at    INTEGER NOT NULL,
    rotated_at    INTEGER,
    version       INTEGER NOT NULL DEFAULT 1
);

INSERT INTO secret_new (id, app_id, key, ciphertext, status, created_at, rotated_at, version)
SELECT id, app_id, key, ciphertext, status, created_at, rotated_at, version
FROM secret;

DROP TABLE secret;
ALTER TABLE secret_new RENAME TO secret;

-- Recreate the index from 0001_init.sql.
CREATE INDEX idx_secret_app ON secret(app_id);

-- Partial unique index: only the live rows are unique. Retired
-- rows can repeat (app_id, key) so the migration can leave the
-- old row in place for audit.
CREATE UNIQUE INDEX uniq_secret_app_key_live
    ON secret (app_id, key)
    WHERE status IN ('active', 'rotating');

PRAGMA foreign_keys=ON;
