-- Keel PostgreSQL role separation (11-SPEC §24).
-- Migration owner ≠ platform runtime ≠ tenant runtime ≠ reporting.
-- Run ONCE as the cluster/bootstrap superuser. Never run app code as owner.
--
--   psql "$DATABASE_URL" -f database/roles.sql
--
-- Secrets/rotation are out-of-band (secret manager). Passwords here are
-- placeholders the operator MUST replace before any non-local use.

-- Owns schema objects. The only role allowed to run migrations.
DO $$ BEGIN
  IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'keel_owner') THEN
    CREATE ROLE keel_owner LOGIN PASSWORD 'REPLACE_ME_owner';
  END IF;
END $$;

-- Long-lived platform-plane runtime (no DDL, tenant-anchored via RLS GUC).
DO $$ BEGIN
  IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'keel_platform') THEN
    CREATE ROLE keel_platform LOGIN PASSWORD 'REPLACE_ME_platform';
  END IF;
END $$;

-- Tenant-plane runtime. Must set keel.tenant_id per transaction; RLS is the
-- final containment barrier (D-47).
DO $$ BEGIN
  IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'keel_tenant') THEN
    CREATE ROLE keel_tenant LOGIN PASSWORD 'REPLACE_ME_tenant';
  END IF;
END $$;

-- Read-only reporting replica access (no writes, no tenant leakage).
DO $$ BEGIN
  IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'keel_report') THEN
    CREATE ROLE keel_report LOGIN PASSWORD 'REPLACE_ME_report';
  END IF;
END $$;

-- Grant owner-produced objects to runtimes. Owner still owns; runtimes use.
ALTER DEFAULT PRIVILEGES FOR ROLE keel_owner GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO keel_platform, keel_tenant;
ALTER DEFAULT PRIVILEGES FOR ROLE keel_owner GRANT SELECT ON TABLES TO keel_report;
ALTER DEFAULT PRIVILEGES FOR ROLE keel_owner GRANT USAGE, SELECT ON SEQUENCES TO keel_platform, keel_tenant;

-- Runtime roles never own tables; they cannot escalate via ownership.
REASSIGN OWNED BY keel_platform TO keel_owner;