//! Run the golden path and print the receipt.

#[tokio::main]
async fn main() {
    match kelson_example_minimal::run().await {
        Ok(report) => {
            println!("golden path OK:");
            println!("  delta stones written : {}", report.stones_written);
            println!("  idempotent replay    : {}", report.replayed);
            println!("  stranger denied with : {}", report.denied_code);
        }
        Err(e) => {
            eprintln!("golden path FAILED: {e}");
            std::process::exit(1);
        }
    }
}
