-- Medicare kernel role separation (05-SECURITY-ISOLATION §7.2).
-- Migration owner ≠ platform runtime ≠ tenant runtime ≠ reporting.
-- Run ONCE as the cluster/bootstrap superuser. Never run app code as owner.
--
--   psql "$DATABASE_URL" -f database/init/00_roles.sql
--
-- Secrets/rotation are out-of-band (secret manager). Passwords here are
-- placeholders the operator MUST replace before any non-local use.

-- Owns schema objects. The only role allowed to run migrations.
DO $$ BEGIN
  IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'medicare_owner') THEN
    CREATE ROLE medicare_owner NOLOGIN PASSWORD 'REPLACE_ME_owner';
  END IF;
END $$;

-- Tenant-plane runtime. Must set medicare.tenant_id per transaction; RLS is the
-- final containment barrier.
DO $$ BEGIN
  IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'medicare_app') THEN
    CREATE ROLE medicare_app LOGIN PASSWORD 'REPLACE_ME_app';
  END IF;
END $$;

-- Long-lived platform-plane runtime (break-glass, NO RLS policies applied).
-- Uses SECURITY DEFINER functions for specific privileged operations.
DO $$ BEGIN
  IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'medicare_platform') THEN
    CREATE ROLE medicare_platform LOGIN PASSWORD 'REPLACE_ME_platform';
  END IF;
END $$;

-- Read-only reporting replica access (no writes, no tenant leakage).
DO $$ BEGIN
  IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'medicare_report') THEN
    CREATE ROLE medicare_report LOGIN PASSWORD 'REPLACE_ME_report';
  END IF;
END $$;

-- Grant owner-produced objects to runtimes. Owner still owns; runtimes use.
ALTER DEFAULT PRIVILEGES FOR ROLE medicare_owner GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO medicare_app, medicare_platform;
ALTER DEFAULT PRIVILEGES FOR ROLE medicare_owner GRANT SELECT ON TABLES TO medicare_report;
ALTER DEFAULT PRIVILEGES FOR ROLE medicare_owner GRANT USAGE, SELECT ON SEQUENCES TO medicare_app, medicare_platform;

-- Runtime roles never own tables; they cannot escalate via ownership.
REASSIGN OWNED BY medicare_app TO medicare_owner;
REASSIGN OWNED BY medicare_platform TO medicare_owner;
REASSIGN OWNED BY medicare_report TO medicare_owner;