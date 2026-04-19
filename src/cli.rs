use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "transcriptd", version, about = "AI transcript indexer")]
pub struct Cli {
    /// Path to SQLite database
    #[arg(long, env = "TRANSCRIPTD_DB", default_value_t = default_db_path())]
    pub db: String,

    #[command(subcommand)]
    pub cmd: Cmd,
}

fn default_db_path() -> String {
    dirs::data_dir()
        .map(|d| d.join("transcriptd").join("transcriptd.db"))
        .unwrap_or_else(|| PathBuf::from("transcriptd.db"))
        .to_string_lossy()
        .to_string()
}

#[derive(Subcommand)]
pub enum Cmd {
    // ── Status & dashboard (top-level for discoverability) ──
    /// Show service status, database stats, and health overview
    Status,

    /// Open the live Ratatui TUI dashboard
    Dash,

    // ── Core transcript operations ──
    /// List transcripts
    List {
        /// Filter by source
        #[arg(long)]
        source: Option<String>,
        /// Max results
        #[arg(long, default_value = "20")]
        limit: usize,
        /// Offset
        #[arg(long, default_value = "0")]
        offset: usize,
        /// Sort field
        #[arg(long, default_value = "date")]
        sort: String,
        /// Output format
        #[arg(long, default_value = "text")]
        format: String,
    },

    /// Render a transcript in the terminal (text or markdown)
    Show {
        /// Transcript ID (full ID or #seq number)
        id: String,
        /// Output format (text, markdown)
        #[arg(long, default_value = "text")]
        format: String,
        /// Print only the body text (pipe-friendly)
        #[arg(long)]
        body_only: bool,
    },

    /// Dump raw transcript metadata as JSON (like docker inspect)
    Inspect {
        /// Transcript ID (full ID or #seq number)
        id: String,
    },

    /// Full-text search transcripts
    Search {
        /// Search query
        query: String,
        /// Max results
        #[arg(long, default_value = "20")]
        limit: usize,
        /// Offset
        #[arg(long, default_value = "0")]
        offset: usize,
        /// Output format
        #[arg(long, default_value = "text")]
        format: String,
    },

    /// Summary digest for a time period
    Digest {
        /// Period: today, yesterday, week, month, or YYYY-MM-DD
        #[arg(default_value = "today")]
        period: String,
        /// Output format
        #[arg(long, default_value = "text")]
        format: String,
    },

    // ── Service management (watcher daemon) ──
    /// Watcher daemon lifecycle (up, down, status, logs)
    #[command(subcommand)]
    Service(ServiceCmd),

    // ── MCP server ──
    /// MCP server transports (stdio for IDEs, serve for HTTP daemon)
    #[command(subcommand)]
    Mcp(McpCmd),

    // ── Data & maintenance ──
    /// Database operations (ingest, dedupe, vacuum)
    #[command(subcommand)]
    Db(DbCmd),

    /// Manage configuration
    #[command(subcommand)]
    Config(ConfigCmd),

    /// Generate shell completions
    Completions {
        /// Shell to generate for
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },

    // ── Hidden backward-compat aliases ──
    #[command(hide = true)]
    Completion {
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
    #[command(hide = true)]
    Stats {
        #[arg(long, default_value = "text")]
        format: String,
    },
    #[command(hide = true)]
    Logs {
        #[arg(short, long)]
        follow: bool,
        #[arg(short, long, default_value = "50")]
        lines: usize,
    },
    #[command(hide = true)]
    Ingest {
        #[arg(long, default_value = "all")]
        source: String,
        #[arg(long)]
        zed_path: Option<String>,
        #[arg(long)]
        claude_path: Option<String>,
        #[arg(long)]
        vscode_path: Option<String>,
        #[arg(long)]
        codex_path: Option<String>,
        #[arg(long)]
        cursor_path: Option<String>,
        #[arg(long)]
        since: Option<String>,
    },

    /// Internal: run the watcher daemon (not user-facing)
    #[command(hide = true, name = "__run-service")]
    RunService,
    /// Internal: run the MCP HTTP server (not user-facing)
    #[command(hide = true, name = "__run-mcp")]
    RunMcp,
}

// ── Service (watcher daemon) subcommands ──

#[derive(Subcommand)]
pub enum ServiceCmd {
    /// Start the watcher daemon in the background
    Up,
    /// Stop the watcher daemon
    Down,
    /// Show daemon status and database summary
    Status {
        /// Output format (text, json)
        #[arg(long, default_value = "text")]
        format: String,
    },
    /// View service logs
    Logs {
        /// Follow log output (like tail -f)
        #[arg(short, long)]
        follow: bool,
        /// Number of lines to show
        #[arg(short, long, default_value = "50")]
        lines: usize,
    },
}

// ── MCP subcommands ──

#[derive(Subcommand)]
pub enum McpCmd {
    /// Launch MCP over stdin/stdout (put this in your IDE config)
    Stdio,
    /// Start the MCP HTTP/SSE server as a background daemon
    Serve,
    /// Stop the MCP HTTP daemon
    Stop,
    /// Show MCP client configuration JSON for editors
    Show,
}

// ── Configuration subcommands ──

#[derive(Subcommand)]
pub enum ConfigCmd {
    /// Show current configuration
    Show,
    /// Generate a new API key
    ResetKey,
}

// ── Database subcommands ──

#[derive(Subcommand)]
pub enum DbCmd {
    /// Ingest transcripts from all (or specific) sources
    Ingest {
        /// Source name filter (zed, claude-code, vscode-copilot, codex, cursor, or "all")
        #[arg(long, default_value = "all")]
        source: String,
        /// Override source path for zed
        #[arg(long)]
        zed_path: Option<String>,
        /// Override source path for claude
        #[arg(long)]
        claude_path: Option<String>,
        /// Override source path for vscode
        #[arg(long)]
        vscode_path: Option<String>,
        /// Override source path for codex
        #[arg(long)]
        codex_path: Option<String>,
        /// Override source path for cursor
        #[arg(long)]
        cursor_path: Option<String>,
        /// Only ingest transcripts since this ISO timestamp
        #[arg(long)]
        since: Option<String>,
    },
    /// Remove duplicate transcripts (canonicalize IDs, merge rows)
    Dedupe {
        /// Show what would be removed without modifying the DB
        #[arg(long)]
        dry_run: bool,
    },
    /// VACUUM the SQLite database to reclaim space
    Vacuum,
}
