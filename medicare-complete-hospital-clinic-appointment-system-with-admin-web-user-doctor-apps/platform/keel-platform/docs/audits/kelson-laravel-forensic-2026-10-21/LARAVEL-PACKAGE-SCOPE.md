# LARAVEL PACKAGE SCOPE — core, official packages, external ecosystem

**The scope mismatch is shown explicitly** ✗ — a Laravel optional package is never compared against a
missing Kelson core feature as if they were the same thing ✓ ✓.

## 1. The tiers

| Tier | What it is | How it is reported |
|---|---|---|
| **L0** | the `laravel/framework` core | **in the core score** ✓ (the 111 rows ✓) ✓ |
| **L1** | the official first-party packages | **separately** ✓ — this document ✓ + `SELECTION-DECISIONS.csv` ✓ ✓ |
| **L2** | the officially documented drivers and integrations | in the core score, with the tier recorded per row ✓ ✓ |
| **L3** | the third-party ecosystem | **annex only** ✓ — never counted as core parity ✓ ✓ |
| **L4** | the infrastructure and developer tooling | the ecosystem annex ✓ + the CI gate analysis ✓ ✓ |

## 2. L0 — the framework core: the 111 rows

The core score ✓ (from `RELEASE-GATES.json` ✓):

| Metric | Value |
|---|---|
| `surface_coverage` | 92 / 111 = **82.9 %** |
| `behavioral_verification` | 40 / 111 = **36.0 %** |
| `integration_verified` | 23 / 111 = **20.7 %** |
| `end_to_end_verified` | 10 / 111 = **9.0 %** |
| `security_hardened` | 40 / 111 = **36.0 %** |
| `production_hardened` | 0 / 111 = **0.0 %** |
| `selection_coverage` | 40 / 111 = **36.0 %** |
| `test_evidence_coverage` | 92 / 111 = **82.9 %** |

**The gap between 82.9 % and 36.0 % is the headline** ✓: most of what the atlas names is
**implemented** ✓; about a third is **proven** ✓ ✗ ✗.

## 3. L1 — the official first-party packages

Re-enumerated from the bundle's package list ✓ (a **seed** ✓ — the live 13.x docs index was **not**
fetched in this environment ✓, so the list must be re-checked ✓ ✓).

