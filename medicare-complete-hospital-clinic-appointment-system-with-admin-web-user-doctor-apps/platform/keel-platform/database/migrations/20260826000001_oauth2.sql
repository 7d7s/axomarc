-- OAuth2 module (B-21, Wave 8 P5): the client registry.
-- Secrets are argon2 hashed by the module (kelson-identity password helpers);
-- the plaintext is shown exactly once at creation. The redirect allowlist
-- and scopes ride jsonb so membership checks are data, not string parsing.
-- Global registry (no tenant column): the token/revoke endpoints are
-- anonymous machine calls that resolve a client by its globally unique
-- client_id, while the authorization endpoint resolves it from the
-- session's tenant context through the same unique key.
CREATE TABLE IF NOT EXISTS oauth_clients (
    id            uuid PRIMARY KEY,
    name          text NOT NULL,
    client_id     text NOT NULL UNIQUE,
    secret_hash   text NOT NULL,
    redirect_uris jsonb NOT NULL,
    scopes        jsonb NOT NULL,
    version       bigint NOT NULL DEFAULT 1,
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now()
);
