#[tokio::test]
async fn golden_path_holds_all_invariants() {
    let report = keel_example_minimal::run()
        .await
        .expect("golden path must pass");
    assert_eq!(report.stones_written, 1);
    assert!(report.replayed);
    assert_eq!(report.denied_code, keel_foundation::codes::DENIED);
}
