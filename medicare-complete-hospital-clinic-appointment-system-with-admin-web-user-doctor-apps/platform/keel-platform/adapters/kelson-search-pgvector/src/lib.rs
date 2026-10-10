//! # kelson-search-pgvector
//! PostgreSQL + pgvector adapter for the frozen `SearchEngine` contract
//! (kelson-search). Zero new dependencies: embeddings travel in pgvector text
//! form (`[0.1,0.2]`) with a `::vector` cast, so plain sqlx is sufficient.
//!
//! One table per index: `kelson_search_<name>` with
//! `id TEXT PK, tenant_id UUID, body TEXT, fields JSONB, embedding vector(N)`.
//! All reads and writes are tenant-scoped. Tables created with `dims: None`
//! omit the embedding column and serve keyword-only search (no extension
//! required); vector/hybrid then fail fast with validation errors.
//!
//! Hybrid parity note: the in-memory reference blends over the full match
//! set; this adapter pulls a capped candidate window (top 500 by
//! max(keyword, similarity)) and blends in Rust. Scores are therefore
//! engine-explicit, as the contract allows.

pub mod bridge;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use kelson_foundation::{AppError, TenantId};
use kelson_search::{
    Facet, FacetValue, Hit, HybridQuery, Index, SearchDocument, SearchEngine, SearchQuery,
    SearchResult, VectorQuery,
};
use sqlx::{PgPool, Row};

/// Hard cap on the hybrid candidate window (see module docs).
pub const HYBRID_WINDOW: i64 = 500;
/// Facet buckets are capped for predictable payloads.
pub const MAX_FACET_VALUES: i64 = 25;

/// PostgreSQL-backed search engine. Cheap to clone (pool handle).
#[derive(Clone)]
pub struct PgVectorSearchEngine {
    pool: PgPool,
    /// index table name -> has embedding column (recorded by create_table;
    /// unknown tables are assumed vector-capable until proven otherwise).
    known: Arc<Mutex<HashMap<String, bool>>>,
}

impl PgVectorSearchEngine {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            known: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Sanitized table name for an index; rejects anything outside
    /// `[a-z][a-z0-9_]{0,39}` after lowercasing.
    pub fn table_name(index: &Index) -> Result<String, AppError> {
        let raw = index.0.to_ascii_lowercase();
        let valid = !raw.is_empty()
            && raw.len() <= 40
            && raw.chars().next().map(|c| c.is_ascii_lowercase()).unwrap_or(false)
            && raw
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
        if !valid {
            return Err(AppError::validation(format!(
                "invalid search index name: {}",
                index.0
            )));
        }
        Ok(format!("kelson_search_{raw}"))
    }

    /// Pure DDL helper (migration-friendly, unit-testable). With `dims`,
    /// includes the extension statement and a vector column; without, the
    /// table is keyword-only.
    pub fn create_table_sql(index: &Index, dims: Option<usize>) -> Result<String, AppError> {
        let table = Self::table_name(index)?;
        let mut sql = String::new();
        if let Some(d) = dims {
            if d == 0 {
                return Err(AppError::validation("vector dims must be > 0"));
            }
            sql.push_str("CREATE EXTENSION IF NOT EXISTS vector;\n");
            sql.push_str(&format!(
                "CREATE TABLE IF NOT EXISTS {table} (\
                 id TEXT PRIMARY KEY, tenant_id UUID NOT NULL, \
                 body TEXT NOT NULL DEFAULT '', fields JSONB NOT NULL DEFAULT '{{}}', \
                 embedding vector({d}));\n"
            ));
        } else {
            sql.push_str(&format!(
                "CREATE TABLE IF NOT EXISTS {table} (\
                 id TEXT PRIMARY KEY, tenant_id UUID NOT NULL, \
                 body TEXT NOT NULL DEFAULT '', fields JSONB NOT NULL DEFAULT '{{}}');\n"
            ));
        }
        sql.push_str(&format!(
            "CREATE INDEX IF NOT EXISTS {table}_tenant_idx ON {table} (tenant_id);\n"
        ));
        Ok(sql)
    }

