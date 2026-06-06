CREATE TABLE update_history (
    sha256           TEXT NOT NULL PRIMARY KEY,
    from_version     TEXT NOT NULL,
    to_version       TEXT NOT NULL,
    channel          TEXT NOT NULL,
    applied_at       TEXT NOT NULL,
    backup_path      TEXT NOT NULL,
    rolled_back_at   TEXT
);

CREATE INDEX idx_update_history_applied_at ON update_history(applied_at DESC);
