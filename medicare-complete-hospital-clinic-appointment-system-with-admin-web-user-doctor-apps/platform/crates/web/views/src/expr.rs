//! Expression + condition mini-language for the views engine (Wave 2).
//!
//! Echo expressions are dot-paths into the JSON context with optional
//! pipe-filters: {{ user.name | upper }} and {!! raw.html !!} for raw
//! output. Conditions power @if / @unless / @includeWhen and friends with
//! a tiny boolean grammar (&&, ||, !, comparisons). Both are hand-parsed -
//! no PHP eval, no regex crate (deliberately SUPERSEDED per docs/18).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One step of a dot-path: an object key or an array index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Segment {
    Key(String),
    Index(usize),
}

/// A pipe filter applied to an echo value, e.g. upper or join(", ").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Filter {
    pub name: String,
    pub args: Vec<String>,
}

/// An echo expression: a dot-path plus zero or more filters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Expr {
    pub path: Vec<Segment>,
    /// Wave 6 P3: quoted-string echo (`{{ "key" | trans }}`) — the literal
    /// seeds the filter chain instead of a context lookup. `None` = path.
    #[serde(default)]
    pub literal: Option<String>,
    pub filters: Vec<Filter>,
}

/// Comparison operators for conditions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// A literal operand in a condition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Lit {
    Str(String),
    Num(f64),
    Bool(bool),
    Null,
}

/// Either a context dot-path or an inline literal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Operand {
    Path(Vec<Segment>),
    Lit(Lit),
}

/// A boolean condition tree. Custom variants call into the engine's
/// custom-conditional registry (Blade::if parity); unregistered names
/// evaluate false (fail closed).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Cond {
    Or(Vec<Cond>),
    And(Vec<Cond>),
    Not(Box<Cond>),
    Cmp { left: Operand, op: CmpOp, right: Operand },
    Truthy(Operand),
    Custom { name: String, args: Vec<Operand> },
}

// ------------------------------------------------------------------ parsing

struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(s: &'a str) -> Self {
        Self { bytes: s.as_bytes(), pos: 0 }
    }
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }
    fn bump(&mut self) -> Option<u8> {
        let b = self.peek();
        if b.is_some() {
            self.pos += 1;
        }
        b
    }
    fn skip_ws(&mut self) {
        while let Some(b) = self.peek() {
            if b == b' ' || b == b'\t' || b == b'\r' || b == b'\n' {
                self.pos += 1;
            } else {
                break;
            }
        }
    }
    fn eat(&mut self, ch: u8) -> bool {
        if self.peek() == Some(ch) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn starts(&self, s: &str) -> bool {
        self.bytes[self.pos..].starts_with(s.as_bytes())
    }
    fn eof(&self) -> bool {
        self.pos >= self.bytes.len()
    }
}

fn parse_segments_inner(cur: &mut Cursor) -> Vec<Segment> {
    let mut segs = Vec::new();
    loop {
        cur.skip_ws();
        let start = cur.pos;
        while let Some(b) = cur.peek() {
            if b.is_ascii_alphanumeric() || b == b'_' || b == b'-' {
                cur.pos += 1;
            } else {
                break;
            }
        }
        let ident = std::str::from_utf8(&cur.bytes[start..cur.pos])
            .unwrap_or("")
            .to_string();
        if !ident.is_empty() {
            segs.push(Segment::Key(ident));
        }
        cur.skip_ws();
        match cur.peek() {
            Some(b'.') => {
                cur.pos += 1;
                continue;
            }
            Some(b'[') => {
                cur.pos += 1;
                cur.skip_ws();
                let nstart = cur.pos;
                while let Some(b) = cur.peek() {
                    if b.is_ascii_digit() {
                        cur.pos += 1;
                    } else {
                        break;
                    }
                }
                let num = std::str::from_utf8(&cur.bytes[nstart..cur.pos]).unwrap_or("");
                if let Ok(i) = num.parse::<usize>() {
                    segs.push(Segment::Index(i));
                }
                cur.skip_ws();
                cur.eat(b']');
                cur.skip_ws();
                if cur.peek() == Some(b'.') {
                    cur.pos += 1;
                    continue;
                }
                break;
            }
            _ => break,
        }
    }
    segs
}

