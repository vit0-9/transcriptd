mod cli;
mod commands;
mod config;
mod daemon;
mod dash;
pub mod extractors;
pub mod format;
mod ipc;
mod mcp;
pub mod parse;

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use transcriptd_store::init_db;

use cli::{Cli, Cmd, ConfigCmd, DbCmd, McpCmd, ServiceCmd};

// ---------------------------------------------------------------------------
// Short ID resolver: "#42" → full ID, passthrough otherwise
// ---------------------------------------------------------------------------

fn resolve_id(conn: &rusqlite::Connection, id: &str) -> Result<String> {
    if let Some(seq_str) = id.strip_prefix('#') {
        if let Ok(seq) = seq_str.parse::<i64>() {
            let full_id: Option<String> = conn
                .query_row(
                    "SELECT id FROM transcripts WHERE rowid = ?1",
                    [seq],
                    |row| row.get(0),
                )
                .ok();
            return full_id.ok_or_else(|| anyhow::anyhow!("no transcript with seq #{seq}"));
        }
    }
    Ok(id.to_string())
}

// ---------------------------------------------------------------------------
// Ingest helper (shared by Cmd::Ingest and DbCmd::Ingest)
// ---------------------------------------------------------------------------

fn run_ingest(
    db_path: &PathBuf,
    source: &str,
    zed_path: Option<String>,
    claude_path: Option<String>,
    vscode_path: Option<String>,
    codex_path: Option<String>,
    cursor_path: Option<String>,
    since: Option<String>,
) -> Result<()> {
    let conn = init_db(db_path)?;
    let mut overrides = HashMap::new();
    if let Some(p) = zed_path {
        overrides.insert("zed".to_string(), p);
    }
    if let Some(p) = claude_path {
        overrides.insert("claude-code".to_string(), p);
    }
    if let Some(p) = vscode_path {
        overrides.insert("vscode-copilot".to_string(), p);
    }
    if let Some(p) = codex_path {
        overrides.insert("codex".to_string(), p);
    }
    if let Some(p) = cursor_path {
        overrides.insert("cursor".to_string(), p);
    }
    commands::ingest::cmd_ingest(&conn, source, &overrides, since.as_deref())
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() -> Result<()> {
    let cli = Cli::parse();
    let db_path = PathBuf::from(&cli.db);
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    match cli.cmd {
        // ── Status & dashboard ──
        Cmd::Status => {
            let conn = init_db(&db_path)?;
            commands::status::cmd_status(&conn)?;
        }
        Cmd::Dash => {
            dash::run(&db_path)?;
        }

        // ── Core transcript operations ──
        Cmd::List {
            source,
            limit,
            offset,
            sort,
            format,
        } => {
            let conn = init_db(&db_path)?;
            commands::list::cmd_list(&conn, source.as_deref(), limit, offset, &sort, &format)?;
        }
        Cmd::Show {
            id,
            format,
            body_only,
        } => {
            let conn = init_db(&db_path)?;
            let resolved = resolve_id(&conn, &id)?;
            if body_only {
                let rec = transcriptd_store::get_transcript(&conn, &resolved)?
                    .ok_or_else(|| anyhow::anyhow!("transcript not found: {id}"))?;
                print!("{}", rec.body_text);
            } else {
                commands::show::cmd_show(&conn, &resolved, &format)?;
            }
        }
        Cmd::Inspect { id } => {
            let conn = init_db(&db_path)?;
            let resolved = resolve_id(&conn, &id)?;
            commands::inspect::cmd_inspect(&conn, &resolved)?;
        }
        Cmd::Search {
            query,
            limit,
            offset,
            format,
        } => {
            let conn = init_db(&db_path)?;
            commands::search::cmd_search(&conn, &query, limit, offset, &format)?;
        }
        Cmd::Digest { period, format } => {
            let conn = init_db(&db_path)?;
            commands::digest::cmd_digest(&conn, &period, &format)?;
        }

        // ── Service (watcher daemon) ──
        Cmd::Service(subcmd) => match subcmd {
            ServiceCmd::Up => commands::service::cmd_service_up(&db_path)?,
            ServiceCmd::Down => commands::service::cmd_service_down()?,
            ServiceCmd::Status { format } => {
                let conn = init_db(&db_path)?;
                if format == "json" {
                    commands::stats::cmd_stats(&conn, "json")?;
                } else {
                    commands::status::cmd_status(&conn)?;
                }
            }
            ServiceCmd::Logs { follow, lines } => {
                commands::logs::cmd_logs(follow, lines)?;
            }
        },

        // ── MCP server ──
        Cmd::Mcp(subcmd) => match subcmd {
            McpCmd::Stdio => mcp::run_stdio(&db_path)?,
            McpCmd::Serve => commands::service::cmd_mcp_up(&db_path)?,
            McpCmd::Stop => commands::service::cmd_mcp_down()?,
            McpCmd::Show => commands::mcp_show::cmd_mcp_show()?,
        },

        // ── Database operations ──
        Cmd::Db(subcmd) => match subcmd {
            DbCmd::Ingest {
                source,
                zed_path,
                claude_path,
                vscode_path,
                codex_path,
                cursor_path,
                since,
            } => {
                run_ingest(
                    &db_path,
                    &source,
                    zed_path,
                    claude_path,
                    vscode_path,
                    codex_path,
                    cursor_path,
                    since,
                )?;
            }
            DbCmd::Dedupe { dry_run } => {
                let conn = init_db(&db_path)?;
                transcriptd_store::dedupe_transcripts(&conn, dry_run)?;
            }
            DbCmd::Vacuum => {
                let conn = init_db(&db_path)?;
                conn.execute_batch("VACUUM")?;
                println!("Database vacuumed.");
            }
        },

        // ── Configuration ──
        Cmd::Config(subcmd) => match subcmd {
            ConfigCmd::Show => {
                let cfg = config::load()?;
                println!("{}", serde_json::to_string_pretty(&cfg)?);
            }
            ConfigCmd::ResetKey => {
                let mut cfg = config::load()?;
                cfg.api_key = config::generate_api_key();
                config::save(&cfg)?;
                println!("API key reset: {}", cfg.api_key);
            }
        },

        Cmd::Completions { shell } | Cmd::Completion { shell } => {
            commands::completion::cmd_completion(shell);
        }

        // ── Hidden backward-compat aliases ──
        Cmd::Stats { format } => {
            let conn = init_db(&db_path)?;
            commands::stats::cmd_stats(&conn, &format)?;
        }
        Cmd::Logs { follow, lines } => {
            commands::logs::cmd_logs(follow, lines)?;
        }
        Cmd::Ingest {
            source,
            zed_path,
            claude_path,
            vscode_path,
            codex_path,
            cursor_path,
            since,
        } => {
            run_ingest(
                &db_path,
                &source,
                zed_path,
                claude_path,
                vscode_path,
                codex_path,
                cursor_path,
                since,
            )?;
        }

        // ── Internal daemon entry points ──
        Cmd::RunService => {
            let cfg = config::load()?;
            tokio::runtime::Runtime::new()?.block_on(daemon::run_service(&db_path, &cfg))?;
        }
        Cmd::RunMcp => {
            let cfg = config::load()?;
            tokio::runtime::Runtime::new()?.block_on(daemon::run_mcp_http(&db_path, &cfg))?;
        }
    }

    Ok(())
}
