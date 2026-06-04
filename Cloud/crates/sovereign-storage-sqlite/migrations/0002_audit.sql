-- 0002_audit.sql — the append-only audit log.
-- Per docs/architecture.md §2.3, audit_event is the source of truth
-- for "who did what, when, with what policy decision." SQLite triggers
-- reject any UPDATE or DELETE; the only way to "remove" an event is
-- `VACUUM INTO` (rare; only for GDPR Art. 17 with a documented basis).
--
-- Note: SQLite forbids `AUTOINCREMENT` on `WITHOUT ROWID` tables. We use
-- a regular `INTEGER PRIMARY KEY` (the rowid alias) which is implicitly
-- auto-incrementing. The size overhead vs. `WITHOUT ROWID` is ~30 bytes
-- per row, which is negligible at the V0 audit volume.
--
-- The V1.5+ G23 work adds a per-box Ed25519 chain (`prev_hash`,
-- `signature`) on top of this row; that's a separate migration.

CREATE TABLE audit_event (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    ts               INTEGER NOT NULL,
    actor            TEXT NOT NULL,                              -- 'user:alice' | 'system' | 'ml:scorer'
    kind             TEXT NOT NULL,                              -- 'app' | 'deploy' | 'secret' | ...
    target           TEXT,                                       -- 'app:api', 'deployment:7f3...', NULL
    payload          TEXT NOT NULL,                              -- JSON
    policy_decision  TEXT                                        -- JSON
);

-- Index by actor for the `sovereign audit --actor` filter.
CREATE INDEX idx_audit_actor ON audit_event(actor);

-- Index by target for the `--target` filter (e.g. all events for one app).
CREATE INDEX idx_audit_target ON audit_event(target);

-- The trust anchor: a BEFORE trigger that aborts the UPDATE statement.
-- The mutation that caused the trigger fires is rolled back, so the
-- audit log is structurally tamper-evident even if the binary is
-- compromised and an attacker has write access to the DB file.
CREATE TRIGGER audit_no_update
BEFORE UPDATE ON audit_event
BEGIN
    SELECT RAISE(ABORT, 'audit_event is append-only');
END;

CREATE TRIGGER audit_no_delete
BEFORE DELETE ON audit_event
BEGIN
    SELECT RAISE(ABORT, 'audit_event is append-only');
END;
