> **Superseded (D-46, 2026-08-20):** this M0 scaffold is retained for reference only and receives no new work. The authoritative foundation is the **Keel platform** at `../platform/` — spec `../docs/rust/11-FOUNDATION-MASTER-SPEC.md`, decisions D-44…D-53. This tree stays compilable and will re-emerge as `applications/reference-healthcare` on top of Keel.

# medicare-kernel

Rust backend for Medicare — the ERP Execution Kernel. Design set: `../docs/rust/`
(PRD/SRS/design/data-model/contracts/security/realtime/execution-plan/decisions/guidelines).

## Shape (D-43: Laravel-familiar layout)

```
apps/api          ← bootstrap/app.php — the only binary
config/           ← config/*.php
database/         ← seeders + init roles
migrations/       ← database/migrations (ordered stream)
kernel/           ← the framework we own (core, runtime, db, delta, auth, capability, workflow, notify)
modules/<name>/   ← business modules: lib.rs + routes + http/{controllers,requests,resources}
                    + services/ + models/ + repositories/ + events/ + workflows/ + policies.rs
xtask/            ← artisan
```

## Quickstart

```sh
docker compose up -d          # postgres 16 + minio
cp .env.example .env
cargo run -p medicare-api     # boots, migrates, seeds demo tenant, serves :8080
curl localhost:8080/healthz   # ok
curl localhost:8080/readyz    # ready (if DB reachable)
```

Booking reference slice (real, end-to-end with delta in same tx):

```sh
# seed a slot (dev), then:
curl -X POST localhost:8080/api/v1/appointments \
  -H 'content-type: application/json' \
  -d '{"slot_id":"<uuid>","patient_ref":{"kind":"self"}}'
# → 201 Envelope{data:{status:"SCHEDULED", allowed_events:[...], version:1}}
```

## Status vs plan

- M0 scaffold: **done** (this tree) — workspace, kernel crates, 11 modules, migrations, compose, CI
- M1–M8: task rows in `../docs/rust/07-EXECUTION-PLAN.md`; module scaffolds return 501 until wired
- Decisions: `../docs/rust/08-DECISIONS.md` (D-01…D-43)

## Conventions

- Money = integer minor units (`MoneyCents`); secrets/PHI in `Secret<T>`/`Phi<T>` (cannot log)
- Handlers ≤ ~15 lines; logic in services; decisions in models; SQL only in repositories
- Mutations go through `Tx` and MUST emit a delta (KER-03 enforced at commit)
- Clippy `-D warnings` in CI (D-34)