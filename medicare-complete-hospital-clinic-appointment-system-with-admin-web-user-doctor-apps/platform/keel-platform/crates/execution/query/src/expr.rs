//! Validated raw SQL expressions.
//!
//! # Why this exists
//!
//! The builder refuses free-form SQL: every identifier is validated and
//! every value is bound. That is the right default, and it is why the
//! framework is safe by construction.
//!
//! But a **framework** must let the apps built on top of it express what
//! the framework does not model. A reporting app needs `JSON_TABLE`; a
//! MySQL-native app needs `GROUP_CONCAT`; a search app needs a custom
//! collation. Refusing outright does not make the framework safer — it makes
//! it unusable, and pushes the author to a raw `sqlx::query` that bypasses
//! every guarantee the framework offers.
//!
//! So the escape is **typed and validated**, not absent:
//!
//! * the text is checked for the shapes that make a statement dangerous
//!   (a statement separator, a comment marker, an unbalanced quote or
//!   parenthesis, a statement keyword at the head);
//! * the **values are bound**, never interpolated: an expression carries
//!   its own binds and the renderer pushes them in place, so a raw
//!   expression is as injection-proof as the rest of the builder;
//! * a validation failure is a **build error**, not a runtime surprise.
//!
//! The trust model is the same as every typed builder that offers a raw
//! escape: the *author* is trusted (they write the SQL text), the *end
//! user* is not (their values are always bound).

use kelson_foundation::AppError;

use crate::select::Value;

/// The maximum length of a raw expression's text (a bound, not a hope).
pub const MAX_RAW_LENGTH: usize = 8_192;

/// A statement keyword that may not head an expression.
const STATEMENT_HEADS: &[&str] = &[
    "SELECT", "INSERT", "UPDATE", "DELETE", "DROP", "ALTER", "CREATE", "TRUNCATE", "GRANT",
    "REVOKE", "SET", "CALL", "EXEC", "EXECUTE", "MERGE", "REPLACE", "WITH", "SHOW", "USE", "BEGIN",
    "COMMIT", "ROLLBACK", "VACUUM", "ANALYZE", "EXPLAIN",
];

/// A validated raw SQL expression with its own bound values.
#[derive(Debug, Clone, PartialEq)]
pub struct SqlExpr {
    text: String,
    binds: Vec<Value>,
}