/// Parse a dot-path from a string.
pub fn parse_path(input: &str) -> Vec<Segment> {
    let mut cur = Cursor::new(input.trim());
    parse_segments_inner(&mut cur)
}

fn parse_filter(cur: &mut Cursor) -> Result<Filter, String> {
    cur.skip_ws();
    let start = cur.pos;
    while let Some(b) = cur.peek() {
        if b.is_ascii_alphanumeric() || b == b'_' {
            cur.pos += 1;
        } else {
            break;
        }
    }
    let name = std::str::from_utf8(&cur.bytes[start..cur.pos])
        .unwrap_or("")
        .to_string();
    if name.is_empty() {
        return Err("empty filter name".into());
    }
    let mut args = Vec::new();
    cur.skip_ws();
    if cur.eat(b'(') {
        loop {
            cur.skip_ws();
            if cur.peek() == Some(b')') {
                cur.pos += 1;
                break;
            }
            if cur.eof() {
                return Err(format!("unterminated argument list for filter {name:?}"));
            }
            args.push(parse_filter_arg(cur)?);
            cur.skip_ws();
            if cur.eat(b',') {
                continue;
            }
            cur.skip_ws();
            if cur.eat(b')') {
                break;
            }
            return Err(format!("expected ',' or ')' in filter {name:?}"));
        }
    }
    Ok(Filter { name, args })
}

fn parse_filter_arg(cur: &mut Cursor) -> Result<String, String> {
    cur.skip_ws();
    match cur.peek() {
        Some(q @ (b'"' | b'\'')) => {
            cur.pos += 1;
            let mut out = String::new();
            loop {
                match cur.bump() {
                    None => return Err("unterminated string argument".into()),
                    Some(b'\\') => {
                        if let Some(c) = cur.bump() {
                            out.push(c as char);
                        }
                    }
                    Some(c) if c == q => break,
                    Some(c) => out.push(c as char),
                }
            }
            Ok(out)
        }
        _ => {
            let start = cur.pos;
            while let Some(b) = cur.peek() {
                if b == b',' || b == b')' {
                    break;
                }
                cur.pos += 1;
            }
            Ok(std::str::from_utf8(&cur.bytes[start..cur.pos])
                .unwrap_or("")
                .trim()
                .to_string())
        }
    }
}

/// Parse an echo expression: user.name | upper | default("n/a"), or a
/// quoted literal with filters (`{{ "key" | trans }}`, Wave 6 P3).
pub fn parse_echo(input: &str) -> Result<Expr, String> {
    let mut cur = Cursor::new(input.trim());
    cur.skip_ws();
    // A leading quoted string is a literal echo value; everything else is
    // a dot-path (as before).
    let (path, literal) = if matches!(cur.peek(), Some(b'"' | b'\'')) {
        (Vec::new(), Some(parse_quoted(&mut cur)?))
    } else {
        (parse_segments_inner(&mut cur), None)
    };
    let mut filters = Vec::new();
    loop {
        cur.skip_ws();
        if cur.eat(b'|') {
            filters.push(parse_filter(&mut cur)?);
        } else {
            break;
        }
    }
    cur.skip_ws();
    if !cur.eof() {
        return Err(format!("trailing characters in expression: {input:?}"));
    }
    Ok(Expr {
        path,
        literal,
        filters,
    })
}

/// Consume a single- or double-quoted string literal (backslash escapes
/// drop the backslash, mirroring filter arguments and condition operands).
fn parse_quoted(cur: &mut Cursor) -> Result<String, String> {
    let Some(q) = cur.peek() else {
        return Err("expected string literal".into());
    };
    if q != b'"' && q != b'\'' {
        return Err("expected string literal".into());
    }
    cur.pos += 1;
    let mut out = String::new();
    loop {
        match cur.bump() {
            None => return Err("unterminated string literal".into()),
            Some(b'\\') => {
                if let Some(c) = cur.bump() {
                    out.push(c as char);
                }
            }
            Some(c) if c == q => break,
            Some(c) => out.push(c as char),
        }
    }
    Ok(out)
}

