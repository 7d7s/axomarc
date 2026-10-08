//! B-31 Phase D: the fluent-schema-builder round-trip gate.
//!
//! Every shipped `database/migrations-mysql/*.sql` file is authored here as a
//! fluent [`keel_mysql::schema::Migration`] program, and the test asserts the
//! rendered bytes equal the shipped bytes — so "the builder can express every
//! construct the fleet ships" is an enforced invariant, and any hand-edit to a
//! migration that the builder cannot represent fails the workspace gate.
//!
//! Layout rules live in the [`keel_mysql::schema`] module docs. The canonical
//! form (name field = longest name + 2, single-line indexes, two-space inline
//! comments) was adopted by the Phase D canonicalization; regeneration writes
//! the files back from these programs with:
//!
//! ```text
//! KEEL_SCHEMA_REGENERATE=1 cargo test -p keel-mysql --test schema_roundtrip
//! ```
//!
//! Without that variable the test is read-only (byte-equality + formatting
//! invariants) and needs no database.

use keel_mysql::schema::Migration;

fn foundation() -> Migration {
    Migration::new()
        .comment("-- MySQL 8 dialect twin of database/migrations/20260819000001_foundation.sql.")
        .comment("-- uuid: BINARY(16) (sqlx MySQL Uuid codec is binary)")
        .comment("-- MySQL: inline column REFERENCES is parsed but ignored by the server, so")
        .comment("-- every foreign key below is rewritten as a table-level FOREIGN KEY clause.")
        .comment("-- Keel foundation DDL (G09 companion). All ids are application-generated UUID v7.")
        .comment("-- RLS here is the FINAL containment barrier; application authorization decides")
        .comment("-- business permission. Runtime must connect with a NON-OWNER role (roles.sql).")
        .blank()
        .section("PLATFORM PLANE")
        .blank()
        .create_table("tenants", |t| {
            t.uuid("id").primary_key();
            t.varchar("slug", 128).not_null().unique();
            t.text("name").not_null();
            t.text("status").not_null().default("('active')");
            t.text("plan").not_null().default("('standard')");
            t.datetime6("created_at").not_null().default_now();
        })
        .blank()
        .create_table("principals", |t| {
            t.uuid("id").primary_key();
            t.text("status").not_null().default("('active')");
            t.varchar("email", 255).unique();
            t.varchar("phone", 64).unique();
            t.datetime6("created_at").not_null().default_now();
        })
        .blank()
        .create_table("credentials", |t| {
            t.uuid("id").primary_key();
            t.uuid("principal_id").not_null();
            t.text("kind")
                .not_null()
                .comment("password_argon2 | otp | api_key");
            t.text("secret_hash").not_null();
            t.datetime6("created_at").not_null().default_now();
            t.datetime6("expires_at");
            t.foreign_key("principal_id", "principals", "id");
        })
        .blank()
        .create_table("sessions", |t| {
            t.uuid("id").primary_key();
            t.uuid("principal_id").not_null();
            t.datetime6("expires_at").not_null();
            t.text("source_ip");
            t.text("user_agent");
            t.foreign_key("principal_id", "principals", "id");
        })
        .blank()
        .comment("-- Global revision counters (single-row table).")
        .create_table("revisions", |t| {
            t.tinyint1("id")
                .not_null()
                .default("1")
                .primary_key()
                .check("id = 1");
            t.bigint("platform_rev").not_null().default("0");
            t.bigint("tenant_rev").not_null().default("0");
            t.bigint("auth_rev").not_null().default("0");
            t.bigint("module_rev").not_null().default("0");
            t.bigint("feature_rev").not_null().default("0");
            t.bigint("schema_rev").not_null().default("0");
        })
        .comment("-- pg: INSERT ... DEFAULT VALUES ON CONFLICT DO NOTHING (single-row table).")
        .raw("INSERT INTO revisions (id) SELECT 1 FROM DUAL WHERE NOT EXISTS (SELECT 1 FROM revisions);")
        .blank()
        .section("TENANT-ANCHORED TABLES (RLS)")
        .blank()
        .create_table("memberships", |t| {
            t.uuid("id").primary_key();
            t.uuid("principal_id").not_null();
            t.uuid("tenant_id").not_null();
            t.text("status").not_null().default("('active')");
            t.datetime6("joined_at").not_null().default_now();
            t.foreign_key("principal_id", "principals", "id");
            t.foreign_key("tenant_id", "tenants", "id");
        })
        .create_index("ix_memberships_principal", "memberships", "principal_id")
        .create_index("ix_memberships_tenant", "memberships", "tenant_id")
        .blank()
        .create_table("scope_nodes", |t| {
            t.uuid("id").primary_key();
            t.uuid("tenant_id").not_null();
            t.text("kind").not_null();
            t.text("name").not_null();
            t.varchar("path", 255).not_null();
            t.uuid("parent_id");
            t.unique(&["tenant_id", "path"]);
            t.foreign_key("tenant_id", "tenants", "id");
        })
        .blank()
        .create_table("scope_grants", |t| {
            t.uuid("id").primary_key();
            t.uuid("tenant_id").not_null();
            t.uuid("membership_id").not_null();
            t.uuid("node_id").not_null();
            t.foreign_key("tenant_id", "tenants", "id");
            t.foreign_key("membership_id", "memberships", "id");
            t.foreign_key("node_id", "scope_nodes", "id");
        })
        .create_index("ix_scope_grants_membership", "scope_grants", "membership_id")
        .blank()
        .create_table("capability_grants", |t| {
            t.uuid("id").primary_key();
            t.uuid("tenant_id").not_null();
            t.uuid("membership_id").not_null();
            t.text("capability").not_null();
            t.json("conditions").not_null().default("('[]')");
            t.foreign_key("tenant_id", "tenants", "id");
            t.foreign_key("membership_id", "memberships", "id");
        })
        .blank()
        .comment("-- Delta machinery: per-stream monotonic sequences + immutable stones.")
        .create_table("delta_streams", |t| {
            t.varchar("stream_key", 191).primary_key();
            t.bigint("next_seq").not_null().default("0");
        })
        .blank()
        .create_table("delta_stones", |t| {
            t.uuid("id").primary_key();
            t.varchar("stream_key", 191).not_null();
            t.bigint("sequence").not_null();
            t.varchar("resource_type", 64).not_null();
            t.uuid("resource_id");
            t.bigint("resource_version").not_null();
            t.uuid("execution_id").not_null();
            t.uuid("principal_id");
            t.uuid("tenant_id");
            t.json("scope_snapshot").not_null().default("('[]')");
            t.json("ownership_snapshot").not_null().default("('[]')");
            t.json("`before`");
            t.text("command").not_null();
            t.json("`after`").not_null();
            t.uuid("causation_id");
            t.uuid("correlation_id");
            t.datetime6("at").not_null();
            t.unique(&["stream_key", "sequence"]);
            t.foreign_key("stream_key", "delta_streams", "stream_key");
            t.foreign_key("tenant_id", "tenants", "id");
        })
        .create_index(
            "ix_delta_stones_tenant_at",
            "delta_stones",
            "tenant_id, at",
        )
        .create_index(
            "ix_delta_stones_resource",
            "delta_stones",
            "resource_type, resource_id, sequence",
        )
        .blank()
        .create_table("audit_records", |t| {
            t.uuid("id").primary_key();
            t.uuid("execution_id").not_null();
            t.uuid("tenant_id");
            t.uuid("principal_id");
            t.text("action").not_null();
            t.text("resource").not_null();
            t.text("authority").not_null();
            t.text("outcome").not_null();
            t.json("detail").not_null().default("('{}')");
            t.datetime6("at").not_null();
            t.foreign_key("tenant_id", "tenants", "id");
        })
        .create_index("ix_audit_tenant_at", "audit_records", "tenant_id, at")
        .blank()
        .create_table("outbox", |t| {
            t.uuid("id").primary_key();
            t.uuid("tenant_id");
            t.text("event").not_null();
            t.text("topic").not_null();
            t.json("payload").not_null();
            t.tinyint1("published").not_null().default("0");
            t.datetime6("created_at").not_null().default_now();
            t.foreign_key("tenant_id", "tenants", "id");
        })
        .comment("-- MySQL has no partial index: WHERE published = false is dropped (the index is")
        .comment("-- non-unique, so only index size diverges; scans still filter on published).")
        .create_index("ix_outbox_unpublished", "outbox", "published, created_at")
        .blank()
        .create_table("idempotency_keys", |t| {
            t.uuid("id").not_null();
            t.uuid("tenant_id");
            t.varchar("operation", 64).not_null();
            t.varchar("`key`", 512)
                .not_null()
                .comment("utf8mb4 PK parts stay under 3072 bytes");
            t.text("payload_hash").not_null();
            t.json("result").not_null();
            t.datetime6("created_at").not_null().default_now();
            t.primary_key(&["tenant_id", "operation", "`key`"]);
        })
        .blank()
        .create_table("elevation_leases", |t| {
            t.uuid("id").primary_key();
            t.uuid("operator").not_null();
            t.uuid("target_tenant").not_null();
            t.text("capability").not_null();
            t.text("scope_paths")
                .not_null()
                .default("('')")
                .comment("pg text[]; comma-separated on MySQL");
            t.text("reason").not_null();
            t.uuid("approver").not_null();
            t.tinyint1("read_only").not_null().default("1");
            t.datetime6("granted_at").not_null().default_now();
            t.datetime6("expires_at").not_null();
            t.foreign_key("operator", "principals", "id");
            t.foreign_key("target_tenant", "tenants", "id");
            t.foreign_key("approver", "principals", "id");
        })
        .blank()
        .comment("-- No RLS on MySQL: tenant isolation is enforced solely by the tenant-guarded query builders (docs/05 divergence).")
}

