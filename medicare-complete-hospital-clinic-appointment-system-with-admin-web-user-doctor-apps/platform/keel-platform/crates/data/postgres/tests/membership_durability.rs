//! MNC-001 / P1.1 acceptance: membership edges are DURABLE, not per-process.
//!
//! The auth gates read `(principal, tenant)` membership through
//! `MembershipStore`. This target proves the durable stores (Pg and MySQL)
//! honor the port at the database:
//! - an edge granted through one store instance is visible to a SECOND
//!   instance over the same pool (the "restart" - no in-process cache);
//! - a second grant for the same pair is a conflict (the pair-unique index),
//!   not a duplicate row;
//! - a REVOKED edge keeps its row (`find` answers) but never satisfies
//!   `active_for`;
//! - lookups are pair-scoped: some other tenant has no edge for the same
//!   principal.
//!
//! Activated with `--features pg-tests`; each half skips gracefully unless
//! DATABASE_URL names its scheme, so local runs without a database stay
//! green and CI (service container per scheme) enforces them.

#![cfg(feature = "pg-tests")]

use kelson_foundation::{MembershipId, PrincipalId, TenantId};
use kelson_identity::store::{MembershipStore, PrincipalStore};
use kelson_identity::{Membership, MembershipStatus, Principal, PrincipalStatus};
use time::OffsetDateTime;

fn principal(email: &str) -> Principal {
    Principal {
        id: PrincipalId::new(),
        status: PrincipalStatus::Active,
        email: Some(email.into()),
        phone: None,
        email_verified_at: None,
        created_at: OffsetDateTime::now_utc(),
    }
}

async fn pg_pool() -> Option<kelson_postgres::PgPool> {
    let url = match std::env::var("DATABASE_URL") {
        Ok(url) if !url.is_empty() => url,
        _ => {
            eprintln!("membership durability: DATABASE_URL not set; skipping");
            return None;
        }
    };
    if !url.starts_with("postgres://") && !url.starts_with("postgresql://") {
        eprintln!("membership durability: DATABASE_URL is not postgres://; skipping");
        return None;
    }
    match kelson_postgres::create_pool(&url, 4).await {
        Ok(pool) => {
            kelson_postgres::run_migrations(&pool, std::path::Path::new("."))
                .await
                .expect("migrations apply");
            Some(pool)
        }
        Err(e) => {
            eprintln!("membership durability: cannot connect ({e}); skipping");
            None
        }
    }
}

