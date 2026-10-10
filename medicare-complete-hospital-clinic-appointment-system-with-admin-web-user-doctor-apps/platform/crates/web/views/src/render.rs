//! AST interpreter for the views engine (Wave 2).
//!
//! Rendering model:
//! - One Renderer per top-level render call: output buffer, scope stack
//!   (loop vars, include/component data), push stacks, @once bookkeeping
//!   and the section table.
//! - Layout inheritance (@extends): the child chain is loaded up front,
//!   top-level sections are collected child-first, child nodes run into a
//!   scratch buffer (side effects like @push still register), then the
//!   layout renders with @yield resolving through the section table.
//!   @parent renders the next section definition in the chain.
//! - Fail-closed guards: VIEW-LOOP on runaway loops, VIEW-RENDER on
//!   runaway nesting, VIEW-NOT-FOUND propagated from discovery.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::{Map, Value};

use crate::ast::{ForSpec, Node, Template};
use crate::engine::{ViewError, ViewsEngine, VIEW_LOOP, VIEW_RENDER};
use crate::expr::{
    apply_filter, compare_values, html_escape, is_empty_value, is_truthy, lit_value,
    resolve_path, to_display, trans_filter, Cond, Expr, Operand, Segment,
};
use crate::MAX_LOOP_ITERATIONS;

/// Maximum include/component nesting depth (fail closed, VIEW-RENDER).
const MAX_DEPTH: usize = 64;

/// One render pass over a template AST.
pub(crate) struct Renderer<'e> {
    engine: &'e ViewsEngine,
    /// Root JSON context supplied by the caller.
    root: Value,
    /// Scoped bindings (loop vars, include/component data); last wins.
    scopes: Vec<Map<String, Value>>,
    out: String,
    /// @push content per stack name, emitted by @stack.
    stacks: BTreeMap<String, String>,
    /// @once nodes already rendered this pass (keyed by node address).
    once_seen: HashSet<usize>,
    /// Section definitions per name, child-most first (extends chains).
    sections: HashMap<String, Vec<Vec<Node>>>,
    /// Rendered section cache (a section renders once per pass).
    section_cache: HashMap<String, String>,
    /// Active (section, next-parent-index) while a section body renders.
    current_parent: Option<(String, usize)>,
    /// True while rendering an extends chain (sections pre-collected).
    layout_mode: bool,
    depth: usize,
}

impl<'e> Renderer<'e> {
    pub(crate) fn new(engine: &'e ViewsEngine, root: Value) -> Self {
        Self {
            engine,
            root,
            scopes: Vec::new(),
            out: String::new(),
            stacks: BTreeMap::new(),
            once_seen: HashSet::new(),
            sections: HashMap::new(),
            section_cache: HashMap::new(),
            current_parent: None,
            layout_mode: false,
            depth: 0,
        }
    }

    // ------------------------------------------------------------- entry

    pub(crate) fn render_template(
        &mut self,
        template: &Template,
        _name: &str,
    ) -> Result<String, ViewError> {
        let mut chain: Vec<Template> = vec![template.clone()];
        while let Some(parent_name) = &chain.last().unwrap().extends {
            if chain.len() > MAX_DEPTH {
                return Err(ViewError::new(
                    VIEW_RENDER,
                    "layout inheritance too deep (possible @extends cycle)",
                ));
            }
            let parent = self.engine.template(parent_name)?;
            chain.push(parent);
        }
        if chain.len() > 1 {
            self.layout_mode = true;
            // Collect top-level sections child-first so @parent resolves
            // toward the layout's definition.
            for t in &chain {
                for node in &t.nodes {
                    if let Node::Section { name, body, .. } = node {
                        self.sections
                            .entry(name.clone())
                            .or_default()
                            .push(body.clone());
                    }
                }
            }
            // Child passes render into scratch: content outside sections is
            // discarded (Blade semantics) but @push/@once side effects stay.
            for t in chain[..chain.len() - 1].iter() {
                let nodes = t.nodes.clone();
                self.render_nodes(&nodes)?;
            }
            self.out.clear();
            let layout = chain.last().unwrap().clone();
            self.render_nodes(&layout.nodes)?;
        } else {
            let nodes = template.nodes.clone();
            self.render_nodes(&nodes)?;
        }
        Ok(std::mem::take(&mut self.out))
    }

    // ---------------------------------------------------------- evaluation

