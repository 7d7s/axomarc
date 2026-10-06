-- Target-arch §5.1 (Phase 3: job payload versioning, KEEL-005/KEEL-V09):
-- durable jobs carry their payload schema version, stamped at enqueue and
-- migrated at claim. NULL = pre-Phase-3 rows — the mappers read those as
-- v1, so every in-flight job loads with zero deserialization errors after
-- the v2 deploy (the §10 DoD row).
-- MySQL twin of migrations/20261005000003_jobs_queue_schema_version.sql.
ALTER TABLE jobs_queue ADD COLUMN schema_version INT NULL;
