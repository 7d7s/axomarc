//! The template lint: the build-time half of the binder.
//!
//! The binder's render-time check refuses a `vm.*` path the view model
//! does not declare — but only once the page is served. The lint refuses
//! the same mistake **before** the page is ever served: a template that
//! reads a path its model does not declare is a broken build, not a
//! broken page.
//!
//! The lint and the render check share one rule book — the same
//! [`governing_field`] lookup — so the two can never disagree about what
//! a template is allowed to read.
//!
//! What the lint reads is the **template**, not the context: the paths
//! are found in the AST, so the lint needs no model instance and no
//! request.

use crate::ast::{ForSpec, Node};
use crate::engine::{ViewError, VIEW_RENDER};
use crate::expr::{Cond, Operand, Segment};
use crate::viewmodel::{governing_field, key_path, FieldSchema, VM_NAMESPACE};

/// The binder declaration a template carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmDeclaration {
    /// The view whose registered view model governs the template's
    /// `vm.*` reads; `None` = the current view.
    pub view: Option<String>,
}

/// The `@vm` declaration in a node tree, if the template has one.
///
/// The first declaration found wins: a template declares what it reads,
/// and the outermost declaration is the template's own.
pub fn vm_declaration(nodes: &[Node]) -> Option<VmDeclaration> {
    for node in nodes {
        if let Node::VmBind { model } = node {
            return Some(VmDeclaration {
                view: model.clone(),
            });
        }
        for body in bodies(node) {
            if let Some(found) = vm_declaration(body) {
                return Some(found);
            }
        }
    }
    None
}

/// Every `vm.*` path the template reads, as dotted key paths.
pub fn vm_paths(nodes: &[Node]) -> Vec<String> {
    let mut out = Vec::new();
    collect_paths(nodes, &mut out);
    out
}

/// Check the `vm.*` paths of a template against a declaration.
///
/// The rule is the render's rule minus the value: a path the model does
/// not declare is refused; a path nested under a declared object or list
/// is allowed.
pub fn check_paths(
    schema: &[FieldSchema],
    paths: &[String],
    view: &str,
) -> Result<(), ViewError> {
    for path in paths {
        if governing_field(schema, path).is_none() {
            return Err(ViewError::new(
                VIEW_RENDER,
                format!("the view model for {view:?} does not declare the path {path:?}"),
            ));
        }
    }
    Ok(())
}

/// The node bodies a node carries (the templates nested inside it).
fn bodies(node: &Node) -> Vec<&[Node]> {
    match node {
        Node::If { branches, fallback } => {
            let mut out: Vec<&[Node]> = branches.iter().map(|(_, body)| body.as_slice()).collect();
            if let Some(fb) = fallback {
                out.push(fb.as_slice());
            }
            out
        }
        Node::Auth { body, fallback }
        | Node::Guest { body, fallback }
        | Node::Env { body, fallback, .. }
        | Node::Production { body, fallback, .. } => {
            let mut out = vec![body.as_slice()];
            if let Some(fb) = fallback {
                out.push(fb.as_slice());
            }
            out
        }
        Node::Isset { body, .. }
        | Node::Empty { body, .. }
        | Node::Once { body }
        | Node::Push { body, .. }
        | Node::Error { body, .. }
        | Node::Section { body, .. }
        | Node::For { body, .. } => vec![body.as_slice()],
        Node::ForEach { body, empty, .. } => {
            let mut out = vec![body.as_slice()];
            if let Some(empty) = empty {
                out.push(empty.as_slice());
            }
            out
        }
        Node::Component { body, slots, .. } => {
            let mut out: Vec<&[Node]> = slots.iter().map(|(_, body)| body.as_slice()).collect();
            out.push(body.as_slice());
            out
        }
        _ => Vec::new(),
    }
}

