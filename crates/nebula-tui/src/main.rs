//! Standalone Nebula terminal client.
//!
//! ```text
//! nebula-tui --data-dir D:/nebula-data --name Nova
//! ```

use anyhow::Result;
use nebula_core::{parse_cli, NebulaNode, CLI_HELP};

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let options = match parse_cli(args) {
        Ok(options) => options,
        Err(err) if err.to_string() == "HELP" => {
            println!("{CLI_HELP}");
            return Ok(());
        }
        Err(err) => {
            eprintln!("nebula-tui: {err}\n\n{CLI_HELP}");
            std::process::exit(2);
        }
    };

    let node = NebulaNode::start(options.config).await?;
    nebula_tui::run_tui(node).await
}
