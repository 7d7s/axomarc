-- P5: Add 'native' to deploy_mode CHECK constraint.
-- SQLite cannot ALTER CONSTRAINT, so we recreate the app table
-- with the new CHECK and copy data over.

PRAGMA foreign_keys = OFF;

CREATE TABLE app_new (
    id            BLOB PRIMARY KEY,
    name          TEXT NOT NULL UNIQUE,
    owner         TEXT NOT NULL,
    env           TEXT NOT NULL CHECK (env IN ('dev','staging','prod')),
    git_repo      TEXT,
    image_ref     TEXT,
    config_yaml   TEXT NOT NULL,
    health_path   TEXT,
    status        TEXT NOT NULL CHECK (status IN ('active','draining','archived')) DEFAULT 'active',
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL,
    version       INTEGER NOT NULL DEFAULT 1,
    deploy_mode   TEXT NOT NULL DEFAULT 'pull'
        CHECK (deploy_mode IN ('pull', 'build', 'pack', 'native')),
    source_repo         TEXT,
    source_branch       TEXT,
    auto_deploy         INTEGER NOT NULL DEFAULT 0,
    auto_deploy_window  TEXT,
    max_auto_deploys_per_hour INTEGER NOT NULL DEFAULT 0
);

INSERT INTO app_new (
    id, name, owner, env, git_repo, image_ref, config_yaml,
    health_path, status, created_at, updated_at, version,
    deploy_mode, source_repo, source_branch, auto_deploy,
    auto_deploy_window, max_auto_deploys_per_hour
)
SELECT
    id, name, owner, env, git_repo, image_ref, config_yaml,
    health_path, status, created_at, updated_at, version,
    deploy_mode, source_repo, source_branch, auto_deploy,
    auto_deploy_window, max_auto_deploys_per_hour
FROM app;

DROP TABLE app;

ALTER TABLE app_new RENAME TO app;

CREATE INDEX idx_app_owner ON app(owner);
CREATE INDEX idx_app_env   ON app(env);

PRAGMA foreign_keys = ON;
