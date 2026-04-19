use anyhow::{Context, Result};
use clap::Parser;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use transcriptd_zed::*;

#[derive(Parser)]
#[command(name = "transcriptd-zed", about = "Extract Zed AI threads to markdown")]
struct Cli {
    #[arg(short, long, default_value_t = default_db_string())]
    db: String,
    #[arg(short, long, default_value = "./out")]
    output: PathBuf,
    #[arg(long)]
    since: Option<String>,
    #[arg(long)]
    id: Option<String>,
    #[arg(long)]
    dry_run: bool,
}

fn default_db_string() -> String {
    default_threads_db()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_default();
            format!("{home}/Library/Application Support/Zed/threads/threads.db")
        })
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    if let Some(ref thread_id) = cli.id {
        if cli.dry_run {
            println!("Would extract thread {thread_id}");
            return Ok(());
        }
        let md = extract_one(&cli.db, thread_id)?;
        fs::create_dir_all(&cli.output)?;
        let path = cli.output.join(format!("zed-{thread_id}.md"));
        let mut f = fs::File::create(&path)?;
        f.write_all(md.as_bytes())?;
        eprintln!("wrote {}", path.display());
        return Ok(());
    }

    // Full extraction — preserve original dry_run / file-write logic
    let conn = rusqlite::Connection::open(&cli.db)
        .with_context(|| format!("Failed to open {}", cli.db))?;

    let mut query = String::from(
        "SELECT id, summary, updated_at, created_at, data, folder_paths, worktree_branch FROM threads",
    );
    if let Some(ref since) = cli.since {
        query.push_str(&format!(" WHERE updated_at > '{since}'"));
    }
    query.push_str(" ORDER BY updated_at ASC");

    let mut stmt = conn.prepare(&query)?;
    let rows = stmt.query_map([], |row| {
        Ok(ThreadRow {
            id: row.get(0)?,
            summary: row.get(1)?,
            updated_at: row.get(2)?,
            created_at: row.get(3)?,
            data: row.get(4)?,
            folder_paths: row.get(5)?,
            worktree_branch: row.get(6)?,
        })
    })?;

    let (mut success, mut errors, mut total_in, mut total_out) = (0u32, 0u32, 0u64, 0u64);
    if !cli.dry_run {
        fs::create_dir_all(&cli.output)?;
    }

    for row_result in rows {
        let row = row_result?;
        let json_bytes = match decompress_zstd(&row.data) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("SKIP {}: zstd: {e}", row.id);
                errors += 1;
                continue;
            }
        };
        let thread = match parse_thread(&json_bytes) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("SKIP {}: json: {e}", row.id);
                errors += 1;
                continue;
            }
        };
        let tokens = aggregate_tokens(&thread.request_token_usage);
        total_in += tokens.total_in;
        total_out += tokens.total_out;

        if cli.dry_run {
            let (u, a) = count_turns(&thread);
            println!(
                "{} | {} | turns={}/{} | tok_in={} tok_out={} | tools={:?}",
                row.id,
                thread.title.as_deref().unwrap_or("?"),
                u,
                a,
                tokens.total_in,
                tokens.total_out,
                extract_tools(&thread)
            );
            success += 1;
            continue;
        }

        let md = match render_markdown(&row, &thread) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("SKIP {}: render: {e}", row.id);
                errors += 1;
                continue;
            }
        };

        let date_prefix = if row.updated_at.len() >= 10 {
            row.updated_at[..10].replace('-', "/")
        } else {
            "unknown".to_string()
        };

        let dir = cli.output.join(&date_prefix);
        fs::create_dir_all(&dir)?;
        let path = dir.join(format!("zed-{}.md", row.id));
        let mut file = fs::File::create(&path)?;
        file.write_all(md.as_bytes())?;
        success += 1;
    }

    eprintln!("\n=== transcriptd-zed ===");
    eprintln!("decoded: {success}");
    eprintln!("errors:  {errors}");
    eprintln!("tokens:  in={total_in} out={total_out}");
    if !cli.dry_run {
        eprintln!("output:  {}", cli.output.display());
    }
    Ok(())
}
