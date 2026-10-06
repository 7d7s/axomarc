# ADR 0007 — JSON:API spec server (compound documents over Wave 3 resources)

Status: **Accepted** (2026-10-02) · Gate: Wave 8 decision gate (docs/19, "Decision gates") ·
Implementation: landed 2026-10-02, the day the gate passed review — serializer in
`crates/web/http/src/jsonapi.rs` (`negotiate`/`split`/`render_document`), consulted by
`render_success`; the 406 refusal is `AppError::not_acceptable()` (`KER_NOT_ACCEPTABLE`)
with its wire status pinned in both keel-api error-render registries.
Implementation-reconciliation notes are folded into the sections below.

## Context

Wave 3 shipped the resource surface: `keel make resource` emits a compiling
`keel_http::Resource` impl, resource routes are registry-validated (`RouteSpec.resource`,
`GRAPH-RESOURCE-OP`), and rendering wraps the body under `Resource::wrap()` (default
`DEFAULT_WRAP = "data"`). Laravel ships a first-class JSON:API server (json-api rendering
classes); the platform's compound-document server — sparse fieldsets, includes, links — is
the Wave 8 decision gate (docs/19 Wave 3 + Decision gates; docs/18 parity row).

Constraints the decision must honor:

1. **Budget-neutral, serialization only** — no engine, registry, config, or manifest
   changes; `MODULE_ABI` stays 1; zero new dependencies (serde_json is already everywhere).
2. **Byte-identical default path** — a client that does not ask for JSON:API must get
   exactly today's bytes (no existing test may change).
3. Decision-complete: exact activation, mapping, and refusal rules — the implementer makes
   no design choices.

## Decision

### Activation (content negotiation)

- A resource route (its `RouteSpec` declares `resource = "<name>"` — at the dispatch edge
  these are the expanded CRUD routes carrying `from_resource`, each named
  `{basename}.{op}` where the basename is that declared resource name; that basename is
  the document `type`, carried on `RouteDispatch.resource_type` so no other route can
  negotiate) answers with a
  JSON:API 1.1 document **when the request's `Accept` header contains the media type
  `application/vnd.api+json`**.
- `Accept` is parsed as a comma list with `q` weights (case-insensitive essence match).
  An entry for the media type with `q=0`, or its absence, means not acceptable → the
  current renderer runs (byte-identical).
- **Any media-type parameter** on the matched entry (`;...`, e.g. `ext[]`) → `406` with
  the standard platform envelope (JSON:API requires 406 for unsupported parameters —
  a documented divergence from JSON:API error DOCUMENTS, see below).
- The success response sets `Content-Type: application/vnd.api+json`.
- Routes that do not declare `resource`, and every error response, ignore the header and
  render exactly as today (documented: negotiation is defined only for resource routes).

### Resource mapping spec

Input is the `Resource::from_source` body (item object or array) — the Laravel-style
`wrap()` step is replaced by the JSON:API document, not applied on top of it.

*Implementation reconciliation:* handlers return that body through the existing wrap, so
`jsonapi::split` reads the wrap back off — the payload is the `data` slot when present
(item or array); otherwise the remaining members are the body and custom wrap keys read
as attributes (documented collision: a body with its own `data` member must use the
default wrap contract). Top-level `meta` (non-empty object) and the reserved `included`
array are lifted off as siblings in either shape. A scalar body fails closed to
`data: null`, and a non-object entry in a collection is dropped rather than emit a
spec-invalid document.

For one resource object:

| JSON:API member | Source |
|---|---|
| `type` | the route's declared `resource` name (registry-validated, stable across renames) |
| `id` | lifted from the body's `id` member when it is a scalar (string kept verbatim, other scalars rendered as their canonical string); absent/non-scalar → `"id": null` |
| `attributes` | the body minus the reserved members `id`, `type`, `relationships`, `included` |
| `relationships` | body's reserved `relationships` member, shape-checked: only members whose `data` is `{type, id}` or an array thereof survive; an emptied object is omitted |
| `links.self` | the request URI, verbatim (`path?query` — the earlier `PlatformGraph::resolve_url`-when-named note is superseded: that resolver needs extracted path parameters the pure serializer never sees) |

