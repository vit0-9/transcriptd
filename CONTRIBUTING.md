# Contributing to transcriptd

Thanks for your interest in contributing. This guide covers everything you need to get up and running.

## Before you start

- Search [existing issues](https://github.com/vit0-9/transcriptd/issues) and [discussions](https://github.com/vit0-9/transcriptd/discussions) before opening a new one.
- **Issues** = confirmed bugs and actionable feature work.
- **Discussions** = setup help, ideas, questions, feedback.
- For larger changes, open an issue first so we can align on direction before you invest time.
- For security vulnerabilities, follow [SECURITY.md](SECURITY.md) — do **not** file a public issue.

## Prerequisites

- Rust 1.85+ (`rustup update stable`)
- `cargo` (comes with Rust)
- Optional: `cargo-watch` for incremental dev

## Local setup

```sh
git clone https://github.com/vit0-9/transcriptd.git
cd transcriptd

# Build all crates
cargo build --all

# Run tests (all platforms)
cargo test --all

# Format check
cargo fmt --all -- --check

# Lint (zero warnings policy)
cargo clippy --all --all-targets -- -D warnings
```

## Development workflow

```sh
# Ingest from your local IDEs (uses actual data)
cargo run -- db ingest

# Search
cargo run -- search "something"

# Start the watcher daemon
cargo run -- service up

# TUI dashboard
cargo run -- dash

# MCP stdio server
cargo run -- mcp stdio
```

## Before submitting a PR

Run these — CI will enforce them:

```sh
cargo fmt --all
cargo clippy --all --all-targets -- -D warnings
cargo test --all
```

All three must pass clean. CI runs `RUSTFLAGS="-Dwarnings"` so even deprecation warnings are failures.

## PR guidelines

Keep PRs focused on one problem. Don't mix unrelated cleanup or refactors. A good PR includes:

- **What changed and why** — brief summary in the description
- **Which IDE source paths were tested** if you touched an extractor (Zed, Claude, VSCode)
- **Exact commands you ran** to verify the fix
- **Screenshot or terminal output** if you changed CLI output or the TUI dashboard
- **Docs update** if you changed a CLI command, config option, or user-facing behavior

### PR size

- Prefer small, reviewable PRs over large sweeping ones.
- If a fix requires refactoring, do the refactor in a separate PR first.
- If you're unsure about scope, open a draft PR and ask.

## Project structure

```
transcriptd/
├── src/                    # Main binary (CLI + daemon + dashboard + MCP)
├── crates/
│   ├── transcriptd-core/   # Shared types + TranscriptExtractor trait
│   ├── transcriptd-store/  # SQLite + FTS5 storage
│   ├── transcriptd-zed/    # Zed AI thread extractor
│   ├── transcriptd-claude/ # Claude Code JSONL extractor
│   ├── transcriptd-vscode/ # VSCode Copilot extractor
│   ├── transcriptd-codex/  # OpenAI Codex extractor
│   └── transcriptd-cursor/ # Cursor AI extractor
└── docs/                   # Architecture, ADRs, roadmap
```

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for a full data-flow diagram and design rationale.

## Adding a new IDE extractor

1. Create a new crate under `crates/transcriptd-<name>/`
2. Implement the `TranscriptExtractor` trait from `transcriptd-core`
3. Register in `all_extractors()` in `src/main.rs`
4. Add test fixtures under `crates/transcriptd-<name>/tests/`

## Code style

- Follow the patterns in the file you're editing.
- No reformatting of unrelated code.
- Keep comments concise and useful — prefer self-documenting code over inline prose.
- `unwrap()` is acceptable in tests; prefer `?` with `anyhow::Result` in production paths.

## Commit messages

Use conventional commits format: `type(scope): description`

```
feat(extractor): add Cursor AI transcript support
fix(store): correct canonical ID generation for claude-code source
docs(readme): add Homebrew install instructions
```

Types: `feat`, `fix`, `docs`, `refactor`, `test`, `ci`, `chore`.

## Questions?

Open a [Discussion](https://github.com/vit0-9/transcriptd/discussions) — don't open an issue for questions.
