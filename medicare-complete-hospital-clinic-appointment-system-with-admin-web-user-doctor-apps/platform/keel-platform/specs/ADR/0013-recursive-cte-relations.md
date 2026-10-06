# ADR 0013 — Recursive/CTE relations in the query builder

Status: **Proposed** (2026-10-04) · Gate: Wave 9 advanced backlog (docs/19,
"Advanced backlog") — complexity + risk review · Implementation: none.

## Context

`keel-query`'s clause tree renders subqueries into the same parameter
vector as the enclosing statement (recursive clause-tree rendering,
`crates/execution/query/src/lib.rs`) and covers selects, aggregates,
existence, unions and locks — but it has no `WITH` / `WITH RECURSIVE`
node: searching the crate for `RECURSIVE` / `CommonTable` finds only the
renderer's own recursion comment. Hierarchical data (org charts,
category trees, bill-of-materials, anything parent/child without a
closure table) is therefore served one level at a time: the caller
issues N queries (one per depth) or keeps a materialized path/closure
table in the schema.

B-31 makes this decision dual-dialect: every clause added now renders
for PostgreSQL AND MySQL 8 (both support `WITH RECURSIVE`, neither in a
way that offloads termination — the cycle/depth guard is ours to
enforce). The isolation story also differs per fabric: pg rows pass
through RLS, MySQL rows through the explicit tenant predicates the
model layer injects. A CTE adds ADDITIONAL SELECT legs (anchor,
recursive term, final select) that must all stay under the same guard.

Constraints:

1. Termination is mandatory and structural: a build must carry a depth
   bound — the builder never emits an unbounded `WITH RECURSIVE`.
2. Every SELECT inside the CTE carries the same tenant guard as a
   top-level query (no unguarded anchor or recursive term; the pg RLS
   and MySQL explicit-predicate stories must both hold through the
   CTE legs).
3. Values bind as parameters; identifiers come from the registry or the
   closed grammar (quoted) — no request string is ever interpolated.
4. Budget: pure rendering — zero new dependencies (the reason this item
   is complexity-gated, not budget-gated).
5. `MODULE_ABI` and the manifest are untouched: the clause tree is not
   manifest surface.

## Decision

CTE support remains decision-gated: no implementation until the
complexity + risk review passes. The gate conditions if it passes:

- **Clause shape**: an additive leading `WITH` node —
  `WITH [RECURSIVE] name (columns) AS (anchor UNION ALL recursive_term)`
  — rendered before `FROM`, with the recursive flag selecting the
  dialect keyword; the final SELECT may reference the CTE and joins
  normally, with each leg guarded per constraint 2.
- **Termination**: `max_depth` (or an equivalent cycle-safe bound) is
  REQUIRED at build time — the builder refuses to render without it, and
  the bound is pushed into the recursive term (`depth < :max` style)
  rather than trusted to the caller's WHERE clause. The portable guard
  is the depth cap (PG's `CYCLE` clause is not the portable story);
  dialect-specific path-array guards are additive follow-ups, not the
  contract.
- **Evidence**: rendering-shape unit tests in the EXISTING keel-query
  suite (parameter vectors, the mandatory bound, refusal without it,
  tenant guard present on every leg, both dialects' keyword rendering),
  plus a live hierarchical round-trip appended to the existing
  `pg-tests` suites when implemented (the scheme-based skip covers both
  fabrics) — suite count stays 150.
- **Scope-out**: materialized-path and closure-table designs stay
  application-level strategies the builder does not need to know about;
  a raw-SQL escape node does NOT exist in keel-query and would be its
  own separate gate (injection surface), not a shortcut into this one.

Alternatives considered: level-by-level iteration (status quo — correct
for shallow trees, N+1 for deep ones); a schema-level closure table
(already possible without any builder change); a raw fragment node
(refused as a shortcut — the closed-grammar posture exists precisely
because string-shaped SQL is not an API).

## Consequences

- Nothing moves now: no clause-tree change, no rendering, no tests —
  hierarchical reads keep using level-by-level iteration or
  materialized paths, recorded as the status quo.
- This document is the gate artifact: implementation starts only after
  the complexity + risk review accepts the conditions above (the ADR
  then flips Proposed -> Accepted with the review reference).
- When accepted, the work is additive (new clause node + renderer arm),
  budget-neutral, and dual-dialect from the first landing — no
  postgres-only fast path.