    /// Execute the DDL for an index table and remember its shape.
    pub async fn create_table(
        &self,
        index: &Index,
        dims: Option<usize>,
    ) -> Result<(), AppError> {
        let sql = Self::create_table_sql(index, dims)?;
        for stmt in sql.split(';') {
            let stmt = stmt.trim();
            if stmt.is_empty() {
                continue;
            }
            sqlx::query(stmt)
                .execute(&self.pool)
                .await
                .map_err(|e| AppError::persistence(format!("search ddl: {e}")))?;
        }
        self.known
            .lock()
            .expect("known-tables poisoned")
            .insert(Self::table_name(index)?, dims.is_some());
        Ok(())
    }

    /// Drop an index table entirely.
    pub async fn drop_table(&self, index: &Index) -> Result<(), AppError> {
        let table = Self::table_name(index)?;
        sqlx::query(&format!("DROP TABLE IF EXISTS {table}"))
            .execute(&self.pool)
            .await
            .map_err(|e| AppError::persistence(format!("search drop: {e}")))?;
        self.known.lock().expect("known-tables poisoned").remove(&table);
        Ok(())
    }

    fn has_vectors(&self, table: &str) -> bool {
        self.known
            .lock()
            .expect("known-tables poisoned")
            .get(table)
            .copied()
            .unwrap_or(true)
    }

    fn require_vectors(&self, index: &Index, table: &str) -> Result<(), AppError> {
        if self.has_vectors(table) {
            Ok(())
        } else {
            Err(AppError::validation(format!(
                "index {} was created without an embedding column",
                index.0
            )))
        }
    }
}

// ----------------------------------------------------------- vector codec

/// pgvector text form: `[0.1,0.2,...]`.
pub fn encode_vector(v: &[f32]) -> String {
    let body: Vec<String> = v.iter().map(|f| format!("{f}")).collect();
    format!("[{}]", body.join(","))
}

/// Parse pgvector text form back to floats (tolerant of whitespace).
pub fn decode_vector(raw: &str) -> Option<Vec<f32>> {
    let trimmed = raw.trim().trim_start_matches('[').trim_end_matches(']');
    if trimmed.trim().is_empty() {
        return Some(Vec::new());
    }
    trimmed
        .split(',')
        .map(|part| part.trim().parse::<f32>().ok())
        .collect()
}

