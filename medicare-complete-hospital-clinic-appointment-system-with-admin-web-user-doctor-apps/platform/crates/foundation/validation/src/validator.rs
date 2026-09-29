//! Collect-all field validation (S2 item 17, L08/L11): the FormRequest-role engine.
//!
//! The manifest rule grammar is 1:1 with the [`Rule`] enum: [`Rule::from_token`]
//! parses tokens, the registry gate `GRAPH-VALIDATION-RULE` rejects anything
//! outside the grammar at boot, and the HTTP edge compiles the survivors into
//! a [`Validator`]. Wave 4 Phase 1 grew the grammar from the Wave 1 core to
//! the type / string / numeric battery (incl. `gt:<n|field>` field refs);
//! Phase 2 added the conditional family, cross-field rules and `bail`; the
//! date/file/auth and DB rules land in the following phases of the wave.
//!
//! All violations are collected (never fail-fast) and reported as ONE AppError
//! with a `fields` meta map (field -> messages[]) so transports get a stable
//! wire shape. Domain invariants stay in the domain; these rules check input
//! shape only. Regex rules are deliberately SUPERSEDED by closed grammars
//! (same stance as route constraints): there is no `regex:` token by design.
//!
//! Presence semantics mirror Laravel's implicit-rules model: an absent key
//! runs only implicit rules (`required`, `present`, `accepted`, `filled`,
//! `missing*`, `required_*`); a present `null` skips the rest ONLY when the
//! field declares `nullable`; a whitespace-only string drops to
//! implicit-only as well (Laravel's `presentOrRuleIsImplicit` quirk), so an
//! optional `""` skips format rules.

use keel_foundation::{AppError, PrincipalId};
use serde_json::Value;

use crate::checks::{check, check_presence, is_excluded};

/// One validation rule applied to a field.
#[derive(Debug, Clone, PartialEq)]
pub enum Rule {
    // ------------------------------------------------------- Wave 1 core --
    /// Present, non-null, non-empty (whitespace-only strings and empty arrays fail).
    Required,
    /// Minimum size: string char count, array length, or numeric value.
    Min(u64),
    /// Maximum size: string char count, array length, or numeric value.
    Max(u64),
    /// Structural email: one '@', non-empty local, dotted domain.
    Email,
    /// http(s) URL with a non-empty host.
    Url,
    /// Canonical UUID (any version).
    Uuid,
    /// E.164 phone: '+' followed by 8..=15 digits.
    E164,
    /// String must equal one of the allowed values.
    OneOf(Vec<String>),
    /// Numeric value in an inclusive range.
    Range(i64, i64),
    /// RFC-3339 timestamp or plain YYYY-MM-DD date.
    Date,
    // --------------------------------------------------- Wave 4: types ----
    /// Present-null skips every other rule (structural, never `check`ed).
    Nullable,
    /// JSON bool, `0`/`1` or `"0"`/`"1"` (Laravel's non-strict set).
    Boolean,
    /// Integer-valued number, or a string that parses as an integer.
    Integer,
    /// Any JSON number, or a finite numeric string.
    Numeric,
    /// JSON string (grammar token `string`).
    String,
    /// JSON array.
    Array,
    /// String payloads must decode as JSON; structured values already are
    /// JSON. `null` fails unless the field is `nullable`.
    Json,
    /// Every char in the ASCII range (empty passes).
    Ascii,
    // ------------------------------------------------ Wave 4: strings -----
    /// Non-empty Unicode letters only.
    Alpha,
    /// Non-empty letters + numbers (integer numbers pass too, as in Laravel).
    AlphaNum,
    /// Letters + numbers + `-`/`_`.
    AlphaDash,
    /// String equals its own Unicode lowercase.
    Lowercase,
    /// String equals its own Unicode uppercase.
    Uppercase,
    /// String starts with the given prefix.
    StartsWith(String),
    /// String ends with the given suffix.
    EndsWith(String),
    /// String contains the given substring (docs/19 string family — Laravel's
    /// own `contains` rule tests array membership instead).
    Contains(String),
    /// String does NOT start with the given prefix.
    DoesntStartWith(String),
    /// String does NOT end with the given suffix.
    DoesntEndWith(String),
    /// String does NOT contain the given substring.
    DoesntContain(String),
    /// `#` + 3/4/6/8 hex digits (the `#` is required).
    HexColor,
    /// Six octets of two hex digits separated by `:` or `-`.
    MacAddress,
    /// Member of the curated IANA timezone list (bounded subset of tzdata).
    Timezone,
    /// 26-char Crockford base32, uppercase (I, L, O, U excluded).
    Ulid,
    // ----------------------------------------------- Wave 4: numeric ------
    /// Numeric with EXACTLY n decimal places (Laravel 12 semantics).
    Decimal(u8),
    /// Pure digit string of exactly n digits.
    Digits(u64),
    /// Pure digit string with length in the inclusive bounds.
    DigitsBetween(u64, u64),
    /// Value is an integer multiple of n (n > 0).
    MultipleOf(f64),
    /// Strictly greater than a literal or a sibling field's value.
    Gt(NumTarget),
    /// Greater than or equal to a literal or a sibling field's value.
    Gte(NumTarget),
    /// Strictly less than a literal or a sibling field's value.
    Lt(NumTarget),
    /// Less than or equal to a literal or a sibling field's value.
    Lte(NumTarget),
    // ------------------------------------------- Wave 4: conditionals ------
    /// Required when sibling `<f>` is present and equals one of `<v...>`;
    /// an absent sibling imposes no requirement (Laravel `required_if`).
    RequiredIf(String, Vec<String>),
    /// Required unless sibling `<f>` equals one of `<v...>`; an absent
    /// sibling resolves as `null`, which only matches a literal `null` token.
    RequiredUnless(String, Vec<String>),
    /// Required when sibling `<f>` is present AND non-blank.
    RequiredWith(String),
    /// Required when sibling `<f>` is absent OR blank.
    RequiredWithout(String),
    /// Present and in `{yes, on, 1, "1", true, "true"}` (implies required).
    Accepted,
    /// `accepted` only when sibling `<f>` equals one of `<v...>`.
    AcceptedIf(String, Vec<String>),
    /// Present and in `{no, off, 0, "0", false, "false"}` (implies required).
    Declined,
    /// `declined` only when sibling `<f>` equals one of `<v...>`.
    DeclinedIf(String, Vec<String>),
    /// When present, must be `null` / empty string / empty array.
    Prohibited,
    /// `prohibited` only when sibling `<f>` equals one of `<v...>`.
    ProhibitedIf(String, Vec<String>),
    /// `prohibited` when sibling `<f>` is present (key exists).
    ProhibitedWith(String),
    /// The field is ignored entirely (structural, never evaluated).
    Exclude,
    /// The field is ignored entirely when `<f>` equals one of `<v...>`.
    ExcludeIf(String, Vec<String>),
    /// The key must be absent (present `null` still fails).
    Missing,
    /// The key must be absent when sibling `<f>` equals one of `<v...>`.
    MissingIf(String, Vec<String>),
    /// The key must be absent when sibling `<f>` is present.
    MissingWith(String),
    /// An absent key skips every other rule (structural).
    Sometimes,
    /// Present ⇒ non-blank (implicit: runs on `null` even with `nullable`).
    Filled,
    /// The key must exist (a present `null` satisfies it).
    Present,
    // --------------------------------------------- Wave 4: cross-field -----
    /// Strict JSON equality with sibling `<f>`; a missing sibling fails.
    Same(String),
    /// Differs from sibling `<f>`; a missing sibling passes.
    Different(String),
    /// Strict JSON equality with the `<field>_confirmation` sibling.
    Confirmed,
    /// Value is an element of sibling `<f>`'s array (or equals a scalar
    /// sibling); a missing sibling fails closed.
    InArray(String),
    /// Array with unique elements (strict JSON equality).
    Distinct,
    /// Stop evaluating this field after the first failure (structural).
    Bail,
}