| Laravel package | Kelson's position | Tier | Decision |
|---|---|---|---|
| **Sanctum** (the token model) | **implemented and stronger** ✓ — the abilities are a ceiling at the policy boundary ✓ (KFA-010 ✓); the SPA cookie mode ✓ | L1 | `REQUIRED_FOR_PROFILE` ✓ — shipped ✓ ✓ |
| **Passport** (the OAuth server) | the OAuth2 **contract** crates exist ✓ (`kelson-oauth2-contract` ✓) but the server is a separate product ✓ | L1 | `DEFER_UNTIL_FOUNDATION_STABLE` ✓ |
| **Fortify** (the headless auth) | the auth **core** is shipped ✓ (the identity ✓, the tokens ✓, the sessions ✓, the reset ✓, the verification ✓, the 2FA ✓) — the **scaffolding** is not ✓ | L1 | `REQUIRED_FOR_PROFILE` (the core ✓) + `OPTIONAL_ADDON` (the scaffolding ✓) ✓ |
| **Pennant** (the flags) | **implemented** ✓ — the stages ✓, the dimensions ✓, the rollout ✓, the per-tenant override ✓, the cross-instance store ✓; the **stale-cache** and the **fail-open/closed** cases are unnamed ✗ | L1 | `REQUIRED_FOR_PROFILE` ✓ — shipped ✓, with a named gap ✓ ✓ |
| **Scout** (the search) | the **port** is shipped ✓; **no engine adapter** ✗; **no index lifecycle** ✗ | L1 | `ADAPTER_OR_OFFICIAL_INTEGRATION` ✓ — the engine is an adapter ✓ ✓ |
| **Cashier** (Stripe / Paddle) | **nothing** ✗ — no billing crate ✗ | L1 | `DEFER_UNTIL_FOUNDATION_STABLE` ✓ — the atlas says "do not place payment business logic in kernel" ✓ ✓ |
| **Horizon** (the queue ops UI) | the **console commands** are shipped ✓ (`queue:monitor` ✓, `queue:failed` ✓, `queue:retry` ✓, `queue:prune-batches` ✓); **no dashboard** ✗ | L1 | `OPTIONAL_ADDON` ✓ — the commands are the required part ✓ ✓ |
| **Pulse** (the metrics dashboard) | the **metrics and the OTLP exporter** are shipped ✓; **no dashboard** ✗; the cardinality is unproven ✗ | L1 | `OPTIONAL_ADDON` ✓ ✓ |
| **Telescope** (the request debugger) | the **audit records and the incident plane** are shipped ✓; **no request/query inspector** ✗ | L1 | `OPTIONAL_ADDON` ✓ (P3 ✓) ✓ |
| **Reverb** (the WebSocket server) | the **realtime crate** is shipped ✓ (the hub ✓, the channels ✓, the WS route ✓, the presence ✓, the authorized transport ✓); the **revoked authz**, the **origin validation** and the **multi-node pub/sub** are unnamed ✗ | L1 | `OPTIONAL_ADDON` ✓ — P2 ✓ ✓ |
| **Octane** (the long-lived workers) | the **long-lived worker** is shipped ✓ (the claim loop ✓, the tenant scope ✓, the restart signal ✓, the panic boundary ✓); the **drain**, the **memory growth** and the **graceful termination** are unnamed ✗ | L1 | `OPTIONAL_ADDON` ✓ — P2 ✓, but the cautionary model ✓ (the atlas's own words ✓) ✓ |
| **AI SDK** | the **client** ✓ (the messages ✓, the tools ✓, the stub ✓, the usage ✓), the **tools** ✓, the **agents** ✓ (the capability ceiling ✓, the step budget ✓), the **MCP** ✓ (the manifest ✓) — no streaming ✗, no structured output ✗, no timeout/retry ✗ | L1 | `OPTIONAL_ADDON` ✓ — P3 ✓ ✓ |
| **MCP** | the **manifest** ✓ + the console's `mcp:tools` ✓; **no wire transport** ✗ and the tool-execution safety is inherited rather than proven ✗ | L1 | `OPTIONAL_ADDON` ✓ ✓ |
| **Prompts** | the **text and confirm** prompts ✓ with the non-interactive discipline ✓; the rich set ✗ | L1 | `OPTIONAL_ADDON` ✓ ✓ |
| **Dusk** | **nothing** — no browser suite ✗; the starter template ships the Playwright specs ✗ (never executed ✓) | L1 | `OPTIONAL_ADDON` ✓ (the atlas: "if Playwright is selected" ✓) ✓ |
| **Envoy** | the **promotion pipeline** is shipped ✓ (the plan ✓, the checkpoint ✓, the rollback ✓, the **lock** ✓, the digests ✓, the drift ✓) — a Rust-native equivalent, not an SSH runner ✓ | L1 | `EQUIVALENT_DIFFERENT_API` ✓ ✓ |
| **Folio** | the **route files** merged as synthetic modules ✓ (the nearest analogue ✓ — manifest-anchored and typed ✓) | L1 | `EXCLUDE_BY_DESIGN` ✓ ✓ |
| **Socialite** | the **provider-neutral** auth core ✓; no provider set ✗ | L1 | `DEFER_UNTIL_FOUNDATION_STABLE` ✓ |
| **Head** | `@push`/`@stack` ✓ (a stack, not a metadata model ✗) | L1 | `DEFER_UNTIL_FOUNDATION_STABLE` ✓ (P3 ✓) ✓ |
| **Sail** | **nothing tracked** ✗ (a compose file exists in the working tree ✗ — untracked ✓) | L1 | `OPTIONAL_ADDON` ✓ — the atlas says "need not copy Sail" ✓ ✓ |
| **Valet / Homestead** | **nothing** ✓ — `cargo run` is the equivalent ✓ | L1 | `EXCLUDE_BY_DESIGN` ✓ ✓ |
| **Pint** | `rustfmt` ✓ — the atlas's own instruction ("adopt an equivalent Rust formatting/linting gate" ✓); **not enforced** ✗ (1,606 files ✗) | L1 | `REQUIRED_FOR_PROFILE` ✓ — the gate, not the tool ✓ ✓ |
| **Mix** | the views engine + the static-file serving ✓ (the asset **bundler** is external ✓) | L1 | `EXCLUDE_BY_DESIGN` ✓ (legacy ✓) ✓ |
| **Boost** | **nothing** ✓ — a developer-productivity concern ✗ | L1 | `EXCLUDE_BY_DESIGN` ✓ (P3 ✓, the atlas says not to count it ✓) ✓ |
| **Livewire / Inertia** | **nothing** ✓ — the views engine + a SPA-first shell ✓ | L1 | `EXCLUDE_BY_DESIGN` ✓ (product decisions ✓ — F-5 corrected the attribution ✓) ✓ |
| **starter kits** | the **starter template** ✓ (a notes module ✓, a web shell ✓, the error views ✓, the e2e kit ✓) — no kit **choice** ✗ | L1 | `OPTIONAL_ADDON` ✓ ✓ |
| **Precognition** | the **validate-only dispatch** is shipped ✓ (the `Precognition` header ✓, 6 cases ✓) | L1 | `REQUIRED_FOR_PROFILE` ✓ — shipped ✓ ✓ |

## 4. L2 — the documented drivers and integrations

| Driver | Kelson's position |
|---|---|
| PostgreSQL | the adapter ✓ — but the **engine is unavailable on this host** ✗ (KFA-022 ✓) |
| MySQL 8 | the adapter ✓ — the live matrix ran ✓ (the outbox's 4 cases ✓) |
| SQLite / SQL Server | **not implemented** ✗ (the atlas's P1 asks for "PostgreSQL/MySQL/SQLite/SQL Server as declared" ✓ — the *declaration* is what matters ✓, and the declaration is pg + MySQL ✓ ✓) |
| Redis | the adapters ✓ (the cache ✓, the sessions ✓, the job store ✓, the rate limit ✓) — 15 live cases ✓ ✓ |
| Object storage (S3) | a **sandbox** ✗ — no real driver ✗ (KFA-014's sibling ✓) |
| Mail (SMTP) | the adapter ✓ + the chain ✓ ✓ |
| Broadcasting | the realtime crate ✓ (not the Laravel broadcasting abstraction ✗) ✓ |
| Search (an engine) | a **port** ✓ — no engine ✗ ✓ |
| Image manipulation | **nothing** ✗ (LF-077 ✓ — the atlas's P2 ✓) ✓ |

## 5. L3 — the third-party ecosystem (annex, not scored)

The atlas's L3 set (community packages and external applications ✓) is **out of the competitive
scope** ✓ — it is not comparable to a framework's own surface ✗ ✗ and is recorded here only so the
scope is explicit ✓ ✓.

## 6. L4 — the infrastructure and the developer tooling

| Item | Kelson's position |
|---|---|
| PHP / Composer | **not applicable** — the runtime is Rust ✓ (a different tier ✓, never compared line by line ✓) ✓ |
| Vite / an asset bundler | the **manifest reader** is shipped ✓; the bundler is external ✓ ✓ |
| Docker / Sail | **not tracked** ✗ (KFA-021 ✓) ✓ |
| CI | the architecture ✓, the lint ✓, the semver ✓, the conformance ✓, the scaffold ✓ and the coverage steps ✓ — but no fmt ✓, no threshold ✓, no audit step ✗ ✓ |
| Browser automation | the **specs** ship ✓, nothing executes them ✗ ✓ |
| Database engines | pg ✓ + MySQL ✓ (one of them is unavailable here ✓) ✓ |
| Production operations | the release pipeline ✓, the digests ✓, the drift ✓, the lock ✓ — but no container ✓, no clean-room deploy ✓, no restore drill ✗ ✓ |

## 7. The conclusion on scope

**The core is largely implemented and only partly proven** ✓ ✓. The **package layer is where Kelson
is genuinely thin** ✓ ✓ — the equivalents exist for the packages that carry Kelson's architecture
(the tokens ✓, the flags ✓, the deployment ✓, the realtime ✓, the AI ✓) and are absent for the ones
that carry Laravel's *product breadth* (the billing ✓, the dashboards ✓, the auth scaffolding ✓,
the search engine ✓, the browser kit ✓, the dev environment ✓) ✓ ✓.

**That is a coherent position** ✓ — Kelson's chosen scope is the framework core ✓, the module model ✓
and the enterprise isolation ✓, and it is honest about the rest ✓ ✓. The competitive risk is not the
missing packages ✗ — it is that the **implemented** core is only **third-proven** ✗ ✗ (36 % ✓) and
that the **supply chain and the host** are not release-ready ✗ ✗ ✓ ✓.
