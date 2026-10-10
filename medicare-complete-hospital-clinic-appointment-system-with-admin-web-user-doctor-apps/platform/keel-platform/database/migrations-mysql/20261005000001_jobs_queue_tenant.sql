-- Target-arch §3.1 (Phase 2: tenant scoping): jobs carry their tenant so
-- the queue can be claimed / completed / failed per tenant (TenantJobStore).
-- NULL = platform job (infrastructure enqueues and pre-Phase-2 rows) —
-- visible only through the break-glass physical JobStore, mirroring the
-- kelson_platform database role. Tenant is stamped at the port boundary and
-- never changes on an existing row. MySQL twin of
-- migrations/20261005000001_jobs_queue_tenant.sql.
ALTER TABLE jobs_queue ADD COLUMN tenant_id CHAR(36) NULL;

CREATE INDEX ix_jobs_queue_tenant ON jobs_queue (tenant_id, status);