/// Target of the `gt`/`gte`/`lt`/`lte` family: a literal number or another
/// validated field (field refs resolve other validated values from the shared
/// input at check time).
#[derive(Debug, Clone, PartialEq)]
pub enum NumTarget {
    /// Numeric literal from the rule token (`gt:18`).
    Literal(f64),
    /// Sibling field path (`gt:min_age`).
    Field(String),
}

impl NumTarget {
    /// Parse the token argument: finite numbers are literals, otherwise the
    /// argument must look like a field path (letters, digits, `_`, `.`).
    /// Anything else is a boot-time parse error (fail-closed, tighter than
    /// Laravel which silently resolves unmatchable field names at runtime).
    pub fn parse(arg: &str, token: &str, rule: &str) -> Result<Self, String> {
        let t = arg.trim();
        if t.is_empty() {
            return Err(format!("{rule} requires <n|field>: {token}"));
        }
        if let Ok(n) = t.parse::<f64>() {
            if n.is_finite() {
                return Ok(NumTarget::Literal(n));
            }
        }
        // A leading digit plus a dot is a malformed number (`1.2.3`), never a
        // field path — reject at boot instead of resolving a dead sibling.
        let malformed_number = t.starts_with(|c: char| c.is_ascii_digit()) && t.contains('.');
        if !malformed_number
            && t.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
        {
            return Ok(NumTarget::Field(t.to_string()));
        }
        Err(format!(
            "{rule} value must be a number or a field name: {token}"
        ))
    }
}

impl Rule {
    pub fn name(&self) -> &'static str {
        match self {
            Rule::Required => "required",
            Rule::Min(_) => "min",
            Rule::Max(_) => "max",
            Rule::Email => "email",
            Rule::Url => "url",
            Rule::Uuid => "uuid",
            Rule::E164 => "e164",
            Rule::OneOf(_) => "one_of",
            Rule::Range(_, _) => "range",
            Rule::Date => "date",
            Rule::Nullable => "nullable",
            Rule::Boolean => "boolean",
            Rule::Integer => "integer",
            Rule::Numeric => "numeric",
            Rule::String => "string",
            Rule::Array => "array",
            Rule::Json => "json",
            Rule::Ascii => "ascii",
            Rule::Alpha => "alpha",
            Rule::AlphaNum => "alpha_num",
            Rule::AlphaDash => "alpha_dash",
            Rule::Lowercase => "lowercase",
            Rule::Uppercase => "uppercase",
            Rule::StartsWith(_) => "starts_with",
            Rule::EndsWith(_) => "ends_with",
            Rule::Contains(_) => "contains",
            Rule::DoesntStartWith(_) => "doesnt_start_with",
            Rule::DoesntEndWith(_) => "doesnt_end_with",
            Rule::DoesntContain(_) => "doesnt_contain",
            Rule::HexColor => "hex_color",
            Rule::MacAddress => "mac_address",
            Rule::Timezone => "timezone",
            Rule::Ulid => "ulid",
            Rule::Decimal(_) => "decimal",
            Rule::Digits(_) => "digits",
            Rule::DigitsBetween(_, _) => "digits_between",
            Rule::MultipleOf(_) => "multiple_of",
            Rule::Gt(_) => "gt",
            Rule::Gte(_) => "gte",
            Rule::Lt(_) => "lt",
            Rule::Lte(_) => "lte",
            Rule::RequiredIf(_, _) => "required_if",
            Rule::RequiredUnless(_, _) => "required_unless",
            Rule::RequiredWith(_) => "required_with",
            Rule::RequiredWithout(_) => "required_without",
            Rule::Accepted => "accepted",
            Rule::AcceptedIf(_, _) => "accepted_if",
            Rule::Declined => "declined",
            Rule::DeclinedIf(_, _) => "declined_if",
            Rule::Prohibited => "prohibited",
            Rule::ProhibitedIf(_, _) => "prohibited_if",
            Rule::ProhibitedWith(_) => "prohibited_with",
            Rule::Exclude => "exclude",
            Rule::ExcludeIf(_, _) => "exclude_if",
            Rule::Missing => "missing",
            Rule::MissingIf(_, _) => "missing_if",
            Rule::MissingWith(_) => "missing_with",
            Rule::Sometimes => "sometimes",
            Rule::Filled => "filled",
            Rule::Present => "present",
            Rule::Same(_) => "same",
            Rule::Different(_) => "different",
            Rule::Confirmed => "confirmed",
            Rule::InArray(_) => "in_array",
            Rule::Distinct => "distinct",
            Rule::Bail => "bail",
        }
    }
}

