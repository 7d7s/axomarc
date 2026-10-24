//! B-31 Phase D: the fluent, byte-stable MySQL schema builder.
//!
//! Laravel's migration DX is `Schema::create(...)` over a `Blueprint`; kelson's
//! migrations are SQL-first, so this builder's job is different and narrower:
//! it is an *authoring* surface that renders the exact DDL bytes the shipped
//! `database/migrations-mysql/*.sql` files contain. That claim is enforced,
//! not asserted — `tests/schema_roundtrip.rs` regenerates every shipped
//! migration with a builder program and byte-compares it to the file, so
//! "the builder can express every construct the fleet ships" is a tested
//! invariant of this crate rather than a paragraph in a document.
//!
//! Canonical layout (the round-trip gate depends on every rule here):
//!
//! - a column line is `    {name:<max+2}{type}{modifiers}` — the name field is
//!   padded to the longest column name in its group plus two, in `CREATE
//!   TABLE` bodies and `ALTER TABLE ... ADD COLUMN` groups alike;
//! - column modifiers render in a fixed order: `NOT NULL`, `DEFAULT <lit>`,
//!   `PRIMARY KEY`, `UNIQUE`, `CHECK (<expr>)`;
//! - an inline column comment is exactly two spaces after the line's comma;
//! - the trailing comma is dropped on the last line of a table (column or
//!   constraint); the last `ADD COLUMN` clause of an `ALTER` carries the
//!   statement's semicolon;
//! - table constraints (`UNIQUE`, `PRIMARY KEY`, `FOREIGN KEY`) render at
//!   plain four-space indent, list items separated by one space;
//! - `CREATE INDEX` / `CREATE UNIQUE INDEX` render on one line;
//! - one `alter_table` renders one `ALTER TABLE` statement: a lone ADD rides
//!   the `ALTER TABLE` line, further ADDs are indented four, and any column
//!   carrying `GENERATED ALWAYS AS (...) STORED` moves the statement onto its
//!   own header line with each generated expression on an eight-space
//!   continuation line (MySQL's generated-column syntax);
//! - tables close with `) DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;`
//!   (utf8mb4 plus the 0900_ai_ci collation is kelson's uniform MySQL default);
//! - blank lines are explicit (`Migration::blank`): layout belongs to the
//!   program that authors the file, and the gate pins the result.
//!
//! Type strings are literal text (`bigint` and `integer` stay lowercase where
//! the Postgres twin migrations are, `BIGINT UNSIGNED` stays uppercase), so
//! the canonical form fixes *layout*, never spelling.
//!
//! Regenerate the canonical files from the programs with
//! `KEEL_SCHEMA_REGENERATE=1 cargo test -p kelson-mysql --test schema_roundtrip`.

/// A column definition plus its fluent modifiers.
///
/// Built through the helpers on [`Table`] / [`Alter`]; modifiers chain on
/// `&mut Column` in the order they should read in the rendered DDL
/// (`not_null().default("0").primary_key()` renders `NOT NULL DEFAULT 0
/// PRIMARY KEY` regardless of call order).
#[derive(Clone, Debug, Default)]
pub struct Column {
    name: String,
    ty: String,
    not_null: bool,
    default: Option<String>,
    primary_key: bool,
    unique: bool,
    check: Option<String>,
    comment: Option<String>,
    generated: Option<String>,
}

impl Column {
    fn new(name: &str, ty: &str) -> Self {
        // Field-by-field: `..Self::default()` would resolve to the inherent
        // fluent `default()` below, not `Default::default()`.
        Self {
            name: name.to_string(),
            ty: ty.to_string(),
            not_null: false,
            default: None,
            primary_key: false,
            unique: false,
            check: None,
            comment: None,
            generated: None,
        }
    }

    /// Require a value (`NOT NULL`).
    pub fn not_null(&mut self) -> &mut Column {
        self.not_null = true;
        self
    }