impl SqlExpr {
    /// Build a raw expression from text and its bound values.
    ///
    /// The text is validated; the values are bound. `?` in the text is a
    /// bind placeholder (the dialect's own form).
    ///
    /// # Errors
    ///
    /// Refuses an empty expression, one longer than [`MAX_RAW_LENGTH`], one
    /// containing a statement separator (`;`), a comment marker (`--`, `#`,
    /// `/*`), an unbalanced quote or parenthesis, or one that begins with a
    /// statement keyword.
    pub fn new(text: impl Into<String>, binds: Vec<Value>) -> Result<Self, AppError> {
        let text = text.into();
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Err(AppError::validation("a raw expression cannot be empty"));
        }
        if text.len() > MAX_RAW_LENGTH {
            return Err(AppError::validation(format!(
                "a raw expression cannot exceed {MAX_RAW_LENGTH} bytes (this one is {})",
                text.len()
            )));
        }
        if text.contains(';') {
            return Err(AppError::validation(
                "a raw expression cannot contain a statement separator (';')",
            ));
        }
        for marker in ["--", "#", "/*", "*/"] {
            if text.contains(marker) {
                return Err(AppError::validation(format!(
                    "a raw expression cannot contain a comment marker ({marker})"
                )));
            }
        }
        check_balanced(&text)?;
        let head = trimmed
            .split(|c: char| c.is_whitespace() || c == '(')
            .next()
            .unwrap_or("")
            .to_ascii_uppercase();
        if STATEMENT_HEADS.contains(&head.as_str()) {
            return Err(AppError::validation(format!(
                "a raw expression cannot begin with the statement keyword {head}"
            )));
        }
        Ok(Self {
            text: trimmed.to_string(),
            binds,
        })
    }

    /// A raw expression with no binds.
    pub fn raw(text: impl Into<String>) -> Result<Self, AppError> {
        Self::new(text, Vec::new())
    }

    /// A function call: `name(arg, arg, ...)`.
    ///
    /// The function name is validated as an identifier (so it cannot smuggle
    /// SQL), and the arguments are rendered as expressions.
    pub fn call(name: &str, args: &[SqlExpr]) -> Result<Self, AppError> {
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
            || name.starts_with(|c: char| c.is_ascii_digit())
        {
            return Err(AppError::validation(format!(
                "{name:?} is not a valid function name"
            )));
        }
        let rendered: Vec<String> = args.iter().map(|a| a.text.clone()).collect();
        Self::new(format!("{name}({})", rendered.join(", ")), Vec::new())
    }

    /// `GROUP_CONCAT(expression [ORDER BY ...] [SEPARATOR 'x'])`.
    pub fn group_concat(expression: &SqlExpr, separator: Option<&str>) -> Result<Self, AppError> {
        let mut text = format!("GROUP_CONCAT({})", expression.text);
        if let Some(sep) = separator {
            if sep.contains('\'') {
                return Err(AppError::validation(
                    "the GROUP_CONCAT separator cannot contain a quote",
                ));
            }
            text.push_str(&format!(" SEPARATOR '{sep}'"));
        }
        Self::new(text, Vec::new())
    }

    /// MySQL's `JSON_TABLE`: expand a JSON document into rows.
    ///
    /// `columns` is the column definition list (`name type PATH '...'`), and
    /// `binds` are the values bound into the document path.
    pub fn json_table(
        document: &SqlExpr,
        path: &str,
        columns: &str,
        binds: Vec<Value>,
    ) -> Result<Self, AppError> {
        if path.contains('\'') {
            return Err(AppError::validation(
                "the JSON_TABLE path cannot contain a quote",
            ));
        }
        if columns.is_empty() {
            return Err(AppError::validation("JSON_TABLE needs at least one column"));
        }
        Self::new(
            format!(
                "JSON_TABLE({} , '{path}' COLUMNS({columns}))",
                document.text
            ),
            binds,
        )
    }

    /// The expression's text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The expression's bound values.
    pub fn binds(&self) -> &[Value] {
        &self.binds
    }

    /// Render the expression, pushing its binds into the statement's
    /// parameter list in place.
    ///
    /// The placeholders are the dialect's own form, so the statement's bind
    /// order stays continuous (the expression's binds are pushed exactly
    /// where the expression appears).
    pub(crate) fn render(
        &self,
        params: &mut Vec<Value>,
        dialect: &dyn crate::dialect::Dialect,
    ) -> String {
        for bind in &self.binds {
            params.push(bind.clone());
        }
        // The text carries `?` placeholders; the dialect renders its own
        // form. A raw expression's placeholders are positional, so the
        // simplest correct translation is: the text's `?` becomes the
        // dialect's placeholder, in order.
        let mut out = String::with_capacity(self.text.len());
        let mut index = 0usize;
        let mut in_quote: Option<char> = None;
        let chars: Vec<char> = self.text.chars().collect();
        let mut i = 0usize;
        while i < chars.len() {
            let c = chars[i];
            match in_quote {
                Some(q) => {
                    out.push(c);
                    if c == q {
                        in_quote = None;
                    }
                    i += 1;
                }
                None => {
                    if c == '\'' || c == '"' || c == '`' {
                        in_quote = Some(c);
                        out.push(c);
                        i += 1;
                        continue;
                    }
                    if c == '?' {
                        index += 1;
                        out.push_str(&dialect.placeholder(index));
                        i += 1;
                        continue;
                    }
                    out.push(c);
                    i += 1;
                }
            }
        }
        out
    }
}

