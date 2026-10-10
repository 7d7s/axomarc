-- HTTP session persistence (S7 C6/B-03): durable store for kelson-session.
-- Separate from the legacy `sessions` table (kelson-identity session rows):
-- kelson-session ids are 64-hex strings with a JSON data bag.
CREATE TABLE IF NOT EXISTS http_sessions (
    id           text PRIMARY KEY,
    principal_id uuid NOT NULL REFERENCES principals(id),
    tenant_id    uuid,
    data         jsonb NOT NULL DEFAULT '{}',
    created_at   timestamptz NOT NULL DEFAULT now(),
    last_seen    timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS ix_http_sessions_last_seen ON http_sessions (last_seen);