    /// Append `DEFAULT <literal>` — the literal is SQL text as authored
    /// (`'active'`, `('active')`, `0`, `('[]')`).
    pub fn default(&mut self, literal: &str) -> &mut Column {
        self.default = Some(literal.to_string());
        self
    }

    /// `DEFAULT CURRENT_TIMESTAMP(6)` — MySQL's "now", microsecond precision.
    pub fn default_now(&mut self) -> &mut Column {
        self.default("CURRENT_TIMESTAMP(6)")
    }

    /// Inline column-level `PRIMARY KEY` (MySQL keys the row here; kelson ids
    /// are application-generated uuids, so there is no auto-increment).
    pub fn primary_key(&mut self) -> &mut Column {
        self.primary_key = true;
        self
    }

    /// Inline column-level `UNIQUE`.
    pub fn unique(&mut self) -> &mut Column {
        self.unique = true;
        self
    }

    /// Inline column-level `CHECK (<expr>)`.
    pub fn check(&mut self, expr: &str) -> &mut Column {
        self.check = Some(expr.to_string());
        self
    }

    /// An inline `-- <text>` comment, rendered two spaces after the line's
    /// comma (column semantics notes, enum vocabularies, pg divergences).
    pub fn comment(&mut self, text: &str) -> &mut Column {
        self.comment = Some(text.to_string());
        self
    }

    /// `GENERATED ALWAYS AS (<expr>) STORED` — MySQL's stand-in for the
    /// Postgres partial indexes (a non-matching row yields NULL in every
    /// part and unique indexes permit repeated NULLs).
    pub fn generated_stored(&mut self, expr: &str) -> &mut Column {
        self.generated = Some(expr.to_string());
        self
    }

    fn modifiers(&self) -> String {
        let mut out = String::new();
        if self.not_null {
            out.push_str(" NOT NULL");
        }
        if let Some(default) = &self.default {
            out.push_str(&format!(" DEFAULT {default}"));
        }
        if self.primary_key {
            out.push_str(" PRIMARY KEY");
        }
        if self.unique {
            out.push_str(" UNIQUE");
        }
        if let Some(check) = &self.check {
            out.push_str(&format!(" CHECK ({check})"));
        }
        out
    }

    /// The name/type/modifier text (no indent — the statement context owns
    /// that; `suffix` is `","`, `";"` or empty depending on the clause's
    /// position in its statement).
    fn head(&self, width: usize, suffix: &str) -> String {
        let mut head = format!("{:<width$}{}{}", self.name, self.ty, self.modifiers());
        if self.generated.is_none() {
            head.push_str(suffix);
            if let Some(comment) = &self.comment {
                head.push_str("  -- ");
                head.push_str(comment);
            }
        }
        head
    }

    /// The `GENERATED ALWAYS AS (...) STORED` continuation line, if any
    /// (eight-space indent baked in; carries the clause's `suffix`).
    fn generated_line(&self, suffix: &str) -> Option<String> {
        self.generated.as_ref().map(|expr| {
            let mut line = format!("        GENERATED ALWAYS AS ({expr}) STORED");
            line.push_str(suffix);
            if let Some(comment) = &self.comment {
                line.push_str("  -- ");
                line.push_str(comment);
            }
            line
        })
    }
}

fn name_field_width(columns: &[Column]) -> usize {
    columns
        .iter()
        .map(|c| c.name.chars().count())
        .max()
        .unwrap_or(0)
        + 2
}

