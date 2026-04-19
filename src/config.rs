use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

fn data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("transcriptd")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub api_key: String,
    #[serde(skip)]
    pub socket_path: String,
    pub mcp_port: u16,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            api_key: generate_api_key(),
            socket_path: default_socket_path(),
            mcp_port: 3100,
        }
    }
}

fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("transcriptd")
        .join("config.json")
}

fn default_socket_path() -> String {
    if let Ok(runtime) = std::env::var("XDG_RUNTIME_DIR") {
        return format!("{runtime}/transcriptd/transcriptd.sock");
    }
    if cfg!(target_os = "macos")
        && let Some(support) = dirs::data_dir()
    {
        return support
            .join("transcriptd")
            .join("transcriptd.sock")
            .to_string_lossy()
            .to_string();
    }
    let home = dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
        .join(".transcriptd");
    format!("{}/transcriptd.sock", home.display())
}

pub fn log_file_path() -> PathBuf {
    data_dir().join("transcriptd.log")
}

pub fn pid_file_path() -> PathBuf {
    data_dir().join("transcriptd.pid")
}

pub fn mcp_pid_file_path() -> PathBuf {
    data_dir().join("mcp.pid")
}

pub fn generate_api_key() -> String {
    let mut buf = [0u8; 24];
    if getrandom::getrandom(&mut buf).is_err() {
        // fallback: use timestamp-based entropy
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        buf[..16].copy_from_slice(&t.to_le_bytes());
        buf[16..].copy_from_slice(&(t.wrapping_mul(6364136223846793005)).to_le_bytes()[..8]);
    }
    let hex: String = buf.iter().map(|b| format!("{:02x}", b)).collect();
    format!("td_{hex}")
}

/// Load config. Creates default if missing. Env vars override.
pub fn load() -> Result<Config> {
    let path = config_path();
    let mut cfg = if path.exists() {
        let data = std::fs::read_to_string(&path)?;
        serde_json::from_str(&data).unwrap_or_default()
    } else {
        let cfg = Config::default();
        save(&cfg)?;
        cfg
    };

    // Always compute socket_path dynamically (never persisted)
    cfg.socket_path = default_socket_path();

    // Env overrides
    if let Ok(key) = std::env::var("TRANSCRIPTD_API_KEY") {
        cfg.api_key = key;
    }
    if let Ok(sock) = std::env::var("TRANSCRIPTD_SOCKET") {
        cfg.socket_path = sock;
    }
    if let Ok(port) = std::env::var("TRANSCRIPTD_MCP_PORT")
        && let Ok(p) = port.parse()
    {
        cfg.mcp_port = p;
    }

    Ok(cfg)
}

/// Save config to disk.
pub fn save(cfg: &Config) -> Result<()> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_string_pretty(cfg)?;
    std::fs::write(&path, data)?;
    Ok(())
}
