//! Hand-written parser for the views template language (Wave 2).
//!
//! Emits the serde-serializable AST from ast.rs. Fail-closed: every parse
//! problem is a VIEW-PARSE error carrying the offending construct. The
//! SUPERSEDED directives (@php, @while) are refused here and documented in
//! docs/LF-028-TO-LF-035-VERIFICATION.md §3 (LF-029, "Recorded divergences").

use crate::ast::{AttrEntry, ForSpec, LoopVars, Node, Template};
use crate::engine::{ViewError, VIEW_PARSE};
use crate::expr::{parse_cond, parse_echo, parse_operand_str, parse_path, CmpOp, Cond, Operand};

/// Directive names in the closed ledger (plus the SUPERSEDED pair, which
/// parses as a name only to produce a targeted refusal).
fn is_known(name: &str) -> bool {
    matches!(
        name,
        "extends"
            | "section"
            | "endsection"
            | "show"
            | "yield"
            | "parent"
            | "include"
            | "includeIf"
            | "includeWhen"
            | "component"
            | "endcomponent"
            | "slot"
            | "endslot"
            | "props"
            | "if"
            | "elseif"
            | "else"
            | "endif"
            | "unless"
            | "endunless"
            | "isset"
            | "endisset"
            | "empty"
            | "endempty"
            | "auth"
            | "endauth"
            | "guest"
            | "endguest"
            | "env"
            | "endenv"
            | "production"
            | "endproduction"
            | "foreach"
            | "endforeach"
            | "for"
            | "endfor"
            | "forelse"
            | "endforelse"
            | "once"
            | "endonce"
            | "push"
            | "endpush"
            | "stack"
            | "vm"
            | "command"
            | "csrf"
            | "method"
            | "error"
            | "enderror"
            | "lang"
            | "js"
            | "class"
            | "style"
            | "checked"
            | "selected"
            | "disabled"
            | "readonly"
            | "required"
            | "php"
            | "while"
    )
}

/// Parse a template source into an AST.
pub fn parse(source: &str) -> Result<Template, ViewError> {
    let mut p = Parser {
        chars: source.chars().collect(),
        pos: 0,
    };
    let mut extends = None;
    let nodes = p.parse_nodes(&[], &mut extends)?;
    Ok(Template { extends, nodes })
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
}

impl Parser {
    fn eof(&self) -> bool {
        self.pos >= self.chars.len()
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, off: usize) -> Option<char> {
        self.chars.get(self.pos + off).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    fn starts(&self, s: &str) -> bool {
        for (i, c) in s.chars().enumerate() {
            if self.chars.get(self.pos + i) != Some(&c) {
                return false;
            }
        }
        true
    }

    fn err(&self, message: &str) -> ViewError {
        ViewError::new(VIEW_PARSE, message.to_string())
    }

    fn flush(text: &mut String, nodes: &mut Vec<Node>) {
        if !text.is_empty() {
            nodes.push(Node::Text(std::mem::take(text)));
        }
    }

    /// Read literal text until the next echo/directive token, then parse
    /// that token. Returns at a stop directive WITHOUT consuming it.
    fn parse_nodes(
        &mut self,
        stops: &[&str],
        extends_slot: &mut Option<String>,
    ) -> Result<Vec<Node>, ViewError> {
        let mut nodes = Vec::new();
        let mut text = String::new();
        loop {
            if self.eof() {
                if stops.is_empty() {
                    Self::flush(&mut text, &mut nodes);
                    return Ok(nodes);
                }
                let expected = stops
                    .iter()
                    .map(|s| format!("@{s}"))
                    .collect::<Vec<_>>()
                    .join(" or ");
                return Err(self.err(&format!(
                    "unexpected end of template; expected {expected}"
                )));
            }
            if self.starts("{!!") {
                Self::flush(&mut text, &mut nodes);
                self.pos += 3;
                let src = self.read_until("!!}")?;
                let expr = parse_echo(src.trim()).map_err(|e| self.err(&e))?;
                nodes.push(Node::Raw(expr));
                continue;
            }
            if self.starts("{{") {
                Self::flush(&mut text, &mut nodes);
                self.pos += 2;
                let src = self.read_until("}}")?;
                let expr = parse_echo(src.trim()).map_err(|e| self.err(&e))?;
                nodes.push(Node::Echo(expr));
                continue;
            }
            if self.peek() == Some('@') {
                if self.starts("@@") {
                    text.push('@');
                    self.pos += 2;
                    continue;
                }
                if self.starts("@{{") {
                    text.push_str("{{");
                    self.pos += 3;
                    continue;
                }
                let ident = self
                    .peek_at(1)
                    .map(|c| c.is_ascii_alphabetic() || c == '_')
                    .unwrap_or(false);
                if ident {
                    let save = self.pos;
                    let (name, arg) = self.read_directive()?;
                    if stops.contains(&name.as_str()) {
                        self.pos = save;
                        Self::flush(&mut text, &mut nodes);
                        return Ok(nodes);
                    }
                    if !is_known(&name) && arg.is_none() {
                        // Literal text (email addresses etc.); only parenthesized
                        // unknown names become custom directives.
                        text.push('@');
                        text.push_str(&name);
                        continue;
                    }
                    Self::flush(&mut text, &mut nodes);
                    if let Some(node) = self.directive_node(&name, arg, extends_slot)? {
                        nodes.push(node);
                    }
                    continue;
                }
            }
            text.push(self.bump().unwrap());
        }
    }

    fn read_until(&mut self, term: &str) -> Result<String, ViewError> {
        let mut out = String::new();
        loop {
            if self.eof() {
                return Err(self.err(&format!("unterminated expression; expected {term}")));
            }
            if self.starts(term) {
                self.pos += term.chars().count();
                return Ok(out);
            }
            out.push(self.bump().unwrap());
        }
    }

    /// Consume @name plus an optional balanced (...) argument, returning the
    /// raw argument text (quotes preserved).
    fn read_directive(&mut self) -> Result<(String, Option<String>), ViewError> {
        self.pos += 1; // '@'
        let mut name = String::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == '_' {
                name.push(c);
                self.pos += 1;
            } else {
                break;
            }
        }
        if name.is_empty() {
            return Err(self.err("expected a directive name after @"));
        }
        let mut probe = self.pos;
        while probe < self.chars.len() && self.chars[probe].is_whitespace() {
            probe += 1;
        }
        if probe < self.chars.len() && self.chars[probe] == '(' {
            self.pos = probe;
            let arg = self.capture_balanced()?;
            Ok((name, Some(arg)))
        } else {
            Ok((name, None))
        }
    }

