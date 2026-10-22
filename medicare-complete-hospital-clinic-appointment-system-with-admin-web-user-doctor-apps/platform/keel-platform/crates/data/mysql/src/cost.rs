//! The cost model.
//!
//! "10000x" also means **10000x cheaper to run**, and the cheapest
//! enterprise-scale failure is not a crash - it is one tenant's runaway
//! batch write quietly consuming the whole cluster's IO while every other
//! tenant waits.
//!
//! This module makes that failure **visible and bounded**:
//!
//! * [`CostBudget`] is the ceiling a tenant's work may cost: the rows a
//!   single write may touch and the wall-clock a single statement may take.
//! * [`CostLedger`] records what every statement actually cost - the tenant,
//!   the elapsed time and the rows affected - so the cost is an observable
//!   fact rather than an inference.
//! * [`CostGuard`] checks a write **before** it runs: a write that would
//!   touch more rows than the budget allows is refused, fail-closed, with
//!   the numbers in the message.
//!
//! The guard counts under the **same scope predicates** the write uses, so
//! the count and the write agree; a count taken under a wider scope would
//! refuse innocent writes and allow guilty ones.

use std::collections::HashMap;
use std::time::Duration;

use kelson_foundation::{AppError, TenantId};

/// The ceiling one tenant's work may cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CostBudget {
    /// The most rows one write may touch. `None` means unbounded.
    pub max_rows_per_write: Option<u64>,
    /// The most wall-clock one statement may take. `None` means unbounded.
    pub max_statement_elapsed: Option<Duration>,
    /// The most rows one read may return. `None` means unbounded.
    pub max_rows_per_read: Option<u64>,
}

impl CostBudget {
    /// A budget that allows nothing to be written more than `rows` at once.
    pub fn writes_at_most(rows: u64) -> Self {
        Self {
            max_rows_per_write: Some(rows),
            ..Default::default()
        }
    }

    /// Set the read ceiling.
    pub fn reads_at_most(mut self, rows: u64) -> Self {
        self.max_rows_per_read = Some(rows);
        self
    }

    /// Set the statement-time ceiling.
    pub fn statements_within(mut self, elapsed: Duration) -> Self {
        self.max_statement_elapsed = Some(elapsed);
        self
    }

    /// Whether a write of `rows` rows is within the budget.
    pub fn allows_write(&self, rows: u64) -> bool {
        self.max_rows_per_write.is_none_or(|max| rows <= max)
    }

    /// Whether a read of `rows` rows is within the budget.
    pub fn allows_read(&self, rows: u64) -> bool {
        self.max_rows_per_read.is_none_or(|max| rows <= max)
    }

    /// Whether a statement that took `elapsed` is within the budget.
    pub fn allows_elapsed(&self, elapsed: Duration) -> bool {
        self.max_statement_elapsed.is_none_or(|max| elapsed <= max)
    }
}

/// What a statement cost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatementCost {
    /// The tenant the statement ran for (`None` is a platform-plane
    /// statement).
    pub tenant: Option<TenantId>,
    /// Whether the statement read or wrote.
    pub kind: StatementKind,
    /// How long it took.
    pub elapsed: Duration,
    /// The rows it returned or affected.
    pub rows: u64,
}

/// Whether a statement read or wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatementKind {
    Read,
    Write,
}

impl StatementKind {
    /// The report spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            StatementKind::Read => "read",
            StatementKind::Write => "write",
        }
    }
}

/// The per-tenant cost ledger.
///
/// Bounded by design: it keeps the **aggregate** per tenant, not every
/// statement, so the memory it takes is a function of the tenant count and
/// not of the traffic.
#[derive(Debug, Clone, Default)]
pub struct CostLedger {
    totals: HashMap<String, TenantCost>,
}

/// The aggregate cost of one tenant's work.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TenantCost {
    /// How many statements ran.
    pub statements: u64,
    /// How many rows they returned or affected.
    pub rows: u64,
    /// The total wall-clock.
    pub elapsed: Duration,
    /// How many statements the budget refused.
    pub refused: u64,
}

impl CostLedger {
    /// An empty ledger.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a statement's cost.
    pub fn record(&mut self, cost: &StatementCost) {
        let key = cost
            .tenant
            .map(|t| t.as_uuid().to_string())
            .unwrap_or_else(|| "platform".to_string());
        let entry = self.totals.entry(key).or_default();
        entry.statements += 1;
        entry.rows += cost.rows;
        entry.elapsed += cost.elapsed;
    }

    /// Record a refusal (the statement did not run).
    pub fn record_refusal(&mut self, tenant: Option<TenantId>) {
        let key = tenant
            .map(|t| t.as_uuid().to_string())
            .unwrap_or_else(|| "platform".to_string());
        self.totals.entry(key).or_default().refused += 1;
    }

