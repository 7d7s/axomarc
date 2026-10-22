# ADR 0021 — MVVC: the Model, the ViewModel and the View as three framework-provided layers

Status: **Accepted** (2026-10-22) — Gate: the MVVC plan (`docs/MVVC-ARCHITECTURE-PLAN.md`) — Implementation: S-0/S-1 (the `ViewModel` trait lands with this record).

## Context

The architecture table in `docs/rust/10-STRUCTURE-AND-GUIDELINES.md` says, of MVVM:

> **Client-side pattern, mapped to backend separation** — "MVVM as a backend architecture — **no**; there is no 'view' server-side beyond the wire contract."

Kelson has been disagreeing with that sentence in practice for a long time. `kelson-http` ships `RequestViewModel` and `ResponseViewModel` (`crates/web/http/src/lib.rs:194-204`) and the `Validated<M>` / `TypedResource<R>` adapter (`crates/web/http/src/viewmodel.rs:27,48`) — that is the wire plane's MVVM, and it is not "no". The view engine (Wave 2) is a server-side "view" that renders templates, and the `NotificationViewRenderer` port (`crates/services/notifications/src/lib.rs:509`) proves the view layer already reaches outside HTTP. The table records a half-truth.

What the framework genuinely lacks is the part that makes the pattern worth its name on the server: **a typed presentation state between the domain result and the template**. Today the template context is an untyped `serde_json::Value` (`crates/web/views/src/engine.rs:423`), so the view's data requirements are implicit in string keys, and a typo is discovered by a human reading HTML. The reserved context keys are assembled in three separate places — the success path (`runtime/kelson-demo/src/graph_routes.rs:1241-1263`), the error path (`:1121-1146`) and the notification path (`crates/services/notifications/src/lib.rs:1031-1051`) — and `_csrf` is supported by the engine (`crates/web/views/src/render.rs:357`) but never injected, so `@csrf` renders nothing. The ops dashboard bypasses the edge entirely (`runtime/kelson-demo/src/dashboard.rs:202-210`) and therefore carries no `_locale` and no `_lang`. There is no head/metadata model (LF-035 is recorded as NOT IMPLEMENTED), no read-model layer (`docs/APP-AUTHORING.md:257-258`), no form objects on the HTML plane, and no stateful view loop (LF-032 is excluded by design). `kelson_views` is not re-exported by `kelson-framework` (`crates/framework/src/lib.rs`), so an application cannot reach the view layer through the framework facade.

