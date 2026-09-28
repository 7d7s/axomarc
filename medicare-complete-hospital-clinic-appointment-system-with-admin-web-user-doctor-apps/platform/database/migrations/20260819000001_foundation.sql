-- Keel foundation DDL (G09 companion). All ids are application-generated UUID v7.
-- RLS here is the FINAL containment barrier; application authorization decides
-- business permission. Runtime must connect with a NON-OWNER role (roles.sql).

-- ============ PLATFORM PLANE ============

CREATE TABLE IF NOT EXISTS tenants (
    id          uuid PRIMARY KEY,
    slug        text NOT NULL UNIQUE,
    name        text NOT NULL,
    status      text NOT NULL DEFAULT 'active',
    plan        text NOT NULL DEFAULT 'standard',
    created_at  timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS principals (
    id          uuid PRIMARY KEY,
    status      text NOT NULL DEFAULT 'active',
    email       text UNIQUE,
    phone       text UNIQUE,
    created_at  timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS credentials (
    id            uuid PRIMARY KEY,
    principal_id  uuid NOT NULL REFERENCES principals(id),
    kind          text NOT NULL,             -- password_argon2 | otp | api_key
    secret_hash   text NOT NULL,
    created_at    timestamptz NOT NULL DEFAULT now(),
    expires_at    timestamptz
);

CREATE TABLE IF NOT EXISTS sessions (
    id          uuid PRIMARY KEY,
    principal_id uuid NOT NULL REFERENCES principals(id),
    expires_at  timestamptz NOT NULL,
    source_ip   text,
    user_agent  text
);

-- Global revision counters (single-row table).
CREATE TABLE IF NOT EXISTS revisions (
    id           boolean PRIMARY KEY DEFAULT true CHECK (id = true),
    platform_rev bigint NOT NULL DEFAULT 0,
    tenant_rev   bigint NOT NULL DEFAULT 0,
    auth_rev     bigint NOT NULL DEFAULT 0,
    module_rev   bigint NOT NULL DEFAULT 0,
    feature_rev  bigint NOT NULL DEFAULT 0,
    schema_rev   bigint NOT NULL DEFAULT 0
);
INSERT INTO revisions DEFAULT VALUES ON CONFLICT DO NOTHING;

-- ============ TENANT-ANCHORED TABLES (RLS) ============

CREATE TABLE IF NOT EXISTS memberships (
    id           uuid PRIMARY KEY,
    principal_id uuid NOT NULL REFERENCES principals(id),
    tenant_id    uuid NOT NULL REFERENCES tenants(id),
    status       text NOT NULL DEFAULT 'active',
    joined_at    timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS ix_memberships_principal ON memberships (principal_id);
CREATE INDEX IF NOT EXISTS ix_memberships_tenant ON memberships (tenant_id);

CREATE TABLE IF NOT EXISTS scope_nodes (
    id         uuid PRIMARY KEY,
    tenant_id  uuid NOT NULL REFERENCES tenants(id),
    kind       text NOT NULL,
    name       text NOT NULL,
    path       text NOT NULL,
    parent_id  uuid,
    UNIQUE (tenant_id, path)
);

CREATE TABLE IF NOT EXISTS scope_grants (
    id             uuid PRIMARY KEY,
    tenant_id      uuid NOT NULL REFERENCES tenants(id),
    membership_id  uuid NOT NULL REFERENCES memberships(id),
    node_id        uuid NOT NULL REFERENCES scope_nodes(id)
);
CREATE INDEX IF NOT EXISTS ix_scope_grants_membership ON scope_grants (membership_id);

CREATE TABLE IF NOT EXISTS capability_grants (
    id             uuid PRIMARY KEY,
    tenant_id      uuid NOT NULL REFERENCES tenants(id),
    membership_id  uuid NOT NULL REFERENCES memberships(id),
    capability     text NOT NULL,
    conditions     jsonb NOT NULL DEFAULT '[]'
);

-- Delta machinery: per-stream monotonic sequences + immutable stones.
CREATE TABLE IF NOT EXISTS delta_streams (
    stream_key  text PRIMARY KEY,
    next_seq    bigint NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS delta_stones (
    id                 uuid PRIMARY KEY,
    stream_key         text NOT NULL REFERENCES delta_streams(stream_key),
    sequence           bigint NOT NULL,
    resource_type      text NOT NULL,
    resource_id        uuid,
    resource_version   bigint NOT NULL,
    execution_id       uuid NOT NULL,
    principal_id       uuid,
    tenant_id          uuid REFERENCES tenants(id),
    scope_snapshot     jsonb NOT NULL DEFAULT '[]',
    ownership_snapshot jsonb NOT NULL DEFAULT '[]',
    before             jsonb,
    command            text NOT NULL,
    after              jsonb NOT NULL,
    causation_id       uuid,
    correlation_id     uuid,
    at                 timestamptz NOT NULL,
    UNIQUE (stream_key, sequence)
);
CREATE INDEX IF NOT EXISTS ix_delta_stones_tenant_at ON delta_stones (tenant_id, at);
CREATE INDEX IF NOT EXISTS ix_delta_stones_resource ON delta_stones (resource_type, resource_id, sequence);

CREATE TABLE IF NOT EXISTS audit_records (
    id            uuid PRIMARY KEY,
    execution_id  uuid NOT NULL,
    tenant_id     uuid REFERENCES tenants(id),
    principal_id  uuid,
    action        text NOT NULL,
    resource      text NOT NULL,
    authority     text NOT NULL,
    outcome       text NOT NULL,
    detail        jsonb NOT NULL DEFAULT '{}',
    at            timestamptz NOT NULL
);
CREATE INDEX IF NOT EXISTS ix_audit_tenant_at ON audit_records (tenant_id, at);

CREATE TABLE IF NOT EXISTS outbox (
    id          uuid PRIMARY KEY,
    tenant_id   uuid REFERENCES tenants(id),
    event       text NOT NULL,
    topic       text NOT NULL,
    payload     jsonb NOT NULL,
    published   boolean NOT NULL DEFAULT false,
    created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS ix_outbox_unpublished ON outbox (published, created_at) WHERE published = false;

CREATE TABLE IF NOT EXISTS idempotency_keys (
    id            uuid NOT NULL,
    tenant_id     uuid,
    operation     text NOT NULL,
    key           text NOT NULL,
    payload_hash  text NOT NULL,
    result        jsonb NOT NULL,
    created_at    timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, operation, key)
);

CREATE TABLE IF NOT EXISTS elevation_leases (
    id             uuid PRIMARY KEY,
    operator       uuid NOT NULL REFERENCES principals(id),
    target_tenant  uuid NOT NULL REFERENCES tenants(id),
    capability     text NOT NULL,
    scope_paths    text[] NOT NULL DEFAULT '{}',
    reason         text NOT NULL,
    approver       uuid NOT NULL REFERENCES principals(id),
    read_only      boolean NOT NULL DEFAULT true,
    granted_at     timestamptz NOT NULL DEFAULT now(),
    expires_at     timestamptz NOT NULL
);

-- ============ RLS (final containment barrier) ============

ALTER TABLE memberships        ENABLE ROW LEVEL SECURITY;
ALTER TABLE scope_nodes        ENABLE ROW LEVEL SECURITY;
ALTER TABLE scope_grants       ENABLE ROW LEVEL SECURITY;
ALTER TABLE capability_grants  ENABLE ROW LEVEL SECURITY;
ALTER TABLE delta_stones       ENABLE ROW LEVEL SECURITY;
ALTER TABLE audit_records      ENABLE ROW LEVEL SECURITY;
ALTER TABLE outbox             ENABLE ROW LEVEL SECURITY;
ALTER TABLE idempotency_keys   ENABLE ROW LEVEL SECURITY;

-- Tenant GUC anchor: SET LOCAL keel.tenant_id (transaction-scoped).
CREATE OR REPLACE FUNCTION keel_current_tenant() RETURNS uuid AS $$
    SELECT NULLIF(current_setting('keel.tenant_id', true), '')::uuid
$$ LANGUAGE sql STABLE;

CREATE POLICY tenant_isolation_memberships ON memberships
    USING (tenant_id = keel_current_tenant() OR keel_current_tenant() IS NULL);
CREATE POLICY tenant_isolation_scope_nodes ON scope_nodes
    USING (tenant_id = keel_current_tenant() OR keel_current_tenant() IS NULL);
CREATE POLICY tenant_isolation_scope_grants ON scope_grants
    USING (tenant_id = keel_current_tenant() OR keel_current_tenant() IS NULL);
CREATE POLICY tenant_isolation_capability_grants ON capability_grants
    USING (tenant_id = keel_current_tenant() OR keel_current_tenant() IS NULL);
CREATE POLICY tenant_isolation_delta_stones ON delta_stones
    USING (tenant_id = keel_current_tenant() OR keel_current_tenant() IS NULL);
CREATE POLICY tenant_isolation_audit ON audit_records
    USING (tenant_id = keel_current_tenant() OR keel_current_tenant() IS NULL);
CREATE POLICY tenant_isolation_outbox ON outbox
    USING (tenant_id = keel_current_tenant() OR keel_current_tenant() IS NULL);
CREATE POLICY tenant_isolation_idempotency ON idempotency_keys
    USING (tenant_id = keel_current_tenant() OR keel_current_tenant() IS NULL);