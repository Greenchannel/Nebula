//! Tauri v2 host: boots a [`NebulaNode`](nebula_core::NebulaNode) and points
//! the native webview at the node's loopback UI origin. Desktop, Android and a
//! plain browser therefore share exactly the same REST + WebSocket client.

use anyhow::{anyhow, Result};
use nebula_core::{CliOptions, NebulaConfig, NebulaNode};
use std::sync::Arc;
use tauri::Manager;

type SetupResult = std::result::Result<(), Box<dyn std::error::Error>>;

fn io_err(message: impl Into<String>) -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::new(std::io::ErrorKind::Other, message.into()))
}

/// Mobile entry point (Android/iOS). Tauri calls this from the generated
/// activity after it has loaded the native runtime.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let options = CliOptions {
        config: NebulaConfig::default(),
        force_tui: false,
        force_gui: true,
    };
    if let Err(err) = run_gui(options) {
        eprintln!("nebula: {err:#}");
        std::process::exit(1);
    }
}

/// Desktop / mobile GUI boot.
pub fn run_gui(options: CliOptions) -> Result<()> {
    let base_config = options.config;

    let builder = tauri::Builder::default().setup(move |app| -> SetupResult {
        // On mobile there is no HOME/XDG; use the platform app-data directory.
        // The UI port is pinned so the bundled launcher.html (which cannot know
        // a dynamic port) can find the node even if WebView navigation is
        // unreliable on that platform.
        #[cfg(mobile)]
        let config = {
            let mut config = base_config.clone();
            if let Ok(dir) = app.path().app_data_dir() {
                config.data_dir = dir.join("nebula");
            }
            config.ui_port = 48610;
            config
        };
        #[cfg(not(mobile))]
        let config = base_config.clone();

        let node = tauri::async_runtime::block_on(NebulaNode::start(config))
            .map_err(|err| io_err(format!("failed to start Nebula node: {err:#}")))?;
        let ui_url: url::Url = node
            .ui_base()
            .parse()
            .map_err(|err| io_err(format!("invalid UI url: {err}")))?;
        app.manage(node.clone());

        // The config-defined window exists already (created before `setup`)
        // but is hidden on the bundled launcher page. Point it at the *actual*
        // ephemeral port and show. Navigation issues are non-fatal: the
        // launcher page polls the fixed mobile port as a fallback.
        match app.get_webview_window("main") {
            Some(window) => {
                if window.navigate(ui_url.clone()).is_err() {
                    let script = format!("window.location.replace({:?})", node.ui_base());
                    let _ = window.eval(&script);
                }
                let _ = window.show();
                let _ = window.set_focus();
            }
            None => {
                tauri::WebviewWindowBuilder::new(
                    app,
                    "main",
                    tauri::WebviewUrl::External(ui_url),
                )
                .title("Nebula · 星云")
                .inner_size(1380.0, 900.0)
                .min_inner_size(390.0, 620.0)
                .build()
                .map_err(|err| io_err(format!("window creation failed: {err}")))?;
            }
        }
        Ok(())
    });

    let app = builder
        .build(tauri::generate_context!())
        .map_err(|err| anyhow!("tauri application failed to build: {err}"))?;

    app.run(|app_handle, event| {
        if let tauri::RunEvent::Exit = event {
            if let Some(state) = app_handle.try_state::<Arc<NebulaNode>>() {
                let node = state.inner().clone();
                tauri::async_runtime::block_on(node.shutdown());
            }
        }
    });
    Ok(())
}
