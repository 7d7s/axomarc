# __PROJECT_NAME__

SPA-first starter module (S9). __PROJECT_DESCRIPTION__

## Web story (recorded decision)

Keel v1 is **API-first / SPA-first**: the framework serves JSON envelopes over
the graph-driven HTTP edge and emits an OpenAPI document (`keel sdk:openapi`).
Browser UIs are SPAs served by their own toolchain (vite/next/plain static);
SSR/template engines are deliberately deferred to v2.0 with a fresh
dependency-budget decision. The `web/` folder here is a zero-build vanilla
shell you can serve from any static host pointed at the same origin.

## Auth pattern

- **Browser (same-origin):** cookie session. POST `/auth/register` then
  `/auth/login` with `mode = "session"`; the `keel_session` cookie is
  HttpOnly + SameSite and carries every subsequent request.
- **API clients:** hashed PAT. Use `mode = "token"` on login and send
  `Authorization: Bearer <token>`. Tokens are revocable (`/auth/logout`).
- JWT is intentionally not the default (future equivalent add-on only).

## Run it

```bash
keel serve                 # boots keel-api (+ --worker if needed)
keel sdk:openapi out.json  # generate the OpenAPI client contract
keel route:list            # named routes: notes.create / notes.list
```

## Persist notes to PostgreSQL

The module ships on the in-memory `ModelStore` port (degraded boot). To make
notes durable:

1. `keel make:migration create___PROJECT_NAME___notes` and mirror
   `note_model()` from `src/lib.rs` (id uuid PK, tenant_id uuid NOT NULL
   REFERENCES tenants(id), body text NOT NULL, created_at/updated_at
   timestamptz NOT NULL DEFAULT now(), deleted_at timestamptz; index
   tenant_id).
2. `keel migrate`, then boot with a reachable `DATABASE_URL` — the Platform
   phase swaps in `PgModelStore` automatically.

## Layout

- `module.toml` — contract: 2 capabilities, 2 named routes, 1 event.
- `src/lib.rs` — handlers over `ModelStore` + `ModuleProvider`.
- `web/` — SPA shell (auth + notes CRUD against the module routes).