/// Validate a JSONB filter/facet key as a safe identifier fragment.
fn valid_field_key(key: &str) -> bool {
    let mut chars = key.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

// ------------------------------------------------------------ async bridge

// The SearchEngine port is sync and sqlx is async; calls bridge through
// bridge.rs (block_in_place inside multi-threaded runtimes, a persistent
// bridge runtime otherwise). Futures must stay owned ('static), so every
// call clones the pool handle first.

// ------------------------------------------------------------ engine port

impl SearchEngine for PgVectorSearchEngine {
    fn index(&self, index: &Index, doc: SearchDocument) -> Result<(), AppError> {
        let table = Self::table_name(index)?;
        let fields = serde_json::to_value(&doc.fields)
            .map_err(|e| AppError::validation(format!("fields serialize: {e}")))?;
        let with_vectors = self.has_vectors(&table);
        let embedding: Option<String> = if doc.vector.is_empty() {
            None
        } else {
            Some(encode_vector(&doc.vector))
        };
        let pool = self.pool.clone();
        crate::bridge::block_on(async move {
            if with_vectors {
                sqlx::query(&format!(
                    "INSERT INTO {table} (id, tenant_id, body, fields, embedding)
                     VALUES ($1,$2,$3,$4,$5::vector)
                     ON CONFLICT (id) DO UPDATE SET tenant_id = EXCLUDED.tenant_id,
                       body = EXCLUDED.body, fields = EXCLUDED.fields,
                       embedding = EXCLUDED.embedding"
                ))
                .bind(&doc.id)
                .bind(*doc.tenant.as_uuid())
                .bind(&doc.text)
                .bind(fields)
                .bind(embedding)
                .execute(&pool)
                .await
            } else {
                sqlx::query(&format!(
                    "INSERT INTO {table} (id, tenant_id, body, fields)
                     VALUES ($1,$2,$3,$4)
                     ON CONFLICT (id) DO UPDATE SET tenant_id = EXCLUDED.tenant_id,
                       body = EXCLUDED.body, fields = EXCLUDED.fields"
                ))
                .bind(&doc.id)
                .bind(*doc.tenant.as_uuid())
                .bind(&doc.text)
                .bind(fields)
                .execute(&pool)
                .await
            }
        })
        .map_err(|e| AppError::persistence(format!("search index: {e}")))?;
        Ok(())
    }

    fn delete(&self, index: &Index, id: &str) -> Result<(), AppError> {
        let table = Self::table_name(index)?;
        let id = id.to_string();
        let pool = self.pool.clone();
        crate::bridge::block_on(async move {
            sqlx::query(&format!("DELETE FROM {table} WHERE id = $1"))
                .bind(&id)
                .execute(&pool)
                .await
        })
        .map_err(|e| AppError::persistence(format!("search delete: {e}")))?;
        Ok(())
    }

    fn search(&self, index: &Index, query: &SearchQuery) -> Result<SearchResult, AppError> {
        let table = Self::table_name(index)?;
        // Bind layout: $1 tenant, $2 text, $3.. filters.
        let where_core =
            "($2 = '' OR to_tsvector('simple', body) @@ plainto_tsquery('simple', $2))";
        let mut filter_sql = Vec::new();
        let mut filter_values: Vec<String> = Vec::new();
        let mut next = 3usize;
        for (key, value) in &query.filters {
            if !valid_field_key(key) {
                return Err(AppError::validation(format!("invalid filter key: {key}")));
            }
            filter_sql.push(format!("fields->>'{key}' = ${next}"));
            filter_values.push(value.clone());
            next += 1;
        }
        let mut facet_keys = Vec::new();
        for key in &query.facets {
            if !valid_field_key(key) {
                return Err(AppError::validation(format!("invalid facet key: {key}")));
            }
            facet_keys.push(key.clone());
        }
        let where_sql = if filter_sql.is_empty() {
            format!("tenant_id = $1 AND {where_core}")
        } else {
            format!("tenant_id = $1 AND {where_core} AND {}", filter_sql.join(" AND "))
        };
        // Precompute every SQL string so the async closures only own data.
        let count_sql = format!("SELECT COUNT(*) AS n FROM {table} WHERE {where_sql}");
        let limit = query.limit.max(1) as i64;
        let offset = query.offset as i64;
        let hits_sql = format!(
            "SELECT id, fields,
                    (CASE WHEN $2 = '' THEN 0.0
                          ELSE ts_rank(to_tsvector('simple', body), plainto_tsquery('simple', $2))
                     END)::float8 AS score
             FROM {table} WHERE {where_sql}
             ORDER BY score DESC, id LIMIT {limit} OFFSET {offset}"
        );
        let facet_sqls: Vec<(String, String)> = facet_keys
            .iter()
            .map(|key| {
                (
                    key.clone(),
                    format!(
                        "SELECT fields->>'{key}' AS v, COUNT(*) AS n
                         FROM {table}
                         WHERE {where_sql} AND fields->>'{key}' IS NOT NULL
                         GROUP BY 1 ORDER BY n DESC, v LIMIT {MAX_FACET_VALUES}"
                    ),
                )
            })
            .collect();
        let tenant = *query.tenant.as_uuid();
        let text = query.text.clone();
        // total over the full match set
        let pool = self.pool.clone();
        let filters = filter_values.clone();
        let count_sql = count_sql.clone();
        let total: i64 = crate::bridge::block_on(async move {
            let mut q = sqlx::query(&count_sql).bind(tenant).bind(&text);
            for v in &filters {
                q = q.bind(v);
            }
            let row = q.fetch_one(&pool).await?;
            row.try_get("n")
        })
        .map_err(|e| AppError::persistence(format!("search count: {e}")))?;
        // page of hits
        let pool = self.pool.clone();
        let filters = filter_values.clone();
        let text = query.text.clone();
        let rows = crate::bridge::block_on(async move {
            let mut q = sqlx::query(&hits_sql).bind(tenant).bind(&text);
            for v in &filters {
                q = q.bind(v);
            }
            q.fetch_all(&pool).await
        })
        .map_err(|e| AppError::persistence(format!("search hits: {e}")))?;
        // facets over the same match set
        let mut facets = Vec::new();
        for (key, facet_sql) in facet_sqls {
            let pool = self.pool.clone();
            let filters = filter_values.clone();
            let text = query.text.clone();
            let rows = crate::bridge::block_on(async move {
                let mut q = sqlx::query(&facet_sql).bind(tenant).bind(&text);
                for v in &filters {
                    q = q.bind(v);
                }
                q.fetch_all(&pool).await
            })
            .map_err(|e| AppError::persistence(format!("facet {key}: {e}")))?;
            facets.push(Facet {
                field: key,
                values: rows
                    .iter()
                    .filter_map(|r| {
                        let v: Option<String> = r.try_get("v").ok()?;
                        let n: i64 = r.try_get("n").ok()?;
                        v.map(|value| FacetValue {
                            value,
                            count: n.max(0) as usize,
                        })
                    })
                    .collect(),
            });
        }
        let hits = rows
            .iter()
            .map(|r| {
                let id: String = r.try_get("id").unwrap_or_default();
                let score: f64 = r.try_get("score").unwrap_or(0.0);
                let fields_value: serde_json::Value =
                    r.try_get("fields").unwrap_or(serde_json::Value::Null);
                Hit {
                    id,
                    score: score as f32,
                    fields: serde_json::from_value(fields_value).unwrap_or_default(),
                }
            })
            .collect();
        Ok(SearchResult {
            total: total.max(0) as usize,
            hits,
            facets,
        })
    }
    fn vector_search(&self, index: &Index, query: &VectorQuery) -> Result<SearchResult, AppError> {
        let table = Self::table_name(index)?;
        self.require_vectors(index, &table)?;
        if query.vector.is_empty() {
            return Err(AppError::validation("vector query requires a probe"));
        }
        let probe = encode_vector(&query.vector);
        let limit = query.limit.max(1) as i64;
        let tenant = *query.tenant.as_uuid();
        let pool = self.pool.clone();
        let rows = crate::bridge::block_on(async move {
            sqlx::query(&format!(
                "SELECT id, fields, 1 - (embedding <=> $2::vector) AS sim
                 FROM {table}
                 WHERE tenant_id = $1 AND embedding IS NOT NULL
                 ORDER BY embedding <=> $2::vector
                 LIMIT {limit}"
            ))
            .bind(tenant)
            .bind(&probe)
            .fetch_all(&pool)
            .await
        })
        .map_err(|e| AppError::persistence(format!("vector search: {e}")))?;
        let threshold = query.min_score;
        let hits: Vec<Hit> = rows
            .iter()
            .filter_map(|r| {
                let sim: f64 = r.try_get("sim").ok()?;
                if (sim as f32) < threshold {
                    return None;
                }
                let id: String = r.try_get("id").ok()?;
                let fields_value: serde_json::Value =
                    r.try_get("fields").unwrap_or(serde_json::Value::Null);
                Some(Hit {
                    id,
                    score: sim as f32,
                    fields: serde_json::from_value(fields_value).unwrap_or_default(),
                })
            })
            .collect();
        Ok(SearchResult {
            total: hits.len(),
            hits,
            facets: Vec::new(),
        })
    }

    fn hybrid_search(&self, index: &Index, query: &HybridQuery) -> Result<SearchResult, AppError> {
        let table = Self::table_name(index)?;
        let alpha = query.alpha.clamp(0.0, 1.0);
        let has_text = !query.query.text.is_empty();
        let has_probe = !query.vector.is_empty();
        if has_probe {
            self.require_vectors(index, &table)?;
        }
        // Bind layout: $1 tenant, [$2 text], filters..., [probe last].
        let text_pos = if has_text { Some(2usize) } else { None };
        let mut next = text_pos.map(|p| p + 1).unwrap_or(2);
        let mut filter_sql = Vec::new();
        let mut filter_values: Vec<String> = Vec::new();
        for (key, value) in &query.query.filters {
            if !valid_field_key(key) {
                return Err(AppError::validation(format!("invalid filter key: {key}")));
            }
            filter_sql.push(format!("fields->>'{key}' = ${next}"));
            filter_values.push(value.clone());
            next += 1;
        }
        let probe_pos = if has_probe { Some(next) } else { None };
        let kw_expr = match text_pos {
            Some(p) => format!(
                "ts_rank(to_tsvector('simple', body), plainto_tsquery('simple', ${p}))::float8"
            ),
            None => "0.0::float8".to_string(),
        };
        let sim_expr = match probe_pos {
            Some(p) => format!("(1 - (embedding <=> ${p}::vector))::float8"),
            None => "0.0::float8".to_string(),
        };
        let mut clauses = vec!["tenant_id = $1".to_string()];
        clauses.extend(filter_sql);
        match (has_text, has_probe) {
            (true, true) => clauses.push(format!(
                "(to_tsvector('simple', body) @@ plainto_tsquery('simple', ${}) OR embedding IS NOT NULL)",
                text_pos.unwrap()
            )),
            (true, false) => clauses.push(format!(
                "to_tsvector('simple', body) @@ plainto_tsquery('simple', ${})",
                text_pos.unwrap()
            )),
            (false, true) => clauses.push("embedding IS NOT NULL".into()),
            (false, false) => {}
        }
        let where_sql = clauses.join(" AND ");
        let sql = format!(
            "SELECT id, fields, {kw_expr} AS kw, {sim_expr} AS sim
             FROM {table} WHERE {where_sql}
             ORDER BY GREATEST({kw_expr}, {sim_expr}) DESC, id LIMIT {HYBRID_WINDOW}"
        );
        let tenant = *query.query.tenant.as_uuid();
        let text = query.query.text.clone();
        let probe = has_probe.then(|| encode_vector(&query.vector));
        let pool = self.pool.clone();
        let rows = crate::bridge::block_on(async move {
            let mut q = sqlx::query(&sql).bind(tenant);
            if has_text {
                q = q.bind(&text);
            }
            for v in &filter_values {
                q = q.bind(v);
            }
            if let Some(p) = &probe {
                q = q.bind(p);
            }
            q.fetch_all(&pool).await
        })
        .map_err(|e| AppError::persistence(format!("hybrid search: {e}")))?;
        // Blend with keyword normalization (reference-engine parity).
        let candidates: Vec<(f32, f32, String, serde_json::Value)> = rows
            .iter()
            .map(|r| {
                (
                    r.try_get::<f64, _>("kw").unwrap_or(0.0) as f32,
                    r.try_get::<f64, _>("sim").unwrap_or(0.0) as f32,
                    r.try_get("id").unwrap_or_default(),
                    r.try_get("fields").unwrap_or(serde_json::Value::Null),
                )
            })
            .collect();
        let max_kw = candidates
            .iter()
            .map(|(kw, _, _, _)| *kw)
            .fold(0.0f32, f32::max);
        let mut blended: Vec<(f32, String, serde_json::Value)> = candidates
            .into_iter()
            .map(|(kw, sim, id, fields)| {
                let kw_norm = if max_kw > 0.0 { kw / max_kw } else { 0.0 };
                (alpha * kw_norm + (1.0 - alpha) * sim, id, fields)
            })
            .collect();
        blended.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.1.cmp(&b.1))
        });
        let total = blended.len();
        let hits: Vec<Hit> = blended
            .into_iter()
            .skip(query.query.offset)
            .take(query.query.limit.max(1))
            .map(|(score, id, fields)| Hit {
                id,
                score,
                fields: serde_json::from_value(fields).unwrap_or_default(),
            })
            .collect();
        Ok(SearchResult {
            total,
            hits,
            facets: Vec::new(),
        })
    }

    fn count(&self, index: &Index, tenant: TenantId) -> Result<usize, AppError> {
        let table = Self::table_name(index)?;
        let tenant_uuid = *tenant.as_uuid();
        let pool = self.pool.clone();
        let n: i64 = crate::bridge::block_on(async move {
            let row = sqlx::query(&format!(
                "SELECT COUNT(*) AS n FROM {table} WHERE tenant_id = $1"
            ))
            .bind(tenant_uuid)
            .fetch_one(&pool)
            .await?;
            row.try_get("n")
        })
        .map_err(|e| AppError::persistence(format!("search count: {e}")))?;
        Ok(n.max(0) as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_name_sanitizes_and_rejects() {
        assert_eq!(
            PgVectorSearchEngine::table_name(&Index("Invoices".into())).unwrap(),
            "kelson_search_invoices"
        );
        assert!(PgVectorSearchEngine::table_name(&Index("ok_42".into())).is_ok());
        assert!(PgVectorSearchEngine::table_name(&Index("9bad".into())).is_err());
        assert!(PgVectorSearchEngine::table_name(&Index("has space".into())).is_err());
        assert!(PgVectorSearchEngine::table_name(&Index("drop;--".into())).is_err());
        assert!(
            PgVectorSearchEngine::table_name(&Index("x".repeat(41))).is_err(),
            "oversized names rejected"
        );
    }

    #[test]
    fn create_table_sql_shapes() {
        let with_vec =
            PgVectorSearchEngine::create_table_sql(&Index("notes".into()), Some(3)).unwrap();
        assert!(with_vec.contains("CREATE EXTENSION IF NOT EXISTS vector;"));
        assert!(with_vec.contains("embedding vector(3)"));
        assert!(with_vec.contains("kelson_search_notes_tenant_idx"));
        let kw_only =
            PgVectorSearchEngine::create_table_sql(&Index("notes".into()), None).unwrap();
        assert!(!kw_only.contains("vector"));
        assert!(kw_only.contains("fields JSONB"));
        assert!(PgVectorSearchEngine::create_table_sql(&Index("notes".into()), Some(0)).is_err());
        assert!(PgVectorSearchEngine::create_table_sql(&Index("Bad Name".into()), Some(3)).is_err());
    }

    #[test]
    fn vector_codec_round_trip() {
        let v = vec![0.1f32, -2.5, 3.0];
        let raw = encode_vector(&v);
        assert!(raw.starts_with('[') && raw.ends_with(']'));
        let back = decode_vector(&raw).unwrap();
        assert_eq!(back.len(), 3);
        assert!((back[0] - 0.1).abs() < 1e-6);
        assert_eq!(decode_vector("[]"), Some(vec![]));
        assert_eq!(decode_vector("[1,x]"), None);
    }

    #[test]
    fn field_keys_are_validated() {
        assert!(valid_field_key("status"));
        assert!(valid_field_key("_x1"));
        assert!(!valid_field_key("1x"));
        assert!(!valid_field_key("a-b"));
        assert!(!valid_field_key("a';--"));
        assert!(!valid_field_key(""));
    }
}