fn parse_operand(cur: &mut Cursor) -> Result<Operand, String> {
    cur.skip_ws();
    match cur.peek() {
        Some(q @ (b'"' | b'\'')) => {
            cur.pos += 1;
            let mut out = String::new();
            loop {
                match cur.bump() {
                    None => return Err("unterminated string literal".into()),
                    Some(b'\\') => {
                        if let Some(c) = cur.bump() {
                            out.push(c as char);
                        }
                    }
                    Some(c) if c == q => break,
                    Some(c) => out.push(c as char),
                }
            }
            Ok(Operand::Lit(Lit::Str(out)))
        }
        Some(b) if b == b'-' || b.is_ascii_digit() => {
            let start = cur.pos;
            cur.pos += 1;
            while let Some(b) = cur.peek() {
                if b.is_ascii_digit() || b == b'.' {
                    cur.pos += 1;
                } else {
                    break;
                }
            }
            let num = std::str::from_utf8(&cur.bytes[start..cur.pos]).unwrap_or("");
            num.parse::<f64>()
                .map(Lit::Num)
                .map(Operand::Lit)
                .map_err(|_| format!("invalid number literal {num:?}"))
        }
        _ => {
            let start = cur.pos;
            while let Some(b) = cur.peek() {
                if b.is_ascii_alphanumeric()
                    || b == b'_'
                    || b == b'.'
                    || b == b'['
                    || b == b']'
                    || b == b'-'
                {
                    cur.pos += 1;
                } else {
                    break;
                }
            }
            let word = std::str::from_utf8(&cur.bytes[start..cur.pos]).unwrap_or("");
            if word.is_empty() {
                return Err("expected an operand".into());
            }
            match word {
                "true" => return Ok(Operand::Lit(Lit::Bool(true))),
                "false" => return Ok(Operand::Lit(Lit::Bool(false))),
                "null" | "nil" => return Ok(Operand::Lit(Lit::Null)),
                _ => {}
            }
            Ok(Operand::Path(parse_path(word)))
        }
    }
}

/// Parse a single operand from a string: number, quoted string, bool,
/// null, or a dot-path.
pub fn parse_operand_str(input: &str) -> Result<Operand, String> {
    let mut cur = Cursor::new(input.trim());
    let op = parse_operand(&mut cur)?;
    cur.skip_ws();
    if !cur.eof() {
        return Err(format!("trailing characters in operand: {input:?}"));
    }
    Ok(op)
}

fn parse_cmp(cur: &mut Cursor) -> Result<Cond, String> {
    let left = parse_operand(cur)?;
    cur.skip_ws();
    let op = if cur.starts("==") {
        cur.pos += 2;
        Some(CmpOp::Eq)
    } else if cur.starts("!=") {
        cur.pos += 2;
        Some(CmpOp::Ne)
    } else if cur.starts("<=") {
        cur.pos += 2;
        Some(CmpOp::Le)
    } else if cur.starts(">=") {
        cur.pos += 2;
        Some(CmpOp::Ge)
    } else if cur.starts("<") {
        cur.pos += 1;
        Some(CmpOp::Lt)
    } else if cur.starts(">") {
        cur.pos += 1;
        Some(CmpOp::Gt)
    } else {
        None
    };
    match op {
        Some(op) => {
            let right = parse_operand(cur)?;
            Ok(Cond::Cmp { left, op, right })
        }
        None => Ok(Cond::Truthy(left)),
    }
}