    /// Capture the text inside balanced parens (strings respected).
    fn capture_balanced(&mut self) -> Result<String, ViewError> {
        self.pos += 1; // '('
        let mut depth = 0i32;
        let mut in_str: Option<char> = None;
        let mut out = String::new();
        loop {
            let Some(c) = self.bump() else {
                return Err(self.err("unterminated directive arguments"));
            };
            match in_str {
                Some(q) => {
                    if c == '\\' {
                        out.push(c);
                        if let Some(e) = self.bump() {
                            out.push(e);
                        }
                        continue;
                    }
                    if c == q {
                        in_str = None;
                    }
                    out.push(c);
                }
                None => {
                    if c == '"' || c == '\'' {
                        in_str = Some(c);
                        out.push(c);
                    } else if c == '(' || c == '[' || c == '{' {
                        depth += 1;
                        out.push(c);
                    } else if c == ')' || c == ']' || c == '}' {
                        if c == ')' && depth == 0 {
                            return Ok(out);
                        }
                        depth -= 1;
                        out.push(c);
                    } else {
                        out.push(c);
                    }
                }
            }
        }
    }

    fn expect_end(&mut self, expected: &str) -> Result<(), ViewError> {
        let (name, _) = self.read_directive()?;
        if name != expected {
            return Err(self.err(&format!("expected @{expected}, found @{name}")));
        }
        Ok(())
    }

