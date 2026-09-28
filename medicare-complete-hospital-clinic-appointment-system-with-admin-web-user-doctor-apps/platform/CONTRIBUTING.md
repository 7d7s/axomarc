# Contributing

Thanks for helping build Keel. Read `ARCHITECTURE.md` and GOVERNANCE.md first.

## Workflow
1. Pick or propose work aligned with the current milestone (`docs/rust/07-EXECUTION-PLAN.md`).
2. Branch `codex/<topic>` or `<you>/<topic>`.
3. Keep changes within one topology class where possible.
4. `cargo fmt --all`, then `just check` and `just test` before pushing.
5. New operations require: ExecutionPlan, capability entry in the module's
   `module.toml`, delta/audit expectations, and at least one engine-level test.

## Rules
- Do not add external dependencies without checking DEPENDENCY-POLICY.md.
- Do not bypass the dependency matrix; if the matrix is wrong, change the spec
  first (GOVERNANCE.md → Spec change).
- No PHI/PII in logs, error meta, or fixtures.

## Formatting
`cargo fmt` is canonical; CI enforces it.

## Commit messages (Conventional Commits)

Release notes are generated from commit messages by git-cliff (`cliff.toml`),
so every commit that should appear in the changelog must use this format:

```
type(scope): summary
```

- `type`: `feat` | `fix` | `perf` | `refactor` | `docs` | `style` | `test` | `build` | `ci` | `chore`
- `scope`: free-form, usually a crate name (`keel-foundation`, `analyzer`, `ci`, `release`)
- Mark breaking changes with `!` after the type (e.g. `feat(http)!: ...`) and
  follow `DEPRECATION.md` + `API-STABILITY.md` for the migration guidance.

`chore(release)` commits (changelog regeneration) are excluded automatically.
