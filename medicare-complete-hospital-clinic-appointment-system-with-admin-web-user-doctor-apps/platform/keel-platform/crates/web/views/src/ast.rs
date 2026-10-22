//! Template AST for the views engine (Wave 2). Every node is serde
//! serializable so compiled templates can be cached on disk (the compiled
//! cache pins entries by template mtime + content hash).

use serde::{Deserialize, Serialize};

use crate::expr::{CmpOp, Cond, Expr, Operand, Segment};

/// One parsed template.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Template {
    /// Parent layout name (@extends), if any.
    #[serde(default)]
    pub extends: Option<String>,
    pub nodes: Vec<Node>,
}

/// @foreach loop variables: item plus optional key (map/array index).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopVars {
    pub item: String,
    #[serde(default)]
    pub key: Option<String>,
}

/// @for loop shape (zero-dep interpretation of the Blade forms).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ForSpec {
    /// for(i in 0..10) - numeric range, exclusive end.
    Range {
        var: String,
        from: Operand,
        to: Operand,
    },
    /// for($i = 0; $i < 10; $i++) - restricted C-style loop.
    CStyle {
        var: String,
        init: Operand,
        op: CmpOp,
        bound: Operand,
        step: f64,
    },
}

/// One entry of @class / @style: a value plus an optional condition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttrEntry {
    pub value: String,
    #[serde(default)]
    pub cond: Option<Cond>,
}

/// One AST node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Node {
    /// Literal text.
    Text(String),
    /// {{ expr }} - auto-escaped output.
    Echo(Expr),
    /// {!! expr !!} - raw output (trusted content only).
    Raw(Expr),
    /// @if / @elseif / @else / @endif (@unless desugars to a negated branch).
    If {
        branches: Vec<(Cond, Vec<Node>)>,
        #[serde(default)]
        fallback: Option<Vec<Node>>,
    },
    /// @isset(path) ... @endisset
    Isset {
        path: Vec<Segment>,
        body: Vec<Node>,
    },
    /// @empty(operand) ... @endempty
    Empty {
        operand: Operand,
        body: Vec<Node>,
    },
    /// @auth ... @else ... @endauth (context _auth truthy = authenticated).
    Auth {
        body: Vec<Node>,
        #[serde(default)]
        fallback: Option<Vec<Node>>,
    },
    /// @guest ... @else ... @endguest
    Guest {
        body: Vec<Node>,
        #[serde(default)]
        fallback: Option<Vec<Node>>,
    },
    /// @env("dev") ... @else ... @endenv
    Env {
        name: String,
        body: Vec<Node>,
        #[serde(default)]
        fallback: Option<Vec<Node>>,
    },
    /// @production ... @else ... @endproduction
    Production {
        body: Vec<Node>,
        #[serde(default)]
        fallback: Option<Vec<Node>>,
    },
    /// @foreach / @forelse (empty branch) / @endforeach / @endforelse.
    ForEach {
        vars: LoopVars,
        source: Operand,
        body: Vec<Node>,
        #[serde(default)]
        empty: Option<Vec<Node>>,
    },
    /// @for / @endfor.
    For { spec: ForSpec, body: Vec<Node> },
    /// @once ... @endonce (rendered at most once per render pass).
    Once { body: Vec<Node> },
    /// @push("stack") ... @endpush
    Push { stack: String, body: Vec<Node> },
    /// @stack("stack")
    Stack { name: String },
    /// @vm - bind the view's registered view model into the `vm`
    /// namespace and turn on schema checking for the `vm.*` paths that
    /// follow. The optional argument names the view whose registered
    /// model governs the reads; without it the current view's binding is
    /// used.
    VmBind {
        /// The governing view's name; `None` = the current view.
        #[serde(default)]
        model: Option<String>,
    },
    /// @csrf - hidden token input (context _csrf).
    Csrf,
    /// @method("PUT") - hidden method-spoofing input.
    Method { verb: String },
    /// @error("field") ... @enderror (context _errors.<field>, exposes message).
    Error { field: String, body: Vec<Node> },
    /// @lang("key") - context _lang lookup, falls back to the key.
    Lang { key: String },
    /// @js(expr) - JSON-encoded inline value.
    Js { expr: Expr },
    /// @class([...]) - conditional class attribute.
    Class { entries: Vec<AttrEntry> },
    /// @style([...]) - conditional style attribute.
    Style { entries: Vec<AttrEntry> },
    /// @checked/@selected/@disabled/@readonly/@required(cond).
    AttrIf { attr: String, cond: Cond },
    /// @section("name") ... @endsection / @show.
    Section {
        name: String,
        body: Vec<Node>,
        #[serde(default)]
        show: bool,
    },
    /// @yield("name", "default")
    Yield {
        name: String,
        #[serde(default)]
        default: Option<String>,
    },
    /// @parent inside a section body.
    Parent,
    /// @include / @includeIf / @includeWhen.
    Include {
        name: String,
        #[serde(default)]
        data: Vec<(String, Operand)>,
        #[serde(default)]
        only_if_exists: bool,
        #[serde(default)]
        when: Option<Cond>,
    },
    /// @component("name", {data}) with @slot blocks; body = default slot.
    Component {
        name: String,
        #[serde(default)]
        data: Vec<(String, Operand)>,
        #[serde(default)]
        slots: Vec<(String, Vec<Node>)>,
        body: Vec<Node>,
    },
    /// @props([names]) inside a component template (documents expected props).
    Props { names: Vec<String> },
    /// Custom directive (resolved through the engine registry; unknown
    /// directives fail closed at render time).
    Directive {
        name: String,
        #[serde(default)]
        arg: Option<String>,
    },
}
