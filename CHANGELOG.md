# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2026-04-19

### Added

- Core architecture (SQLite with FTS5, generic `TranscriptExtractor` trait, Schema V2)
- Zed AI thread extraction via zstd-compressed SQLite database
- Claude Code JSONL block extraction support
- VSCode Copilot workspace storage extraction support
- OpenAI Codex JSONL session extraction support
- Cursor AI composer session extraction support
- Canonical transcript ID format (`{source}-{native_id}`) with idempotent normalization (ADR-001)
- Self-healing deduplication: `db dedupe` and auto-dedupe on every ingest
- Watcher daemon with `notify` FSEvents and Unix domain socket IPC (`service up/down/status/logs`)
- MCP (Model Context Protocol) server — stdio and HTTP/SSE transports (`mcp stdio`, `mcp serve`)
- TUI Dashboard via `ratatui` with service status, today's stats, errors, hourly/daily charts, live log tail
- `list`, `show`, `inspect`, `search`, `digest`, `stats` CLI commands
- Short `#N` transcript ID references (rowid aliases)
- Shell completion generation (`completions` command)
- Docker image and Homebrew formula
- Linux systemd unit and macOS launchd plist in `contrib/`
- GitHub Actions: CI (fmt + clippy + test + build on Linux + macOS), release pipeline (multi-arch binaries + GHCR Docker)
- `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`, `SECURITY.md`, issue templates, PR template
