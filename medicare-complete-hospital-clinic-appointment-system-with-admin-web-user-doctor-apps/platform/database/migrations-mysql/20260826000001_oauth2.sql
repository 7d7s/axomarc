-- MySQL 8 dialect twin of database/migrations/20260826000001_oauth2.sql.
-- uuid: BINARY(16) (sqlx MySQL Uuid codec is binary)
-- OAuth2 module (B-21, Wave 8 P5): the client registry.
-- Secrets are argon2 hashed by the module (keel-identity password helpers);
-- the plaintext is shown exactly once at creation. The redirect allowlist
-- and scopes ride jsonb so membership checks are data, not string parsing.
-- Global registry (no tenant column): the token/revoke endpoints are
-- anonymous machine calls that resolve a client by its globally unique
-- client_id, while the authorization endpoint resolves it from the
-- session's tenant context through the same unique key.
CREATE TABLE IF NOT EXISTS oauth_clients (
    id            BINARY(16) PRIMARY KEY,
    name          TEXT NOT NULL,
    client_id     VARCHAR(191) NOT NULL UNIQUE,
    secret_hash   TEXT NOT NULL,
    redirect_uris JSON NOT NULL,
    scopes        JSON NOT NULL,
    version       bigint NOT NULL DEFAULT 1,
    created_at    DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at    DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
) DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;
