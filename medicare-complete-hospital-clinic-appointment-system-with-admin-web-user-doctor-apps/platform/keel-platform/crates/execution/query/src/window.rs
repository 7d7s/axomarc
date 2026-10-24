//! Common table expressions and window functions.
//!
//! Both are standard SQL that MySQL 8 and Postgres 8.4+ support; Kelson
//! gives them a typed API instead of the raw-SQL escape hatch Laravel has
//! to use (`DB::raw`). The window-function surface in particular is the
//! "more than Laravel" part: Laravel's query builder has **no** window
//! function API at all — a windowed aggregate in Laravel is a `DB::raw`
//! string, which is untyped, unvalidated and untestable.
//!
//! Everything here renders through the dialect, so a construct an engine
//! cannot honour is refused at build time rather than sent to the server.

use crate::select::{Direction, Ident};

/// The window function to compute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowFunction {
    /// `COUNT(*) OVER (...)`
    Count,
    /// `COUNT(column) OVER (...)`
    CountColumn(Ident),
    /// `SUM(column) OVER (...)`
    Sum(Ident),
    /// `AVG(column) OVER (...)`
    Avg(Ident),
    /// `MIN(column) OVER (...)`
    Min(Ident),
    /// `MAX(column) OVER (...)`
    Max(Ident),
    /// `ROW_NUMBER() OVER (...)` — the row's ordinal in the partition.
    RowNumber,
    /// `RANK() OVER (...)` — the row's rank, gaps included.
    Rank,
    /// `DENSE_RANK() OVER (...)` — the row's rank without gaps.
    DenseRank,
    /// `LAG(value) OVER (...)` — the value from the previous row.
    Lag(Ident),
    /// `LEAD(value) OVER (...)` — the value from the next row.
    Lead(Ident),
    /// `FIRST_VALUE(value) OVER (...)`
    FirstValue(Ident),
    /// `LAST_VALUE(value) OVER (...)`
    LastValue(Ident),
}

impl WindowFunction {
    /// The SQL text of the function call (without the OVER clause).
    pub(crate) fn render(&self) -> String {
        match self {
            WindowFunction::Count => "COUNT(*)".to_string(),
            WindowFunction::CountColumn(c) => format!("COUNT({})", c.as_str()),
            WindowFunction::Sum(c) => format!("SUM({})", c.as_str()),
            WindowFunction::Avg(c) => format!("AVG({})", c.as_str()),
            WindowFunction::Min(c) => format!("MIN({})", c.as_str()),
            WindowFunction::Max(c) => format!("MAX({})", c.as_str()),
            WindowFunction::RowNumber => "ROW_NUMBER()".to_string(),
            WindowFunction::Rank => "RANK()".to_string(),
            WindowFunction::DenseRank => "DENSE_RANK()".to_string(),
            WindowFunction::Lag(c) => format!("LAG({})", c.as_str()),
            WindowFunction::Lead(c) => format!("LEAD({})", c.as_str()),
            WindowFunction::FirstValue(c) => format!("FIRST_VALUE({})", c.as_str()),
            WindowFunction::LastValue(c) => format!("LAST_VALUE({})", c.as_str()),
        }
    }

    /// Whether the function needs a frame clause to be meaningful. The
    /// ranking functions (`ROW_NUMBER`, `RANK`, `DENSE_RANK`) ignore the
    /// frame, and `LAG`/`LEAD` do not take one.
    pub(crate) fn takes_frame(&self) -> bool {
        !matches!(
            self,
            WindowFunction::RowNumber
                | WindowFunction::Rank
                | WindowFunction::DenseRank
                | WindowFunction::Lag(_)
                | WindowFunction::Lead(_)
        )
    }
}

/// The window frame: which rows the aggregate sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowFrame {
    /// `ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW` — the running
    /// total (the default a reader expects from "a running sum").
    RowsUnboundedPreceding,
    /// `ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING` — the
    /// whole partition.
    RowsWholePartition,
    /// `ROWS BETWEEN CURRENT ROW AND UNBOUNDED FOLLOWING` — the remaining
    /// rows.
    RowsCurrentToUnbounded,
}

