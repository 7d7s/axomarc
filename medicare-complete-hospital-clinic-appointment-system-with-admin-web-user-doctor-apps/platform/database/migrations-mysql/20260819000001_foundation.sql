-- MySQL 8 dialect twin of database/migrations/20260819000001_foundation.sql.
-- uuid: BINARY(16) (sqlx MySQL Uuid codec is binary)
-- MySQL: inline column REFERENCES is parsed but ignored by the server, so
-- every foreign key below is rewritten as a table-level FOREIGN KEY clause.
-- Keel foundation DDL (G09 companion). All ids are application-generated UUID v7.
-- RLS here is the FINAL containment barrier; application authorization decides
-- business permission. Runtime must connect with a NON-OWNER role (roles.sql).

-- ============ PLATFORM PLANE ============

CREATE TABLE IF NOT EXISTS tenants (
    id          BINARY(16) PRIMARY KEY,
    slug        VARCHAR(128) NOT NULL UNIQUE,
    name        TEXT NOT NULL,
    status      TEXT NOT NULL DEFAULT ('active'),
    plan        TEXT NOT NULL DEFAULT ('standard'),
    created_at  DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
) DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE IF NOT EXISTS principals (
    id          BINARY(16) PRIMARY KEY,
    status      TEXT NOT NULL DEFAULT ('active'),
    email       VARCHAR(255) UNIQUE,
    phone       VARCHAR(64) UNIQUE,
    created_at  DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
) DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE IF NOT EXISTS credentials (
    id            BINARY(16) PRIMARY KEY,
    principal_id  BINARY(16) NOT NULL,
    kind          TEXT NOT NULL,  -- password_argon2 | otp | api_key
    secret_hash   TEXT NOT NULL,
    created_at    DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    expires_at    DATETIME(6),
    FOREIGN KEY (principal_id) REFERENCES principals(id)
) DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE IF NOT EXISTS sessions (
    id            BINARY(16) PRIMARY KEY,
    principal_id  BINARY(16) NOT NULL,
    expires_at    DATETIME(6) NOT NULL,
    source_ip     TEXT,
    user_agent    TEXT,
    FOREIGN KEY (principal_id) REFERENCES principals(id)
) DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

-- Global revision counters (single-row table).
CREATE TABLE IF NOT EXISTS revisions (
    id            TINYINT(1) NOT NULL DEFAULT 1 PRIMARY KEY CHECK (id = 1),
    platform_rev  bigint NOT NULL DEFAULT 0,
    tenant_rev    bigint NOT NULL DEFAULT 0,
    auth_rev      bigint NOT NULL DEFAULT 0,
    module_rev    bigint NOT NULL DEFAULT 0,
    feature_rev   bigint NOT NULL DEFAULT 0,
    schema_rev    bigint NOT NULL DEFAULT 0
) DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;
-- pg: INSERT ... DEFAULT VALUES ON CONFLICT DO NOTHING (single-row table).
INSERT INTO revisions (id) SELECT 1 FROM DUAL WHERE NOT EXISTS (SELECT 1 FROM revisions);

-- ============ TENANT-ANCHORED TABLES (RLS) ============

CREATE TABLE IF NOT EXISTS memberships (
    id            BINARY(16) PRIMARY KEY,
    principal_id  BINARY(16) NOT NULL,
    tenant_id     BINARY(16) NOT NULL,
    status        TEXT NOT NULL DEFAULT ('active'),
    joined_at     DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    FOREIGN KEY (principal_id) REFERENCES principals(id),
    FOREIGN KEY (tenant_id) REFERENCES tenants(id)
) DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;
CREATE INDEX ix_memberships_principal ON memberships (principal_id);
CREATE INDEX ix_memberships_tenant ON memberships (tenant_id);

CREATE TABLE IF NOT EXISTS scope_nodes (
    id         BINARY(16) PRIMARY KEY,
    tenant_id  BINARY(16) NOT NULL,
    kind       TEXT NOT NULL,
    name       TEXT NOT NULL,
    path       VARCHAR(255) NOT NULL,
    parent_id  BINARY(16),
    UNIQUE (tenant_id, path),
    FOREIGN KEY (tenant_id) REFERENCES tenants(id)
) DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE IF NOT EXISTS scope_grants (
    id             BINARY(16) PRIMARY KEY,
    tenant_id      BINARY(16) NOT NULL,
    membership_id  BINARY(16) NOT NULL,
    node_id        BINARY(16) NOT NULL,
    FOREIGN KEY (tenant_id) REFERENCES tenants(id),
    FOREIGN KEY (membership_id) REFERENCES memberships(id),
    FOREIGN KEY (node_id) REFERENCES scope_nodes(id)
) DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;
CREATE INDEX ix_scope_grants_membership ON scope_grants (membership_id);

