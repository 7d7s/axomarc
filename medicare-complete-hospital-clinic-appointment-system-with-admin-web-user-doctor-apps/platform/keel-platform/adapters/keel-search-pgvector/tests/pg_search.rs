//! Live PostgreSQL integration tests for the pgvector search adapter.
//! Run with `--features pg-tests` and `DATABASE_URL` set; without either the
//! suite skips. Vector tests additionally skip when the pgvector extension
//! cannot be created (no superuser / extension not installed).

#![cfg(feature = "pg-tests")]

use keel_foundation::TenantId;
use keel_search::{HybridQuery, Index, SearchDocument, SearchEngine, SearchQuery, VectorQuery};
use keel_search_pgvector::PgVectorSearchEngine;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use std::collections::BTreeMap;

async fn pool() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(&url)
        .await
        .ok()
}

async fn vector_available(pool: &PgPool) -> bool {
    sqlx::query("CREATE EXTENSION IF NOT EXISTS vector")
        .execute(pool)
        .await
        .is_ok()
}

fn doc(id: &str, tenant: TenantId, text: &str, fields: &[(&str, &str)], vector: Vec<f32>) -> SearchDocument {
    SearchDocument {
        id: id.into(),
        tenant,
        text: text.into(),
        fields: fields.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<BTreeMap<_, _>>(),
        vector,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn keyword_search_filters_facets_pagination_and_tenant_isolation() {
    let Some(pool) = pool().await else { return };
    let engine = PgVectorSearchEngine::new(pool.clone());
    let index = Index("kwten01".into());
    engine.drop_table(&index).await.unwrap();
    engine.create_table(&index, None).await.unwrap();

    let t1 = TenantId::new();
    let t2 = TenantId::new();
    engine.index(&index, doc("a", t1, "cardiology consultation followup", &[("status", "open"), ("city", "delhi")], vec![])).unwrap();
    engine.index(&index, doc("b", t1, "cardiology surgery referral", &[("status", "closed"), ("city", "delhi")], vec![])).unwrap();
    engine.index(&index, doc("c", t1, "dermatology rash treatment", &[("status", "open"), ("city", "pune")], vec![])).unwrap();
    engine.index(&index, doc("x", t2, "cardiology secrets of another tenant", &[], vec![])).unwrap();

    // tenant isolation
    let all = engine.search(&index, &SearchQuery { tenant: t1, text: "cardiology".into(), limit: 10, ..Default::default() }).unwrap();
    assert_eq!(all.total, 2, "only tenant t1 docs match");
    assert!(all.hits.iter().all(|h| ["a", "b"].contains(&h.id.as_str())));

    // exact filter
    let open = engine.search(&index, &SearchQuery { tenant: t1, text: "cardiology".into(), filters: [("status".to_string(), "open".to_string())].into_iter().collect(), limit: 10, ..Default::default() }).unwrap();
    assert_eq!(open.total, 1);
    assert_eq!(open.hits[0].id, "a");

    // facets over the keyword match set
    let faceted = engine.search(&index, &SearchQuery { tenant: t1, text: "cardiology".into(), facets: vec!["city".into()], limit: 10, ..Default::default() }).unwrap();
    let city = faceted.facets.iter().find(|f| f.field == "city").unwrap();
    assert_eq!(city.values.len(), 1);
    assert_eq!(city.values[0].value, "delhi");
    assert_eq!(city.values[0].count, 2);

    // empty text matches all (filter/facet semantics), pagination stable by id
    let page = engine.search(&index, &SearchQuery { tenant: t1, text: String::new(), limit: 2, offset: 1, ..Default::default() }).unwrap();
    assert_eq!(page.total, 3);
    assert_eq!(page.hits.len(), 2);
    assert_eq!(page.hits[0].id, "b");

    // count + delete
    assert_eq!(engine.count(&index, t1).unwrap(), 3);
    engine.delete(&index, "a").unwrap();
    assert_eq!(engine.count(&index, t1).unwrap(), 2);
    assert_eq!(engine.count(&index, t2).unwrap(), 1);

    engine.drop_table(&index).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reindex_same_id_upserts() {
    let Some(pool) = pool().await else { return };
    let engine = PgVectorSearchEngine::new(pool);
    let index = Index("kwups01".into());
    engine.drop_table(&index).await.unwrap();
    engine.create_table(&index, None).await.unwrap();
    let t1 = TenantId::new();
    engine.index(&index, doc("a", t1, "first version of the note", &[], vec![])).unwrap();
    engine.index(&index, doc("a", t1, "second version of the note", &[("v", "2")], vec![])).unwrap();
    assert_eq!(engine.count(&index, t1).unwrap(), 1);
    let res = engine.search(&index, &SearchQuery { tenant: t1, text: "second".into(), limit: 5, ..Default::default() }).unwrap();
    assert_eq!(res.total, 1);
    assert_eq!(res.hits[0].fields.get("v"), Some(&"2".to_string()));
    engine.drop_table(&index).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn vector_search_orders_by_similarity_and_applies_min_score() {
    let Some(pool) = pool().await else { return };
    if !vector_available(&pool).await { return };
    let engine = PgVectorSearchEngine::new(pool);
    let index = Index("vecord01".into());
    engine.drop_table(&index).await.unwrap();
    engine.create_table(&index, Some(3)).await.unwrap();
    let t1 = TenantId::new();
    let other = TenantId::new();
    engine.index(&index, doc("near", t1, "close vector", &[], vec![0.9, 0.1, 0.0])).unwrap();
    engine.index(&index, doc("mid", t1, "mid vector", &[], vec![0.5, 0.5, 0.0])).unwrap();
    engine.index(&index, doc("far", t1, "far vector", &[], vec![0.0, 0.0, 1.0])).unwrap();
    engine.index(&index, doc("no-vec", t1, "no embedding at all", &[], vec![])).unwrap();
    engine.index(&index, doc("other-t", other, "other tenant near vector", &[], vec![0.95, 0.05, 0.0])).unwrap();

    let res = engine.vector_search(&index, &VectorQuery { tenant: t1, vector: vec![1.0, 0.0, 0.0], limit: 10, min_score: 0.0 }).unwrap();
    let ids: Vec<&str> = res.hits.iter().map(|h| h.id.as_str()).collect();
    assert_eq!(ids.first(), Some(&"near"));
    assert!(ids.contains(&"mid"));
    assert!(ids.contains(&"far"));
    assert!(!ids.contains(&"no-vec"), "documents without embeddings excluded");
    assert!(!ids.contains(&"other-t"), "tenant isolation holds");

    let strict = engine.vector_search(&index, &VectorQuery { tenant: t1, vector: vec![1.0, 0.0, 0.0], limit: 10, min_score: 0.8 }).unwrap();
    assert_eq!(strict.hits.len(), 1);
    assert_eq!(strict.hits[0].id, "near");
    engine.drop_table(&index).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hybrid_blends_keyword_and_vector() {
    let Some(pool) = pool().await else { return };
    if !vector_available(&pool).await { return };
    let engine = PgVectorSearchEngine::new(pool);
    let index = Index("kwvec01".into());
    engine.drop_table(&index).await.unwrap();
    engine.create_table(&index, Some(3)).await.unwrap();
    let t1 = TenantId::new();
    // doc "both": strong keyword match + close vector -> must rank first
    engine.index(&index, doc("both", t1, "cardiology annual review", &[], vec![0.9, 0.1, 0.0])).unwrap();
    engine.index(&index, doc("kw-only", t1, "cardiology lecture notes", &[], vec![0.0, 0.0, 1.0])).unwrap();
    engine.index(&index, doc("vec-only", t1, "unrelated words entirely", &[], vec![0.95, 0.05, 0.0])).unwrap();

    let res = engine.hybrid_search(&index, &HybridQuery {
        query: SearchQuery { tenant: t1, text: "cardiology".into(), limit: 10, ..Default::default() },
        vector: vec![1.0, 0.0, 0.0],
        alpha: 0.5,
    }).unwrap();
    assert_eq!(res.total, 3);
    assert_eq!(res.hits[0].id, "both", "doc strong on both signals wins");
    let ids: Vec<&str> = res.hits.iter().map(|h| h.id.as_str()).collect();
    assert!(ids.contains(&"kw-only"));
    assert!(ids.contains(&"vec-only"));
    engine.drop_table(&index).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn keyword_only_table_rejects_vector_ops() {
    let Some(pool) = pool().await else { return };
    let engine = PgVectorSearchEngine::new(pool);
    let index = Index("kwnovec01".into());
    engine.drop_table(&index).await.unwrap();
    engine.create_table(&index, None).await.unwrap();
    let t1 = TenantId::new();
    let err = engine.vector_search(&index, &VectorQuery { tenant: t1, vector: vec![0.1], limit: 5, min_score: 0.0 });
    assert!(err.is_err(), "vector ops fail fast on keyword-only tables");
    engine.drop_table(&index).await.unwrap();
}