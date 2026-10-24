//! Membership edge on PostgreSQL (MNC-001 / S-001): the `memberships` table.
//! Mirrors principalstore.rs: sync trait over sqlx via block_on.
//!
//! The lookups are explicitly (principal, tenant)-scoped because they run on
//! the pre-auth path, before a tenant GUC can be trusted from the request.
//! Phase 2's RLS rewrite must keep this read path working through the
//! platform role (or set the claimed tenant inside the transaction).

use kelson_foundation::{AppError, MembershipId, PrincipalId, TenantId};
use kelson_identity::{store::MembershipStore, Membership, MembershipStatus};
use sqlx::postgres::PgPool;

use crate::bridge::block_on;
use uuid::Uuid;

pub struct PgMembershipStore {
    pool: PgPool,
}

impl PgMembershipStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn pg(e: impl std::fmt::Display) -> AppError {
    AppError::persistence(format!("membership: {e}"))
}

fn status_text(status: MembershipStatus) -> &'static str {
    match status {
        MembershipStatus::Invited => "invited",
        MembershipStatus::Active => "active",
        MembershipStatus::Suspended => "suspended",
        MembershipStatus::Revoked => "revoked",
    }
}

fn status_from(text: &str) -> Result<MembershipStatus, AppError> {
    Ok(match text {
        "invited" => MembershipStatus::Invited,
        "active" => MembershipStatus::Active,
        "suspended" => MembershipStatus::Suspended,
        "revoked" => MembershipStatus::Revoked,
        other => {
            return Err(AppError::internal(format!(
                "unknown membership status {other}"
            )))
        }
    })
}

fn membership_from_row(row: sqlx::postgres::PgRow) -> Result<Membership, AppError> {
    use sqlx::Row;
    let id: Uuid = row.try_get("id").map_err(pg)?;
    let principal_id: Uuid = row.try_get("principal_id").map_err(pg)?;
    let tenant_id: Uuid = row.try_get("tenant_id").map_err(pg)?;
    Ok(Membership {
        id: MembershipId::from_uuid(id),
        principal_id: PrincipalId::from_uuid(principal_id),
        tenant_id: TenantId::from_uuid(tenant_id),
        status: status_from(&row.try_get::<String, _>("status").map_err(pg)?)?,
        joined_at: row.try_get("joined_at").map_err(pg)?,
    })
}

impl MembershipStore for PgMembershipStore {
    fn grant(&self, membership: Membership) -> Result<Membership, AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            let result = sqlx::query(
                "INSERT INTO memberships (id, principal_id, tenant_id, status, joined_at)
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(*membership.id.as_uuid())
            .bind(*membership.principal_id.as_uuid())
            .bind(*membership.tenant_id.as_uuid())
            .bind(status_text(membership.status))
            .bind(membership.joined_at)
            .execute(&pool)
            .await;
            match result {
                Ok(_) => Ok(membership),
                Err(sqlx::Error::Database(db)) if db.is_unique_violation() => Err(
                    AppError::conflict("membership exists for this principal in this tenant"),
                ),
                Err(e) => Err(pg(e)),
            }
        })
    }

    fn find(
        &self,
        principal: PrincipalId,
        tenant: TenantId,
    ) -> Result<Option<Membership>, AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            let row: Option<sqlx::postgres::PgRow> =
                sqlx::query("SELECT * FROM memberships WHERE principal_id = $1 AND tenant_id = $2")
                    .bind(*principal.as_uuid())
                    .bind(*tenant.as_uuid())
                    .fetch_optional(&pool)
                    .await
                    .map_err(pg)?;
            row.map(membership_from_row).transpose()
        })
    }

    fn update_status(&self, id: MembershipId, status: MembershipStatus) -> Result<(), AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            let done = sqlx::query("UPDATE memberships SET status = $2 WHERE id = $1")
                .bind(*id.as_uuid())
                .bind(status_text(status))
                .execute(&pool)
                .await
                .map_err(pg)?;
            if done.rows_affected() == 0 {
                return Err(AppError::not_found(format!("membership {id}")));
            }
            Ok(())
        })
    }
}
