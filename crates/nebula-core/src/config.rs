//! CLI / environment configuration for a Nebula node.

use anyhow::{anyhow, Result};
use std::path::PathBuf;
use std::time::Duration;

/// Everything a node needs to boot. All fields have sensible defaults so that
/// `NebulaConfig::default()` yields a fully functional zero-config node.
#[derive(Clone, Debug)]
pub struct NebulaConfig {
    pub data_dir: PathBuf,
    /// Port of the P2P HTTP/WebSocket server (0 = OS assigned).
    pub p2p_port: u16,
    /// Port of the loopback-only UI server (0 = OS assigned).
    pub ui_port: u16,
    /// Optional nickname used only when the identity is created for the first time.
    pub nickname: Option<String>,
    pub bio: String,
    pub discovery_base_port: u16,
    pub discovery_slots: u16,
    pub heartbeat: Duration,
    pub offline_timeout: Duration,
    pub sweep_interval: Duration,
    pub sync_interval: Duration,
    pub blob_fetch_interval: Duration,
}

impl Default for NebulaConfig {
    fn default() -> Self {
        Self {
            data_dir: default_data_dir(),
            p2p_port: 0,
            ui_port: 0,
            nickname: None,
            bio: "Exploring the Nebula LAN...".to_string(),
            discovery_base_port: 48600,
            discovery_slots: 10,
            heartbeat: Duration::from_secs(2),
            offline_timeout: Duration::from_secs(8),
            sweep_interval: Duration::from_secs(3),
            sync_interval: Duration::from_secs(2),
            blob_fetch_interval: Duration::from_secs(1),
        }
    }
}

impl NebulaConfig {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            ..Self::default()
        }
    }

    /// Apply environment overrides (`NEBULA_DATA_DIR`, `NEBULA_P2P_PORT`, ...).
    pub fn with_env(mut self) -> Self {
        if let Ok(dir) = std::env::var("NEBULA_DATA_DIR") {
            if !dir.trim().is_empty() {
                self.data_dir = PathBuf::from(dir);
            }
        }
        if let Ok(port) = std::env::var("NEBULA_P2P_PORT") {
            if let Ok(port) = port.parse() {
                self.p2p_port = port;
            }
        }
        if let Ok(port) = std::env::var("NEBULA_UI_PORT") {
            if let Ok(port) = port.parse() {
                self.ui_port = port;
            }
        }
        if let Ok(name) = std::env::var("NEBULA_NAME") {
            if !name.trim().is_empty() {
                self.nickname = Some(name.trim().to_string());
            }
        }
        self
    }
}

/// Resolve the default per-user data directory.
pub fn default_data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("NEBULA_DATA_DIR") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir);
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            return PathBuf::from(appdata).join("Nebula");
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
            if !xdg.trim().is_empty() {
                return PathBuf::from(xdg).join("nebula");
            }
        }
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(".local/share/nebula");
        }
    }
    PathBuf::from("nebula-data")
}

/// Parsed command line. Kept in the core crate so the TUI binary and the
/// Tauri host understand exactly the same flags.
#[derive(Clone, Debug)]
pub struct CliOptions {
    pub config: NebulaConfig,
    pub force_tui: bool,
    pub force_gui: bool,
}

pub const CLI_HELP: &str = "\
Nebula (星云) — zero-server LAN P2P messenger

USAGE:
    nebula [--gui | --tui] [OPTIONS]

OPTIONS:
    --data-dir <PATH>   Data directory (identity, nebula.db, blobs/)
    --p2p-port <PORT>   P2P HTTP/WS port (0 = auto)
    --ui-port <PORT>    Loopback UI port (0 = auto)
    --name <NAME>       Nickname used when creating a new identity
    --bio <TEXT>        Bio used when creating a new identity
    --gui               Force GUI mode (default on desktop)
    --tui               Force terminal UI mode
    -h, --help          Show this help
";

/// Parse CLI arguments (without the program name) into a [`CliOptions`].
pub fn parse_cli<I, S>(args: I) -> Result<CliOptions>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut config = NebulaConfig::default();
    let mut force_tui = false;
    let mut force_gui = false;
    let mut iter = args.into_iter().map(Into::into);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--gui" => force_gui = true,
            "--tui" => force_tui = true,
            "-h" | "--help" => {
                return Err(anyhow!("HELP"));
            }
            "--data-dir" => {
                let value = iter
                    .next()
                    .ok_or_else(|| anyhow!("--data-dir requires a value"))?;
                config.data_dir = PathBuf::from(value);
            }
            "--p2p-port" => {
                let value = iter
                    .next()
                    .ok_or_else(|| anyhow!("--p2p-port requires a value"))?;
                config.p2p_port = value
                    .parse()
                    .map_err(|_| anyhow!("invalid --p2p-port: {value}"))?;
            }
            "--ui-port" => {
                let value = iter
                    .next()
                    .ok_or_else(|| anyhow!("--ui-port requires a value"))?;
                config.ui_port = value
                    .parse()
                    .map_err(|_| anyhow!("invalid --ui-port: {value}"))?;
            }
            "--name" => {
                config.nickname = Some(
                    iter.next()
                        .ok_or_else(|| anyhow!("--name requires a value"))?,
                );
            }
            "--bio" => {
                config.bio = iter
                    .next()
                    .ok_or_else(|| anyhow!("--bio requires a value"))?;
            }
            other => return Err(anyhow!("unknown argument: {other}")),
        }
    }
    Ok(CliOptions {
        config,
        force_tui,
        force_gui,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_flags() {
        let opts = parse_cli([
            "--tui",
            "--data-dir",
            "C:/tmp/nebula",
            "--p2p-port",
            "52101",
            "--ui-port",
            "48610",
            "--name",
            "Nova",
        ])
        .unwrap();
        assert!(opts.force_tui);
        assert_eq!(opts.config.p2p_port, 52101);
        assert_eq!(opts.config.ui_port, 48610);
        assert_eq!(opts.config.nickname.as_deref(), Some("Nova"));
        assert_eq!(opts.config.data_dir, PathBuf::from("C:/tmp/nebula"));
    }

    #[test]
    fn rejects_unknown_flag() {
        assert!(parse_cli(["--nope"]).is_err());
    }
}