/// The shared column vocabulary of [`Table`] and [`Alter`].
macro_rules! column_helpers {
    () => {
        /// A column with a literal SQL type (`bigint`, `BIGINT UNSIGNED`, …)
        /// — the escape hatch for types with no kelson convention.
        pub fn col(&mut self, name: &str, ty: &str) -> &mut Column {
            self.columns_mut().push(Column::new(name, ty));
            let index = self.columns_mut().len() - 1;
            &mut self.columns_mut()[index]
        }

        /// `BINARY(16)` — kelson's uuid (sqlx's MySQL codec writes raw bytes).
        pub fn uuid(&mut self, name: &str) -> &mut Column {
            self.col(name, "BINARY(16)")
        }

        /// `VARCHAR(<len>)`.
        pub fn varchar(&mut self, name: &str, len: u32) -> &mut Column {
            self.col(name, &format!("VARCHAR({len})"))
        }

        /// `TEXT`.
        pub fn text(&mut self, name: &str) -> &mut Column {
            self.col(name, "TEXT")
        }

        /// `DATETIME(6)` — kelson's timestamp (UTC, microsecond precision).
        pub fn datetime6(&mut self, name: &str) -> &mut Column {
            self.col(name, "DATETIME(6)")
        }

        /// `JSON` — kelson's document type (MySQL has no `jsonb`).
        pub fn json(&mut self, name: &str) -> &mut Column {
            self.col(name, "JSON")
        }

        /// `TINYINT(1)` — kelson's boolean.
        pub fn tinyint1(&mut self, name: &str) -> &mut Column {
            self.col(name, "TINYINT(1)")
        }

        /// `BLOB` — raw bytes (hashes).
        pub fn blob(&mut self, name: &str) -> &mut Column {
            self.col(name, "BLOB")
        }

        /// `bigint` — lowercase on purpose: it mirrors the Postgres twin
        /// migrations, whose integer types are lowercase.
        pub fn bigint(&mut self, name: &str) -> &mut Column {
            self.col(name, "bigint")
        }

        /// `integer` — lowercase, same Postgres-twin reasoning as
        /// [`bigint`](Self::bigint).
        pub fn integer(&mut self, name: &str) -> &mut Column {
            self.col(name, "integer")
        }
    };
}

/// A `CREATE TABLE` under construction.
#[derive(Debug, Default)]
pub struct Table {
    columns: Vec<Column>,
    constraints: Vec<String>,
}

impl Table {
    fn columns_mut(&mut self) -> &mut Vec<Column> {
        &mut self.columns
    }

    column_helpers!();

    /// Table-level `UNIQUE (<a>, <b>)`.
    pub fn unique(&mut self, columns: &[&str]) {
        self.constraints
            .push(format!("UNIQUE ({})", columns.join(", ")));
    }

    /// Table-level `PRIMARY KEY (<a>, <b>)` — for composite keys, where the
    /// key cannot ride a single column line.
    pub fn primary_key(&mut self, columns: &[&str]) {
        self.constraints
            .push(format!("PRIMARY KEY ({})", columns.join(", ")));
    }

    /// Table-level `FOREIGN KEY (<column>) REFERENCES <table>(<column>)` —
    /// MySQL parses but ignores inline `REFERENCES`, so every kelson foreign
    /// key is rewritten into this clause.
    pub fn foreign_key(&mut self, column: &str, referenced_table: &str, referenced_column: &str) {
        self.constraints.push(format!(
            "FOREIGN KEY ({column}) REFERENCES {referenced_table}({referenced_column})"
        ));
    }
}

/// An `ALTER TABLE` under construction: each `add` becomes one
/// `ADD COLUMN` clause, and the whole group renders as a single statement.
#[derive(Debug, Default)]
pub struct Alter {
    columns: Vec<Column>,
}

impl Alter {
    fn columns_mut(&mut self) -> &mut Vec<Column> {
        &mut self.columns
    }

    column_helpers!();
}

/// A MySQL migration file under construction.
///
/// Every method returns `Self`, so a program reads top-to-bottom exactly as
/// the file renders; layout (blank lines) is explicit because the round-trip
/// gate compares bytes, not structure.
#[derive(Debug, Default)]
pub struct Migration {
    out: String,
}

impl Migration {
    /// An empty migration.
    pub fn new() -> Self {
        Self::default()
    }

    fn line(&mut self, text: &str) {
        self.out.push_str(text);
        self.out.push('\n');
    }