impl Rule {
    /// Non-empty argument required by a parameterised token.
    fn arg<'a>(arg: Option<&'a str>, token: &str, rule: &str) -> Result<&'a str, String> {
        match arg {
            Some(a) if !a.is_empty() => Ok(a),
            _ => Err(format!("{rule} requires a value: {token}")),
        }
    }

    /// A single field-path argument (`same:<f>`, `required_with:<f>`): the
    /// shared `[A-Za-z0-9_.]` charset keeps junk out of the boot graph
    /// (fail-closed, like `NumTarget::parse`).
    fn field_arg(arg: Option<&str>, token: &str, rule: &str) -> Result<String, String> {
        let a = Rule::arg(arg, token, rule)?;
        let ok = a
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.');
        if !ok {
            return Err(format!("{rule} value must be a field name: {token}"));
        }
        Ok(a.to_string())
    }

    /// A `<f,v[,v...]>` argument (the `*_if` family): first part is the
    /// sibling field, the rest are the values it may equal (Laravel's
    /// `parseDependentRuleParameters` shape).
    fn condition_arg(
        arg: Option<&str>,
        token: &str,
        rule: &str,
    ) -> Result<(String, Vec<String>), String> {
        let a = Rule::arg(arg, token, rule)?;
        let Some((field, rest)) = a.split_once(',') else {
            return Err(format!("{rule} must be {rule}:<field,value>: {token}"));
        };
        let field = field.trim();
        if field.is_empty()
            || !field
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
        {
            return Err(format!("{rule} field must be a field name: {token}"));
        }
        let values: Vec<String> = rest.split(',').map(|v| v.trim().to_string()).collect();
        if values.iter().any(|v| v.is_empty()) {
            return Err(format!("{rule} requires non-empty values: {token}"));
        }
        Ok((field.to_string(), values))
    }

    /// Parse one manifest rule token into a [`Rule`]. Grammar (1:1 with the
    /// enum): "required", "min:<n>", "max:<n>", "email", "url", "uuid",
    /// "e164", "one_of:<a,b,c>", "range:<lo>,<hi>", "date" (Wave 1) plus the
    /// Wave 4 battery — types: "nullable", "boolean", "integer", "numeric",
    /// "string", "array", "json", "ascii"; strings: "alpha", "alpha_num",
    /// "alpha_dash", "lowercase", "uppercase", "starts_with:<s>",
    /// "ends_with:<s>", "contains:<s>", "doesnt_start_with:<s>",
    /// "doesnt_end_with:<s>", "doesnt_contain:<s>", "hex_color",
    /// "mac_address", "timezone", "ulid"; numeric: "decimal:<n>",
    /// "digits:<n>", "digits_between:<a>,<b>", "multiple_of:<n>",
    /// "gt:<n|field>", "gte:<n|field>", "lt:<n|field>", "lte:<n|field>";
    /// conditionals: "required_if:<f,v...>", "required_unless:<f,v...>",
    /// "required_with:<f>", "required_without:<f>", "accepted",
    /// "accepted_if:<f,v...>", "declined", "declined_if:<f,v...>",
    /// "prohibited", "prohibited_if:<f,v...>", "prohibited_with:<f>",
    /// "exclude", "exclude_if:<f,v...>", "missing", "missing_if:<f,v...>",
    /// "missing_with:<f>", "sometimes", "filled", "present"; cross-field:
    /// "same:<f>", "different:<f>", "confirmed", "in_array:<f>", "distinct";
    /// control: "bail".
    /// Used by the registry GRAPH-VALIDATION-RULE gate and by the HTTP edge.
    pub fn from_token(token: &str) -> Result<Rule, String> {
        let token = token.trim();
        let (head, arg) = match token.split_once(':') {
            Some((h, a)) => (h.trim(), Some(a.trim())),
            None => (token, None),
        };
        Ok(match head {
            "required" => Rule::Required,
            "email" => Rule::Email,
            "url" => Rule::Url,
            "uuid" => Rule::Uuid,
            "e164" => Rule::E164,
            "date" => Rule::Date,
            "nullable" => Rule::Nullable,
            "boolean" => Rule::Boolean,
            "integer" => Rule::Integer,
            "numeric" => Rule::Numeric,
            "string" => Rule::String,
            "array" => Rule::Array,
            "json" => Rule::Json,
            "ascii" => Rule::Ascii,
            "alpha" => Rule::Alpha,
            "alpha_num" => Rule::AlphaNum,
            "alpha_dash" => Rule::AlphaDash,
            "lowercase" => Rule::Lowercase,
            "uppercase" => Rule::Uppercase,
            "hex_color" => Rule::HexColor,
            "mac_address" => Rule::MacAddress,
            "timezone" => Rule::Timezone,
            "ulid" => Rule::Ulid,
            "min" => {
                let n = Rule::arg(arg, token, "min")?;
                Rule::Min(
                    n.parse::<u64>()
                        .map_err(|_| format!("min value must be an unsigned integer: {token}"))?,
                )
            }
            "max" => {
                let n = Rule::arg(arg, token, "max")?;
                Rule::Max(
                    n.parse::<u64>()
                        .map_err(|_| format!("max value must be an unsigned integer: {token}"))?,
                )
            }
            "one_of" => {
                let a = Rule::arg(arg, token, "one_of")?;
                let values: Vec<String> = a
                    .split(',')
                    .map(|v| v.trim().to_string())
                    .filter(|v| !v.is_empty())
                    .collect();
                if values.is_empty() {
                    return Err(format!("one_of requires at least one value: {token}"));
                }
                Rule::OneOf(values)
            }
            "range" => {
                let a = Rule::arg(arg, token, "range")?;
                let mut parts = a.split(',');
                let lo = parts.next().and_then(|v| v.trim().parse::<i64>().ok());
                let hi = parts.next().and_then(|v| v.trim().parse::<i64>().ok());
                match (lo, hi) {
                    (Some(lo), Some(hi)) => Rule::Range(lo, hi),
                    _ => return Err(format!("range must be range:<lo>,<hi>: {token}")),
                }
            }
            "starts_with" => Rule::StartsWith(Rule::arg(arg, token, "starts_with")?.to_string()),
            "ends_with" => Rule::EndsWith(Rule::arg(arg, token, "ends_with")?.to_string()),
            "contains" => Rule::Contains(Rule::arg(arg, token, "contains")?.to_string()),
            "doesnt_start_with" => Rule::DoesntStartWith(
                Rule::arg(arg, token, "doesnt_start_with")?.to_string(),
            ),
            "doesnt_end_with" => {
                Rule::DoesntEndWith(Rule::arg(arg, token, "doesnt_end_with")?.to_string())
            }
            "doesnt_contain" => {
                Rule::DoesntContain(Rule::arg(arg, token, "doesnt_contain")?.to_string())
            }
            "decimal" => {
                let n = Rule::arg(arg, token, "decimal")?;
                Rule::Decimal(
                    n.parse::<u8>()
                        .map_err(|_| format!("decimal value must be an unsigned integer: {token}"))?,
                )
            }
            "digits" => {
                let n = Rule::arg(arg, token, "digits")?;
                Rule::Digits(
                    n.parse::<u64>()
                        .map_err(|_| format!("digits value must be an unsigned integer: {token}"))?,
                )
            }
            "digits_between" => {
                let a = Rule::arg(arg, token, "digits_between")?;
                let mut parts = a.split(',');
                let lo = parts.next().and_then(|v| v.trim().parse::<u64>().ok());
                let hi = parts.next().and_then(|v| v.trim().parse::<u64>().ok());
                match (lo, hi) {
                    (Some(lo), Some(hi)) => Rule::DigitsBetween(lo, hi),
                    _ => {
                        return Err(format!(
                            "digits_between must be digits_between:<a>,<b>: {token}"
                        ))
                    }
                }
            }
            "multiple_of" => {
                let a = Rule::arg(arg, token, "multiple_of")?;
                let n = a
                    .parse::<f64>()
                    .ok()
                    .filter(|n| n.is_finite() && *n > 0.0)
                    .ok_or_else(|| {
                        format!("multiple_of value must be a positive number: {token}")
                    })?;
                Rule::MultipleOf(n)
            }
            "gt" => Rule::Gt(NumTarget::parse(
                Rule::arg(arg, token, "gt")?,
                token,
                "gt",
            )?),
            "gte" => Rule::Gte(NumTarget::parse(
                Rule::arg(arg, token, "gte")?,
                token,
                "gte",
            )?),
            "lt" => Rule::Lt(NumTarget::parse(
                Rule::arg(arg, token, "lt")?,
                token,
                "lt",
            )?),
            "lte" => Rule::Lte(NumTarget::parse(
                Rule::arg(arg, token, "lte")?,
                token,
                "lte",
            )?),
            // ------------------------------------------- conditionals ----
            "accepted" => Rule::Accepted,
            "declined" => Rule::Declined,
            "prohibited" => Rule::Prohibited,
            "exclude" => Rule::Exclude,
            "missing" => Rule::Missing,
            "sometimes" => Rule::Sometimes,
            "filled" => Rule::Filled,
            "present" => Rule::Present,
            "required_if" => {
                let (f, vals) = Rule::condition_arg(arg, token, "required_if")?;
                Rule::RequiredIf(f, vals)
            }
            "required_unless" => {
                let (f, vals) = Rule::condition_arg(arg, token, "required_unless")?;
                Rule::RequiredUnless(f, vals)
            }
            "required_with" => Rule::RequiredWith(Rule::field_arg(arg, token, "required_with")?),
            "required_without" => {
                Rule::RequiredWithout(Rule::field_arg(arg, token, "required_without")?)
            }
            "accepted_if" => {
                let (f, vals) = Rule::condition_arg(arg, token, "accepted_if")?;
                Rule::AcceptedIf(f, vals)
            }
            "declined_if" => {
                let (f, vals) = Rule::condition_arg(arg, token, "declined_if")?;
                Rule::DeclinedIf(f, vals)
            }
            "prohibited_if" => {
                let (f, vals) = Rule::condition_arg(arg, token, "prohibited_if")?;
                Rule::ProhibitedIf(f, vals)
            }
            "prohibited_with" => {
                Rule::ProhibitedWith(Rule::field_arg(arg, token, "prohibited_with")?)
            }
            "exclude_if" => {
                let (f, vals) = Rule::condition_arg(arg, token, "exclude_if")?;
                Rule::ExcludeIf(f, vals)
            }
            "missing_if" => {
                let (f, vals) = Rule::condition_arg(arg, token, "missing_if")?;
                Rule::MissingIf(f, vals)
            }
            "missing_with" => Rule::MissingWith(Rule::field_arg(arg, token, "missing_with")?),
            // --------------------------------------------- cross-field ----
            "same" => Rule::Same(Rule::field_arg(arg, token, "same")?),
            "different" => Rule::Different(Rule::field_arg(arg, token, "different")?),
            "confirmed" => Rule::Confirmed,
            "in_array" => Rule::InArray(Rule::field_arg(arg, token, "in_array")?),
            "distinct" => Rule::Distinct,
            // ------------------------------------------- control ---------
            "bail" => Rule::Bail,
            other => return Err(format!("unknown validation rule: {other}")),
        })
    }
}

/// Parse manifest rule tokens (collect-all): returns every parse error so the
/// registry gate can report them together (GRAPH-VALIDATION-RULE).
pub fn parse_rules(tokens: &[String]) -> Result<Vec<Rule>, Vec<String>> {
    let mut rules = Vec::new();
    let mut errors = Vec::new();
    for token in tokens {
        match Rule::from_token(token) {
            Ok(rule) => rules.push(rule),
            Err(err) => errors.push(err),
        }
    }
    if errors.is_empty() {
        Ok(rules)
    } else {
        Err(errors)
    }
}

