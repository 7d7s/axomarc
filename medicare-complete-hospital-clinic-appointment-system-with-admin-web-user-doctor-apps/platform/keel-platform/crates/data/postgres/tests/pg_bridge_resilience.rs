//! MNC-030 bridge resilience (no database required): a panicking job on the
//! process-wide Postgres bridge must not permanently break database access.
//!
//! Historically the bridge ran every job inline on one thread with two
//! `.expect` sites, so a single panicking future unwound the bridge thread and
//! degraded every later call for the whole process. The bridge now isolates a
//! job's panic (as a `JoinError`) and rebuilds itself if it ever fails, so the
//! next call recovers. This test runs on a **current-thread** runtime so
//! `block_on` takes the bridge fallback path (the multi-threaded fast path
//! never touches the bridge).
#[tokio::test]
async fn a_panicking_job_does_not_kill_the_bridge() {
    // Job 1: a future that panics. `submit` has no value to return for a job
    // that never completes, so the caller panics too — isolate that to its own
    // thread so the panic surfaces there, not in this test.
    let worker = std::thread::spawn(|| {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            kelson_postgres::bridge::block_on(async { panic!("deliberate job panic") });
        }));
        assert!(outcome.is_err(), "the panicking job must fail its caller");
    });
    assert!(worker.join().is_ok(), "the isolated thread must not abort");

    // Job 2: the bridge must still be alive (or rebuilt) and complete a normal
    // job — the process did not lose its database path to one bad future.
    let value = kelson_postgres::bridge::block_on(async { 41 + 1 });
    assert_eq!(value, 42, "the bridge recovers after a panicking job");
}
