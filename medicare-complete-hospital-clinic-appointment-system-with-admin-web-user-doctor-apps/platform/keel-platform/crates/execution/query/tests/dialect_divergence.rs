//! The divergence proof.
//!
//! A Postgres-only construct must never reach MySQL. This test renders the
//! builder's whole surface through BOTH dialects and asserts:
//!
//! 1. the MySQL rendering carries **no** Postgres-only construct
//!    (`::date`, `RETURNING`, `ON CONFLICT`, `ILIKE`, `TO_CHAR`, `$N`);
//! 2. the Postgres rendering is byte-identical to the committed golden
//!    catalogue (that is `tests/golden_capture.rs`; this file is its
//!    MySQL-side twin);
//! 3. the compiled statement records the dialect it was rendered for, so an
//!    adapter can fail closed on a mismatch;
//! 4. a `RETURNING` request on a dialect that cannot honour it is
//!    **refused**, not silently dropped.

use kelson_query::{
    ConflictPolicy, Delete, DialectId, Direction, Ident, Insert, Operator, Select, Truncate, Update,
    Value, MYSQL, POSTGRES,
};

fn id(raw: &str) -> Ident {
    Ident::new(raw).expect("the identifier is valid")
}

fn v(text: &str) -> Value {
    Value::Text(text.to_string())
}

/// The Postgres-only constructs that must never appear in a MySQL
/// statement. Each entry: (a human name, the needle).
const PG_ONLY: &[(&str, &str)] = &[
    ("the :: cast", "::"),
    ("RETURNING", "RETURNING"),
    ("ON CONFLICT", "ON CONFLICT"),
    ("EXCLUDED", "EXCLUDED."),
    ("ILIKE", "ILIKE"),
    ("TO_CHAR", "TO_CHAR"),
    ("a numbered placeholder", "$1"),
];

