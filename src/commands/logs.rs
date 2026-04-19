use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::config;

pub fn cmd_logs(follow: bool, lines: usize) -> Result<()> {
    let log_path = config::log_file_path();
    if !log_path.exists() {
        bail!(
            "No log file found at {}\nIs the service running? Start it with: transcriptd service up",
            log_path.display()
        );
    }

    if follow {
        tail_follow(&log_path, lines)?;
    } else {
        tail_lines(&log_path, lines)?;
    }

    Ok(())
}

/// Print the last N lines of a file.
fn tail_lines(path: &std::path::Path, n: usize) -> Result<()> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let all_lines: Vec<&str> = content.lines().collect();
    let start = all_lines.len().saturating_sub(n);
    for line in &all_lines[start..] {
        println!("{line}");
    }
    Ok(())
}

/// Tail -f: print last N lines, then follow new output.
fn tail_follow(path: &std::path::Path, initial_lines: usize) -> Result<()> {
    // Print initial lines
    tail_lines(path, initial_lines)?;

    // Now follow
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut reader = BufReader::new(file);
    reader.seek(SeekFrom::End(0))?;

    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => {
                // No new data — poll
                thread::sleep(Duration::from_millis(200));
            }
            Ok(_) => {
                print!("{line}");
            }
            Err(e) => {
                eprintln!("read error: {e}");
                break;
            }
        }
    }

    Ok(())
}
