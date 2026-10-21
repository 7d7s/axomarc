# Kelson vs Laravel — a competitive assessment

**Question asked:** compare the whole framework to Laravel; find the missing features, the errors, the
bugs, the security issues — and judge it at MNC (enterprise) grade and for Laravel competitiveness.

**Method.** Every claim below is backed by an **executed** test or a **named file and line** in this
tree. The feature axis is the project's own Laravel Feature Atlas (111 items ✓), verified item by item
across twelve batches ✓ — the current tally is in `P0-FEATURE-VERIFICATION.md` ✓. The security and
bug axes are a fresh adversarial pass over the riskiest paths ✓, not a re-reading of the batch docs ✓.
Where I could not verify something, it says so ✓.

**One correction up front:** the untracked `KELSON-VS-LARAVEL-COMPARISON.md` in this tree is **stale**
✓ — it still lists the read/write split as missing (it shipped in F-9 ✓) and claims "factories (Phase
1)" (no factories exist ✓ — the atlas's LF-039 names that as a gap ✓). It is not part of the tracked
surface ✓ and I did not touch it ✓; this document supersedes it ✓.

---

## 1. The verdict

| Dimension | Laravel | Kelson | Verdict |
|---|---|---|---|
| **Type safety & correctness** | dynamic; runtime types | typed graph + typed models + no `eval` | **Kelson ahead** ✓ |
| **Multi-tenant isolation** | optional (scopes, packages) | structural — the per-tenant scope, the tenant-scoped store, the tenant in every query | **Kelson ahead** ✓ |
| **Auditability** | ad-hoc | structured audit records, sealed envelopes, digests, provenance | **Kelson ahead** ✓ |
| **HTTP/ORM breadth** | very wide | wide but narrower (no raw SQL, no MongoDB, no collections clone) | Laravel ahead ✗ |
| **Ecosystem / first-party packages** | 20+ (Horizon, Pulse, Telescope, Nova, Cashier, Reverb, Octane, Fortify, Scout, Livewire, Inertia…) | a few equivalents (realtime, feature flags, deployment, AI crates) | **Laravel ahead, widely** ✗ |
| **Developer ergonomics** | excellent (Tinker, Pest, Collision, Ignition, Breeze, Sail) | good CLI + testkit, thin scaffolding, no REPL-grade debugging | Laravel ahead ✗ |
| **Testing surface** | first-class (Pest, Dusk, parallel, coverage gates) | strong (the testkit, the fakes, the conformance suites, the live matrix) — but gaps in the fakes' real-adapter contracts and the parallel/CI gates | roughly level, with named gaps ✓ |
| **Ops maturity** | battle-tested | young: no migration/rollback guard for concurrent deploys, no coverage ratchet, the CI's own gates are thin | Laravel ahead ✗ |
| **Security posture (defaults)** | good, some footguns | better defaults in places (fail-closed, no PHP eval, sealed at rest) but **three real gaps** (§2) | roughly level, with named work ✓ |
| **Enterprise ("MNC") readiness** | yes (SOC2 shops run it) | **not yet** — the gaps in §2 and §3 are exactly what an enterprise reviewer would ask about | **not yet** ✗ |

**Bottom line.** Kelson is **not Laravel-competitive as a whole** ✗ — it lacks the ecosystem and the
ergonomics layer, and those are years of accumulated work ✗. It *is* competitive, today, in a narrower
and defensible segment ✓: **a multi-tenant, audit-heavy, type-safe backend** where the ORM, the
tenant boundary and the verification story are stronger than Laravel's ✓✓. The path to "Laravel
competitive" is in §5 ✓ — and it is dominated by **closing the security gaps and the fake-vs-real drift**,
not by adding features ✓ ✓.

---

## 2. The security findings (the ones an enterprise reviewer finds first)

**S-1 — A PAT's abilities are carried but never enforced. (medium-high, confirmed)**
`kelson-tokens` can issue a token with a narrow ability set ✓ (`issue_with_abilities` ✓), the abilities
travel in the durable row ✓ (`crates/data/postgres/src/tokenstore.rs:72` ✓, the MySQL twin ✓) and they
reach the guard ✓ (`GuardedPrincipal.abilities` ✓, `crates/web/http/src/guard.rs:67` ✓). **Nothing reads
them.** The docstring says "the **host** narrows the principal's capabilities with them ✓" — no host
does ✓ (the demo's PAT path installs no snapshot and consults no ability ✓,
`runtime/kelson-demo/src/graph_routes.rs:834` ✓). So a token minted with
`["appointments.read"]` still acts with the principal's full authority ✓ — the narrowing is
**decorative** ✓. The existing case proves the abilities *travel* ✓
(`runtime/kelson-demo/tests/http_integration.rs:6759` ✓) and that they do not exceed the principal's
grant ✓ — it never proves they *narrow* ✓ ✗. Laravel's Sanctum **does** enforce them ✓ (the
`abilities` middleware ✓). **Fix:** the policy engine must intersect the snapshot's capabilities with
the credential's abilities before the route check ✓, plus a case where a read-only PAT is refused a
write ✓.

