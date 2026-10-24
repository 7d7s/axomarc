//! Kelson views engine (Wave 2): a zero-dep, interpreted-AST template engine
//! with Blade-familiar syntax over a JSON context.
//!
//! Deliberately SUPERSEDES Blade's arbitrary PHP eval: @php and @while are
//! refused at parse time (recorded among the parity divergences in
//! `docs/LF-028-TO-LF-035-VERIFICATION.md` §3, LF-029).
//!
//! # Surface
//! - {{ expr }} auto-escaped output; {!! expr !!} raw output. Expressions
//!   are dot-paths with pipe filters (user.name | upper | default("n/a")).
//! - Directive ledger: @extends/@section/@endsection/@yield/@show/@parent,
//!   @include/@includeIf/@includeWhen, @component/@slot/@endslot/@props,
//!   @if/@elseif/@else/@endif/@unless/@isset/@empty, @auth/@guest/@env/
//!   @production, @foreach/@for/@forelse, @once, @push/@stack, @vm,
//!   @command, @csrf/@method, @error/@enderror, @lang, @js, @class/@style/
//!   @checked/@selected/@disabled/@readonly/@required.
//! - The binder: @vm binds the view's registered view model into the
//!   `vm` namespace and turns on schema checking - a path the model does
//!   not declare, or a value of the wrong kind, is a VIEW-RENDER error.
//!   A template without @vm behaves exactly as it did before.
//! - The command binder: @command("op") renders a CSRF-carrying form
//!   bound to the route that declared the operation; an undeclared
//!   command is refused (no free-form dispatch).
//! - The live spine (S-11): an opt-in-per-view stateful session whose
//!   state IS the view's declared view model; every re-render is the
//!   SAME schema-checked render. The live wire (S-12) carries the wire
//!   half: the leaf diff (only the changed parts travel), the session
//!   registry (a closed or never-known id is stale and refused), and —
//!   behind `feature = "live-transport"` — the hub publisher
//!   (`global:live-<id>`, event `live.update`).
//! - Extension points: custom directives, Blade::if-style custom
//!   conditionals, package view namespaces (pkg::view) with a resolution
//!   hook, and view hooks (composer-equivalent callbacks).
//! - Compiled cache: parsed AST serialized under the cache dir, pinned by
//!   template mtime + content hash; kelson view:cache / kelson view:clear.
//!
//! # Context conventions (injected by the web edge)
//! _csrf (token), _auth (principal), _errors (field -> message), _lang
//! (translation table). @env/@production read the engine environment.

pub mod ast;
pub mod builtins;
pub mod engine;
pub mod expr;
pub mod form_bridge;
pub mod lint;
pub mod live;
pub mod live_wire;
mod parser;
pub mod registry;
mod render;
pub mod viewmodel;

pub use ast::{AttrEntry, ForSpec, LoopVars, Node, Template};
pub use builtins::{
    EmptyVm, ErrorVm, FormField, FormOption, FormVm, OpsVm, PaginationVm, TableColumn, TableVm,
    ValidationRule,
};
pub use engine::{
    ConditionalHandler, DirectiveHandler, HookHandler, ResolverFn, ViewError, ViewRender, ViewTap,
    ViewsConfig, ViewsEngine,
};
pub use expr::{
    html_escape, parse_cond, parse_echo, parse_operand_str, parse_path, to_display, CmpOp, Cond,
    Expr, Filter, Lit, Operand, Segment,
};
pub use form_bridge::BridgedForm;
pub use lint::{
    check_paths, check_root_reads, root_reads, vm_declaration, vm_paths, VmDeclaration,
    LINT_ROOT_READ, LINT_UNDECLARED_PATH, RESERVED_CONTEXT_KEYS,
};
pub use live::{LiveRender, LiveSession};
#[cfg(feature = "live-transport")]
pub use live_wire::LiveWire;
pub use live_wire::{state_diff, DiffEntry, LiveId, LiveNode, LiveSessions};
pub use registry::{CommandRegistry, CommandRoute, ViewBinding, ViewRegistry};
pub use viewmodel::{
    check_schema, governing_field, key_path, schema_field, value_at, value_kind, view_model_value,
    FieldKind, FieldSchema, ViewModel, VM_NAMESPACE,
};

/// Maximum iterations for @for/@foreach/@forelse before VIEW-LOOP
/// (fail-closed guard against runaway loops).
pub const MAX_LOOP_ITERATIONS: usize = 100_000;

/// Parse a template source into an AST (parse errors are VIEW-PARSE).
pub fn parse_template(source: &str) -> Result<Template, ViewError> {
    parser::parse(source)
}
