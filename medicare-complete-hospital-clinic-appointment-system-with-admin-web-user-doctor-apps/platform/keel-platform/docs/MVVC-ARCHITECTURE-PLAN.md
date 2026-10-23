# MVVC architecture plan — the three framework-provided layers

**Status:** plan (no code yet). **Branch:** `p0-runtime-safety`. **Date:** 2026-10-22.

This is the plan for giving Kelson a real **MVVC** layer — the Model, the
ViewModel and the View as three layers the **framework** provides, so an
application built on Kelson declares its pages as triples rather than
hand-assembling a JSON blob and hoping the template agrees.

Every status in this document is a **plan**, not a claim. The evidence rule
for this work is unchanged from the rest of the repository: a feature is
shipped when a **named, executed** test proves it, and the parity map names
that test.

---

## 1. The decisions this plan is built on

Six questions were put to the project owner; the answers are the plan's
premises. They are recorded here so that a later reader knows what was
chosen and why, not what was merely considered.

| # | Question | Decision |
|---|---|---|
| D-1 | What does "MVVC" mean for Kelson? | **Three framework-provided layers** — the framework provides the Model, the ViewModel and the View contracts, not just the View. |
| D-2 | What shape has a ViewModel? | **A typed Rust struct plus a JSON escape hatch** — compile-time checked by default, dynamic where the app insists. |
| D-3 | Who owns the ViewModel? | **The framework ships built-in view models too** — pagination, tables, forms, error pages, the dashboard — not only the trait. |
| D-4 | How does a template reach the ViewModel (the "binder")? | **New template syntax** — a `@vm` binding directive plus schema-checked paths, not a plain namespace. |
| D-5 | What happens to the reserved context keys (`_errors`, `_locale`, `_lang`, `_auth`, `_csrf`)? | **A typed `ViewContext` the framework fills** — one assembler, used by the success path, the error path, the notification path and the dashboard. |
| D-6 | Which companion features are in scope? | **All four** — head metadata (LF-035), typed forms, the read-model layer, and live/stateful views. |

**D-1 and D-6 are the expensive ones.** The plan therefore proceeds in
dependency order, and the two large companions (the read-model layer, the
live/stateful views) land *after* the ViewModel spine is proven, because
both are worthless without it.

### 1.1 A written decision this plan contradicts — and must amend

`docs/rust/10-STRUCTURE-AND-GUIDELINES.md` (the architecture table) says:

> MVVM as a backend architecture — **no**; there is no "view" server-side
> beyond the wire contract.

Kelson already disagreed with that sentence in practice: `kelson-http`
ships `RequestViewModel` / `ResponseViewModel` (`crates/web/http/src/lib.rs:194-204`)
and the view engine has been a server-side "view" since Wave 2. This plan
turns the half-truth into a decision. **Slice S-0 amends the table and adds
an ADR** under `specs/ADR/` — the repository's convention, and the reason
the contradiction must be recorded rather than quietly overwritten.

---

## 2. The research

### 2.1 The pattern itself

MVVM was coined by Microsoft architects (Ken Cooper, Ted Peters) for
event-driven desktop UI; John Gossman announced it publicly in 2005, and
Wikipedia records the aliases *model–view–binder* (ZK, KnockoutJS) and its
derivation from **Fowler's Presentation Model** — the canonical statement
being: *the view model is a value converter, responsible for exposing the
data objects from the model in a way they can be easily managed and
presented*.

The three load-bearing properties, and what they mean for a **server-rendered**
framework:

| Property | In a desktop UI | In Kelson (server-rendered) |
|---|---|---|
| The ViewModel knows the Model; the Model does not know the ViewModel | the VM queries repositories | the VM reads a read-model / an operation result |
| The ViewModel never knows the View | the VM exposes properties and commands | the VM serialises to a context the template reads |
| The **binder** connects them | XAML / KnockoutJS data binding | the template engine's path resolution **plus a declared binding** |

Two of the classic properties transfer directly, and one needs translation:

- *"the ViewModel isolates the view from the model"* → the ViewModel is the
  only thing that knows both the domain result and the template's shape.
  That is testable **without HTTP** — the property Kelson cares about.