fn services() -> Migration {
    Migration::new()
        .comment("-- MySQL 8 dialect twin of database/migrations/20260820000002_services.sql.")
        .comment("-- uuid: BINARY(16) (sqlx MySQL Uuid codec is binary)")
        .comment("-- Keel services DDL (S2 item 19): durable jobs, diagnostics chain, auth tokens.")
        .comment("-- The outbox table already exists in 20260819000001_foundation.sql (the outbox")
        .comment("-- writer landed with the engine); the pump (item 24) reads it. Same conventions:")
        .comment("-- application-generated UUID v7 ids, runtime role never owns tables (roles.sql).")
        .blank()
        .section("DURABLE JOB QUEUE (SKIP LOCKED, item 22/23)")
        .blank()
        .create_table("jobs_queue", |t| {
            t.uuid("id").primary_key();
            t.uuid("execution_id").not_null();
            t.text("name").not_null();
            t.json("payload").not_null().default("('{}')");
            t.text("queue").not_null().default("('default')");
            t.integer("attempt").not_null().default("0");
            t.integer("max_attempts").not_null().default("3");
            t.datetime6("deadline").not_null();
            t.datetime6("available_at").not_null().default_now();
            t.varchar("status", 64)
                .not_null()
                .default("'pending'")
                .comment("pending|running|succeeded|failed|dead");
            t.integer("priority").not_null().default("0");
            t.text("claimed_by");
            t.datetime6("claimed_at");
            t.uuid("causation_id");
            t.uuid("correlation_id");
            t.text("last_error");
            t.datetime6("enqueued_at").not_null().default_now();
            t.datetime6("finished_at");
        })
        .blank()
        .comment("-- Claim hot path: highest priority first, FIFO within priority, pending only.")
        .comment("-- MySQL has no partial index: WHERE status = 'pending' is dropped (non-unique")
        .comment("-- index, so only index size / scan selectivity diverges; the claim query keeps")
        .comment("-- its explicit status = 'pending' predicate).")
        .create_index("ix_jobs_claimable", "jobs_queue", "priority DESC, available_at")
        .create_index("ix_jobs_status", "jobs_queue", "status")
        .create_index("ix_jobs_correlation", "jobs_queue", "correlation_id")
        .blank()
        .section("DIAGNOSTIC CHAIN (item 29 db:inspect support)")
        .blank()
        .create_table("diag_chain", |t| {
            t.uuid("id").primary_key();
            t.uuid("execution_id").not_null();
            t.integer("seq").not_null();
            t.text("kind")
                .not_null()
                .comment("boot|validate|authz|execute|persist|publish|fail");
            t.text("message").not_null();
            t.json("data").not_null().default("('{}')");
            t.datetime6("created_at").not_null().default_now();
            t.unique(&["execution_id", "seq"]);
        })
        .create_index("ix_diag_execution", "diag_chain", "execution_id, seq")
        .blank()
        .section("AUTH TOKENS (item 21: hashed, TTL, single-use)")
        .blank()
        .create_table("auth_tokens", |t| {
            t.uuid("id").primary_key();
            t.uuid("principal_id").not_null();
            t.text("purpose").not_null().comment("api | reset | verify");
            t.varchar("token_hash", 64)
                .not_null()
                .unique()
                .comment("sha256 hex; plaintext never stored");
            t.text("label");
            t.datetime6("expires_at").not_null();
            t.datetime6("consumed_at");
            t.datetime6("created_at").not_null().default_now();
            t.foreign_key("principal_id", "principals", "id");
        })
        .create_index("ix_auth_tokens_principal", "auth_tokens", "principal_id")
}

