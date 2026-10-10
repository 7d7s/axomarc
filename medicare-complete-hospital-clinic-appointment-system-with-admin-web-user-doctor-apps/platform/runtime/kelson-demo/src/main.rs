//! kelson-api binary entrypoint: config -> tracing -> db probe -> host boot ->
//! graph compile -> edge wiring -> serve (see lib.rs::run).

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    kelson_demo::run().await
}
