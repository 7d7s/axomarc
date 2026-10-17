//! kelson-demo binary entrypoint: config -> tracing -> db probe -> host boot ->
//! graph compile -> edge wiring -> serve (see lib.rs::run).

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    demo_main().await
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
