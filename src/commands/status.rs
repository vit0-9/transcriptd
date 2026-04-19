use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use rusqlite::Connection;
use transcriptd_store::get_stats;

use crate::config;
use crate::format::format_tokens;

pub fn cmd_status(conn: &Connection) -> Result<()> {
    let cfg = config::load()?;
    let version = env!("CARGO_PKG_VERSION");

    println!("transcriptd v{version}");
    println!();

    // ── Daemon status ──
    let daemon_running = check_daemon_socket(&cfg.socket_path);
    let http_healthy = check_http_health(cfg.mcp_port);

    if daemon_running && http_healthy {
        println!("Service:    \x1b[32m● running\x1b[0m");
    } else if daemon_running {
        println!("Service:    \x1b[33m● degraded\x1b[0m (socket ok, http down)");
    } else {
        println!("Service:    \x1b[31m● stopped\x1b[0m");
    }

    println!("MCP HTTP:   http://127.0.0.1:{}", cfg.mcp_port);
    println!();

    if !daemon_running {
        println!("Hint: start the service with `transcriptd service up`");
        println!();
    }

    // ── Database stats ──
    let stats = get_stats(conn)?;
    println!("Database:");
    println!("  Transcripts: {}", stats.total_transcripts);
    println!("  Turns:       {}", stats.total_turns);
    println!(
        "  Tokens:      {} in / {} out",
        format_tokens(stats.total_tokens_in),
        format_tokens(stats.total_tokens_out)
    );

    if !stats.sources.is_empty() {
        println!("  Sources:");
        for (src, cnt) in &stats.sources {
            println!("    {src}: {cnt}");
        }
    }

    Ok(())
}

/// Try connecting to the daemon's Unix socket and sending a ping
fn check_daemon_socket(socket_path: &str) -> bool {
    let path = Path::new(socket_path);
    if !path.exists() {
        return false;
    }

    // Try connecting with a short timeout
    let stream = UnixStream::connect(path);
    match stream {
        Ok(mut s) => {
            let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
            let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
            // The daemon should accept the connection — that's enough to confirm it's alive
            // Send a newline to trigger a response cycle
            let _ = s.write_all(b"\n");
            let mut buf = [0u8; 1];
            // Even if we don't get data back, a successful connect means the daemon is listening
            let _ = s.read(&mut buf);
            true
        }
        Err(_) => false,
    }
}

/// Try hitting the HTTP health endpoint
fn check_http_health(port: u16) -> bool {
    use std::net::TcpStream;

    let addr = format!("127.0.0.1:{port}");
    match TcpStream::connect_timeout(&addr.parse().unwrap(), Duration::from_secs(2)) {
        Ok(mut stream) => {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
            let request = format!(
                "GET /health HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
            );
            if stream.write_all(request.as_bytes()).is_err() {
                return false;
            }
            let mut response = String::new();
            let _ = stream.read_to_string(&mut response);
            response.contains("200")
        }
        Err(_) => false,
    }
}
