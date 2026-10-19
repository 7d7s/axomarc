# Views, Templates & Frontend — Laravel Feature Atlas Verification (LF-028 to LF-035)

> **Revision 2 — 2026-10-19.** Replaces revision 1, which marked LF-028/LF-029 VERIFIED on a bare
> "kelson-views: 53 tests ✅" line and left the other six items with **no evidence at all** (six
> `—` cells) while still concluding "All applicable view/template features are VERIFIED". Every
> status below is backed by a named test executed on 2026-10-19 against this working tree; the
> corrections are listed in §5.
>
> Status vocabulary: **VERIFIED** (executed evidence matches the atlas expectation) ·
> **PARTIALLY VERIFIED** (implemented; at least one expectation uncovered, unproven, or a recorded
> divergence) · **EXCLUDED BY DESIGN** (a product decision, not a missing feature) · **DEFER**
> (a deliberate, documented deferral behind a decision gate) · **NOT IMPLEMENTED** · **NOT RUN** /
> **EXTERNAL** (belongs to the ecosystem, not to the framework).

## 1. Verification Matrix

| ID | Feature | Disposition | Kelson equivalent | Executed evidence (2026-10-19) | Status |
|---|---|---|---|---|---|
| LF-028 | Views and composers | MATCH | `kelson-views` engine: view factory (`resolve`/`exists`/`template` over `ViewsConfig.paths`, dot-path→directory, package namespaces via a resolver hook), data binding (JSON context + edge conventions `_csrf`/`_auth`/`_errors`/`_lang`), layouts (`@extends`/`@section`/`@yield`/`@show`/`@parent`), composers (`register_hook` — per-view and `"*"` shared), view caching (compiled AST pinned by mtime + content hash, `view:cache`/`view:clear`) | `config_parses_views_section`; `package_namespace_resolver` (incl. `VIEW-NOT-FOUND` for an unresolvable namespace); `view_hooks_compose_context`; `compiled_cache_roundtrip_and_invalidation`; `extends_sections_yield_parent_show`; `sections_without_extends_define_then_yield`; `show_emits_the_section_in_place`; demo: `html_route_renders_the_view_with_the_operation_result_as_context`, `authenticated_html_route_carries_the_principal_as_auth_context`, `missing_error_view_falls_back_to_the_envelope` | VERIFIED |
| LF-029 | Blade template engine | FUNCTIONAL EQUIVALENT (no PHP eval) | Zero-dep **interpreted-AST** engine (parser → AST → renderer), Blade-familiar syntax; `{{ }}` auto-escaped vs `{!! !!}` raw; the full directive ledger; components/slots/props, stacks, includes, loops; custom directives + `Blade::if`-style custom conditionals | `escaping_security_facet`; `escaping_covers_all_significant_chars`; `js_encoding_escapes_html_significant_chars`; `quoted_literal_echo_escapes`; `parses_text_echo_and_raw`; `component_slots_and_props`; `parses_component_with_slots_and_props`; `push_and_stack`; `include_variants` (incl. the missing-include fail-closed); `loops_foreach_forelse_for`; `parses_forelse_with_empty_branch`; `runaway_for_fails_closed`; `custom_directives_registered_and_literal_fallback`; `custom_conditional_blade_if_parity`; `superseded_directives_are_refused`; `superseded_directives_refused_at_render_entry`; `unterminated_constructs_fail_closed`; `stray_closing_directive_is_refused` | VERIFIED (divergences recorded — §3) |
| LF-030 | Asset bundling / Vite | PARTIAL (manifest ours, bundler external) | `kelson_http::assets` reads the Vite build manifest (`build/manifest.json`, Vite 5+ `build/.vite/manifest.json`): source key → hashed file + stylesheets; `asset_url` refuses `/../`; a malformed manifest is a boot error, not an empty manifest that ships wrong links; the views `@asset` helper resolves through it | `manifest_keys_resolve_to_hashed_urls`; `unknown_keys_fall_back_to_literal_paths`; `malformed_manifest_is_an_error_not_an_empty_map`; `traversal_in_a_key_never_escapes_the_root`; demo: `asset_directive_resolves_through_the_vite_manifest`, `without_a_build_manifest_the_helper_emits_literal_urls` | PARTIALLY VERIFIED (bundler/HMR/build external; **no CSP nonce** — F-6) |
| LF-031 | Frontend modes and starter kits | PARTIAL (one SPA-first kit) | `kelson new --with-starter` ships a coherent SPA-first app: a full-CRUD notes module (5 capabilities / 5 routes), the `web/` shell, `resources/views/errors/{404,500,maintenance}.html`, and a Playwright e2e kit (`e2e/auth.spec.ts`, `e2e/notes.spec.ts`, `playwright.config.ts`); templates are embedded in `kelson-templates` so a published console still generates them | `tests::starter_template_renders_parses_and_compiles` (the generated project renders, its manifest parses and it compiles); `kelson-templates`: `embedded_templates_are_complete_and_unambiguous`, `substitution_leaves_no_marker_behind`, `a_marker_next_to_an_underscore_renders_one_separator` | PARTIALLY VERIFIED (no kit *choice* — Blade/Livewire/Inertia kits are not offered; the e2e kit is not wired into CI) |
| LF-032 | Livewire | EXCLUDE BY DESIGN | Not implemented, and deliberately so: Livewire is a PHP-package reactive-UI model (PHP component classes + Blade) with no Rust-framework equivalent; the product direction is the Rust view engine + a SPA-first shell | — (no code, no test; the exclusion is a recorded product decision in `KELSON-VS-LARAVEL-COMPARISON.md` and `CLAIMS-VERIFICATION-VERDICT.md`) | EXCLUDED BY DESIGN (P3) — attribution finding F-5 |
| LF-033 | Inertia adapters | DEFER (v2.0 gate) | Not implemented: Inertia needs a React/Vue/Svelte client adapter; the starter ships a vanilla-JS shell. The framework blueprint gates the SSR/template-engine direction as a v2.0 decision (B-26) | — (no code, no test; the deferral is a decision gate, not an omission) | DEFER (P2) |
| LF-034 | Folio file-based routing | DEFER BY DESIGN (analogue exists) | Folio's page-file convention is not implemented; Kelson's routing stays explicit and manifest/graph-anchored (typed analysis, no convention magic). The nearest analogue — **route files** merged as synthetic modules — is implemented and exercised | registry: `root_route_files_merge_as_synthetic_modules`, `routefile_routes_with_registered_ops_compile`, `routefile_op_gate_requires_registered_module_operations`; demo: `route_file_route_dispatches_to_the_declared_module_handler` | DEFER (P2) — the Folio feature itself is NOT IMPLEMENTED |
| LF-035 | Head / document metadata | NOT IMPLEMENTED | No declarative head manager (no per-page title/meta store, no merge/ordering rules, no nonce). The nearest analogue is Blade's own head pattern, `@push`/`@stack`, which the engine implements | `push_and_stack` (the @push/@stack analogue only) | NOT IMPLEMENTED (P3) |