impl WindowFrame {
    pub(crate) fn render(self) -> &'static str {
        match self {
            WindowFrame::RowsUnboundedPreceding => {
                "ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW"
            }
            WindowFrame::RowsWholePartition => {
                "ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING"
            }
            WindowFrame::RowsCurrentToUnbounded => {
                "ROWS BETWEEN CURRENT ROW AND UNBOUNDED FOLLOWING"
            }
        }
    }
}

/// One windowed projection: the function, its window definition, an
/// optional cast and the alias it is projected under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowProjection {
    pub function: WindowFunction,
    pub partition_by: Vec<Ident>,
    pub order_by: Vec<(Ident, Direction)>,
    pub frame: Option<WindowFrame>,
    pub alias: Option<Ident>,
    /// An optional cast of the window's result.
    ///
    /// Needed because the result type is engine-determined: MySQL's `SUM`
    /// over an exact-value column returns `DECIMAL` and its `ROW_NUMBER()`
    /// returns `BIGINT UNSIGNED`, while Postgres returns `NUMERIC` and
    /// `BIGINT`. A cast pins the type the application reads.
    pub cast: Option<crate::dialect::CastTarget>,
}

impl WindowProjection {
    /// Render `fn(...) OVER (...) AS alias`.
    pub(crate) fn render(&self, dialect: &dyn crate::dialect::Dialect) -> String {
        let mut out = self.function.render();
        out.push_str(" OVER (");
        if !self.partition_by.is_empty() {
            let cols: Vec<&str> = self.partition_by.iter().map(|c| c.as_str()).collect();
            out.push_str("PARTITION BY ");
            out.push_str(&cols.join(", "));
        }
        if !self.order_by.is_empty() {
            if !self.partition_by.is_empty() {
                out.push(' ');
            }
            let parts: Vec<String> = self
                .order_by
                .iter()
                .map(|(col, dir)| {
                    format!(
                        "{} {}",
                        col.as_str(),
                        match dir {
                            Direction::Asc => "ASC",
                            Direction::Desc => "DESC",
                        }
                    )
                })
                .collect();
            out.push_str("ORDER BY ");
            out.push_str(&parts.join(", "));
        }
        if let Some(frame) = self.frame {
            if self.function.takes_frame() {
                out.push(' ');
                out.push_str(frame.render());
            }
        }
        out.push(')');
        if let Some(cast) = self.cast {
            out = dialect.render_cast(&out, cast);
        }
        if let Some(alias) = &self.alias {
            out.push_str(" AS ");
            out.push_str(alias.as_str());
        }
        out
    }
}

/// The specification of a windowed projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowSpec {
    pub function: WindowFunction,
    pub partition_by: Vec<Ident>,
    pub order_by: Vec<(Ident, Direction)>,
    pub frame: Option<WindowFrame>,
    pub alias: Option<Ident>,
    pub cast: Option<crate::dialect::CastTarget>,
}

impl WindowSpec {
    /// A window with no frame, no alias and no cast.
    pub fn new(function: WindowFunction) -> Self {
        Self {
            function,
            partition_by: Vec::new(),
            order_by: Vec::new(),
            frame: None,
            alias: None,
            cast: None,
        }
    }

    /// Partition the window by these columns.
    pub fn partition_by(mut self, columns: Vec<Ident>) -> Self {
        self.partition_by = columns;
        self
    }

    /// Order the window by these columns.
    pub fn order_by(mut self, columns: Vec<(Ident, Direction)>) -> Self {
        self.order_by = columns;
        self
    }

    /// Set the window frame.
    pub fn frame(mut self, frame: WindowFrame) -> Self {
        self.frame = Some(frame);
        self
    }

    /// Project the window under an alias.
    pub fn alias(mut self, alias: Ident) -> Self {
        self.alias = Some(alias);
        self
    }

    /// Cast the window's result to a target type.
    pub fn cast(mut self, target: crate::dialect::CastTarget) -> Self {
        self.cast = Some(target);
        self
    }
}

/// A common table expression: a named, typed subquery the statement can
/// select from.
#[derive(Debug, Clone)]
pub struct Cte {
    pub name: Ident,
    /// The column names the CTE projects (optional; the body's own names
    /// are used when absent).
    pub columns: Vec<Ident>,
    /// The CTE body.
    pub query: crate::select::Select,
    /// Whether the CTE may reference itself (the `WITH RECURSIVE` form).
    pub recursive: bool,
}