/// Upload part metadata visible to file rules (Wave 4 Phase 3). The edge's
/// `StoredUpload` lives in keel-http, which foundation crates cannot depend
/// on, so the descriptor shape is declared here and mapped at the edge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadMeta {
    /// Form field the file arrived under.
    pub field: String,
    /// Sanitized basename of the upload.
    pub filename: String,
    /// Declared content type of the part.
    pub content_type: String,
    /// Size in bytes.
    pub size: u64,
}

/// Everything a rule may need beyond the value under test. Built by the HTTP
/// edge and passed to [`Validator::validate_with`]; plain shape rules need
/// none of it, so [`Validator::validate`] constructs an empty context.
#[derive(Debug, Clone, Copy)]
pub struct ValidationContext<'a> {
    input: &'a Value,
    principal: Option<PrincipalId>,
    uploads: &'a [UploadMeta],
}

impl<'a> ValidationContext<'a> {
    /// Context over the input being validated, no principal, no uploads.
    pub fn new(input: &'a Value) -> Self {
        Self {
            input,
            principal: None,
            uploads: &[],
        }
    }

    /// Carry the authenticated principal (`current_password`, Phase 3).
    pub fn with_principal(mut self, principal: PrincipalId) -> Self {
        self.principal = Some(principal);
        self
    }

    /// Carry the upload part metadata (`file`/`image`/`mimes`, Phase 3).
    pub fn with_uploads(mut self, uploads: &'a [UploadMeta]) -> Self {
        self.uploads = uploads;
        self
    }

    /// The full input object (field refs resolve against it).
    pub fn input(&self) -> &'a Value {
        self.input
    }

    /// Authenticated principal, when the request carried one.
    pub fn principal(&self) -> Option<PrincipalId> {
        self.principal
    }

    /// Upload parts attached to this request.
    pub fn uploads(&self) -> &'a [UploadMeta] {
        self.uploads
    }

    /// Dot-path lookup into the shared input (`items.0.name` style segments).
    pub fn resolve(&self, path: &str) -> Option<&'a Value> {
        lookup(self.input, path)
    }
}

/// Rules bound to one field (dot paths allowed, e.g. address.city).
#[derive(Debug, Clone)]
pub struct FieldSpec {
    pub field: String,
    pub rules: Vec<Rule>,
}

/// Collect-all validator: binds rules to fields, validates a JSON object.
#[derive(Debug, Clone, Default)]
pub struct Validator {
    specs: Vec<FieldSpec>,
}

impl Validator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Builder: bind rules to a field. Later calls on the same field merge.
    pub fn field(mut self, name: impl Into<String>, rules: impl Into<Vec<Rule>>) -> Self {
        let name = name.into();
        let rules = rules.into();
        if let Some(spec) = self.specs.iter_mut().find(|s| s.field == name) {
            spec.rules.extend(rules);
        } else {
            self.specs.push(FieldSpec { field: name, rules });
        }
        self
    }

    pub fn field_names(&self) -> Vec<&str> {
        self.specs.iter().map(|s| s.field.as_str()).collect()
    }

    /// Validate input (must be a JSON object) with an empty context. For
    /// rules that need request state (principal, uploads, row probes) build a
    /// [`ValidationContext`] and call [`Validator::validate_with`].
    pub fn validate(&self, input: &Value) -> Result<(), ValidationReport> {
        self.validate_with(&ValidationContext::new(input))
    }

    /// Validate under a context: collects every violation across all fields.
    ///
    /// Evaluation breadth mirrors Laravel's `isValidatable`:
    /// - an absent key runs only IMPLICIT rules (`required`, `present`,
    ///   `accepted`, `declined`, `filled`, `missing*`, `required_*`);
    /// - a present `null` runs everything unless `nullable` is declared,
    ///   in which case it drops to implicit-only;
    /// - a blank string (whitespace only) also drops to implicit-only, so an
    ///   optional `""` skips format rules exactly as Laravel does;
    /// - any presence failure suppresses the field's value rules (the
    ///   Wave 1 one-message-per-field shape for `required`);
    /// - `bail` stops the field at its first failure;
    /// - `sometimes` and `exclude`/`exclude_if` skip the field entirely.
    pub fn validate_with(&self, ctx: &ValidationContext<'_>) -> Result<(), ValidationReport> {
        let input = ctx.input();
        let mut report = ValidationReport::default();
        if !input.is_object() {
            report.push("_", "input must be a JSON object");
            return Err(report);
        }
        for spec in &self.specs {
            let rules = &spec.rules;
            let value = lookup(input, &spec.field);
            // Structural: `sometimes` lets an absent key through untouched.
            if value.is_none() && rules.iter().any(|r| matches!(r, Rule::Sometimes)) {
                continue;
            }
            // Structural: `exclude`/`exclude_if` ignore the field entirely
            // (their other rules — including `required` — never run).
            if rules.iter().any(|r| is_excluded(r, ctx)) {
                continue;
            }
            let bail = rules.iter().any(|r| matches!(r, Rule::Bail));
            let implicit_only = match value {
                None => true,
                Some(Value::Null) => rules.iter().any(|r| matches!(r, Rule::Nullable)),
                Some(Value::String(s)) if s.trim().is_empty() => true,
                _ => false,
            };
            let mut presence_failed = false;
            for rule in rules {
                if is_presence(rule) {
                    if !is_implicit(rule) && implicit_only {
                        continue;
                    }
                    if let Some(msg) = check_presence(rule, &spec.field, value, ctx) {
                        report.push(&spec.field, msg);
                        presence_failed = true;
                        if bail {
                            break;
                        }
                    }
                    continue;
                }
                if implicit_only || presence_failed {
                    continue;
                }
                if let Some(v) = value {
                    if let Some(msg) = check(rule, &spec.field, v, ctx) {
                        report.push(&spec.field, msg);
                        if bail {
                            break;
                        }
                    }
                }
            }
        }
        if report.is_empty() {
            Ok(())
        } else {
            Err(report)
        }
    }
}

/// Presence/conditional rules: they reason about the key's state (absent /
/// null / blank / present) rather than only its value. Evaluated through
/// [`crate::checks::check_presence`] in every branch.
fn is_presence(rule: &Rule) -> bool {
    matches!(
        rule,
        Rule::Required
            | Rule::RequiredIf(_, _)
            | Rule::RequiredUnless(_, _)
            | Rule::RequiredWith(_)
            | Rule::RequiredWithout(_)
            | Rule::Accepted
            | Rule::AcceptedIf(_, _)
            | Rule::Declined
            | Rule::DeclinedIf(_, _)
            | Rule::Prohibited
            | Rule::ProhibitedIf(_, _)
            | Rule::ProhibitedWith(_)
            | Rule::Exclude
            | Rule::ExcludeIf(_, _)
            | Rule::Missing
            | Rule::MissingIf(_, _)
            | Rule::MissingWith(_)
            | Rule::Sometimes
            | Rule::Filled
            | Rule::Present
    )
}

/// Laravel's `implicitRules`: the ones that still evaluate when the key is
/// absent, blank, or `null` under `nullable`. Everything else (value rules,
/// `prohibited*`, `exclude*`, `sometimes`) is skipped in those states.
fn is_implicit(rule: &Rule) -> bool {
    matches!(
        rule,
        Rule::Required
            | Rule::RequiredIf(_, _)
            | Rule::RequiredUnless(_, _)
            | Rule::RequiredWith(_)
            | Rule::RequiredWithout(_)
            | Rule::Accepted
            | Rule::AcceptedIf(_, _)
            | Rule::Declined
            | Rule::DeclinedIf(_, _)
            | Rule::Missing
            | Rule::MissingIf(_, _)
            | Rule::MissingWith(_)
            | Rule::Filled
            | Rule::Present
    )
}

