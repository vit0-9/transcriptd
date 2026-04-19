use anyhow::Result;
use serde_json::{Value, json};
use std::path::Path;

const PROTOCOL_VERSION: &str = "2024-11-05";
const SERVER_NAME: &str = "transcriptd";
const SERVER_VERSION: &str = "0.1.0";

/// Process a single JSON-RPC request and return the response JSON.
/// `db_path` is used to open a fresh SQLite connection per request.
pub fn handle_request(db_path: &Path, request: &Value) -> Value {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let method = request.get("method").and_then(|v| v.as_str()).unwrap_or("");
    let params = request.get("params").cloned().unwrap_or(json!({}));

    match method {
        "initialize" => json_rpc_response(
            id,
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {
                    "tools": {}
                },
                "serverInfo": {
                    "name": SERVER_NAME,
                    "version": SERVER_VERSION
                }
            }),
        ),

        "notifications/initialized" | "initialized" => {
            // Notification — no response
            Value::Null
        }

        "tools/list" => json_rpc_response(
            id,
            json!({
                "tools": tool_definitions()
            }),
        ),

        "tools/call" => {
            let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            match call_tool(db_path, tool_name, &args) {
                Ok(result) => json_rpc_response(
                    id,
                    json!({
                        "content": [{
                            "type": "text",
                            "text": result
                        }]
                    }),
                ),
                Err(e) => json_rpc_response(
                    id,
                    json!({
                        "content": [{
                            "type": "text",
                            "text": format!("Error: {e}")
                        }],
                        "isError": true
                    }),
                ),
            }
        }

        "ping" => json_rpc_response(id, json!({})),

        _ => json_rpc_error(id, -32601, &format!("Method not found: {method}")),
    }
}

/// Run MCP over stdio (blocking). Reads JSON-RPC from stdin line by line.
pub fn run_stdio(db_path: &Path) -> Result<()> {
    use std::io::{self, BufRead, IsTerminal, Write};

    let stdin = io::stdin();
    let stdout = io::stdout();

    // If stdin is a TTY, the user ran `transcriptd mcp` interactively by mistake.
    // Stdio MCP is meant to be invoked by an editor (Claude Desktop / Cursor / Zed).
    if stdin.is_terminal() {
        eprintln!("transcriptd mcp — stdio MCP server (JSON-RPC over stdin/stdout)");
        eprintln!();
        eprintln!("This command is invoked by editors (Claude Desktop / Cursor / Zed),");
        eprintln!("not run interactively.");
        eprintln!();
        eprintln!("What you probably want:");
        eprintln!("  transcriptd mcp serve   Start the MCP HTTP daemon on port 3100");
        eprintln!("  transcriptd mcp show    Print MCP client config JSON for your editor");
        eprintln!("  transcriptd mcp stop    Stop the MCP HTTP daemon");
        eprintln!("  transcriptd status      Show service status");
        eprintln!();
        eprintln!("To exit this stdio session: Ctrl-D");
        eprintln!();
    }

    for line in stdin.lock().lines() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let request: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                let err = json_rpc_error(Value::Null, -32700, &format!("Parse error: {e}"));
                writeln!(stdout.lock(), "{}", serde_json::to_string(&err)?)?;
                stdout.lock().flush()?;
                continue;
            }
        };

        let response = handle_request(db_path, &request);
        if response.is_null() {
            // Notification — no response
            continue;
        }

        writeln!(stdout.lock(), "{}", serde_json::to_string(&response)?)?;
        stdout.lock().flush()?;
    }

    Ok(())
}

// -- Tool definitions -------------------------------------------------------

