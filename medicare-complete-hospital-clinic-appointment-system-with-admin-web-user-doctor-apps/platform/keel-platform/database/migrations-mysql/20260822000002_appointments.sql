-- MySQL 8 dialect twin of database/migrations/20260822000002_appointments.sql.
-- uuid: BINARY(16) (sqlx MySQL Uuid codec is binary)
-- Appointments domain table (S8 C3/B-04): first real module table persisted
-- through the framework ModelStore port (PgModelStore over keel-models).
-- Partial unique index = the real double-booking contention guard: one BOOKED
-- appointment per (tenant, patient, slot); cancelled rows do not block.
CREATE TABLE IF NOT EXISTS appointments (
    id          BINARY(16) PRIMARY KEY,
    tenant_id   BINARY(16) NOT NULL,
    patient     VARCHAR(191) NOT NULL,
    slot        VARCHAR(64) NOT NULL,
    status      TEXT NOT NULL DEFAULT ('booked'),
    version     bigint NOT NULL DEFAULT 1,
    created_at  DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at  DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    deleted_at  DATETIME(6),
    FOREIGN KEY (tenant_id) REFERENCES tenants(id)
) DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;
CREATE INDEX ix_appointments_tenant ON appointments (tenant_id);
-- MySQL has no partial indexes. The WHERE deleted_at IS NULL AND
-- status = 'booked' predicate moves into STORED generated columns: rows that
-- do not match yield NULL in every part, and a MySQL UNIQUE index permits
-- repeated NULLs, so the guard is exactly the Postgres partial unique index.
ALTER TABLE appointments
    ADD COLUMN tenant_id_active  BINARY(16)
        GENERATED ALWAYS AS (IF(deleted_at IS NULL AND status = 'booked', tenant_id, NULL)) STORED,
    ADD COLUMN patient_active    VARCHAR(191)
        GENERATED ALWAYS AS (IF(deleted_at IS NULL AND status = 'booked', patient, NULL)) STORED,
    ADD COLUMN slot_active       VARCHAR(64)
        GENERATED ALWAYS AS (IF(deleted_at IS NULL AND status = 'booked', slot, NULL)) STORED;
CREATE UNIQUE INDEX ux_appointments_booked_slot ON appointments (tenant_id_active, patient_active, slot_active);
