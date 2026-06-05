-- 0003_rollback_target.sql — the rollback target column.
--
-- F5 needs to record, for each rollback deployment, the deployment
-- that was current at the time of the rollback. This is the "what
-- did we roll back FROM" pointer; the new deployment's `image_ref`
-- is the target (what we rolled back TO).
--
-- Forward-only: we add a nullable column, no destructive ALTER.
-- The audit log (added in 0002) gets the same info via the
-- `audit_event.payload` JSON column, so the migration is double-
-- redundant: SQL column for fast queries, audit for tamper-evidence.

ALTER TABLE deployment
    ADD COLUMN target_deployment_id BLOB
        REFERENCES deployment(id);

-- Speed up `get_current_deployment(app_id)` and
-- `list_healthy_deployments_before(app_id, ts, limit)`: the common
-- pattern is "find the most recent Healthy for this app" or
-- "find the most recent Healthy for this app before a given ts".
CREATE INDEX idx_deployment_app_status_started
    ON deployment(app_id, status, started_at DESC);