The pattern itself (MVVM, from Fowler's *Presentation Model*) says the ViewModel is a value converter that exposes the model's data in a form the view can present, that the ViewModel knows the Model and never knows the View, and that the **binder** connects them. Two of those three transfer to a server-rendered framework unchanged. The third needs an honest translation: there is no client-side synchronisation problem on the server, so the binder's job here is not to keep two copies of the state in step — it is to make a template's reads **checkable**, so a path the ViewModel does not declare is a render error rather than an empty string. The "commands" of the desktop pattern translate as bindings to **named operations**, which the framework already dispatches (`crates/execution/engine/src/engine.rs:20-34`); there is no client-side event bus, because the server is the source of truth.

The server-side precedents agree: Fowler's Presentation Model is "a fully self-contained class that represents all the data and behavior of the UI window, but without any of the controls used to render that UI"; AppFolio's view model is "the single object of interaction for the view… responsible for implementing the interface (data requirements) of a particular view template or partial", with form objects as the request-side counterpart; GitLab's presenters "must not know the view context" and "must not generate complex DOM elements"; LiveView's presentation models exist so the view logic is testable without the socket. The requirement the precedents all carry, and this framework's version lacks, is a **declared interface** for what a view may read.

Constraints:

1. **No breaking change for an application.** Every existing template, route, view and application keeps working. New syntax is additive; an unbound view still renders.
2. **No new dependency edge that breaks the matrix.** `kelson-views` keeps its zero-dependency posture (`serde`, `serde_json`, `sha2`, `toml`); the shared context assembler lives in `crates/foundation/core`, because the notifications service crate may not depend on the web layer; the read model lives in the data layer.
3. **Every feature is proven by an executed, named case** — the same rule the rest of the repository obeys. A feature that cannot be shown to fail when removed is not in the plan.
4. **The framework stays a framework.** The framework provides the three layers' contracts, the built-in view models and the machinery; the application provides its own pages.

## Decision

Kelson adopts MVVC as a **server-side presentation architecture**: the Model, the ViewModel and the View are three layers the framework provides.

1. **The ViewModel** (`kelson-views`): a typed, serialisable struct that declares the view it renders into (`const VIEW`) and the view's **data requirements** (`fn schema() -> &'static [FieldSchema]`), with a JSON escape hatch for the rare dynamic field. It is a **value, not a session**: built per request, serialised, discarded. A view that wants live state reads a read model, not a mutable object in the request path.
2. **The View** (`kelson-views`): a template plus a **registration** — the `ViewRegistry` binds a view name to the ViewModel it declares. The framework registers the built-in views; the application registers its own. A view whose file exists without a binding still renders, unbound — today's behaviour, unchanged.
3. **The Model** (the existing execution and data layers, plus the new **read-model layer**): operations and models as they are, and, for the read path, a named, declared, materialised projection with a refresh policy and a staleness surface. A read model is a **projection of the existing store**, never a second source of truth, and its refresh is tenant-scoped by the same rule every other write obeys.
4. **The binder** (new template syntax, additive): `@vm` binds the view's registered ViewModel into the `vm` namespace and turns on **schema-checked paths** — every `vm.*` path a template reads is validated against the model's declared schema at render time, and an unknown path or a type mismatch is a render error, fail-closed like the rest of the engine. `@command("op")` binds a template element to a **named operation** the route declares; a command the route does not declare is refused.
5. **The ViewContext** (`crates/foundation/core`): the reserved keys — `_errors`, `_locale`, `_lang`, `_auth`, `_csrf` — are assembled **once**, by the framework, for every path that renders a view: the success path, the error path, the notification path and the dashboard. `_csrf` becomes actually wired; the dashboard becomes internationalised; the three copies of the injection logic collapse into one.
6. **The companions**, all in scope: the head metadata store (LF-035) with request-scoped isolation; typed form objects; the read-model layer; and live/stateful views, which reuse the same ViewModel spine over `kelson-realtime` and are opt-in per view.
7. **The built-in view models**: the framework ships them — pagination, table, form, error page, empty state, ops dashboard — so an application does not re-invent the common ones.

Non-goals, stated so they are not smuggled in later: there is no client-side data binding and no client-side state machine; the server remains the source of truth; a ViewModel produces **data, never markup**; the read model does not become a second write path.

**This record supersedes the MVVM row of the architecture table** (§1 of `docs/rust/10-STRUCTURE-AND-GUIDELINES.md`), which is amended from "no" to "adopted, as the server-side presentation layer — see ADR 0021". The MVC row ("axum handler = controller role; serde DTOs = view contract") still holds: the handler is the controller, and the ViewModel is the view contract for the **server-rendered** plane, alongside the wire DTOs for the API plane.

## Consequences

- The contradiction between the table and the code is closed **explicitly**, in the record, rather than left to be discovered.
- The vocabulary is fixed before any code depends on it: the `ViewModel` trait, the field schema and the escape hatch land first, with the engine untouched, so the risk of the whole plan is contained in the slice that changes the parser.
- The framework's own application is the first consumer: the demo's inline context assembly is replaced by the framework's assembler, and the dashboard and the notification path follow. A layer the framework does not itself use is a layer nobody maintains.
- The cost is real: a new trait, a registry, a binder, a context assembler, a read-model store and a live-view execution model. The plan sequences them by dependency and each ships with its named case, so any prefix of the plan is a coherent framework.
- The parity map gains rows for the layers as they land (LDB-112 and following), each citing the case that proves it.