/// Collect the `vm.*` paths a node tree reads.
fn collect_paths(nodes: &[Node], out: &mut Vec<String>) {
    for node in nodes {
        match node {
            Node::Echo(expr) | Node::Raw(expr) => push_expr(expr, out),
            Node::Js { expr } => push_expr(expr, out),
            Node::Isset { path, .. } => push_path(path, out),
            Node::Empty { operand, .. } => push_operand(operand, out),
            Node::If { branches, .. } => {
                for (cond, _) in branches {
                    push_cond(cond, out);
                }
            }
            Node::AttrIf { cond, .. } => push_cond(cond, out),
            Node::Class { entries } | Node::Style { entries } => {
                for entry in entries {
                    if let Some(cond) = &entry.cond {
                        push_cond(cond, out);
                    }
                }
            }
            Node::ForEach { source, .. } => push_operand(source, out),
            Node::For { spec, .. } => match spec {
                ForSpec::Range { from, to, .. } => {
                    push_operand(from, out);
                    push_operand(to, out);
                }
                ForSpec::CStyle { init, bound, .. } => {
                    push_operand(init, out);
                    push_operand(bound, out);
                }
            },
            Node::Include { data, when, .. } => {
                for (_, operand) in data {
                    push_operand(operand, out);
                }
                if let Some(cond) = when {
                    push_cond(cond, out);
                }
            }
            Node::Component { data, .. } => {
                for (_, operand) in data {
                    push_operand(operand, out);
                }
            }
            _ => {}
        }
        for body in bodies(node) {
            collect_paths(body, out);
        }
    }
}

/// The path of an echo expression (a literal reads nothing).
fn push_expr(expr: &crate::expr::Expr, out: &mut Vec<String>) {
    if expr.literal.is_some() {
        return;
    }
    push_path(&expr.path, out);
}

/// A `vm.*` path, as a dotted key path.
fn push_path(path: &[Segment], out: &mut Vec<String>) {
    let Some(Segment::Key(first)) = path.first() else {
        return;
    };
    if first != VM_NAMESPACE {
        return;
    }
    let dotted = key_path(&path[1..]);
    if dotted.is_empty() {
        return;
    }
    out.push(dotted);
}

fn push_operand(op: &Operand, out: &mut Vec<String>) {
    if let Operand::Path(path) = op {
        push_path(path, out);
    }
}

fn push_cond(cond: &Cond, out: &mut Vec<String>) {
    match cond {
        Cond::Or(parts) | Cond::And(parts) => {
            for part in parts {
                push_cond(part, out);
            }
        }
        Cond::Not(inner) => push_cond(inner, out),
        Cond::Cmp { left, right, .. } => {
            push_operand(left, out);
            push_operand(right, out);
        }
        Cond::Truthy(op) => push_operand(op, out),
        Cond::Custom { args, .. } => {
            for arg in args {
                push_operand(arg, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_template;
    use crate::viewmodel::{FieldKind, FieldSchema};

    fn nodes(source: &str) -> Vec<Node> {
        parse_template(source).expect("the template parses").nodes
    }

    #[test]
    fn a_template_without_the_directive_declares_nothing() {
        assert!(vm_declaration(&nodes("<p>{{ vm.title }}</p>")).is_none());
    }

    #[test]
    fn the_directive_is_found_in_a_nested_body() {
        let declaration = vm_declaration(&nodes("@if(x)@vm@endif")).expect("the directive is found");
        assert_eq!(declaration.view, None);
        let named = vm_declaration(&nodes("@if(x)@vm(\"other.view\")@endif")).expect("found");
        assert_eq!(named.view, Some("other.view".to_string()));
    }

    #[test]
    fn every_vm_path_the_template_reads_is_found() {
        let paths = vm_paths(&nodes(
            "{{ vm.title }}@if(vm.count > 1){{ vm.rows[0].name }}@endif@isset(vm.flag)x@endisset",
        ));
        assert!(paths.contains(&"title".to_string()), "{paths:?}");
        assert!(paths.contains(&"count".to_string()), "{paths:?}");
        assert!(paths.contains(&"rows.name".to_string()), "{paths:?}");
        assert!(paths.contains(&"flag".to_string()), "{paths:?}");
    }

    #[test]
    fn a_path_outside_the_namespace_is_not_the_models_business() {
        let paths = vm_paths(&nodes("{{ user.name }}{{ _lang.hello }}"));
        assert!(paths.is_empty(), "{paths:?}");
    }

    #[test]
    fn an_undeclared_path_is_refused_by_the_check() {
        let schema = crate::view_schema![FieldSchema::required("title", FieldKind::String)];
        assert!(check_paths(schema, &["title".to_string()], "page").is_ok());
        let error = check_paths(schema, &["ghost".to_string()], "page")
            .expect_err("an undeclared path is refused");
        assert_eq!(error.code, VIEW_RENDER);
        assert!(error.message.contains("ghost"), "{}", error.message);
    }
}