- *"the view model implements properties and commands to which the view can
  data bind"* → **commands** are the part that transfers badly to a
  server-rendered world. A desktop command mutates client state; in Kelson
  a command is a **named operation** the framework already dispatches. So
  the binder's command support is a binding from a template element to an
  *operation id* (§3.4), not a client-side event bus.
- *"the binder frees the developer from the obligation to write
  boiler-plate logic to synchronize the view and the view model"* → there
  is no synchronisation problem on the server. The equivalent guarantee is
  **schema-checked paths**: a template that reads a field the ViewModel
  does not declare is a **render error**, not an empty string. That is the
  guarantee this plan builds the binder around.

### 2.2 The server-side precedents (what the literature actually does)

The pattern's server-side relatives are well trodden; the useful finding is
that they converge on the same three objects:

- **Fowler's Presentation Model** — "a fully self-contained class that
  represents all the data and behavior of the UI window, but without any of
  the controls used to render that UI on the screen". A view projects the
  PM's state onto the glass. This is the ViewModel.
- **AppFolio's view models** — "the single object of interaction for the
  view… responsible for implementing the interface (data requirements) of a
  particular view template or partial". The *interface* part is what Kelson
  currently lacks: the template's data requirements are implicit in string
  keys. This plan makes them explicit (a declared schema).
- **AppFolio's form objects** — the same shape on the request side, with
  validation. → the typed forms (F-08).
- **GitLab's presenters** — "give the view an object which defines
  view-related logic/data methods"; presenters must not know the view
  context and must not generate complex DOM. → a boundary rule for the
  ViewModel: it produces **data**, never markup.
- **LiveView's presentation models** (the "Organizing LiveView logic with
  Presentation Models" essay) — the LiveView process becomes a thin shell
  over a presentation model, so the view logic is testable without the
  socket. → the shape of F-11.
- **htmx's "Model/View/Controller"** — the HTTP round trip *is* the
  boundary; a server-rendered framework's ViewModel is the thing that
  survives the round trip. → the ViewModel must be **serialisable and
  stateless**, which is why this plan's ViewModel is a value, not a session.

### 2.3 What Kelson already has

The survey (this session) of `keel-platform` found more MVVM-shaped
machinery than the architecture table admits:

| Present | Where | Note |
|---|---|---|
| `RequestViewModel` / `ResponseViewModel` traits | `crates/web/http/src/lib.rs:194-204` | the **wire** view models — the API plane's MVVM |
| `Validated<M>` / `TypedResource<R>` | `crates/web/http/src/viewmodel.rs:27,48` | the typed-handler adapter |
| The view engine, the AST, the parser, the renderer | `crates/web/views/src/{engine,ast,expr,parser,render}.rs` | the **View** layer: a pure `Value -> String` renderer |
| View **hooks** (composers) | `crates/web/views/src/engine.rs:227` | "a per-view transformer that mutates the context" — a per-view presenter, untyped |
| The `ViewTap` observation seam | `crates/web/views/src/engine.rs:81-95` | records the **post-hook** context: "what the template actually got" |
| The `NotificationViewRenderer` port | `crates/services/notifications/src/lib.rs:509` | the engine already reaches outside HTTP through a one-method port |
| The context assembler — **three copies** | `graph_routes.rs:1241-1263`, `graph_routes.rs:1121-1146`, `notifications/src/lib.rs:1031-1051` | success, error and notification paths each inject `_locale`/`_lang` themselves |
| The typed command/operation model | `crates/execution/engine/src/engine.rs:20-34` | `OperationHandler` → `OperationOutcome { result: Value, .. }` |

### 2.4 What is missing (the gap the plan closes)

