//! Policy engine (Sentinel S5 §10): a restricted boolean grammar over event
//! facts — no arbitrary code, no scripting. Evaluation is total: every event
//! resolves to exactly one terminal routing (rule-merged exporters or the
//! default route; `drop` wins over everything).

use kelson_telemetry::Level;
use serde::{Deserialize, Serialize};

/// Event facts the grammar can see.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    Environment,
    Level,
    Sensitivity,
    Category,
    Schema,
    TenantId,
    Incident,
    Durability,
    Sampled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    Ne,
    Ge,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    Num(f64),
    Bool(bool),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    True,
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Cmp {
        field: Field,
        op: CmpOp,
        value: Value,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Retain,
    Drop,
    Export(String),
    Redact(Vec<String>),
    Hash(Vec<String>),
    Sample(f64),
    Alert,
}

impl Action {
    /// Parse `retain | drop | alert | export:<name> | redact:a,b | hash:a,b | sample:<0..1>`.
    pub fn parse(raw: &str) -> Result<Action, String> {
        let raw = raw.trim();
        let (head, arg) = match raw.split_once(':') {
            Some((h, a)) => (h.trim(), Some(a.trim())),
            None => (raw, None),
        };
        match head {
            "retain" => Ok(Action::Retain),
            "drop" => Ok(Action::Drop),
            "alert" => Ok(Action::Alert),
            "export" => arg
                .filter(|s| !s.is_empty())
                .map(|s| Action::Export(s.to_string()))
                .ok_or_else(|| "export needs a target: export:<name>".to_string()),
            "redact" | "hash" => {
                let fields: Vec<String> = arg
                    .unwrap_or_default()
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                if fields.is_empty() {
                    return Err(format!("{head} needs field list: {head}:a,b"));
                }
                if head == "redact" {
                    Ok(Action::Redact(fields))
                } else {
                    Ok(Action::Hash(fields))
                }
            }
            "sample" => {
                let n: f64 = arg
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| "sample needs a float rate".to_string())?;
                if !(0.0..=1.0).contains(&n) {
                    return Err("sample rate must be within [0,1]".to_string());
                }
                Ok(Action::Sample(n))
            }
            other => Err(format!("unknown action {other:?}")),
        }
    }
}

/// One policy rule (`[[telemetry.policy]]` entry).
#[derive(Debug, Clone)]
pub struct PolicyRule {
    pub name: String,
    pub when: Expr,
    pub actions: Vec<Action>,
}

/// Merged outcome of all matching rules for one event.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PolicyDecision {
    pub drop: bool,
    pub retain: bool,
    pub alert: bool,
    pub exporters: Vec<String>,
    pub redact: Vec<String>,
    pub hash: Vec<String>,
    pub sample: Option<f64>,
}

/// Runtime facts supplied alongside the event (control-plane state).
#[derive(Debug, Clone)]
pub struct PolicyFacts {
    pub incident_active: bool,
}

// --- parser (recursive descent) ---

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Ident(String),
    Str(String),
    Num(f64),
    Eq,
    Ne,
    Ge,
    And,
    Or,
    LParen,
    RParen,
    Bool(bool),
}