    /// Scope-aware dot-path lookup: scoped bindings shadow the root.
    fn lookup(&self, path: &[Segment]) -> Value {
        if let Some(Segment::Key(key)) = path.first() {
            for scope in self.scopes.iter().rev() {
                if let Some(value) = scope.get(key) {
                    return resolve_path(value, &path[1..])
                        .cloned()
                        .unwrap_or(Value::Null);
                }
            }
        }
        resolve_path(&self.root, path).cloned().unwrap_or(Value::Null)
    }

    fn eval_operand(&self, op: &Operand) -> Value {
        match op {
            Operand::Path(p) => self.lookup(p),
            Operand::Lit(l) => lit_value(l),
        }
    }

    fn eval_expr(&self, expr: &Expr) -> Value {
        let mut value = match &expr.literal {
            Some(lit) => Value::String(lit.clone()),
            None => self.lookup(&expr.path),
        };
        for filter in &expr.filters {
            value = if filter.name == "trans" {
                // Wave 6 P3: catalog-backed key lookup through the injected
                // `_lang` flat map (scope-aware, the `@lang` source).
                trans_filter(value, filter, |key| {
                    let found = self.lookup(&[
                        Segment::Key("_lang".into()),
                        Segment::Key(key.to_string()),
                    ]);
                    if matches!(found, Value::String(_)) {
                        Some(found)
                    } else {
                        None
                    }
                })
            } else {
                apply_filter(value, filter)
            };
        }
        value
    }

    fn eval_cond(&self, cond: &Cond) -> bool {
        match cond {
            Cond::Or(parts) => parts.iter().any(|p| self.eval_cond(p)),
            Cond::And(parts) => parts.iter().all(|p| self.eval_cond(p)),
            Cond::Not(inner) => !self.eval_cond(inner),
            Cond::Cmp { left, op, right } => {
                let l = self.eval_operand(left);
                let r = self.eval_operand(right);
                compare_values(&l, *op, &r)
            }
            Cond::Truthy(op) => is_truthy(&self.eval_operand(op)),
            Cond::Custom { name, args } => {
                let Some(handler) = self.engine.conditional(name) else {
                    return false; // unregistered conditionals fail closed
                };
                let values: Vec<Value> = args.iter().map(|a| self.eval_operand(a)).collect();
                handler(&values, &self.root)
            }
        }
    }

    /// Merge root + scopes into one object (custom directives, includes).
    fn effective_context(&self) -> Value {
        let mut map = self.root.as_object().cloned().unwrap_or_default();
        for scope in &self.scopes {
            for (k, v) in scope {
                map.insert(k.clone(), v.clone());
            }
        }
        Value::Object(map)
    }

    /// Render nodes into a side buffer and return the captured output.
    fn render_capture(&mut self, nodes: &[Node]) -> Result<String, ViewError> {
        let saved = std::mem::take(&mut self.out);
        let result = self.render_nodes(nodes);
        let captured = std::mem::replace(&mut self.out, saved);
        result.map(|_| captured)
    }

    // ----------------------------------------------------------- sections

    fn render_section(&mut self, name: &str) -> Result<String, ViewError> {
        if let Some(cached) = self.section_cache.get(name) {
            return Ok(cached.clone());
        }
        if !self.sections.contains_key(name) {
            return Ok(String::new());
        }
        let rendered = self.render_section_at(name, 0)?;
        self.section_cache.insert(name.to_string(), rendered.clone());
        Ok(rendered)
    }

    /// Render one link of a section chain; @parent descends to index+1.
    fn render_section_at(&mut self, name: &str, idx: usize) -> Result<String, ViewError> {
        let Some(body) = self.sections.get(name).and_then(|c| c.get(idx).cloned()) else {
            return Ok(String::new());
        };
        let saved_parent = self.current_parent.replace((name.to_string(), idx + 1));
        let rendered = self.render_capture(&body)?;
        self.current_parent = saved_parent;
        Ok(rendered)
    }

    // -------------------------------------------------------------- nodes

