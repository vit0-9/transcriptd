use anyhow::{Context, Result};
use clap::Parser;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use transcriptd_claude::*;
use walkdir::WalkDir;

#[derive(Parser)]
#[command(
    name = "transcriptd-claude",
    about = "Extract Claude Code sessions to markdown"
)]
struct Cli {
    #[arg(short, long)]
    dir: Option<String>,
    #[arg(short, long, default_value = "./out")]
    output: PathBuf,
    #[arg(long)]
    since: Option<String>,
    #[arg(long)]
    id: Option<String>,
    #[arg(long)]
    dry_run: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let claude_dir = cli.dir.unwrap_or_else(|| {
        default_claude_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| {
                let home = std::env::var("HOME").unwrap_or_default();
                format!("{home}/.claude/projects")
            })
    });

    if let Some(ref session_id) = cli.id {
        // Find matching file
        let mut found = None;
        for entry in WalkDir::new(&claude_dir).min_depth(2).max_depth(2) {
            let entry = entry?;
            let p = entry.path();
            if p.file_stem()
                .is_some_and(|s| s.to_string_lossy() == session_id.as_str())
                && p.extension().is_some_and(|e| e == "jsonl")
            {
                found = Some(p.to_string_lossy().to_string());
                break;
            }
        }
        let jsonl_path = found.with_context(|| format!("session {session_id} not found"))?;

        if cli.dry_run {
            let s = summarize_one(&jsonl_path)?;
            println!(
                "{} | {} | u={} a={} | tools={:?}",
                s.id, s.title, s.user_turns, s.agent_turns, s.tools
            );
            return Ok(());
        }

        let md = extract_one(&jsonl_path)?;
        fs::create_dir_all(&cli.output)?;
        let path = cli.output.join(format!("claude-{session_id}.md"));
        let mut f = fs::File::create(&path)?;
        f.write_all(md.as_bytes())?;
        eprintln!("wrote {}", path.display());
        return Ok(());
    }

    // Full extraction
    let total = count_sessions(&claude_dir)?;
    eprintln!("found {total} session files in {claude_dir}");

    if cli.dry_run {
        let mut success = 0u32;
        let mut errors = 0u32;
        for entry in WalkDir::new(&claude_dir).min_depth(2).max_depth(2) {
            let entry = entry?;
            let p = entry.path();
            if p.extension().is_none_or(|e| e != "jsonl") {
                continue;
            }
            let jsonl_str = p.to_string_lossy().to_string();
            match summarize_one(&jsonl_str) {
                Ok(s) => {
                    println!(
                        "{} | {} | u={} a={} | tools={:?} | {}..{}",
                        s.id,
                        s.title,
                        s.user_turns,
                        s.agent_turns,
                        s.tools,
                        s.first_ts.as_deref().unwrap_or("?"),
                        s.last_ts.as_deref().unwrap_or("?")
                    );
                    success += 1;
                }
                Err(e) => {
                    eprintln!("SKIP {}: {e}", p.display());
                    errors += 1;
                }
            }
        }
        eprintln!("\n=== transcriptd-claude (dry-run) ===");
        eprintln!("decoded: {success}  errors: {errors}");
        return Ok(());
    }

    let pairs = extract_all(&claude_dir, cli.since.as_deref())?;
    fs::create_dir_all(&cli.output)?;

    for (filename, md) in &pairs {
        let path = cli.output.join(filename);
        let mut f = fs::File::create(&path)?;
        f.write_all(md.as_bytes())?;
    }

    eprintln!("\n=== transcriptd-claude ===");
    eprintln!("extracted: {} sessions", pairs.len());
    eprintln!("output:    {}", cli.output.display());
    Ok(())
}
