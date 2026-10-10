//! Query-budget assertions (item 49, S4): prove an execution stays within its
//! probe budget using the engine's global `QueryTap`.
//!
//! The tap is process-global and exists for dev/test diagnostics — it is not
//! an accounting system. Tests running in parallel share it, so assert on
//! ranges (`<=`) or use unique probe-shape prefixes per test file.

/// Assert how many database probes an awaited body records.
///
/// Must be used inside an async context (the body is awaited). The macro marks
/// the global `QueryTap`, awaits the body, takes everything recorded since the
/// mark and checks the count against the budget before returning the body's
/// result.
///
/// ```ignore
/// let outcome = assert_queries!(<= 5, engine.execute(ctx, &handler, input, key));
/// assert_queries!(== 1, engine.execute(ctx, &query, input, None)).await;
/// ```
#[macro_export]
macro_rules! assert_queries {
    (<= $max:expr, $body:expr $(,)?) => {{
        let kelson_qt_mark = $crate::QueryTap::mark();
        let kelson_qt_result = ($body).await;
        let kelson_qt_probes = $crate::QueryTap::take_since(kelson_qt_mark);
        let kelson_qt_max: usize = $max;
        assert!(
            kelson_qt_probes.len() <= kelson_qt_max,
            "query budget exceeded: {} probes recorded (budget <= {kelson_qt_max})",
            kelson_qt_probes.len()
        );
        kelson_qt_result
    }};
    (== $exact:expr, $body:expr $(,)?) => {{
        let kelson_qt_mark = $crate::QueryTap::mark();
        let kelson_qt_result = ($body).await;
        let kelson_qt_probes = $crate::QueryTap::take_since(kelson_qt_mark);
        let kelson_qt_exact: usize = $exact;
        assert_eq!(
            kelson_qt_probes.len(),
            kelson_qt_exact,
            "query count mismatch: {} probes recorded (expected exactly {kelson_qt_exact})",
            kelson_qt_probes.len()
        );
        kelson_qt_result
    }};
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::time::Duration;

    // The QueryTap is process-global and take_since drains a shared buffer, so
    // concurrent tests would steal each other's probes. Serialize the tap tests
    // (mirrors the production guidance: one attributed execution at a time).
    // The guard is intentionally held across the await — that is the point of
    // the serialization; the test future runs on one thread.
    static TAP_LOCK: Mutex<()> = Mutex::new(());

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn assert_queries_passes_within_budget() {
        let _guard = TAP_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let value = assert_queries!(<= 4, async {
            crate::QueryTap::record("TAPKIT-BUDGET", "select budget_probe", Duration::ZERO);
            41 + 1
        });
        assert_eq!(value, 42);
    }

    #[tokio::test]
    #[should_panic(expected = "query budget exceeded")]
    #[allow(clippy::await_holding_lock)]
    async fn assert_queries_fails_over_budget() {
        let _guard = TAP_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        assert_queries!(<= 0, async {
            crate::QueryTap::record("TAPKIT-OVER", "select budget_probe", Duration::ZERO);
        });
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn assert_queries_exact_match() {
        let _guard = TAP_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        assert_queries!(== 2, async {
            crate::QueryTap::record("TAPKIT-EXACT", "select a", Duration::ZERO);
            crate::QueryTap::record("TAPKIT-EXACT", "select b", Duration::ZERO);
        });
    }
}