fn try_parse_custom(cur: &mut Cursor) -> Result<Option<Cond>, String> {
    let saved = cur.pos;
    let start = cur.pos;
    while let Some(b) = cur.peek() {
        if b.is_ascii_alphanumeric() || b == b'_' {
            cur.pos += 1;
        } else {
            break;
        }
    }
    let name = std::str::from_utf8(&cur.bytes[start..cur.pos]).unwrap_or("");
    if name.is_empty() || matches!(name, "true" | "false" | "null" | "nil") {
        cur.pos = saved;
        return Ok(None);
    }
    let mut probe = cur.pos;
    while probe < cur.bytes.len()
        && matches!(cur.bytes[probe], b' ' | b'\t' | b'\r' | b'\n')
    {
        probe += 1;
    }
    if probe >= cur.bytes.len() || cur.bytes[probe] != b'(' {
        cur.pos = saved;
        return Ok(None);
    }
    cur.pos = probe + 1;
    let mut args = Vec::new();
    loop {
        cur.skip_ws();
        if cur.peek() == Some(b')') {
            cur.pos += 1;
            break;
        }
        if cur.eof() {
            return Err(format!("unterminated argument list in custom condition {name:?}"));
        }
        args.push(parse_operand(cur)?);
        cur.skip_ws();
        if cur.eat(b',') {
            continue;
        }
        cur.skip_ws();
        if cur.eat(b')') {
            break;
        }
        return Err(format!("expected ',' or ')' in custom condition {name:?}"));
    }
    Ok(Some(Cond::Custom {
        name: name.to_string(),
        args,
    }))
}

fn parse_not(cur: &mut Cursor) -> Result<Cond, String> {
    cur.skip_ws();
    if cur.eat(b'!') {
        Ok(Cond::Not(Box::new(parse_not(cur)?)))
    } else if cur.eat(b'(') {
        let inner = parse_or(cur)?;
        cur.skip_ws();
        if !cur.eat(b')') {
            return Err("expected ')' in condition".into());
        }
        Ok(inner)
    } else if let Some(custom) = try_parse_custom(cur)? {
        Ok(custom)
    } else {
        parse_cmp(cur)
    }
}

fn parse_and(cur: &mut Cursor) -> Result<Cond, String> {
    let mut parts = vec![parse_not(cur)?];
    loop {
        cur.skip_ws();
        if cur.starts("&&") {
            cur.pos += 2;
            parts.push(parse_not(cur)?);
        } else {
            break;
        }
    }
    if parts.len() == 1 {
        Ok(parts.pop().unwrap())
    } else {
        Ok(Cond::And(parts))
    }
}

fn parse_or(cur: &mut Cursor) -> Result<Cond, String> {
    let mut parts = vec![parse_and(cur)?];
    loop {
        cur.skip_ws();
        if cur.starts("||") {
            cur.pos += 2;
            parts.push(parse_and(cur)?);
        } else {
            break;
        }
    }
    if parts.len() == 1 {
        Ok(parts.pop().unwrap())
    } else {
        Ok(Cond::Or(parts))
    }
}

/// Parse a boolean condition: a && b == "x" || !c.
pub fn parse_cond(input: &str) -> Result<Cond, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("empty condition".into());
    }
    let mut cur = Cursor::new(input);
    let cond = parse_or(&mut cur)?;
    cur.skip_ws();
    if !cur.eof() {
        return Err(format!("trailing characters in condition: {input:?}"));
    }
    Ok(cond)
}

// -------------------------------------------------------------- evaluation

pub fn resolve_path<'v>(root: &'v Value, path: &[Segment]) -> Option<&'v Value> {
    let mut cur = root;
    for seg in path {
        match seg {
            Segment::Key(k) => cur = cur.get(k.as_str())?,
            Segment::Index(i) => cur = cur.get(*i)?,
        }
    }
    Some(cur)
}

/// Wave 6 P3: the `trans` filter core — the key is the explicit filter
/// argument (`| trans("page.title")`) or the input string, resolved
/// through the injected `_lang` flat map via `lookup` (the catalog with
/// the fallback chain pre-merged). A miss degrades exactly like `@lang`:
/// the key itself; a non-string input with no argument is untouched.
pub(crate) fn trans_filter(
    value: Value,
    filter: &Filter,
    lookup: impl Fn(&str) -> Option<Value>,
) -> Value {
    let key = match filter.args.first() {
        Some(arg) => arg.clone(),
        None => match value {
            Value::String(s) => s,
            other => return other,
        },
    };
    match lookup(&key) {
        Some(Value::String(text)) => Value::String(text),
        _ => Value::String(key),
    }
}