fn http_sessions() -> Migration {
    Migration::new()
        .comment("-- MySQL 8 dialect twin of database/migrations/20260822000001_http_sessions.sql.")
        .comment("-- uuid: BINARY(16) (sqlx MySQL Uuid codec is binary)")
        .comment("-- HTTP session persistence (S7 C6/B-03): durable store for keel-session.")
        .comment("-- Separate from the legacy `sessions` table (keel-identity session rows):")
        .comment("-- keel-session ids are 64-hex strings with a JSON data bag.")
        .create_table("http_sessions", |t| {
            t.varchar("id", 64).primary_key();
            t.uuid("principal_id").not_null();
            t.uuid("tenant_id");
            t.json("data").not_null().default("('{}')");
            t.datetime6("created_at").not_null().default_now();
            t.datetime6("last_seen").not_null().default_now();
            t.foreign_key("principal_id", "principals", "id");
        })
        .create_index("ix_http_sessions_last_seen", "http_sessions", "last_seen")
}

fn appointments() -> Migration {
    Migration::new()
        .comment("-- MySQL 8 dialect twin of database/migrations/20260822000002_appointments.sql.")
        .comment("-- uuid: BINARY(16) (sqlx MySQL Uuid codec is binary)")
        .comment("-- Appointments domain table (S8 C3/B-04): first real module table persisted")
        .comment("-- through the framework ModelStore port (PgModelStore over keel-models).")
        .comment("-- Partial unique index = the real double-booking contention guard: one BOOKED")
        .comment("-- appointment per (tenant, patient, slot); cancelled rows do not block.")
        .create_table("appointments", |t| {
            t.uuid("id").primary_key();
            t.uuid("tenant_id").not_null();
            t.varchar("patient", 191).not_null();
            t.varchar("slot", 64).not_null();
            t.text("status").not_null().default("('booked')");
            t.bigint("version").not_null().default("1");
            t.datetime6("created_at").not_null().default_now();
            t.datetime6("updated_at").not_null().default_now();
            t.datetime6("deleted_at");
            t.foreign_key("tenant_id", "tenants", "id");
        })
        .create_index("ix_appointments_tenant", "appointments", "tenant_id")
        .comment("-- MySQL has no partial indexes. The WHERE deleted_at IS NULL AND")
        .comment("-- status = 'booked' predicate moves into STORED generated columns: rows that")
        .comment("-- do not match yield NULL in every part, and a MySQL UNIQUE index permits")
        .comment("-- repeated NULLs, so the guard is exactly the Postgres partial unique index.")
        .alter_table("appointments", |a| {
            a.uuid("tenant_id_active")
                .generated_stored("IF(deleted_at IS NULL AND status = 'booked', tenant_id, NULL)");
            a.varchar("patient_active", 191)
                .generated_stored("IF(deleted_at IS NULL AND status = 'booked', patient, NULL)");
            a.varchar("slot_active", 64)
                .generated_stored("IF(deleted_at IS NULL AND status = 'booked', slot, NULL)");
        })
        .create_unique_index(
            "ux_appointments_booked_slot",
            "appointments",
            "tenant_id_active, patient_active, slot_active",
        )
}

