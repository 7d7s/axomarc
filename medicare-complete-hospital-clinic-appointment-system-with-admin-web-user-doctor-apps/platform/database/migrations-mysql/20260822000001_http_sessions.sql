-- MySQL 8 dialect twin of database/migrations/20260822000001_http_sessions.sql.
-- uuid: BINARY(16) (sqlx MySQL Uuid codec is binary)
-- HTTP session persistence (S7 C6/B-03): durable store for kelson-session.
-- Separate from the legacy `sessions` table (kelson-identity session rows):
-- kelson-session ids are 64-hex strings with a JSON data bag.
CREATE TABLE IF NOT EXISTS http_sessions (
    id            VARCHAR(64) PRIMARY KEY,
    principal_id  BINARY(16) NOT NULL,
    tenant_id     BINARY(16),
    data          JSON NOT NULL DEFAULT ('{}'),
    created_at    DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    last_seen     DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    FOREIGN KEY (principal_id) REFERENCES principals(id)
) DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;
CREATE INDEX ix_http_sessions_last_seen ON http_sessions (last_seen);