pub(crate) fn apply_filter(value: Value, filter: &Filter) -> Value {
    match filter.name.as_str() {
        "upper" => Value::String(to_display(&value).to_uppercase()),
        "lower" => Value::String(to_display(&value).to_lowercase()),
        "title" => Value::String(title_case(&to_display(&value))),
        "capitalize" => {
            let s = to_display(&value);
            let mut chars = s.chars();
            let out = match chars.next() {
                Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            };
            Value::String(out)
        }
        "trim" => Value::String(to_display(&value).trim().to_string()),
        "length" | "count" => match &value {
            Value::String(s) => Value::Number(s.chars().count().into()),
            Value::Array(a) => Value::Number(a.len().into()),
            Value::Object(o) => Value::Number(o.len().into()),
            _ => Value::Number(0.into()),
        },
        "first" => match &value {
            Value::Array(a) => a.first().cloned().unwrap_or(Value::Null),
            Value::String(s) => s
                .chars()
                .next()
                .map(|c| Value::String(c.to_string()))
                .unwrap_or(Value::Null),
            _ => Value::Null,
        },
        "last" => match &value {
            Value::Array(a) => a.last().cloned().unwrap_or(Value::Null),
            Value::String(s) => s
                .chars()
                .last()
                .map(|c| Value::String(c.to_string()))
                .unwrap_or(Value::Null),
            _ => Value::Null,
        },
        "reverse" => match &value {
            Value::Array(a) => Value::Array(a.iter().rev().cloned().collect()),
            Value::String(s) => Value::String(s.chars().rev().collect()),
            _ => value,
        },
        "join" => {
            let sep = filter.args.first().cloned().unwrap_or_default();
            match &value {
                Value::Array(a) => {
                    Value::String(a.iter().map(to_display).collect::<Vec<_>>().join(&sep))
                }
                _ => value,
            }
        }
        "default" => {
            if is_empty_value(&value) {
                Value::String(filter.args.first().cloned().unwrap_or_default())
            } else {
                value
            }
        }
        "escape" | "e" => Value::String(html_escape(&to_display(&value))),
        "json" => Value::String(serde_json::to_string(&value).unwrap_or_default()),
        _ => value,
    }
}

/// Apply a filter chain to a value (used by the renderer's scoped eval).
pub fn apply_filters(mut value: Value, filters: &[Filter]) -> Value {
    for filter in filters {
        value = apply_filter(value, filter);
    }
    value
}

/// Evaluate an echo expression against a single JSON context root.
pub fn eval_expr(expr: &Expr, ctx: &Value) -> Value {
    let mut value = match &expr.literal {
        Some(lit) => Value::String(lit.clone()),
        None => resolve_path(ctx, &expr.path).cloned().unwrap_or(Value::Null),
    };
    for filter in &expr.filters {
        value = if filter.name == "trans" {
            trans_filter(value, filter, |key| {
                resolve_path(
                    ctx,
                    &[Segment::Key("_lang".into()), Segment::Key(key.to_string())],
                )
                .cloned()
            })
        } else {
            apply_filter(value, filter)
        };
    }
    value
}

/// Convert a literal operand to a JSON value.
pub fn lit_value(l: &Lit) -> Value {
    match l {
        Lit::Str(s) => Value::String(s.clone()),
        Lit::Num(n) => serde_json::Number::from_f64(*n)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        Lit::Bool(b) => Value::Bool(*b),
        Lit::Null => Value::Null,
    }
}

fn value_as_f64(v: &Value) -> Option<f64> {
    v.as_f64()
}

/// Compare two JSON values (numeric when possible, display-order fallback).
pub fn compare_values(l: &Value, op: CmpOp, r: &Value) -> bool {
    match op {
        CmpOp::Eq => values_equal(l, r),
        CmpOp::Ne => !values_equal(l, r),
        _ => match (value_as_f64(l), value_as_f64(r)) {
            (Some(a), Some(b)) => match op {
                CmpOp::Lt => a < b,
                CmpOp::Le => a <= b,
                CmpOp::Gt => a > b,
                CmpOp::Ge => a >= b,
                _ => false,
            },
            _ => {
                let a = to_display(l);
                let b = to_display(r);
                match op {
                    CmpOp::Lt => a < b,
                    CmpOp::Le => a <= b,
                    CmpOp::Gt => a > b,
                    CmpOp::Ge => a >= b,
                    _ => false,
                }
            }
        },
    }
}

