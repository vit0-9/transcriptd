# transcriptd

Capture, index and search your AI coding conversations.

Single binary. Local SQLite. Works with **Zed**, **Claude Code**, **VSCode Copilot**, **Codex**, and **Cursor**.

## Install

### From source (requires Rust 1.80+)

```sh
git clone https://github.com/vit0-9/transcriptd.git
cd transcriptd
cargo install --path .
```

### From GitHub

```sh
cargo install --git https://github.com/vit0-9/transcriptd.git
```

### Docker

```sh
docker run -v ~/.local/share/transcriptd:/data ghcr.io/vit0-9/transcriptd db ingest
```

### Homebrew (coming with first release)

```sh
brew tap vit0-9/tap
brew install transcriptd
```

## Quick start

```sh
# Ingest all IDE transcripts (auto-detects Zed, Claude Code, VSCode, Codex, Cursor)
transcriptd db ingest

# Open the live dashboard
transcriptd dash

# Search across all conversations
transcriptd search "auth bug"

# Browse recent sessions
transcriptd list --sort created --limit 20

# View a specific transcript
transcriptd show <id> --full

# Inspect raw metadata as JSON (like docker inspect)
transcriptd inspect <id>

# Service status
transcriptd status

# Generate a digest of recent activity
transcriptd digest --since 24h
```

## Commands

```
transcriptd <COMMAND>

  status       Show service status, database stats, and health overview
  dash         Open the live TUI dashboard
  list         List transcripts
  show         Render a transcript in the terminal
  inspect      Dump raw transcript metadata as JSON
  search       Full-text search transcripts
  digest       Summary digest for a time period
  service      Watcher daemon lifecycle (up, down, status, logs)
  mcp          MCP server transports (stdio, serve, stop, show)
  db           Database operations (ingest, dedupe, vacuum)
  config       Manage configuration
  completions  Generate shell completions
```

### Service daemon

```sh
transcriptd service up       # Start the background watcher
transcriptd service down     # Stop it
transcriptd service status   # Check status
transcriptd service logs     # View logs
```

The watcher monitors IDE transcript directories and ingests new conversations automatically.

### MCP integration

```sh
transcriptd mcp stdio        # Launch MCP over stdin/stdout (for IDE config)
transcriptd mcp serve        # Start the MCP HTTP/SSE daemon on port 3100
transcriptd mcp stop         # Stop the MCP HTTP daemon
transcriptd mcp show         # Print MCP client config JSON for editors
```

### Database

```sh
transcriptd db ingest        # Ingest from all sources
transcriptd db ingest --source zed   # Ingest from a specific source
transcriptd db dedupe        # Remove duplicate transcripts
transcriptd db vacuum        # Reclaim disk space
```

## Dashboard

`transcriptd dash` opens a live TUI dashboard (refreshes every 2s):

- **Service panel** — watcher and MCP HTTP status with PIDs
- **Today panel** — sessions, tokens in/out, burn rate, per-source breakdown
- **Errors panel** — tool error count and recent failures
- **Recent sessions** — scrollable table with source, title, model, turns, tokens
- **Hourly chart** — today's token usage by hour with peak annotation
- **14-day trend** — daily token sparkline with date labels
- **Live log** — tails the service log, color-coded by severity

Keybindings: `↑↓`/`jk` scroll, `r` refresh, `q` quit.

## What it does

`transcriptd` reads the local databases where your AI coding tools store conversations and indexes them into a searchable SQLite database with full-text search.

| Source | Storage format | Status |
|--------|---------------|--------|
| Zed | SQLite + zstd compressed JSON | ✅ |
| Claude Code | JSONL files in `~/.claude/projects/` | ✅ |
| VSCode Copilot | JSONL files in workspace storage | ✅ |
| Codex | JSONL files in `~/.codex/` | ✅ |
| Cursor | SQLite in workspace storage | ✅ |
| Windsurf | — | planned |
| Browser (ChatGPT, Claude, Gemini) | — | planned |

## How it works

```
IDE databases ──► transcriptd db ingest ──► SQLite + FTS5
                                                 │
                        transcriptd search ◄─────┤
                        transcriptd list   ◄─────┤
                        transcriptd show   ◄─────┤
                        transcriptd dash   ◄─────┤
                        transcriptd digest ◄─────┘

transcriptd service up  ──► background watcher ──► auto-ingest on file change
transcriptd mcp serve   ──► HTTP/SSE server on :3100 for IDE MCP clients
```

Each IDE adapter normalizes conversations into markdown with YAML frontmatter. Metadata includes token usage, tool invocations (with error tracking), model info, timestamps, and project paths.

The SQLite database lives at `~/Library/Application Support/transcriptd/transcriptd.db` on macOS (override with `--db` or `TRANSCRIPTD_DB` env var).

## Development

```sh
git clone https://github.com/vit0-9/transcriptd.git
cd transcriptd

# Build
cargo build

# Run tests
cargo test --all

# Run locally
cargo run -- db ingest
cargo run -- search "something"
cargo run -- dash
```

### Project structure

```
crates/
  transcriptd-core/     Shared types and extractors
  transcriptd-store/    SQLite + FTS5 storage layer
  transcriptd-zed/      Zed thread decoder
  transcriptd-claude/   Claude Code JSONL decoder
  transcriptd-vscode/   VSCode Copilot JSONL decoder
src/
  main.rs               CLI entry point
  cli.rs                Clap command definitions
  dash.rs               Live TUI dashboard (ratatui)
  daemon.rs             Background watcher + MCP HTTP server
  mcp.rs                MCP protocol implementation
  config.rs             Configuration and paths
  commands/             Subcommand handlers
```

Each adapter in `crates/` reads one IDE's proprietary format and outputs normalized transcript records. The main binary orchestrates ingestion, search, dashboarding, and MCP serving.

## License

AGPL-3.0-or-later — see [LICENSE](LICENSE).