/// Check that the quotes and the parentheses in a raw expression balance.
fn check_balanced(text: &str) -> Result<(), AppError> {
    let mut quote: Option<char> = None;
    let mut depth: i64 = 0;
    for c in text.chars() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
            }
            None => match c {
                '\'' | '"' | '`' => quote = Some(c),
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth < 0 {
                        return Err(AppError::validation(
                            "a raw expression has an unbalanced parenthesis",
                        ));
                    }
                }
                _ => {}
            },
        }
    }
    if quote.is_some() {
        return Err(AppError::validation(
            "a raw expression has an unterminated quote",
        ));
    }
    if depth != 0 {
        return Err(AppError::validation(
            "a raw expression has an unbalanced parenthesis",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_expression_is_accepted() {
        let e = SqlExpr::raw("amount + 1").expect("accepted");
        assert_eq!(e.text(), "amount + 1");
        assert!(e.binds().is_empty());
    }

    #[test]
    fn a_statement_separator_is_refused() {
        let err = SqlExpr::raw("1; DROP TABLE users").expect_err("refused");
        assert!(err.public_message.contains("statement separator"));
    }

    #[test]
    fn a_comment_marker_is_refused() {
        for marker in ["1 -- x", "1 # x", "1 /* x */"] {
            let err = SqlExpr::raw(marker).expect_err("refused");
            assert!(err.public_message.contains("comment marker"), "{marker}");
        }
    }

    #[test]
    fn a_statement_keyword_is_refused() {
        for head in ["SELECT 1", "DROP TABLE t", "insert into t values (1)"] {
            let err = SqlExpr::raw(head).expect_err("refused");
            assert!(err.public_message.contains("statement keyword"), "{head}");
        }
    }

    #[test]
    fn an_unbalanced_expression_is_refused() {
        assert!(SqlExpr::raw("(a + b").is_err());
        assert!(SqlExpr::raw("a + b)").is_err());
        assert!(SqlExpr::raw("'unterminated").is_err());
    }

    #[test]
    fn an_empty_expression_is_refused() {
        assert!(SqlExpr::raw("   ").is_err());
    }

    #[test]
    fn an_oversized_expression_is_refused() {
        let big = "a".repeat(MAX_RAW_LENGTH + 1);
        assert!(SqlExpr::raw(big).is_err());
    }

    #[test]
    fn a_function_call_is_typed() {
        let arg = SqlExpr::raw("amount").expect("accepted");
        let call = SqlExpr::call("COALESCE", &[arg]).expect("accepted");
        assert_eq!(call.text(), "COALESCE(amount)");
        assert!(SqlExpr::call("bad name", &[]).is_err());
        assert!(SqlExpr::call("1bad", &[]).is_err());
    }

    #[test]
    fn the_json_table_shape_is_built() {
        let doc = SqlExpr::new("doc", vec![Value::Text("{}".into())]).expect("accepted");
        let table = SqlExpr::json_table(&doc, "$.items[*]", "id INT PATH '$.id'", vec![])
            .expect("accepted");
        assert!(table
            .text()
            .starts_with("JSON_TABLE(doc , '$.items[*]' COLUMNS("));
        assert!(SqlExpr::json_table(&doc, "bad'path", "id INT", vec![]).is_err());
    }

    #[test]
    fn the_placeholders_are_translated_per_dialect() {
        let mut params: Vec<Value> = Vec::new();
        let e = SqlExpr::new(
            "amount > ? AND tenant = ?",
            vec![Value::Int(5), Value::Text("t".into())],
        )
        .expect("accepted");
        let pg = e.render(&mut params, &crate::dialect::POSTGRES);
        assert_eq!(pg, "amount > $1 AND tenant = $2");
        assert_eq!(params.len(), 2);

        let mut params: Vec<Value> = Vec::new();
        let my = e.render(&mut params, &crate::dialect::MYSQL);
        assert_eq!(my, "amount > ? AND tenant = ?");
        assert_eq!(params.len(), 2);
    }

    #[test]
    fn a_question_mark_inside_a_string_is_not_a_placeholder() {
        let e = SqlExpr::new("note = 'why?'", vec![]).expect("accepted");
        let mut params: Vec<Value> = Vec::new();
        let rendered = e.render(&mut params, &crate::dialect::POSTGRES);
        assert_eq!(rendered, "note = 'why?'");
        assert!(params.is_empty());
    }
}