    fn render_nodes(&mut self, nodes: &[Node]) -> Result<(), ViewError> {
        'outer: for node in nodes {
            match node {
                Node::Text(text) => self.out.push_str(text),
                Node::Echo(expr) => {
                    let value = self.eval_expr(expr);
                    self.out.push_str(&html_escape(&to_display(&value)));
                }
                Node::Raw(expr) => {
                    let value = self.eval_expr(expr);
                    self.out.push_str(&to_display(&value));
                }
                Node::If { branches, fallback } => {
                    for (cond, body) in branches {
                        if self.eval_cond(cond) {
                            self.render_nodes(body)?;
                            continue 'outer;
                        }
                    }
                    if let Some(fb) = fallback {
                        self.render_nodes(fb)?;
                    }
                }
                Node::Isset { path, body } => {
                    if !self.lookup(path).is_null() {
                        self.render_nodes(body)?;
                    }
                }
                Node::Empty { operand, body } => {
                    if is_empty_value(&self.eval_operand(operand)) {
                        self.render_nodes(body)?;
                    }
                }
                Node::Auth { body, fallback } => {
                    let authed = is_truthy(&self.lookup(&[Segment::Key("_auth".into())]));
                    let branch = if authed { Some(body) } else { fallback.as_ref() };
                    if let Some(b) = branch {
                        self.render_nodes(b)?;
                    }
                }
                Node::Guest { body, fallback } => {
                    let authed = is_truthy(&self.lookup(&[Segment::Key("_auth".into())]));
                    let branch = if authed { fallback.as_ref() } else { Some(body) };
                    if let Some(b) = branch {
                        self.render_nodes(b)?;
                    }
                }
                Node::Env { name, body, fallback } => {
                    let hit = self.engine.config().env == *name;
                    let branch = if hit { Some(body) } else { fallback.as_ref() };
                    if let Some(b) = branch {
                        self.render_nodes(b)?;
                    }
                }
                Node::Production { body, fallback } => {
                    let hit = self.engine.config().env == "production";
                    let branch = if hit { Some(body) } else { fallback.as_ref() };
                    if let Some(b) = branch {
                        self.render_nodes(b)?;
                    }
                }
                Node::ForEach { vars, source, body, empty } => {
                    let value = self.eval_operand(source);
                    let mut items: Vec<(Value, Value)> = Vec::new();
                    match &value {
                        Value::Array(a) => {
                            for (i, v) in a.iter().enumerate() {
                                items.push((Value::Number(i.into()), v.clone()));
                            }
                        }
                        Value::Object(o) => {
                            for (k, v) in o {
                                items.push((Value::String(k.clone()), v.clone()));
                            }
                        }
                        _ => {}
                    }
                    if items.is_empty() {
                        if let Some(eb) = empty {
                            self.render_nodes(eb)?;
                        }
                    } else {
                        let mut count = 0usize;
                        for (key, item) in items {
                            count += 1;
                            if count > MAX_LOOP_ITERATIONS {
                                return Err(ViewError::new(
                                    VIEW_LOOP,
                                    format!(
                                        "loop over {} exceeded {MAX_LOOP_ITERATIONS} iterations",
                                        vars.item
                                    ),
                                ));
                            }
                            let mut frame = Map::new();
                            frame.insert(vars.item.clone(), item);
                            if let Some(k) = &vars.key {
                                frame.insert(k.clone(), key);
                            }
                            self.scopes.push(frame);
                            let result = self.render_nodes(body);
                            self.scopes.pop();
                            result?;
                        }
                    }
                }
                Node::For { spec, body } => self.render_for(spec, body)?,
                Node::Once { body } => {
                    let key = node as *const Node as usize;
                    if self.once_seen.insert(key) {
                        self.render_nodes(body)?;
                    }
                }
                Node::Push { stack, body } => {
                    let captured = self.render_capture(body)?;
                    self.stacks.entry(stack.clone()).or_default().push_str(&captured);
                }
                Node::Stack { name } => {
                    if let Some(content) = self.stacks.get(name) {
                        self.out.push_str(content);
                    }
                }
                Node::Csrf => {
                    let token = self.lookup(&[Segment::Key("_csrf".into())]);
                    self.out.push_str(&format!(
                        "<input type=\"hidden\" name=\"_token\" value=\"{}\">",
                        html_escape(&to_display(&token))
                    ));
                }
                Node::Method { verb } => {
                    self.out.push_str(&format!(
                        "<input type=\"hidden\" name=\"_method\" value=\"{}\">",
                        html_escape(verb)
                    ));
                }
                Node::Error { field, body } => {
                    let message = self.lookup(&[
                        Segment::Key("_errors".into()),
                        Segment::Key(field.clone()),
                    ]);
                    if !message.is_null() {
                        let mut frame = Map::new();
                        frame.insert("message".into(), message);
                        self.scopes.push(frame);
                        let result = self.render_nodes(body);
                        self.scopes.pop();
                        result?;
                    }
                }
                Node::Lang { key } => {
                    let value = self.lookup(&[
                        Segment::Key("_lang".into()),
                        Segment::Key(key.clone()),
                    ]);
                    match value {
                        Value::String(s) => self.out.push_str(&s),
                        _ => self.out.push_str(key),
                    }
                }
                Node::Js { expr } => {
                    let value = self.eval_expr(expr);
                    self.out.push_str(&js_encode(&value));
                }
                Node::Class { entries } => {
                    let parts = self.attr_entries(entries);
                    if !parts.is_empty() {
                        self.out
                            .push_str(&format!("class=\"{}\"", html_escape(&parts.join(" "))));
                    }
                }
                Node::Style { entries } => {
                    let parts = self.attr_entries(entries);
                    if !parts.is_empty() {
                        self.out
                            .push_str(&format!("style=\"{}\"", html_escape(&parts.join("; "))));
                    }
                }
                Node::AttrIf { attr, cond } => {
                    if self.eval_cond(cond) {
                        self.out.push_str(attr);
                    }
                }
                Node::Section { name, body, show } => {
                    if !self.layout_mode {
                        self.sections
                            .entry(name.clone())
                            .or_insert_with(|| vec![body.clone()]);
                    }
                    if *show {
                        let rendered = self.render_section(name)?;
                        self.out.push_str(&rendered);
                    }
                }
                Node::Yield { name, default } => {
                    if self.sections.contains_key(name) {
                        let rendered = self.render_section(name)?;
                        self.out.push_str(&rendered);
                    } else if let Some(d) = default {
                        self.out.push_str(d);
                    }
                }
                Node::Parent => {
                    if let Some((name, idx)) = self.current_parent.clone() {
                        let rendered = self.render_section_at(&name, idx)?;
                        self.out.push_str(&rendered);
                    }
                }
                Node::Include { name, data, only_if_exists, when } => {
                    if let Some(cond) = when {
                        if !self.eval_cond(cond) {
                            continue 'outer;
                        }
                    }
                    if let Err(e) = self.engine.resolve(name) {
                        if *only_if_exists {
                            continue 'outer;
                        }
                        return Err(e);
                    }
                    if self.depth >= MAX_DEPTH {
                        return Err(ViewError::new(
                            VIEW_RENDER,
                            format!("include nesting too deep at {name:?}"),
                        ));
                    }
                    let mut ctx = self.effective_context();
                    if let Some(obj) = ctx.as_object_mut() {
                        for (k, v) in data {
                            obj.insert(k.clone(), self.eval_operand(v));
                        }
                    }
                    for hook in self.engine.hooks_for(name) {
                        hook(&mut ctx);
                    }
                    let frame = ctx.as_object().cloned().unwrap_or_default();
                    let tpl = self.engine.template(name)?;
                    let nodes = tpl.nodes;
                    self.scopes.push(frame);
                    self.depth += 1;
                    let result = self.render_nodes(&nodes);
                    self.depth -= 1;
                    self.scopes.pop();
                    result?;
                }
                Node::Component { name, data, slots, body } => {
                    if self.depth >= MAX_DEPTH {
                        return Err(ViewError::new(
                            VIEW_RENDER,
                            format!("component nesting too deep at {name:?}"),
                        ));
                    }
                    let mut ctx = self.effective_context();
                    if let Some(obj) = ctx.as_object_mut() {
                        for (k, v) in data {
                            obj.insert(k.clone(), self.eval_operand(v));
                        }
                    }
                    let default_slot = self.render_capture(body)?;
                    if let Some(obj) = ctx.as_object_mut() {
                        obj.insert("slot".into(), Value::String(default_slot));
                        for (slot_name, slot_nodes) in slots {
                            let rendered = self.render_capture(slot_nodes)?;
                            obj.insert(slot_name.clone(), Value::String(rendered));
                        }
                    }
                    for hook in self.engine.hooks_for(name) {
                        hook(&mut ctx);
                    }
                    let frame = ctx.as_object().cloned().unwrap_or_default();
                    let tpl = self.engine.template(name)?;
                    let nodes = tpl.nodes;
                    self.scopes.push(frame);
                    self.depth += 1;
                    let result = self.render_nodes(&nodes);
                    self.depth -= 1;
                    self.scopes.pop();
                    result?;
                }
                Node::Props { .. } => {
                    // Documentation-only inside component templates.
                }
                Node::Directive { name, arg } => {
                    if let Some(handler) = self.engine.directive(name) {
                        let ctx = self.effective_context();
                        let rendered = handler(name, arg.as_deref(), &ctx)?;
                        self.out.push_str(&rendered);
                    } else {
                        // Unregistered directives render literally (Blade
                        // parity; keeps CSS at-rules like @media intact).
                        self.out.push('@');
                        self.out.push_str(name);
                        if let Some(a) = arg {
                            self.out.push('(');
                            self.out.push_str(a);
                            self.out.push(')');
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn attr_entries(&self, entries: &[crate::ast::AttrEntry]) -> Vec<String> {
        entries
            .iter()
            .filter(|e| e.cond.as_ref().map(|c| self.eval_cond(c)).unwrap_or(true))
            .map(|e| e.value.clone())
            .collect()
    }

    fn render_for(&mut self, spec: &ForSpec, body: &[Node]) -> Result<(), ViewError> {
        match spec {
            ForSpec::Range { var, from, to } => {
                let (Some(start), Some(end)) = (
                    self.eval_operand(from).as_f64(),
                    self.eval_operand(to).as_f64(),
                ) else {
                    return Ok(());
                };
                let mut i = start;
                let mut count = 0usize;
                while i < end {
                    count += 1;
                    if count > MAX_LOOP_ITERATIONS {
                        return Err(ViewError::new(
                            VIEW_LOOP,
                            format!("loop {var} exceeded {MAX_LOOP_ITERATIONS} iterations"),
                        ));
                    }
                    let mut frame = Map::new();
                    frame.insert(var.clone(), num_value(i));
                    self.scopes.push(frame);
                    let result = self.render_nodes(body);
                    self.scopes.pop();
                    result?;
                    i += 1.0;
                }
            }
            ForSpec::CStyle { var, init, op, bound, step } => {
                if *step == 0.0 {
                    return Err(ViewError::new(
                        VIEW_LOOP,
                        format!("loop {var} has a zero step and would never terminate"),
                    ));
                }
                let mut i = self.eval_operand(init).as_f64().unwrap_or(0.0);
                let mut count = 0usize;
                loop {
                    let bound_value = self.eval_operand(bound);
                    let var_value = num_value(i);
                    if !compare_values(&var_value, *op, &bound_value) {
                        break;
                    }
                    count += 1;
                    if count > MAX_LOOP_ITERATIONS {
                        return Err(ViewError::new(
                            VIEW_LOOP,
                            format!("loop {var} exceeded {MAX_LOOP_ITERATIONS} iterations"),
                        ));
                    }
                    let mut frame = Map::new();
                    frame.insert(var.clone(), var_value);
                    self.scopes.push(frame);
                    let result = self.render_nodes(body);
                    self.scopes.pop();
                    result?;
                    i += step;
                }
            }
        }
        Ok(())
    }
}

/// Loop counters render as integers when they are whole numbers.
fn num_value(f: f64) -> Value {
    if f.fract() == 0.0 && f.abs() <= i64::MAX as f64 {
        Value::Number((f as i64).into())
    } else {
        serde_json::Number::from_f64(f)
            .map(Value::Number)
            .unwrap_or(Value::Null)
    }
}

/// JSON-encode a value for inline use, escaping HTML-significant chars so
/// the output is safe inside a script body (G17).
fn js_encode(value: &Value) -> String {
    let json = serde_json::to_string(value).unwrap_or_else(|_| "null".to_string());
    json.replace('&', "\\u0026")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn render_str(template: &str, ctx: Value) -> Result<String, ViewError> {
        let dir = std::env::temp_dir().join(format!(
            "kelson-views-render-{}",
            std::process::id()
        ));
        let views = dir.join("resources").join("views");
        std::fs::create_dir_all(&views).unwrap();
        let config = crate::engine::ViewsConfig {
            paths: vec![views],
            cache_dir: None,
            env: "testing".into(),
        };
        let engine = crate::engine::ViewsEngine::new(config);
        let tpl = crate::parse_template(template)?;
        let mut renderer = Renderer::new(&engine, ctx);
        renderer.render_template(&tpl, "inline")
    }

    #[test]
    fn scopes_shadow_the_root_context() {
        let out = render_str(
            "@foreach(items as item){{ item }}@endforeach",
            json!({ "items": ["x", "y"], "item": "root" }),
        )
        .unwrap();
        assert_eq!(out, "xy");
    }

    #[test]
    fn js_encoding_escapes_html_significant_chars() {
        let out = render_str("@js(payload)", json!({ "payload": {"k": "<a>&"} })).unwrap();
        assert!(!out.contains('<'));
        assert!(!out.contains('>'));
        assert!(out.contains("\\u003c"));
        assert!(out.contains("\\u0026"));
    }

    #[test]
    fn sections_without_extends_define_then_yield() {
        let out = render_str(
            "@section(\"greet\")hi@endsection[@yield(\"greet\")][@yield(\"missing\", \"dflt\")]",
            json!({}),
        )
        .unwrap();
        assert_eq!(out, "[hi][dflt]");
    }

    #[test]
    fn show_emits_the_section_in_place() {
        let out = render_str("@section(\"x\")X@show<@yield(\"x\")>", json!({})).unwrap();
        assert_eq!(out, "X<X>");
    }
}
