# AGENT.md — Working Boundary & Isolation Layer

> This file defines the **hard scope boundary** for any AI agent (or human) working in
> this repository. Read it before touching a single file.

Repo root: `medicare-complete-hospital-clinic-appointment-system-with-admin-web-user-doctor-apps/`

---

## 1. The working area (only these paths may be created, edited, or deleted)

```
platform/**    ← the Keel framework (the ONLY code area in scope)
docs/**        ← domain + Rust-track documentation
runbooks/**    ← operational runbooks
```

Everything else in this repository is **out of scope**.

## 2. Out-of-scope paths (leave untouched)

| Path | Status | Rule |
|---|---|---|
| `medicare-kernel/**` | **Superseded** (decision D-46, 2026-08-20) — retained for reference, "receives no new work" | ❌ No edits. Do not "fix", format, migrate, or delete. |
| `.github/**` | CI/CD workflows (`keel.yml`, `keel-publish.yml`) | ❌ No edits unless explicitly asked for in the task. |
| Any file at repo root other than `AGENT.md` | — | ❌ No edits unless explicitly asked for in the task. |
| Anything **outside** this repo root | e.g. sibling folders in `C:\Users\Victo\Downloads\webproj\` (`Cloud/`, `axomarc/`, `agrowok-erp/`, `medlattice/`, …) and the parent git repo | ❌ Never read, write, build, or run commands there. Different projects. |

### Read-only exception
Reading `medicare-kernel/**` for historical context is allowed (it documents the M0
scaffold that `platform/` replaced). **Reading is not permission to modify.**

---

## 3. Operating rules inside the working area

1. **Scope every change.** Before editing, confirm the target path starts with
   `platform/`, `docs/`, or `runbooks/`. If it does not → stop and ask.
2. **`platform/` is the code boundary.** All Rust work (crates, adapters, runtime,
   tools, tests, examples, applications) happens under `platform/` and is built from
   there:
   ```sh
   cd platform
   cargo check --workspace
   cargo test --workspace --locked
   cargo run -p xtask -- dep-rules        # architecture gate
   cargo clippy --workspace -- -D warnings
   ```
   Never run cargo from the repo root (there is no workspace there).
3. **`docs/` and `runbooks/` are documentation only.** Do not put code, configs, or
   build artifacts there.
4. **Keep the boundary clean.** No new top-level folders. New framework code goes in
   an existing `platform/` subtree (`crates/`, `adapters/`, `runtime/`, `tools/`,
   `applications/`); new domain modules are scaffolded with
   `cargo run -p keel-cli -- make:module <name>` inside `platform/applications/`.
5. **Do not cross-contaminate.** Never copy files between `medicare-kernel/` and
   `platform/`, never add `medicare-kernel` as a path dependency, and never "fix"
   the old scaffold so it compiles with newer toolchains.
6. **Do not touch the parent git repo's other projects.** This folder is currently
   untracked inside `C:\Users\Victo\Downloads\webproj` — scope any `git` command to
   this directory and never commit paths belonging to sibling projects.
7. **Respect the frozen foundation.** `platform/specs/STAGE-0-FOUNDATION-SPEC.md`,
   `docs/rust/11-FOUNDATION-MASTER-SPEC.md`, and `docs/rust/13-FRAMEWORK-BLUEPRINT.md`
   are frozen; architecture changes require an ADR in `platform/specs/ADR/` first.
8. **Ask before expanding scope.** If a task genuinely requires touching
   `medicare-kernel/`, `.github/`, or anything outside the repo, stop and get
   explicit confirmation.

---

## 4. Pre-flight checklist (run before finishing any task)

```sh
# from repo root — every changed/new path must be inside platform/, docs/, runbooks/
git status --short
```

Confirm **none** of the changed paths match:

- `medicare-kernel/`
- `.github/`
- any absolute path outside `medicare-complete-hospital-clinic-.../`
- any sibling project under `C:\Users\Victo\Downloads\webproj\`

Then confirm the build gates for `platform/` still pass:

```sh
cd platform && cargo check --workspace && cargo test --workspace --locked
```

---

## 5. Quick orientation

| Question | Answer |
|---|---|
| Where is the framework? | `platform/` (70-crate workspace, `Cargo.toml`) |
| Where is the runnable API? | `platform/runtime/keel-api` → `/healthz`, `/readyz`, `/api/v1/demo/echo` |
| Where are domain modules? | `platform/applications/` (currently only `appointments`) |
| Where are the rules? | `platform/justfile`, `platform/README.md`, `docs/rust/11-FOUNDATION-MASTER-SPEC.md` |
| What is frozen/archived? | `medicare-kernel/` (superseded, D-46) |
| What is docs-only? | `docs/`, `runbooks/` |
