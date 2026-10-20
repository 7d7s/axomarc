//! # Shared `ModelStore` conformance suite (LF-007)
//!
//! One contract, every adapter: every case in this module asserts the
//! behavioural contract of the [`ModelStore`](crate::store::ModelStore) port
//! (through the same [`ModelRuntime`] prepare pipeline the domain layers use)
//! exactly once, and every backend runs the same code —
//! [`InMemoryModelStore`](crate::store::InMemoryModelStore) from this crate's
//! own tests, `PgModelStore` / `MySqlModelStore` from `kelson-postgres`'s
//! matrix test files.
//!
//! ## Why this exists
//!
//! Before LF-007's harness the pg and mysql model-store matrices were
//! hand-copied twins: the same scenario written twice, so any semantic drift
//! between the drivers — or between a driver and the TestKit fake — could only
//! be caught by reading both bodies and noticing. Here the scenario lives in
//! one function; each adapter supplies only a connection, a migrated schema
//! and seeded tenants.
//!
//! ## Contract source of truth
//!
//! The cases below encode the *documented* port contract (the trait and
//! method docs in `crate::store`) plus the runtime write pipeline
//! (`insert` prepares id/tenant/timestamps; `update` is partial; the
//! soft-delete veil applies to reads and to the batch/counter writes). Where
//! the contract is silent, the case is deliberately not written — an
//! assertion invented here would pin one backend's accident as the rule.
//!
//! ## How adapters run it
//!
//! ```ignore
//! #[tokio::test]
//! async fn mysql_conformance_crud_roundtrip() {
//!     let Some(pool) = mysql_pool().await else { return };
//!     let store = kelson_mysql::MySqlModelStore::new(pool.clone());
//!     let tenant = mysql_seed_tenant(&pool).await;
//!     kelson_models::conformance::crud_roundtrip(&store, tenant);
//! }
//! ```
//!
//! Every case assumes a migrated schema (`database/migrations` /
//! `migrations-mysql`) and a tenant row in `tenants` for each `TenantId` it
//! receives (both SQL fabrics carry a `tenant_id` FK on the tables used here).
//! All row values are freshly generated per call, so cases are safe to run in
//! parallel against a shared database and safe to re-run without cleanup.

use kelson_foundation::{ErrorCategory, TenantId};
use kelson_query::PageReq;
use serde_json::{json, Value as Json};
use uuid::Uuid;

use crate::store::AggregateOp;
use crate::{
    FieldDef, FieldFilter, FieldType, ListOpts, ModelDef, ModelRuntime, ModelStore, Record,
};

