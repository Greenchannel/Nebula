//! Headless Nebula node used for local testing and browser-based UI work.
//!
//! ```text
//! cargo run -p nebula-core --example nebula-node -- --data-dir D:/tmp/n1 --ui-port 48610
//! ```

use nebula_core::{parse_cli, NebulaNode, CLI_HELP};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("nebula_core=info")),
        )
        .try_init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let opts = match parse_cli(args) {
        Ok(opts) => opts,
        Err(err) if err.to_string() == "HELP" => {
            println!("{CLI_HELP}");
            return Ok(());
        }
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(2);
        }
    };

    let node = NebulaNode::start(opts.config).await?;
    println!("==========================================================");
    println!(" Nebula node online");
    println!("   nickname : {}", node.identity().nickname);
    println!("   peer id  : {}", node.peer_id());
    println!("   web ui   : {}", node.ui_base());
    println!("   p2p port : {}", node.p2p_port);
    println!("   data dir : {}", node.cfg.data_dir.display());
    println!("==========================================================");
    println!("Press Ctrl+C to stop and broadcast LEAVE.");

    let _ = tokio::signal::ctrl_c().await;
    node.shutdown().await;
    Ok(())
}
