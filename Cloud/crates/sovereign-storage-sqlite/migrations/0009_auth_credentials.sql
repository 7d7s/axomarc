-- S3: User credentials, API tokens, bootstrap state.
-- Expands the existing user table (0001) and adds two new tables.

-- Add credential columns to the existing user table.
ALTER TABLE user ADD COLUMN password_hash TEXT;            -- argon2id; NULL for OIDC-only users
ALTER TABLE user ADD COLUMN display_name TEXT NOT NULL DEFAULT '';
ALTER TABLE user ADD COLUMN telegram_chat_id INTEGER;     -- bound via chatops
ALTER TABLE user ADD COLUMN oidc_subject TEXT;            -- bound via OIDC RP
ALTER TABLE user ADD COLUMN disabled_at INTEGER;          -- soft-delete timestamp

-- API tokens for programmatic access (CI/CD, scripts).
CREATE TABLE api_token (
    id            BLOB PRIMARY KEY,                       -- TokenId (16 bytes)
    user_id       BLOB NOT NULL REFERENCES user(id) ON DELETE CASCADE,
    name          TEXT NOT NULL,
    hash          TEXT NOT NULL,                          -- sha256(token), never plaintext
    scopes        TEXT NOT NULL DEFAULT '',               -- CSV: "deploy,secret.read,service.install"
    created_at    INTEGER NOT NULL,
    expires_at    INTEGER,                                -- NULL = no expiry
    UNIQUE(user_id, name)
);

CREATE INDEX idx_api_token_user ON api_token(user_id);

-- Bootstrap state singleton (first admin).
CREATE TABLE bootstrap_state (
    id            INTEGER PRIMARY KEY CHECK (id = 1),     -- singleton row
    admin_user_id BLOB REFERENCES user(id)
);
