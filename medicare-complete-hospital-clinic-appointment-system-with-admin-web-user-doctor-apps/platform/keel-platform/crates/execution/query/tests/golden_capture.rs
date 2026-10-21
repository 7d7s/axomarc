//! GOLDEN CATALOGUE - the Postgres rendering regression.
//!
//! This file builds a catalogue of every construct the builder can emit and
//! compares the result against the committed fixture
//! `tests/golden_postgres.txt`.
//!
//! It is the proof that the dialect refactor did not change the **Postgres**
//! rendering by a single byte — which matters because PostgreSQL cannot be
//! executed on this host (the client's native backends fault), so the golden
//! catalogue is the only available regression signal for that engine.
//!
//! Run it with:
//!   cargo test -p kelson-query --test golden_capture
//!
//! To re-record the fixture after an INTENDED change:
//!   KELSON_REGEN_GOLDEN=1 cargo test -p kelson-query --test golden_capture

use kelson_query::{
    ConflictPolicy, Delete, Ident, Insert, Operator, Select, Truncate, Update, Value, POSTGRES,
};

fn id(raw: &str) -> Ident {
    Ident::new(raw).expect("the identifier is valid")
}

fn v(text: &str) -> Value {
    Value::Text(text.to_string())
}

/// One catalogue entry: a name and the rendered statement.
fn entry(name: &str, sql: &str, params: &[Value]) -> String {
    let binds = params
        .iter()
        .map(|p| format!("{p:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("{name}\n  sql: {sql}\n  params: [{binds}]")
}

#[test]
fn the_postgres_rendering_matches_the_golden_catalogue() {
    let mut out: Vec<String> = Vec::new();

    // ---- SELECT ----------------------------------------------------------
    let q = Select::from_table(id("users")).build(&POSTGRES).unwrap();
    out.push(entry("select_star", &q.sql, &q.params));

    let q = Select::from_table(id("users"))
        .select(vec![id("id"), id("email")])
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("select_columns", &q.sql, &q.params));

    let q = Select::from_table(id("users")).distinct().build(&POSTGRES).unwrap();
    out.push(entry("select_distinct", &q.sql, &q.params));

    let q = Select::from_table(id("users"))
        .where_eq(id("tenant_id"), v("t1"))
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("select_where_eq", &q.sql, &q.params));

    let q = Select::from_table(id("users"))
        .where_op(id("age"), Operator::Gt, 30i64)
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("select_where_op", &q.sql, &q.params));

    let q = Select::from_table(id("users"))
        .where_in(id("id"), vec![v("a"), v("b")])
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("select_where_in", &q.sql, &q.params));

    let q = Select::from_table(id("users"))
        .where_not_in(id("id"), vec![v("a"), v("b")])
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("select_where_not_in", &q.sql, &q.params));

    let q = Select::from_table(id("users"))
        .where_between(id("age"), 10i64, 20i64)
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("select_where_between", &q.sql, &q.params));

    let q = Select::from_table(id("users"))
        .where_not_between(id("age"), 10i64, 20i64)
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("select_where_not_between", &q.sql, &q.params));

    let q = Select::from_table(id("users"))
        .where_null(id("deleted_at"))
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("select_where_null", &q.sql, &q.params));

    let q = Select::from_table(id("users"))
        .where_not_null(id("deleted_at"))
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("select_where_not_null", &q.sql, &q.params));

    let q = Select::from_table(id("users"))
        .where_column(id("users.id"), id("orders.user_id"))
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("select_where_column", &q.sql, &q.params));

    let q = Select::from_table(id("users"))
        .where_column_op(id("users.id"), Operator::Eq, id("orders.user_id"))
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("select_where_column_op", &q.sql, &q.params));

    let q = Select::from_table(id("users"))
        .where_like(id("email"), v("%@x.test"))
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("select_where_like", &q.sql, &q.params));

    let q = Select::from_table(id("users"))
        .where_op(id("email"), Operator::Ilike, v("%@x.test"))
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("select_where_ilike", &q.sql, &q.params));

    let q = Select::from_table(id("users"))
        .where_date(id("created_at"), v("2026-01-01"))
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("select_where_date", &q.sql, &q.params));

    let q = Select::from_table(id("users"))
        .where_year(id("created_at"), 2026i64)
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("select_where_year", &q.sql, &q.params));

    let q = Select::from_table(id("users"))
        .where_month(id("created_at"), 3i64)
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("select_where_month", &q.sql, &q.params));

    let q = Select::from_table(id("users"))
        .where_day(id("created_at"), 7i64)
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("select_where_day", &q.sql, &q.params));

    let q = Select::from_table(id("users"))
        .where_time(id("created_at"), v("10:30:00"))
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("select_where_time", &q.sql, &q.params));

    let sub = Select::from_table(id("orders"))
        .select(vec![id("user_id")])
        .where_eq(id("status"), v("paid"))
        .build(&POSTGRES)
        .unwrap();
    let q = Select::from_table(id("users"))
        .where_exists(Select::from_table(id("orders")).select(vec![id("user_id")]).where_eq(id("status"), v("paid")))
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("select_where_exists", &q.sql, &q.params));
    let _ = sub;

    let q = Select::from_table(id("users"))
        .where_not_exists(Select::from_table(id("orders")).select(vec![id("user_id")]).where_eq(id("status"), v("paid")))
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("select_where_not_exists", &q.sql, &q.params));

    let q = Select::from_table(id("users"))
        .where_row_cmp(vec![id("a"), id("b")], Operator::Eq, vec![v("1"), v("2")])
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("select_where_row_cmp", &q.sql, &q.params));

    let q = Select::from_table(id("users"))
        .where_eq(id("a"), v("1"))
        .or_where(id("b"), Operator::Eq, v("2"))
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("select_or_where", &q.sql, &q.params));

    let q = Select::from_table(id("users"))
        .group(|g| g.where_eq(id("a"), v("1")).or_where(id("b"), Operator::Eq, v("2")))
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("select_where_group", &q.sql, &q.params));

    let q = Select::from_table(id("users"))
        .having_aggregate(kelson_query::Aggregate::Count, Operator::Gt, 5i64)
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("select_having_aggregate", &q.sql, &q.params));

    let q = Select::from_table(id("users"))
        .having_eq(id("status"), v("paid"))
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("select_having_eq", &q.sql, &q.params));

    let q = Select::from_table(id("users"))
        .group_by(id("tenant_id"))
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("select_group_by", &q.sql, &q.params));

    let q = Select::from_table(id("users"))
        .order_by(id("created_at"), kelson_query::Direction::Desc)
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("select_order_by", &q.sql, &q.params));

    let q = Select::from_table(id("users")).count().build(&POSTGRES).unwrap();
    out.push(entry("select_count", &q.sql, &q.params));

    let q = Select::from_table(id("users"))
        .sum(id("amount"))
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("select_sum", &q.sql, &q.params));

    let q = Select::from_table(id("users"))
        .limit(25)
        .offset(50)
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("select_limit_offset", &q.sql, &q.params));

    let q = Select::from_table(id("users"))
        .union(Select::from_table(id("admins")).select(vec![id("id")]))
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("select_union", &q.sql, &q.params));

    let q = Select::from_table(id("users"))
        .union_all(Select::from_table(id("admins")).select(vec![id("id")]))
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("select_union_all", &q.sql, &q.params));

    let q = Select::from_table(id("users")).lock_for_update().build(&POSTGRES).unwrap();
    out.push(entry("select_lock_for_update", &q.sql, &q.params));

    let q = Select::from_table(id("users")).shared_lock().build(&POSTGRES).unwrap();
    out.push(entry("select_shared_lock", &q.sql, &q.params));

    let q = Select::from_table(id("users"))
        .join_inner(id("orders"), id("users.id"), id("orders.user_id"))
        .join_left(id("plans"), id("users.plan_id"), id("plans.id"))
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("select_joins", &q.sql, &q.params));

    let q = Select::from_table(id("recent"))
        .with_cte("recent", |cte| {
            cte.table(id("orders"))
                .select(vec![id("user_id")])
                .where_eq(id("status"), v("paid"))
        })
        .select(vec![id("user_id")])
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("select_with_cte", &q.sql, &q.params));

    let q = Select::from_table(id("tree"))
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
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("select_with_recursive_cte", &q.sql, &q.params));

    let q = Select::from_table(id("orders"))
        .select(vec![id("label")])
        .window(
            kelson_query::WindowSpec::new(kelson_query::WindowFunction::Sum(id("amount")))
                .partition_by(vec![id("tenant_id")])
                .order_by(vec![(id("created_at"), kelson_query::Direction::Asc)])
                .cast(kelson_query::CastTarget::BigInt),
        )
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("select_with_window", &q.sql, &q.params));

    let q = Select::from_table(id("jobs"))
        .lock_for_update_skip_locked()
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("select_lock_skip_locked", &q.sql, &q.params));

    let q = Select::from_table(id("jobs"))
        .lock_for_update_nowait()
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("select_lock_nowait", &q.sql, &q.params));

    let q = Select::from_table(id("users"))
        .intersect(Select::from_table(id("admins")).select(vec![id("id")]))
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("select_intersect", &q.sql, &q.params));

    let q = Select::from_table(id("users"))
        .except(Select::from_table(id("admins")).select(vec![id("id")]))
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("select_except", &q.sql, &q.params));

    // ---- INSERT ----------------------------------------------------------
    let q = Insert::into_table(id("users"))
        .columns(vec![id("id"), id("email")])
        .row(vec![v("u1"), v("a@b.test")])
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("insert_plain", &q.sql, &q.params));

    let q = Insert::into_table(id("users"))
        .columns(vec![id("id"), id("email")])
        .row(vec![v("u1"), v("a@b.test")])
        .on_conflict(vec![id("email")], ConflictPolicy::DoNothing)
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("insert_on_conflict_do_nothing", &q.sql, &q.params));

    let q = Insert::into_table(id("users"))
        .columns(vec![id("id"), id("email")])
        .row(vec![v("u1"), v("a@b.test")])
        .on_conflict(vec![id("email")], ConflictPolicy::DoUpdate)
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("insert_on_conflict_do_update", &q.sql, &q.params));

    let q = Insert::into_table(id("users"))
        .columns(vec![id("id"), id("email")])
        .row(vec![v("u1"), v("a@b.test")])
        .on_conflict(vec![id("email")], ConflictPolicy::DoNothing)
        .returning(vec![id("id")])
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("insert_returning", &q.sql, &q.params));

    // ---- UPDATE ----------------------------------------------------------
    let q = Update::table(id("users"))
        .set(id("email"), v("new@b.test"))
        .where_eq(id("id"), v("u1"))
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("update_set_where", &q.sql, &q.params));

    let q = Update::table(id("users"))
        .increment(id("hits"), 1)
        .where_eq(id("id"), v("u1"))
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("update_increment", &q.sql, &q.params));

    let q = Update::table(id("users"))
        .set(id("email"), v("new@b.test"))
        .where_eq(id("id"), v("u1"))
        .returning(vec![id("id")])
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("update_returning", &q.sql, &q.params));

    // ---- DELETE ----------------------------------------------------------
    let q = Delete::from_table(id("users"))
        .where_eq(id("id"), v("u1"))
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("delete_where", &q.sql, &q.params));

    let q = Delete::from_table(id("users"))
        .where_eq(id("id"), v("u1"))
        .returning(vec![id("id")])
        .build(&POSTGRES)
        .unwrap();
    out.push(entry("delete_returning", &q.sql, &q.params));

    // ---- TRUNCATE --------------------------------------------------------
    let q = Truncate::table(id("users")).build(&POSTGRES).unwrap();
    out.push(entry("truncate", &q.sql, &q.params));

    let text = format!("{}\n", out.join("\n"));
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden_postgres.txt");

    if std::env::var("KELSON_REGEN_GOLDEN").is_ok() {
        std::fs::write(&fixture, &text).expect("the golden file is written");
        println!("re-recorded the golden catalogue: {} bytes", text.len());
        return;
    }

    let expected = std::fs::read_to_string(&fixture).unwrap_or_else(|e| {
        panic!(
            "the golden fixture is missing ({e}); re-record it with KELSON_REGEN_GOLDEN=1"
        )
    });
    if expected != text {
        let exp_lines: Vec<&str> = expected.lines().collect();
        let got_lines: Vec<&str> = text.lines().collect();
        let mut report = String::new();
        for (i, (e, g)) in exp_lines.iter().zip(got_lines.iter()).enumerate() {
            if e != g {
                report.push_str(&format!(
                    "line {}:\n  expected: {e}\n  actual:   {g}\n",
                    i + 1
                ));
            }
        }
        if exp_lines.len() != got_lines.len() {
            report.push_str(&format!(
                "the line counts differ: expected {}, actual {}\n",
                exp_lines.len(),
                got_lines.len()
            ));
        }
        panic!("the Postgres rendering diverged from the golden catalogue:\n{report}");
    }
}