fn values_equal(l: &Value, r: &Value) -> bool {
    if l == r {
        return true;
    }
    match (l, r) {
        (Value::Null, _) | (_, Value::Null) => false,
        _ => {
            // Numeric loose equality: JSON 1 equals 1.0 (serde keeps the
            // integer and float representations distinct).
            if let (Some(a), Some(b)) = (value_as_f64(l), value_as_f64(r)) {
                return a == b;
            }
            to_display(l) == to_display(r)
        }
    }
}

fn operand_value(op: &Operand, ctx: &Value) -> Value {
    match op {
        Operand::Path(p) => resolve_path(ctx, p).cloned().unwrap_or(Value::Null),
        Operand::Lit(l) => lit_value(l),
    }
}

/// Evaluate a boolean condition against a single JSON context root.
/// Custom conditionals evaluate false here (no registry in scope) — the
/// renderer uses [eval_cond_with] with the engine's registry.
pub fn eval_cond(cond: &Cond, ctx: &Value) -> bool {
    eval_cond_with(cond, ctx, &|_, _, _| false)
}

/// Evaluate a condition with a custom-conditional resolver (name, args,
/// context root). Unknown/unregistered names evaluate false (fail closed).
pub fn eval_cond_with(
    cond: &Cond,
    ctx: &Value,
    custom: &dyn Fn(&str, &[Value], &Value) -> bool,
) -> bool {
    match cond {
        Cond::Or(parts) => parts.iter().any(|p| eval_cond_with(p, ctx, custom)),
        Cond::And(parts) => parts.iter().all(|p| eval_cond_with(p, ctx, custom)),
        Cond::Not(inner) => !eval_cond_with(inner, ctx, custom),
        Cond::Cmp { left, op, right } => {
            let l = operand_value(left, ctx);
            let r = operand_value(right, ctx);
            compare_values(&l, *op, &r)
        }
        Cond::Truthy(op) => is_truthy(&operand_value(op, ctx)),
        Cond::Custom { name, args } => {
            let values: Vec<Value> = args.iter().map(|a| operand_value(a, ctx)).collect();
            custom(name, &values, ctx)
        }
    }
}

pub fn is_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(false),
        Value::String(s) => !s.is_empty() && s != "0" && s != "false",
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

pub fn is_empty_value(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
        _ => false,
    }
}

fn title_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_ws = true;
    for c in s.chars() {
        if c.is_whitespace() {
            prev_ws = true;
            out.push(c);
        } else if prev_ws {
            out.extend(c.to_uppercase());
            prev_ws = false;
        } else {
            out.extend(c.to_lowercase());
        }
    }
    out
}

/// Render a JSON value to its display string.
pub fn to_display(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        Value::Array(_) | Value::Object(_) => serde_json::to_string(v).unwrap_or_default(),
    }
}

