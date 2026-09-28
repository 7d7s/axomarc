//! xtask — dev tooling (10-GUIDELINES §8). Commands:
//!   scaffold-module <name>  → print module template steps (full codegen lands M0-07)
//!   openapi                 → emit OpenAPI from RouteRegistry (D-38: zero-dep; M8-06)
fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("scaffold-module") => match args.next() {
            Some(name) => {
                println!("scaffold for modules/{name} (template, M0-07):");
                for f in [
                    "Cargo.toml",
                    "src/lib.rs (Module + HttpModule impl)",
                    "src/routes.rs",
                    "src/http/{controllers,requests,resources}.rs",
                    "src/services/",
                    "src/models/",
                    "src/repositories/",
                    "src/events.rs",
                    "src/policies.rs",
                    "migrations/NNNN_<name>.sql",
                    "tests/contract.rs",
                ] {
                    println!("  - modules/{name}/{f}");
                }
                println!("then register in apps/api/src/modules.rs");
            }
            None => eprintln!("usage: xtask scaffold-module <name>"),
        },
        Some("openapi") => {
            println!("OpenAPI emission lands in M8-06 (RouteSpec registry → YAML, D-38)");
        }
        _ => {
            println!("medicare-kernel xtask");
            println!("  scaffold-module <name>");
            println!("  openapi");
        }
    }
}