/// Every statement shape the builder can emit, rendered for a dialect.
fn catalogue(dialect: &dyn kelson_query::Dialect) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut push = |name: &str, sql: String| out.push((name.to_string(), sql));

    push(
        "select_star",
        Select::from_table(id("users")).build(dialect).unwrap().sql,
    );
    push(
        "select_columns",
        Select::from_table(id("users"))
            .select(vec![id("id"), id("email")])
            .build(dialect)
            .unwrap()
            .sql,
    );
    push(
        "select_distinct",
        Select::from_table(id("users"))
            .distinct()
            .build(dialect)
            .unwrap()
            .sql,
    );
    push(
        "select_where_eq",
        Select::from_table(id("users"))
            .where_eq(id("tenant_id"), v("t1"))
            .build(dialect)
            .unwrap()
            .sql,
    );
    push(
        "select_where_in",
        Select::from_table(id("users"))
            .where_in(id("id"), vec![v("a"), v("b")])
            .build(dialect)
            .unwrap()
            .sql,
    );
    push(
        "select_where_between",
        Select::from_table(id("users"))
            .where_between(id("age"), 10i64, 20i64)
            .build(dialect)
            .unwrap()
            .sql,
    );
    push(
        "select_where_null",
        Select::from_table(id("users"))
            .where_null(id("deleted_at"))
            .build(dialect)
            .unwrap()
            .sql,
    );
    push(
        "select_where_ilike",
        Select::from_table(id("users"))
            .where_op(id("email"), Operator::Ilike, v("%@x.test"))
            .build(dialect)
            .unwrap()
            .sql,
    );
    push(
        "select_where_date",
        Select::from_table(id("users"))
            .where_date(id("created_at"), v("2026-01-01"))
            .build(dialect)
            .unwrap()
            .sql,
    );
    push(
        "select_where_year",
        Select::from_table(id("users"))
            .where_year(id("created_at"), 2026i64)
            .build(dialect)
            .unwrap()
            .sql,
    );
    push(
        "select_where_time",
        Select::from_table(id("users"))
            .where_time(id("created_at"), v("10:30:00"))
            .build(dialect)
            .unwrap()
            .sql,
    );
    push(
        "select_where_exists",
        Select::from_table(id("users"))
            .where_exists(
                Select::from_table(id("orders"))
                    .select(vec![id("user_id")])
                    .where_eq(id("status"), v("paid")),
            )
            .build(dialect)
            .unwrap()
            .sql,
    );
    push(
        "select_where_row_cmp",
        Select::from_table(id("users"))
            .where_row_cmp(vec![id("a"), id("b")], Operator::Eq, vec![v("1"), v("2")])
            .build(dialect)
            .unwrap()
            .sql,
    );
    push(
        "select_having_aggregate",
        Select::from_table(id("users"))
            .having_aggregate(kelson_query::Aggregate::Count, Operator::Gt, 5i64)
            .build(dialect)
            .unwrap()
            .sql,
    );
    push(
        "select_group_by",
        Select::from_table(id("users"))
            .group_by(id("tenant_id"))
            .build(dialect)
            .unwrap()
            .sql,
    );
    push(
        "select_limit_offset",
        Select::from_table(id("users"))
            .limit(25)
            .offset(50)
            .build(dialect)
            .unwrap()
            .sql,
    );
    push(
        "select_union",
        Select::from_table(id("users"))
            .union(Select::from_table(id("admins")).select(vec![id("id")]))
            .build(dialect)
            .unwrap()
            .sql,
    );
    push(
        "select_lock_for_update",
        Select::from_table(id("users"))
            .lock_for_update()
            .build(dialect)
            .unwrap()
            .sql,
    );
    push(
        "select_joins",
        Select::from_table(id("users"))
            .join_inner(id("orders"), id("users.id"), id("orders.user_id"))
            .join_left(id("plans"), id("users.plan_id"), id("plans.id"))
            .build(dialect)
            .unwrap()
            .sql,
    );
    push(
        "insert_plain",
        Insert::into_table(id("users"))
            .columns(vec![id("id"), id("email")])
            .row(vec![v("u1"), v("a@b.test")])
            .build(dialect)
            .unwrap()
            .sql,
    );
    push(
        "insert_do_nothing",
        Insert::into_table(id("users"))
            .columns(vec![id("id"), id("email")])
            .row(vec![v("u1"), v("a@b.test")])
            .on_conflict(vec![id("email")], ConflictPolicy::DoNothing)
            .build(dialect)
            .unwrap()
            .sql,
    );
    push(
        "insert_do_update",
        Insert::into_table(id("users"))
            .columns(vec![id("id"), id("email")])
            .row(vec![v("u1"), v("a@b.test")])
            .on_conflict(vec![id("email")], ConflictPolicy::DoUpdate)
            .build(dialect)
            .unwrap()
            .sql,
    );
    push(
        "update_set_where",
        Update::table(id("users"))
            .set(id("email"), v("new@b.test"))
            .where_eq(id("id"), v("u1"))
            .build(dialect)
            .unwrap()
            .sql,
    );
    push(
        "update_increment",
        Update::table(id("users"))
            .increment(id("hits"), 1)
            .where_eq(id("id"), v("u1"))
            .build(dialect)
            .unwrap()
            .sql,
    );
    push(
        "delete_where",
        Delete::from_table(id("users"))
            .where_eq(id("id"), v("u1"))
            .build(dialect)
            .unwrap()
            .sql,
    );
    push(
        "select_with_cte",
        Select::from_table(id("recent"))
            .with_cte("recent", |cte| {
                cte.table(id("orders"))
                    .select(vec![id("user_id")])
                    .where_eq(id("status"), v("paid"))
            })
            .select(vec![id("user_id")])
            .build(dialect)
            .unwrap()
            .sql,
    );
    push(
        "select_with_recursive_cte",
        Select::from_table(id("tree"))
            .with_recursive_cte(
                "tree",
                vec![id("id"), id("parent_id")],
                |cte| {
                    cte.table(id("nodes"))
                        .select(vec![id("id"), id("parent_id")])
                        .where_null(id("parent_id"))
                        .union_all(
                            Select::from_table(id("nodes"))
                                .select(vec![id("id"), id("parent_id")])
                                .join_inner(id("tree"), id("nodes.parent_id"), id("tree.id")),
                        )
                },
            )
            .select(vec![id("id")])
            .build(dialect)
            .unwrap()
            .sql,
    );
    push(
        "select_with_window",
        Select::from_table(id("orders"))
            .select(vec![id("label")])
            .window(
                kelson_query::WindowSpec::new(kelson_query::WindowFunction::Sum(id("amount")))
                    .partition_by(vec![id("tenant_id")])
                    .order_by(vec![(id("created_at"), Direction::Asc)])
                    .cast(kelson_query::CastTarget::BigInt),
            )
            .build(dialect)
            .unwrap()
            .sql,
    );
    push(
        "select_lock_skip_locked",
        Select::from_table(id("jobs"))
            .lock_for_update_skip_locked()
            .build(dialect)
            .unwrap()
            .sql,
    );
    push(
        "select_lock_nowait",
        Select::from_table(id("jobs"))
            .lock_for_update_nowait()
            .build(dialect)
            .unwrap()
            .sql,
    );
    push(
        "select_intersect",
        Select::from_table(id("users"))
            .intersect(Select::from_table(id("admins")).select(vec![id("id")]))
            .build(dialect)
            .unwrap()
            .sql,
    );
    push(
        "select_except",
        Select::from_table(id("users"))
            .except(Select::from_table(id("admins")).select(vec![id("id")]))
            .build(dialect)
            .unwrap()
            .sql,
    );
    push(
        "truncate",
        Truncate::table(id("users")).build(dialect).unwrap().sql,
    );
    out
}

