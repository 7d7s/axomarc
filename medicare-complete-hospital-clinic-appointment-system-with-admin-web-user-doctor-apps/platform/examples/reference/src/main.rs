//! Run the reference application and print the report.

#[tokio::main]
async fn main() {
    match keel_example_reference::run().await {
        Ok(report) => {
            println!("reference application OK:");
            println!("  graph operations     : {}", report.graph_operations);
            println!("  graph routes         : {}", report.graph_routes);
            println!("  graph events         : {}", report.graph_events);
            println!("  booked appointment   : {}", report.booked_id);
            println!("  delta stones written : {}", report.delta_stones);
            println!("  outbox events queued : {}", report.outbox_queued);
            println!("  listed appointments  : {}", report.listed);
            println!("  idempotent replay    : {}", report.replayed);
            println!("  stranger denied with : {}", report.denied_code);
            println!("  search hits          : {}", report.search_hits);
            println!("  MCP tools generated  : {}", report.mcp_tools);
        }
        Err(e) => {
            eprintln!("reference application FAILED: {e}");
            std::process::exit(1);
        }
    }
}