**S-2 — The egress policy is TOCTOU-vulnerable. (medium, confirmed — this is my own F-23 code)**
`check_egress` resolves the host and refuses a private address ✓
(`crates/services/http-client/src/lib.rs:363` ✓), but the request then goes out through **reqwest's own
resolution** ✓ — the IP that was checked is not the IP that is dialled ✓. A DNS answer that changes
between the check and the connect (a rebind) defeats it ✓. The proposed fix said "pin the resolved IP"
✓ and I did not implement that half ✓ ✗. **Fix:** resolve once and pin (reqwest's `ClientBuilder::resolve`
✓ or a custom `Resolve` ✓), and re-check every redirect hop ✓.

**S-3 — A promotion has no concurrency lock. (medium, confirmed)**
`kelson-deployment` has no lock, no lease and no compare-and-swap on the release state ✓ — the only
mutexes in the crate are in the test fakes ✓ (`crates/enterprise/deployment/src/fakes.rs:23` ✓). Two
`kelson promote` runs can interleave ✓ and both can believe they are the current release ✓. The
atlas's Envoy row asks for exactly this ("a concurrency lock" ✓) and it is **not named** ✓. **Fix:** a
lock file + an owner token, or a CAS on the release revision ✓, plus a case that two concurrent
promotions produce one winner ✓.

**S-4 — The fakes can drift from the real adapters, and only two of them are guarded. (medium, confirmed)**
The atlas's LF-092 says: "never let fakes replace a mandatory **real adapter contract suite**" ✓. The
framework has that suite for the **cache** ✓ (`kelson_caching::cache_contract` ✓ — the in-memory and
the live Redis drivers run one suite ✓) and for the **model store** ✓ (`modelstore_conformance` ✓ — the
fake, pg and mysql ✓). The **queue**, **events**, **mail**, **notifications** and **storage** fakes
have **no** real-adapter contract ✓ ✗ — a fake can drift and no test notices ✓ ✗. That is the classic
"the test passes, production fails" ✓.

**S-5 — The PostgreSQL half of the P0 database claim is unproven on this host. (medium, environmental)**
The atlas's LF-089 is P0 and says: "A fake store **cannot** substantiate PostgreSQL production
behavior" ✓. The MySQL half runs against a live MySQL 8 ✓ (the conformance's 34 ✓), but the **pg** half
self-skips ✗ — the host's PostgreSQL dies with a native fault (`0xC0000142` ✗, diagnosed in the
sessions: not install integrity, not the data dir, not AppInit_DLLs, not IFEO ✗). So the **P0**
"real PostgreSQL integration" ✓ has never been executed here ✓ ✗.

**S-6 — Captured diagnostics can carry bound values. (low-medium, unproven)**
The audit records redact secrets ✓ (`config_show_renders_one_redacted_section` ✓,
`env_summary_reports_environment_secrets_and_cache_state` ✓) but the atlas's Telescope row asks for
"**query bindings privacy**" ✓ ✗ — a captured query's bound values can carry a patient's data ✓ and
nothing asserts they do not ✓.

