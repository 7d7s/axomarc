# ADR 0007 — JSON:API spec server (compound documents over Wave 3 resources)

Status: **Proposed** (2026-10-02) · Gate: Wave 8 decision gate (docs/19, "Decision gates") ·
Implementation: only after the gate passes review

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

- A resource route (its `RouteSpec` declares `resource = "<name>"`) answers with a
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

For one resource object:

| JSON:API member | Source |
|---|---|
| `type` | the route's declared `resource` name (registry-validated, stable across renames) |
| `id` | lifted from the body's `id` member when it is a scalar (string kept verbatim, other scalars rendered as their canonical string); absent/non-scalar → `"id": null` |
| `attributes` | the body minus the reserved members `id`, `type`, `relationships`, `included` |
| `relationships` | body's reserved `relationships` member, shape-checked: only members whose `data` is `{type, id}` or an array thereof survive; an emptied object is omitted |
| `links.self` | the request URI (named routes: `PlatformGraph::resolve_url` when the route has a name) |

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

Serializer in `crates/web/http/src/jsonapi.rs` (pure: resource name + body + meta +
query + request URI → `serde_json::Value`), consulted by `render_success` when negotiation
matched on a `from_resource` route. No engine, registry, config, or manifest surface
moves.

## Consequences

- Zero new dependencies (budget untouched); `MODULE_ABI` stays 1 (no manifest fields).
- Default wire bytes are unchanged, so the existing suite count is unaffected; the
  evidence matrix lands in EXISTING suites (keel-http unit matrix: item/collection,
  id+type lift, reserved-member stripping, relationships shape check, include dedupe +
  bad-entry drop, sparse fieldsets, links at bounds, parameter-reject 406; keel-api:
  negotiation roundtrip on a resource route) — suite count stays 150.
- Resources gain a second stable wire profile without a trait change: mapping reads only
  the body value and the route declaration, so generated `Resource` impls need no edits.
- The envelope/error posture stays platform-native even under negotiation — a JSON:API
  client handles errors via HTTP status + the documented envelope (recorded divergence).
