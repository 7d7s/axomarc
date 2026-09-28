//! Bridge regression: a PgPool created under the test's multi-threaded
//! runtime must survive repeated sync store calls (historically deadlocked
//! when pooled connections were polled from a foreign runtime).
#![cfg(feature = "pg-tests")]

use keel_postgres::{bridge, create_pool, PgPool};

async fn pool() -> Option<PgPool> {
    let url = match std::env::var("DATABASE_URL") {
        Ok(url) if !url.is_empty() => url,
        _ => return None,
    };
    create_pool(&url, 4).await.ok()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pg_bridged_queries_reuse_pool_across_runtimes() {
    let Some(pool) = pool().await else { return };
    for round in 0..3 {
        let p = pool.clone();
        let row: (i32,) = bridge::block_on(async move {
            sqlx::query_as("SELECT 1").fetch_one(&p).await.expect("bridge query")
        });
        assert_eq!(row.0, 1, "round {round}");
        // Interleave an async query on the test runtime between bridge calls.
        let (n,): (i32,) = sqlx::query_as("SELECT 2").fetch_one(&pool).await.expect("direct");
        assert_eq!(n, 2);
    }
}