| Gap | Evidence | Consequence today |
|---|---|---|
| The template context is an untyped `serde_json::Value` | `crates/web/views/src/engine.rs:423` | a typo in a key renders as empty; the failure is discovered by a human reading HTML |
| The template's **data requirements are implicit** | no schema anywhere | the view and the code that feeds it can drift apart silently |
| No page-level ViewModel | zero "presenter" hits in the tree | presentation logic lives in the HTTP edge function `graph_dispatch` (`graph_routes.rs:1700-2185`), untestable without a request |
| `_csrf` is supported by the engine (`render.rs:357`) but **never injected** | `graph_routes.rs:1241-1263` | `@csrf` renders nothing — a silently dead security affordance |
| The ops dashboard bypasses the edge path | `dashboard.rs:202-210` | no `_locale`, no `_lang`: the dashboard is not internationalised |
| No head/metadata model | LF-035 "NOT IMPLEMENTED (P3)" — `docs/LF-028-TO-LF-035-VERIFICATION.md:202-204` | no per-page title/meta, no merge rules, no request-scoped isolation |
| No read-model layer | `docs/APP-AUTHORING.md:257-258` | a page ViewModel must assemble its own projection from live queries |
| No form objects on the HTML plane | only the wire-level `RequestViewModel` | form state and validation are re-invented per app |
| No stateful view loop | LF-032 (Livewire) is `MISSING_OR_EXCLUDED_BY_DESIGN` | interactivity is the application's problem |
| `kelson_views` is not re-exported by `kelson-framework` | `crates/framework/src/lib.rs` lists 25 crates; `kelson_views` is absent | an app cannot depend on the view layer through the framework facade |
| View assertions are raw substring matching | `tests/views_integration.rs:224`, `dashboard.rs:531` | "the view contains `<p class="date">`" is not a typed assertion |

---

## 3. The target architecture

### 3.1 The three layers

```text
   ┌────────────────────────────────────────────────────────────────────┐
   │  THE VIEW            the template (.html) + the view registry      │
   │  kelson-views       a view declares the ViewModel it binds to      │
   ├────────────────────────────────────────────────────────────────────┤
   │  THE BINDER         @vm  +  @command  +  schema-checked paths      │
   │  kelson-views       a template reads vm.* and the engine CHECKS it  │
   ├────────────────────────────────────────────────────────────────────┤
   │  THE VIEWMODEL      a typed struct + a declared schema             │
   │  kelson-views       PaginationVm, TableVm, FormVm, ErrorVm, ...     │
   │                     built-ins AND application-defined ones         │
   ├────────────────────────────────────────────────────────────────────┤
   │  THE CONTEXT        ViewContext: _errors _locale _lang _auth _csrf │
   │  foundation/core    filled ONCE, by the framework, for every path   │
   ├────────────────────────────────────────────────────────────────────┤
   │  THE MODEL          operations + models  +  READ MODELS             │
   │  execution, models  a declared, materialised, refreshable projection│
   └────────────────────────────────────────────────────────────────────┘
```

**The layer rules the plan obeys (from `docs/rust/08-DECISIONS.md` D-77 and the
G07 dependency matrix):**

- `kelson-views` keeps its **zero-dependency** posture (`serde`,
  `serde_json`, `sha2`, `toml` only) — the ViewModel trait, the schema, the
  registry and the binder live there ✓.
- The shared context assembler goes to **`crates/foundation/core`** (which
  already owns `MessageCatalog`), because the notifications service crate
  may not depend on the web layer ✓. The `NotificationViewRenderer` port
  keeps its shape ✓ — only its *source* of context changes ✓.
- The read-model layer goes to the **data** layer (it needs a store) and
  the model/operation contract stays where it is ✓.
- `kelson-framework` re-exports `kelson_views` (and the new read-model
  crate) so an app depends on the framework facade ✓.

### 3.2 The ViewModel (D-2)

```rust
// crates/web/views/src/viewmodel.rs
/// The presentation state of one view.
pub trait ViewModel: Serialize {
    /// The template this model renders into.
    const VIEW: &'static str;

    /// The view's declared data requirements: the fields a template may
    /// read. A template that reads a field the model does not declare is
    /// a render error, not an empty string.
    fn schema() -> &'static [FieldSchema];

    /// The escape hatch: raw JSON merged into the model.
    fn extra(&self) -> Option<&Map<String, Value>> { None }
}
```

- `FieldSchema { path: &'static str, kind: FieldKind, required: bool }` —
  the declared requirements ✓.
- `FieldKind` — `String | Int | Float | Bool | List | Object | Timestamp |
  Html` ✓ — so the engine can refuse a *type* mismatch as well as an
  unknown path ✓.
- The JSON escape hatch (D-2) is `extra()` ✓ — a `Map` merged into the
  serialised model ✓ **after** the schema check, so an escape-hatch key is
  explicitly undeclared ✓ (and marked as such ✓).

**The ViewModel is a value, not a session** (per the htmx/LiveView reading
above): it is built per request, serialised, and discarded ✓. A ViewModel
that wants live state is a **read model** (F-06) with a staleness surface
✓ — not a mutable object in the request path ✓.

