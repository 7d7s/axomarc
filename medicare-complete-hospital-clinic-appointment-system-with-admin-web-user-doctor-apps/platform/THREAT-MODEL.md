# Threat Model (foundation)

Assets: tenant data, delta/audit integrity, identity/authorization state, release
integrity.

## Trust boundaries
1. Internet → Edge (untrusted input; validation + envelope errors only).
2. Edge → Tenant plane (P1: edge cannot mutate tenant state).
3. Platform → Tenant (P2/P3: no implicit cross-plane authority; support via lease).
4. App → Postgres (RLS is the final barrier; runtime roles are least-privilege).

## Principal threats & mitigations
- **Cross-tenant leakage** — typed context + policy + `SET LOCAL kelson.tenant_id`
  + RLS policies on every tenant table (defense in depth; any single layer
  failing does not leak).
- **Privilege escalation** — capability conjunction, no deny-overrides needed
  because allow is conjunction-shaped; elevation only via audited lease.
- **Replay/duplicate execution** — idempotency port with payload-hash conflict
  detection; replay returns stored result.
- **Partial writes** — single-transaction delta/audit/outbox; events post-commit.
- **Tampered history** — deltas are append-only stones with per-stream sequence;
  audit references execution chain.
- **Supply chain** — pinned minimal external set, budget <= 20 direct deps,
  reviewed upgrades (DEPENDENCY-POLICY.md).
- **Secrets exposure** — boot-time asserts; no prod defaults; secrets never in
  error meta (PHI-free rule).

## Out of scope (foundation)
Network segmentation, WAF, and operator-level controls belong to deployment
hardening, tracked separately.