    fn directive_node(
        &mut self,
        name: &str,
        arg: Option<String>,
        es: &mut Option<String>,
    ) -> Result<Option<Node>, ViewError> {
        match name {
            "extends" => {
                let layout = parse_string_arg(arg, name, &self.loc())?;
                if es.is_some() {
                    return Err(self.err("duplicate @extends in one template"));
                }
                *es = Some(layout);
                Ok(None)
            }
            "section" => self.parse_section(arg, es),
            "yield" => {
                let parts = split_args(&arg_required(arg, name, &self.loc())?);
                let yname = unquote(
                    parts
                        .first()
                        .ok_or_else(|| self.err("@yield requires a section name"))?
                        .trim(),
                );
                if yname.is_empty() {
                    return Err(self.err("@yield requires a section name"));
                }
                let default = parts.get(1).map(|d| unquote(d.trim()));
                Ok(Some(Node::Yield {
                    name: yname,
                    default,
                }))
            }
            "parent" => Ok(Some(Node::Parent)),
            "include" => self.parse_include(arg, false, false),
            "includeIf" => self.parse_include(arg, true, false),
            "includeWhen" => self.parse_include(arg, false, true),
            "component" => self.parse_component(arg, es),
            "props" => {
                let inner = strip_brackets(&arg_required(arg, name, &self.loc())?);
                let names = split_all_top(&inner, ',')
                    .iter()
                    .map(|p| unquote(p.trim()))
                    .filter(|s| !s.is_empty())
                    .collect();
                Ok(Some(Node::Props { names }))
            }
            "if" => self.parse_if(arg, false, es),
            "unless" => self.parse_if(arg, true, es),
            "isset" => {
                let path = parse_path(&arg_required(arg, name, &self.loc())?);
                if path.is_empty() {
                    return Err(self.err("@isset requires a variable path"));
                }
                let body = self.parse_nodes(&["endisset"], es)?;
                self.expect_end("endisset")?;
                Ok(Some(Node::Isset { path, body }))
            }
            "empty" => {
                let operand = parse_operand_str(&arg_required(arg, name, &self.loc())?)
                    .map_err(|e| self.err(&e))?;
                let body = self.parse_nodes(&["endempty"], es)?;
                self.expect_end("endempty")?;
                Ok(Some(Node::Empty { operand, body }))
            }
            "auth" => {
                let (body, fallback) = self.parse_body_with_else("endauth", es)?;
                Ok(Some(Node::Auth { body, fallback }))
            }
            "guest" => {
                let (body, fallback) = self.parse_body_with_else("endguest", es)?;
                Ok(Some(Node::Guest { body, fallback }))
            }
            "env" => {
                let env_name = parse_string_arg(arg, name, &self.loc())?;
                let (body, fallback) = self.parse_body_with_else("endenv", es)?;
                Ok(Some(Node::Env {
                    name: env_name,
                    body,
                    fallback,
                }))
            }
            "production" => {
                let (body, fallback) = self.parse_body_with_else("endproduction", es)?;
                Ok(Some(Node::Production { body, fallback }))
            }
            "foreach" => {
                let (vars, source) =
                    parse_foreach_arg(&arg_required(arg, name, &self.loc())?)
                        .map_err(|e| self.err(&e))?;
                let body = self.parse_nodes(&["endforeach"], es)?;
                self.expect_end("endforeach")?;
                Ok(Some(Node::ForEach {
                    vars,
                    source,
                    body,
                    empty: None,
                }))
            }
            "forelse" => {
                let (vars, source) =
                    parse_foreach_arg(&arg_required(arg, name, &self.loc())?)
                        .map_err(|e| self.err(&e))?;
                let body = self.parse_nodes(&["empty", "endforelse"], es)?;
                let (stop, _) = self.read_directive()?;
                let empty = if stop == "empty" {
                    let branch = self.parse_nodes(&["endforelse"], es)?;
                    self.expect_end("endforelse")?;
                    Some(branch)
                } else {
                    None
                };
                Ok(Some(Node::ForEach {
                    vars,
                    source,
                    body,
                    empty,
                }))
            }
            "for" => {
                let spec = parse_for_arg(&arg_required(arg, name, &self.loc())?)
                    .map_err(|e| self.err(&e))?;
                let body = self.parse_nodes(&["endfor"], es)?;
                self.expect_end("endfor")?;
                Ok(Some(Node::For { spec, body }))
            }
            "once" => {
                let body = self.parse_nodes(&["endonce"], es)?;
                self.expect_end("endonce")?;
                Ok(Some(Node::Once { body }))
            }
            "push" => {
                let stack = parse_string_arg(arg, name, &self.loc())?;
                let body = self.parse_nodes(&["endpush"], es)?;
                self.expect_end("endpush")?;
                Ok(Some(Node::Push { stack, body }))
            }
            "stack" => {
                let stack = parse_string_arg(arg, name, &self.loc())?;
                Ok(Some(Node::Stack { name: stack }))
            }
            "vm" => {
                // `@vm` binds the current view's registered view model;
                // `@vm("other.view")` names the view whose registered model
                // governs the reads under the `vm` namespace.
                let model = match arg {
                    None => None,
                    Some(raw) => {
                        let parts = split_args(&raw);
                        let name = unquote(
                            parts
                                .first()
                                .ok_or_else(|| self.err("@vm requires a view name"))?
                                .trim(),
                        );
                        if name.is_empty() {
                            return Err(self.err("@vm requires a view name"));
                        }
                        Some(name)
                    }
                };
                Ok(Some(Node::VmBind { model }))
            }
            "command" => {
                // `@command("op")` binds to the operation a route declared;
                // `@command("op", label="Cancel")` carries a button label.
                // Unknown keyword arguments are refused (fail-closed), so a
                // typo'd option is a parse error, not a silently dropped
                // attribute.
                let raw = arg_required(arg, name, &self.loc())?;
                let mut parts = split_args(&raw).into_iter();
                let op = unquote(
                    parts
                        .next()
                        .ok_or_else(|| self.err("@command requires an operation id"))?
                        .trim(),
                );
                if op.is_empty() {
                    return Err(self.err("@command requires an operation id"));
                }
                let mut label: Option<String> = None;
                for part in parts {
                    let part = part.trim();
                    let Some((key, value)) = part.split_once('=') else {
                        return Err(self.err(&format!(
                            "@command does not take the argument {part:?} (the operation id is \
                             the first argument; label=\"...\" is the keyword form)"
                        )));
                    };
                    match key.trim() {
                        "label" => {
                            let value = unquote(value);
                            if value.is_empty() {
                                return Err(self.err("@command label=\"\" would render a blank \
                                                    submit button"));
                            }
                            label = Some(value);
                        }
                        unknown => {
                            return Err(self.err(&format!(
                                "@command does not recognise the option {unknown:?} (the op id \
                                 comes first; the keyword form is label=\"...\")"
                            )));
                        }
                    }
                }
                Ok(Some(Node::Command { op, label }))
            }
            "csrf" => Ok(Some(Node::Csrf)),
            "method" => {
                let verb = parse_string_arg(arg, name, &self.loc())?.to_ascii_uppercase();
                Ok(Some(Node::Method { verb }))
            }
            "error" => {
                let field = parse_string_arg(arg, name, &self.loc())?;
                let body = self.parse_nodes(&["enderror"], es)?;
                self.expect_end("enderror")?;
                Ok(Some(Node::Error { field, body }))
            }
            "lang" => {
                let key = parse_string_arg(arg, name, &self.loc())?;
                Ok(Some(Node::Lang { key }))
            }
            "js" => {
                let expr =
                    parse_echo(&arg_required(arg, name, &self.loc())?).map_err(|e| self.err(&e))?;
                Ok(Some(Node::Js { expr }))
            }
            "class" => {
                let entries =
                    parse_attr_entries(&arg_required(arg, name, &self.loc())?)
                        .map_err(|e| self.err(&e))?;
                Ok(Some(Node::Class { entries }))
            }
            "style" => {
                let entries =
                    parse_attr_entries(&arg_required(arg, name, &self.loc())?)
                        .map_err(|e| self.err(&e))?;
                Ok(Some(Node::Style { entries }))
            }
            "checked" | "selected" | "disabled" | "readonly" | "required" => {
                let cond = parse_cond(&arg_required(arg, name, &self.loc())?)
                    .map_err(|e| self.err(&e))?;
                Ok(Some(Node::AttrIf {
                    attr: name.to_string(),
                    cond,
                }))
            }
            "php" => Err(self.err(
                "@php is deliberately SUPERSEDED: arbitrary PHP eval is not implemented \
                 (docs/LF-028-TO-LF-035-VERIFICATION.md)",
            )),
            "while" => Err(self.err(
                "@while is deliberately SUPERSEDED: use @foreach over a range \
                 (docs/LF-028-TO-LF-035-VERIFICATION.md)",
            )),
            "endsection" | "show" | "endif" | "elseif" | "else" | "endunless" | "endisset"
            | "endempty" | "endauth" | "endguest" | "endenv" | "endproduction" | "endforeach"
            | "endfor" | "endforelse" | "endonce" | "endpush" | "slot" | "endslot"
            | "endcomponent" | "enderror" => {
                Err(self.err(&format!("unexpected @{name}")))
            }
            _ => Ok(Some(Node::Directive {
                name: name.to_string(),
                arg,
            })),
        }
    }

