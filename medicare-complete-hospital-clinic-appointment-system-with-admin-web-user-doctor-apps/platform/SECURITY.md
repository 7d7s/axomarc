# Security Policy

## Scope
The trusted computing base is `crates/foundation/` + `crates/execution/` + `crates/security/`. Anything touching delta
integrity, tenant isolation, or authorization is security-sensitive and requires
review under GOVERNANCE.md.

## Tenant isolation model
1. Typed identities — Principal ≠ Actor ≠ Membership (`keel-identity`, `keel-context`).
2. Policy decision with allow-conjunction + revision staleness (`keel-policy`).
3. Postgres GUC `keel.tenant_id` + Row-Level-Security as the final containment
   barrier (D-47). Business code never hand-writes tenant filters.
4. Runtime DB roles never own tables (`database/roles.sql`).

## Elevation
Platform support access is lease-based (P4/P5) via `keel-elevation`: bounded,
audited, and revoked by expiry. No implicit platform authority over tenant data.

## Secrets
- No defaults in prod; boot asserts secret posture (`keel-config::assert_secrets`).
- Dev placeholders are refused when `app.env = "prod"`.

## Reporting a vulnerability
Do not open a public issue. Report privately to the maintainers
(`security@keel.invalid` placeholder until the project publishes a real address).
Acknowledge within 5 working days; target fix disclosure within 90 days.