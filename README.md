# transcriptd

Capture, index and search your AI coding conversations.

Single binary. Local SQLite. Works with **Zed**, **Claude Code**, and **VSCode Copilot**.

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
docker run -v ~/.local/share/transcriptd:/data ghcr.io/vit0-9/transcriptd ingest
```

### Homebrew (coming with first release)

```sh
brew tap vit0-9/tap
brew install transcriptd
```

## Quick start

```sh
# Ingest all IDE transcripts (auto-detects Zed, Claude Code, VSCode)
transcriptd ingest

# Search across all conversations
transcriptd search "auth bug"

# Browse recent sessions
transcriptd list --sort created --limit 20

# View a specific transcript
transcriptd show zed-abc123 --full

# Stats
transcriptd stats
```

## What it does

`transcriptd` reads the local databases where your AI coding tools store conversations and indexes them into a searchable SQLite database with full-text search.

| Source | Storage format | Status |
|--------|---------------|--------|
| Zed | SQLite + zstd compressed JSON | ✅ |
| Claude Code | JSONL files in `~/.claude/projects/` | ✅ |
| VSCode Copilot | JSON files in workspace storage | ✅ |
| Cursor | — | planned |
| Windsurf | — | planned |
| Browser (ChatGPT, Claude, Gemini) | — | planned |

## How it works

```
IDE databases ──► transcriptd ingest ──► SQLite + FTS5
                                              │
                                    transcriptd search
                                    transcriptd list
                                    transcriptd show
                                    transcriptd stats
```

Each IDE adapter normalizes conversations into markdown with YAML frontmatter. Metadata includes token usage, tool invocations, model info, timestamps, and project paths.

The SQLite database lives at `~/.local/share/transcriptd/transcriptd.db` by default (override with `--db` or `TRANSCRIPTD_DB` env var).

## Development

```sh
git clone https://github.com/vit0-9/transcriptd.git
cd transcriptd

# Build
cargo build

# Run tests
cargo test --all

# Run locally
cargo run -- ingest
cargo run -- search "something"
```

### Project structure

```
crates/
  transcriptd-core/     Shared types
  transcriptd-store/    SQLite + FTS5 storage
  transcriptd-zed/      Zed thread decoder
  transcriptd-claude/   Claude Code JSONL decoder
  transcriptd-vscode/   VSCode Copilot decoder
src/
  main.rs               CLI binary
```

Each adapter in `crates/` reads one IDE's proprietary format and outputs normalized markdown. The main binary orchestrates ingestion and provides search/browse commands.

## License

AGPL-3.0-or-later — see [LICENSE](LICENSE).