    /// Location tag for diagnostics (1-based line of the current position).
    fn loc(&self) -> String {
        let line = self.chars[..self.pos].iter().filter(|c| **c == '\n').count() + 1;
        format!("line {line}")
    }

    fn parse_section(
        &mut self,
        arg: Option<String>,
        es: &mut Option<String>,
    ) -> Result<Option<Node>, ViewError> {
        let parts = split_args(&arg_required(arg, "section", &self.loc())?);
        let sname = unquote(
            parts
                .first()
                .ok_or_else(|| self.err("@section requires a name"))?
                .trim(),
        );
        if sname.is_empty() {
            return Err(self.err("@section requires a name"));
        }
        if parts.len() >= 2 {
            // Inline form: @section("title", "content") defines a literal section.
            let content = unquote(parts[1..].join(",").trim());
            return Ok(Some(Node::Section {
                name: sname,
                body: vec![Node::Text(content)],
                show: false,
            }));
        }
        let body = self.parse_nodes(&["endsection", "show"], es)?;
        let (stop, _) = self.read_directive()?;
        Ok(Some(Node::Section {
            name: sname,
            body,
            show: stop == "show",
        }))
    }

    fn parse_include(
        &mut self,
        arg: Option<String>,
        only_if_exists: bool,
        when: bool,
    ) -> Result<Option<Node>, ViewError> {
        let name = if when { "includeWhen" } else if only_if_exists { "includeIf" } else { "include" };
        let parts = split_args(&arg_required(arg, name, &self.loc())?);
        let (cond, tpl_part, data_part) = if when {
            let cond_src = parts
                .first()
                .ok_or_else(|| self.err("@includeWhen requires a condition"))?;
            let cond = parse_cond(cond_src.trim()).map_err(|e| self.err(&e))?;
            (Some(cond), parts.get(1), parts.get(2))
        } else {
            (None, parts.first(), parts.get(1))
        };
        let tpl_name = unquote(
            tpl_part
                .ok_or_else(|| self.err(&format!("@{name} requires a template name")))?
                .trim(),
        );
        if tpl_name.is_empty() {
            return Err(self.err(&format!("@{name} requires a template name")));
        }
        let data = match data_part {
            Some(d) => parse_data_map(d).map_err(|e| self.err(&e))?,
            None => Vec::new(),
        };
        Ok(Some(Node::Include {
            name: tpl_name,
            data,
            only_if_exists,
            when: cond,
        }))
    }

    fn parse_component(
        &mut self,
        arg: Option<String>,
        es: &mut Option<String>,
    ) -> Result<Option<Node>, ViewError> {
        let parts = split_args(&arg_required(arg, "component", &self.loc())?);
        let cname = unquote(
            parts
                .first()
                .ok_or_else(|| self.err("@component requires a template name"))?
                .trim(),
        );
        if cname.is_empty() {
            return Err(self.err("@component requires a template name"));
        }
        let data = match parts.get(1) {
            Some(d) => parse_data_map(d).map_err(|e| self.err(&e))?,
            None => Vec::new(),
        };
        let mut body = Vec::new();
        let mut slots = Vec::new();
        loop {
            let mut chunk = self.parse_nodes(&["slot", "endcomponent"], es)?;
            body.append(&mut chunk);
            let (stop, sarg) = self.read_directive()?;
            if stop == "endcomponent" {
                break;
            }
            let slot_name = parse_string_arg(sarg, "slot", &self.loc())?;
            let mut snodes = self.parse_nodes(&["endslot"], es)?;
            self.expect_end("endslot")?;
            slots.push((slot_name, std::mem::take(&mut snodes)));
        }
        Ok(Some(Node::Component {
            name: cname,
            data,
            slots,
            body,
        }))
    }