fn tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "search_transcripts",
            "description": "Search AI coding transcripts using full-text search. Returns matching transcripts with metadata (no full body). Supports pagination via offset.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query (FTS5 syntax supported)"
                    },
                    "limit": {
                        "type": "number",
                        "description": "Max results (default 10)"
                    },
                    "offset": {
                        "type": "number",
                        "description": "Offset for pagination (default 0)"
                    }
                },
                "required": ["query"]
            }
        }),
        json!({
            "name": "get_transcript",
            "description": "Get a single transcript by ID. Returns metadata and body text. Supports short IDs like #42.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Transcript ID (full ID like zed-61be... or short ID like #42)"
                    },
                    "max_body": {
                        "type": "number",
                        "description": "Max body chars to return (default 8000). Use 0 for no limit."
                    }
                },
                "required": ["id"]
            }
        }),
        json!({
            "name": "get_stats",
            "description": "Get overall transcriptd statistics: total transcripts, turns, tokens, sources breakdown, top tools.",
            "inputSchema": {
                "type": "object",
                "properties": {}
            }
        }),
        json!({
            "name": "list_recent",
            "description": "List AI coding transcripts. Supports filtering by source, sorting, and pagination. Use sort=turns to find the longest/most complex sessions.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "limit": {
                        "type": "number",
                        "description": "Max results (default 10)"
                    },
                    "offset": {
                        "type": "number",
                        "description": "Offset for pagination (default 0)"
                    },
                    "source": {
                        "type": "string",
                        "description": "Filter by source (zed, claude-code, vscode-copilot, codex, cursor)"
                    },
                    "sort": {
                        "type": "string",
                        "description": "Sort order: created (newest first, default), tokens (most tokens first), turns (most turns first)"
                    }
                }
            }
        }),
        json!({
            "name": "get_digest",
            "description": "Get a summary digest for a time period: total sessions, tokens, tools used, broken down by source.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "period": {
                        "type": "string",
                        "description": "Period: today, yesterday, week, month, or YYYY-MM-DD (default: today)"
                    }
                }
            }
        }),
    ]
}

// -- Short ID resolution ----------------------------------------------------