/// The `appointments` table shape, mirrored 1:1 from migration
/// `20260822000002_appointments.sql` (both fabrics): soft-deleting,
/// tenant-scoped, with the partial-unique double-booking guard the real
/// module relies on.
pub fn appointments_model() -> ModelDef {
    ModelDef::new("ConformanceAppointment", "appointments")
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

/// The `scope_nodes` table shape, mirrored 1:1 from the foundation migration
/// (both fabrics): hard-deleting, tenant-scoped, with the plain
/// `UNIQUE (tenant_id, path)` constraint the real scope tree relies on — the
/// port's upsert / insert-or-ignore contract surface that the partial index
/// on `appointments` deliberately cannot exercise.
///
/// `path` is declared `unique` so the in-memory backend applies the same
/// tenant-bound uniqueness the database constraint enforces.
pub fn scope_nodes_model() -> ModelDef {
    ModelDef::new("ConformanceScopeNode", "scope_nodes")
        .tenant_scoped()
        .hard_deletes()
        .field(FieldDef::new("id", FieldType::Uuid).required())
        .field(FieldDef::new("tenant_id", FieldType::Uuid).required())
        .field(FieldDef::new("kind", FieldType::Text).required())
        .field(FieldDef::new("name", FieldType::Text).required())
        .field(FieldDef::new("path", FieldType::Text).required().unique())
        .field(FieldDef::new("parent_id", FieldType::Uuid))
}

fn marker(prefix: &str) -> String {
    format!("conf-{prefix}-{}", Uuid::now_v7())
}

/// A full appointment input: patient/slot/status are explicit and the
/// version is passed (not left to DDL defaults) so the in-memory backend,
/// which has no DDL, stores exactly what the SQL backends store.
fn booking(patient: &str, slot: &str, version: i64) -> Record {
    let mut record = Record::new();
    record.insert("patient".into(), json!(patient));
    record.insert("slot".into(), json!(slot));
    record.insert("status".into(), json!("booked"));
    record.insert("version".into(), json!(version));
    record
}

fn scope_node(path: &str, name: &str) -> Record {
    let mut record = Record::new();
    record.insert("kind".into(), json!("conformance"));
    record.insert("name".into(), json!(name));
    record.insert("path".into(), json!(path));
    record
}

fn record_id(record: &Record) -> Uuid {
    record["id"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .expect("stored record carries a uuid id")
}

fn sorted_ids(rows: &[Record]) -> Vec<Uuid> {
    let mut ids: Vec<Uuid> = rows.iter().map(record_id).collect();
    ids.sort();
    ids
}

/// Aggregate results cross the wire as int8/numeric/DECIMAL depending on the
/// backend and column type (SQL `SUM` over integers is `numeric` on pg and
/// `DECIMAL` on MySQL), so compare numerically instead of by JSON variant.
fn as_number(value: &Json) -> i64 {
    if let Some(int) = value.as_i64() {
        return int;
    }
    value
        .as_f64()
        .map(|float| float.round() as i64)
        .unwrap_or_else(|| panic!("expected a JSON number, got {value:?}"))
}

// ------------------------------------------------------------------- cases

/// CRUD roundtrip: prepared insert (tenant/timestamps stamped), keyed and
/// filtered reads, partial update, soft-delete veil, trash toggle, and the
/// match-nothing posture for foreign / absent tenants.
pub fn crud_roundtrip<S: ModelStore>(store: &S, tenant: TenantId) {
    let runtime = ModelRuntime::new(appointments_model());
    let patient = marker("crud-patient");
    let slot = marker("crud-slot");

    let stored = runtime
        .insert(store, &booking(&patient, &slot, 1), Some(tenant))
        .expect("insert succeeds");
    let id = record_id(&stored);
    assert_eq!(
        stored["tenant_id"],
        json!(tenant.as_uuid().to_string()),
        "insert stamps the caller's tenant"
    );
    assert!(
        stored["created_at"].is_string() && stored["updated_at"].is_string(),
        "insert round-trips framework timestamps"
    );

    let found = runtime
        .find(store, id, Some(tenant))
        .expect("find succeeds")
        .expect("find by id returns the row");
    assert_eq!(found["patient"], json!(patient));

    let page = runtime
        .list(
            store,
            Some(tenant),
            vec![FieldFilter::eq("patient", patient.clone())],
            &[],
            None,
        )
        .expect("list succeeds");
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.total, Some(1));

    // Partial update: only the supplied columns move.
    let mut changes = Record::new();
    changes.insert("status".into(), json!("cancelled"));
    changes.insert("version".into(), json!(2));
    let updated = runtime
        .update(store, id, Some(tenant), &changes)
        .expect("update succeeds");
    assert_eq!(updated["status"], json!("cancelled"));
    assert_eq!(updated["version"], json!(2));
    assert_eq!(
        updated["patient"],
        json!(patient),
        "update is partial: untouched columns survive"
    );

    // Soft delete: veiled by default, visible behind the trash toggle.
    assert!(store
        .delete(runtime.model(), id, Some(tenant))
        .expect("delete succeeds"));
    assert!(
        runtime.find(store, id, Some(tenant)).unwrap().is_none(),
        "soft-deleted row is veiled"
    );
    let trashed = store
        .find(runtime.model(), id, Some(tenant), true)
        .expect("trash-toggle find succeeds")
        .expect("trash toggle finds the row");
    assert!(
        trashed["deleted_at"].is_string(),
        "soft delete stamps deleted_at"
    );

    // Tenant posture: foreign tenant and no tenant both match nothing.
    assert!(
        store
            .find(runtime.model(), id, Some(TenantId::new()), true)
            .unwrap()
            .is_none(),
        "foreign tenant cannot read the row even with the trash toggle"
    );
    assert!(
        store.find(runtime.model(), id, None, true).unwrap().is_none(),
        "a tenant-scoped model queried without a tenant matches nothing"
    );
}

/// Tenant isolation end-to-end: identical rows for two tenants coexist, each
/// tenant reads only its own rows, and cross-tenant writes are no-ops.
pub fn tenant_isolation<S: ModelStore>(store: &S, tenant_a: TenantId, tenant_b: TenantId) {
    let runtime = ModelRuntime::new(appointments_model());
    let patient = marker("iso-patient");
    let slot = marker("iso-slot");

    // Same patient + slot string in two tenants: the tenant dimension is
    // part of every uniqueness the schema declares, so both rows coexist.
    let row_a = runtime
        .insert(store, &booking(&patient, &slot, 1), Some(tenant_a))
        .expect("insert for tenant A");
    let id_a = record_id(&row_a);
    let row_b = runtime
        .insert(store, &booking(&patient, &slot, 1), Some(tenant_b))
        .expect("insert for tenant B");
    let id_b = record_id(&row_b);

    let page_b = runtime
        .list(
            store,
            Some(tenant_b),
            vec![FieldFilter::eq("patient", patient.clone())],
            &[],
            None,
        )
        .expect("list for tenant B");
    assert_eq!(page_b.items.len(), 1, "tenant B sees exactly its own row");
    assert_eq!(page_b.items[0]["id"], json!(id_b.to_string()));

    // Cross-tenant reads, updates and deletes all see nothing.
    assert!(runtime.find(store, id_a, Some(tenant_b)).unwrap().is_none());
    let mut changes = Record::new();
    changes.insert("status".into(), json!("cancelled"));
    let cross_update = store
        .update(runtime.model(), id_a, Some(tenant_b), &changes)
        .expect("cross-tenant update executes");
    assert!(cross_update.is_none(), "foreign tenant cannot update");
    let cross_delete = store
        .delete(runtime.model(), id_a, Some(tenant_b))
        .expect("cross-tenant delete executes");
    assert!(!cross_delete, "foreign tenant cannot delete");
    assert!(
        runtime.find(store, id_a, Some(tenant_a)).unwrap().is_some(),
        "owner row untouched"
    );
}

/// Soft-delete undo: `restore` clears the mark on a trashed row, never
/// resurrects a live one, and returns the row to every default read.
pub fn soft_delete_restore<S: ModelStore>(store: &S, tenant: TenantId) {
    let runtime = ModelRuntime::new(appointments_model());
    let patient = marker("restore-patient");
    let slot = marker("restore-slot");

    let stored = runtime
        .insert(store, &booking(&patient, &slot, 1), Some(tenant))
        .expect("insert succeeds");
    let id = record_id(&stored);

    assert!(store
        .delete(runtime.model(), id, Some(tenant))
        .expect("delete succeeds"));
    let restored = store
        .restore(runtime.model(), id, Some(tenant))
        .expect("restore succeeds")
        .expect("trashed row is restored");
    assert!(
        restored["deleted_at"].is_null(),
        "restore clears deleted_at back to NULL"
    );
    assert_eq!(
        runtime
            .find(store, id, Some(tenant))
            .expect("find succeeds")
            .expect("restored row is live again")["patient"],
        json!(patient)
    );

    let page = runtime
        .list(
            store,
            Some(tenant),
            vec![FieldFilter::eq("patient", patient.clone())],
            &[],
            None,
        )
        .expect("list succeeds");
    assert_eq!(page.total, Some(1), "restored row is visible in default reads");

    assert!(
        store
            .restore(runtime.model(), id, Some(tenant))
            .expect("second restore executes")
            .is_none(),
        "an already-live row is never 'restored' again"
    );
}

/// **Trash contract (adjudicated 2026-10-19, backlog #1).** The port-level
/// `update` is the raw write primitive and carries **no trash veil**: a
/// trashed row IS written and stays trashed (Laravel's instance-`save()`
/// semantics). The veil lives in the batch write, in `delete`, and in the
/// runtime's pre-read — where a deliberate write to a trashed row is the
/// explicit `with_trashed` opt-in.
pub fn update_on_a_trashed_row_writes_and_stays_trashed<S: ModelStore>(
    store: &S,
    tenant: TenantId,
) {
    let runtime = ModelRuntime::new(appointments_model());
    let patient = marker("trash-update-patient");
    let slot = marker("trash-update-slot");

    let stored = runtime
        .insert(store, &booking(&patient, &slot, 1), Some(tenant))
        .expect("insert succeeds");
    let id = record_id(&stored);
    assert!(
        store
            .delete(runtime.model(), id, Some(tenant))
            .expect("delete succeeds"),
        "the row is trashed"
    );

    // The raw port write reaches the trashed row.
    let mut changes = Record::new();
    changes.insert("status".into(), json!("cancelled"));
    let written = store
        .update(runtime.model(), id, Some(tenant), &changes)
        .expect("update executes")
        .expect("the trashed row is written (the port write has no trash veil)");
    assert_eq!(
        written["status"],
        json!("cancelled"),
        "the change lands on the trashed row"
    );
    assert!(
        !written["deleted_at"].is_null(),
        "the row is still trashed after the write"
    );

    // ... and it is still invisible to the default reads.
    assert!(
        runtime
            .find(store, id, Some(tenant))
            .expect("find succeeds")
            .is_none(),
        "the trashed row stays out of the default reads"
    );
    let trashed = runtime
        .find_trashed(store, id, Some(tenant))
        .expect("find_trashed succeeds")
        .expect("the trashed row is visible with the veil lifted");
    assert_eq!(trashed["status"], json!("cancelled"));

    // The domain write refuses the trashed row by default ...
    let mut changes = Record::new();
    changes.insert("status".into(), json!("cancelled"));
    assert!(
        runtime
            .update(store, id, Some(tenant), &changes)
            .is_err(),
        "the runtime's update reads the trashed row as 'not found'"
    );

    // ... unless the caller opts in explicitly (`withTrashed()->update()`).
    let written = runtime
        .update_with_trashed(store, id, Some(tenant), &changes)
        .expect("the with_trashed write succeeds");
    assert_eq!(written["status"], json!("cancelled"));
    assert!(
        !written["deleted_at"].is_null(),
        "a with_trashed write leaves the row trashed"
    );
}

/// **Trash contract:** deleting twice is a no-op — the second delete reports
/// "nothing done" and does **not** re-stamp `deleted_at`.
pub fn delete_twice_is_a_noop_and_does_not_restamp<S: ModelStore>(
    store: &S,
    tenant: TenantId,
) {
    let runtime = ModelRuntime::new(appointments_model());
    let patient = marker("trash-delete-patient");
    let slot = marker("trash-delete-slot");

    let stored = runtime
        .insert(store, &booking(&patient, &slot, 1), Some(tenant))
        .expect("insert succeeds");
    let id = record_id(&stored);

    assert!(
        store
            .delete(runtime.model(), id, Some(tenant))
            .expect("first delete succeeds"),
        "the first delete trashes the row"
    );
    let trashed = runtime
        .find_trashed(store, id, Some(tenant))
        .expect("find_trashed succeeds")
        .expect("the trashed row is visible with the veil lifted");
    let stamp = trashed["deleted_at"].clone();

    assert!(
        !store
            .delete(runtime.model(), id, Some(tenant))
            .expect("second delete executes"),
        "deleting an already-trashed row reports 'nothing done'"
    );
    let after = runtime
        .find_trashed(store, id, Some(tenant))
        .expect("find_trashed succeeds")
        .expect("the trashed row is still there");
    assert_eq!(
        after["deleted_at"], stamp,
        "the second delete does not re-stamp deleted_at"
    );
}

/// **Trash contract:** the read paths and the batch write honour the veil —
/// a trashed row is invisible to a default read, visible with
/// `include_deleted`, and skipped by the batch write.
pub fn reads_and_batch_writes_honour_the_trash_veil<S: ModelStore>(
    store: &S,
    tenant: TenantId,
) {
    let runtime = ModelRuntime::new(appointments_model());
    let patient = marker("trash-read-patient");
    let slot = marker("trash-read-slot");

    let live = runtime
        .insert(store, &booking(&patient, &slot, 1), Some(tenant))
        .expect("insert succeeds");
    let live_id = record_id(&live);
    // The second row gets its OWN slot: the schema's double-booking guard
    // allows one live booked row per (tenant, patient, slot), so a second
    // live row for the same patient needs a second slot.
    let other_slot = marker("trash-read-slot-2");
    let trashed = runtime
        .insert(store, &booking(&patient, &other_slot, 2), Some(tenant))
        .expect("insert succeeds");
    let trashed_id = record_id(&trashed);
    assert!(
        store
            .delete(runtime.model(), trashed_id, Some(tenant))
            .expect("delete succeeds"),
        "one of the two rows is trashed"
    );

    // Default read: only the live row.
    assert!(
        runtime
            .find(store, trashed_id, Some(tenant))
            .expect("find succeeds")
            .is_none(),
        "a trashed row is invisible to the default read"
    );
    assert!(
        runtime
            .find(store, live_id, Some(tenant))
            .expect("find succeeds")
            .is_some(),
        "the live row is visible"
    );

    // Veil lifted: the trashed row resolves.
    assert!(
        store
            .find(runtime.model(), trashed_id, Some(tenant), true)
            .expect("find with the veil lifted succeeds")
            .is_some(),
        "include_deleted resolves the trashed row"
    );

    // The batch write skips trashed rows.
    let mut changes = Record::new();
    changes.insert("status".into(), json!("cancelled"));
    let updated = store
        .update_filtered(
            runtime.model(),
            Some(tenant),
            &[FieldFilter::eq("patient", patient.clone())],
            &changes,
        )
        .expect("batch update executes");
    assert_eq!(
        updated.len(),
        1,
        "the batch write touches the live row only"
    );
    assert_eq!(updated[0]["id"], live["id"]);
    let still = runtime
        .find_trashed(store, trashed_id, Some(tenant))
        .expect("find_trashed succeeds")
        .expect("the trashed row is still there");
    assert_eq!(
        still["status"],
        json!("booked"),
        "the batch write left the trashed row alone"
    );
}

/// `find_by` (first match, veil-aware) and `find_for_update` (row lock)
/// agree with `find` on ids and trash posture.
pub fn find_by_and_find_for_update<S: ModelStore>(store: &S, tenant: TenantId) {
    let runtime = ModelRuntime::new(appointments_model());
    let patient = marker("findby-patient");
    let slot = marker("findby-slot");

    let stored = runtime
        .insert(store, &booking(&patient, &slot, 1), Some(tenant))
        .expect("insert succeeds");
    let id = record_id(&stored);

    let by = store
        .find_by(
            runtime.model(),
            Some(tenant),
            &[FieldFilter::eq("patient", patient.clone())],
            false,
        )
        .expect("find_by succeeds")
        .expect("find_by matches the row");
    assert_eq!(record_id(&by), id);
    assert!(
        store
            .find_by(
                runtime.model(),
                Some(tenant),
                &[FieldFilter::eq("patient", marker("findby-miss"))],
                false,
            )
            .unwrap()
            .is_none(),
        "no match returns None"
    );

    let locked = store
        .find_for_update(runtime.model(), id, Some(tenant), false)
        .expect("find_for_update succeeds")
        .expect("row is lockable");
    assert_eq!(record_id(&locked), id);

    runtime.delete(store, id, Some(tenant)).expect("delete succeeds");
    assert!(
        store
            .find_by(
                runtime.model(),
                Some(tenant),
                &[FieldFilter::eq("patient", patient.clone())],
                false,
            )
            .unwrap()
            .is_none(),
        "find_by honors the soft-delete veil"
    );
    assert!(
        store
            .find_by(
                runtime.model(),
                Some(tenant),
                &[FieldFilter::eq("patient", patient.clone())],
                true,
            )
            .unwrap()
            .is_some(),
        "find_by include_deleted sees the trashed row"
    );
    assert!(
        store
            .find_for_update(runtime.model(), id, Some(tenant), false)
            .unwrap()
            .is_none(),
        "find_for_update honors the veil"
    );
    assert!(
        store
            .find_for_update(runtime.model(), id, Some(tenant), true)
            .unwrap()
            .is_some(),
        "find_for_update include_deleted sees the trashed row"
    );
}

/// List semantics: stable sort before pagination, `total` across the whole
/// match set, `page` mechanics, and the shared `ListOpts` validations.
pub fn list_sort_pagination_and_validation<S: ModelStore>(store: &S, tenant: TenantId) {
    let runtime = ModelRuntime::new(appointments_model());
    let patient = marker("list-patient");
    let base = marker("list-slot");
    // Fixed-width suffixes: lexicographic order == numeric order on every
    // backend's collation.
    let slots: Vec<String> = (1..=5).map(|i| format!("{base}-{i:02}")).collect();
    for (index, slot) in slots.iter().enumerate() {
        runtime
            .insert(
                store,
                &booking(&patient, slot, index as i64 + 1),
                Some(tenant),
            )
            .expect("insert succeeds");
    }
    let filter = vec![FieldFilter::eq("patient", patient.clone())];

    let ascending = store
        .list(
            runtime.model(),
            Some(tenant),
            &filter,
            &ListOpts {
                sort: Some(("slot".into(), false)),
                limit: Some(2),
                ..Default::default()
            },
        )
        .expect("sorted list succeeds");
    assert_eq!(ascending.items.len(), 2, "limit caps the row count");
    assert_eq!(
        ascending.total,
        Some(5),
        "total counts the whole match set, not the page"
    );
    assert_eq!(ascending.items[0]["slot"], json!(slots[0]));
    assert_eq!(ascending.items[1]["slot"], json!(slots[1]));

    let descending = store
        .list(
            runtime.model(),
            Some(tenant),
            &filter,
            &ListOpts {
                sort: Some(("slot".into(), true)),
                page: Some(PageReq {
                    page: 2,
                    per_page: 2,
                }),
                ..Default::default()
            },
        )
        .expect("paged list succeeds");
    assert_eq!(descending.page, 2);
    assert_eq!(descending.per_page, 2);
    assert_eq!(descending.total, Some(5));
    assert_eq!(descending.items.len(), 2);
    // Descending order is slots[4], [3], [2], [1], [0]; page 2 starts at
    // offset 2.
    assert_eq!(descending.items[0]["slot"], json!(slots[2]));
    assert_eq!(descending.items[1]["slot"], json!(slots[1]));

    let both = store.list(
        runtime.model(),
        Some(tenant),
        &filter,
        &ListOpts {
            page: Some(PageReq::default()),
            limit: Some(1),
            ..Default::default()
        },
    );
    let err = both.expect_err("page + limit are mutually exclusive");
    assert_eq!(err.category, ErrorCategory::Validation);

    let unknown_sort = store.list(
        runtime.model(),
        Some(tenant),
        &filter,
        &ListOpts {
            sort: Some(("not_a_field".into(), false)),
            ..Default::default()
        },
    );
    let err = unknown_sort.expect_err("sorting on an unknown field is refused");
    assert_eq!(err.category, ErrorCategory::Validation);
}

/// Cursor (keyset) pagination: the page starts *after* the anchor, so a
/// concurrent insert cannot shift the window the way OFFSET does.
///
/// The anchor is the `(column, value)` the previous page ended on; the store
/// renders it as a row comparison (`(slot) > ($1)`). The sort direction flips
/// the comparison for a descending sort. An unknown column is a validation
/// error, never a silently ignored anchor.
pub fn cursor_keyset_pagination<S: ModelStore>(store: &S, tenant: TenantId) {
    let runtime = ModelRuntime::new(appointments_model());
    let patient = marker("cursor-patient");
    let base = marker("cursor-slot");
    let slots: Vec<String> = (1..=5).map(|i| format!("{base}-{i:02}")).collect();
    for (index, slot) in slots.iter().enumerate() {
        runtime
            .insert(
                store,
                &booking(&patient, slot, index as i64 + 1),
                Some(tenant),
            )
            .expect("insert succeeds");
    }
    let filter = vec![FieldFilter::eq("patient", patient.clone())];

    // Page 1 ends on slot 2; page 2 starts strictly after it.
    let page2 = store
        .list(
            runtime.model(),
            Some(tenant),
            &filter,
            &ListOpts {
                sort: Some(("slot".into(), false)),
                limit: Some(2),
                after: Some(vec![("slot".into(), json!(slots[1]))]),
                ..Default::default()
            },
        )
        .expect("keyset list succeeds");
    assert_eq!(
        page2.items.len(),
        2,
        "the limit still caps the page size"
    );
    assert_eq!(page2.items[0]["slot"], json!(slots[2]), "the page starts after the anchor");
    assert_eq!(page2.items[1]["slot"], json!(slots[3]));
    assert_eq!(
        page2.total,
        Some(3),
        "total counts the window that is left, not the whole match set"
    );

    // The last page is short (or empty) once the anchor is past the end.
    let tail = store
        .list(
            runtime.model(),
            Some(tenant),
            &filter,
            &ListOpts {
                sort: Some(("slot".into(), false)),
                limit: Some(2),
                after: Some(vec![("slot".into(), json!(slots[4]))]),
                ..Default::default()
            },
        )
        .expect("keyset list succeeds");
    assert!(tail.items.is_empty(), "nothing sorts after the last row");
    assert_eq!(tail.total, Some(0), "the window is exhausted");

    // A descending sort flips the comparison: the anchor bounds the window
    // from the other side.
    let descending = store
        .list(
            runtime.model(),
            Some(tenant),
            &filter,
            &ListOpts {
                sort: Some(("slot".into(), true)),
                limit: Some(2),
                after: Some(vec![("slot".into(), json!(slots[3]))]),
                ..Default::default()
            },
        )
        .expect("keyset list succeeds");
    assert_eq!(descending.items[0]["slot"], json!(slots[2]));
    assert_eq!(descending.items[1]["slot"], json!(slots[1]));

    // THE POINT OF KEYSET: a row inserted BEFORE the anchor (it sorts earlier
    // than the anchor) must not shift the window. With OFFSET it would push
    // the page's contents down by one.
    runtime
        .insert(
            store,
            &booking(&patient, &format!("{base}-00"), 99),
            Some(tenant),
        )
        .expect("insert succeeds");
    let after_insert = store
        .list(
            runtime.model(),
            Some(tenant),
            &filter,
            &ListOpts {
                sort: Some(("slot".into(), false)),
                limit: Some(2),
                after: Some(vec![("slot".into(), json!(slots[1]))]),
                ..Default::default()
            },
        )
        .expect("keyset list succeeds");
    assert_eq!(
        after_insert.items[0]["slot"],
        json!(slots[2]),
        "a concurrent insert before the anchor does not shift the window"
    );

    // An unknown anchor column is refused rather than silently ignored.
    let unknown = store.list(
        runtime.model(),
        Some(tenant),
        &filter,
        &ListOpts {
            sort: Some(("slot".into(), false)),
            after: Some(vec![("not_a_field".into(), json!("x"))]),
            ..Default::default()
        },
    );
    let err = unknown.expect_err("an unknown anchor column is refused");
    assert_eq!(err.category, ErrorCategory::Validation);
}

/// Aggregates and existence: COUNT/SUM/MIN/MAX over the filtered landlord
/// set, SQL NULL semantics on empty match sets, the soft-delete veil, and
/// the shared numeric-field validation.
pub fn aggregate_and_exists<S: ModelStore>(store: &S, tenant: TenantId) {
    let runtime = ModelRuntime::new(appointments_model());
    let patient = marker("agg-patient");
    let base = marker("agg-slot");
    let slots: Vec<String> = (1..=3).map(|i| format!("{base}-{i:02}")).collect();
    let mut ids = Vec::new();
    for (index, slot) in slots.iter().enumerate() {
        let stored = runtime
            .insert(
                store,
                &booking(&patient, slot, index as i64 + 1),
                Some(tenant),
            )
            .expect("insert succeeds");
        ids.push(record_id(&stored));
    }
    let filter = vec![FieldFilter::eq("patient", patient.clone())];

    let count = store
        .aggregate(runtime.model(), Some(tenant), &filter, AggregateOp::Count)
        .expect("count succeeds");
    assert_eq!(as_number(&count), 3, "COUNT(*) counts matched live rows");
    let sum = store
        .aggregate(
            runtime.model(),
            Some(tenant),
            &filter,
            AggregateOp::Sum("version".into()),
        )
        .expect("sum succeeds");
    assert_eq!(as_number(&sum), 6, "SUM adds the matched versions");
    let min = store
        .aggregate(
            runtime.model(),
            Some(tenant),
            &filter,
            AggregateOp::Min("slot".into()),
        )
        .expect("min succeeds");
    assert_eq!(min, json!(slots[0]));
    let max = store
        .aggregate(
            runtime.model(),
            Some(tenant),
            &filter,
            AggregateOp::Max("slot".into()),
        )
        .expect("max succeeds");
    assert_eq!(max, json!(slots[2]));

    // Empty match set: COUNT is 0, SUM/AVG/MIN/MAX are SQL NULL.
    let miss = vec![FieldFilter::eq("patient", marker("agg-miss"))];
    let zero = store
        .aggregate(runtime.model(), Some(tenant), &miss, AggregateOp::Count)
        .expect("count succeeds");
    assert_eq!(as_number(&zero), 0);
    let null_sum = store
        .aggregate(
            runtime.model(),
            Some(tenant),
            &miss,
            AggregateOp::Sum("version".into()),
        )
        .expect("sum succeeds");
    assert_eq!(null_sum, Json::Null, "SUM over no rows is SQL NULL");

    // Existence: matches live rows only, veil-aware like every read.
    assert!(store
        .exists(runtime.model(), Some(tenant), &filter)
        .expect("exists succeeds"));
    assert!(!store
        .exists(runtime.model(), Some(tenant), &miss)
        .expect("exists succeeds"));

    // Soft-deleted rows never count.
    runtime
        .delete(store, ids[2], Some(tenant))
        .expect("delete succeeds");
    let count = store
        .aggregate(runtime.model(), Some(tenant), &filter, AggregateOp::Count)
        .expect("count succeeds");
    assert_eq!(as_number(&count), 2, "aggregates exclude trashed rows");
    let sum = store
        .aggregate(
            runtime.model(),
            Some(tenant),
            &filter,
            AggregateOp::Sum("version".into()),
        )
        .expect("sum succeeds");
    assert_eq!(as_number(&sum), 3);

    // The shared validation refuses non-numeric SUM fields.
    let err = store
        .aggregate(
            runtime.model(),
            Some(tenant),
            &filter,
            AggregateOp::Sum("patient".into()),
        )
        .expect_err("SUM on a text field is refused");
    assert_eq!(err.category, ErrorCategory::Validation);
}

/// Atomic counter writes: one-statement increment plus extra assignments,
/// `None` on missing or trashed rows, and the shared counter-field
/// validation.
pub fn increment_counter<S: ModelStore>(store: &S, tenant: TenantId) {
    let runtime = ModelRuntime::new(appointments_model());
    let patient = marker("inc-patient");
    let slot = marker("inc-slot");

    let stored = runtime
        .insert(store, &booking(&patient, &slot, 1), Some(tenant))
        .expect("insert succeeds");
    let id = record_id(&stored);

    let mut extra = Record::new();
    extra.insert("status".into(), json!("cancelled"));
    let bumped = store
        .increment(runtime.model(), id, Some(tenant), "version", 1, &extra)
        .expect("increment executes")
        .expect("live row increments");
    assert_eq!(bumped["version"], json!(2));
    assert_eq!(
        bumped["status"],
        json!("cancelled"),
        "extra assignments land in the same statement"
    );
    let again = store
        .increment(runtime.model(), id, Some(tenant), "version", 1, &Record::new())
        .expect("second increment executes")
        .expect("live row increments");
    assert_eq!(again["version"], json!(3));

    assert!(
        store
            .increment(
                runtime.model(),
                Uuid::now_v7(),
                Some(tenant),
                "version",
                1,
                &Record::new()
            )
            .expect("increment on a missing row executes")
            .is_none(),
        "no live row means Ok(None), not an error"
    );

    let second = runtime
        .insert(store, &booking(&marker("inc-b"), &marker("inc-slot-b"), 1), Some(tenant))
        .expect("insert succeeds");
    runtime
        .delete(store, record_id(&second), Some(tenant))
        .expect("delete succeeds");
    assert!(
        store
            .increment(
                runtime.model(),
                record_id(&second),
                Some(tenant),
                "version",
                1,
                &Record::new()
            )
            .unwrap()
            .is_none(),
        "trashed rows are veiled from counter writes"
    );

    let err = store
        .increment(runtime.model(), id, Some(tenant), "patient", 1, &Record::new())
        .expect_err("counting a text field is refused");
    assert_eq!(err.category, ErrorCategory::Validation);
}

/// Filter-keyed batch writes: unfiltered writes are refused, updates return
/// every matched row, and the soft-delete batch returns the stamped rows.
pub fn batch_update_and_delete_filtered<S: ModelStore>(store: &S, tenant: TenantId) {
    let runtime = ModelRuntime::new(appointments_model());
    let patient = marker("batch-patient");
    let base = marker("batch-slot");
    let mut ids = Vec::new();
    for index in 0..3 {
        let slot = format!("{base}-{index:02}");
        let stored = runtime
            .insert(store, &booking(&patient, &slot, 1), Some(tenant))
            .expect("insert succeeds");
        ids.push(record_id(&stored));
    }
    ids.sort();
    let filter = vec![FieldFilter::eq("patient", patient.clone())];

    let mut changes = Record::new();
    changes.insert("status".into(), json!("cancelled"));
    changes.insert("version".into(), json!(9));

    let err = store
        .update_filtered(runtime.model(), Some(tenant), &[], &changes)
        .expect_err("update_filtered refuses an empty filter list");
    assert_eq!(err.category, ErrorCategory::Validation);
    let err = store
        .delete_filtered(runtime.model(), Some(tenant), &[])
        .expect_err("delete_filtered refuses an empty filter list");
    assert_eq!(err.category, ErrorCategory::Validation);

    let updated = store
        .update_filtered(runtime.model(), Some(tenant), &filter, &changes)
        .expect("batch update succeeds");
    assert_eq!(updated.len(), 3, "batch update returns every matched row");
    assert_eq!(sorted_ids(&updated), ids, "the same rows were updated");
    for row in &updated {
        assert_eq!(row["version"], json!(9));
        assert_eq!(row["status"], json!("cancelled"));
    }
    let page = runtime
        .list(store, Some(tenant), filter.clone(), &[], None)
        .expect("list succeeds");
    assert_eq!(page.total, Some(3), "batch update keeps rows live");

    let deleted = store
        .delete_filtered(runtime.model(), Some(tenant), &filter)
        .expect("batch delete succeeds");
    assert_eq!(deleted.len(), 3, "batch delete returns every soft-marked row");
    assert_eq!(sorted_ids(&deleted), ids);
    for row in &deleted {
        assert!(
            row["deleted_at"].is_string(),
            "batch delete soft-marks with deleted_at"
        );
    }
    let page = runtime
        .list(store, Some(tenant), filter.clone(), &[], None)
        .expect("list succeeds");
    assert_eq!(page.total, Some(0), "batch-deleted rows are veiled");
    let page = store
        .list(
            runtime.model(),
            Some(tenant),
            &filter,
            &ListOpts {
                include_deleted: true,
                ..Default::default()
            },
        )
        .expect("trash list succeeds");
    assert_eq!(page.total, Some(3), "trash toggle still sees all three");
}

/// `force_delete` purges live and trashed rows alike, and reports honestly
/// on rows that are already gone.
pub fn force_delete_purges_trash<S: ModelStore>(store: &S, tenant: TenantId) {
    let runtime = ModelRuntime::new(appointments_model());

    assert!(
        !store
            .force_delete(runtime.model(), Uuid::now_v7(), Some(tenant))
            .expect("force_delete on a missing row executes"),
        "no row means false"
    );

    let live = runtime
        .insert(
            store,
            &booking(&marker("purge-a"), &marker("purge-slot-a"), 1),
            Some(tenant),
        )
        .expect("insert succeeds");
    assert!(store
        .force_delete(runtime.model(), record_id(&live), Some(tenant))
        .expect("force_delete executes"));
    assert!(
        store
            .find(runtime.model(), record_id(&live), Some(tenant), true)
            .unwrap()
            .is_none(),
        "force_delete purges a live row"
    );
    assert!(
        !store
            .delete(runtime.model(), record_id(&live), Some(tenant))
            .expect("delete on a purged row executes"),
        "a purged row cannot be deleted again"
    );

    let trashed = runtime
        .insert(
            store,
            &booking(&marker("purge-b"), &marker("purge-slot-b"), 1),
            Some(tenant),
        )
        .expect("insert succeeds");
    runtime
        .delete(store, record_id(&trashed), Some(tenant))
        .expect("soft delete succeeds");
    assert!(store
        .force_delete(runtime.model(), record_id(&trashed), Some(tenant))
        .expect("force_delete on a trashed row executes"));
    assert!(
        store
            .find(runtime.model(), record_id(&trashed), Some(tenant), true)
            .unwrap()
            .is_none(),
        "force_delete purges through the trash veil"
    );
}

/// Upsert on a plain unique key: first call inserts, the second merges into
/// the same row (identity preserved), and the uniqueness is per tenant.
pub fn upsert_merges_on_plain_unique<S: ModelStore>(
    store: &S,
    tenant_a: TenantId,
    tenant_b: TenantId,
) {
    let runtime = ModelRuntime::new(scope_nodes_model());
    let conflict = vec!["tenant_id".to_string(), "path".to_string()];
    let path = marker("node-path");

    let first_input = scope_node(&path, "first");
    let first = store
        .upsert(
            runtime.model(),
            &runtime
                .prepare_insert(&first_input, Some(tenant_a))
                .expect("insert input prepares"),
            &conflict,
        )
        .expect("first upsert inserts");
    assert_eq!(first["name"], json!("first"));

    let merge_input = scope_node(&path, "merged");
    let merged = store
        .upsert(
            runtime.model(),
            &runtime
                .prepare_insert(&merge_input, Some(tenant_a))
                .expect("merge input prepares"),
            &conflict,
        )
        .expect("second upsert merges");
    assert_eq!(merged["name"], json!("merged"));
    assert_eq!(
        record_id(&first),
        record_id(&merged),
        "the merge keeps the surviving row's identity"
    );

    let page = store
        .list(
            runtime.model(),
            Some(tenant_a),
            &[FieldFilter::eq("path", path.clone())],
            &ListOpts::default(),
        )
        .expect("list succeeds");
    assert_eq!(page.total, Some(1), "upsert never duplicates on conflict");

    // The unique key is (tenant_id, path): the same path in another tenant
    // is a different node.
    let other_input = scope_node(&path, "other-tenant");
    let other = store
        .upsert(
            runtime.model(),
            &runtime
                .prepare_insert(&other_input, Some(tenant_b))
                .expect("second-tenant input prepares"),
            &conflict,
        )
        .expect("upsert for the second tenant inserts");
    assert_eq!(other["name"], json!("other-tenant"));
    assert_ne!(record_id(&other), record_id(&first));
    let page = store
        .list(
            runtime.model(),
            Some(tenant_b),
            &[FieldFilter::eq("path", path.clone())],
            &ListOpts::default(),
        )
        .expect("list succeeds");
    assert_eq!(page.total, Some(1));
}

/// `insert_or_ignore`: the same id or the same unique field within the
/// tenant is dropped (`Ok(None)`), a different tenant is not a conflict.
pub fn insert_or_ignore_dedupes<S: ModelStore>(
    store: &S,
    tenant_a: TenantId,
    tenant_b: TenantId,
) {
    let runtime = ModelRuntime::new(scope_nodes_model());
    let path = marker("ignore-path");

    let first_input = scope_node(&path, "one");
    let prepared = runtime
        .prepare_insert(&first_input, Some(tenant_a))
        .expect("insert input prepares");
    let first = store
        .insert_or_ignore(runtime.model(), &prepared)
        .expect("first insert_or_ignore executes")
        .expect("the first write lands");
    assert_eq!(first["name"], json!("one"));

    assert!(
        store
            .insert_or_ignore(runtime.model(), &prepared)
            .expect("duplicate execute")
            .is_none(),
        "the same id is ignored, not an error"
    );

    let mut colliding = scope_node(&path, "two");
    colliding.insert("id".into(), json!(Uuid::now_v7().to_string()));
    let prepared_colliding = runtime
        .prepare_insert(&colliding, Some(tenant_a))
        .expect("colliding input prepares");
    assert!(
        store
            .insert_or_ignore(runtime.model(), &prepared_colliding)
            .expect("unique collision executes")
            .is_none(),
        "a unique-field collision is ignored, not an error"
    );

    let page = store
        .list(
            runtime.model(),
            Some(tenant_a),
            &[FieldFilter::eq("path", path.clone())],
            &ListOpts::default(),
        )
        .expect("list succeeds");
    assert_eq!(page.total, Some(1), "ignored writes leave one row");

    let other_input = scope_node(&path, "other-tenant");
    let other = store
        .insert_or_ignore(
            runtime.model(),
            &runtime
                .prepare_insert(&other_input, Some(tenant_b))
                .expect("second-tenant input prepares"),
        )
        .expect("second-tenant insert executes")
        .expect("the tenant dimension makes the key unique again");
    assert_eq!(other["name"], json!("other-tenant"));
    assert_ne!(
        record_id(&other),
        record_id(&first),
        "the same path in another tenant is a distinct row"
    );
}

/// Hard-delete models: `restore` is a no-op (there is no trash) and `delete`
/// removes the row outright, invisibly to every read including the trash
/// toggle.
pub fn hard_delete_and_restore_noop<S: ModelStore>(store: &S, tenant: TenantId) {
    let runtime = ModelRuntime::new(scope_nodes_model());
    let path = marker("hard-path");

    let stored = runtime
        .insert(store, &scope_node(&path, "doomed"), Some(tenant))
        .expect("insert succeeds");
    let id = record_id(&stored);

    assert!(
        store
            .restore(runtime.model(), id, Some(tenant))
            .expect("restore executes")
            .is_none(),
        "hard-delete models have no trash to restore from"
    );

    assert!(store
        .delete(runtime.model(), id, Some(tenant))
        .expect("delete succeeds"));
    assert!(
        store
            .find(runtime.model(), id, Some(tenant), true)
            .unwrap()
            .is_none(),
        "hard delete removes the row outright"
    );
    assert!(
        !store
            .delete(runtime.model(), id, Some(tenant))
            .expect("second delete executes"),
        "a removed row cannot be deleted again"
    );
    assert!(
        store
            .restore(runtime.model(), id, Some(tenant))
            .unwrap()
            .is_none(),
        "restore stays a no-op after the row is gone"
    );
}

/// The appointments double-booking guard: the schema's unique index (pg's
/// partial index, MySQL's STORED generated columns) surfaces as a
/// persistence error, and cancelling the row frees the slot.
///
/// **Database-only case**: the guard lives in the schema, and the in-memory
/// store has no indexes by design — its `insert_or_ignore` unique-field
/// surface is the closest analog and is covered by
/// [`insert_or_ignore_dedupes`]. Adapter test files call this case; the
/// in-memory self-tests intentionally do not.
pub fn unique_index_blocks_double_booking<S: ModelStore>(store: &S, tenant: TenantId) {
    let runtime = ModelRuntime::new(appointments_model());
    let patient = marker("book-patient");
    let slot = marker("book-slot");

    let first = runtime
        .insert(store, &booking(&patient, &slot, 1), Some(tenant))
        .expect("first booking succeeds");
    let id = record_id(&first);

    let duplicate = runtime.insert(store, &booking(&patient, &slot, 1), Some(tenant));
    let err = duplicate.expect_err("a second booked row for the same slot is rejected");
    assert_eq!(
        err.category,
        ErrorCategory::Persistence,
        "the unique violation surfaces as a persistence error"
    );

    // Cancel frees the slot: the guard's predicate excludes cancelled rows.
    let mut changes = Record::new();
    changes.insert("status".into(), json!("cancelled"));
    runtime
        .update(store, id, Some(tenant), &changes)
        .expect("cancel succeeds");
    runtime
        .insert(store, &booking(&patient, &slot, 1), Some(tenant))
        .expect("cancelled row no longer blocks the slot");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::InMemoryModelStore;

    // The fake contract (LF-007): every case the fabrics can run, the
    // in-memory backend must pass too — that is what makes TestKit-based
    // tests trustworthy as evidence about production behavior.

    #[test]
    fn crud_roundtrip_matches_the_contract() {
        let store = InMemoryModelStore::new();
        crud_roundtrip(&*store, TenantId::new());
    }

    #[test]
    fn tenant_isolation_matches_the_contract() {
        let store = InMemoryModelStore::new();
        tenant_isolation(&*store, TenantId::new(), TenantId::new());
    }

    #[test]
    fn soft_delete_restore_matches_the_contract() {
        let store = InMemoryModelStore::new();
        soft_delete_restore(&*store, TenantId::new());
    }

    #[test]
    fn update_on_a_trashed_row_matches_the_contract() {
        let store = InMemoryModelStore::new();
        update_on_a_trashed_row_writes_and_stays_trashed(&*store, TenantId::new());
    }

    #[test]
    fn delete_twice_matches_the_contract() {
        let store = InMemoryModelStore::new();
        delete_twice_is_a_noop_and_does_not_restamp(&*store, TenantId::new());
    }

    #[test]
    fn reads_and_batch_writes_match_the_contract() {
        let store = InMemoryModelStore::new();
        reads_and_batch_writes_honour_the_trash_veil(&*store, TenantId::new());
    }

    #[test]
    fn find_by_and_find_for_update_matches_the_contract() {
        let store = InMemoryModelStore::new();
        find_by_and_find_for_update(&*store, TenantId::new());
    }

    #[test]
    fn list_sort_pagination_and_validation_matches_the_contract() {
        let store = InMemoryModelStore::new();
        list_sort_pagination_and_validation(&*store, TenantId::new());
    }

    #[test]
    fn cursor_keyset_pagination_matches_the_contract() {
        let store = InMemoryModelStore::new();
        cursor_keyset_pagination(&*store, TenantId::new());
    }

    #[test]
    fn aggregate_and_exists_matches_the_contract() {
        let store = InMemoryModelStore::new();
        aggregate_and_exists(&*store, TenantId::new());
    }

    #[test]
    fn increment_counter_matches_the_contract() {
        let store = InMemoryModelStore::new();
        increment_counter(&*store, TenantId::new());
    }

    #[test]
    fn batch_update_and_delete_filtered_matches_the_contract() {
        let store = InMemoryModelStore::new();
        batch_update_and_delete_filtered(&*store, TenantId::new());
    }

    #[test]
    fn force_delete_purges_trash_matches_the_contract() {
        let store = InMemoryModelStore::new();
        force_delete_purges_trash(&*store, TenantId::new());
    }

    #[test]
    fn upsert_merges_on_plain_unique_matches_the_contract() {
        let store = InMemoryModelStore::new();
        upsert_merges_on_plain_unique(&*store, TenantId::new(), TenantId::new());
    }

    #[test]
    fn insert_or_ignore_dedupes_matches_the_contract() {
        let store = InMemoryModelStore::new();
        insert_or_ignore_dedupes(&*store, TenantId::new(), TenantId::new());
    }

    #[test]
    fn hard_delete_and_restore_noop_matches_the_contract() {
        let store = InMemoryModelStore::new();
        hard_delete_and_restore_noop(&*store, TenantId::new());
    }
}
