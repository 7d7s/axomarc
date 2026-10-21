//! kelson-demo binary entrypoint: config -> tracing -> db probe -> host boot ->
//! graph compile -> edge wiring -> serve (see lib.rs::run).

/// The stack the boot runs on.
///
/// The boot future is large (the whole host: the graph, the routes, the
/// telemetry, the WASM plugin loading) and `#[tokio::main]` polls it on the
/// **main thread**, whose stack the Windows loader sizes at 1 MiB by default
/// (8 MiB on Linux). The boot overflowed that stack and the process aborted
/// with `thread 'main' has overflowed its stack` before it could bind its
/// listener - the audit's KFA-005.
///
/// The fix is the standard one: the boot runs on a thread with an explicit
/// stack, so the host starts on every platform regardless of the loader's
/// default. The runtime is still multi-threaded (the workers keep their own
/// stacks); only the thread that *polls* the boot future is resized.
const BOOT_STACK_BYTES: usize = 16 * 1024 * 1024;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let boot = std::thread::Builder::new()
        .name("kelson-demo-boot".into())
        .stack_size(BOOT_STACK_BYTES)
        .spawn(|| {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map_err(|e| e.to_string())?;
            // The error type is not `Send`, so it becomes a message before it
            // crosses the thread boundary (the detail is in the logs).
            runtime.block_on(demo_main()).map_err(|e| e.to_string())
        })?;
    // The inner result is the boot's own; the outer is the thread's.
    match boot.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(message)) => Err(message.into()),
        Err(panic) => Err(format!("the boot thread panicked: {panic:?}").into()),
    }
}

/// Boot the demo host.
///
/// Stage C.2 removed the `demo` feature: the application modules' handlers
/// are no longer compiled into this binary at all. They cross the ADR 0020
/// seam as WASM artifacts loaded at boot from `packages.toml`, so a kernel
/// build cannot link their behaviour even by accident — which is the whole
/// point, and also why there is no longer a "built without the feature"
/// variant of this entrypoint to explain.
async fn demo_main() -> Result<(), Box<dyn std::error::Error>> {
    kelson_demo::run().await
}
