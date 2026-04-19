use std::path::Path;

use anyhow::Result;

use crate::config;

// ---------------------------------------------------------------------------
// Generic daemon launcher (shared by service up / mcp up)
// ---------------------------------------------------------------------------

fn start_daemon(
    db_path: &Path,
    hidden_cmd: &str,
    pid_path: &std::path::Path,
    label: &str,
) -> Result<()> {
    let log_path = config::log_file_path();

    // Check if already running
    if pid_path.exists() {
        if let Ok(pid_str) = std::fs::read_to_string(pid_path) {
            if let Ok(pid) = pid_str.trim().parse::<u32>() {
                #[cfg(unix)]
                {
                    let alive = unsafe { libc::kill(pid as i32, 0) } == 0;
                    if alive {
                        eprintln!("{label} already running (pid {pid})");
                        return Ok(());
                    }
                }
            }
        }
        // Stale PID file
        let _ = std::fs::remove_file(pid_path);
    }

    // Ensure log directory exists
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    let err_file = log_file.try_clone()?;

    let exe = std::env::current_exe()?;
    let child = std::process::Command::new(exe)
        .arg("--db")
        .arg(db_path.as_os_str())
        .arg(hidden_cmd)
        .stdout(log_file)
        .stderr(err_file)
        .stdin(std::process::Stdio::null())
        .spawn()?;

    let pid = child.id();
    std::fs::write(pid_path, pid.to_string())?;

    println!("{label} started (pid {pid})");
    println!("  Logs: transcriptd service logs -f");
    Ok(())
}

fn stop_daemon(pid_path: &std::path::Path, label: &str) -> Result<()> {
    if !pid_path.exists() {
        eprintln!("No PID file found. Is {label} running?");
        return Ok(());
    }

    let pid_str = std::fs::read_to_string(pid_path)?;
    let pid: u32 = match pid_str.trim().parse() {
        Ok(p) => p,
        Err(_) => {
            eprintln!("Invalid PID file. Removing it.");
            let _ = std::fs::remove_file(pid_path);
            return Ok(());
        }
    };

    #[cfg(unix)]
    {
        println!("Stopping {label} (pid {pid})...");
        let res = unsafe { libc::kill(pid as i32, libc::SIGTERM) };
        if res == 0 {
            println!("{label} stopped.");
        } else {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::ESRCH) {
                println!("Process {pid} was already dead.");
            } else {
                eprintln!("Failed to stop process {pid}: {err}");
            }
        }
    }
    #[cfg(not(unix))]
    {
        eprintln!("Stop is only supported on Unix. Please kill PID {pid} manually.");
    }

    let _ = std::fs::remove_file(pid_path);
    Ok(())
}

// ---------------------------------------------------------------------------
// Service (watcher daemon) up/down
// ---------------------------------------------------------------------------

pub fn cmd_service_up(db_path: &Path) -> Result<()> {
    let pid_path = config::pid_file_path();
    start_daemon(db_path, "__run-service", &pid_path, "Service")
}

pub fn cmd_service_down() -> Result<()> {
    let pid_path = config::pid_file_path();
    stop_daemon(&pid_path, "Service")
}

// ---------------------------------------------------------------------------
// MCP HTTP daemon up/down
// ---------------------------------------------------------------------------

pub fn cmd_mcp_up(db_path: &Path) -> Result<()> {
    let pid_path = config::mcp_pid_file_path();
    let cfg = config::load()?;
    start_daemon(db_path, "__run-mcp", &pid_path, "MCP HTTP")?;
    println!("  MCP:  http://127.0.0.1:{}", cfg.mcp_port);
    Ok(())
}

pub fn cmd_mcp_down() -> Result<()> {
    let pid_path = config::mcp_pid_file_path();
    stop_daemon(&pid_path, "MCP HTTP")
}
