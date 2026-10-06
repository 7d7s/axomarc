//! Wave 7 P3 (docs/19 L341): transactional test isolation against live Pg.
//!
//! The RefreshDatabase-equivalent suite setup for the pg matrix: each test
//! body wraps its writes in `BEGIN; SAVEPOINT keel_test; … ; ROLLBACK TO
//! SAVEPOINT keel_test`, so two tests can touch the SAME table without
//! schema drops, without truncation, and without leaking rows into each
//! other. The outer `ROLLBACK` closes the scope, so the pooled connection
//! is always returned idle (no open transaction can leak into the next
//! test that checks the connection out).

use keel_foundation::AppError;

/// Savepoint name used by [`Savepoint::begin`] (docs/19 L341).
pub const TEST_SAVEPOINT: &str = "keel_test";

/// One test body's transactional isolation scope.
///
/// ```ignore
/// let mut conn = pool.acquire().await?;
/// let mut sp = Savepoint::begin(&mut conn).await?;
/// // ... the body's SQL runs through sp.conn() ...
/// sp.rollback().await?; // ROLLBACK TO keel_test — nothing survives
/// ```
pub struct Savepoint<'c> {
    conn: &'c mut sqlx::PgConnection,
}

impl<'c> Savepoint<'c> {
    /// `BEGIN` + `SAVEPOINT keel_test` on this connection (fail-closed:
    /// any step erroring leaves nothing half-open on the caller's side —
    /// the borrow is returned with the `Err`).
    pub async fn begin(conn: &'c mut sqlx::PgConnection) -> Result<Self, AppError> {
        sqlx::query("BEGIN")
            .execute(&mut *conn)
            .await
            .map_err(|e| AppError::persistence(format!("savepoint begin: {e}")))?;
        sqlx::query(&format!("SAVEPOINT {TEST_SAVEPOINT}"))
            .execute(&mut *conn)
            .await
            .map_err(|e| AppError::persistence(format!("savepoint: {e}")))?;
        Ok(Self { conn })
    }

    /// The connection INSIDE the scope — run the test body's SQL through it
    /// (inserts are visible to this transaction and to nothing else).
    pub fn conn(&mut self) -> &mut sqlx::PgConnection {
        self.conn
    }

    /// `ROLLBACK TO SAVEPOINT keel_test` — every write the body made is
    /// discarded — then rolls the outer `BEGIN` closed so the connection
    /// returns to the pool idle, exactly as found.
    pub async fn rollback(self) -> Result<(), AppError> {
        sqlx::query(&format!("ROLLBACK TO SAVEPOINT {TEST_SAVEPOINT}"))
            .execute(&mut *self.conn)
            .await
            .map_err(|e| AppError::persistence(format!("rollback to savepoint: {e}")))?;
        sqlx::query("ROLLBACK")
            .execute(&mut *self.conn)
            .await
            .map_err(|e| AppError::persistence(format!("savepoint close: {e}")))?;
        Ok(())
    }
}