fn resolve_short_id(conn: &rusqlite::Connection, id: &str) -> Result<String> {
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

// -- Tool execution ---------------------------------------------------------

fn call_tool(db_path: &Path, name: &str, args: &Value) -> Result<String> {
    let conn = transcriptd_store::init_db(db_path)?;

    match name {
        "search_transcripts" => {
            let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
            let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let results = transcriptd_store::search(&conn, query, limit, offset)?;
            let summaries: Vec<Value> = results
                .iter()
                .map(|r| {
                    json!({
                        "id": r.id,
                        "source": r.source,
                        "title": r.title,
                        "turns": r.turns_total,
                        "tokens_in": r.tokens_in,
                        "tokens_out": r.tokens_out,
                        "tools_used": r.tools_used,
                        "model": format!("{}/{}", r.model_provider, r.model_name),
                        "created_at": r.created_at,
                    })
                })
                .collect();
            Ok(serde_json::to_string_pretty(&json!({
                "count": summaries.len(),
                "results": summaries
            }))?)
        }

        "get_transcript" => {
            let raw_id = args.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let id = resolve_short_id(&conn, raw_id)?;
            let max_body = args
                .get("max_body")
                .and_then(|v| v.as_u64())
                .unwrap_or(8000) as usize;
            let rec = transcriptd_store::get_transcript(&conn, &id)?
                .ok_or_else(|| anyhow::anyhow!("transcript not found: {raw_id}"))?;
            let body_truncated = if max_body > 0 && rec.body_text.len() > max_body {
                format!(
                    "{}...\n\n[truncated, {} total chars]",
                    &rec.body_text[..max_body],
                    rec.body_text.len()
                )
            } else {
                rec.body_text.clone()
            };
            Ok(serde_json::to_string_pretty(&json!({
                "id": rec.id,
                "source": rec.source,
                "title": rec.title,
                "model": format!("{}/{}", rec.model_provider, rec.model_name),
                "turns_user": rec.turns_user,
                "turns_agent": rec.turns_agent,
                "turns_total": rec.turns_total,
                "tokens_in": rec.tokens_in,
                "tokens_out": rec.tokens_out,
                "tools_used": rec.tools_used,
                "folder_paths": rec.folder_paths,
                "branch": rec.branch,
                "created_at": rec.created_at,
                "updated_at": rec.updated_at,
                "body": body_truncated,
            }))?)
        }

        "get_stats" => {
            let stats = transcriptd_store::get_stats(&conn)?;
            Ok(serde_json::to_string_pretty(&json!({
                "total_transcripts": stats.total_transcripts,
                "total_turns": stats.total_turns,
                "total_tokens_in": stats.total_tokens_in,
                "total_tokens_out": stats.total_tokens_out,
                "sources": stats.sources,
                "top_tools": stats.top_tools,
            }))?)
        }

        "list_recent" => {
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
            let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let source = args.get("source").and_then(|v| v.as_str());
            let sort = match args.get("sort").and_then(|v| v.as_str()) {
                Some("tokens") => "tokens_in",
                Some("turns") => "turns_total",
                _ => "created_at",
            };
            let results = transcriptd_store::list_transcripts(&conn, source, limit, offset, sort)?;
            let summaries: Vec<Value> = results
                .iter()
                .map(|r| {
                    json!({
                        "id": r.id,
                        "source": r.source,
                        "title": r.title,
                        "model": format!("{}/{}", r.model_provider, r.model_name),
                        "turns": r.turns_total,
                        "tokens_in": r.tokens_in,
                        "tokens_out": r.tokens_out,
                        "tools_used": r.tools_used,
                        "created_at": r.created_at,
                    })
                })
                .collect();
            Ok(serde_json::to_string_pretty(&json!({
                "count": summaries.len(),
                "transcripts": summaries
            }))?)
        }

        "get_digest" => {
            let period = args
                .get("period")
                .and_then(|v| v.as_str())
                .unwrap_or("today");
            let today = chrono::Local::now().date_naive();
            let (start, end) = match period {
                "today" => (today, today),
                "yesterday" => {
                    let y = today - chrono::Duration::days(1);
                    (y, y)
                }
                "week" => (today - chrono::Duration::days(7), today),
                "month" => (today - chrono::Duration::days(30), today),
                other => {
                    if let Ok(d) = chrono::NaiveDate::parse_from_str(other, "%Y-%m-%d") {
                        (d, d)
                    } else {
                        anyhow::bail!("invalid period: {other}")
                    }
                }
            };
            let start_str = format!("{}T00:00:00", start);
            let end_str = format!("{}T23:59:59", end);

            let mut stmt = conn.prepare(
                "SELECT source, COUNT(*) as cnt, COALESCE(SUM(tokens_in),0), COALESCE(SUM(tokens_out),0) FROM transcripts WHERE created_at >= ?1 AND created_at <= ?2 GROUP BY source"
            )?;
            let rows: Vec<Value> = stmt
                .query_map([&start_str, &end_str], |row| {
                    Ok(json!({
                        "source": row.get::<_, String>(0)?,
                        "sessions": row.get::<_, i64>(1)?,
                        "tokens_in": row.get::<_, i64>(2)?,
                        "tokens_out": row.get::<_, i64>(3)?,
                    }))
                })?
                .filter_map(|r| r.ok())
                .collect();

            let total_sessions: i64 = rows
                .iter()
                .map(|r| r["sessions"].as_i64().unwrap_or(0))
                .sum();
            let total_in: i64 = rows
                .iter()
                .map(|r| r["tokens_in"].as_i64().unwrap_or(0))
                .sum();
            let total_out: i64 = rows
                .iter()
                .map(|r| r["tokens_out"].as_i64().unwrap_or(0))
                .sum();

            Ok(serde_json::to_string_pretty(&json!({
                "period": period,
                "start": start_str,
                "end": end_str,
                "total_sessions": total_sessions,
                "total_tokens_in": total_in,
                "total_tokens_out": total_out,
                "by_source": rows,
            }))?)
        }

        _ => anyhow::bail!("unknown tool: {name}"),
    }
}

// -- JSON-RPC helpers -------------------------------------------------------

fn json_rpc_response(id: Value, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result
    })
}

fn json_rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message
        }
    })
}
