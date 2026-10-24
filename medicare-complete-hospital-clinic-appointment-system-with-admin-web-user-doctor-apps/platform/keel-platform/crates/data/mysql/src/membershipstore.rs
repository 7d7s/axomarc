//! Membership edge on MySQL 8 (MNC-001 / S-001): the `memberships` table.
//! MySQL port notes mirror principalstore.rs: `?` binds in appearance order,
//! ids are BINARY(16), timestamps are DATETIME(6), and duplicate keys are
//! detected with `is_unique_violation()` (MySQL's SQLSTATE 23000 is shared
//! with foreign-key violations, so a literal code check would misclassify).
//!
//! The lookups are explicitly (principal, tenant)-scoped because they run on
//! the pre-auth path, before a tenant context exists.

use kelson_foundation::{AppError, MembershipId, PrincipalId, TenantId};
use kelson_identity::{store::MembershipStore, Membership, MembershipStatus};
use sqlx::mysql::MySqlPool;

use crate::bridge::block_on;
use uuid::Uuid;

pub struct MySqlMembershipStore {
    pool: MySqlPool,
}

impl MySqlMembershipStore {
    pub fn new(pool: MySqlPool) -> Self {
        Self { pool }
    }
}

fn my(e: impl std::fmt::Display) -> AppError {
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

fn membership_from_row(row: sqlx::mysql::MySqlRow) -> Result<Membership, AppError> {
    use sqlx::Row;
    let id: Uuid = row.try_get("id").map_err(my)?;
    let principal_id: Uuid = row.try_get("principal_id").map_err(my)?;
    let tenant_id: Uuid = row.try_get("tenant_id").map_err(my)?;
    Ok(Membership {
        id: MembershipId::from_uuid(id),
        principal_id: PrincipalId::from_uuid(principal_id),
        tenant_id: TenantId::from_uuid(tenant_id),
        status: status_from(&row.try_get::<String, _>("status").map_err(my)?)?,
        joined_at: row.try_get("joined_at").map_err(my)?,
    })
}

impl MembershipStore for MySqlMembershipStore {
    fn grant(&self, membership: Membership) -> Result<Membership, AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            let result = sqlx::query(
                "INSERT INTO memberships (id, principal_id, tenant_id, status, joined_at)
                 VALUES (?, ?, ?, ?, ?)",
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
                Err(e) => Err(my(e)),
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
            let row: Option<sqlx::mysql::MySqlRow> =
                sqlx::query("SELECT * FROM memberships WHERE principal_id = ? AND tenant_id = ?")
                    .bind(*principal.as_uuid())
                    .bind(*tenant.as_uuid())
                    .fetch_optional(&pool)
                    .await
                    .map_err(my)?;
            row.map(membership_from_row).transpose()
        })
    }

    fn update_status(&self, id: MembershipId, status: MembershipStatus) -> Result<(), AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            let done = sqlx::query("UPDATE memberships SET status = ? WHERE id = ?")
                .bind(status_text(status))
                .bind(*id.as_uuid())
                .execute(&pool)
                .await
                .map_err(my)?;
            if done.rows_affected() == 0 {
                return Err(AppError::not_found(format!("membership {id}")));
            }
            Ok(())
        })
    }
}