    /// What a tenant spent.
    pub fn cost_of(&self, tenant: &TenantId) -> Option<TenantCost> {
        self.totals.get(&tenant.as_uuid().to_string()).copied()
    }

    /// How many tenants the ledger knows about.
    pub fn tenants(&self) -> usize {
        self.totals.len()
    }

    /// The total statements recorded.
    pub fn statements(&self) -> u64 {
        self.totals.values().map(|c| c.statements).sum()
    }

    /// The total rows recorded.
    pub fn rows(&self) -> u64 {
        self.totals.values().map(|c| c.rows).sum()
    }
}

/// The guard: the ceiling a write must clear before it runs.
#[derive(Debug, Clone, Default)]
pub struct CostGuard {
    budget: CostBudget,
}

impl CostGuard {
    /// A guard with no ceiling (everything is allowed).
    pub fn new() -> Self {
        Self::default()
    }

    /// A guard with a budget.
    pub fn with_budget(budget: CostBudget) -> Self {
        Self { budget }
    }

    /// The budget in force.
    pub fn budget(&self) -> &CostBudget {
        &self.budget
    }

    /// Check a write that would touch `rows` rows.
    ///
    /// The check happens **before** the write: an over-budget write is
    /// refused with the numbers, not truncated after the fact.
    pub fn check_write(&self, tenant: Option<TenantId>, rows: u64) -> Result<(), AppError> {
        if self.budget.allows_write(rows) {
            return Ok(());
        }
        let tenant_label = tenant
            .map(|t| t.as_uuid().to_string())
            .unwrap_or_else(|| "platform".to_string());
        Err(AppError::denied(format!(
            "cost budget refused the write: tenant {tenant_label} would touch {rows} rows, the ceiling is {}",
            self.budget.max_rows_per_write.unwrap_or_default()
        )))
    }

    /// Check a read that returned `rows` rows.
    pub fn check_read(&self, tenant: Option<TenantId>, rows: u64) -> Result<(), AppError> {
        if self.budget.allows_read(rows) {
            return Ok(());
        }
        let tenant_label = tenant
            .map(|t| t.as_uuid().to_string())
            .unwrap_or_else(|| "platform".to_string());
        Err(AppError::denied(format!(
            "cost budget refused the read: tenant {tenant_label} would read {rows} rows, the ceiling is {}",
            self.budget.max_rows_per_read.unwrap_or_default()
        )))
    }

    /// Check a statement that took `elapsed`.
    pub fn check_elapsed(
        &self,
        tenant: Option<TenantId>,
        elapsed: Duration,
    ) -> Result<(), AppError> {
        if self.budget.allows_elapsed(elapsed) {
            return Ok(());
        }
        let tenant_label = tenant
            .map(|t| t.as_uuid().to_string())
            .unwrap_or_else(|| "platform".to_string());
        Err(AppError::denied(format!(
            "cost budget refused the statement: tenant {tenant_label} took {elapsed:?}, the ceiling is {:?}",
            self.budget.max_statement_elapsed
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_budget_allows_what_is_under_the_ceiling() {
        let budget = CostBudget::writes_at_most(100);
        assert!(budget.allows_write(100));
        assert!(!budget.allows_write(101));
        assert!(budget.allows_read(1_000_000), "an unset ceiling is unbounded");
        assert!(budget.allows_elapsed(Duration::from_secs(999)));
    }

    #[test]
    fn the_guard_refuses_an_over_budget_write_with_the_numbers() {
        let guard = CostGuard::with_budget(CostBudget::writes_at_most(10));
        assert!(guard.check_write(None, 10).is_ok());
        let err = guard
            .check_write(None, 11)
            .expect_err("the write is refused");
        assert!(
            err.public_message.contains("11 rows") && err.public_message.contains("ceiling is 10"),
            "the refusal must carry the numbers: {}",
            err.public_message
        );
    }

    #[test]
    fn the_ledger_aggregates_per_tenant() {
        let mut ledger = CostLedger::new();
        let tenant = TenantId::new();
        for _ in 0..3 {
            ledger.record(&StatementCost {
                tenant: Some(tenant),
                kind: StatementKind::Write,
                elapsed: Duration::from_millis(10),
                rows: 5,
            });
        }
        let cost = ledger.cost_of(&tenant).expect("the tenant is known");
        assert_eq!(cost.statements, 3);
        assert_eq!(cost.rows, 15);
        assert_eq!(cost.elapsed, Duration::from_millis(30));
        assert_eq!(ledger.tenants(), 1);
    }

    #[test]
    fn the_ledger_counts_a_refusal() {
        let mut ledger = CostLedger::new();
        let tenant = TenantId::new();
        ledger.record_refusal(Some(tenant));
        assert_eq!(ledger.cost_of(&tenant).expect("the tenant is known").refused, 1);
    }
}
