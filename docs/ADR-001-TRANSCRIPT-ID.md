# ADR-001: Canonical Transcript ID Format

**Status:** Accepted
**Date:** 2026-04-19
**Decision Makers:** @skanderbenabdelmalak

## Context

Transcripts are ingested from multiple AI coding tools (Zed, Claude Code, VS Code
Copilot, Codex, Cursor). Each source uses a different native identifier format
(UUIDs, composite strings, file stems). During development, the ID format evolved,
creating **duplicate rows** where the same session exists under two different IDs:

| Source | Legacy ID | Current (wrong) ID |
|---|---|---|
| zed | `a5fa027c-4e51-...` | `zed-a5fa027c-4e51-...` |
| claude-code | `claude-55d40bc6-...` | `claude-code-claude-55d40bc6-...` |
| vscode-copilot | `vscode-abc123-...` | `vscode-copilot-vscode-abc123-...` |

This results in inflated counts, incorrect stats, and confusing search results.

## Decision

### Rule: `{source}-{native_id}`

Every transcript ID follows the format:

```
{source}-{native_id}
```

Where:

- **`source`** is the extractor's canonical name: `zed`, `claude-code`,
  `vscode-copilot`, `codex`, `cursor`
- **`native_id`** is the unique session identifier from the source tool,
  stripped of any prefixes, extensions, or path components

### Examples

| Source | Native ID | Canonical ID |
|---|---|---|
| zed | `a5fa027c-4e51-429e-b37e-...` | `zed-a5fa027c-4e51-429e-b37e-...` |
| claude-code | `55d40bc6-2e9c-...` | `claude-code-55d40bc6-2e9c-...` |
| vscode-copilot | `16919133-8393-4ba7-...` | `vscode-copilot-16919133-8393-4ba7-...` |
| codex | `rollout-2025-10-13T08-50-11-...` | `codex-rollout-2025-10-13T08-50-11-...` |
| cursor | `abc123-...` | `cursor-abc123-...` |

### Invariants (LOCKED — do not change in future releases)

1. **Single source of truth**: `canonical_id(raw_id, source)` in `src/parse.rs`
   is the ONLY function that produces canonical IDs.
2. **Idempotent**: `canonical_id(canonical_id(x, s), s) == canonical_id(x, s)`
3. **Extractors return raw native IDs**: No prefix, no `.md` extension, no path.
4. **Primary key**: The canonical ID is the SQLite `PRIMARY KEY` in the
   `transcripts` table. Upsert semantics ensure re-ingestion updates, not duplicates.
5. **Self-healing**: Every `db ingest` run performs auto-dedupe after insertion,
   cleaning up any legacy-format rows that map to the same canonical ID.

## Implementation

### Extractor contract

Each `TranscriptExtractor::extract_all()` returns `Vec<(String, String)>` where
the first element is the **raw native ID** (no prefix, no extension):

```
Zed:    "a5fa027c-4e51-429e-..."          (thread UUID from Zed SQLite)
Claude: "55d40bc6-2e9c-..."              (JSONL filename stem)
VSCode: "16919133-8393-4ba7-..."         (session ID from workspace storage)
Codex:  "rollout-2025-10-13T08-50-..."   (JSONL filename stem)
Cursor: "abc123-..."                     (composer_id from Cursor SQLite)
```

### canonical_id()

```rust
pub fn canonical_id(raw: &str, source: &str) -> String {
    let stem = raw.rsplit('/').next().unwrap_or(raw);
    let stem = stem.strip_suffix(".md").unwrap_or(stem);
    let prefix = format!("{source}-");
    let mut body = stem;
    // Strip source prefix if already present (idempotent)
    if let Some(rest) = body.strip_prefix(&prefix) {
        body = rest;
    }
    // Handle legacy wrong prefixes from older extractors
    body = match source {
        "claude-code" => body.strip_prefix("claude-").unwrap_or(body),
        "vscode-copilot" => body.strip_prefix("vscode-").unwrap_or(body),
        _ => body,
    };
    format!("{source}-{body}")
}
```

### Deduplication

`dedupe_transcripts()` maps every existing ID through `canonical_id()` to find
groups that collapse to the same canonical form. Within each group, the row with
the richest data (longest body_text, most tokens) is kept; others are deleted.

## Consequences

- All existing duplicates will be cleaned up on next `db ingest` or `db dedupe`.
- Future source additions must follow the `{source}-{native_id}` rule.
- Old bookmarks/references using legacy IDs will break (acceptable for pre-release).
- The `#seq` short-ID system (rowid-based) is unaffected.
