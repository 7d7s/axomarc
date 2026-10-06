-- Target-arch §3.1 (Phase 2: tenant scoping): jobs carry their tenant so
-- the queue can be claimed / completed / failed per tenant (TenantJobStore).
-- NULL = platform job (infrastructure enqueues and pre-Phase-2 rows) —
-- visible only through the break-glass physical JobStore, mirroring the
-- keel_platform database role. Tenant is stamped at the port boundary and
-- never changes on an existing row.
ALTER TABLE jobs_queue ADD COLUMN IF NOT EXISTS tenant_id uuid;

CREATE INDEX IF NOT EXISTS ix_jobs_queue_tenant ON jobs_queue (tenant_id, status);
