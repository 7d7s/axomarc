-- 0001_init.sql — the 7 core tables.
-- Per docs/architecture.md §2.1, schema is forward-only: no
-- DROP COLUMN, no destructive ALTER. New tables, new columns, new
-- indexes. If you find yourself wanting to "fix" the schema, add a
-- 0003_*.sql migration instead.
--
-- ID storage: BLOB (16 raw bytes). The Rust newtypes wrap `Uuid` and
-- rely on sqlx's built-in Uuid <-> BLOB codec. The hyphenated string
-- form is only used at API boundaries; on disk it's binary. This is
-- 36 -> 16 bytes per row cheaper and avoids hex-decode on every read.
--
-- Timestamps: INTEGER (unix seconds). The Timestamp newtype in
-- sovereign-core stores/reads this directly.

CREATE TABLE app (
    id            BLOB PRIMARY KEY,                              -- AppId(Uuid), 16 bytes
    name          TEXT NOT NULL UNIQUE,
    owner         TEXT NOT NULL,                                 -- 'user:<id>' | 'system' | 'team:<name>'
    env           TEXT NOT NULL CHECK (env IN ('dev','staging','prod')),
    git_repo      TEXT,
    image_ref     TEXT,                                          -- last deployed image
    config_yaml   TEXT NOT NULL,                                 -- last app.yaml
    health_path   TEXT,
    status        TEXT NOT NULL CHECK (status IN ('active','draining','archived')) DEFAULT 'active',
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL,
    version       INTEGER NOT NULL DEFAULT 1                     -- optimistic concurrency
);

CREATE INDEX idx_app_owner ON app(owner);
CREATE INDEX idx_app_env   ON app(env);

CREATE TABLE deployment (
    id            BLOB PRIMARY KEY,                            -- DeploymentId
    app_id            BLOB NOT NULL REFERENCES app(id),
    image_ref       TEXT NOT NULL,
    strategy        TEXT NOT NULL CHECK (strategy IN ('recreate','rolling','bluegreen')),
    status          TEXT NOT NULL CHECK (status IN ('pending','building','pushing','starting','healthy','failed','rolled_back')),
    started_at      INTEGER NOT NULL,
    finished_at     INTEGER,
    triggered_by    TEXT NOT NULL,                               -- 'user:alice' | 'system' | 'web:github'
    risk_score      INTEGER,                                     -- 0-100, V2+
    policy_decision TEXT,                                        -- JSON
    error           TEXT,
    version         INTEGER NOT NULL DEFAULT 1
);

CREATE INDEX idx_deployment_app_started ON deployment(app_id, started_at DESC);
CREATE INDEX idx_deployment_status      ON deployment(status);

CREATE TABLE domain (
    id            BLOB PRIMARY KEY,                              -- DomainId
    app_id            BLOB NOT NULL REFERENCES app(id),
    hostname      TEXT NOT NULL UNIQUE,
    tls_status    TEXT NOT NULL CHECK (tls_status IN ('provisioning','valid','expired','failed')) DEFAULT 'provisioning',
    tls_expires   INTEGER,
    created_at    INTEGER NOT NULL
);

CREATE INDEX idx_domain_app ON domain(app_id);

CREATE TABLE secret (
    id            BLOB PRIMARY KEY,                              -- SecretId
    app_id            BLOB NOT NULL REFERENCES app(id),
    key           TEXT NOT NULL,
    ciphertext    BLOB NOT NULL,                                 -- age-encrypted
    status        TEXT NOT NULL CHECK (status IN ('active','rotating','retired')) DEFAULT 'active',
    created_at    INTEGER NOT NULL,
    rotated_at    INTEGER,
    version       INTEGER NOT NULL DEFAULT 1,
    UNIQUE (app_id, key)
);

CREATE INDEX idx_secret_app ON secret(app_id);

CREATE TABLE server (
    id            BLOB PRIMARY KEY,                              -- ServerId
    hostname      TEXT NOT NULL UNIQUE,
    role          TEXT NOT NULL CHECK (role IN ('control_plane','agent')),
    api_url       TEXT NOT NULL,
    status        TEXT NOT NULL CHECK (status IN ('healthy','unreachable','drained','removed')),
    last_seen     INTEGER NOT NULL,
    cpu_cores     INTEGER,
    mem_mb        INTEGER,
    disk_gb       INTEGER,
    joined_at     INTEGER NOT NULL
);

CREATE INDEX idx_server_status ON server(status);

CREATE TABLE backup (
    id            BLOB PRIMARY KEY,                            -- BackupId
    target          TEXT NOT NULL,                               -- 'sqlite' | 'postgres:<app_id>'
    status          TEXT NOT NULL CHECK (status IN ('pending','success','failed','verified')) DEFAULT 'pending',
    size_bytes      INTEGER,
    location        TEXT,                                        -- 's3://...' | '/var/lib/sovereign/backups/...'
    started_at      INTEGER NOT NULL,
    finished_at     INTEGER,
    verified_at     INTEGER,
    verify_result   TEXT,                                        -- JSON
    error           TEXT
);

CREATE INDEX idx_backup_target_started ON backup(target, started_at DESC);
CREATE INDEX idx_backup_status         ON backup(status);

CREATE TABLE user (
    id            BLOB PRIMARY KEY,                              -- UserId
    email         TEXT NOT NULL UNIQUE,
    role          TEXT NOT NULL CHECK (role IN ('owner','admin','developer','readonly')),
    created_at    INTEGER NOT NULL,
    last_seen     INTEGER
);

CREATE INDEX idx_user_role ON user(role);
