# transcriptd — Architecture Overview

## What is transcriptd?

A local-first CLI tool that captures, indexes, and searches AI coding conversations from multiple IDEs. Think `docker` for your AI transcripts — a daemon watches for changes, a CLI queries the data, and a TUI dashboard shows live stats.

## Design Philosophy

- **Local-first:** All data stays on your machine in SQLite
- **Unix philosophy:** Small composable commands, text output, pipes
- **Docker/Tailscale-inspired CLI:** Intuitive command hierarchy, daemon managed by OS
- **Extensible:** Trait-based adapter pattern for new IDE sources

## Workspace Layout

```
transcriptd/
├── src/                        # Main binary
│   ├── main.rs                 # CLI entrypoint + command dispatch
│   ├── config.rs               # Configuration management
│   ├── daemon.rs               # Background daemon (watcher + MCP + IPC)
│   ├── dash.rs                 # TUI dashboard (ratatui)
│   ├── ipc.rs                  # IPC protocol (JSON-lines over socket)
│   └── mcp.rs                  # Model Context Protocol server
├── crates/
│   ├── transcriptd-core/       # Shared types + TranscriptExtractor trait
│   ├── transcriptd-store/      # SQLite + FTS5 storage layer (V2 schema)
│   ├── transcriptd-zed/        # Zed AI thread extractor
│   ├── transcriptd-claude/     # Claude Code JSONL extractor
│   ├── transcriptd-vscode/     # VSCode Copilot extractor (.json + .jsonl)
│   ├── transcriptd-codex/      # OpenAI Codex CLI session extractor
│   └── transcriptd-cursor/     # Cursor AI conversation extractor
├── docs/                       # Documentation
├── .github/workflows/          # CI + Release pipelines
├── Formula/                    # Homebrew formula
└── Dockerfile                  # Container build
```

## Data Flow

```
┌──────────────┐     ┌──────────────┐     ┌──────────────┐     ┌──────────────┐     ┌──────────────┐
│   Zed DB     │     │ Claude JSONL │     │ VSCode JSON  │     │ Codex JSONL  │     │ Cursor vscdb │
│  (zstd+SQL)  │     │  (~/.claude) │     │ + JSONL      │     │ (~/.codex)   │     │  (SQLite)    │
└──────┬───────┘     └──────┬───────┘     └──────┬───────┘     └──────┬───────┘     └──────┬───────┘
       │                    │                    │                    │                    │
       └────────────┬───────┘────────────────────┘────────────────────┘────────────────────┘
                    │
            TranscriptExtractor trait
            (extract_all, watch_paths)
                    │
                    ▼
        ┌───────────────────────┐
        │   transcriptd-store   │
        │  SQLite + FTS5 + WAL  │
        │  (V2: tool_invocations│
        │   mode, originator)   │
        └───────────┬───────────┘
                    │
        ┌───────────┼───────────────────┐
        │           │                   │
        ▼           ▼                   ▼
   CLI commands   TUI Dashboard      MCP Server
   (search,list)  (ratatui live)   (JSON-RPC + SSE)
```

## Key Abstractions

### TranscriptExtractor (core trait)

```rust
pub trait TranscriptExtractor: Send + Sync {
    fn name(&self) -> &str;
    fn default_source_path(&self) -> Option<PathBuf>;
    fn count(&self, source: &Path) -> Result<usize>;
    fn extract_all(&self, source: &Path, since: Option<&str>) -> Result<Vec<(String, String)>>;
    fn extract_one(&self, source: &Path, id: &str) -> Result<String>;
    fn watch_paths(&self, source: &Path) -> Vec<PathBuf>;
}
```

Each adapter implements this. Adding a new IDE: implement the trait, register in `all_extractors()`.

### Store Layer

- SQLite with WAL mode for concurrent reads
- FTS5 virtual table for full-text search
- Proper indexes on source, created_at, model, tools
- Upsert semantics (insert or update on conflict)

### IPC Protocol

JSON-lines over Unix domain socket. Messages:

- `stats` — full statistics snapshot
- `event` — ingest/watch events
- `recent` — recent transcripts list
- `daily` — daily token counts (sparkline data)

### MCP (Model Context Protocol)

JSON-RPC 2.0 over stdio (for agent integrations) and HTTP (for remote access).

Tools exposed:

- `search_transcripts` — FTS5 search
- `get_transcript` — single transcript with body (truncated)
- `get_stats` — aggregate statistics
- `list_recent` — recent transcripts
- `get_digest` — time-period summary

## Target Architecture (after Phase 1 rework)

```
┌─────────────────────────────────────────────────┐
│              transcriptd daemon                  │
│  ┌─────────┐  ┌──────────┐  ┌────────────────┐ │
│  │ Watcher  │  │   axum   │  │  IPC broadcast │ │
│  │ (notify) │  │  server  │  │  (tokio mpsc)  │ │
│  │          │  │          │  │                │ │
│  │ detect   │  │ /mcp     │  │ dashboard      │ │
│  │ changes  │──│ /sse     │  │ subscribers    │ │
│  │ ingest   │  │ /health  │  │                │ │
│  └─────────┘  └──────────┘  └────────────────┘ │
│        │              │              │           │
│        └──────────────┼──────────────┘           │
│                       │                          │
│              SQLite + FTS5                       │
└─────────────────────────────────────────────────┘
         ▲                      ▲
         │                      │
    Unix socket            HTTP :3100
    (dashboard,             (MCP clients,
     CLI status)            AI agents)
```

Key changes from current:

- **tokio async runtime** — single event loop, not 4 threads
- **axum** — native SSE, Unix socket support, tower middleware
- **Proper socket path** — `$XDG_RUNTIME_DIR/transcriptd/` or `~/Library/Application Support/transcriptd/`
- **Health endpoint** — `GET /health` for daemon status checks
- **SSE endpoint** — `GET /sse` for streaming events to MCP clients

## CLI Command Map

```
transcriptd
├── status                    # Check daemon, show summary stats
├── ingest [--source X]       # One-shot ingest from IDE sources
├── search <query>            # Full-text search
├── list [--source X]         # List transcripts
├── show <id>                 # Show single transcript
├── stats                     # Detailed statistics
├── digest [--period X]       # Generate time-period summary
├── dash                      # TUI dashboard
├── mcp                       # MCP stdio server
├── config
│   ├── show                  # Show current config
│   └── reset-key             # Regenerate API key
└── daemon                    # Run daemon (foreground, for systemd/launchd)
```

## Socket & File Locations

| Item | macOS | Linux |
|------|-------|-------|
| Database | `~/Library/Application Support/transcriptd/transcriptd.db` | `~/.local/share/transcriptd/transcriptd.db` |
| Config | `~/Library/Application Support/transcriptd/config.json` | `~/.config/transcriptd/config.json` |
| Socket | `~/Library/Application Support/transcriptd/transcriptd.sock` | `$XDG_RUNTIME_DIR/transcriptd/transcriptd.sock` |
| PID file | `~/Library/Application Support/transcriptd/transcriptd.pid` | `$XDG_RUNTIME_DIR/transcriptd/transcriptd.pid` |

## Security Model

- MCP HTTP requires `Authorization: Bearer <token>`
- Token auto-generated on first run, stored in config
- Override via `TRANSCRIPTD_API_KEY` env var
- HTTP binds to `127.0.0.1` only (no network exposure)
- Unix socket uses filesystem permissions