**S-7 — The rate limiter's shared store, the CORS allowlist and the panic boundary are fixed** ✓
(F-3's three phases ✓, F-1 ✓, F-2 ✓, F-25 ✓) — listed here so the reader knows what is **not** a
gap ✓.

---

## 3. The bugs and the operational risks

**B-1 — The build host fills its disk, and that breaks the build silently. (high, observed twice)**
The workspace's `target/` is ~9 GB ✓ and the host hit **0 bytes free** twice during this work ✓ —
the second time it surfaced as an MSVC `LNK1318` PDB error ✓, i.e. a **link failure that looks like a
code error** ✓. A CI runner with a small disk would fail the same way ✓ and the failure message
points at the linker, not at the disk ✓. **Fix:** a documented disk floor, a smaller profile for CI,
and PDB off by default in CI (`RUSTFLAGS="-C debuginfo=0"` ✓).

**B-2 — The tree is not rustfmt-normalised: 1606 files would change. (low, but it blocks a gate)**
`cargo fmt --check` reports 1606 files ✓ → a `cargo fmt --check` CI step would fail every build ✓
(the atlas's Pint row wants exactly that gate ✓ ✓). The lint gate today is clippy + the env-var
lint ✓, which is *not* the formatting gate ✓.

**B-3 — The CI's coverage step was decorative. (fixed)** Both `|| true`s are gone ✓ (F-29 ✓) — the
install and the run must now succeed ✓. The **numeric threshold** is still open ✓: a ratchet needs a
measured baseline ✓ and `cargo-llvm-cov` is not installed on this host ✓.

**B-4 — The documented "wave spec" references point at a document that is not in the tree. (low, doc debt)**
The views batch's F-4 found a dead pointer ✓ and noted "**~20 further** `docs/19 L###` references
across the crates cite a *wave spec* document (`19-GAP-ANALYSIS-LARAVEL.md`) that is not in this tree"
✓. Every one of those is a citation into a void ✓ — the same class of defect, unfixed ✓.

**B-5 — The comparison documents in the tree are stale. (low, but it is what a buyer reads)**
`KELSON-VS-LARAVEL-COMPARISON.md` ✓ claims a missing read/write split ✓ (it shipped ✓) and factories
that do not exist ✓; `LARAVEL-PARITY-MATRIX.md` and `LARAVEL-FEATURE-ATLAS-DISPOSITION.md` ✓ are
untracked ✓. A competitive claim that is not maintained is worse than no claim ✓.

**B-6 — The e2e kit ships but never runs. (low-medium)**
The starter template's Playwright specs (`auth.spec.ts` ✓, `notes.spec.ts` ✓) are shipped ✓ and there is
**no browser and no CI job** for them ✓ (the atlas's LF-031 names this ✓). "User-visible end-to-end
tests" are the atlas's own requirement for LF-091 ✓ — so the kit that exists is unproven ✓.

**B-7 — The framework's own "real adapter" claims need care. (low, mostly corrected)**
The S3 side is an **in-memory sandbox** ✓ (the real signed-HTTP client does not exist ✓), and the
atlas row that called it a "driver" was corrected ✓ (F-30 ✓). The AI crates are **first-party** ✓
(four of them ✓) — the atlas's stale "EXTERNAL/ECOSYSTEM" disposition was corrected ✓ ✓.

---

## 4. The feature gap — the atlas, all 111 items

`P0-FEATURE-VERIFICATION.md` carries every row ✓ with its evidence ✓. The tally:
**40 VERIFIED · 52 PARTIALLY VERIFIED · 19 NOT IMPLEMENTED** ✓.

### 4.1 The P0/P1 gaps that cost the most

| # | Gap | Item | Why it matters | Effort |
|---|---|---|---|---|
| 1 | The **real PostgreSQL** integration never executed (S-5 ✓) | LF-089 | A P0 claim that a fake cannot substantiate ✓ — currently unproven on this host ✓ | 1d (needs a healthy PG) ✓ |
| 2 | The **fake-vs-real contract suites** for the queue/events/mail/notifications/storage (S-4 ✓) | LF-092/095 | Tests that prove nothing if the fake drifts ✓ | 3–5d ✓ |
| 3 | The **PAT abilities are not enforced** (S-1 ✓) | LF-053 | A security control that is decorative ✓ | 2d ✓ |
| 4 | The **promotion's concurrency lock** (S-3 ✓) | LF-105 | Two deploys can interleave ✓ | 2d ✓ |
| 5 | The **egress check's TOCTOU window** (S-2 ✓) | LF-073 | A rebind defeats the check ✓ | 2d ✓ |
| 6 | The **outbox's** `rollback-suppresses` / `commit-then-crash` cases ✗ | LF-064 | The durability boundary's two hardest cases are unnamed ✓ | 3d ✓ |
| 7 | The **random order** and the **repeated run** ✗ | LF-087 | A P0 testing requirement ✓ | 1d ✓ |
| 8 | The **RLS** assertion ✗ | LF-089 | A tenant-isolation claim without the database's own guard ✓ | 2d ✓ |
| 9 | The **teardown after failure** and the **resource caps** ✗ | LF-093 | The parallel-suite safety nets ✓ | 2d ✓ |
| 10 | The **interrupt** (Ctrl-C) and the **invalid-args** cases ✗ | LF-090 | A P1 CLI requirement ✓ | 1d ✓ |
| 11 | The **endpoint matrix** ✗ (no single case enumerates the auth endpoints) ✗ | LF-106 | The auth surface's completeness is unproven ✓ | 1d ✓ |
| 12 | The **rustfmt normalisation** (B-2 ✓) + the **coverage ratchet** (B-3 ✓) | LF-109/094 | The two CI gates Laravel has and Kelson does not ✓ | 2–3d ✓ |

### 4.2 The deliberate exclusions (fine — the atlas's own criteria bless them)

- **Facades** (LF-008 ✓) — "deliberate exclusion is reasonable for Rust ✓ ... exclude only the syntax,
  not the underlying service access or testability" ✓ — the typed injection + the fakes + the CLI
  cover the access and the testability ✓ ✓.
- **Collections** (LF-044 ✓) and **MongoDB** (LF-048 ✓) — "do not reproduce every PHP helper
  mechanically ✓" ✓ and "never score as core SQL ORM parity" ✓.
- **Livewire** (LF-032 ✓) — "exclusion is a product decision ✓" ✓; the blueprint's list now names it ✓
  (F-5 ✓).
- **Inertia/Folio/Head** (LF-033/034/035 ✓) — deferred behind the blueprint's v2.0 SSR gate (B-26 ✓) ✓.
- **Cashier ×2** (LF-103/104 ✓) — "do not place payment business logic in kernel" ✓ ✓.
- **Valet/Homestead/Mix/Boost** (LF-108/110/085 ✓) — dev tooling ✓ ✓.

### 4.3 Where Laravel has surface Kelson does not (and the atlas does not score)

These are **not** atlas items, so they do not appear in the 111 — but a buyer will ask ✓:

- **The dashboards**: Horizon (the queue's ops UI — Kelson has the *console* commands ✓ but no UI ✓),
  Pulse (the metrics dashboard — Kelson has the OTLP exporter ✓ but no UI ✓), Telescope (the request
  debugger — Kelson has the audit + the incident plane ✓ but no request debugger ✓), Nova (the admin
  app — Kelson has **no** admin UI at all ✗).
- **The full-stack DX**: Blade components + Blade's `@props`/slots ✓ (Kelson has slots ✓), the
  Vite/asset pipeline ✓ (the manifest half ✓), Breeze/Jetstream scaffolding ✓ (Kelson has the
  `make:module` ✓ but no auth-scaffold ✓), Laravel Prompts ✓ (Kelson has the minimal text/confirm ✓),
  Collision's pretty CLI errors ✓ (Kelson's error rendering ✓ is decent but not Collision-level ✓).
- **The debugging**: Ignition's error page with a stack trace and a "whoops" frame ✓ — Kelson's error
  envelope is redacted ✓ and machine-friendly ✓, and the starter template ships error views ✓
  (`404/500/maintenance` ✓), but there is no **interactive** error page ✗ (no frame, no "whoops"
  view, no request-context dump ✓).
- **The first-party breadth**: Scout (the search — Kelson has a search **port** ✓ but no engine
  adapter ✓), Socialite (OAuth — Kelson has OAuth2 contract crates ✓ but no Socialite-style
  provider set ✓), Passport (the OAuth server — Kelson has `kelson-oauth2-contract` ✓), Cashier,
  Dusk, Sail, Herd, Env.
- **The breadth of the helpers**: Laravel's `Str`/`Arr`/`Collection`/`Date`/`Number`/`Js`/`Vite`
  helpers are a large, tested surface ✓. Kelson's equivalent is Rust's std + the `inflect` module ✓
  (which covers the English pluralisation ✓) — the right answer for Rust ✓ ✓, but not a 1:1 surface ✓;
  the pieces Laravel ships as helpers, Kelson ships as **ports** ✓ (the HTTP client ✓, the process ✓,
  the realtime ✓, the mail ✓, the notifications ✓, the storage ✓) — which is the better architecture ✓
  but leaves the DX to the application ✓ ✗.

---

## 5. What it would take to be Laravel-competitive

**Tranche 0 — the enterprise floor (≈ 2 weeks).** Close S-1 (the PAT abilities), S-2 (the egress
pinning), S-3 (the promotion lock), S-4 (the fake-vs-real contract suites for the five remaining
fakes ✓) and the P0 PostgreSQL proof (S-5 ✓). Add the disk floor + the CI's formatting and coverage
gates (B-1 ✓, B-2 ✓, B-3 ✓). **Nothing here is a feature** — it is the work an enterprise reviewer
asks for ✓ ✓.

**Tranche 1 — the parity floor (≈ 4 weeks).** The remaining P0/P1 atlas gaps in §4.1 ✓: the outbox's
rollback/commit-then-crash cases ✓, the RLS assertion ✓, the random order + repeated run ✓, the
teardown + caps ✓, the CLI interrupt + invalid args ✓, the endpoint matrix ✓.

**Tranche 2 — the competitive layer (≈ 8–12 weeks).** Pick **one** of the two directions ✓:
(a) **the ops UI** — a queue dashboard (Horizon's role ✓), a metrics dashboard (Pulse's ✓) and a
request inspector (Telescope's ✓) over the telemetry Kelson already has ✓ — or (b) **the full-stack
DX** — the Blade-component parity ✓, the auth scaffolding ✓, the e2e runner ✓ and the Ignition-style
error page ✓. Doing both is a year of work ✓; doing neither keeps Kelson a backend framework ✓ ✗.

**Tranche 3 — the moat (continuous).** The parts where Kelson is already ahead and should be defended
✓✓: the typed graph (a route without a registered operation is refused at compile time ✓ — Laravel
resolves it at runtime ✓), the tenant scope (every store, queue, event and file is tenant-scoped by
construction ✓), the auditability (sealed envelopes ✓, digests ✓, the drift detector ✓), the no-`eval`
view engine ✓, and the conformance suites ✓ — the last is the *reason* a Kelson app can claim
PostgreSQL behaviour a Laravel app only claims on faith ✓ ✓.

### The measurable definition of done

1. Every P0/P1 atlas row is VERIFIED **or** carries a written, signed exclusion ✓ — today **all 19**
   NOT IMPLEMENTED rows are deliberate ✓ (the exclusions ✓, the deferrals ✓ and the by-design
   divergences ✓ that the atlas's own criteria bless ✓: the facades ✓, Livewire ✓, Inertia ✓, Folio ✓,
   the head manager ✓, the collections ✓, MongoDB ✓, Socialite ✓, the concurrency facade ✓, the
   images ✓, the helpers ✓, Boost ✓, Dusk ✓, Cashier ×2 ✓, Sail ✓, Valet ✓, Mix ✓). The real
   remaining work therefore lives in the **52 PARTIALLY VERIFIED** rows ✓ — in the named gaps inside
   them ✓ (the P0 PostgreSQL proof ✓, the RLS assertion ✓, the outbox's two cases ✓, the fake
   contracts ✓, the CLI interrupt ✓, the parallel-suite safety nets ✓, the endpoint matrix ✓, the
   end-to-end interpolation escaping ✓ …) ✓ ✓.
2. Every fake has a real-adapter contract suite ✓ (today: 2 of 7 ✓).
3. Every credential narrowing is enforced ✓ (today: the PAT abilities are not ✓).
4. The CI enforces the formatting ✓, the coverage ratchet ✓ and the live-database matrix ✓ (today:
   the matrix ✓ and the architecture gate ✓ are on ✓; the formatting and the ratchet are not ✓).
5. No P0 security finding is open ✓ (today: S-1 ✓, S-2 ✓, S-3 ✓ are open ✓).

---

## 6. The honest summary

**Where Kelson beats Laravel today:** the type system over the whole request path ✓ (a route that names
no operation is refused at compile time ✓), the tenant boundary as a construction property rather
than a convention ✓, the audit trail ✓, the views engine that cannot execute arbitrary code ✓, the
conformance suites that hold a fake to the same contract as the real adapter ✓ (two of them so far
✓), and the CI gates that actually fail the build ✓.

**Where Laravel beats Kelson today:** the ecosystem ✓, the operator dashboards ✓, the debugging
experience ✓, the breadth of the helpers and the packages ✓, the DX for a new developer ✓, and the
operational maturity that comes from a decade of production use ✓.

**The verdict:** Kelson is **not** Laravel-competitive as a whole today ✗, and no amount of
foundational work will make it so ✓ — Laravel's lead is in the ecosystem and the DX ✓, not in the
framework's correctness ✓. It is competitive **as a type-safe, multi-tenant, audit-grade backend** ✓,
where its verification discipline is genuinely ahead ✓✓. Tranche 0 (the three security findings ✓ +
the fake contracts ✓) is what turns "a promising framework" into "one an enterprise can adopt" ✓✓ —
and that is the cheapest, highest-leverage work remaining ✓ ✓.
