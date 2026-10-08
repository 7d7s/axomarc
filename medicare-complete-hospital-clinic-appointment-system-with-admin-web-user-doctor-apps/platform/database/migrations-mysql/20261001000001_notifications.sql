-- MySQL 8 dialect twin of database/migrations/20261001000001_notifications.sql.
-- uuid: BINARY(16) (sqlx MySQL Uuid codec is binary)
-- Wave 6 P5 database channel (19-PARITY "Notifications v2"): rows written by
-- the notification store when a typed notification dispatches on the
-- `database` channel. Read by `GET /api/v1/notifications`, marked read by
-- `POST /api/v1/notifications/read` (principal-scoped self service - a
-- caller only ever reads or marks its own rows).
CREATE TABLE IF NOT EXISTS notifications (
    id            BINARY(16) PRIMARY KEY,
    principal_id  BINARY(16) NOT NULL,
    tenant_id     BINARY(16),
    type          TEXT NOT NULL,
    data          JSON NOT NULL DEFAULT ('{}'),
    read_at       DATETIME(6),
    created_at    DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    FOREIGN KEY (principal_id) REFERENCES principals(id),
    FOREIGN KEY (tenant_id) REFERENCES tenants(id)
) DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;
-- Feed order (newest first) + the unread fast path for the count meta.
CREATE INDEX ix_notifications_principal ON notifications (principal_id, created_at DESC);
-- MySQL has no partial index: WHERE read_at IS NULL is dropped (non-unique
-- index, so only index size diverges; the count query keeps its explicit
-- read_at IS NULL predicate).
CREATE INDEX ix_notifications_unread ON notifications (principal_id);
