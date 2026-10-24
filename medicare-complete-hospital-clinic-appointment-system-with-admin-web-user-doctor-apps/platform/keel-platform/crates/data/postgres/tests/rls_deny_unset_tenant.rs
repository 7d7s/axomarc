//! MNC-010 live proof (CI-gated): the tenant RLS policies fail CLOSED on an
//! unset GUC.
//!
//! Needs the role-separated fabric the finding is about:
//! - `DATABASE_URL`         — the seeding/owner connection (applies migrations)
//! - `KEEL_RLS_TENANT_URL`  — a connection as the `kelson_tenant` runtime role
//! - `KEEL_RLS_PLATFORM_URL`— a connection as the `kelson_platform` break-glass role
//!
//! Skips (like every pg-tests file) when any of the three is absent; the
//! code-only posture is separately pinned by `rls_policy_pin.rs`.
#![cfg(feature = "pg-tests")]

use sqlx::postgres::PgPoolOptions;

fn env_url(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

#[tokio::test]
async fn an_unset_tenant_guc_denies_every_tenant_row() {
    let (Some(base_url), Some(tenant_url), Some(platform_url)) = (
        env_url("DATABASE_URL"),
        env_url("KEEL_RLS_TENANT_URL"),
        env_url("KEEL_RLS_PLATFORM_URL"),
    ) else {
        eprintln!("skipping: MNC-010 live proof needs DATABASE_URL + KEEL_RLS_TENANT_URL + KEEL_RLS_PLATFORM_URL");
        return;
    };

    let base = PgPoolOptions::new()
        .max_connections(2)
        .connect(&base_url)
        .await
        .expect("owner pool");
    let tenant = PgPoolOptions::new()
        .max_connections(2)
        .connect(&tenant_url)
        .await
        .expect("tenant-role pool");
    let platform = PgPoolOptions::new()
        .max_connections(2)
        .connect(&platform_url)
        .await
        .expect("platform-role pool");

    // Seed a principal + tenant + membership, keyed by a marker so the test
    // cleans up after itself.
    let marker: String = sqlx::query_scalar("SELECT gen_random_uuid()::text")
        .fetch_one(&base)
        .await
        .expect("marker uuid");
    let email = format!("mnc010-{marker}@example.test");
    let principal_id: String = sqlx::query_scalar(
        "INSERT INTO principals (id, status, email, created_at) VALUES (gen_random_uuid(), 'active', $1, now()) RETURNING id::text",
    )
    .bind(&email)
    .fetch_one(&base)
    .await
    .expect("seed principal");
    let tenant_id: String = sqlx::query_scalar(
        "INSERT INTO tenants (id, name, created_at) VALUES (gen_random_uuid(), $1, now()) RETURNING id::text",
    )
    .bind(&format!("mnc010-{marker}"))
    .fetch_one(&base)
    .await
    .expect("seed tenant");
    sqlx::query(
        "INSERT INTO memberships (id, principal_id, tenant_id, status, joined_at) VALUES (gen_random_uuid(), $1::uuid, $2::uuid, 'active', now())",
    )
    .bind(&principal_id)
    .bind(&tenant_id)
    .execute(&base)
    .await
    .expect("seed membership");

    // 1. The platform (break-glass) plane keeps cross-tenant access.
    let platform_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM memberships WHERE principal_id = $1::uuid")
            .bind(&principal_id)
            .fetch_one(&platform)
            .await
            .expect("platform reads through RLS");
    assert_eq!(platform_rows, 1, "the platform plane sees the row");

    // Steps 2 and 3 run on ONE connection: the GUC is session-scoped, so a
    // pooled second connection would not carry the anchor.
    let mut conn = tenant.acquire().await.expect("tenant conn");

    // 2. The tenant role with NO GUC set sees NOTHING (the deny — previously
    //    the `IS NULL` branch returned every tenant's rows).
    let denied: i64 = sqlx::query_scalar("SELECT count(*) FROM memberships")
        .fetch_one(&mut *conn)
        .await
        .expect("tenant read with unset GUC");
    assert_eq!(denied, 0, "an unset tenant GUC must deny every row (MNC-010)");

    // 3. Anchoring the GUC restores exactly the one tenant's rows.
    let anchored: String = sqlx::query_scalar("SELECT set_config('kelson.tenant_id', $1, false)")
        .bind(&tenant_id)
        .fetch_one(&mut *conn)
        .await
        .expect("anchor the GUC");
    assert_eq!(anchored, tenant_id, "the GUC anchors to the tenant");
    let scoped: i64 = sqlx::query_scalar("SELECT count(*) FROM memberships")
        .fetch_one(&mut *conn)
        .await
        .expect("tenant read with anchored GUC");
    assert_eq!(
        scoped, 1,
        "the anchored tenant sees exactly its own row"
    );

    // Cleanup (owner connection; best-effort but asserted).
    sqlx::query("DELETE FROM memberships WHERE principal_id = $1::uuid")
        .bind(&principal_id)
        .execute(&base)
        .await
        .expect("cleanup membership");
    sqlx::query("DELETE FROM principals WHERE id = $1::uuid")
        .bind(&principal_id)
        .execute(&base)
        .await
        .expect("cleanup principal");
    sqlx::query("DELETE FROM tenants WHERE id = $1::uuid")
        .bind(&tenant_id)
        .execute(&base)
        .await
        .expect("cleanup tenant");
}