/// HTML-escape the five significant characters (G17 security facet).
pub fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_dot_paths_and_filters() {
        let e = parse_echo("user.name | upper").unwrap();
        assert_eq!(
            e.path,
            vec![Segment::Key("user".into()), Segment::Key("name".into())]
        );
        assert_eq!(e.filters.len(), 1);
        assert_eq!(e.filters[0].name, "upper");
        assert!(e.literal.is_none());
    }

    /// Wave 6 P3: a leading quoted string parses into `literal` (feeding
    /// the filter chain instead of a context lookup), and expressions
    /// serialized before the field existed still deserialize (the views
    /// disk cache keeps old ASTs).
    #[test]
    fn quoted_literal_parses_and_old_json_still_loads() {
        let e = parse_echo("\"a.b\" | trans").unwrap();
        assert_eq!(e.literal.as_deref(), Some("a.b"));
        assert!(e.path.is_empty());
        assert_eq!(e.filters[0].name, "trans");

        // Trailing junk after a literal still fails closed.
        assert!(parse_echo("\"a\" bogus").is_err());

        // Cache entries written before `literal` existed.
        let old: Expr =
            serde_json::from_str("{\"path\":[{\"Key\":\"user\"}],\"filters\":[]}").unwrap();
        assert!(old.literal.is_none());
        assert_eq!(old.path.len(), 1);
    }

    /// Wave 6 P3: the `trans` filter resolves literal and path keys
    /// through the `_lang` flat map; a miss degrades to the key itself.
    #[test]
    fn trans_filter_resolves_lang_map_and_degrades() {
        let ctx = json!({
            "_lang": { "dashboard.title": "Dashboard", "appointments.booked": "Booked" },
            "status_key": "appointments.booked"
        });
        // Literal key.
        let e = parse_echo("\"dashboard.title\" | trans").unwrap();
        assert_eq!(to_display(&eval_expr(&e, &ctx)), "Dashboard");
        // Path value used as the key.
        let e = parse_echo("status_key | trans").unwrap();
        assert_eq!(to_display(&eval_expr(&e, &ctx)), "Booked");
        // Miss → the key itself (same contract as @lang).
        let e = parse_echo("\"missing.key\" | trans").unwrap();
        assert_eq!(to_display(&eval_expr(&e, &ctx)), "missing.key");
        // Explicit argument wins over the input and still resolves.
        let e = parse_echo("status_key | trans(\"dashboard.title\")").unwrap();
        assert_eq!(to_display(&eval_expr(&e, &ctx)), "Dashboard");
        // Without an injected `_lang` map everything degrades safely.
        let bare = json!({ "status_key": "k2" });
        let e = parse_echo("status_key | trans").unwrap();
        assert_eq!(to_display(&eval_expr(&e, &bare)), "k2");
    }

    #[test]
    fn parses_indexed_paths_and_filter_args() {
        let e = parse_echo("items[0].title | default(\"n/a\")").unwrap();
        assert_eq!(e.path[1], Segment::Index(0));
        assert_eq!(e.filters[0].args, vec!["n/a".to_string()]);
    }

    #[test]
    fn evaluates_filters() {
        let ctx = json!({ "name": "ada", "tags": ["a", "b"] });
        let e = parse_echo("name | upper").unwrap();
        assert_eq!(to_display(&eval_expr(&e, &ctx)), "ADA");
        let e = parse_echo("tags | join(\", \")").unwrap();
        assert_eq!(to_display(&eval_expr(&e, &ctx)), "a, b");
        let e = parse_echo("missing | default(\"none\")").unwrap();
        assert_eq!(to_display(&eval_expr(&e, &ctx)), "none");
    }

    #[test]
    fn conditions_evaluate() {
        let ctx = json!({ "count": 3, "role": "admin", "active": true });
        assert!(eval_cond(&parse_cond("count > 2").unwrap(), &ctx));
        assert!(eval_cond(
            &parse_cond("role == \"admin\" && active").unwrap(),
            &ctx
        ));
        assert!(!eval_cond(
            &parse_cond("count < 2 || !active").unwrap(),
            &ctx
        ));
        assert!(eval_cond(&parse_cond("count >= 3").unwrap(), &ctx));
    }

    #[test]
    fn escaping_covers_all_significant_chars() {
        assert_eq!(
            html_escape("<a href=\"x\">&'"),
            "&lt;a href=&quot;x&quot;&gt;&amp;&#39;"
        );
    }

    #[test]
    fn truthiness_matches_blade_intuition() {
        assert!(!is_truthy(&json!(null)));
        assert!(!is_truthy(&json!("")));
        assert!(!is_truthy(&json!(0)));
        assert!(!is_truthy(&json!(false)));
        assert!(is_truthy(&json!("x")));
        assert!(is_truthy(&json!(1)));
        assert!(is_truthy(&json!([1])));
    }

    #[test]
    fn operand_parsing_covers_literals_and_paths() {
        assert_eq!(parse_operand_str("12").unwrap(), Operand::Lit(Lit::Num(12.0)));
        assert_eq!(
            parse_operand_str("\"hi\"").unwrap(),
            Operand::Lit(Lit::Str("hi".into()))
        );
        assert_eq!(parse_operand_str("true").unwrap(), Operand::Lit(Lit::Bool(true)));
        assert!(matches!(
            parse_operand_str("user.name").unwrap(),
            Operand::Path(_)
        ));
    }
}
