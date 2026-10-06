//! PgModelStore integration matrix (S8 B-19 / C3 / C4): the semantic CRUD
//! port is proven against live PostgreSQL through the same model runtime the
//! domain modules use — insert/find/update/delete with RETURNING, soft-delete
//! veil + trash, tenant isolation, upsert merge, BelongsTo eager loading, and
//! the appointments partial-unique double-booking guard. Activated with
//! `--features pg-tests`; every test skips gracefully when DATABASE_URL is
//! unset, so local runs without a database stay green.

#![cfg(feature = "pg-tests")]

use keel_foundation::TenantId;
use keel_models::{
    FieldDef, FieldFilter, FieldType, ListOpts, ModelDef, ModelRegistry, ModelRuntime, ModelStore,
    Record, RelationDef,
};
use keel_postgres::{create_pool, run_migrations, PgModelStore, PgPool};
use serde_json::json;
use uuid::Uuid;

async fn pool() -> Option<PgPool> {
    let url = match std::env::var("DATABASE_URL") {
        Ok(url) if !url.is_empty() => url,
        _ => {
            eprintln!("pg-tests: DATABASE_URL not set; skipping");
            return None;
        }
    };
    match create_pool(&url, 4).await {
        Ok(pool) => {
            run_migrations(&pool, std::path::Path::new("."))
                .await
                .expect("migrations apply");
            Some(pool)
        }
        Err(e) => {
            eprintln!("pg-tests: cannot connect ({e}); skipping");
            None
        }
    }
}

/// Mirrors the appointments migration 1:1 (same shape the module declares).
fn appointment_model() -> ModelDef {
    ModelDef::new("Appointment", "appointments")
        .tenant_scoped()
        .field(FieldDef::new("id", FieldType::Uuid).required())
        .field(FieldDef::new("tenant_id", FieldType::Uuid).required())
        .field(FieldDef::new("patient", FieldType::Text).required())
        .field(FieldDef::new("slot", FieldType::Text).required())
        .field(
            FieldDef::new(
                "status",
                FieldType::Enum {
                    variants: vec!["booked".into(), "cancelled".into()],
                },
            )
            .required()
            .with_default("'booked'"),
        )
        .field(
            FieldDef::new("version", FieldType::Int64)
                .required()
                .with_default("1"),
        )
        .field(
            FieldDef::new("created_at", FieldType::Timestamp)
                .required()
                .with_default("now()"),
        )
        .field(
            FieldDef::new("updated_at", FieldType::Timestamp)
                .required()
                .with_default("now()"),
        )
        .field(FieldDef::new("deleted_at", FieldType::Timestamp))
}

fn booking(patient: &str, slot: &str) -> Record {
    let mut r = Record::new();
    r.insert("patient".into(), json!(patient));
    r.insert("slot".into(), json!(slot));
    r.insert("status".into(), json!("booked"));
    r
}

async fn seed_tenant(pool: &PgPool) -> TenantId {
    let tenant = TenantId::new();
    sqlx::query("INSERT INTO tenants (id, slug, name) VALUES ($1, $2, $3)")
        .bind(tenant.as_uuid())
        .bind(format!("pg-{}", Uuid::now_v7()))
        .bind("Pg Matrix Tenant")
        .execute(pool)
        .await
        .expect("tenant seeds");
    tenant
}