    fn parse_if(
        &mut self,
        arg: Option<String>,
        unless: bool,
        es: &mut Option<String>,
    ) -> Result<Option<Node>, ViewError> {
        let name = if unless { "unless" } else { "if" };
        let mut cond =
            parse_cond(&arg_required(arg, name, &self.loc())?).map_err(|e| self.err(&e))?;
        if unless {
            cond = Cond::Not(Box::new(cond));
        }
        let end = if unless { "endunless" } else { "endif" };
        let mut stops: Vec<&str> = vec!["else", end];
        if !unless {
            stops.push("elseif");
        }
        let mut branches = vec![(cond, self.parse_nodes(&stops, es)?)];
        let mut fallback = None;
        loop {
            let (stop, sarg) = self.read_directive()?;
            if stop == end {
                break;
            }
            if stop == "else" {
                fallback = Some(self.parse_nodes(&[end], es)?);
                self.expect_end(end)?;
                break;
            }
            if stop == "elseif" && !unless {
                let c = parse_cond(&arg_required(sarg, "elseif", &self.loc())?)
                    .map_err(|e| self.err(&e))?;
                branches.push((c, self.parse_nodes(&stops, es)?));
                continue;
            }
            return Err(self.err(&format!("unexpected @{stop} inside @{name}")));
        }
        Ok(Some(Node::If { branches, fallback }))
    }

    fn parse_body_with_else(
        &mut self,
        end: &str,
        es: &mut Option<String>,
    ) -> Result<(Vec<Node>, Option<Vec<Node>>), ViewError> {
        let body = self.parse_nodes(&["else", end], es)?;
        let (stop, _) = self.read_directive()?;
        if stop == "else" {
            let fallback = self.parse_nodes(&[end], es)?;
            self.expect_end(end)?;
            Ok((body, Some(fallback)))
        } else {
            Ok((body, None))
        }
    }
}

// ------------------------------------------------------------- arg helpers

fn arg_required(arg: Option<String>, name: &str, loc: &str) -> Result<String, ViewError> {
    arg.ok_or_else(|| ViewError::new(VIEW_PARSE, format!("@{name} requires arguments ({loc})")))
}

fn parse_string_arg(
    arg: Option<String>,
    name: &str,
    loc: &str,
) -> Result<String, ViewError> {
    let raw = arg_required(arg, name, loc)?;
    let value = unquote(raw.trim());
    if value.is_empty() {
        return Err(ViewError::new(
            VIEW_PARSE,
            format!("@{name} requires a non-empty string argument ({loc})"),
        ));
    }
    Ok(value)
}

/// Split a directive argument on top-level commas (quotes/brackets respected).
fn split_args(s: &str) -> Vec<String> {
    split_all_top(s, ',')
}

/// Find the first occurrence of sep at depth zero outside strings.
fn split_top(s: &str, sep: &str) -> Option<(String, String)> {
    let chars: Vec<char> = s.chars().collect();
    let sep_chars: Vec<char> = sep.chars().collect();
    let mut depth = 0i32;
    let mut in_str: Option<char> = None;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match in_str {
            Some(q) => {
                if c == '\\' {
                    i += 2;
                    continue;
                }
                if c == q {
                    in_str = None;
                }
            }
            None => {
                if c == '"' || c == '\'' {
                    in_str = Some(c);
                } else if c == '(' || c == '[' || c == '{' {
                    depth += 1;
                } else if c == ')' || c == ']' || c == '}' {
                    depth -= 1;
                } else if depth == 0 && chars[i..].starts_with(&sep_chars) {
                    let left: String = chars[..i].iter().collect();
                    let right: String = chars[i + sep_chars.len()..].iter().collect();
                    return Some((left, right));
                }
            }
        }
        i += 1;
    }
    None
}

/// Split on every top-level occurrence of sep.
fn split_all_top(s: &str, sep: char) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut current: Vec<char> = Vec::new();
    let mut depth = 0i32;
    let mut in_str: Option<char> = None;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match in_str {
            Some(q) => {
                current.push(c);
                if c == '\\' {
                    if i + 1 < chars.len() {
                        i += 1;
                        current.push(chars[i]);
                    }
                } else if c == q {
                    in_str = None;
                }
            }
            None => {
                if c == '"' || c == '\'' {
                    in_str = Some(c);
                    current.push(c);
                } else if c == '(' || c == '[' || c == '{' {
                    depth += 1;
                    current.push(c);
                } else if c == ')' || c == ']' || c == '}' {
                    depth -= 1;
                    current.push(c);
                } else if c == sep && depth == 0 {
                    out.push(current.iter().collect());
                    current = Vec::new();
                } else {
                    current.push(c);
                }
            }
        }
        i += 1;
    }
    out.push(current.iter().collect());
    out
}

/// Strip one matching pair of surrounding quotes, unescaping the content.
fn unquote(s: &str) -> String {
    let t = s.trim();
    let bytes = t.chars().collect::<Vec<char>>();
    if bytes.len() >= 2 && (bytes[0] == '"' || bytes[0] == '\'') && bytes[bytes.len() - 1] == bytes[0]
    {
        let q = bytes[0];
        let mut out = String::new();
        let mut i = 1;
        while i < bytes.len() - 1 {
            if bytes[i] == '\\' && i + 1 < bytes.len() - 1 {
                i += 1;
                out.push(bytes[i]);
            } else if bytes[i] == q {
                // stray quote: keep as-is
                out.push(bytes[i]);
            } else {
                out.push(bytes[i]);
            }
            i += 1;
        }
        out
    } else {
        t.to_string()
    }
}