/// One failed rule on one field (public-safe wording only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldViolation {
    pub field: String,
    pub message: String,
}

/// Collect-all report: empty == valid.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ValidationReport {
    pub violations: Vec<FieldViolation>,
}

impl ValidationReport {
    pub fn push(&mut self, field: impl Into<String>, message: impl Into<String>) {
        self.violations.push(FieldViolation {
            field: field.into(),
            message: message.into(),
        });
    }

    pub fn is_empty(&self) -> bool {
        self.violations.is_empty()
    }

    pub fn len(&self) -> usize {
        self.violations.len()
    }

    pub fn fields(&self) -> Vec<&str> {
        let mut out: Vec<&str> = self.violations.iter().map(|v| v.field.as_str()).collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Uniform error map: one validation AppError carrying a
    /// fields meta object ({field}: [messages...]).
    pub fn into_app_error(self) -> AppError {
        let mut fields = serde_json::Map::new();
        for v in &self.violations {
            let entry = fields
                .entry(v.field.clone())
                .or_insert_with(|| Value::Array(Vec::new()));
            if let Value::Array(messages) = entry {
                messages.push(Value::String(v.message.clone()));
            }
        }
        AppError::validation(format!(
            "validation failed: {} field(s) invalid",
            self.violations.len()
        ))
        .with_meta("fields", fields)
    }
}

/// Dot-path lookup on nested JSON objects.
fn lookup<'a>(input: &'a Value, field: &str) -> Option<&'a Value> {
    let mut cur = input;
    for seg in field.split('.') {
        cur = cur.get(seg)?;
    }
    Some(cur)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checks::{is_date, is_e164, is_email, is_url};
    use serde_json::json;

    #[test]
    fn collect_all_reports_every_violation() {
        let v = Validator::new()
            .field("name", vec![Rule::Required, Rule::Max(3)])
            .field("email", vec![Rule::Required, Rule::Email])
            .field("age", vec![Rule::Range(0, 130)]);
        let report = v
            .validate(&json!({ "name": "toolong", "age": 200 }))
            .unwrap_err();
        assert_eq!(report.len(), 3);
        assert_eq!(report.fields(), vec!["age", "email", "name"]);
    }

    #[test]
    fn report_maps_to_app_error_with_fields_meta() {
        let mut report = ValidationReport::default();
        report.push("email", "must be a valid email");
        report.push("email", "second message");
        let err = report.into_app_error();
        assert_eq!(err.category, keel_foundation::ErrorCategory::Validation);
        let fields = err.meta.get("fields").expect("fields meta");
        assert_eq!(fields["email"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn required_semantics() {
        let v = Validator::new().field("name", vec![Rule::Required]);
        assert!(v.validate(&json!({ "name": "ok" })).is_ok());
        assert!(v.validate(&json!({ "name": "   " })).is_err());
        assert!(v.validate(&json!({ "name": null })).is_err());
        assert!(v.validate(&json!({})).is_err());
    }

    #[test]
    fn optional_absent_fields_skip_rules() {
        let v = Validator::new().field("nick", vec![Rule::Email]);
        assert!(v.validate(&json!({})).is_ok());
        assert!(v.validate(&json!({ "nick": "not-an-email" })).is_err());
    }

    #[test]
    fn present_null_needs_nullable_to_skip_other_rules() {
        // Laravel presence model: null only skips when `nullable` is declared.
        let strict = Validator::new().field("nick", vec![Rule::Email]);
        let report = strict
            .validate(&json!({ "nick": null }))
            .expect_err("null fails a non-nullable field");
        assert_eq!(report.len(), 1);

        let soft = Validator::new().field("nick", vec![Rule::Nullable, Rule::Email]);
        assert!(soft.validate(&json!({ "nick": null })).is_ok());

        // `required` still fails on null even with `nullable`.
        let both = Validator::new().field("nick", vec![Rule::Required, Rule::Nullable]);
        assert!(both.validate(&json!({ "nick": null })).is_err());

        // Absent stays absent: only `required` reacts.
        assert!(soft.validate(&json!({})).is_ok());
        assert!(both.validate(&json!({})).is_err());

        // One violation, not two, when required rejects null.
        assert_eq!(
            Validator::new()
                .field("nick", vec![Rule::Required, Rule::Email])
                .validate(&json!({ "nick": null }))
                .unwrap_err()
                .len(),
            1
        );
    }

    #[test]
    fn size_rules_measure_by_kind() {
        let v = Validator::new()
            .field("s", vec![Rule::Min(2), Rule::Max(4)])
            .field("n", vec![Rule::Min(10)])
            .field("a", vec![Rule::Max(2)]);
        assert!(v
            .validate(&json!({ "s": "abc", "n": 11, "a": [1, 2] }))
            .is_ok());
        assert!(
            v.validate(&json!({ "s": "a", "n": 9, "a": [1, 2, 3] }))
                .unwrap_err()
                .len()
                == 3
        );
    }

    #[test]
    fn format_rules() {
        assert!(is_email("alice@example.co.in"));
        assert!(!is_email("alice@@example.com"));
        assert!(!is_email("alice@examplecom"));
        assert!(is_url("https://keel.dev/docs"));
        assert!(!is_url("ftp://keel.dev"));
        assert!(is_e164("+919876543210"));
        assert!(!is_e164("00919876543210"));
        assert!(is_date("2026-08-20"));
        assert!(is_date("2026-08-20T10:00:00+05:30"));
        assert!(is_date("2026-08-20T10:00Z"), "minute precision Z");
        assert!(is_date("2026-08-20T10:00+05:30"), "minute precision offset");
        assert!(!is_date("2026-08-20T10:00"), "no timezone is ambiguous");
        assert!(!is_date("20/08/2026"));
        assert!(uuid::Uuid::parse_str("018f1c2a-0000-7000-8000-000000000001").is_ok());
    }

    #[test]
    fn one_of_and_range() {
        let v = Validator::new()
            .field(
                "kind",
                vec![Rule::OneOf(vec!["self".into(), "family".into()])],
            )
            .field("qty", vec![Rule::Range(1, 5)]);
        assert!(v.validate(&json!({ "kind": "self", "qty": 3 })).is_ok());
        let report = v
            .validate(&json!({ "kind": "other", "qty": 9 }))
            .unwrap_err();
        assert_eq!(report.len(), 2);
    }

    #[test]
    fn dot_paths_validate_nested_fields() {
        let v = Validator::new().field("address.city", vec![Rule::Required, Rule::Max(20)]);
        assert!(v
            .validate(&json!({ "address": { "city": "Pune" } }))
            .is_ok());
        assert!(v.validate(&json!({ "address": {} })).is_err());
    }

    #[test]
    fn non_object_input_is_rejected() {
        let v = Validator::new().field("x", vec![Rule::Required]);
        assert!(v.validate(&json!([1, 2])).is_err());
    }

    #[test]
    fn field_ref_rules_resolve_through_the_context() {
        let v = Validator::new()
            .field("start", vec![Rule::Required, Rule::Numeric])
            .field("end", vec![Rule::Required, Rule::Gt(NumTarget::Field("start".into()))]);
        assert!(v
            .validate(&json!({ "start": "09:00", "end": "10:00" }))
            .is_err(), "non-numeric sibling fails closed");
        assert!(v
            .validate(&json!({ "start": 9, "end": 10 }))
            .is_ok());
        let report = v
            .validate(&json!({ "start": 10, "end": 9 }))
            .unwrap_err();
        assert!(report.violations[0].message.contains("start"));
    }

    #[test]
    fn context_exposes_request_state() {
        let input = json!({ "age": 21 });
        let ctx = ValidationContext::new(&input);
        assert_eq!(ctx.resolve("age"), Some(&json!(21)));
        assert_eq!(ctx.resolve("absent"), None);
        assert!(ctx.principal().is_none());
        assert!(ctx.uploads().is_empty());

        let principal = keel_foundation::PrincipalId::new();
        let ctx = ctx.with_principal(principal);
        assert_eq!(ctx.principal(), Some(principal));
    }

    #[test]
    fn rule_tokens_parse() {
        assert_eq!(Rule::from_token("required").unwrap(), Rule::Required);
        assert_eq!(Rule::from_token("min:3").unwrap(), Rule::Min(3));
        assert_eq!(Rule::from_token("max:500").unwrap(), Rule::Max(500));
        assert_eq!(Rule::from_token("email").unwrap(), Rule::Email);
        assert_eq!(Rule::from_token("url").unwrap(), Rule::Url);
        assert_eq!(Rule::from_token("uuid").unwrap(), Rule::Uuid);
        assert_eq!(Rule::from_token("e164").unwrap(), Rule::E164);
        assert_eq!(
            Rule::from_token("one_of:self,family").unwrap(),
            Rule::OneOf(vec!["self".into(), "family".into()])
        );
        assert_eq!(Rule::from_token("range:1,5").unwrap(), Rule::Range(1, 5));
        assert_eq!(Rule::from_token("date").unwrap(), Rule::Date);
        assert_eq!(Rule::from_token(" date ").unwrap(), Rule::Date);
    }

    #[test]
    fn wave4_tokens_parse() {
        // types
        for (token, rule) in [
            ("nullable", Rule::Nullable),
            ("boolean", Rule::Boolean),
            ("integer", Rule::Integer),
            ("numeric", Rule::Numeric),
            ("string", Rule::String),
            ("array", Rule::Array),
            ("json", Rule::Json),
            ("ascii", Rule::Ascii),
        ] {
            assert_eq!(Rule::from_token(token).unwrap(), rule, "{token}");
        }
        // strings
        for (token, rule) in [
            ("alpha", Rule::Alpha),
            ("alpha_num", Rule::AlphaNum),
            ("alpha_dash", Rule::AlphaDash),
            ("lowercase", Rule::Lowercase),
            ("uppercase", Rule::Uppercase),
            ("hex_color", Rule::HexColor),
            ("mac_address", Rule::MacAddress),
            ("timezone", Rule::Timezone),
            ("ulid", Rule::Ulid),
        ] {
            assert_eq!(Rule::from_token(token).unwrap(), rule, "{token}");
        }
        assert_eq!(
            Rule::from_token("starts_with:ab").unwrap(),
            Rule::StartsWith("ab".into())
        );
        assert_eq!(
            Rule::from_token("ends_with:ab").unwrap(),
            Rule::EndsWith("ab".into())
        );
        assert_eq!(
            Rule::from_token("contains:ab").unwrap(),
            Rule::Contains("ab".into())
        );
        assert_eq!(
            Rule::from_token("doesnt_start_with:ab").unwrap(),
            Rule::DoesntStartWith("ab".into())
        );
        assert_eq!(
            Rule::from_token("doesnt_end_with:ab").unwrap(),
            Rule::DoesntEndWith("ab".into())
        );
        assert_eq!(
            Rule::from_token("doesnt_contain:ab").unwrap(),
            Rule::DoesntContain("ab".into())
        );
        // parameterised strings keep colons in their argument
        assert_eq!(
            Rule::from_token("starts_with:https://").unwrap(),
            Rule::StartsWith("https://".into())
        );
        // numeric
        assert_eq!(Rule::from_token("decimal:2").unwrap(), Rule::Decimal(2));
        assert_eq!(Rule::from_token("digits:4").unwrap(), Rule::Digits(4));
        assert_eq!(
            Rule::from_token("digits_between:4,6").unwrap(),
            Rule::DigitsBetween(4, 6)
        );
        assert_eq!(
            Rule::from_token("multiple_of:0.5").unwrap(),
            Rule::MultipleOf(0.5)
        );
        assert_eq!(
            Rule::from_token("gt:18").unwrap(),
            Rule::Gt(NumTarget::Literal(18.0))
        );
        assert_eq!(
            Rule::from_token("gte:min_age").unwrap(),
            Rule::Gte(NumTarget::Field("min_age".into()))
        );
        assert_eq!(
            Rule::from_token("lt:max_age").unwrap(),
            Rule::Lt(NumTarget::Field("max_age".into()))
        );
        assert_eq!(
            Rule::from_token("lte:64.5").unwrap(),
            Rule::Lte(NumTarget::Literal(64.5))
        );
    }

    #[test]
    fn wave4_phase2_tokens_parse() {
        for (token, rule) in [
            ("accepted", Rule::Accepted),
            ("declined", Rule::Declined),
            ("prohibited", Rule::Prohibited),
            ("exclude", Rule::Exclude),
            ("missing", Rule::Missing),
            ("sometimes", Rule::Sometimes),
            ("filled", Rule::Filled),
            ("present", Rule::Present),
            ("confirmed", Rule::Confirmed),
            ("distinct", Rule::Distinct),
            ("bail", Rule::Bail),
            ("required_with:phone", Rule::RequiredWith("phone".into())),
            (
                "required_without:phone",
                Rule::RequiredWithout("phone".into()),
            ),
            (
                "prohibited_with:backup",
                Rule::ProhibitedWith("backup".into()),
            ),
            ("missing_with:legacy", Rule::MissingWith("legacy".into())),
            ("same:password", Rule::Same("password".into())),
            ("different:email", Rule::Different("email".into())),
            ("in_array:tags", Rule::InArray("tags".into())),
            (
                "required_if:type,guest",
                Rule::RequiredIf("type".into(), vec!["guest".into()]),
            ),
            (
                "required_unless:type,guest,hotel",
                Rule::RequiredUnless("type".into(), vec!["guest".into(), "hotel".into()]),
            ),
            (
                "accepted_if:flow,signup",
                Rule::AcceptedIf("flow".into(), vec!["signup".into()]),
            ),
            (
                "declined_if:flow,edit",
                Rule::DeclinedIf("flow".into(), vec!["edit".into()]),
            ),
            (
                "prohibited_if:role,admin",
                Rule::ProhibitedIf("role".into(), vec!["admin".into()]),
            ),
            (
                "exclude_if:country,US",
                Rule::ExcludeIf("country".into(), vec!["US".into()]),
            ),
            (
                "missing_if:legacy,1",
                Rule::MissingIf("legacy".into(), vec!["1".into()]),
            ),
        ] {
            assert_eq!(Rule::from_token(token).unwrap(), rule, "{token}");
        }
        for bad in [
            "required_if",
            "required_if:",
            "required_if:type",
            "required_if:,guest",
            "required_if:type,",
            "required_unless",
            "required_with",
            "required_with:",
            "required_with:a-b",
            "required_without:",
            "accepted_if:x",
            "prohibited_if:role",
            "exclude_if:type",
            "missing_if:",
            "missing_with",
            "missing_with:",
            "same",
            "same:",
            "same:a b",
            "different:",
            "in_array",
            "in_array:",
        ] {
            assert!(Rule::from_token(bad).is_err(), "{bad} should fail");
        }
    }

    #[test]
    fn conditional_rules_follow_sibling_state() {
        // required_if: an absent sibling imposes NO requirement.
        let v = Validator::new()
            .field("type", vec![Rule::Required])
            .field(
                "guest_name",
                vec![Rule::RequiredIf("type".into(), vec!["guest".into()])],
            );
        assert!(v.validate(&json!({ "type": "hotel" })).is_ok());
        let report = v
            .validate(&json!({ "type": "guest" }))
            .expect_err("condition met requires the field");
        assert_eq!(
            report.violations[0].message,
            "guest_name is required when type is guest"
        );

        // required_unless inverts it; an absent sibling resolves as null.
        let u = Validator::new().field(
            "email",
            vec![Rule::RequiredUnless("kind".into(), vec!["anon".into()])],
        );
        assert!(u.validate(&json!({ "kind": "anon" })).is_ok());
        assert!(u.validate(&json!({ "kind": "member" })).is_err());
        assert!(u
            .validate(&json!({}))
            .is_err(), "absent sibling never matches a string token");

        // required_with: triggered only by a present AND non-blank sibling.
        let w = Validator::new().field("email", vec![Rule::RequiredWith("phone".into())]);
        assert!(w.validate(&json!({})).is_ok());
        assert!(w.validate(&json!({ "phone": "   " })).is_ok());
        assert!(w
            .validate(&json!({ "phone": "+919876543210" }))
            .is_err());

        // required_without: triggered when the sibling is absent OR blank.
        let wo = Validator::new().field("email", vec![Rule::RequiredWithout("phone".into())]);
        assert!(wo.validate(&json!({})).is_err());
        assert!(wo.validate(&json!({ "phone": "" })).is_err());
        assert!(wo.validate(&json!({ "phone": "x" })).is_ok());

        // numeric siblings compare numerically against the token.
        let n = Validator::new().field(
            "code",
            vec![Rule::RequiredIf("age".into(), vec!["21".into()])],
        );
        assert!(n.validate(&json!({ "age": 21 })).is_err());
        assert!(n.validate(&json!({ "age": "21" })).is_err());
        assert!(n.validate(&json!({ "age": 22 })).is_ok());
    }

    #[test]
    fn accepted_declined_and_their_conditionals() {
        let a = Validator::new().field("tos", vec![Rule::Accepted]);
        assert!(a.validate(&json!({})).is_err(), "accepted implies required");
        assert!(a.validate(&json!({ "tos": "yes" })).is_ok());
        assert!(a.validate(&json!({ "tos": true })).is_ok());
        assert!(a.validate(&json!({ "tos": 1 })).is_ok());
        assert!(a.validate(&json!({ "tos": "1" })).is_ok());
        assert!(a.validate(&json!({ "tos": 1.0 })).is_err(), "strict: float 1.0 is not 1");
        let report = a
            .validate(&json!({ "tos": "no" }))
            .expect_err("no is not accepted");
        assert_eq!(report.violations[0].message, "tos must be accepted");

        let d = Validator::new().field("opt_out", vec![Rule::Declined]);
        assert!(d.validate(&json!({ "opt_out": "off" })).is_ok());
        assert!(d.validate(&json!({ "opt_out": 0 })).is_ok());
        assert!(d.validate(&json!({ "opt_out": false })).is_ok());
        assert!(d.validate(&json!({})).is_err(), "declined implies required");
        assert!(d.validate(&json!({ "opt_out": "yes" })).is_err());

        let ai = Validator::new().field(
            "tos",
            vec![Rule::AcceptedIf("flow".into(), vec!["signup".into()])],
        );
        assert!(ai.validate(&json!({ "flow": "edit" })).is_ok());
        assert!(ai.validate(&json!({ "tos": "yes" })).is_ok());
        assert!(ai
            .validate(&json!({ "flow": "signup" }))
            .is_err(), "condition on ⇒ must be accepted");
        assert!(ai
            .validate(&json!({ "flow": "signup", "tos": "yes" }))
            .is_ok());
    }

    #[test]
    fn prohibited_present_filled_missing_families() {
        // prohibited: null / "" / [] pass; any value fails; absent passes.
        let p = Validator::new().field("admin", vec![Rule::Prohibited]);
        assert!(p.validate(&json!({})).is_ok());
        assert!(p.validate(&json!({ "admin": null })).is_ok());
        assert!(p.validate(&json!({ "admin": "" })).is_ok());
        assert!(p.validate(&json!({ "admin": [] })).is_ok());
        let report = p
            .validate(&json!({ "admin": "root" }))
            .expect_err("prohibited rejects values");
        assert_eq!(
            report.violations[0].message,
            "admin must be omitted or empty"
        );

        let pi = Validator::new().field(
            "note",
            vec![Rule::ProhibitedIf("role".into(), vec!["admin".into()])],
        );
        assert!(pi.validate(&json!({ "role": "user", "note": "hi" })).is_ok());
        assert!(pi.validate(&json!({ "role": "admin", "note": "hi" })).is_err());
        assert!(pi.validate(&json!({ "role": "admin" })).is_ok());

        let pw = Validator::new().field("b", vec![Rule::ProhibitedWith("a".into())]);
        assert!(pw.validate(&json!({ "b": "x" })).is_ok());
        assert!(pw.validate(&json!({ "a": "", "b": "x" })).is_err());

        // present: the key must exist; null satisfies it.
        let pr = Validator::new().field("token", vec![Rule::Present]);
        assert!(pr.validate(&json!({})).is_err());
        assert!(pr.validate(&json!({ "token": null })).is_ok());
        assert!(pr.validate(&json!({ "token": "" })).is_ok());

        // filled: implicit — absent passes, present must be non-blank, and
        // null FAILS even under `nullable` (Laravel's implicit-rule quirk).
        let f = Validator::new().field("nick", vec![Rule::Filled]);
        assert!(f.validate(&json!({})).is_ok());
        assert!(f.validate(&json!({ "nick": "" })).is_err());
        assert!(f.validate(&json!({ "nick": null })).is_err());
        assert!(f.validate(&json!({ "nick": "x" })).is_ok());
        let soft = Validator::new().field("nick", vec![Rule::Nullable, Rule::Filled]);
        assert!(soft.validate(&json!({ "nick": null })).is_err());
        assert!(soft.validate(&json!({})).is_ok());

        // missing: the key must be absent (a present null still fails).
        let m = Validator::new().field("legacy", vec![Rule::Missing]);
        assert!(m.validate(&json!({})).is_ok());
        assert!(m.validate(&json!({ "legacy": null })).is_err());
        assert!(m.validate(&json!({ "legacy": "x" })).is_err());

        let mi = Validator::new().field(
            "legacy",
            vec![Rule::MissingIf("version".into(), vec!["1".into()])],
        );
        assert!(mi.validate(&json!({ "version": "1" })).is_ok());
        assert!(mi.validate(&json!({ "version": "1", "legacy": "x" })).is_err());
        assert!(mi.validate(&json!({ "version": "2", "legacy": "x" })).is_ok());

        let mw = Validator::new().field("old", vec![Rule::MissingWith("new".into())]);
        assert!(mw.validate(&json!({ "old": "x" })).is_ok());
        assert!(mw.validate(&json!({ "new": "y", "old": "x" })).is_err());
    }

    #[test]
    fn exclude_and_sometimes_skip_validation() {
        // `exclude` ignores the field entirely — even `required`.
        let e = Validator::new().field(
            "noise",
            vec![Rule::Exclude, Rule::Required, Rule::Email],
        );
        assert!(e.validate(&json!({})).is_ok());
        assert!(e.validate(&json!({ "noise": "whatever" })).is_ok());

        // exclude_if: only when the sibling exists AND matches.
        let ei = Validator::new().field(
            "region",
            vec![
                Rule::ExcludeIf("country".into(), vec!["US".into()]),
                Rule::Required,
            ],
        );
        assert!(ei.validate(&json!({ "country": "US" })).is_ok());
        assert!(ei.validate(&json!({ "country": "IN" })).is_err());
        assert!(
            ei.validate(&json!({})).is_err(),
            "absent sibling never triggers exclusion"
        );

        // sometimes: an absent key skips every rule (incl. required).
        let s = Validator::new().field(
            "nick",
            vec![Rule::Sometimes, Rule::Required, Rule::Email],
        );
        assert!(s.validate(&json!({})).is_ok());
        assert!(s.validate(&json!({ "nick": "bad" })).is_err());
        assert!(s.validate(&json!({ "nick": "" })).is_err());
    }

    #[test]
    fn bail_stops_the_field_at_the_first_failure() {
        let b = Validator::new().field("code", vec![Rule::Bail, Rule::Min(5), Rule::Email]);
        let report = b
            .validate(&json!({ "code": "abc" }))
            .expect_err("bail still reports the first failure");
        assert_eq!(report.len(), 1);

        // without bail every failing rule reports
        let all = Validator::new().field("code", vec![Rule::Min(5), Rule::Email]);
        assert_eq!(all.validate(&json!({ "code": "abc" })).unwrap_err().len(), 2);

        // bail also stops after the first PRESENCE failure
        let both = Validator::new().field("x", vec![Rule::Required, Rule::Present]);
        assert_eq!(both.validate(&json!({})).unwrap_err().len(), 2);
        let bailed = Validator::new().field("x", vec![Rule::Bail, Rule::Required, Rule::Present]);
        assert_eq!(bailed.validate(&json!({})).unwrap_err().len(), 1);
    }

    #[test]
    fn cross_field_rules_compare_siblings() {
        // same / different
        let s = Validator::new().field("b", vec![Rule::Same("a".into())]);
        assert!(s.validate(&json!({ "a": "x", "b": "x" })).is_ok());
        assert!(s.validate(&json!({ "a": "x", "b": "y" })).is_err());
        assert!(s.validate(&json!({ "b": "x" })).is_err());
        let d = Validator::new().field("b", vec![Rule::Different("a".into())]);
        assert!(d.validate(&json!({ "a": "x", "b": "y" })).is_ok());
        assert!(d.validate(&json!({ "a": "x", "b": "x" })).is_err());
        assert!(d.validate(&json!({ "b": "y" })).is_ok());

        // confirmed: matches <field>_confirmation (Laravel's isset semantics)
        let c = Validator::new().field("password", vec![Rule::Required, Rule::Confirmed]);
        assert!(c
            .validate(&json!({ "password": "s3cret", "password_confirmation": "s3cret" }))
            .is_ok());
        let report = c
            .validate(&json!({ "password": "s3cret", "password_confirmation": "other" }))
            .expect_err("mismatched confirmation");
        assert!(report
            .violations
            .iter()
            .any(|v| v.message == "confirmation does not match"));
        assert!(c.validate(&json!({ "password": "s3cret" })).is_err());

        // in_array: element of the sibling array; missing sibling fails closed
        let ia = Validator::new().field("tag", vec![Rule::InArray("tags".into())]);
        assert!(ia.validate(&json!({ "tags": ["a", "b"], "tag": "a" })).is_ok());
        assert!(ia.validate(&json!({ "tags": ["a", "b"], "tag": "c" })).is_err());
        assert!(ia.validate(&json!({ "tag": "a" })).is_err());
        assert!(ia
            .validate(&json!({ "tags": "a", "tag": "a" }))
            .is_ok(), "scalar sibling compares equal");

        // distinct: unique array elements
        let dist = Validator::new().field("ids", vec![Rule::Distinct]);
        assert!(dist.validate(&json!({ "ids": [1, 2, 3] })).is_ok());
        let report = dist
            .validate(&json!({ "ids": [1, 2, 1] }))
            .expect_err("duplicate element");
        assert_eq!(report.violations[0].message, "must contain unique values");
        assert!(dist.validate(&json!({ "ids": "x" })).is_err());
    }

    #[test]
    fn blank_strings_skip_non_implicit_rules() {
        // Laravel's presentOrRuleIsImplicit quirk: whitespace-only strings
        // only run implicit rules, so an optional "" passes format checks.
        let v = Validator::new().field("email", vec![Rule::Email]);
        assert!(v.validate(&json!({ "email": "" })).is_ok());
        assert!(v.validate(&json!({ "email": "   " })).is_ok());
        assert!(v.validate(&json!({ "email": "x" })).is_err());

        // ... but implicit rules (`required`) still reject them, alone.
        let r = Validator::new().field("email", vec![Rule::Required, Rule::Email]);
        let report = r
            .validate(&json!({ "email": "  " }))
            .expect_err("required rejects blank strings");
        assert_eq!(report.len(), 1);
        assert_eq!(report.violations[0].message, "email is required");
    }

    #[test]
    fn rule_tokens_reject_malformed() {
        for bad in [
            "min", "min:x", "max:-1", "one_of", "one_of:", "one_of: ,", "range:1", "range:a,b",
            "nope", "min:3:4", "",
            // Wave 4: missing arguments
            "starts_with",
            "starts_with:",
            "ends_with",
            "contains:",
            "doesnt_start_with",
            "doesnt_end_with",
            "doesnt_contain:",
            "decimal",
            "decimal:x",
            "digits",
            "digits:4.5",
            "digits_between:1",
            "digits_between:a,b",
            "multiple_of",
            "multiple_of:0",
            "multiple_of:-1",
            "gt",
            "gt:",
            "gte:1.2.3",
            "lt:x y",
            "lte:",
        ] {
            assert!(Rule::from_token(bad).is_err(), "{bad} should fail");
        }
    }

    #[test]
    fn num_target_parse_is_fail_closed() {
        assert_eq!(
            NumTarget::parse("18", "gt:18", "gt").unwrap(),
            NumTarget::Literal(18.0)
        );
        assert_eq!(
            NumTarget::parse("min_age", "gt:min_age", "gt").unwrap(),
            NumTarget::Field("min_age".into())
        );
        assert_eq!(
            NumTarget::parse("items.0.qty", "gt:items.0.qty", "gt").unwrap(),
            NumTarget::Field("items.0.qty".into())
        );
        // field-path charset rejects punctuation garbage at boot
        assert!(NumTarget::parse("a-b", "gt:a-b", "gt").is_err());
        assert!(NumTarget::parse("", "gt:", "gt").is_err());
        // non-finite literals are not numbers
        assert_eq!(
            NumTarget::parse("nan", "gt:nan", "gt").unwrap(),
            NumTarget::Field("nan".into())
        );
    }

    #[test]
    fn names_match_the_grammar_tokens() {
        assert_eq!(Rule::String.name(), "string");
        assert_eq!(Rule::DigitsBetween(1, 2).name(), "digits_between");
        assert_eq!(Rule::Gt(NumTarget::Literal(1.0)).name(), "gt");
        assert_eq!(Rule::Nullable.name(), "nullable");
        assert_eq!(Rule::StartsWith("x".into()).name(), "starts_with");
        assert_eq!(
            Rule::RequiredIf("a".into(), vec!["b".into()]).name(),
            "required_if"
        );
        assert_eq!(Rule::RequiredWith("a".into()).name(), "required_with");
        assert_eq!(Rule::MissingIf("a".into(), vec!["b".into()]).name(), "missing_if");
        assert_eq!(Rule::Confirmed.name(), "confirmed");
        assert_eq!(Rule::InArray("a".into()).name(), "in_array");
        assert_eq!(Rule::Bail.name(), "bail");
    }

    #[test]
    fn parse_rules_collects_all_errors() {
        let tokens: Vec<String> = vec!["required".into(), "bogus".into(), "min:x".into()];
        let errs = parse_rules(&tokens).unwrap_err();
        assert_eq!(errs.len(), 2);
        let ok = parse_rules(&["required".into(), "email".into()]).unwrap();
        assert_eq!(ok.len(), 2);
        // the whole Wave 4 battery parses as one manifest entry
        let battery: Vec<String> = [
            "nullable",
            "boolean",
            "integer",
            "numeric",
            "string",
            "array",
            "json",
            "ascii",
            "alpha",
            "alpha_num",
            "alpha_dash",
            "lowercase",
            "uppercase",
            "starts_with:a",
            "ends_with:z",
            "contains:m",
            "doesnt_start_with:x",
            "doesnt_end_with:y",
            "doesnt_contain:q",
            "hex_color",
            "mac_address",
            "timezone",
            "ulid",
            "decimal:2",
            "digits:4",
            "digits_between:4,6",
            "multiple_of:3",
            "gt:0",
            "gte:floor",
            "lt:100",
            "lte:ceiling",
            // Phase 2: conditionals, cross-field, bail
            "required_if:type,guest",
            "required_unless:type,anon",
            "required_with:phone",
            "required_without:phone",
            "accepted",
            "accepted_if:flow,signup",
            "declined",
            "declined_if:flow,edit",
            "prohibited",
            "prohibited_if:role,admin",
            "prohibited_with:backup",
            "exclude",
            "exclude_if:country,US",
            "missing",
            "missing_if:legacy,1",
            "missing_with:new",
            "sometimes",
            "filled",
            "present",
            "same:password",
            "different:email",
            "confirmed",
            "in_array:tags",
            "distinct",
            "bail",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(parse_rules(&battery).unwrap().len(), battery.len());
    }
}