fn record_id(record: &Record) -> Uuid {
    record["id"].as_str().and_then(|s| s.parse().ok()).expect("record carries a uuid id")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pg_modelstore_crud_roundtrip_with_returning() {
    let Some(pool) = pool().await else { return };
    let store = PgModelStore::new(pool.clone());
    let runtime = ModelRuntime::new(appointment_model());
    let tenant = seed_tenant(&pool).await;

    // INSERT: prepared by the runtime, RETURNING carries server columns.
    let stored = runtime
        .insert(&store, &booking("P. One", "2026-10-01T09:00Z"), Some(tenant))
        .expect("insert succeeds");
    let id = record_id(&stored);
    assert_eq!(stored["tenant_id"], json!(*tenant.as_uuid().to_string()));
    assert!(
        stored["created_at"].is_string() && stored["updated_at"].is_string(),
        "RETURNING round-trips framework timestamps"
    );

    // FIND / LIST: tenant-scoped reads see the row.
    let found = runtime
        .find(&store, id, Some(tenant))
        .unwrap()
        .expect("find by id");
    assert_eq!(found["patient"], json!("P. One"));
    let page = runtime
        .list(
            &store,
            Some(tenant),
            vec![FieldFilter::eq("patient", "P. One")],
            &[],
            None,
        )
        .expect("list works");
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.total, Some(1));

    // UPDATE: partial change via RETURNING; version bump lands.
    let mut changes = Record::new();
    changes.insert("status".into(), json!("cancelled"));
    changes.insert("version".into(), json!(2));
    let updated = runtime
        .update(&store, id, Some(tenant), &changes)
        .expect("update succeeds");
    assert_eq!(updated["status"], json!("cancelled"));
    assert_eq!(updated["version"], json!(2));

    // DELETE: soft-delete veil hides the row; trash toggle reveals it.
    runtime.delete(&store, id, Some(tenant)).expect("delete succeeds");
    assert!(
        runtime.find(&store, id, Some(tenant)).unwrap().is_none(),
        "soft-deleted row is veiled"
    );
    let in_trash = store
        .find(runtime.model(), id, Some(tenant), true)
        .unwrap()
        .expect("trash toggle finds the row");
    assert!(
        in_trash["deleted_at"].is_string(),
        "soft delete stamps deleted_at"
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pg_modelstore_tenant_isolation_end_to_end() {
    let Some(pool) = pool().await else { return };
    let store = PgModelStore::new(pool.clone());
    let runtime = ModelRuntime::new(appointment_model());
    let tenant_a = seed_tenant(&pool).await;
    let tenant_b = seed_tenant(&pool).await;

    let row_a = runtime
        .insert(&store, &booking("Iso A", "2026-10-03T09:00Z"), Some(tenant_a))
        .unwrap();
    let id_a = record_id(&row_a);
    runtime
        .insert(&store, &booking("Iso B", "2026-10-03T09:00Z"), Some(tenant_b))
        .unwrap();

    // Same slot string, different tenants: both coexist.
    let list_b = runtime
        .list(&store, Some(tenant_b), vec![], &[], None)
        .unwrap();
    assert_eq!(list_b.items.len(), 1);
    assert_eq!(list_b.items[0]["patient"], json!("Iso B"));

    // Cross-tenant find/update/delete see nothing.
    assert!(runtime.find(&store, id_a, Some(tenant_b)).unwrap().is_none());
    let cross_update = store
        .update(
            runtime.model(),
            id_a,
            Some(tenant_b),
            &{
                let mut c = Record::new();
                c.insert("status".into(), json!("cancelled"));
                c
            },
        )
        .unwrap();
    assert!(cross_update.is_none(), "foreign tenant cannot update");
    let cross_delete = store.delete(runtime.model(), id_a, Some(tenant_b)).unwrap();
    assert!(!cross_delete, "foreign tenant cannot delete");
    assert!(
        runtime.find(&store, id_a, Some(tenant_a)).unwrap().is_some(),
        "owner row untouched"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pg_partial_unique_index_blocks_double_booking() {
    let Some(pool) = pool().await else { return };
    let store = PgModelStore::new(pool.clone());
    let runtime = ModelRuntime::new(appointment_model());
    let tenant = seed_tenant(&pool).await;

    let first = runtime
        .insert(&store, &booking("P. Two", "2026-10-02T09:00Z"), Some(tenant))
        .unwrap();
    let id = record_id(&first);

    let dup = runtime.insert(&store, &booking("P. Two", "2026-10-02T09:00Z"), Some(tenant));
    assert!(
        dup.is_err(),
        "partial unique index rejects a second BOOKED row for the same slot"
    );
    assert_eq!(
        dup.unwrap_err().category,
        keel_foundation::ErrorCategory::Persistence
    );

    // Cancel frees the slot: the predicate excludes cancelled rows.
    let mut changes = Record::new();
    changes.insert("status".into(), json!("cancelled"));
    changes.insert("version".into(), json!(2));
    runtime.update(&store, id, Some(tenant), &changes).unwrap();
    let rebook = runtime.insert(&store, &booking("P. Two", "2026-10-02T09:00Z"), Some(tenant));
    assert!(rebook.is_ok(), "cancelled row no longer blocks the slot");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pg_modelstore_upsert_merges_on_conflict() {
    let Some(pool) = pool().await else { return };
    // Test-only probe table with a PLAIN unique constraint (the appointments
    // unique index is partial, so it needs ON CONFLICT ... WHERE support;
    // plain-constraint upsert is the port's contract surface).
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS pg_modelstore_probe (
            id        uuid PRIMARY KEY,
            tenant_id uuid NOT NULL,
            name      text NOT NULL,
            value     text NOT NULL DEFAULT '',
            UNIQUE (tenant_id, name)
        )",
    )
    .execute(&pool)
    .await
    .expect("probe table creates");

    let model = ModelDef::new("Probe", "pg_modelstore_probe")
        .tenant_scoped()
        .hard_deletes()
        .field(FieldDef::new("id", FieldType::Uuid).required())
        .field(FieldDef::new("tenant_id", FieldType::Uuid).required())
        .field(FieldDef::new("name", FieldType::Text).required())
        .field(FieldDef::new("value", FieldType::Text).required());
    let store = PgModelStore::new(pool.clone());
    let tenant = seed_tenant(&pool).await;
    let conflict = vec!["tenant_id".to_string(), "name".to_string()];

    let mut input = Record::new();
    input.insert("name".into(), json!("gauge"));
    input.insert("value".into(), json!("first"));
    let first = store
        .upsert(&model, &runtime_prepare(&model, &input, tenant), &conflict)
        .expect("upsert inserts");
    assert_eq!(first["value"], json!("first"));

    let mut again = Record::new();
    again.insert("name".into(), json!("gauge"));
    again.insert("value".into(), json!("merged"));
    let merged = store
        .upsert(&model, &runtime_prepare(&model, &again, tenant), &conflict)
        .expect("upsert merges");
    assert_eq!(merged["value"], json!("merged"));
    assert_eq!(record_id(&first), record_id(&merged), "same row merged");

    let count = store
        .list(&model, Some(tenant), &[], &ListOpts::default())
        .unwrap();
    assert_eq!(count.total, Some(1), "upsert never duplicates on conflict");
}

/// Minimal runtime prep for direct-port upsert calls (id + tenant + casts).
fn runtime_prepare(model: &ModelDef, input: &Record, tenant: TenantId) -> Record {
    ModelRuntime::new(model.clone())
        .prepare_insert(input, Some(tenant))
        .expect("record prepares")
}
/// BelongsTo eager loading against live tables: credentials belong to a
/// principal; the runtime resolves the owner in one extra query.
fn principal_model() -> ModelDef {
    ModelDef::new("PrimePrincipal", "principals")
        .hard_deletes()
        .field(FieldDef::new("id", FieldType::Uuid).required())
        .field(FieldDef::new("email", FieldType::Text))
}

fn credential_model() -> ModelDef {
    ModelDef::new("PrimeCredential", "credentials")
        .hard_deletes()
        .field(FieldDef::new("id", FieldType::Uuid).required())
        .field(FieldDef::new("principal_id", FieldType::Uuid).required())
        .field(FieldDef::new("kind", FieldType::Text))
        .field(FieldDef::new("secret_hash", FieldType::Text).required())
        .relation(RelationDef::belongs_to("owner", "PrimePrincipal", "principal_id"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pg_modelstore_belongs_to_eager_load() {
    let Some(pool) = pool().await else { return };
    let mut registry = ModelRegistry::new();
    registry.register(principal_model()).unwrap();
    registry.register(credential_model()).unwrap();

    let store = PgModelStore::new(pool.clone());
    let principal_id = Uuid::now_v7();
    let email = format!("rel-{}@it.test", Uuid::now_v7());
    sqlx::query("INSERT INTO principals (id, status, email) VALUES ($1, 'active', $2)")
        .bind(principal_id)
        .bind(&email)
        .execute(&pool)
        .await
        .expect("principal seeds");

    let cred_rt = ModelRuntime::new(registry.get("PrimeCredential").cloned().unwrap())
        .with_registry(std::sync::Arc::new(registry));
    let mut cred = Record::new();
    cred.insert("id".into(), json!(Uuid::now_v7().to_string()));
    cred.insert("principal_id".into(), json!(principal_id.to_string()));
    cred.insert("kind".into(), json!("password_argon2"));
    cred.insert("secret_hash".into(), json!("$argon2id$test-only-hash"));
    let stored = store.insert(cred_rt.model(), &cred).expect("credential inserts");

    let mut rows = vec![stored];
    cred_rt
        .load_relation(&store, "owner", &mut rows, None)
        .expect("relation loads");
    assert_eq!(rows[0]["owner"]["email"], json!(email));
    assert_eq!(
        rows[0]["owner"]["id"],
        json!(principal_id.to_string()),
        "eager-loaded parent is keyed correctly"
    );
}

// ----------------------------------------------------- mysql driver matrix
// Wave 9 B-31 (ADR 0010): the model-store matrix over the second fabric —
// CRUD (select-back replaces RETURNING), tenant isolation, the appointments
// partial-unique guard reproduced with a STORED generated column + unique
// index, conflict upsert through `ON DUPLICATE KEY UPDATE`, and BelongsTo
// eager loading. Each test skips unless DATABASE_URL points at MySQL.

async fn mysql_pool() -> Option<keel_mysql::MySqlPool> {
    let url = match std::env::var("DATABASE_URL") {
        Ok(url) if !url.is_empty() => url,
        _ => {
            eprintln!("mysql matrix: DATABASE_URL not set; skipping");
            return None;
        }
    };
    if !url.starts_with("mysql://") {
        eprintln!("mysql matrix: DATABASE_URL is not mysql://; skipping");
        return None;
    }
    match keel_mysql::create_pool(&url, 4).await {
        Ok(pool) => {
            keel_mysql::run_migrations(&pool, std::path::Path::new("."))
                .await
                .expect("migrations apply");
            Some(pool)
        }
        Err(e) => {
            eprintln!("mysql matrix: cannot connect ({e}); skipping");
            None
        }
    }
}

async fn mysql_seed_tenant(pool: &keel_mysql::MySqlPool) -> TenantId {
    let tenant = TenantId::new();
    sqlx::query("INSERT INTO tenants (id, slug, name) VALUES (?, ?, ?)")
        .bind(tenant.as_uuid())
        .bind(format!("mysql-{}", Uuid::now_v7()))
        .bind("MySQL Matrix Tenant")
        .execute(pool)
        .await
        .expect("tenant seeds");
    tenant
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_modelstore_crud_roundtrip() {
    let Some(pool) = mysql_pool().await else { return };
    let store = keel_mysql::MySqlModelStore::new(pool.clone());
    let runtime = ModelRuntime::new(appointment_model());
    let tenant = mysql_seed_tenant(&pool).await;

    // INSERT: prepared by the runtime; the store selects the row back on
    // the same connection (MySQL has no RETURNING).
    let stored = runtime
        .insert(&store, &booking("P. One", "2026-10-01T09:00Z"), Some(tenant))
        .expect("insert succeeds");
    let id = record_id(&stored);
    assert_eq!(stored["tenant_id"], json!(*tenant.as_uuid().to_string()));
    assert!(
        stored["created_at"].is_string() && stored["updated_at"].is_string(),
        "select-back round-trips framework timestamps"
    );

    // FIND / LIST: tenant-scoped reads see the row.
    let found = runtime
        .find(&store, id, Some(tenant))
        .unwrap()
        .expect("find by id");
    assert_eq!(found["patient"], json!("P. One"));
    let page = runtime
        .list(
            &store,
            Some(tenant),
            vec![FieldFilter::eq("patient", "P. One")],
            &[],
            None,
        )
        .expect("list works");
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.total, Some(1));

    // UPDATE: partial change; version bump lands.
    let mut changes = Record::new();
    changes.insert("status".into(), json!("cancelled"));
    changes.insert("version".into(), json!(2));
    let updated = runtime
        .update(&store, id, Some(tenant), &changes)
        .expect("update succeeds");
    assert_eq!(updated["status"], json!("cancelled"));
    assert_eq!(updated["version"], json!(2));

    // DELETE: soft-delete veil hides the row; trash toggle reveals it.
    runtime.delete(&store, id, Some(tenant)).expect("delete succeeds");
    assert!(
        runtime.find(&store, id, Some(tenant)).unwrap().is_none(),
        "soft-deleted row is veiled"
    );
    let in_trash = store
        .find(runtime.model(), id, Some(tenant), true)
        .unwrap()
        .expect("trash toggle finds the row");
    assert!(
        in_trash["deleted_at"].is_string(),
        "soft delete stamps deleted_at"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_modelstore_tenant_isolation_end_to_end() {
    let Some(pool) = mysql_pool().await else { return };
    let store = keel_mysql::MySqlModelStore::new(pool.clone());
    let runtime = ModelRuntime::new(appointment_model());
    let tenant_a = mysql_seed_tenant(&pool).await;
    let tenant_b = mysql_seed_tenant(&pool).await;

    let row_a = runtime
        .insert(&store, &booking("Iso A", "2026-10-03T09:00Z"), Some(tenant_a))
        .unwrap();
    let id_a = record_id(&row_a);
    runtime
        .insert(&store, &booking("Iso B", "2026-10-03T09:00Z"), Some(tenant_b))
        .unwrap();

    // Same slot string, different tenants: both coexist.
    let list_b = runtime
        .list(&store, Some(tenant_b), vec![], &[], None)
        .unwrap();
    assert_eq!(list_b.items.len(), 1);
    assert_eq!(list_b.items[0]["patient"], json!("Iso B"));

    // Cross-tenant find/update/delete see nothing — MySQL has no RLS, so
    // the explicit tenant predicates ARE the isolation story (docs/05).
    assert!(runtime.find(&store, id_a, Some(tenant_b)).unwrap().is_none());
    let cross_update = store
        .update(
            runtime.model(),
            id_a,
            Some(tenant_b),
            &{
                let mut c = Record::new();
                c.insert("status".into(), json!("cancelled"));
                c
            },
        )
        .unwrap();
    assert!(cross_update.is_none(), "foreign tenant cannot update");
    let cross_delete = store.delete(runtime.model(), id_a, Some(tenant_b)).unwrap();
    assert!(!cross_delete, "foreign tenant cannot delete");
    assert!(
        runtime.find(&store, id_a, Some(tenant_a)).unwrap().is_some(),
        "owner row untouched"
    );
}

/// The appointments partial-unique guard on MySQL: three STORED generated
/// columns (`IF(pred, col, NULL)`) plus one unique index reproduce the pg
/// `WHERE deleted_at IS NULL AND status = 'booked'` semantics exactly —
/// non-matching rows key to all-NULL and MySQL unique indexes allow
/// repeated NULLs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_generated_unique_index_blocks_double_booking() {
    let Some(pool) = mysql_pool().await else { return };
    let store = keel_mysql::MySqlModelStore::new(pool.clone());
    let runtime = ModelRuntime::new(appointment_model());
    let tenant = mysql_seed_tenant(&pool).await;

    let first = runtime
        .insert(&store, &booking("P. Two", "2026-10-02T09:00Z"), Some(tenant))
        .unwrap();
    let id = record_id(&first);

    let dup = runtime.insert(&store, &booking("P. Two", "2026-10-02T09:00Z"), Some(tenant));
    assert!(
        dup.is_err(),
        "generated-column unique index rejects a second BOOKED row for the same slot"
    );
    assert_eq!(
        dup.unwrap_err().category,
        keel_foundation::ErrorCategory::Persistence
    );

    // Cancel frees the slot: the predicate columns go NULL for that row.
    let mut changes = Record::new();
    changes.insert("status".into(), json!("cancelled"));
    changes.insert("version".into(), json!(2));
    runtime.update(&store, id, Some(tenant), &changes).unwrap();
    let rebook = runtime.insert(&store, &booking("P. Two", "2026-10-02T09:00Z"), Some(tenant));
    assert!(rebook.is_ok(), "cancelled row no longer blocks the slot");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_modelstore_upsert_merges_on_conflict() {
    let Some(pool) = mysql_pool().await else { return };
    // Test-only probe table with a PLAIN unique constraint — the port's
    // `AS new ON DUPLICATE KEY UPDATE` contract surface.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS mysql_modelstore_probe (
            id        BINARY(16) PRIMARY KEY,
            tenant_id BINARY(16) NOT NULL,
            name      VARCHAR(191) NOT NULL,
            value     TEXT NOT NULL DEFAULT (''),
            UNIQUE KEY uniq_mysql_probe_tenant_name (tenant_id, name)
        ) DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci",
    )
    .execute(&pool)
    .await
    .expect("probe table creates");

    let model = ModelDef::new("Probe", "mysql_modelstore_probe")
        .tenant_scoped()
        .hard_deletes()
        .field(FieldDef::new("id", FieldType::Uuid).required())
        .field(FieldDef::new("tenant_id", FieldType::Uuid).required())
        .field(FieldDef::new("name", FieldType::Text).required())
        .field(FieldDef::new("value", FieldType::Text).required());
    let store = keel_mysql::MySqlModelStore::new(pool.clone());
    let tenant = mysql_seed_tenant(&pool).await;
    let conflict = vec!["tenant_id".to_string(), "name".to_string()];

    let mut input = Record::new();
    input.insert("name".into(), json!("gauge"));
    input.insert("value".into(), json!("first"));
    let first = store
        .upsert(&model, &runtime_prepare(&model, &input, tenant), &conflict)
        .expect("upsert inserts");
    assert_eq!(first["value"], json!("first"));

    let mut again = Record::new();
    again.insert("name".into(), json!("gauge"));
    again.insert("value".into(), json!("merged"));
    let merged = store
        .upsert(&model, &runtime_prepare(&model, &again, tenant), &conflict)
        .expect("upsert merges");
    assert_eq!(merged["value"], json!("merged"));
    assert_eq!(record_id(&first), record_id(&merged), "same row merged");

    let count = store
        .list(&model, Some(tenant), &[], &ListOpts::default())
        .unwrap();
    assert_eq!(count.total, Some(1), "upsert never duplicates on conflict");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_modelstore_belongs_to_eager_load() {
    let Some(pool) = mysql_pool().await else { return };
    let mut registry = ModelRegistry::new();
    registry.register(principal_model()).unwrap();
    registry.register(credential_model()).unwrap();

    let store = keel_mysql::MySqlModelStore::new(pool.clone());
    let principal_id = Uuid::now_v7();
    let email = format!("rel-{}@it.test", Uuid::now_v7());
    sqlx::query("INSERT INTO principals (id, status, email) VALUES (?, 'active', ?)")
        .bind(principal_id)
        .bind(&email)
        .execute(&pool)
        .await
        .expect("principal seeds");

    let cred_rt = ModelRuntime::new(registry.get("PrimeCredential").cloned().unwrap())
        .with_registry(std::sync::Arc::new(registry));
    let mut cred = Record::new();
    cred.insert("id".into(), json!(Uuid::now_v7().to_string()));
    cred.insert("principal_id".into(), json!(principal_id.to_string()));
    cred.insert("kind".into(), json!("password_argon2"));
    cred.insert("secret_hash".into(), json!("$argon2id$test-only-hash"));
    let stored = store.insert(cred_rt.model(), &cred).expect("credential inserts");

    let mut rows = vec![stored];
    cred_rt
        .load_relation(&store, "owner", &mut rows, None)
        .expect("relation loads");
    assert_eq!(rows[0]["owner"]["email"], json!(email));
    assert_eq!(
        rows[0]["owner"]["id"],
        json!(principal_id.to_string()),
        "eager-loaded parent is keyed correctly"
    );
}