- **Collections**: a body that is an array becomes `data: [ ... ]` with order preserved;
  each entry maps by the same rules.
- **Includes**: the body's reserved `included` array (entries are resource-shaped bodies
  carrying their own `type` + `id`) is lifted to the document's `included`; an entry
  without a usable `type` is dropped (fail-closed — never emit a spec-invalid document),
  `id`/`type` are lifted exactly as above and stripped from its attributes, no `self`
  links are generated for included entries. Deduplicated by `(type, id)`, first wins;
  an empty result omits `included`.
- **Sparse fieldsets**: `fields[<type>]=a,b` query parameters filter, per occurring type
  (root and included), which `attributes` members survive — `id` and `type` are always
  present, and `relationships` members are filtered by the same list. A
  `fields[<type>]` naming a type absent from the document is ignored (documented).
  Field names are simple identifiers (`[a-z0-9_.-]`), so the platform's
  parse-without-decode query posture cannot corrupt them.
- **Pagination + links**: top-level `meta` carries the operation's existing meta verbatim
  (page envelope, counts). When it carries `page`/`per_page`/`total`, document
  `links.first/prev/next/last` are derived by rewriting those query parameters on the
  request URI; `prev`/`next` omit at the bounds, `total` unknown → `first`/`self` only.
  An empty `meta` is omitted entirely.
- **Errors**: every failure keeps the platform envelope (`{error: ..., ok: false}` /
  RFC-7807 option) on every status — JSON:API error objects are explicitly out of scope
  (recorded divergence; the gate bullet scopes the server to sparse fieldsets, includes
  and links).
- **Out of scope, recorded**: `filter[...]`/`sort[...]` query conventions, `meta` global
  extensions, `Accept` versioning parameters, and pagination `cursors` — future gates if
  parity demands them.

### Implementation locus

Serializer in `crates/web/http/src/jsonapi.rs` (pure: resource name + split payload/meta/
included + query + request URI → `serde_json::Value`), consulted by `render_success` when
negotiation matched on a `from_resource` route (`RouteDispatch.resource_type` carries the
derived type name). The 406 refusal is `AppError::not_acceptable()` — code
`KER_NOT_ACCEPTABLE`, Validation category (non-reportable client mistake) — with the
wire status pinned to HTTP 406 by `ErrorRenderRegistry::set_status` at BOTH keel-api
state constructors (production boot and the test harness); the registry itself stays
empty until a module registers, per its contract. Deprecated routes merge
`deprecated: true` into the document meta (envelope parity with `meta.deprecated`), and
the response still flows through the shared `Deprecation` header + caching pass. No
engine, registry, config, or manifest surface moves.

## Consequences

- Zero new dependencies (budget untouched); `MODULE_ABI` stays 1 (no manifest fields).
- Default wire bytes are unchanged, so the existing suite count is unaffected; the
  evidence matrix landed in EXISTING suites (keel-http unit matrix: item/collection,
  id+type lift, reserved-member stripping, relationships shape check, include dedupe +
  bad-entry drop, sparse fieldsets, links at bounds, parameter-reject 406; keel-api:
  negotiation roundtrip on a resource route) as 13 `jsonapi` unit tests plus 5
  `response_layer_integration` HTTP tests — including a byte-for-byte default-path
  assertion against `Envelope::ok` output for absent / `q=0` / plain-JSON `Accept` —
  suite count stays 150.
- Resources gain a second stable wire profile without a trait change: mapping reads only
  the body value and the route declaration, so generated `Resource` impls need no edits.
- The envelope/error posture stays platform-native even under negotiation — a JSON:API
  client handles errors via HTTP status + the documented envelope (recorded divergence).
