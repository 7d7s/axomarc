//! MySQL materialised read model store.
//!
//! This store implements the `ReadModelStore` trait using MySQL. It creates
//! materialised tables for each read model and refreshes them by running
//! the declared query, scoped by tenant.

use std::time::Instant;

use kelson_foundation::{AppError, TenantId};
use kelson_models::store::{FieldFilter, ListOpts, Record};
use kelson_query::Page;
use sqlx::{Column, MySql, Pool, Row};
use std::future::Future;
use std::pin::Pin;

use kelson_models::readmodel::{
    ReadModelDef, ReadModelRefreshResult, ReadModelStaleness, ReadModelState, ReadModelStore,
};

/// MySQL-backed materialised read model store.
#[derive(Clone, Debug)]
pub struct MySqlReadModelStore {
    pool: Pool<MySql>,
}

impl MySqlReadModelStore {
    /// Create a new MySQL read model store.
    pub fn new(pool: Pool<MySql>) -> Self {
        Self { pool }
    }
}

impl ReadModelStore for MySqlReadModelStore {
    fn refresh(
        &self,
        read_model: &ReadModelDef,
        tenant: Option<TenantId>,
    ) -> Pin<Box<dyn Future<Output = Result<ReadModelRefreshResult, AppError>> + Send + '_>> {
        let pool = self.pool.clone();
        let table = read_model.table.clone();
        let query = read_model.query.clone();
        let Some(tenant) = tenant else {
            return Box::pin(async {
                Err(AppError::validation(
                    "read model refresh requires a tenant scope (S-012: cross-tenant refresh is refused)",
                ))
            });
        };
        let tenant_where = format!("tenant_id = '{}'", tenant.as_uuid());