fn audit_hash_chain() -> Migration {
    Migration::new()
        .comment("-- MySQL 8 dialect twin of database/migrations/20260823000001_audit_hash_chain.sql.")
        .comment("-- uuid: BINARY(16) (sqlx MySQL Uuid codec is binary)")
        .comment("-- Sentinel (S5): audit hash-chain columns (D-74, STAGE-SENTINEL-SPEC).")
        .comment("-- Per-(tenant, day) chain segments; chain computation happens inside the")
        .comment("-- engine transaction via keel-integrity (no side channels). Old rows keep")
        .comment("-- NULL columns: pre-chain history is not backfilled, chains start fresh.")
        .comment("-- MySQL has no ADD COLUMN IF NOT EXISTS; plain ADD COLUMN (sqlx runs each")
        .comment("-- migration file exactly once).")
        .blank()
        .alter_table("audit_records", |a| {
            a.uuid("chain_id");
        })
        .alter_table("audit_records", |a| {
            a.blob("prev_hash");
        })
        .alter_table("audit_records", |a| {
            a.blob("chain_hash");
        })
        .blank()
        .create_index("ix_audit_chain", "audit_records", "tenant_id, chain_id, at")
}

fn email_verification() -> Migration {
    Migration::new()
        .comment("-- MySQL 8 dialect twin of database/migrations/20260824000001_email_verification.sql.")
        .comment("-- B-23 email verification: record when a principal proved ownership of the")
        .comment("-- contact email. NULL = unverified. Additive column only (forward-only")
        .comment("-- migration discipline, G08); existing rows stay unverified until proven.")
        .comment("-- MySQL has no ADD COLUMN IF NOT EXISTS; plain ADD COLUMN (sqlx runs each")
        .comment("-- migration file exactly once).")
        .alter_table("principals", |a| {
            a.datetime6("email_verified_at");
        })
}