fn tokenize(src: &str) -> Result<Vec<Token>, String> {
    let mut out = Vec::new();
    let bytes: Vec<char> = src.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            c if c.is_whitespace() => i += 1,
            '(' => {
                out.push(Token::LParen);
                i += 1;
            }
            ')' => {
                out.push(Token::RParen);
                i += 1;
            }
            '=' if bytes.get(i + 1) == Some(&'=') => {
                out.push(Token::Eq);
                i += 2;
            }
            '!' if bytes.get(i + 1) == Some(&'=') => {
                out.push(Token::Ne);
                i += 2;
            }
            '>' if bytes.get(i + 1) == Some(&'=') => {
                out.push(Token::Ge);
                i += 2;
            }
            '&' if bytes.get(i + 1) == Some(&'&') => {
                out.push(Token::And);
                i += 2;
            }
            '|' if bytes.get(i + 1) == Some(&'|') => {
                out.push(Token::Or);
                i += 2;
            }
            '"' => {
                let start = i + 1;
                i += 1;
                let mut s = String::new();
                while i < bytes.len() && bytes[i] != '"' {
                    s.push(bytes[i]);
                    i += 1;
                }
                if i >= bytes.len() {
                    return Err("unterminated string literal".into());
                }
                i += 1; // closing quote
                let _ = start;
                out.push(Token::Str(s));
            }
            c if c.is_ascii_digit() || c == '.' => {
                let start = i;
                while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == '.') {
                    i += 1;
                }
                let raw: String = bytes[start..i].iter().collect();
                let n: f64 = raw.parse().map_err(|_| format!("bad number {raw:?}"))?;
                out.push(Token::Num(n));
            }
            c if c.is_ascii_alphabetic() || c == '_' => {
                let start = i;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == '_') {
                    i += 1;
                }
                let word: String = bytes[start..i].iter().collect();
                match word.as_str() {
                    "true" => out.push(Token::Bool(true)),
                    "false" => out.push(Token::Bool(false)),
                    _ => out.push(Token::Ident(word)),
                }
            }
            other => return Err(format!("unexpected character {other:?}")),
        }
    }
    Ok(out)
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0 }
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn next(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn parse_or(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_and()?;
        while self.peek() == Some(&Token::Or) {
            self.next();
            let right = self.parse_and()?;
            left = Expr::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_primary()?;
        while self.peek() == Some(&Token::And) {
            self.next();
            let right = self.parse_primary()?;
            left = Expr::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_primary(&mut self) -> Result<Expr, String> {
        match self.next() {
            Some(Token::LParen) => {
                let inner = self.parse_or()?;
                if self.next() != Some(Token::RParen) {
                    return Err("missing closing paren".into());
                }
                Ok(inner)
            }
            Some(Token::Bool(true)) => Ok(Expr::True),
            Some(Token::Bool(false)) => Ok(Expr::Cmp {
                field: Field::Sampled,
                op: CmpOp::Eq,
                value: Value::Bool(false), // always false against any fact? see eval
            }),
            Some(Token::Ident(name)) => {
                let field = parse_field(&name)?;
                let op = match self.next() {
                    Some(Token::Eq) => CmpOp::Eq,
                    Some(Token::Ne) => CmpOp::Ne,
                    Some(Token::Ge) => CmpOp::Ge,
                    other => return Err(format!("expected comparison operator, got {other:?}")),
                };
                let value = match self.next() {
                    Some(Token::Str(s)) => Value::Str(s),
                    Some(Token::Num(n)) => Value::Num(n),
                    Some(Token::Bool(b)) => Value::Bool(b),
                    Some(Token::Ident(s)) => Value::Str(s),
                    other => return Err(format!("expected value, got {other:?}")),
                };
                Ok(Expr::Cmp { field, op, value })
            }
            other => Err(format!("unexpected token {other:?} in policy expr")),
        }
    }
}

fn parse_field(name: &str) -> Result<Field, String> {
    match name {
        "environment" | "env" => Ok(Field::Environment),
        "level" => Ok(Field::Level),
        "sensitivity" | "classification" => Ok(Field::Sensitivity),
        "category" => Ok(Field::Category),
        "schema" => Ok(Field::Schema),
        "tenant_id" | "tenant" => Ok(Field::TenantId),
        "incident" => Ok(Field::Incident),
        "durability" => Ok(Field::Durability),
        "sampled" => Ok(Field::Sampled),
        other => Err(format!("unknown policy field {other:?}")),
    }
}

/// Parse a `when` expression source string.
pub fn parse_expr(src: &str) -> Result<Expr, String> {
    let tokens = tokenize(src)?;
    if tokens.is_empty() {
        return Err("empty policy expression".into());
    }
    let mut p = Parser::new(tokens);
    let expr = p.parse_or()?;
    if p.peek().is_some() {
        return Err(format!("trailing tokens in policy expression {src:?}"));
    }
    Ok(expr)
}

impl PolicyRule {
    pub fn parse(name: impl Into<String>, when: &str, actions: &[String]) -> Result<Self, String> {
        Ok(Self {
            name: name.into(),
            when: parse_expr(when)?,
            actions: actions
                .iter()
                .map(|a| Action::parse(a))
                .collect::<Result<_, _>>()?,
        })
    }
}

// --- evaluation ---

fn level_rank(level: Level) -> u8 {
    match level {
        Level::Debug => 0,
        Level::Info => 1,
        Level::Notice => 2,
        Level::Warn => 3,
        Level::Error => 4,
        Level::Critical => 5,
    }
}

fn fact_str(event: &kelson_telemetry::Event, facts: &PolicyFacts, field: Field) -> Option<String> {
    Some(match field {
        Field::Environment => event.service.environment.clone(),
        Field::Level => event.payload.level_hint()?.as_str().to_string(),
        Field::Sensitivity => serde_json::to_string(&event.sensitivity)
            .ok()?
            .trim_matches('"')
            .to_string(),
        Field::Category => serde_json::to_string(&event.payload.category())
            .ok()?
            .trim_matches('"')
            .to_string(),
        Field::Schema => event.schema.clone(),
        Field::TenantId => event.context.tenant_id.map(|t| t.to_string())?,
        Field::Incident => {
            if facts.incident_active || event.context.incident_id.is_some() {
                "active".into()
            } else {
                "inactive".into()
            }
        }
        Field::Durability => serde_json::to_string(&event.durability)
            .ok()?
            .trim_matches('"')
            .to_string(),
        Field::Sampled => return None,
    })
}

fn eval_cmp(
    event: &kelson_telemetry::Event,
    facts: &PolicyFacts,
    field: Field,
    op: CmpOp,
    value: &Value,
) -> bool {
    if field == Field::Sampled {
        // `sampled == false` literal support (used only by the parser's
        // constant-false lowering); runtime sampling lives in the sampler.
        return matches!((op, value), (CmpOp::Eq, Value::Bool(false)));
    }
    if field == Field::Level {
        let Some(actual) = event.payload.level_hint() else {
            return false;
        };
        let want = match value {
            Value::Str(s) => match Level::parse(s) {
                Some(l) => l,
                None => return false,
            },
            _ => return false,
        };
        return match op {
            CmpOp::Eq => actual == want,
            CmpOp::Ne => actual != want,
            CmpOp::Ge => level_rank(actual) >= level_rank(want),
        };
    }
    let Some(actual) = fact_str(event, facts, field) else {
        return matches!((op, value), (CmpOp::Ne, Value::Str(_)));
    };
    match (op, value) {
        (CmpOp::Eq, Value::Str(s)) => actual.eq_ignore_ascii_case(s),
        (CmpOp::Ne, Value::Str(s)) => !actual.eq_ignore_ascii_case(s),
        (CmpOp::Eq, Value::Bool(b)) => (actual == "true" || actual == "active") == *b,
        (CmpOp::Ge, Value::Num(n)) => actual.parse::<f64>().map(|a| a >= *n).unwrap_or(false),
        _ => false,
    }
}

fn eval_expr(event: &kelson_telemetry::Event, facts: &PolicyFacts, expr: &Expr) -> bool {
    match expr {
        Expr::True => true,
        Expr::And(a, b) => eval_expr(event, facts, a) && eval_expr(event, facts, b),
        Expr::Or(a, b) => eval_expr(event, facts, a) || eval_expr(event, facts, b),
        Expr::Cmp { field, op, value } => eval_cmp(event, facts, *field, *op, value),
    }
}

/// The engine: evaluate all rules in order, merge actions into one decision.
#[derive(Debug, Clone, Default)]
pub struct PolicyEngine {
    pub rules: Vec<PolicyRule>,
    pub default_route: Vec<String>,
}

impl PolicyEngine {
    pub fn new(default_route: Vec<String>) -> Self {
        Self {
            rules: Vec::new(),
            default_route,
        }
    }

    pub fn evaluate(&self, event: &kelson_telemetry::Event, facts: &PolicyFacts) -> PolicyDecision {
        let mut d = PolicyDecision::default();
        for rule in &self.rules {
            if !eval_expr(event, facts, &rule.when) {
                continue;
            }
            for action in &rule.actions {
                match action {
                    Action::Drop => d.drop = true,
                    Action::Retain => d.retain = true,
                    Action::Alert => d.alert = true,
                    Action::Export(name) => {
                        if !d.exporters.contains(name) {
                            d.exporters.push(name.clone());
                        }
                    }
                    Action::Redact(fields) => d.redact.extend(fields.iter().cloned()),
                    Action::Hash(fields) => d.hash.extend(fields.iter().cloned()),
                    Action::Sample(rate) => {
                        d.sample = Some(d.sample.map(|s: f64| s.min(*rate)).unwrap_or(*rate))
                    }
                }
            }
        }
        if d.exporters.is_empty() && !d.drop {
            d.exporters = self.default_route.clone();
        }
        d
    }

    /// Tenant-consistency guard (S5 §20): an event whose payload claims a
    /// different tenant than its propagation context is a policy violation;
    /// drop it loudly rather than leak it across isolation boundaries.
    pub fn enforce_tenant_consistency(event: &kelson_telemetry::Event) -> Result<(), String> {
        let Some(ctx_tenant) = event.context.tenant_id else {
            return Ok(());
        };
        if let Some(fields) = event.payload.fields_ref() {
            if let Some(serde_json::Value::String(claimed)) = fields.get("tenant_id") {
                if claimed != &ctx_tenant.to_string() {
                    return Err(format!(
                        "tenant mismatch: context {ctx_tenant} vs payload {claimed}"
                    ));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kelson_foundation::TelemetryContext;
    use kelson_telemetry::{
        EventEnvelope, Fields, LogPayload, ServiceIdentity, TelemetryPayload, SCHEMA_LOG,
        SCHEMA_LOG_VERSION,
    };

    pub(crate) fn log_event(env: &str, level: Level, msg: &str) -> kelson_telemetry::Event {
        EventEnvelope::new(
            SCHEMA_LOG,
            SCHEMA_LOG_VERSION,
            ServiceIdentity::new("api", env),
            TelemetryContext::root(),
            TelemetryPayload::Log(LogPayload {
                level,
                message: msg.into(),
                target: None,
                fields: Fields::new(),
                extra: Fields::new(),
            }),
        )
    }

    fn facts() -> PolicyFacts {
        PolicyFacts {
            incident_active: false,
        }
    }

    #[test]
    fn parses_grammar() {
        let e = parse_expr(r#"environment == "production" && level >= "error""#).unwrap();
        let ev = log_event("production", Level::Error, "x");
        assert!(eval_expr(&ev, &facts(), &e));
        let ok = log_event("production", Level::Info, "x");
        assert!(!eval_expr(&ok, &facts(), &e));
    }

    #[test]
    fn rejects_unknown_fields_and_bad_tokens() {
        assert!(parse_expr("bogus == 1").is_err());
        assert!(parse_expr("level >").is_err());
        assert!(parse_expr("level >= error &&").is_err());
        assert!(Action::parse("export:").is_err());
        assert!(Action::parse("sample:1.5").is_err());
    }

    #[test]
    fn decisions_merge_and_default_route_is_total() {
        let mut engine = PolicyEngine::new(vec!["console".into()]);
        engine.rules.push(
            PolicyRule::parse(
                "prod-errors",
                r#"environment == "prod" && level >= "error""#,
                &["retain".into(), "export:error_file".into(), "alert".into()],
            )
            .unwrap(),
        );
        engine.rules.push(
            PolicyRule::parse(
                "pii",
                r#"sensitivity == "secret""#,
                &["redact:password".into()],
            )
            .unwrap(),
        );
        let err = log_event("prod", Level::Error, "db down");
        let d = engine.evaluate(&err, &facts());
        assert!(d.retain && d.alert);
        assert_eq!(d.exporters, vec!["error_file".to_string()]);
        let info = log_event("prod", Level::Info, "ok");
        let d2 = engine.evaluate(&info, &facts());
        assert_eq!(
            d2.exporters,
            vec!["console".to_string()],
            "totality: default route"
        );
    }

    #[test]
    fn drop_wins() {
        let mut engine = PolicyEngine::new(vec!["console".into()]);
        engine.rules.push(
            PolicyRule::parse(
                "no-debug-prod",
                r#"environment == "prod" && level == "debug""#,
                &["drop".into()],
            )
            .unwrap(),
        );
        let dbg = log_event("prod", Level::Debug, "chatter");
        let d = engine.evaluate(&dbg, &facts());
        assert!(d.drop);
    }

    #[test]
    fn tenant_consistency_guard() {
        let mut ev = log_event("dev", Level::Info, "x");
        ev.context.tenant_id = Some(kelson_foundation::TenantId::new());
        assert!(PolicyEngine::enforce_tenant_consistency(&ev).is_ok());
        if let TelemetryPayload::Log(l) = &mut ev.payload {
            l.fields.insert(
                "tenant_id".into(),
                serde_json::json!("11111111-1111-7111-8111-111111111111"),
            );
        }
        assert!(PolicyEngine::enforce_tenant_consistency(&ev).is_err());
    }

    #[test]
    fn incident_fact_matches() {
        let e = parse_expr(r#"incident == "active""#).unwrap();
        let ev = log_event("prod", Level::Info, "x");
        let mut f = facts();
        assert!(!eval_expr(&ev, &f, &e));
        f.incident_active = true;
        assert!(eval_expr(&ev, &f, &e));
    }
}
