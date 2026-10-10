-- Kelson services DDL (S2 item 19): durable jobs, diagnostics chain, auth tokens.
-- The outbox table already exists in 20260819000001_foundation.sql (the outbox
-- writer landed with the engine); the pump (item 24) reads it. Same conventions:
-- application-generated UUID v7 ids, runtime role never owns tables (roles.sql).

-- ============ DURABLE JOB QUEUE (SKIP LOCKED, item 22/23) ============

CREATE TABLE IF NOT EXISTS jobs_queue (
    id             uuid PRIMARY KEY,
    execution_id   uuid NOT NULL,
    name           text NOT NULL,
    payload        jsonb NOT NULL DEFAULT '{}',
    queue          text NOT NULL DEFAULT 'default',
    attempt        integer NOT NULL DEFAULT 0,
    max_attempts   integer NOT NULL DEFAULT 3,
    deadline       timestamptz NOT NULL,
    available_at   timestamptz NOT NULL DEFAULT now(),
    status         text NOT NULL DEFAULT 'pending',  -- pending|running|succeeded|failed|dead
    priority       integer NOT NULL DEFAULT 0,
    claimed_by     text,
    claimed_at     timestamptz,
    causation_id   uuid,
    correlation_id uuid,
    last_error     text,
    enqueued_at    timestamptz NOT NULL DEFAULT now(),
    finished_at    timestamptz
);

-- Claim hot path: highest priority first, FIFO within priority, pending only.
CREATE INDEX IF NOT EXISTS ix_jobs_claimable
    ON jobs_queue (priority DESC, available_at)
    WHERE status = 'pending';
CREATE INDEX IF NOT EXISTS ix_jobs_status ON jobs_queue (status);
CREATE INDEX IF NOT EXISTS ix_jobs_correlation ON jobs_queue (correlation_id);

-- ============ DIAGNOSTIC CHAIN (item 29 db:inspect support) ============

CREATE TABLE IF NOT EXISTS diag_chain (
    id           uuid PRIMARY KEY,
    execution_id uuid NOT NULL,
    seq          integer NOT NULL,
    kind         text NOT NULL,          -- boot|validate|authz|execute|persist|publish|fail
    message      text NOT NULL,
    data         jsonb NOT NULL DEFAULT '{}',
    created_at   timestamptz NOT NULL DEFAULT now(),
    UNIQUE (execution_id, seq)
);
CREATE INDEX IF NOT EXISTS ix_diag_execution ON diag_chain (execution_id, seq);

-- ============ AUTH TOKENS (item 21: hashed, TTL, single-use) ============

CREATE TABLE IF NOT EXISTS auth_tokens (
    id           uuid PRIMARY KEY,
    principal_id uuid NOT NULL REFERENCES principals(id),
    purpose      text NOT NULL,          -- api | reset | verify
    token_hash   text NOT NULL UNIQUE,   -- sha256 hex; plaintext never stored
    label        text,
    expires_at   timestamptz NOT NULL,
    consumed_at  timestamptz,
    created_at   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS ix_auth_tokens_principal ON auth_tokens (principal_id);
