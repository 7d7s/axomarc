//! Wave 9 B-31 / Phase B: transactional test isolation against live MySQL.
//!
//! The RefreshDatabase-equivalent suite setup for the mysql matrix — the
//! exact counterpart of [`keel_postgres::test_support::Savepoint`]: each
//! test body wraps its writes in `BEGIN; SAVEPOINT keel_test; … ; ROLLBACK
//! TO SAVEPOINT keel_test`, so two tests can touch the SAME table without
//! schema drops, without truncation, and without leaking rows into each
//! other. The outer `ROLLBACK` closes the scope, so the pooled connection
//! is always returned idle (no open transaction can leak into the next test
//! that checks the connection out).
//!
//! **MySQL divergence (documented, not silent):** MySQL issues an implicit
//! `COMMIT` on any DDL statement (`CREATE`/`ALTER`/`DROP`/`TRUNCATE`), so
//! this scope isolates **DML only** — a body that creates a table inside the
//! scope cannot roll that creation back. Postgres has no such restriction
//! (DDL is transactional there). This is the same limitation Laravel works
//! under on MySQL: migrations run once, outside each test's transaction,
//! rather than inside it. Test bodies that need a fresh schema must create
//! it before `begin`, or use distinct table names.
//!
//! Savepoint names are also not released by `ROLLBACK TO SAVEPOINT` in
//! MySQL (they survive until commit/rollback), which is why the scope still
//! issues the closing `ROLLBACK` rather than reusing the savepoint.
//!
//! Transaction control must additionally bypass the prepared-statement
//! protocol: MySQL rejects `BEGIN`/`SAVEPOINT`/`ROLLBACK` there with error
//! 1295 (HY000, "This command is not supported in the prepared statement
//! protocol yet"), so every statement in this module runs unprepared through
//! `raw_sql`, while a body's parameterized DML stays prepared as usual.
//! Postgres has no such restriction, so `keel_postgres::test_support` may
//! issue the same statements through `sqlx::query`.

use keel_foundation::AppError;

/// Savepoint name used by [`Savepoint::begin`] (docs/19 L341).
pub const TEST_SAVEPOINT: &str = "keel_test";

/// One test body's transactional isolation scope over MySQL.
///
/// ```ignore
/// let mut conn = pool.acquire().await?;
/// let mut sp = Savepoint::begin(&mut conn).await?;
/// // ... the body's SQL runs through sp.conn() ...
/// sp.rollback().await?; // ROLLBACK TO keel_test — nothing survives
/// ```
pub struct Savepoint<'c> {
    conn: &'c mut sqlx::MySqlConnection,
}

impl<'c> Savepoint<'c> {
    /// `BEGIN` + `SAVEPOINT keel_test` on this connection (fail-closed:
    /// any step erroring leaves nothing half-open on the caller's side —
    /// the borrow is returned with the `Err`).
    pub async fn begin(conn: &'c mut sqlx::MySqlConnection) -> Result<Self, AppError> {
        // Unprepared on purpose: MySQL error 1295 (HY000) refuses transaction
        // control through the prepared-statement protocol. See the module doc.
        sqlx::raw_sql("BEGIN")
            .execute(&mut *conn)
            .await
            .map_err(|e| AppError::persistence(format!("savepoint begin: {e}")))?;
        sqlx::raw_sql(&format!("SAVEPOINT {TEST_SAVEPOINT}"))
            .execute(&mut *conn)
            .await
            .map_err(|e| AppError::persistence(format!("savepoint: {e}")))?;
        Ok(Self { conn })
    }

    /// The connection INSIDE the scope — run the test body's SQL through it
    /// (inserts are visible to this transaction and to nothing else).
    pub fn conn(&mut self) -> &mut sqlx::MySqlConnection {
        self.conn
    }

    /// `ROLLBACK TO SAVEPOINT keel_test` — every write the body made is
    /// discarded — then rolls the outer `BEGIN` closed so the connection
    /// returns to the pool idle, exactly as found.
    pub async fn rollback(self) -> Result<(), AppError> {
        sqlx::raw_sql(&format!("ROLLBACK TO SAVEPOINT {TEST_SAVEPOINT}"))
            .execute(&mut *self.conn)
            .await
            .map_err(|e| AppError::persistence(format!("rollback to savepoint: {e}")))?;
        sqlx::raw_sql("ROLLBACK")
            .execute(&mut *self.conn)
            .await
            .map_err(|e| AppError::persistence(format!("savepoint close: {e}")))?;
        Ok(())
    }
}
