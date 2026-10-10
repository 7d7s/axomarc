//! Golden invariants of the reference application (S6).

#[tokio::test]
async fn reference_application_holds_all_invariants() {
    let report = kelson_example_reference::run()
        .await
        .expect("reference application must pass");

    // The module manifest compiles into a platform graph.
    assert_eq!(report.graph_operations, 2, "book + list operations");
    assert_eq!(
        report.graph_routes, 2,
        "POST /api/v1/appointments + GET /api/v1/appointments/{{id}}"
    );
    assert_eq!(report.graph_events, 1, "appointment.booked");

    // Execution through the engine: exactly one delta stone, one outbox event.
    assert!(!report.booked_id.is_empty());
    assert_eq!(
        report.delta_stones, 1,
        "booking writes exactly one delta stone"
    );
    assert_eq!(
        report.outbox_queued, 1,
        "booking queues exactly one outbox event"
    );

    // Read path, idempotency, and authorization.
    assert_eq!(report.listed, 1, "the booking is visible to the read path");
    assert!(report.replayed, "same key + same payload replays");
    assert_eq!(
        report.denied_code,
        kelson_foundation::codes::DENIED,
        "stranger is denied"
    );

    // Search is tenant-scoped and sees the booking.
    assert_eq!(
        report.search_hits, 1,
        "tenant-scoped search finds the booking"
    );

    // The ecosystem surface is generated from the same contracts.
    assert_eq!(report.mcp_tools, 2, "one MCP tool per registered operation");
}

#[test]
fn manifest_rejects_tampering() {
    // Duplicate operations across modules must not compile (G05).
    let manifest = kelson_registry::ModuleManifest::from_toml(kelson_example_reference::MANIFEST)
        .expect("manifest parses");
    let doubled = kelson_registry::PlatformGraph::compile(vec![manifest.clone(), manifest]);
    assert!(doubled.is_err(), "duplicate module must violate the graph");
}
