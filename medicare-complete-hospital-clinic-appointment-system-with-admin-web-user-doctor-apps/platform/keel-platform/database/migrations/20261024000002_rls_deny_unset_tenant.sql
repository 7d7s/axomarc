-- MNC-010 (S0): deny on an UNSET tenant GUC.
--
-- Every tenant policy in 20260819000001_foundation.sql ended
--     ... OR kelson_current_tenant() IS NULL
-- which made the final containment barrier vanish exactly when the
-- application misbehaved: a forgotten `SET LOCAL kelson.tenant_id` (or a
-- leaked/reset GUC) read ALL tenants instead of failing closed. The NULL
-- branch is dropped: an unset GUC now matches NOTHING.
--
-- Platform access is expressed explicitly instead of by accident:
--     OR current_user = 'kelson_platform'
-- preserves the break-glass platform plane (`PostgresBackend::platform`
-- never sets the GUC and relies on cross-tenant access for operations),
-- while the tenant role (`kelson_tenant`) must always anchor its
-- transaction, and the report role (`kelson_report`) no longer gets a
-- silent cross-tenant read (the "no tenant leakage" contract in
-- database/roles.sql now actually holds).
--
-- Applied by the owner role (migrations run as kelson_owner, which bypasses
-- RLS as the table owner); policies bind the runtime roles.

DROP POLICY tenant_isolation_memberships ON memberships;
CREATE POLICY tenant_isolation_memberships ON memberships
    USING (tenant_id = kelson_current_tenant() OR current_user = 'kelson_platform');

DROP POLICY tenant_isolation_scope_nodes ON scope_nodes;
CREATE POLICY tenant_isolation_scope_nodes ON scope_nodes
    USING (tenant_id = kelson_current_tenant() OR current_user = 'kelson_platform');

DROP POLICY tenant_isolation_scope_grants ON scope_grants;
CREATE POLICY tenant_isolation_scope_grants ON scope_grants
    USING (tenant_id = kelson_current_tenant() OR current_user = 'kelson_platform');

DROP POLICY tenant_isolation_capability_grants ON capability_grants;
CREATE POLICY tenant_isolation_capability_grants ON capability_grants
    USING (tenant_id = kelson_current_tenant() OR current_user = 'kelson_platform');

DROP POLICY tenant_isolation_delta_stones ON delta_stones;
CREATE POLICY tenant_isolation_delta_stones ON delta_stones
    USING (tenant_id = kelson_current_tenant() OR current_user = 'kelson_platform');

DROP POLICY tenant_isolation_audit ON audit_records;
CREATE POLICY tenant_isolation_audit ON audit_records
    USING (tenant_id = kelson_current_tenant() OR current_user = 'kelson_platform');

DROP POLICY tenant_isolation_outbox ON outbox;
CREATE POLICY tenant_isolation_outbox ON outbox
    USING (tenant_id = kelson_current_tenant() OR current_user = 'kelson_platform');

DROP POLICY tenant_isolation_idempotency ON idempotency_keys;
CREATE POLICY tenant_isolation_idempotency ON idempotency_keys
    USING (tenant_id = kelson_current_tenant() OR current_user = 'kelson_platform');
