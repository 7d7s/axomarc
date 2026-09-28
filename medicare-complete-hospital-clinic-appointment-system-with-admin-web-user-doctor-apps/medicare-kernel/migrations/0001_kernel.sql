-- 0001_kernel.sql — kernel tables (docs/rust/03-DATA-MODEL.md §1).
-- RLS = the final isolation blade (D-03). tenant tables keyed by medicare.tenant_id GUC.

-- idempotent role creation (CI containers lack the init script roles)
DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'medicare_app') THEN
    CREATE ROLE medicare_app LOGIN;
  END IF;
  IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'medicare_platform') THEN
    CREATE ROLE medicare_platform LOGIN;
  END IF;
END $$;

-- ---------- tenancy & platform ----------
CREATE TABLE IF NOT EXISTS tenants (
  id         uuid PRIMARY KEY,
  slug       text UNIQUE NOT NULL,
  name       text NOT NULL,
  status     text NOT NULL DEFAULT 'provisioning', -- provisioning|active|suspended|archived
  plan       text NOT NULL DEFAULT 'standard',
  kb         jsonb NOT NULL DEFAULT '{}',
  created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS tenant_limits (
  tenant_id uuid REFERENCES tenants(id),
  metric    text,
  quota     bigint,
  PRIMARY KEY (tenant_id, metric)
);

-- ---------- hierarchy / scope lattice ----------
CREATE TABLE IF NOT EXISTS org_nodes (
  id         uuid PRIMARY KEY,
  tenant_id  uuid NOT NULL REFERENCES tenants(id),
  kind       text NOT NULL, -- legal_entity|region|business_unit|location|department|warehouse|cost_center
  parent_id  uuid REFERENCES org_nodes(id),
  name       text NOT NULL,
  path       text NOT NULL, -- materialized: LE1|LOC5|DEP3
  meta       jsonb NOT NULL DEFAULT '{}',
  active     boolean NOT NULL DEFAULT true,
  created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS org_nodes_tenant_path ON org_nodes (tenant_id, path text_pattern_ops);

-- ---------- identity & access ----------
CREATE TABLE IF NOT EXISTS identities (
  id            uuid PRIMARY KEY,
  kind          text NOT NULL,            -- phone_otp|email|platform (D-13)
  subject_key   text NOT NULL,
  email         text,
  phone_e164    text,
  password_hash text,
  status        text NOT NULL DEFAULT 'active',
  created_at    timestamptz NOT NULL DEFAULT now(),
  UNIQUE (kind, subject_key)
);

CREATE TABLE IF NOT EXISTS users (
  id           uuid PRIMARY KEY,
  tenant_id    uuid NOT NULL REFERENCES tenants(id),
  identity_id  uuid NOT NULL REFERENCES identities(id),
  display_name text NOT NULL,
  kind         text NOT NULL, -- admin|staff|doctor|patient
  status       text NOT NULL DEFAULT 'active',
  meta         jsonb NOT NULL DEFAULT '{}'
);
CREATE INDEX IF NOT EXISTS users_tenant_identity ON users (tenant_id, identity_id);

CREATE TABLE IF NOT EXISTS memberships (
  id      uuid PRIMARY KEY,
  user_id uuid NOT NULL REFERENCES users(id),
  node_id uuid NOT NULL REFERENCES org_nodes(id),
  title   text
);

CREATE TABLE IF NOT EXISTS roles (
  id        uuid PRIMARY KEY,
  tenant_id uuid REFERENCES tenants(id),
  name      text NOT NULL,
  built_in  boolean NOT NULL DEFAULT false
);

CREATE TABLE IF NOT EXISTS user_roles (
  user_id uuid REFERENCES users(id),
  role_id uuid REFERENCES roles(id),
  PRIMARY KEY (user_id, role_id)
);

CREATE TABLE IF NOT EXISTS grants (
  id            uuid PRIMARY KEY,
  tenant_id     uuid NOT NULL REFERENCES tenants(id),
  subject_type  text NOT NULL, -- user|role
  subject_id    uuid NOT NULL,
  action_class  text NOT NULL,
  module        text,
  node_id       uuid NOT NULL REFERENCES org_nodes(id),
  conditions    jsonb NOT NULL DEFAULT '{}',
  valid_from    timestamptz NOT NULL DEFAULT now(),
  valid_until   timestamptz,
  granted_by    uuid,
  delegation_of uuid REFERENCES grants(id)
);
CREATE INDEX IF NOT EXISTS grants_subject ON grants (tenant_id, subject_type, subject_id);
CREATE INDEX IF NOT EXISTS grants_node ON grants (tenant_id, node_id);

CREATE TABLE IF NOT EXISTS sessions (
  id           uuid PRIMARY KEY,
  identity_id  uuid NOT NULL REFERENCES identities(id),
  plane        text NOT NULL, -- tenant|platform|support (D-21)
  refresh_hash text NOT NULL,
  device       jsonb NOT NULL DEFAULT '{}',
  created_at   timestamptz NOT NULL DEFAULT now(),
  expires_at   timestamptz NOT NULL,
  revoked_at   timestamptz
);

CREATE TABLE IF NOT EXISTS otp_codes (
  id          uuid PRIMARY KEY,
  subject_key text NOT NULL,
  code_hash   text NOT NULL,
  attempts    int NOT NULL DEFAULT 0,
  sent_via    text NOT NULL DEFAULT 'sms',
  ip_bucket   text,
  expires_at  timestamptz NOT NULL,
  consumed_at timestamptz
);
CREATE INDEX IF NOT EXISTS otp_codes_subject ON otp_codes (subject_key, expires_at);

CREATE TABLE IF NOT EXISTS idempotency_keys (
  tenant_id  uuid,
  key        text,
  route      text,
  result     jsonb,
  created_at timestamptz NOT NULL DEFAULT now(),
  expires_at timestamptz NOT NULL,
  PRIMARY KEY (tenant_id, key)
);

CREATE TABLE IF NOT EXISTS feature_flags (
  tenant_id uuid REFERENCES tenants(id),
  flag      text NOT NULL,
  enabled   boolean NOT NULL DEFAULT false,
  cohort    jsonb NOT NULL DEFAULT '{}',
  PRIMARY KEY (tenant_id, flag)
);

-- ---------- deltas & outbox ----------
CREATE SEQUENCE IF NOT EXISTS deltas_global_seq;

CREATE TABLE IF NOT EXISTS deltas (
  partition_id        smallint NOT NULL DEFAULT 0,
  seq                 bigint NOT NULL DEFAULT nextval('deltas_global_seq'),
  tenant_id           uuid NOT NULL,
  stream              text NOT NULL,
  aggregate           text NOT NULL,
  aggregate_id        uuid NOT NULL,
  version             bigint NOT NULL,
  action              text NOT NULL, -- create|update|transition|compensate
  state               jsonb NOT NULL,
  changed             jsonb NOT NULL DEFAULT '{}',
  owner_path          text NOT NULL,
  actor_id            uuid,
  capability_grant_id uuid,
  causation_id        uuid,
  occurred_at         timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (partition_id, seq)
);
CREATE INDEX IF NOT EXISTS deltas_tenant_stream ON deltas (tenant_id, stream, seq);
CREATE INDEX IF NOT EXISTS deltas_aggregate ON deltas (tenant_id, aggregate, aggregate_id, version);

CREATE TABLE IF NOT EXISTS sync_cursors (
  tenant_id   uuid,
  consumer_id uuid,
  stream      text,
  last_seq    bigint NOT NULL DEFAULT 0,
  PRIMARY KEY (tenant_id, consumer_id, stream)
);

CREATE TABLE IF NOT EXISTS outbox (
  id         bigserial PRIMARY KEY,
  tenant_id  uuid NOT NULL,
  channel    text NOT NULL,
  target     text NOT NULL,
  payload    jsonb NOT NULL,
  dedupe_key text NOT NULL,
  attempts   int NOT NULL DEFAULT 0,
  next_at    timestamptz NOT NULL DEFAULT now(),
  status     text NOT NULL DEFAULT 'pending',
  created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS outbox_due ON outbox (status, next_at) WHERE status IN ('pending','failed');

CREATE TABLE IF NOT EXISTS platform_grants (
  id             uuid PRIMARY KEY,
  actor_id       uuid NOT NULL,
  tenant_id      uuid NOT NULL REFERENCES tenants(id),
  scope_prefix   text NOT NULL,
  action_classes text[] NOT NULL,
  reason         text NOT NULL,
  approved_by    uuid,
  valid_from     timestamptz NOT NULL DEFAULT now(),
  valid_until    timestamptz NOT NULL,
  revoked_at     timestamptz
);

-- ---------- RLS (03-DATA-MODEL §2) ----------
ALTER TABLE org_nodes          ENABLE ROW LEVEL SECURITY;
ALTER TABLE users              ENABLE ROW LEVEL SECURITY;
ALTER TABLE memberships        ENABLE ROW LEVEL SECURITY;
ALTER TABLE roles              ENABLE ROW LEVEL SECURITY;
ALTER TABLE grants             ENABLE ROW LEVEL SECURITY;
ALTER TABLE idempotency_keys   ENABLE ROW LEVEL SECURITY;
ALTER TABLE feature_flags      ENABLE ROW LEVEL SECURITY;
ALTER TABLE deltas             ENABLE ROW LEVEL SECURITY;
ALTER TABLE tenants            ENABLE ROW LEVEL SECURITY;

DROP POLICY IF EXISTS tenant_isolation ON org_nodes;
CREATE POLICY tenant_isolation ON org_nodes
  USING (tenant_id = current_setting('medicare.tenant_id', true)::uuid);
DROP POLICY IF EXISTS tenant_isolation ON users;
CREATE POLICY tenant_isolation ON users
  USING (tenant_id = current_setting('medicare.tenant_id', true)::uuid);
DROP POLICY IF EXISTS tenant_isolation ON memberships;
CREATE POLICY tenant_isolation ON memberships USING (true); -- joined via users in queries; hardened M2-04
DROP POLICY IF EXISTS tenant_isolation ON roles;
CREATE POLICY tenant_isolation ON roles
  USING (tenant_id IS NULL OR tenant_id = current_setting('medicare.tenant_id', true)::uuid);
DROP POLICY IF EXISTS tenant_isolation ON grants;
CREATE POLICY tenant_isolation ON grants
  USING (tenant_id = current_setting('medicare.tenant_id', true)::uuid);
DROP POLICY IF EXISTS tenant_isolation ON idempotency_keys;
CREATE POLICY tenant_isolation ON idempotency_keys
  USING (tenant_id = current_setting('medicare.tenant_id', true)::uuid);
DROP POLICY IF EXISTS tenant_isolation ON feature_flags;
CREATE POLICY tenant_isolation ON feature_flags
  USING (tenant_id IS NULL OR tenant_id = current_setting('medicare.tenant_id', true)::uuid);
DROP POLICY IF EXISTS tenant_isolation ON deltas;
CREATE POLICY tenant_isolation ON deltas
  USING (tenant_id = current_setting('medicare.tenant_id', true)::uuid);
DROP POLICY IF EXISTS tenant_isolation ON tenants;
CREATE POLICY tenant_isolation ON tenants
  USING (id = current_setting('medicare.tenant_id', true)::uuid);

-- ---------- grants to the two plane roles ----------
GRANT USAGE ON SCHEMA public TO medicare_app, medicare_platform;
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO medicare_app;
GRANT ALL ON ALL TABLES IN SCHEMA public TO medicare_platform;
GRANT USAGE ON ALL SEQUENCES IN SCHEMA public TO medicare_app, medicare_platform;

-- platform-only tables denied to the app role (belt & braces pre-schema split)
REVOKE SELECT, INSERT, UPDATE, DELETE ON platform_grants, tenant_limits FROM medicare_app;