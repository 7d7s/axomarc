-- Wave 6 P5 database channel (19-PARITY "Notifications v2"): rows written by
-- the notification store when a typed notification dispatches on the
-- `database` channel. Read by `GET /api/v1/notifications`, marked read by
-- `POST /api/v1/notifications/read` (principal-scoped self service — a
-- caller only ever reads or marks its own rows).
CREATE TABLE IF NOT EXISTS notifications (
    id           uuid PRIMARY KEY,
    principal_id uuid NOT NULL REFERENCES principals(id),
    tenant_id    uuid REFERENCES tenants(id),
    type         text NOT NULL,
    data         jsonb NOT NULL DEFAULT '{}'::jsonb,
    read_at      timestamptz,
    created_at   timestamptz NOT NULL DEFAULT now()
);
-- Feed order (newest first) + the unread fast path for the count meta.
CREATE INDEX IF NOT EXISTS ix_notifications_principal
    ON notifications (principal_id, created_at DESC);
CREATE INDEX IF NOT EXISTS ix_notifications_unread
    ON notifications (principal_id) WHERE read_at IS NULL;