fn strip_brackets(s: &str) -> String {
    let t = s.trim();
    if (t.starts_with('[') && t.ends_with(']')) || (t.starts_with('{') && t.ends_with('}')) {
        t[1..t.len() - 1].to_string()
    } else {
        t.to_string()
    }
}

/// Parse include/component data maps: ["key" => operand, ...].
fn parse_data_map(s: &str) -> Result<Vec<(String, Operand)>, String> {
    let inner = strip_brackets(s.trim());
    if inner.trim().is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for part in split_all_top(&inner, ',') {
        let part = part.trim().to_string();
        if part.is_empty() {
            continue;
        }
        let (k, v) = split_top(&part, "=>")
            .or_else(|| split_top(&part, ":"))
            .ok_or_else(|| format!("expected key => value in data map, found {part:?}"))?;
        let key = unquote(k.trim());
        if key.is_empty() {
            return Err(format!("empty key in data map entry {part:?}"));
        }
        let value = parse_operand_str(v.trim())?;
        out.push((key, value));
    }
    Ok(out)
}

/// Parse @class/@style entries: ["always", "sometimes" => cond, ...].
fn parse_attr_entries(s: &str) -> Result<Vec<AttrEntry>, String> {
    let inner = strip_brackets(s.trim());
    let mut out = Vec::new();
    for part in split_all_top(&inner, ',') {
        let part = part.trim().to_string();
        if part.is_empty() {
            continue;
        }
        match split_top(&part, "=>") {
            Some((v, c)) => {
                let value = unquote(v.trim());
                let cond = parse_cond(c.trim())?;
                out.push(AttrEntry {
                    value,
                    cond: Some(cond),
                });
            }
            None => out.push(AttrEntry {
                value: unquote(&part),
                cond: None,
            }),
        }
    }
    Ok(out)
}

fn clean_var(s: &str) -> &str {
    s.trim().trim_start_matches('$').trim()
}

/// foreach arguments: "items as item", "items as key => item",
/// "item in items", "(key, item) in items".
fn parse_foreach_arg(s: &str) -> Result<(LoopVars, Operand), String> {
    if let Some((left, right)) = split_top(s, " as ") {
        let source = parse_operand_str(clean_var(left.trim()))?;
        let right = right.trim();
        let (key, item) = match split_top(right, "=>") {
            Some((k, v)) => (Some(clean_var(k.trim()).to_string()), clean_var(v.trim()).to_string()),
            None => (None, clean_var(right).to_string()),
        };
        if item.is_empty() {
            return Err("foreach requires an item variable".into());
        }
        return Ok((LoopVars { item, key }, source));
    }
    if let Some((left, right)) = split_top(s, " in ") {
        let l = left
            .trim()
            .trim_start_matches('(')
            .trim_end_matches(')')
            .trim();
        let (key, item) = match split_top(l, ",") {
            Some((k, v)) => (Some(clean_var(k.trim()).to_string()), clean_var(v.trim()).to_string()),
            None => (None, clean_var(l).to_string()),
        };
        let source = parse_operand_str(clean_var(right.trim()))?;
        if item.is_empty() {
            return Err("foreach requires an item variable".into());
        }
        return Ok((LoopVars { item, key }, source));
    }
    Err("expected \"items as item\" or \"item in items\"".into())
}

