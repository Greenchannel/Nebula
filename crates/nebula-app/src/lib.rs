//! Tauri v2 host: boots a [`NebulaNode`](nebula_core::NebulaNode) and points
//! the native webview at the node's loopback UI origin. Desktop, Android and a
//! plain browser therefore share exactly the same REST + WebSocket client.
//!
//! The window first loads the bundled `launcher.html`. It asks the Rust side
//! for the *actual* (possibly ephemeral) UI URL over Tauri IPC instead of
//! probing HTTP from the page. This avoids mixed-content/CORS restrictions on
//! Android WebView; the page then performs a top-level navigation to the node.

use anyhow::{anyhow, Result};
use nebula_core::{CliOptions, NebulaConfig, NebulaNode};
use serde::Serialize;
use std::sync::{Arc, Mutex};
use tauri::Manager;

type SetupResult = std::result::Result<(), Box<dyn std::error::Error>>;

fn io_err(message: impl Into<String>) -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::new(std::io::ErrorKind::Other, message.into()))
}

/// Snapshot surfaced to the launcher page over IPC.
#[derive(Debug, Clone, Serialize)]
pub struct StartupInfo {
    /// "starting" | "ready" | "error"
    pub state: String,
    pub url: Option<String>,
    pub port: u16,
    pub error: Option<String>,
}

impl Default for StartupInfo {
    fn default() -> Self {
        Self {
            state: "starting".to_string(),
            url: None,
            port: 0,
            error: None,
        }
    }
}

#[tauri::command]
fn nebula_info(state: tauri::State<'_, Arc<Mutex<StartupInfo>>>) -> StartupInfo {
    state
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_else(|_| StartupInfo {
            state: "error".to_string(),
            error: Some("startup state lock poisoned".to_string()),
            ..Default::default()
        })
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
    let startup = Arc::new(Mutex::new(StartupInfo::default()));
    let startup_for_setup = startup.clone();

    let builder = tauri::Builder::default()
        .manage(startup.clone())
        .invoke_handler(tauri::generate_handler![nebula_info])
        .setup(move |app| -> SetupResult {
            // On mobile there is no HOME/XDG; use the platform app-data dir.
            #[cfg(mobile)]
            let config = {
                let mut config = base_config.clone();
                if let Ok(dir) = app.path().app_data_dir() {
                    config.data_dir = dir.join("nebula");
                }
                // Keep a predictable fallback for the launcher's manual button;
                // IPC reports the real URL regardless.
                config.ui_port = 48610;
                config
            };
            #[cfg(not(mobile))]
            let config = base_config.clone();

            match tauri::async_runtime::block_on(NebulaNode::start(config)) {
                Ok(node) => {
                    let url = node.ui_base();
                    if let Ok(mut info) = startup_for_setup.lock() {
                        info.state = "ready".to_string();
                        info.url = Some(url.clone());
                        info.port = node.ui_port;
                        info.error = None;
                    }
                    app.manage(node);

                    if let Some(window) = app.get_webview_window("main") {
                        match url::Url::parse(&url) {
                            Ok(parsed) => {
                                if window.navigate(parsed).is_err() {
                                    let _ =
                                        window.eval(&format!("window.location.replace({url:?})"));
                                }
                            }
                            Err(_) => {
                                let _ = window.eval(&format!("window.location.replace({url:?})"));
                            }
                        }
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }
                Err(err) => {
                    // Keep the app alive so the launcher can display the error.
                    let message = format!("{err:#}");
                    if let Ok(mut info) = startup_for_setup.lock() {
                        info.state = "error".to_string();
                        info.error = Some(message);
                    }
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.show();
                    }
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
