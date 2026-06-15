-- P3: Add deploy_mode and source columns to app table.
-- These enable git-based deploy, webhook triggers, and pack/unpack mode.

ALTER TABLE app ADD COLUMN deploy_mode TEXT NOT NULL DEFAULT 'pull'
    CHECK (deploy_mode IN ('pull', 'build', 'pack'));
ALTER TABLE app ADD COLUMN source_repo TEXT;
ALTER TABLE app ADD COLUMN source_branch TEXT;
ALTER TABLE app ADD COLUMN auto_deploy INTEGER NOT NULL DEFAULT 0;
ALTER TABLE app ADD COLUMN auto_deploy_window TEXT;
ALTER TABLE app ADD COLUMN max_auto_deploys_per_hour INTEGER NOT NULL DEFAULT 0;
