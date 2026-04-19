# transcriptd — Roadmap

## Current State: v0.2 (Refactored, ready for hardening)

Working:

- ✅ Zed, Claude Code, VSCode Copilot extractors (7,300+ transcripts indexed in dev)
- ✅ SQLite + FTS5 storage with upserts
- ✅ Tokio + axum service with HTTP MCP + SSE
- ✅ stdio MCP server (Claude Desktop / Cursor / Zed)
- ✅ CLI: status, ingest, search, list, show, stats, logs, mcp, config
- ✅ Background service via `transcriptd mcp -d`
- ✅ Docker-style logs (`transcriptd logs -f`)
- ✅ MCP client config printer (`transcriptd mcp show`)
- ✅ Cross-platform release pipeline, SHA-pinned CI
- ✅ OSS infrastructure (CONTRIBUTING, SECURITY, CoC, templates)

Known issues (from real-world testing):

- ⚠️  Duplicate transcripts (same UUID with/without `zed-` prefix) — ingestion dedup bug
- ⚠️  No live observability dashboard (removed in refactor; needs proper redesign)
- ⚠️  No tests yet
- ⚠️  No vector search (FTS5 only)

---

## Phase 1: Bug fixes & polish (next)

**Goal:** Fix what real usage exposed.

| # | Task | Priority | Effort |
|---|------|----------|--------|
| 1.1 | Fix duplicate transcript IDs (`zed-xxx` vs `xxx`) | P0 | S |
| 1.2 | Add `cargo fmt --check` + `cargo clippy -D warnings` to CI | P0 | S |
| 1.3 | Integration test: service start → ingest → query → stop | P0 | M |
| 1.4 | Unit tests for MCP request handling | P0 | M |
| 1.5 | Unit tests for config load/save + env overrides | P1 | S |
| 1.6 | `transcriptd completion <shell>` (bash/zsh/fish) | P1 | S |
| 1.7 | `transcriptd mcp stop` to kill background service via PID file | P1 | S |

---

## Phase 2: Live observability dashboard (`transcriptd dash`)

**Goal:** btop/lazygit-quality TUI dashboard, connected to running service via SSE.

The previous dashboard was removed in the refactor. Rebuild with these principles:

- **Live, not polled** — subscribe to SSE event stream from the service
- **Service-required** — fail gracefully with hint to run `transcriptd mcp -d`
- **Focused panels** — token usage trend, recent transcripts, top tools, tool failures, ingestion events
- **Keyboard-first** — vim-style navigation, search, filter

Reference implementations to study:

- [`bottom`](https://github.com/ClementTsang/bottom) — system monitor TUI in Rust (excellent layout)
- [`gitui`](https://github.com/extrawurst/gitui) — gitui's component model
- [`lazygit`](https://github.com/jesseduffield/lazygit) — Go but the UX gold standard
- [`zellij`](https://github.com/zellij-org/zellij) — Rust TUI with plugin system
- [Tailscale CLI](https://github.com/tailscale/tailscale/tree/main/cmd/tailscale) — reference for `status`, JSON output, subcommand grouping

| # | Task | Priority | Effort |
|---|------|----------|--------|
| 2.1 | Design panel layout + interaction model | P0 | M |
| 2.2 | SSE client (subscribe to service event stream) | P0 | M |
| 2.3 | Panels: tokens trend, recent, top tools, errors, sources | P0 | L |
| 2.4 | Filter/search panel | P1 | M |
| 2.5 | Color theme matching `bottom`/`gitui` polish level | P1 | S |

---

## Phase 3: Vector embeddings & semantic search

**Goal:** Hybrid retrieval (FTS5 + vector) for MCP — find conceptually similar transcripts, not just keyword matches.

Architecture:

- **`sqlite-vec`** for embedding storage in same SQLite DB (no extra service)
- **Local embedding model** via `fastembed-rs` (BAAI/bge-small-en-v1.5, ~30MB, runs on CPU)
- **Hybrid search** — RRF (Reciprocal Rank Fusion) of FTS5 + vector results
- **Incremental indexing** — embed only on insert/update, batch backfill on first run

| # | Task | Priority | Effort |
|---|------|----------|--------|
| 3.1 | Add `sqlite-vec` extension + schema migration | P0 | M |
| 3.2 | Integrate `fastembed-rs` for local embeddings | P0 | M |
| 3.3 | Embed transcript body on insert (chunked, e.g. by turn) | P0 | M |
| 3.4 | Backfill command: `transcriptd reindex --embeddings` | P0 | S |
| 3.5 | New MCP tool: `semantic_search` | P0 | S |
| 3.6 | Hybrid: extend `search_transcripts` with `mode: hybrid\|fts\|vec` | P1 | M |
| 3.7 | Optional: pluggable embedder (OpenAI, Cohere, local) | P2 | L |

References:

- [`sqlite-vec`](https://github.com/asg017/sqlite-vec) — vector search inside SQLite
- [`fastembed-rs`](https://github.com/Anush008/fastembed-rs) — local embeddings, no Python
- [LlamaIndex hybrid retrieval](https://docs.llamaindex.ai/en/stable/examples/retrievers/relative_score_dist_fusion/) — RRF reference

---

## Phase 4: CLI excellence (gold standard)

**Goal:** Match `docker`, `tailscale`, `gh` for CLI ergonomics.

Patterns to adopt:

- **JSON output everywhere** — every command supports `--format json`
- **Quiet/verbose flags** — `-q`/`-v`/`-vv` like `docker`
- **Inspect command** — `transcriptd inspect <id>` returns full JSON (like `docker inspect`)
- **Filter syntax** — `transcriptd list --filter source=zed --filter tokens>10000`
- **Watch flag** — `transcriptd stats --watch` (like `kubectl get -w`)
- **Pager integration** — auto-pipe long output to `less` when stdout is TTY
- **Colored, aligned tables** — use `comfy-table` or `tabled`
- **Spinners for slow ops** — use `indicatif`

| # | Task | Priority | Effort |
|---|------|----------|--------|
| 4.1 | Adopt `comfy-table` for table output | P1 | M |
| 4.2 | Add `--format json` to every command | P1 | S |
| 4.3 | Add `transcriptd inspect <id>` | P1 | S |
| 4.4 | `--filter` syntax for `list`/`search` | P2 | M |
| 4.5 | Pager integration for long output | P2 | S |
| 4.6 | Shell completions (bash/zsh/fish) | P1 | S |

---

## Phase 5: Distribution

| # | Task | Priority | Effort |
|---|------|----------|--------|
| 5.1 | Publish to crates.io | P0 | XS |
| 5.2 | Homebrew formula automation | P0 | S |
| 5.3 | Test systemd + launchd service files end-to-end | P0 | S |
| 5.4 | Docker image hardening (non-root, distroless) | P1 | S |
| 5.5 | AUR package | P2 | S |
| 5.6 | Nix flake | P2 | M |

---

## Deferred (post-v1.0)

| Task | Reason |
|------|--------|
| Cursor adapter | Need plugin system first |
| Windsurf adapter | Same |
| Plugin system (dynamic loading) | Substantial design work |
| Centralized SaaS / web UI | Different product |
| WebSocket transport | SSE sufficient |

---

## Size Estimates

- **XS:** < 1 hour
- **S:** 1-3 hours
- **M:** 3-8 hours
- **L:** 1-3 days

## Definition of Done

1. Code compiles with zero warnings (`clippy -D warnings`)
2. `cargo fmt` passes
3. Relevant tests pass
4. Docs updated if user-facing
