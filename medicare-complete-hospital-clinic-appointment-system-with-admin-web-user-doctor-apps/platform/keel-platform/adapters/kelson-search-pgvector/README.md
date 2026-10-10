# kelson-search-pgvector

PostgreSQL + pgvector `SearchEngine` adapter over the frozen kelson-search
contract. No new dependencies: vectors are exchanged in pgvector text form
(`[0.1,0.2,...]`) cast with `::vector`, so plain sqlx is enough.

Layout: one table per index (created via `create_table` / the pure
`create_table_sql` migration helper), columns
`id TEXT PK, tenant_id UUID, body TEXT, fields JSONB, embedding vector(N)`.
Every query is tenant-scoped.

Modes:
- keyword: `to_tsvector('simple', body) @@ plainto_tsquery` + `ts_rank`,
  exact JSONB filters, GROUP BY facets, LIMIT/OFFSET pagination;
- vector: cosine distance `embedding <=> probe`, `min_score` post-filter;
- hybrid: one windowed candidate query (top 500 by max(kw, sim)), alpha-blend
  with keyword normalization in Rust (mirrors the in-memory reference).

Tables without an embedding column (created with `dims: None`) support the
keyword-only surface; vector/hybrid ops then fail fast with a validation
error. Integration tests gate on `--features pg-tests` + `DATABASE_URL` and
skip gracefully when the pgvector extension is unavailable.