fn oauth2() -> Migration {
    Migration::new()
        .comment("-- MySQL 8 dialect twin of database/migrations/20260826000001_oauth2.sql.")
        .comment("-- uuid: BINARY(16) (sqlx MySQL Uuid codec is binary)")
        .comment("-- OAuth2 module (B-21, Wave 8 P5): the client registry.")
        .comment("-- Secrets are argon2 hashed by the module (keel-identity password helpers);")
        .comment("-- the plaintext is shown exactly once at creation. The redirect allowlist")
        .comment("-- and scopes ride jsonb so membership checks are data, not string parsing.")
        .comment("-- Global registry (no tenant column): the token/revoke endpoints are")
        .comment("-- anonymous machine calls that resolve a client by its globally unique")
        .comment("-- client_id, while the authorization endpoint resolves it from the")
        .comment("-- session's tenant context through the same unique key.")
        .create_table("oauth_clients", |t| {
            t.uuid("id").primary_key();
            t.text("name").not_null();
            t.varchar("client_id", 191).not_null().unique();
            t.text("secret_hash").not_null();
            t.json("redirect_uris").not_null();
            t.json("scopes").not_null();
            t.bigint("version").not_null().default("1");
            t.datetime6("created_at").not_null().default_now();
            t.datetime6("updated_at").not_null().default_now();
        })
}

fn notifications() -> Migration {
    Migration::new()
        .comment("-- MySQL 8 dialect twin of database/migrations/20261001000001_notifications.sql.")
        .comment("-- uuid: BINARY(16) (sqlx MySQL Uuid codec is binary)")
        .comment("-- Wave 6 P5 database channel (19-PARITY \"Notifications v2\"): rows written by")
        .comment("-- the notification store when a typed notification dispatches on the")
        .comment("-- `database` channel. Read by `GET /api/v1/notifications`, marked read by")
        .comment("-- `POST /api/v1/notifications/read` (principal-scoped self service - a")
        .comment("-- caller only ever reads or marks its own rows).")
        .create_table("notifications", |t| {
            t.uuid("id").primary_key();
            t.uuid("principal_id").not_null();
            t.uuid("tenant_id");
            t.text("type").not_null();
            t.json("data").not_null().default("('{}')");
            t.datetime6("read_at");
            t.datetime6("created_at").not_null().default_now();
            t.foreign_key("principal_id", "principals", "id");
            t.foreign_key("tenant_id", "tenants", "id");
        })
        .comment("-- Feed order (newest first) + the unread fast path for the count meta.")
        .create_index(
            "ix_notifications_principal",
            "notifications",
            "principal_id, created_at DESC",
        )
        .comment("-- MySQL has no partial index: WHERE read_at IS NULL is dropped (non-unique")
        .comment("-- index, so only index size diverges; the count query keeps its explicit")
        .comment("-- read_at IS NULL predicate).")
        .create_index("ix_notifications_unread", "notifications", "principal_id")
}