/// @for arguments: range form "i in 0..10" or restricted C-style
/// "i = 0; i < 10; i++" (step: ++, --, += n, -= n).
fn parse_for_arg(s: &str) -> Result<ForSpec, String> {
    if let Some((left, right)) = split_top(s, " in ") {
        let var = clean_var(left.trim()).to_string();
        if var.is_empty() {
            return Err("for requires a loop variable".into());
        }
        let (from_s, to_s) =
            split_top(right.trim(), "..").ok_or("expected a range like 0..10")?;
        let from = parse_operand_str(from_s.trim())?;
        let to = parse_operand_str(to_s.trim())?;
        return Ok(ForSpec::Range { var, from, to });
    }
    let parts = split_all_top(s, ';');
    if parts.len() != 3 {
        return Err("expected (i in 0..10) or (i = 0; i < 10; i++)".into());
    }
    let (var, init) =
        split_top(parts[0].trim(), "=").ok_or("expected i = <value>")?;
    let var = clean_var(var.trim()).to_string();
    let init = parse_operand_str(init.trim())?;
    let cond = parts[1].trim().to_string();
    let mut found = None;
    for op_s in ["<=", ">=", "==", "!=", "<", ">"] {
        if let Some((l, r)) = split_top(&cond, op_s) {
            found = Some((op_s, l, r));
            break;
        }
    }
    let (op_s, l, r) = found.ok_or("expected a comparison in the loop condition")?;
    if clean_var(l.trim()) != var {
        return Err("the loop condition must test the loop variable".into());
    }
    let op = match op_s {
        "<" => CmpOp::Lt,
        "<=" => CmpOp::Le,
        ">" => CmpOp::Gt,
        ">=" => CmpOp::Ge,
        "==" => CmpOp::Eq,
        "!=" => CmpOp::Ne,
        _ => unreachable!(),
    };
    let bound = parse_operand_str(r.trim())?;
    let step_s = parts[2].trim();
    let step = if step_s.ends_with("++") {
        1.0
    } else if step_s.ends_with("--") {
        -1.0
    } else if let Some((_, n)) = split_top(step_s, "+=") {
        n.trim()
            .parse::<f64>()
            .map_err(|_| "step must be numeric".to_string())?
    } else if let Some((_, n)) = split_top(step_s, "-=") {
        -n.trim()
            .parse::<f64>()
            .map_err(|_| "step must be numeric".to_string())?
    } else {
        return Err("expected i++, i--, i += n or i -= n".into());
    };
    Ok(ForSpec::CStyle {
        var,
        init,
        op,
        bound,
        step,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(src: &str) -> Template {
        parse(src).unwrap_or_else(|e| panic!("parse failed: {} {}", e.code, e.message))
    }

    #[test]
    fn parses_text_echo_and_raw() {
        let t = parse_ok("Hello {{ name }} and {!! html !!}");
        assert_eq!(t.nodes.len(), 4);
        assert!(matches!(&t.nodes[0], Node::Text(s) if s == "Hello "));
        assert!(matches!(&t.nodes[1], Node::Echo(_)));
        assert!(matches!(&t.nodes[2], Node::Text(s) if s == " and "));
        assert!(matches!(&t.nodes[3], Node::Raw(_)));
    }

    #[test]
    fn escaped_at_and_echo_are_literal() {
        let t = parse_ok("@@literal @{{ not_an_echo }} john@example.com");
        let text: String = t
            .nodes
            .iter()
            .map(|n| match n {
                Node::Text(s) => s.clone(),
                _ => String::new(),
            })
            .collect();
        assert_eq!(text, "@literal {{ not_an_echo }} john@example.com");
    }

    #[test]
    fn parses_if_elseif_else() {
        let t = parse_ok("@if(a > 1)big @elseif(a == 1)one @else small @endif");
        let Node::If { branches, fallback } = &t.nodes[0] else {
            panic!("expected if");
        };
        assert_eq!(branches.len(), 2);
        assert!(fallback.is_some());
    }

    #[test]
    fn parses_command_binding() {
        let t = parse_ok("@command(\"cancel\", label=\"Cancel it\")");
        let Node::Command { op, label } = &t.nodes[0] else {
            panic!("expected a command binding")
        };
        assert_eq!(op, "cancel");
        assert_eq!(label.as_deref(), Some("Cancel it"));

        // A quoted op alone; unquoted op ids work too.
        let t = parse_ok("@command(cancel)");
        assert!(matches!(&t.nodes[0], Node::Command { op, label: None } if op == "cancel"));
    }

    #[test]
    fn a_command_requires_an_operation_id() {
        assert!(
            parse("@command()").is_err(),
            "a command with no op id is refused: there is nothing to bind"
        );
        assert!(
            parse("@command").is_err(),
            "the bare directive is refused - the op id is the declarative half"
        );
    }

    #[test]
    fn a_command_refuses_unknown_options() {
        assert!(
            parse("@command(\"cancel\", verb=\"PUT\")").is_err(),
            "the verb is the ROUTE's declaration, not the template author's choice"
        );
        assert!(
            parse("@command(\"cancel\", label=\"\")").is_err(),
            "an empty label would render a blank button"
        );
    }

    #[test]
    fn unless_negates_and_takes_else() {
        let t = parse_ok("@unless(hidden)shown @else gone @endunless");
        let Node::If { branches, fallback } = &t.nodes[0] else {
            panic!("expected if");
        };
        assert!(matches!(branches[0].0, Cond::Not(_)));
        assert!(fallback.is_some());
    }

    #[test]
    fn parses_foreach_forms() {
        let t = parse_ok("@foreach(items as item){{ item }}@endforeach");
        assert!(matches!(&t.nodes[0], Node::ForEach { vars, .. } if vars.item == "item" && vars.key.is_none()));
        let t = parse_ok("@foreach($items as $k => $v)x@endforeach");
        assert!(matches!(&t.nodes[0], Node::ForEach { vars, .. } if vars.item == "v" && vars.key.as_deref() == Some("k")));
        let t = parse_ok("@foreach((key, item) in items)x@endforeach");
        assert!(matches!(&t.nodes[0], Node::ForEach { vars, .. } if vars.item == "item" && vars.key.as_deref() == Some("key")));
    }

    #[test]
    fn parses_forelse_with_empty_branch() {
        let t = parse_ok("@forelse(items as item){{ item }}@empty none @endforelse");
        let Node::ForEach { empty, .. } = &t.nodes[0] else {
            panic!("expected forelse");
        };
        assert!(empty.is_some());
    }

    #[test]
    fn parses_for_range_and_cstyle() {
        let t = parse_ok("@for(i in 0..3){{ i }}@endfor");
        assert!(matches!(&t.nodes[0], Node::For { spec: ForSpec::Range { var, .. }, .. } if var == "i"));
        let t = parse_ok("@for($i = 0; $i < 10; $i++){{ i }}@endfor");
        assert!(matches!(
            &t.nodes[0],
            Node::For { spec: ForSpec::CStyle { var, step, .. }, .. } if var == "i" && *step == 1.0
        ));
    }

    #[test]
    fn parses_sections_yield_parent_show() {
        let t = parse_ok("@extends(\"layouts.app\")@section(\"title\")Page@endsection@yield(\"title\")@parent@section(\"side\")s@show");
        assert_eq!(t.extends.as_deref(), Some("layouts.app"));
        assert!(matches!(&t.nodes[0], Node::Section { name, show, .. } if name == "title" && !show));
        assert!(matches!(&t.nodes[1], Node::Yield { name, .. } if name == "title"));
        assert!(matches!(&t.nodes[2], Node::Parent));
        assert!(matches!(&t.nodes[3], Node::Section { name, show, .. } if name == "side" && *show));
    }

    #[test]
    fn parses_inline_section() {
        let t = parse_ok("@section(\"title\", \"Hello\")");
        let Node::Section { name, body, show } = &t.nodes[0] else {
            panic!("expected section");
        };
        assert_eq!(name, "title");
        assert!(!show);
        assert!(matches!(&body[0], Node::Text(s) if s == "Hello"));
    }

    #[test]
    fn parses_include_variants() {
        let t = parse_ok("@include(\"partials.nav\", [\"active\" => active])");
        assert!(matches!(&t.nodes[0], Node::Include { name, data, .. } if name == "partials.nav" && data.len() == 1));
        let t = parse_ok("@includeIf(\"maybe\")");
        assert!(matches!(&t.nodes[0], Node::Include { only_if_exists: true, .. }));
        let t = parse_ok("@includeWhen(flag, \"cond\")");
        assert!(matches!(&t.nodes[0], Node::Include { when: Some(_), .. }));
    }

    #[test]
    fn parses_component_with_slots_and_props() {
        let t = parse_ok(
            "@component(\"alert\", [\"level\" => level])@slot(\"title\")T@endslot body @endcomponent",
        );
        let Node::Component { name, slots, body, data } = &t.nodes[0] else {
            panic!("expected component");
        };
        assert_eq!(name, "alert");
        assert_eq!(slots.len(), 1);
        assert_eq!(slots[0].0, "title");
        assert!(!body.is_empty());
        assert_eq!(data.len(), 1);
        let t = parse_ok("@props([\"a\", \"b\"])");
        assert!(matches!(&t.nodes[0], Node::Props { names } if names.len() == 2));
    }

    #[test]
    fn parses_attribute_helpers() {
        let t = parse_ok("@class([\"btn\", \"active\" => is_on])@style([\"color: red\" => hot])@checked(done)");
        assert!(matches!(&t.nodes[0], Node::Class { entries } if entries.len() == 2));
        assert!(matches!(&t.nodes[1], Node::Style { entries } if entries.len() == 1));
        assert!(matches!(&t.nodes[2], Node::AttrIf { attr, .. } if attr == "checked"));
    }

    #[test]
    fn parses_small_directives() {
        let t = parse_ok("@csrf@method(\"PUT\")@lang(\"welcome\")@js(config)@stack(\"scripts\")@push(\"scripts\")s@endpush@once o@endonce@error(\"email\"){{ message }}@enderror@isset(user.name)has@endisset@empty(list)none@endempty@auth a@else g@endauth@guest g@endguest@env(\"dev\")d@endenv@production p@endproduction");
        assert!(matches!(&t.nodes[0], Node::Csrf));
        assert!(matches!(&t.nodes[1], Node::Method { verb } if verb == "PUT"));
        assert!(matches!(&t.nodes[2], Node::Lang { key } if key == "welcome"));
        assert!(matches!(&t.nodes[3], Node::Js { .. }));
        assert!(matches!(&t.nodes[4], Node::Stack { name } if name == "scripts"));
        assert!(matches!(&t.nodes[5], Node::Push { stack, .. } if stack == "scripts"));
        assert!(matches!(&t.nodes[6], Node::Once { .. }));
        assert!(matches!(&t.nodes[7], Node::Error { field, .. } if field == "email"));
        assert!(matches!(&t.nodes[8], Node::Isset { .. }));
        assert!(matches!(&t.nodes[9], Node::Empty { .. }));
        assert!(matches!(&t.nodes[10], Node::Auth { fallback: Some(_), .. }));
        assert!(matches!(&t.nodes[11], Node::Guest { .. }));
        assert!(matches!(&t.nodes[12], Node::Env { name, .. } if name == "dev"));
        assert!(matches!(&t.nodes[13], Node::Production { .. }));
    }

    #[test]
    fn unknown_parenthesized_directive_becomes_custom() {
        let t = parse_ok("@money(amount)");
        assert!(matches!(&t.nodes[0], Node::Directive { name, arg: Some(_) } if name == "money"));
    }

    #[test]
    fn superseded_directives_are_refused() {
        let e = parse("@php echo 1; @endphp").unwrap_err();
        assert_eq!(e.code, VIEW_PARSE);
        assert!(e.message.contains("SUPERSEDED"));
        let e = parse("@while(true)x@endwhile").unwrap_err();
        assert!(e.message.contains("SUPERSEDED"));
    }

    #[test]
    fn unterminated_constructs_fail_closed() {
        assert!(parse("@if(a)").unwrap_err().code == VIEW_PARSE);
        assert!(parse("{{ x").unwrap_err().code == VIEW_PARSE);
        assert!(parse("@foreach(items as item)").unwrap_err().code == VIEW_PARSE);
        assert!(parse("@section(\"a\")body").unwrap_err().code == VIEW_PARSE);
    }

    #[test]
    fn stray_closing_directive_is_refused() {
        let e = parse("x @endif").unwrap_err();
        assert!(e.message.contains("unexpected @endif"));
    }
}