    /// A verbatim line — the leading `-- ` is part of the text.
    pub fn comment(mut self, text: &str) -> Self {
        self.line(text);
        self
    }

    /// `-- ============ <title> ============`.
    pub fn section(mut self, title: &str) -> Self {
        self.line(&format!("-- ============ {title} ============"));
        self
    }

    /// A blank line.
    pub fn blank(mut self) -> Self {
        self.line("");
        self
    }

    /// A verbatim statement — a seeded `INSERT`, for instance.
    pub fn raw(mut self, sql: &str) -> Self {
        self.line(sql);
        self
    }

    /// `CREATE TABLE IF NOT EXISTS <name> (...) DEFAULT CHARSET=utf8mb4
    /// COLLATE=utf8mb4_0900_ai_ci;`
    pub fn create_table(mut self, name: &str, build: impl FnOnce(&mut Table)) -> Self {
        let mut table = Table::default();
        build(&mut table);
        assert!(
            !table.columns.is_empty(),
            "create_table({name}): at least one column is required"
        );
        self.line(&format!("CREATE TABLE IF NOT EXISTS {name} ("));
        let width = name_field_width(&table.columns);
        let total = table.columns.len() + table.constraints.len();
        let mut rendered = 0usize;
        for column in &table.columns {
            rendered += 1;
            let suffix = if rendered < total { "," } else { "" };
            self.line(&format!("    {}", column.head(width, suffix)));
            if let Some(generated) = column.generated_line(suffix) {
                self.line(&generated);
            }
        }
        for constraint in &table.constraints {
            rendered += 1;
            let suffix = if rendered < total { "," } else { "" };
            self.line(&format!("    {constraint}{suffix}"));
        }
        self.line(") DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;");
        self
    }

    /// One `ALTER TABLE <name>` statement carrying the group's `ADD COLUMN`
    /// clauses (see the module docs for the two shapes).
    pub fn alter_table(mut self, name: &str, build: impl FnOnce(&mut Alter)) -> Self {
        let mut alter = Alter::default();
        build(&mut alter);
        assert!(
            !alter.columns.is_empty(),
            "alter_table({name}): at least one add() is required"
        );
        let width = name_field_width(&alter.columns);
        let last = alter.columns.len() - 1;
        if alter.columns.iter().any(|c| c.generated.is_some()) {
            self.line(&format!("ALTER TABLE {name}"));
            for (index, column) in alter.columns.iter().enumerate() {
                let suffix = if index < last { "," } else { ";" };
                self.line(&format!("    ADD COLUMN {}", column.head(width, suffix)));
                if let Some(generated) = column.generated_line(suffix) {
                    self.line(&generated);
                }
            }
        } else {
            for (index, column) in alter.columns.iter().enumerate() {
                let suffix = if index < last { "," } else { ";" };
                let prefix = if index == 0 {
                    format!("ALTER TABLE {name} ADD COLUMN ")
                } else {
                    "    ADD COLUMN ".to_string()
                };
                self.line(&format!("{prefix}{}", column.head(width, suffix)));
            }
        }
        self
    }

    /// `CREATE INDEX <name> ON <table> (<columns>);` — one line; `<columns>`
    /// is the literal list (`"tenant_id, at"`, `"priority DESC"`).
    pub fn create_index(mut self, name: &str, table: &str, columns: &str) -> Self {
        self.line(&format!("CREATE INDEX {name} ON {table} ({columns});"));
        self
    }

    /// `CREATE UNIQUE INDEX <name> ON <table> (<columns>);`
    pub fn create_unique_index(mut self, name: &str, table: &str, columns: &str) -> Self {
        self.line(&format!(
            "CREATE UNIQUE INDEX {name} ON {table} ({columns});"
        ));
        self
    }

    /// The rendered migration file (LF lines, trailing newline included).
    pub fn render(&self) -> String {
        self.out.clone()
    }
}
