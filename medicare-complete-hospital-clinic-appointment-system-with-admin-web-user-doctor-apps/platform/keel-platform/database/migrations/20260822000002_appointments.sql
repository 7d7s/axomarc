-- Appointments domain table (S8 C3/B-04): first real module table persisted
-- through the framework ModelStore port (PgModelStore over keel-models).
-- Partial unique index = the real double-booking contention guard: one BOOKED
-- appointment per (tenant, patient, slot); cancelled rows do not block.
CREATE TABLE IF NOT EXISTS appointments (
    id         uuid PRIMARY KEY,
    tenant_id  uuid NOT NULL REFERENCES tenants(id),
    patient    text NOT NULL,
    slot       text NOT NULL,
    status     text NOT NULL DEFAULT 'booked',
    version    bigint NOT NULL DEFAULT 1,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    deleted_at timestamptz
);
CREATE INDEX IF NOT EXISTS ix_appointments_tenant ON appointments (tenant_id);
CREATE UNIQUE INDEX IF NOT EXISTS ux_appointments_booked_slot
    ON appointments (tenant_id, patient, slot)
    WHERE deleted_at IS NULL AND status = 'booked';