fn job_batches() -> Migration {
    Migration::new()
        .comment("-- MySQL 8 dialect twin of database/migrations/20261001000002_job_batches.sql.")
        .comment("-- uuid: BINARY(16) (sqlx MySQL Uuid codec is binary)")
        .comment("-- Wave 6 P9 durable batch ledger (19-PARITY \"Queues advanced\" bullet 304;")
        .comment("-- B-32): one row per dispatched batch, written once at dispatch - the")
        .comment("-- membership is immutable, per-member state lives in jobs_queue, so")
        .comment("-- `batch.status` / `batch.cancel` / `batch.retry`, `queue:retry-batch` and")
        .comment("-- `queue:prune-batches` resolve live queue truth from any process and a")
        .comment("-- worker restart never loses a fan-out.")
        .create_table("job_batches", |t| {
            t.uuid("id").primary_key();
            t.text("name").not_null();
            t.json("members").not_null();
            t.text("on_complete");
            t.datetime6("created_at").not_null().default_now();
            t.datetime6("finished_at");
        })
        .comment("-- `queue:prune-batches` ages terminal records out strictly-before this")
        .comment("-- stamp; open batches (finished_at IS NULL) are never pruned.")
        .comment("-- MySQL has no partial index: WHERE finished_at IS NOT NULL is dropped")
        .comment("-- (non-unique index, so only index size diverges; the prune query keeps its")
        .comment("-- explicit finished_at IS NOT NULL predicate).")
        .create_index("ix_job_batches_finished_at", "job_batches", "finished_at")
}

/// The canonical fleet: every shipped MySQL migration and its builder program.
const MIGRATIONS: &[(&str, fn() -> Migration)] = &[
    ("20260819000001_foundation.sql", foundation),
    ("20260820000002_services.sql", services),
    ("20260822000001_http_sessions.sql", http_sessions),
    ("20260822000002_appointments.sql", appointments),
    ("20260823000001_audit_hash_chain.sql", audit_hash_chain),
    ("20260824000001_email_verification.sql", email_verification),
    ("20260826000001_oauth2.sql", oauth2),
    ("20261001000001_notifications.sql", notifications),
    ("20261001000002_job_batches.sql", job_batches),
];

#[test]
fn fluent_schema_builder_byte_round_trips_every_shipped_migration() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../database/migrations-mysql");
    let regenerate = std::env::var("KEEL_SCHEMA_REGENERATE").is_ok_and(|v| v == "1");
    for (file, program) in MIGRATIONS {
        let rendered = program().render();
        assert!(
            rendered.ends_with('\n'),
            "{file}: the builder must emit a trailing newline"
        );
        for (index, line) in rendered.lines().enumerate() {
            assert!(
                !line.ends_with(' '),
                "{file}:{}: trailing whitespace in builder output",
                index + 1
            );
            assert!(
                !line.contains('\t'),
                "{file}:{}: tab in builder output",
                index + 1
            );
        }
        let path = dir.join(file);
        if regenerate {
            std::fs::write(&path, &rendered)
                .unwrap_or_else(|e| panic!("{file}: regeneration write failed: {e}"));
            continue;
        }
        let shipped = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{file}: shipped migration unreadable: {e}"));
        assert_eq!(
            shipped, rendered,
            "{file}: shipped bytes differ from the builder program — either fix the \
             migration or the program, or run KEEL_SCHEMA_REGENERATE=1 to canonicalize"
        );
    }
}