        Box::pin(async move {
            let start = Instant::now();

            // Delete existing rows for this tenant
            let delete_sql = format!("DELETE FROM `{}` WHERE {}", table, tenant_where);
            sqlx::query(&delete_sql)
                .execute(&pool)
                .await
                .map_err(|e| AppError::internal(format!("read model delete failed: {e}")))?;

            // Insert the refreshed data
            let insert_sql = format!("INSERT INTO `{}` {}", table, query);
            let result = sqlx::query(&insert_sql)
                .execute(&pool)
                .await
                .map_err(|e| AppError::internal(format!("read model refresh failed: {e}")))?;

            let duration_ms = start.elapsed().as_millis() as u64;
            Ok(ReadModelRefreshResult {
                rows_materialised: result.rows_affected(),
                tenants_refreshed: 1,
                duration_ms,
                success: true,
                error: None,
            })
        })
    }

    fn refresh_all_tenants(
        &self,
        read_model: &ReadModelDef,
    ) -> Pin<Box<dyn Future<Output = Result<ReadModelRefreshResult, AppError>> + Send + '_>> {
        let pool = self.pool.clone();
        let table = read_model.table.clone();
        let query = read_model.query.clone();

        Box::pin(async move {
            let start = Instant::now();

            // Delete all rows
            let delete_sql = format!("DELETE FROM `{}`", table);
            sqlx::query(&delete_sql)
                .execute(&pool)
                .await
                .map_err(|e| AppError::internal(format!("read model delete all failed: {e}")))?;

            // Insert refreshed data for all tenants
            let insert_sql = format!("INSERT INTO `{}` {}", table, query);
            let result = sqlx::query(&insert_sql)
                .execute(&pool)
                .await
                .map_err(|e| AppError::internal(format!("read model refresh all failed: {e}")))?;

            let duration_ms = start.elapsed().as_millis() as u64;
            // Count distinct tenants affected (approximation)
            let tenant_count: i64 = sqlx::query_scalar(&format!(
                "SELECT COUNT(DISTINCT tenant_id) FROM `{}`",
                table
            ))
            .fetch_one(&pool)
            .await
            .unwrap_or(0);

            Ok(ReadModelRefreshResult {
                rows_materialised: result.rows_affected(),
                tenants_refreshed: tenant_count as u64,
                duration_ms,
                success: true,
                error: None,
            })
        })
    }

    fn staleness(
        &self,
        read_model: &ReadModelDef,
        tenant: Option<TenantId>,
    ) -> Pin<Box<dyn Future<Output = Result<ReadModelStaleness, AppError>> + Send + '_>> {
        let pool = self.pool.clone();
        let table = read_model.table.clone();
        let name = read_model.name.clone();
        let Some(tenant) = tenant else {
            return Box::pin(async {
                Err(AppError::validation(
                    "read model staleness requires a tenant scope (S-012: unscoped reads are refused)",
                ))
            });
        };
        let tenant_where = format!("tenant_id = '{}'", tenant.as_uuid());

        Box::pin(async move {
            // Get the max updated_at or created_at from the materialised table
            let age_query = format!(
                "SELECT TIMESTAMPDIFF(SECOND, MAX(created_at), NOW()) as age, MAX(created_at) as last_refreshed FROM `{}` WHERE {}",
                table, tenant_where
            );

            let row = sqlx::query(&age_query)
                .fetch_optional(&pool)
                .await
                .map_err(|e| AppError::internal(format!("staleness query failed: {e}")))?;

            let (age_seconds, last_refreshed_at) = match row {
                Some(r) => {
                    let age: Option<i64> = r.try_get("age").ok();
                    let last: Option<String> = r.try_get("last_refreshed").ok();
                    (age.map(|a| a as u64), last)
                }
                None => (None, None),
            };

            let state = match age_seconds {
                Some(age) if age > 3600 => ReadModelState::Stale, // 1 hour default threshold
                Some(_) => ReadModelState::Fresh,
                None => ReadModelState::Failed,
            };

            Ok(ReadModelStaleness {
                name,
                state,
                age_seconds,
                last_refreshed_at,
                last_error: None,
            })
        })
    }

    fn query(
        &self,
        read_model: &ReadModelDef,
        tenant: Option<TenantId>,
        filters: &[FieldFilter],
        opts: &ListOpts,
    ) -> Pin<Box<dyn Future<Output = Result<Page<Record>, AppError>> + Send + '_>> {
        let pool = self.pool.clone();
        let table = read_model.table.clone();
        let Some(tenant) = tenant else {
            return Box::pin(async {
                Err(AppError::validation(
                    "read model query requires a tenant scope (S-012: unscoped reads are refused)",
                ))
            });
        };
        let tenant_where = format!("tenant_id = '{}'", tenant.as_uuid());
        let filters = filters.to_vec();
        let opts = opts.clone();

        Box::pin(async move {
            // Build the WHERE clause from filters
            let mut where_parts = vec![tenant_where];

            for filter in filters {
                let (field, op_str, value) = match filter {
                    kelson_models::store::FieldFilter::Cmp { field, op, value } => {
                        let op_str = match op {
                            kelson_models::store::FilterOp::Eq => "=",
                            kelson_models::store::FilterOp::Ne => "!=",
                            kelson_models::store::FilterOp::Lt => "<",
                            kelson_models::store::FilterOp::LtEq => "<=",
                            kelson_models::store::FilterOp::Gt => ">",
                            kelson_models::store::FilterOp::GtEq => ">=",
                            kelson_models::store::FilterOp::In => "IN",
                            kelson_models::store::FilterOp::IsNull => "IS NULL",
                            kelson_models::store::FilterOp::NotNull => "IS NOT NULL",
                        };
                        (field, op_str, value)
                    }
                    kelson_models::store::FieldFilter::Exists(_) => continue, // Skip EXISTS for now
                };
                let value_str = serde_json::to_string(&value).unwrap_or_default();
                where_parts.push(format!(
                    "`{}` {} '{}'",
                    field,
                    op_str,
                    value_str.replace("'", "''")
                ));
            }

            let where_clause = where_parts.join(" AND ");
            let order_by = opts
                .sort
                .as_ref()
                .map(|(col, desc)| {
                    format!("ORDER BY `{}` {}", col, if *desc { "DESC" } else { "ASC" })
                })
                .unwrap_or_default();
            let limit = opts
                .limit
                .map(|l| format!("LIMIT {}", l))
                .unwrap_or_default();

            let sql = format!(
                "SELECT * FROM `{}` WHERE {} {} {}",
                table, where_clause, order_by, limit
            );

            let rows = sqlx::query(&sql)
                .fetch_all(&pool)
                .await
                .map_err(|e| AppError::internal(format!("read model query failed: {e}")))?;

            let records: Vec<Record> = rows
                .into_iter()
                .map(|row| {
                    let mut record = Record::new();
                    for column in row.columns() {
                        let name = column.name();
                        let value = row
                            .try_get::<serde_json::Value, _>(name)
                            .unwrap_or(serde_json::Value::Null);
                        record.insert(name.to_string(), value);
                    }
                    record
                })
                .collect();

            let total = records.len() as u64;
            Ok(Page {
                items: records,
                page: 1,
                per_page: 100,
                total: Some(total),
            })
        })
    }

    fn find(
        &self,
        read_model: &ReadModelDef,
        id: uuid::Uuid,
        tenant: Option<TenantId>,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Record>, AppError>> + Send + '_>> {
        let pool = self.pool.clone();
        let table = read_model.table.clone();
        let Some(tenant) = tenant else {
            return Box::pin(async {
                Err(AppError::validation(
                    "read model find requires a tenant scope (S-012: unscoped reads are refused)",
                ))
            });
        };
        let tenant_where = format!("tenant_id = '{}'", tenant.as_uuid());

        Box::pin(async move {
            let sql = format!(
                "SELECT * FROM `{}` WHERE id = ? AND {} LIMIT 1",
                table, tenant_where
            );

            let row = sqlx::query(&sql)
                .bind(id.to_string())
                .fetch_optional(&pool)
                .await
                .map_err(|e| AppError::internal(format!("read model find failed: {e}")))?;

            match row {
                Some(r) => {
                    let mut record = Record::new();
                    for column in r.columns() {
                        let name = column.name();
                        let value = r
                            .try_get::<serde_json::Value, _>(name)
                            .unwrap_or(serde_json::Value::Null);
                        record.insert(name.to_string(), value);
                    }
                    Ok(Some(record))
                }
                None => Ok(None),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_store_is_debug() {
        // This just ensures the Debug bound is satisfied
        fn assert_debug<T: std::fmt::Debug>() {}
        assert_debug::<MySqlReadModelStore>();
    }
}