## 2. Method and executed evidence

### 2.1 Runs (2026-10-19, from `platform/keel-platform`)

| Run | Result |
|---|---|
| `cargo test -p kelson-views --lib` | **53 passed**, 0 failed |
| `cargo test -p kelson-demo --test views_integration` | **9 passed**, 0 failed |
| `cargo test -p kelson-demo --test static_assets_integration` | **6 passed**, 0 failed |
| `cargo test -p kelson-http --lib assets` | **4 passed**, 0 failed (156 filtered out) |
| `cargo test -p kelson-templates` | **3 passed**, 0 failed |
| `cargo test -p kelson-console --bin kelson starter` | **1 passed**, 0 failed (58 filtered out) |
| `cargo test -p kelson-registry --lib` (the route-file + graph gates behind LF-034) | **108 passed**, 0 failed |
| `cargo test -p kelson-demo --test http_integration -- route_file` (LF-034) | **1 passed**, 0 failed (91 filtered out) |

**185 green results, 0 failures.** Logs: `%TEMP%\lf028-batch\` (01-views.log … 06-routefile.log).

### 2.2 Commands to re-run

```powershell
$env:CARGO_BUILD_JOBS=1; $env:CARGO_INCREMENTAL=0
cargo test -p kelson-views --lib
cargo test -p kelson-demo --test views_integration
cargo test -p kelson-demo --test static_assets_integration
cargo test -p kelson-http --lib assets
cargo test -p kelson-templates
cargo test -p kelson-console --bin kelson starter
cargo test -p kelson-registry --lib
cargo test -p kelson-demo --test http_integration -- route_file
```

### 2.3 What the 53 `kelson-views` tests actually cover

| Area | Tests |
|---|---|
| Config / lookup | `config_parses_views_section`, `package_namespace_resolver` |
| Escaping | `escaping_security_facet`, `escaping_covers_all_significant_chars`, `js_encoding_escapes_html_significant_chars`, `quoted_literal_echo_escapes` |
| Layouts / sections | `extends_sections_yield_parent_show`, `sections_without_extends_define_then_yield`, `show_emits_the_section_in_place`, `parses_sections_yield_parent_show`, `parses_inline_section` |
| Composers | `view_hooks_compose_context` |
| Components / slots | `component_slots_and_props`, `parses_component_with_slots_and_props` |
| Stacks / includes | `push_and_stack`, `include_variants`, `parses_include_variants` |
| Loops | `loops_foreach_forelse_for`, `parses_foreach_forms`, `parses_for_range_and_cstyle`, `parses_forelse_with_empty_branch`, `runaway_for_fails_closed` |
| Conditionals / auth | `conditionals_if_elseif_else_unless`, `parses_if_elseif_else`, `unless_negates_and_takes_else`, `auth_guest_env_production` |
| Directives / extension points | `custom_directives_registered_and_literal_fallback`, `custom_conditional_blade_if_parity`, `unknown_parenthesized_directive_becomes_custom`, `class_style_and_attr_helpers`, `csrf_method_error_lang_js`, `once_renders_once_inside_loops` |
| Expressions / filters | `filters_and_dot_paths`, `parses_dot_paths_and_filters`, `parses_indexed_paths_and_filter_args`, `evaluates_filters`, `operand_parsing_covers_literals_and_paths`, `conditions_evaluate`, `truthiness_matches_blade_intuition` |
| i18n | `lang_falls_back_to_the_key`, `trans_filter_renders_through_lang_map`, `trans_filter_resolves_lang_map_and_degrades` |
| Fail-closed parsing | `superseded_directives_are_refused`, `superseded_directives_refused_at_render_entry`, `unterminated_constructs_fail_closed`, `stray_closing_directive_is_refused`, `escaped_at_and_echo_are_literal` |
| Cache | `compiled_cache_roundtrip_and_invalidation` |
| Scoping | `scopes_shadow_the_root_context` |

### 2.4 NOT RUN / external in this batch
- **The bundler, dev server and HMR** (Vite/npm) — external by the atlas's own guidance ("Use ecosystem
  bundler rather than reproduce it"), not executed and not implemented.
- **The starter's Playwright e2e kit** — the specs ship in the template, but the CI job that would run
  them is not wired (the workflow file is frozen) → not executed.
- **Inertia / Livewire client integrations** — not implemented; nothing to run.

## 3. Detailed verification

### LF-028 — Views and composers — VERIFIED
- **View factory / lookup paths.** `ViewsConfig` (from the `[views]` config section) carries
  `paths` + `cache_dir`; `ViewsEngine::resolve` maps a dot-path name to a directory and walks the
  configured paths, and a package namespace (`pkg::view`) is resolved through an installed resolver
  hook — with no resolver, namespaces fail closed. Executed: `config_parses_views_section`,
  `package_namespace_resolver` (asserts `VIEW-NOT-FOUND` for an unresolvable namespace).
- **Data binding.** `render(name, context)` takes a JSON context; the web edge injects the framework
  conventions `_csrf`, `_auth`, `_errors`, `_lang`, and the demo binds the operation result as the
  context. Executed: `html_route_renders_the_view_with_the_operation_result_as_context`,
  `authenticated_html_route_carries_the_principal_as_auth_context`, `filters_and_dot_paths`.
- **Layouts.** `@extends`/`@section`/`@endsection`/`@yield`/`@show`/`@parent`, including a section
  without `@extends` and a section rendered in place. Executed: `extends_sections_yield_parent_show`,
  `sections_without_extends_define_then_yield`, `show_emits_the_section_in_place`.
- **Composers / creators.** View hooks mutate the context before render; a `"*"` hook runs for every
  view (the shared-data injection). Executed: `view_hooks_compose_context`.
- **View caching.** The parsed AST is cached under the cache dir, pinned by template mtime **and**
  content hash; a content change reparses, `precompile_all`/`clear_cache`/`forget_cache` back the
  `kelson view:cache` / `kelson view:clear` commands. Executed:
  `compiled_cache_roundtrip_and_invalidation`.
- **Note:** a *directly* named missing template is covered only through the include and namespace
  paths; a `render("no-such-view")` unit test does not exist.

### LF-029 — Blade template engine — VERIFIED (functional equivalent; divergences recorded)
- Kelson does **not** build a Blade parser. It implements a zero-dep interpreted-AST engine with
  Blade-familiar syntax over a JSON context — the "document an equivalent" branch the atlas asks for.
- **Escaping.** `{{ expr }}` is auto-escaped; `{!! expr !!}` is the explicit raw form. Executed:
  `escaping_security_facet`, `escaping_covers_all_significant_chars`,
  `js_encoding_escapes_html_significant_chars`, `quoted_literal_echo_escapes`, `parses_text_echo_and_raw`.
- **Directives.** The ledger covers extends/section/yield/show/parent, include/includeIf/includeWhen,
  component/slot/endslot/props, if/elseif/else/unless/isset/empty, auth/guest/env/production,
  foreach/for/forelse, once, push/stack, csrf/method/error/enderror, lang, js and the
  class/style/checked/selected/disabled/readonly/required helpers.
- **Extension points.** Custom directives and `Blade::if`-style custom conditionals; an unregistered
  directive renders **literally** (Blade parity — `@media(min)` survives), and a registered one
  overrides the literal fallback.
- **Recorded divergences** (by design, fail-closed):
  1. **No PHP evaluation.** `@php` and `@while` are refused at parse time; the engine never evals
     arbitrary code. Executed: `superseded_directives_are_refused`,
     `superseded_directives_refused_at_render_entry`.
  2. **No Blade compiler.** Templates are parsed to an AST and interpreted; there is no PHP
     compilation step, so Blade-specific runtime semantics (e.g. `@php` blocks, custom PHP helpers)
     have no equivalent.
  3. **A runaway loop fails closed.** `@for`/`@foreach`/`@forelse` are capped at
     `MAX_LOOP_ITERATIONS` (100 000) → `VIEW-LOOP`. Executed: `runaway_for_fails_closed`.
  4. **Malformed templates fail closed** at parse time (`unterminated_constructs_fail_closed`,
     `stray_closing_directive_is_refused`).

### LF-030 — Asset bundling / Vite — PARTIALLY VERIFIED
- **What is ours and proven.** The build-manifest reader (`kelson_http::assets`): source key →
  hashed artifact + its stylesheets, root-absolute URLs, `/../` refusal, and a **malformed manifest
  is a boot error** (never an empty manifest that silently ships wrong links). The views `@asset`
  helper resolves through the same manifest, with a literal-URL fallback when no build exists.
  Executed: `manifest_keys_resolve_to_hashed_urls`, `malformed_manifest_is_an_error_not_an_empty_map`,
  `traversal_in_a_key_never_escapes_the_root`, `unknown_keys_fall_back_to_literal_paths`,
  `asset_directive_resolves_through_the_vite_manifest`,
  `without_a_build_manifest_the_helper_emits_literal_urls`.
- **What is external.** The bundler, the dev server and HMR are ecosystem tooling; the atlas itself
  says to use the ecosystem bundler rather than reproduce it, and to include the item in parity scope
  only for a first-party full-stack frontend. The starter template ships **no** `vite.config.js` and no
  app-root `package.json` (only `e2e/package.json` for the browser kit) → the production build step is
  not in-repo.
- **Gap (F-6).** The atlas lists "CSP nonce" — there is **no nonce support**: the security-headers
  CSP is a static string (`SecurityHeadersSection.csp`), so a per-response nonce cannot be injected.
- **Gap.** "Missing assets fail clearly" holds for a *malformed* manifest, but an unknown asset key
  falls back to a literal URL (`unknown_keys_fall_back_to_literal_paths`) — a missing-asset 404 is
  not raised.

### LF-031 — Frontend modes and starter kits — PARTIALLY VERIFIED
- **What exists.** `kelson new --with-starter` materializes a coherent SPA-first app: a full-CRUD
  notes module (5 capabilities, 5 routes, 3 events), the `web/` shell (`app.js`, `index.html`),
  `resources/views/errors/{404,500,maintenance}.html`, and a Playwright browser kit
  (`e2e/auth.spec.ts`, `e2e/notes.spec.ts`, `e2e/playwright.config.ts`, `e2e/package.json`).
  The templates are **embedded** in the `kelson-templates` crate, so a console installed from crates.io
  still generates a complete project (that is the crate's stated reason to exist).
- **Executed.** `tests::starter_template_renders_parses_and_compiles` — the generated project renders,
  its manifest parses and it compiles. Plus the templates crate: every embedded file is non-empty, no
  path appears twice (a duplicate would silently overwrite), and the marker substitution leaves no
  marker behind.
- **Gaps.**
  1. **No kit choice.** The atlas expects a project to *choose* a frontend approach (server-rendered,
     SPA, API-only, mixed) and to get a kit for it. Kelson offers two fixed templates (project /
     starter); there is no Blade kit, no Livewire kit, no Inertia kit, no kit selector.
  2. **The e2e kit is not executed.** The specs ship, but no CI job runs them (the workflow file is
     frozen) — the browser-level expectations ("route/UI flow", "asset compile", "accessibility
     smoke") are therefore unproven.
  3. **No upgrade/migration tests** for a generated project (the atlas lists them).

### LF-032 — Livewire — EXCLUDED BY DESIGN (P3)
- Livewire is a PHP package: PHP component classes + Blade templates driving server-side reactive UI.
  Kelson's product direction is a Rust framework with its own view engine and a SPA-first shell, so
  the reactive-component model has no equivalent to implement, and the atlas itself marks the item
  "may be deliberately excluded … Exclusion is a product decision, not 100% parity".
- **Nothing was executed** (there is nothing to run) and the exclusion is recorded, not implied.
- **Finding F-5**: the attribution behind this row is wrong (see §4).

### LF-033 — Inertia adapters — DEFER (P2)
- Inertia needs a React/Vue/Svelte client adapter plus server-side page props; the starter ships a
  vanilla-JS shell and the framework has no page-props contract. The blueprint gates the
  SSR/template-engine direction as a v2.0 decision (B-26), so this item is **deferred behind a
  decision gate** rather than omitted.
- Nothing executed; the deferral is recorded in `docs/rust/13-FRAMEWORK-BLUEPRINT.md` (B-26).

### LF-034 — Folio file-based routing — DEFER (P2), with an analogue
- Folio's convention-over-configuration (page files under `resources/views/pages` define routes) is
  **not implemented**, and the atlas itself says to "avoid if graph/manifest routing already provides
  better explicitness and typed analysis" — which it does: routes are declared in a manifest, compiled
  into a typed graph, and a route without a registered operation is refused at compile time.
- **The nearest analogue is implemented and exercised**: *route files* (`routes/*.toml`) are discovered
  and merged as synthetic modules, and an operation that is not registered is refused
  (`routefile_op_gate_requires_registered_module_operations`). Executed:
  `root_route_files_merge_as_synthetic_modules`, `routefile_routes_with_registered_ops_compile`,
  `route_file_route_dispatches_to_the_declared_module_handler`.
- Difference worth recording: route files are **manifest-anchored and typed** (the operation must
  exist), not Folio's convention-only page files.

### LF-035 — Head / document metadata — NOT IMPLEMENTED (P3)
- There is **no declarative head manager**: no per-page title/meta store, no merge or ordering rules
  for duplicate keys, no escaping policy for head values, no request-scoped isolation of head state,
  and no SSR/hydration story.
- The nearest analogue is Blade's own head pattern, `@push`/`@stack` (executed:
  `push_and_stack`), which the engine implements — but that is a stack, not a metadata model.
- The atlas marks the item P3 and "low priority unless Kelson owns server-rendered frontend and SEO
  conventions"; the framework's SSR direction is the blueprint's v2.0 gate (B-26).

## 4. Findings (new in this pass)

- **F-4 (low) — the views crate points at a doc record that does not exist.** `kelson-views/src/lib.rs`
  states the `@php`/`@while` divergence is "recorded in docs/18 among the parity divergences". There is
  no such record: `platform/docs/18-USER-FLOWS.md` is a user-flows document, and
  `platform/docs/rust/` has no 18 either. The divergence is real and tested
  (`superseded_directives_are_refused`), but the pointer is dead — either write the record or drop the
  claim. Next: add the divergence to a parity record (or to `docs/CLAIMS-VERIFICATION-VERDICT.md`) and
  repoint the comment.
- **F-5 (low) — the Livewire exclusion is attributed to a blueprint section that does not list it.**
  The atlas row (LF-032), `KELSON-VS-LARAVEL-COMPARISON.md` (lines 100, 249) and
  `CLAIMS-VERIFICATION-VERDICT.md` (line 186) all attribute the exclusion to "framework blueprint
  sec.26.3". The blueprint's own sec-26.3 permanent-exclusion list
  (`docs/rust/13-FRAMEWORK-BLUEPRINT.md:118`) names facades/global helpers, the Collection wrapper,
  MongoDB, JWT-by-default, Cashier/Socialite/Dusk/Valet core clones and the marketplace — **Livewire is
  not in it**. The exclusion may still be the right call, but the citation does not support it. Next:
  either add Livewire to the blueprint's exclusion list or re-cite the actual decision.
- **F-6 (low, P1 item) — CORS-adjacent CSP nonce support is absent.** LF-030 expects a "CSP nonce";
  `SecurityHeadersSection.csp` is a static string and no nonce is generated per response, so a
  strict-CSP policy cannot be used with the shipped header helper. Next: either generate a per-request
  nonce (and thread it into the CSP header) or record the CSP-nonce expectation as out of scope.

## 5. Corrections to revision 1 (the previous version of this file)

1. **"All applicable view/template features are VERIFIED"** was not defensible: six of the eight rows
   carried **no evidence at all** (a `—` cell) and two were marked VERIFIED on a bare crate-level test
   count.
2. **LF-028** was evidenced by "kelson-views: 53 tests ✅" — a crate-level count, not a mapping from
   the six atlas expectations (lookup paths, data binding, layouts, composers, caching) to named tests.
   All six are now mapped to named, executed tests.
3. **LF-029** cited only `component_slots_and_props` and `include_variants`. The escaping/XSS
   expectation — the one that actually matters for a template engine — had no citation at all; it is
   now evidenced by five escaping tests, and the fail-closed parsing behaviour is recorded.
4. **LF-030** was "EXTERNAL/ECOSYSTEM" with no evidence. The **manifest half is implemented and
   tested** (six named tests) and the bundler half is external — the item is partially verified, not
   external. The missing CSP-nonce support is recorded (F-6).
5. **LF-031** was "EXTERNAL/ECOSYSTEM" with no evidence. A **starter kit exists** and is proven to
   render/parse/compile (`starter_template_renders_parses_and_compiles`); the item is partially
   verified (no kit choice, e2e not wired).
6. **LF-032**'s "HTMX/Alpine (blueprint sec.26.3)" citation does not hold — the blueprint's exclusion
   list does not mention Livewire (F-5). The exclusion stance is kept; the attribution is corrected.
7. **LF-034** and **LF-035** were "DEFER | Not implemented" with no evidence and no rationale. Both
   now carry the rationale and the nearest-analogue evidence, and LF-034 records that Kelson's
   route-file mechanism already covers the file-based half of the idea.

## 6. Summary

- **8/8 items covered**: **2 VERIFIED** (LF-028 views+composers, LF-029 template engine) ·
  **2 PARTIALLY VERIFIED** (LF-030 Vite manifest, LF-031 starter kits) · **1 EXCLUDED BY DESIGN**
  (LF-032 Livewire) · **3 DEFER / NOT IMPLEMENTED** (LF-033 Inertia, LF-034 Folio, LF-035 Head).
- **76 green results, 0 failures** in this pass (53 views + 9 views-integration + 6 static-assets +
  4 assets + 3 templates + 1 starter scaffold), plus 108 registry + 1 demo route-file run for the
  LF-034 evidence — **185 green results, 0 failures** in total.
- **New findings**: F-4 (dead doc pointer for the `@php`/`@while` divergence), F-5 (Livewire
  exclusion attributed to a blueprint section that does not list it), F-6 (no CSP nonce support).
- **Recorded divergences** (not defects): no PHP eval (`@php`/`@while` refused), no Blade compiler,
  runaway loops fail closed, malformed templates fail closed, unknown asset keys fall back to literal
  URLs.
