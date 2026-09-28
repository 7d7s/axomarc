-- 0002_appointment_slice.sql — minimal W5/W6 slice: slots, appointments, queue tokens.
-- Doctor/patient/clinic full tables land with M4 tasks (module-owned migrations).

CREATE TABLE IF NOT EXISTS slots (
  id             uuid PRIMARY KEY,
  tenant_id      uuid NOT NULL REFERENCES tenants(id),
  node_id        uuid REFERENCES org_nodes(id),
  doctor_user_id uuid NOT NULL,
  service_id     uuid,
  starts_at      timestamptz NOT NULL,
  ends_at        timestamptz,
  state          text NOT NULL DEFAULT 'open', -- open|held|booked|done|expired
  hold_until     timestamptz,
  appointment_id uuid
);
CREATE INDEX IF NOT EXISTS slots_doctor_time ON slots (doctor_user_id, starts_at);
CREATE INDEX IF NOT EXISTS slots_open ON slots (state) WHERE state = 'open';

ALTER TABLE slots ENABLE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS tenant_isolation ON slots;
CREATE POLICY tenant_isolation ON slots
  USING (tenant_id = current_setting('medicare.tenant_id', true)::uuid);

CREATE TABLE IF NOT EXISTS appointments (
  id             uuid PRIMARY KEY,
  tenant_id      uuid NOT NULL REFERENCES tenants(id),
  owner_node     uuid REFERENCES org_nodes(id),
  slot_id        uuid UNIQUE REFERENCES slots(id),
  patient_kind   text NOT NULL DEFAULT 'self',
  patient_id     uuid,
  doctor_user_id uuid NOT NULL,
  starts_at      timestamptz NOT NULL,
  status         text NOT NULL DEFAULT 'SCHEDULED',
  version        bigint NOT NULL DEFAULT 1,
  created_at     timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS appointments_owner_time ON appointments (owner_node, starts_at);
CREATE INDEX IF NOT EXISTS appointments_doctor_time ON appointments (doctor_user_id, starts_at);

ALTER TABLE appointments ENABLE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS tenant_isolation ON appointments;
CREATE POLICY tenant_isolation ON appointments
  USING (tenant_id = current_setting('medicare.tenant_id', true)::uuid);

CREATE TABLE IF NOT EXISTS queue_tokens (
  appointment_id uuid PRIMARY KEY REFERENCES appointments(id),
  doctor_user_id uuid NOT NULL,
  day            date NOT NULL,
  token_no       int NOT NULL,
  issued_at      timestamptz NOT NULL DEFAULT now(),
  called_at      timestamptz,
  completed_at   timestamptz,
  UNIQUE (doctor_user_id, day, token_no)
);