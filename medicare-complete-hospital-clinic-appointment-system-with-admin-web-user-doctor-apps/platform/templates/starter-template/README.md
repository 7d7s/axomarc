# __PROJECT_NAME__

SPA-first starter module (S9). __PROJECT_DESCRIPTION__

## Web story (recorded decision)

Kelson v1 is **API-first / SPA-first**: the framework serves JSON envelopes over
the graph-driven HTTP edge and emits an OpenAPI document (`kelson sdk:openapi`).
Browser UIs are SPAs served by their own toolchain (vite/next/plain static);
SSR/template engines are deliberately deferred to v2.0 with a fresh
dependency-budget decision. The `web/` folder here is a zero-build vanilla
shell you can serve from any static host pointed at the same origin.

## Auth pattern

- **Browser (same-origin):** cookie session. POST `/auth/register` then
  `/auth/login` with `mode = "session"`; the `kelson_session` cookie is
  HttpOnly + SameSite and carries every subsequent request.
- **API clients:** hashed PAT. Use `mode = "token"` on login and send
  `Authorization: Bearer <token>`. Tokens are revocable (`/auth/logout`).
- JWT is intentionally not the default (future equivalent add-on only).

## Run it

```bash
kelson pkg:add .             # register this module in packages.toml (step 1)
kelson graph                 # 1 module, 5 operations, 3 events
kelson route:list            # named routes: notes.create / notes.list / ...
kelson sdk:openapi --output out.json
```

`kelson serve` is deliberately not in that list. It boots the `kelson-demo`
host, which is a member of a Kelson workspace: it works from inside a Kelson
checkout, but this project is its own `[workspace]` and does not depend on
`kelson-demo`, so `serve` here exits 101 with
`` package(s) `kelson-demo` not found in workspace ``. The three commands
above all work standalone.

`pkg:add` is the step that makes the module visible: `graph`, `route:list`,
`check`, `boot` and `sdk:openapi` all merge the `packages.toml` index, so
before it runs the platform reports zero modules and the OpenAPI document
comes back with no paths.

## Persist notes to PostgreSQL

The module ships on the in-memory `ModelStore` port (degraded boot). To make
notes durable:

1. `kelson make:migration create___PROJECT_NAME___notes` and mirror
   `note_model()` from `src/lib.rs` (id uuid PK, tenant_id uuid NOT NULL
   REFERENCES tenants(id), body text NOT NULL, created_at/updated_at
   timestamptz NOT NULL DEFAULT now(), deleted_at timestamptz; index
   tenant_id).
2. `kelson migrate`, then boot with a reachable `DATABASE_URL` — the Platform
   phase swaps in `PgModelStore` automatically.

## Layout

- `module.toml` - contract: 5 capabilities, 5 named routes, 3 events.
- `src/lib.rs` — handlers over `ModelStore` + `ModuleProvider`.
- `web/` — SPA shell (auth + notes CRUD against the module routes).