async fn mysql_pool() -> Option<kelson_mysql::MySqlPool> {
    let url = match std::env::var("DATABASE_URL") {
        Ok(url) if !url.is_empty() => url,
        _ => {
            eprintln!("membership durability: DATABASE_URL not set; skipping");
            return None;
        }
    };
    if !url.starts_with("mysql://") {
        eprintln!("membership durability: DATABASE_URL is not mysql://; skipping");
        return None;
    }
    match kelson_mysql::create_pool(&url, 4).await {
        Ok(pool) => {
            kelson_mysql::run_migrations(&pool, std::path::Path::new("."))
                .await
                .expect("migrations apply");
            Some(pool)
        }
        Err(e) => {
            eprintln!("membership durability: cannot connect ({e}); skipping");
            None
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pg_membership_edges_survive_a_restart() {
    let Some(pool) = pg_pool().await else { return };

    let created = kelson_postgres::PgPrincipalStore::new(pool.clone())
        .create(principal(&format!(
            "pg-membership-{}@it.test",
            uuid::Uuid::now_v7()
        )))
        .expect("principal inserts");
    let tenant = TenantId::new();
    sqlx::query("INSERT INTO tenants (id, slug, name) VALUES ($1, $2, $3)")
        .bind(tenant.as_uuid())
        .bind(format!("pg-membership-{}", uuid::Uuid::now_v7()))
        .bind("Pg Membership Tenant")
        .execute(&pool)
        .await
        .expect("tenant seeds");

    let edge = Membership {
        id: MembershipId::new(),
        principal_id: created.id,
        tenant_id: tenant,
        status: MembershipStatus::Active,
        joined_at: OffsetDateTime::now_utc(),
    };
    kelson_postgres::PgMembershipStore::new(pool.clone())
        .grant(edge.clone())
        .expect("the edge grants");

    // The "restart": a fresh store holding only the pool re-reads the row.
    let after_restart = kelson_postgres::PgMembershipStore::new(pool.clone());
    let loaded = after_restart
        .find(created.id, tenant)
        .expect("find answers")
        .expect("the edge survived the restart");
    assert_eq!(loaded.id, edge.id);
    assert_eq!(loaded.status, MembershipStatus::Active);
    assert!(
        after_restart
            .active_for(created.id, tenant)
            .expect("answers")
            .is_some(),
        "an active edge is entry for the pair"
    );

    assert!(
        after_restart.grant(edge.clone()).is_err(),
        "a second grant for the same pair is a conflict, not a second edge"
    );
    assert!(
        after_restart
            .find(created.id, TenantId::new())
            .expect("answers")
            .is_none(),
        "the lookup is pair-scoped: another tenant has no edge"
    );

    after_restart
        .update_status(edge.id, MembershipStatus::Revoked)
        .expect("revoke");
    assert!(
        after_restart
            .find(created.id, tenant)
            .expect("answers")
            .is_some(),
        "a revoked edge keeps its row"
    );
    assert!(
        after_restart
            .active_for(created.id, tenant)
            .expect("answers")
            .is_none(),
        "a revoked edge never satisfies the gate"
    );

    assert!(
        after_restart
            .update_status(MembershipId::new(), MembershipStatus::Active)
            .is_err(),
        "an unknown edge fails closed"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_membership_edges_survive_a_restart() {
    let Some(pool) = mysql_pool().await else {
        return;
    };

    let created = kelson_mysql::MySqlPrincipalStore::new(pool.clone())
        .create(principal(&format!(
            "mysql-membership-{}@it.test",
            uuid::Uuid::now_v7()
        )))
        .expect("principal inserts");
    let tenant = TenantId::new();
    sqlx::query("INSERT INTO tenants (id, slug, name) VALUES (?, ?, ?)")
        .bind(tenant.as_uuid())
        .bind(format!("mysql-membership-{}", uuid::Uuid::now_v7()))
        .bind("MySQL Membership Tenant")
        .execute(&pool)
        .await
        .expect("tenant seeds");

    let edge = Membership {
        id: MembershipId::new(),
        principal_id: created.id,
        tenant_id: tenant,
        status: MembershipStatus::Active,
        joined_at: OffsetDateTime::now_utc(),
    };
    kelson_mysql::MySqlMembershipStore::new(pool.clone())
        .grant(edge.clone())
        .expect("the edge grants");

    // The "restart": a fresh store holding only the pool re-reads the row.
    let after_restart = kelson_mysql::MySqlMembershipStore::new(pool.clone());
    let loaded = after_restart
        .find(created.id, tenant)
        .expect("find answers")
        .expect("the edge survived the restart");
    assert_eq!(loaded.id, edge.id);
    assert_eq!(loaded.status, MembershipStatus::Active);
    assert!(
        after_restart
            .active_for(created.id, tenant)
            .expect("answers")
            .is_some(),
        "an active edge is entry for the pair"
    );

    assert!(
        after_restart.grant(edge.clone()).is_err(),
        "a second grant for the same pair is a conflict, not a second edge"
    );
    assert!(
        after_restart
            .find(created.id, TenantId::new())
            .expect("answers")
            .is_none(),
        "the lookup is pair-scoped: another tenant has no edge"
    );

    after_restart
        .update_status(edge.id, MembershipStatus::Revoked)
        .expect("revoke");
    assert!(
        after_restart
            .find(created.id, tenant)
            .expect("answers")
            .is_some(),
        "a revoked edge keeps its row"
    );
    assert!(
        after_restart
            .active_for(created.id, tenant)
            .expect("answers")
            .is_none(),
        "a revoked edge never satisfies the gate"
    );

    assert!(
        after_restart
            .update_status(MembershipId::new(), MembershipStatus::Active)
            .is_err(),
        "an unknown edge fails closed"
    );
}
