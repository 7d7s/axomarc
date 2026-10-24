-- MNC-001 / P1.1: one membership edge per (principal, tenant) pair.
--
-- The MembershipStore port contract: a second grant for the same pair is a
-- conflict, not a second edge. The in-memory store enforces it in code; this
-- unique index is what lets the durable stores enforce it too (Pg/MySQL
-- `grant` maps the unique violation to `conflict`).
--
-- Fail-closed on an upgrade that already holds duplicates: the operator must
-- resolve them explicitly - the platform never guesses which edge is real.
CREATE UNIQUE INDEX IF NOT EXISTS uq_memberships_principal_tenant
    ON memberships (principal_id, tenant_id);
