# LARAVEL SCOPE BASELINE

**Frozen at audit start (2026-10-21).** The scope is fixed **before** any scoring. Deliberate
exclusions, unimplemented features and third-party integrations stay visible in the wide matrix; no
scored metric excludes them silently.

## 1. The repository under audit

| Item | Value |
|---|---|
| Repository root | `medicare-complete-hospital-clinic-appointment-system-with-admin-web-user-doctor-apps` |
| Audited workspace | `platform/keel-platform` (84 members) |
| Commit | `722a8931794cbfae5f95265c7a26d44e4ec6a71f` |
| Branch | `p0-runtime-safety` |
| Working tree | 282 entries dirty/untracked at the audit start (**the audit is read-only**; the dirt is pre-existing and unrelated) |
| Toolchain | `rustc 1.92.0 (ded5c06cf 2025-12-08)`, `cargo 1.92.0 (344c4567 2025-10-21)` |
| OS | Microsoft Windows 11 Home Single Language, x64 |
| Cargo features in use | the default set; the CI additionally builds `--features component-model` and `--all-features` |

## 2. The environment, re-detected (not assumed)

| Service / tool | State at audit time |
|---|---|
| MySQL 8 (port 3307, the `keel` user) | **not reachable** — the portable instance's `mysqld` binary is gone; the system instance on 3306 denies the `keel` user (credentials unknown) |
| Redis (port 6379) | reachable — **restarted by the audit** (`C:\Redis\redis-server.exe --port 6379 --bind 127.0.0.1`) |
| PostgreSQL (port 5433) | **not reachable** — the known native fault (`0xC0000142`) in the client backends |
| Docker | **absent** |
| chromedriver / Playwright | **absent** |
| `cargo-llvm-cov` 0.9.1 | present |
| `cargo-audit` 0.22.2 | present |
| `cargo-deny` 0.19.8 | present |
| `cargo-fuzz` 0.13.2 | present |
| node / npm / php / composer / python | present |

**Consequence:** the live-MySQL, live-PostgreSQL, container and browser gates are **NOT RUN** or
**BLOCKED** in this pass. They are recorded as such; they are not treated as passes.

## 3. The Laravel baseline

| Item | Value |
|---|---|
| Baseline | Laravel **13.x** stable documentation, as at the audit date |
| Source of record | the bundle's `catalog/laravel-feature-catalog.csv` (**111 rows**, historical seed) and `catalog/official-docs-domain-register.csv` (**133 rows**) |
| Re-check status | **NOT performed in this environment** — no live documentation fetch was made, so the baseline's currency is **unverified** and must be re-checked against the 13.x docs index before the catalogue is treated as complete |
| Official packages in scope | AI SDK, Cashier (Stripe/Paddle), Dusk, Envoy, Fortify, Folio, Head, Homestead, Horizon, Mix, Octane, Passport, Pennant, Pint, Precognition, Prompts, Pulse, Reverb, Sail, Sanctum, Scout, Socialite, Telescope, Valet, starter kits, Boost, MCP |

**Source tiers (per capability row, never merged):**

- **L0** — the `laravel/framework` core.
- **L1** — an official first-party package.
- **L2** — an officially documented integration or driver (database, cache, queue, mail, broadcasting,
  search, image, object storage, identity provider).
- **L3** — the third-party ecosystem (only where the declared scope requires it).
- **L4** — infrastructure and developer tooling (PHP/Composer, Vite, Docker/Sail, CI, browser
  automation, database engines, production operations).

**Every row in the matrix records its tier** — a Laravel optional package is never compared against a
missing Kelson core feature without the scope mismatch being shown.

## 4. The four comparison profiles

| # | Profile | In scope |
|---|---|---|
| 1 | **API backend** | JSON/HTTP APIs, auth, persistence, validation, queues/events, storage, observability, deployment |
| 2 | **Full-stack web** | profile 1 plus server-rendered UI/templates, assets, sessions/CSRF, URL/redirect/flash semantics, browser testing |
| 3 | **Enterprise multi-tenant platform** | profile 2 plus isolation, tenant configuration, module boundaries, independent artifacts, multi-node rollout, auditability, operations |
| 4 | **Ecosystem** | the official Laravel packages and the selected external integrations — **reported separately**, never merged into the core score |

## 5. The scoring scope (frozen)

- The **111 atlas rows** are the scoring universe (`P0-FEATURE-VERIFICATION.md` ✓).
- The 19 NOT-IMPLEMENTED rows stay in the denominator ✓ (no denominator games ✓) ✓.
- The ecosystem profile is **not** merged into the core score ✓; the official-package decisions are in
  `LARAVEL-PACKAGE-SCOPE.md` and `SELECTION-DECISIONS.csv` ✓ ✓.
- Two scopes are published: the **broad** one (all 111 rows ✓) and the **scoped** one (the rows in
  the declared profiles ✓) ✓ ✓.

## 6. What the baseline does **not** assert

- The 111 rows are a **seed**, not a proof of completeness ✓ (the 133-row domain register is the
  reconciliation checklist ✓) ✓.
- The official package list is re-enumerated from the current docs index in the next pass ✓ — it is
  **not** frozen to the bundle's list ✓ ✓.
- Nothing in this bundle asserts anything about Kelson's implementation ✓ — that is what this audit
  measured ✓ ✓.