CREATE TABLE IF NOT EXISTS capability_grants (
    id             BINARY(16) PRIMARY KEY,
    tenant_id      BINARY(16) NOT NULL,
    membership_id  BINARY(16) NOT NULL,
    capability     TEXT NOT NULL,
    conditions     JSON NOT NULL DEFAULT ('[]'),
    FOREIGN KEY (tenant_id) REFERENCES tenants(id),
    FOREIGN KEY (membership_id) REFERENCES memberships(id)
) DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

-- Delta machinery: per-stream monotonic sequences + immutable stones.
CREATE TABLE IF NOT EXISTS delta_streams (
    stream_key  VARCHAR(191) PRIMARY KEY,
    next_seq    bigint NOT NULL DEFAULT 0
) DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE IF NOT EXISTS delta_stones (
    id                  BINARY(16) PRIMARY KEY,
    stream_key          VARCHAR(191) NOT NULL,
    sequence            bigint NOT NULL,
    resource_type       VARCHAR(64) NOT NULL,
    resource_id         BINARY(16),
    resource_version    bigint NOT NULL,
    execution_id        BINARY(16) NOT NULL,
    principal_id        BINARY(16),
    tenant_id           BINARY(16),
    scope_snapshot      JSON NOT NULL DEFAULT ('[]'),
    ownership_snapshot  JSON NOT NULL DEFAULT ('[]'),
    `before`            JSON,
    command             TEXT NOT NULL,
    `after`             JSON NOT NULL,
    causation_id        BINARY(16),
    correlation_id      BINARY(16),
    at                  DATETIME(6) NOT NULL,
    UNIQUE (stream_key, sequence),
    FOREIGN KEY (stream_key) REFERENCES delta_streams(stream_key),
    FOREIGN KEY (tenant_id) REFERENCES tenants(id)
) DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;
CREATE INDEX ix_delta_stones_tenant_at ON delta_stones (tenant_id, at);
CREATE INDEX ix_delta_stones_resource ON delta_stones (resource_type, resource_id, sequence);

CREATE TABLE IF NOT EXISTS audit_records (
    id            BINARY(16) PRIMARY KEY,
    execution_id  BINARY(16) NOT NULL,
    tenant_id     BINARY(16),
    principal_id  BINARY(16),
    action        TEXT NOT NULL,
    resource      TEXT NOT NULL,
    authority     TEXT NOT NULL,
    outcome       TEXT NOT NULL,
    detail        JSON NOT NULL DEFAULT ('{}'),
    at            DATETIME(6) NOT NULL,
    FOREIGN KEY (tenant_id) REFERENCES tenants(id)
) DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;
CREATE INDEX ix_audit_tenant_at ON audit_records (tenant_id, at);

CREATE TABLE IF NOT EXISTS outbox (
    id          BINARY(16) PRIMARY KEY,
    tenant_id   BINARY(16),
    event       TEXT NOT NULL,
    topic       TEXT NOT NULL,
    payload     JSON NOT NULL,
    published   TINYINT(1) NOT NULL DEFAULT 0,
    created_at  DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    FOREIGN KEY (tenant_id) REFERENCES tenants(id)
) DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;
-- MySQL has no partial index: WHERE published = false is dropped (the index is
-- non-unique, so only index size diverges; scans still filter on published).
CREATE INDEX ix_outbox_unpublished ON outbox (published, created_at);

CREATE TABLE IF NOT EXISTS idempotency_keys (
    id            BINARY(16) NOT NULL,
    tenant_id     BINARY(16),
    operation     VARCHAR(64) NOT NULL,
    `key`         VARCHAR(512) NOT NULL,  -- utf8mb4 PK parts stay under 3072 bytes
    payload_hash  TEXT NOT NULL,
    result        JSON NOT NULL,
    created_at    DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    PRIMARY KEY (tenant_id, operation, `key`)
) DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE IF NOT EXISTS elevation_leases (
    id             BINARY(16) PRIMARY KEY,
    operator       BINARY(16) NOT NULL,
    target_tenant  BINARY(16) NOT NULL,
    capability     TEXT NOT NULL,
    scope_paths    TEXT NOT NULL DEFAULT (''),  -- pg text[]; comma-separated on MySQL
    reason         TEXT NOT NULL,
    approver       BINARY(16) NOT NULL,
    read_only      TINYINT(1) NOT NULL DEFAULT 1,
    granted_at     DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    expires_at     DATETIME(6) NOT NULL,
    FOREIGN KEY (operator) REFERENCES principals(id),
    FOREIGN KEY (target_tenant) REFERENCES tenants(id),
    FOREIGN KEY (approver) REFERENCES principals(id)
) DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

-- No RLS on MySQL: tenant isolation is enforced solely by the tenant-guarded query builders (docs/05 divergence).