#[test]
fn the_mysql_rendering_carries_no_postgres_only_construct() {
    let mut leaks: Vec<String> = Vec::new();
    for (name, sql) in catalogue(&MYSQL) {
        for (label, needle) in PG_ONLY {
            if sql.contains(needle) {
                leaks.push(format!("{name}: the {label} ({needle}) leaked into: {sql}"));
            }
        }
    }
    assert!(
        leaks.is_empty(),
        "a Postgres-only construct reached the MySQL rendering:\n{}",
        leaks.join("\n")
    );
}

#[test]
fn the_postgres_rendering_carries_no_mysql_only_construct() {
    // The mirror: MySQL's row alias and its duplicate-key clause must not
    // reach Postgres.
    const MY_ONLY: &[(&str, &str)] = &[("the row alias", "AS new"), ("ON DUPLICATE KEY", "ON DUPLICATE KEY")];
    let mut leaks: Vec<String> = Vec::new();
    for (name, sql) in catalogue(&POSTGRES) {
        for (label, needle) in MY_ONLY {
            if sql.contains(needle) {
                leaks.push(format!("{name}: the {label} ({needle}) leaked into: {sql}"));
            }
        }
    }
    assert!(
        leaks.is_empty(),
        "a MySQL-only construct reached the Postgres rendering:\n{}",
        leaks.join("\n")
    );
}

#[test]
fn the_two_dialects_differ_where_they_must() {
    let pg: Vec<(String, String)> = catalogue(&POSTGRES);
    let my: Vec<(String, String)> = catalogue(&MYSQL);
    assert_eq!(pg.len(), my.len(), "the catalogues must be the same shape");

    // A statement that BINDS a value must differ: the placeholder text is
    // the difference (`$1` vs `?`). A statement with no binds renders
    // identically in both engines UNLESS it uses a dialect-specific
    // construct (the UNION form, the row lock, the cast, the date part).
    let mut with_binds = 0;
    let mut differing_with_binds = 0;
    for ((pg_name, pg_sql), (my_name, my_sql)) in pg.iter().zip(my.iter()) {
        assert_eq!(pg_name, my_name);
        let binds = bind_count(pg_sql);
        if binds > 0 {
            with_binds += 1;
            if pg_sql != my_sql {
                differing_with_binds += 1;
            }
        } else if pg_sql != my_sql {
            assert!(
                DIALECT_SPECIFIC.iter().any(|m| pg_sql.contains(m)),
                "{pg_name} has no binds and no dialect-specific construct, yet it differs:\n  pg:   {pg_sql}\n  mysql: {my_sql}"
            );
        }
    }
    assert!(with_binds > 0, "the catalogue must contain binding statements");
    assert_eq!(
        differing_with_binds, with_binds,
        "every binding statement must differ by dialect"
    );
}

