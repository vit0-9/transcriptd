use anyhow::Result;
use clap::Parser;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use transcriptd_vscode::*;

#[derive(Parser)]
#[command(
    name = "transcriptd-vscode",
    about = "Extract VSCode Copilot Chat sessions to markdown"
)]
struct Cli {
    /// Path to VSCode workspaceStorage directory
    #[arg(short, long, default_value_t = default_dir_string())]
    dir: String,

    /// Output directory
    #[arg(short, long, default_value = "./out")]
    output: PathBuf,

    /// Only extract sessions updated after this ISO date
    #[arg(long)]
    since: Option<String>,

    /// Extract single session file
    #[arg(long)]
    file: Option<String>,

    /// Dry run: print summary, don't write files
    #[arg(long)]
    dry_run: bool,
}

fn default_dir_string() -> String {
    default_vscode_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "~/.config/Code/User/workspaceStorage".to_string())
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Single-file mode
    if let Some(ref path) = cli.file {
        let md = extract_one(path)?;
        if cli.dry_run {
            println!("{md}");
        } else {
            fs::create_dir_all(&cli.output)?;
            let fname = std::path::Path::new(path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("session");
            let out = cli.output.join(format!("vscode-{fname}.md"));
            let mut f = fs::File::create(&out)?;
            f.write_all(md.as_bytes())?;
            eprintln!("wrote {}", out.display());
        }
        return Ok(());
    }

    // Count
    let total = count_sessions(&cli.dir)?;
    eprintln!("found {total} sessions in {}", cli.dir);

    // Extract all
    let pairs = extract_all(&cli.dir, cli.since.as_deref())?;

    if cli.dry_run {
        for (rel, md) in &pairs {
            // extract title from frontmatter
            let title_line = md
                .lines()
                .find(|l| l.starts_with("title: "))
                .unwrap_or("title: ?");
            let turns_line = md
                .lines()
                .find(|l| l.starts_with("turns_total: "))
                .unwrap_or("turns_total: ?");
            println!("{rel} | {title_line} | {turns_line}");
        }
        eprintln!("\nwould write {} files", pairs.len());
        return Ok(());
    }

    let mut written = 0u32;
    for (rel, md) in &pairs {
        let path = cli.output.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut f = fs::File::create(&path)?;
        f.write_all(md.as_bytes())?;
        written += 1;
    }

    eprintln!("\n=== transcriptd-vscode ===");
    eprintln!("total sessions: {total}");
    eprintln!("written:        {written}");
    eprintln!("output:         {}", cli.output.display());
    Ok(())
}
