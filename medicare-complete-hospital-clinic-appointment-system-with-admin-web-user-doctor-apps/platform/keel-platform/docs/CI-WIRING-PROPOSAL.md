# CI wiring — the exact patch, and how to prove it locally

`.github/workflows/keel.yml` is **frozen** (the program's no-touch constraint), so this document
carries the patch instead of applying it. Every step below is **already proven to work on this
machine** — `tools/ci-parity-check.ps1` runs the same commands locally, against the live MySQL.

Unfreezing `keel.yml` is then a mechanical paste of the four hunks below.

---

## Row 4 — the shared conformance target in the mysql matrix

The `platform-mysql` job runs the **integration** target only:

```yaml
      - name: mysql driver matrix (pg half self-skips on mysql://)
        run: cargo test -p kelson-postgres --features pg-tests --test pg_integration --locked
```

The **conformance** target (`modelstore_conformance.rs` — the shared `ModelStore` contract the
in-memory fake, pg and mysql all run) is **not** in CI. That is how the conformance suite could sit
**red against the live schema** for a whole session (it violated the schema's double-booking guard)
without CI noticing.

**Hunk 1** — add one step to the `platform-mysql` job, right after the driver matrix:

```yaml
      - name: shared modelstore conformance (the contract every backend runs)
        run: cargo test -p kelson-postgres --features pg-tests --test modelstore_conformance --locked
```

*Proven locally*: 34 passed / 0 failed against MySQL 8 (17 conformance cases × the pg + mysql halves;
each half self-skips on the other fabric's scheme).

---

## Row 11 — the semver gate

`cargo xtask semver` exists and runs, but **no CI job calls it**. It is deliberately a no-op until
the repository has a release baseline:

```
semver: no kelson/platform-v* release tag yet — no baseline to compare (skipping gate)
```

**Hunk 2** — add a step to the `platform` job (after the architecture analyzer):

```yaml
      - name: semver gate (breaking-change check)
        run: |
          cargo install cargo-semver-checks --locked
          cargo run -p xtask -- semver
```

*Proven locally*: the gate runs and reports the skip reason above. It becomes a real gate at the
first `kelson/platform-v*` tag; until then it is an honest, documented no-op. The tool install is
in the step because the xtask probes for it and skips when it is absent (`cargo semver-checks
--version` — a missing subcommand still exits 0, so the probe is required).

---

## Row 12 — the scaffold build

`kelson make:module` writes a module into `applications/<name>`, adds the workspace member and
prints `cargo check` as the next step. The generated crate **is not built by CI**, so a broken
template ships silently — and it did: the template's `authorize` carried the pre-`AuthzRequest`
signature, so **every** scaffolded module failed to compile.

**Hunk 3** — add a job (a throwaway checkout needs no cleanup):

```yaml
  scaffold-build:
    name: scaffold build (a generated module must compile)
    runs-on: ubuntu-latest
    defaults:
      run:
        working-directory: platform/keel-platform
    steps:
      - uses: actions/checkout@v4
      - name: Install Rust 1.92.0
        run: |
          rustup toolchain install 1.92.0 --profile minimal
          rustup default 1.92.0
      - name: Cache cargo
        uses: actions/cache@v4
        with:
          path: |
            ~/.cargo/registry
            ~/.cargo/git
            platform/keel-platform/target
          key: keel-scaffold-${{ hashFiles('platform/keel-platform/Cargo.lock') }}
          restore-keys: |
            keel-scaffold-
      - name: scaffold a module and build it
        run: |
          cargo run -p kelson-console -- make:module ci-probe --description "CI scaffold probe"
          cargo check -p kelson-mod-ci-probe --all-targets
          cargo test -p kelson-mod-ci-probe --locked
```

*Proven locally*: `kelson make:module scaffoldprobe` → `cargo check -p
kelson-mod-scaffoldprobe --all-targets` is clean and the scaffolded crate's own test
(`tests::ping_executes_through_engine`) passes. The probe crate and its workspace-member entry were
removed afterwards (the root `Cargo.toml`/`Cargo.lock` restored byte-for-byte).

A **local** run needs that cleanup, because the workspace's own `Cargo.toml` is edited. In CI the
checkout is disposable, so the job needs none.

**The regression guard already exists** (it is not a CI step — it is a unit test):
`kelson-codegen`'s `the_generated_authorize_matches_the_current_port_signature` fails if the
template's `authorize` drifts from the trait again (verified by re-injecting the stale signature:
the guard goes red).

---

## Row 10 — the contracts-crate registry tests

**No change needed.** The `platform` job already runs `cargo test --workspace --locked`, and
`kelson-contracts` is a workspace member, so the api-registry tests already run in CI.

*Proven locally*: `cargo test -p kelson-contracts --lib` → **3 passed / 0 failed**
(`registry_is_valid`, `deprecated_entries_carry_migration_data`, `invalid_entries_is_rejected`). The
gap was only that the LF-007 pass did not execute them locally; they are executed now.

---

## The local mirror

`tools/ci-parity-check.ps1` runs the DB-backed and tool steps of the hunks above against a live
MySQL, so the proposal can be proven **without** unfreezing `keel.yml`:

```powershell
# from platform/keel-platform
pwsh tools/ci-parity-check.ps1 -DatabaseUrl 'mysql://keel:keel@127.0.0.1:3307/keel'
```

It runs: the conformance target (hunk 1), the contracts registry (row 10) and the semver gate
(hunk 2). The scaffold build (hunk 3) is a two-line manual sequence — it edits the workspace
`Cargo.toml`, so it is run in a throwaway checkout (as CI does) rather than in a working tree.
