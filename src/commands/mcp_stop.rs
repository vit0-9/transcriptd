use anyhow::Result;

use crate::config;

pub fn cmd_mcp_stop() -> Result<()> {
    let pid_path = config::pid_file_path();
    if !pid_path.exists() {
        eprintln!("No PID file found. Is the service running?");
        return Ok(());
    }

    let pid_str = std::fs::read_to_string(&pid_path)?;
    let pid: u32 = match pid_str.trim().parse() {
        Ok(p) => p,
        Err(_) => {
            eprintln!("Invalid PID file contents. Removing it.");
            let _ = std::fs::remove_file(&pid_path);
            return Ok(());
        }
    };

    #[cfg(unix)]
    {
        println!("Stopping service (pid {pid})...");
        let res = unsafe { libc::kill(pid as i32, libc::SIGTERM) };
        if res == 0 {
            println!("Service stopped.");
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
        eprintln!("'mcp stop' is only supported on Unix systems currently.");
        eprintln!("Please kill PID {pid} manually.");
    }

    let _ = std::fs::remove_file(&pid_path);
    Ok(())
}
