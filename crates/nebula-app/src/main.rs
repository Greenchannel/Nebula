//! `nebula` — GUI by default, terminal UI with `--tui`.
//!
//! On Windows release builds the process is linked as a *windowed* subsystem
//! (no console flash when double-clicking). When `--tui` is requested we
//! re-attach to the parent terminal — or allocate a fresh one — before the TUI
//! starts, so PowerShell/CMD users still get a real interactive terminal.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use anyhow::{anyhow, Result};
use nebula_core::{parse_cli, CliOptions, NebulaNode, CLI_HELP};

fn main() {
    if let Err(err) = real_main() {
        eprintln!("nebula: {err:#}");
        std::process::exit(1);
    }
}

fn real_main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let options = match parse_cli(args) {
        Ok(options) => options,
        Err(err) if err.to_string() == "HELP" => {
            println!("{CLI_HELP}");
            return Ok(());
        }
        Err(err) => {
            eprintln!("nebula: {err}\n\n{CLI_HELP}");
            std::process::exit(2);
        }
    };

    if options.force_tui && options.force_gui {
        return Err(anyhow!("--gui and --tui are mutually exclusive"));
    }

    if options.force_tui {
        run_tui_mode(options)
    } else {
        nebula_app::run_gui(options)
    }
}

fn run_tui_mode(options: CliOptions) -> Result<()> {
    #[cfg(windows)]
    attach_parent_console();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let node = NebulaNode::start(options.config).await?;
        nebula_tui::run_tui(node).await
    })
}

/// Pitfall #4: `AttachConsole(ATTACH_PARENT_PROCESS)` re-uses the launching
/// terminal; if there is none (Explorer double-click), allocate a new one.
#[cfg(windows)]
fn attach_parent_console() {
    use windows_sys::Win32::System::Console::{AllocConsole, AttachConsole, ATTACH_PARENT_PROCESS};
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            let _ = AllocConsole();
        }
    }
}
