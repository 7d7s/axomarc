//! kelson-demo binary entrypoint: config -> tracing -> db probe -> host boot ->
//! graph compile -> edge wiring -> serve (see lib.rs::run).

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    demo_main().await
}

/// Phase P made the demo module providers opt-in (`--features demo`, default
/// off) so this harness could be published without pulling unpublished crates
/// into the dependency graph. Every local command in `kelson` — `serve`,
/// `call`, `shell`, `model:prune` — passes the feature, so this is the normal
/// path.
#[cfg(feature = "demo")]
async fn demo_main() -> Result<(), Box<dyn std::error::Error>> {
    kelson_demo::run().await
}

/// A non-demo build cannot boot, and it is worth saying why rather than
/// letting it fall through to a boot-gate violation that names none of it.
///
/// `discover_manifests` scans `applications/*/module.toml` off disk
/// unconditionally, so the appointments graph is present even when the
/// providers that register its route-binding resolvers are compiled out —
/// the boot then dies at
/// `GRAPH-BINDING-RESOLVER: module appointments: route DELETE
/// /api/v1/appointments/{id} binds model Appointment but no resolver is
/// registered`. The feature gates the code, not the data. Fail fast instead.
#[cfg(not(feature = "demo"))]
async fn demo_main() -> Result<(), Box<dyn std::error::Error>> {
    eprintln!(
        "kelson-demo was built without `--features demo`, so the demo module\n\
         providers are absent while `applications/*/module.toml` is still\n\
         discovered from disk. The platform graph therefore cannot resolve its\n\
         route bindings, and this harness cannot boot.\n\
         \n\
         Re-run with the feature:\n\
         \n\
             cargo run -p kelson-demo --features demo -- <args>\n\
         \n\
         `kelson serve`, `kelson call`, `kelson shell` and `kelson model:prune`\n\
         already do this for you."
    );
    Err("kelson-demo requires --features demo".into())
}