/// The constructs whose spelling differs between the engines. A statement
/// that uses none of them must render identically for both.
const DIALECT_SPECIFIC: &[&str] = &[
    "UNION",
    "FOR UPDATE",
    "FOR SHARE",
    "NOWAIT",
    "SKIP LOCKED",
    "AS new",
    "ON DUPLICATE KEY",
    "DATE(",
    "DATE_FORMAT",
    "EXTRACT",
    "TO_CHAR",
    "::",
    "ILIKE",
    "CAST",
    "INTERSECT",
    "EXCEPT",
];

/// How many bind placeholders a rendered statement carries.
fn bind_count(sql: &str) -> usize {
    // Postgres numbers its placeholders; MySQL uses `?`.
    if sql.contains('$') {
        let mut highest = 0usize;
        for part in sql.split('$').skip(1) {
            let digits: String = part.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(n) = digits.parse::<usize>() {
                if n > highest {
                    highest = n;
                }
            }
        }
        highest
    } else {
        sql.matches('?').count()
    }
}

#[test]
fn the_compiled_statement_records_its_dialect() {
    let q = Select::from_table(id("users")).build(&MYSQL).unwrap();
    assert_eq!(q.dialect, DialectId::MySql);
    let q = Select::from_table(id("users")).build(&POSTGRES).unwrap();
    assert_eq!(q.dialect, DialectId::Postgres);

    let w = Insert::into_table(id("users"))
        .columns(vec![id("id")])
        .row(vec![v("u1")])
        .build(&MYSQL)
        .unwrap();
    assert_eq!(w.dialect, DialectId::MySql);
}

#[test]
fn a_returning_request_on_mysql_is_refused_not_dropped() {
    let err = Insert::into_table(id("users"))
        .columns(vec![id("id")])
        .row(vec![v("u1")])
        .returning(vec![id("id")])
        .build(&MYSQL)
        .expect_err("MySQL cannot RETURNING; the build must be refused");
    assert!(
        err.public_message.contains("RETURNING"),
        "the refusal must name the construct: {}",
        err.public_message
    );

    // ... and Postgres still honours it.
    let ok = Insert::into_table(id("users"))
        .columns(vec![id("id")])
        .row(vec![v("u1")])
        .returning(vec![id("id")])
        .build(&POSTGRES)
        .expect("Postgres supports RETURNING");
    assert!(ok.sql.ends_with("RETURNING id"));
}

#[test]
fn the_mysql_bind_order_is_the_postgres_bind_order() {
    // The parameter vector is dialect-independent: only the placeholder
    // text changes. A statement that binds three values must carry the
    // same three values in the same order for both engines.
    let build = |d: &dyn kelson_query::Dialect| {
        Select::from_table(id("users"))
            .where_eq(id("a"), v("1"))
            .where_in(id("b"), vec![v("2"), v("3")])
            .order_by(id("id"), kelson_query::Direction::Asc)
            .limit(10)
            .build(d)
            .unwrap()
    };
    let pg = build(&POSTGRES);
    let my = build(&MYSQL);
    assert_eq!(pg.params, my.params, "the binds must be identical");
    assert_eq!(pg.params.len(), 4);
    assert_eq!(pg.sql.matches("$").count(), 4);
    assert_eq!(my.sql.matches('?').count(), 4);
}
