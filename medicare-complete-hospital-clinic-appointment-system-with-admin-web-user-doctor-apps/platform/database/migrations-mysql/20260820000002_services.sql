-- MySQL 8 dialect twin of database/migrations/20260820000002_services.sql.
-- uuid: BINARY(16) (sqlx MySQL Uuid codec is binary)
-- Kelson services DDL (S2 item 19): durable jobs, diagnostics chain, auth tokens.
-- The outbox table already exists in 20260819000001_foundation.sql (the outbox
-- writer landed with the engine); the pump (item 24) reads it. Same conventions:
-- application-generated UUID v7 ids, runtime role never owns tables (roles.sql).

-- ============ DURABLE JOB QUEUE (SKIP LOCKED, item 22/23) ============

CREATE TABLE IF NOT EXISTS jobs_queue (
    id              BINARY(16) PRIMARY KEY,
    execution_id    BINARY(16) NOT NULL,
    name            TEXT NOT NULL,
    payload         JSON NOT NULL DEFAULT ('{}'),
    queue           TEXT NOT NULL DEFAULT ('default'),
    attempt         integer NOT NULL DEFAULT 0,
    max_attempts    integer NOT NULL DEFAULT 3,
    deadline        DATETIME(6) NOT NULL,
    available_at    DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    status          VARCHAR(64) NOT NULL DEFAULT 'pending',  -- pending|running|succeeded|failed|dead
    priority        integer NOT NULL DEFAULT 0,
    claimed_by      TEXT,
    claimed_at      DATETIME(6),
    causation_id    BINARY(16),
    correlation_id  BINARY(16),
    last_error      TEXT,
    enqueued_at     DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    finished_at     DATETIME(6)
) DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

-- Claim hot path: highest priority first, FIFO within priority, pending only.
-- MySQL has no partial index: WHERE status = 'pending' is dropped (non-unique
-- index, so only index size / scan selectivity diverges; the claim query keeps
-- its explicit status = 'pending' predicate).
CREATE INDEX ix_jobs_claimable ON jobs_queue (priority DESC, available_at);
CREATE INDEX ix_jobs_status ON jobs_queue (status);
CREATE INDEX ix_jobs_correlation ON jobs_queue (correlation_id);

-- ============ DIAGNOSTIC CHAIN (item 29 db:inspect support) ============

CREATE TABLE IF NOT EXISTS diag_chain (
    id            BINARY(16) PRIMARY KEY,
    execution_id  BINARY(16) NOT NULL,
    seq           integer NOT NULL,
    kind          TEXT NOT NULL,  -- boot|validate|authz|execute|persist|publish|fail
    message       TEXT NOT NULL,
    data          JSON NOT NULL DEFAULT ('{}'),
    created_at    DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    UNIQUE (execution_id, seq)
) DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;
CREATE INDEX ix_diag_execution ON diag_chain (execution_id, seq);

-- ============ AUTH TOKENS (item 21: hashed, TTL, single-use) ============

CREATE TABLE IF NOT EXISTS auth_tokens (
    id            BINARY(16) PRIMARY KEY,
    principal_id  BINARY(16) NOT NULL,
    purpose       TEXT NOT NULL,  -- api | reset | verify
    token_hash    VARCHAR(64) NOT NULL UNIQUE,  -- sha256 hex; plaintext never stored
    label         TEXT,
    expires_at    DATETIME(6) NOT NULL,
    consumed_at   DATETIME(6),
    created_at    DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    FOREIGN KEY (principal_id) REFERENCES principals(id)
) DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;
CREATE INDEX ix_auth_tokens_principal ON auth_tokens (principal_id);