### 3.3 The view registry (D-1, D-3)

```rust
// crates/web/views/src/registry.rs
/// What the framework knows about one view.
pub struct ViewBinding {
    pub view: String,                 // "dashboard.ops"
    pub model: String,                // the model's type name, for diagnostics
    pub schema: &'static [FieldSchema],
}

pub struct ViewRegistry { bindings: BTreeMap<String, ViewBinding> }
```

- The registry is **how the framework provides the View layer** (D-1): a
  view is not a loose file, it is a registered template **with its declared
  ViewModel** ✓.
- Built-in views are registered by the framework (D-3): the error pages, the
  ops dashboard, the pagination and table partials ✓ — the app's own views
  are registered at boot ✓ (`register_package_view_namespaces` is the
  existing precedent ✓).
- A view whose file exists but whose binding is missing renders with an
  **unbound** context ✓ (today's behaviour ✓) — no breaking change ✓.

### 3.4 The binder — new template syntax (D-4)

Two additions to the template language, both fail-closed:

```html
@vm                                  {{ vm.sections.queue.depth }}
@vm("dashboard.ops")                 {{ vm.head.title }}
@command("cancel", label="Cancel")   {{ vm.rows }}
```

- **`@vm`** — binds the view's registered ViewModel into the `vm` namespace
  and **turns on schema checking for the paths that follow** ✓. Every
  `vm.*` path in the template is validated against the model's declared
  schema at render time ✓ → an unknown path is a `VIEW-RENDER` error ✓
  (fail-closed, like the rest of the engine ✓) and a **type mismatch** is
  too ✓.
- **`@command("op")`** — binds a template element to a **named operation**
  ✓. The engine renders the element with the operation's id and the
  method/verb the route declared ✓; the edge already dispatches named
  operations ✓ — so the command is a **binding**, and the framework
  refuses a command the route does not declare ✓ (no free-form dispatch ✓).
- The syntax is **additive** ✓: a template without `@vm` behaves exactly as
  today ✓ → every existing template and every existing app keeps working ✓
  ✓ (no breaking change ✓).

### 3.5 The typed ViewContext (D-5)

```rust
// crates/foundation/core/src/viewcontext.rs
/// The reserved context the framework fills for every view.
pub struct ViewContext {
    pub locale: String,
    pub lang: Value,        // the flat message map
    pub auth: Option<String>,
    pub errors: Value,
    pub csrf: Option<String>,
}
```

- One assembler ✓ — `ViewContext::assemble(...)` ✓ — used by the success
  path, the error path, the notification path and the dashboard ✓ ✓.
- `_csrf` becomes **actually wired** ✓ (the session CSRF token exists:
  `kelson_http::session_csrf_token`, `crates/web/http/src/lib.rs:32`) ✓ —
  closing a silently dead security affordance ✓.
- The dashboard gains `_locale`/`_lang` ✓ — it is internationalised as a
  side effect ✓ ✓.
- The three copies of the injection logic collapse into one ✓ — the
  duplication is measured before and after ✓ (the plan's evidence for F-04
  includes a "one assembler" grep gate ✓).

---

## 4. The feature catalogue

Every feature carries: the **shape**, the **evidence** (a named case), the
**risk**, and the **slice** that ships it. No feature ships without its
named case executed.

### F-01 — The ViewModel trait and the field schema
The trait, `FieldSchema`, `FieldKind`, the serialisation boundary, the JSON
escape hatch. `kelson-views/src/viewmodel.rs`.
**Evidence:** `a_view_model_declares_its_fields`; `the_escape_hatch_merges_undeclared_json`;
`a_view_model_serialises_to_the_context`.
**Risk:** low. **Slice:** S-1.

### F-02 — The view registry
`ViewBinding`, `ViewRegistry`, the boot-time registration of the app's views
and the framework's built-ins, the "unbound view" fallback.
**Evidence (reconciled at S-13 to the executed names):**
`a_registered_view_declares_its_model`; `an_unregistered_view_has_no_binding`;
`a_registration_replaces_the_previous_one`; `a_built_in_view_is_registered_by_the_framework`.
**Risk:** low. **Slice:** S-2.

### F-03 — The binder: `@vm` and schema-checked paths
The parser nodes, the render-time path validation against the schema, the
type-mismatch refusal, the error code (`VIEW-RENDER`).
**Evidence:** `a_bound_path_resolves_through_the_model`; `an_unknown_bound_path_is_a_render_error`;
`a_bound_path_of_the_wrong_type_is_refused`; `a_template_without_the_directive_behaves_as_before`.
**Risk:** **medium** — the parser and renderer change. Mitigation: the new
syntax is additive; the existing 50+ view cases must stay green unchanged ✓.
**Slice:** S-3.

### F-04 — The typed ViewContext and the single assembler
`ViewContext` in `foundation/core`, the assembler, the four call sites
migrated, `_csrf` wired, the dashboard internationalised.
**Evidence (reconciled at S-13 to the executed names):**
`the_reserved_context_fills_the_documented_keys` (the assembler fills `_locale`,
`_lang`, `_auth`, `_errors`, `_csrf` — the CSRF token reaches the view through
`_csrf`, that case included); `an_absent_field_stays_absent`;
`the_frameworks_keys_win_and_the_results_errors_survive`;
`a_context_that_is_not_an_object_is_left_alone`;
`the_dashboard_carries_the_locale_and_the_messages`;
`the_notification_view_carries_the_same_reserved_keys`.
**Risk:** low-medium. **Slice:** S-4.

### F-05 — The built-in view models (D-3)
`PaginationVm`, `TableVm<T>`, `FormVm`, `ErrorVm`, `EmptyVm`, `OpsVm` — typed,
with schemas, usable by any app.
**Evidence (one name reconciled at S-13):** `the_pagination_view_model_carries_the_window`;
`the_table_view_model_declares_its_columns`; `the_error_model_carries_the_status_and_the_code`.
**Risk:** low. **Slice:** S-5.

### F-06 — The Model layer: read models (D-1, D-6)
The read-model contract (a named, declared projection), the materialised-view
store (MySQL + Postgres), the refresh policy (interval / on-write / on-demand),
the staleness surface (the age, the state, the last refresh), the registry.
**Evidence (reconciled at S-13 to the executed names):**
`a_read_model_declares_its_sources_and_policy`; `an_on_write_policy_triggers_for_its_sources`;
`the_registry_rejects_duplicate_names`; `the_registry_tracks_stores`. Not built
with the slice: the PostgreSQL materialised store and a budget-refused
refresh — stated in `docs/APP-AUTHORING.md` §6 rather than claimed.
**Risk:** **high** — a second persistence shape beside the model store, and
the tenancy scope must hold for it ✓. Mitigation: the read model is a
**projection over the existing store**, scoped by the same tenant rule ✓ —
it never becomes a second source of truth ✓ ✓.
**Slice:** S-6, S-7 (MySQL, then Postgres).

### F-07 — The head metadata store (LF-035, D-6)
A per-request head state: the title, the metas, the order, the merge policy
(the child's value wins, the parent's defaults fill), and **request-scoped
isolation** (no leakage between requests — the gap LF-035 names).
**Evidence:** `the_head_merges_the_child_over_the_parent`;
`the_head_state_does_not_leak_between_requests`; `the_head_reaches_the_template`.
**Risk:** medium (the isolation is the whole point — a shared mutable head
would be a cross-tenant leak). **Slice:** S-8.

### F-08 — Typed forms (D-6)
The form object: the declared fields, the submitted values, the validation
errors, the binding to the template, and the bridge to the existing
`RequestViewModel` (the wire plane) so a form has **one** definition.
**Evidence (reconciled at S-13 to the executed names):**
`the_form_view_model_carries_fields_and_errors`;
`a_bridged_form_validates_submitted_data`; `the_wire_plane_drops_a_key_no_field_declared`.
**Risk:** medium. **Slice:** S-9.

### F-09 — The `@command` binding
The command binding from a template element to a named operation, the
refusal of an undeclared command, the CSRF-carrying rendering.
**Evidence:** `a_command_binds_to_the_declared_operation`;
`an_undeclared_command_is_refused`; `a_command_spoofs_the_declared_verb_and_refuses_a_read_only_one`;
`a_command_without_a_csrf_token_is_refused`; `a_command_requires_an_operation_id`;
`a_command_refuses_unknown_options`.
**Risk:** medium-high (it touches the request path). **Slice:** S-10.

### F-10 — Live / stateful views (D-6)
The stateful session, the diff, the transport. The framework already ships
`kelson-realtime`; the LiveView-ish loop is a **presentation** state machine
over it, with the same ViewModel (the state is a ViewModel, re-rendered on
change) ✓.
**Evidence (S-11 spine, S-12 wire):** `a_state_change_rerenders_the_view_model`;
`the_diff_carries_only_the_changed_parts`; `a_stale_session_is_refused`;
`the_wire_carries_only_the_changed_parts`; `the_wire_refuses_to_publish_a_stale_session`.
**Risk:** **highest** — it is a new execution model (a long-lived session
per browser), and the framework currently excludes Livewire by design
(LF-032). Mitigation: it is **opt-in per view** and reuses the ViewModel
spine ✓; the exclusion is lifted only when the evidence is green ✓.
**Slice:** S-11, S-12 (the spine, then the transport).

### F-11 — The framework facade re-exports
`kelson-framework` re-exports `kelson_views` and the read-model crate, so an
app depends on the framework, not on the internals.
**Evidence:** `the_framework_facade_exports_the_view_layer`.
**Risk:** low. **Slice:** S-2 (with the registry).

### F-12 — The app-facing documentation
`docs/APP-AUTHORING.md` gains the MVVC section (the three layers, the
binder, the built-ins, the forms, the head, the read models, the live
views), and §6 "What is not there yet" is corrected as each feature lands.
**Evidence:** the document's claims are checked by the parity map's cited
cases (the same rule as every other row).
**Risk:** none. **Slice:** with each feature's slice.

### F-13 — The testkit's typed view assertions
`TestResponse::assert_view_has` (the `ViewTap` seam) grows a typed
assertion: assert on the **ViewModel**, not the raw context — plus a
golden-HTML option and a template lint (a template that reads an undeclared
path is caught **at build time**, not at render time).
**Evidence:** `a_view_model_is_assertable_without_the_html`;
`a_template_that_reads_an_undeclared_path_is_refused_by_the_lint`.
**Risk:** low. **Slice:** S-3 (with the binder), S-5.

---

## 5. The refactor plan

### 5.1 The invariants every slice obeys

1. **No breaking change for an application.** Every existing template,
   route, view and app keeps working ✓ — the new syntax is additive ✓, the
   unbound view still renders ✓, the old context assembly keeps its shape
   until the app opts in ✓.
2. **No new dependency edges that break the matrix.** The `ViewModel` trait
   and the binder live in `kelson-views` (zero-dep) ✓; the shared assembler
   lives in `foundation/core` ✓; the read model lives in the data layer ✓.
3. **Every feature ships with its named case executed** ✓ — the parity map
   names the case ✓ (the map's verifier script checks that the case exists
   ✓; the case is also *run* ✓).
4. **The layer rules are gates, not prose.** `cargo xtask dep-rules` and the
   G07 matrix must stay green ✓ — a new edge is a design failure, not a
   lint suppression ✓.
5. **The demo migrates as the proof.** The demo's inline context assembly
   (`graph_routes.rs:1241-1263`) is replaced by the assembler ✓ and the
   dashboard and the notification path follow ✓ — the framework's own app is
   the first consumer ✓.

### 5.2 The slices (ordered)

| Slice | Feature(s) | What lands | The gate that proves it |
|---|---|---|---|
| **S-0** | the decision | the ADR (`specs/ADR/`), the amendment to `docs/rust/10-STRUCTURE-AND-GUIDELINES.md`, the MVVC map rows | the ADR is cited by the map; the doc no longer says "no" |
| **S-1** | F-01 | the `ViewModel` trait, the schema, the escape hatch | the three F-01 cases |
| **S-2** | F-02, F-11 | the `ViewRegistry`, the built-in registrations, the facade re-export | the F-02 + F-11 cases; `cargo xtask dep-rules` green |
| **S-3** | F-03, F-13 | `@vm`, the schema-checked paths, the type refusal, the template lint, the typed assertions | the F-03 + F-13 cases **and** the existing 50+ view cases unchanged |
| **S-4** | F-04 | the `ViewContext`, the one assembler, the four call sites, `_csrf` wired, the dashboard i18n | the F-04 cases; the "one assembler" gate |
| **S-5** | F-05 | the built-in view models | the F-05 cases; the built-ins used by the demo's error pages and the dashboard |
| **S-6** | F-06a | the read-model contract, the registry, the refresh policy, the staleness surface (in-memory first) | the F-06 cases (in-memory) |
| **S-7** | F-06b | the materialised-view store: MySQL, then Postgres, tenant-scoped | the live MySQL + Postgres cases; the tenancy proof |
| **S-8** | F-07 | the head store, the merge policy, the request-scoped isolation | the F-07 cases (the isolation one is the important one) |
| **S-9** | F-08 | the form objects, the validation, the wire bridge | the F-08 cases |
| **S-10** | F-09 | the `@command` binding | the F-09 cases |
| **S-11** | F-10a | the live spine: a stateful ViewModel + a re-render, no transport | the F-10a cases |
| **S-12** | F-10b | the transport (over `kelson-realtime`), the diff | the F-10b cases |
| **S-13** | F-12 | the app-facing documentation, the map rows, the findings | the map's verifier + the cited cases |

### 5.3 The file-level edit points (the plan's map)

| File | Change | Slice |
|---|---|---|
| `crates/web/views/src/viewmodel.rs` (new) | the trait, `FieldSchema`, `FieldKind` | S-1 |
| `crates/web/views/src/registry.rs` (new) | `ViewBinding`, `ViewRegistry` | S-2 |
| `crates/web/views/src/ast.rs` | the `Node::VmBind` / `Node::Command` variants | S-3 |
| `crates/web/views/src/parser.rs` | the `@vm` / `@command` directives, the known-directive ledger | S-3 |
| `crates/web/views/src/render.rs` | the bound-path resolution, the schema check, the type check | S-3 |
| `crates/web/views/src/engine.rs` | the registry on the engine, the built-in bindings, the `@vm` context | S-2, S-3 |
| `crates/web/views/src/lib.rs` | the re-exports | S-1, S-2 |
| `crates/foundation/core/src/viewcontext.rs` (new) | `ViewContext` + the assembler | S-4 |
| `crates/foundation/core/src/lib.rs` | the re-export | S-4 |
| `crates/data/models/src/readmodel.rs` (new) | the read-model contract, the refresh policy, the staleness | S-6 |
| `crates/data/mysql/src/readmodel.rs` (new) | the materialised-view store (MySQL) | S-7 |
| `crates/data/postgres/src/readmodel.rs` (new) | the materialised-view store (Postgres) | S-7 |
| `runtime/kelson-demo/src/graph_routes.rs` | the context assembly → the assembler; the `@command` dispatch; the head | S-4, S-8, S-10 |
| `runtime/kelson-demo/src/dashboard.rs` | route through the assembler; the `OpsVm` | S-4, S-5 |
| `runtime/kelson-demo/src/host.rs` | register the app's views + the built-ins | S-2 |
| `crates/services/notifications/src/lib.rs` | the port's context source → the shared assembler | S-4 |
| `crates/framework/src/lib.rs` | re-export `kelson_views` (+ the read-model crate) | S-2, S-6 |
| `testing/testkit/src/app.rs` | the typed view assertions, the golden HTML | S-3, S-5 |
| `docs/APP-AUTHORING.md` | the MVVC contract; §6 corrected per slice | S-13 |
| `specs/ADR/00xx-mvvc-layers.md` (new) | the decision | S-0 |

### 5.4 The parity map and the findings register

New map rows (the same shape as LDB-107..LDB-111 ✓):

| Row | Feature | The Laravel counterpart |
|---|---|---|
| `LDB-112` | the ViewModel layer | Blade's `@php` blocks in views / Laravel's view composers |
| `LDB-113` | the binder (`@vm`, schema-checked paths) | none — Laravel has no binder |
| `LDB-114` | the typed `ViewContext` | Laravel's `@shared` / `View::share` |
| `LDB-115` | the built-in view models | Blade components (`x-pagination`) |
| `LDB-116` | the head metadata store | `@stack('head')` / the `title` share |
| `LDB-117` | the typed forms | Laravel's form requests + `@error` |
| `LDB-118` | the read-model layer | Laravel's `scout`/cached aggregates — **a deliberate divergence** (a declared, refreshable projection) |
| `LDB-119` | the live/stateful views | Livewire (LF-032: currently excluded — this row supersedes it) |

**Shipped numbering (reconciled at S-13):** the ledger followed the shipped
order, not this table's preallocation — `LDB-118` = the forms (S-9), `LDB-119`
= the `@command` binding (S-10), `LDB-120` = the live spine (S-11), `LDB-121`
= the live wire (S-12), and the read-model layer — which this table
preallocated as `LDB-118` — landed as `LDB-122`, since it shipped (S-6/S-7,
materialised for MySQL; PostgreSQL's store still open) before the forms did.
Every row's evidence column names the cases that actually ran.

Findings-register rows: one per defect the plan's implementation uncovers
✓ (the register's rule ✓: a repro case, a report, a proposed fix ✓).

---

## 6. The risks

| Risk | Why it is real | The mitigation |
|---|---|---|
| **The scope is larger than the evidence budget** | D-1 + D-6 put four substantial companions in one plan | the slices are ordered by dependency; each ships with its named case; the plan can stop after any slice and still be a coherent framework |
| **The binder changes the parser and the renderer** | the two most exercised files in the view crate | additive syntax only; the existing view cases must pass **unchanged**; the new nodes are separate `Node` variants |
| **The head store leaks across requests** | a shared mutable head is a cross-tenant leak | the head state is **request-scoped by construction** (it is a parameter, not a field), and the isolation case is a *named* case, not a comment |
| **A read model becomes a second source of truth** | the fastest way to corrupt a multi-tenant system | the read model is a **projection of the existing store**, refreshed by the same policy, and its refresh is tenant-scoped |
| **Live views change the execution model** | a long-lived session per browser | opt-in per view; the ViewModel spine is reused; the exclusion (LF-032) is lifted only with green evidence |
| **The plan contradicts a written decision** | `docs/rust/10-STRUCTURE-AND-GUIDELINES.md` says MVVM-as-backend is "no" | S-0 amends it **explicitly**, with an ADR — the contradiction is recorded, not hidden |
| **"MVVC" becomes a buzzword** | the pattern's value is testability, not the label | every feature's evidence is a **case that fails without the feature** — if a feature cannot be shown to fail when removed, it is not in the plan |

---

## 7. The first slice in detail (S-0 + S-1)

Because the user asked for a plan and the decisions are taken, the first two
slices are specified to the point of being implementable:

**S-0 — the decision.** A new ADR `specs/ADR/00xx-mvvc-layers.md` that
states: the three layers, the binding, the typed context, the four
companions, the layer rules, and the **non-goals** (no client-side state
machine, no client-side data binding, the server remains the source of
truth ✓). The architecture table's MVVM row is amended from "no" to
"adopted, as the server-side presentation layer — see ADR-00xx" ✓.

**S-1 — the ViewModel trait.** `crates/web/views/src/viewmodel.rs`:

```rust
pub enum FieldKind { String, Int, Float, Bool, List, Object, Timestamp, Html }
pub struct FieldSchema { pub path: &'static str, pub kind: FieldKind, pub required: bool }

pub trait ViewModel: Serialize {
    const VIEW: &'static str;
    fn schema() -> &'static [FieldSchema];
    fn extra(&self) -> Option<&Map<String, Value>> { None }
}
```

with the three cases (the declared fields, the escape hatch, the
serialisation) ✓ — and **no** change to the engine yet ✓, so the slice is
additive and provable in isolation ✓.

---

## 8. What the plan does **not** decide

- **The exact template syntax** beyond `@vm` and `@command` — the parser's
  grammar is settled in S-3, against the real templates in the demo and the
  starter kit ✓ (the syntax must survive real templates, not just a
  toy one ✓).
- **The live-view transport** — websocket framing versus server-sent
  events is settled in S-12, against what `kelson-realtime` already
  offers ✓.
- **The read model's storage shape** — a table per projection versus a
  generic key/value envelope is settled in S-6/S-7, against the MySQL
  matrix ✓ (the tenancy proof decides it ✓).
- **Whether the built-in view models are the ones the demo uses** — S-5
  decides it by trying them on the demo's error pages and the dashboard ✓
  (a built-in nobody uses is a built-in nobody maintains ✓).

---

## 9. The next move

S-0 and S-1 are small, additive and independently provable ✓. They are the
right next slice: they fix the **vocabulary** (what a ViewModel *is*) before
any code depends on it ✓, and they leave the engine untouched ✓ — so the
risk of the whole plan is contained in S-3, where the parser changes ✓.
