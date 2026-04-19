use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Constants — eliminate magic strings
// ---------------------------------------------------------------------------

/// Source names used in CLI, DB, and adapters.
pub mod sources {
    pub const ZED: &str = "zed";
    pub const CLAUDE_CODE: &str = "claude-code";
    pub const VSCODE_COPILOT: &str = "vscode-copilot";
    pub const CODEX: &str = "codex";
    pub const CURSOR: &str = "cursor";
    pub const ALL: &str = "all";
    pub const IMPORT: &str = "import";
}

/// Event kinds emitted by the daemon watcher.
pub mod events {
    pub const INGESTED: &str = "ingested";
    pub const WATCHING: &str = "watching";
    pub const ERROR: &str = "error";
}

/// Transcript status values.
pub mod status {
    pub const DONE: &str = "done";
    pub const IN_PROGRESS: &str = "in-progress";
}

// ---------------------------------------------------------------------------
// Shared helpers — deduplicated from adapter crates
// ---------------------------------------------------------------------------

/// UTF-8 safe string truncation. Returns a slice up to `max` bytes,
/// ensuring it doesn't split a multi-byte character.
pub fn safe_truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Human-readable token count formatting: 1234 → "1.2K", 1234567 → "1.2M".
pub fn format_tokens(n: i64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

// ---------------------------------------------------------------------------
// Extractor trait contract
// ---------------------------------------------------------------------------

/// Unified interface all IDE/tool extractors must implement.
/// Each adapter (zed, claude, vscode, cursor, ...) provides one of these.
pub trait TranscriptExtractor: Send + Sync {
    /// Short name used in CLI and DB (e.g. "zed", "claude-code", "vscode-copilot")
    fn name(&self) -> &str;

    /// Platform-aware default path to this source data (DB file or directory).
    /// Returns None if source not installed on this system.
    fn default_source_path(&self) -> Option<PathBuf>;

    /// Count sessions/threads available at source.
    fn count(&self, source: &Path) -> Result<usize>;

    /// Extract all sessions from source, optionally filtering by since timestamp.
    /// Returns Vec of (id, rendered_markdown).
    fn extract_all(&self, source: &Path, since: Option<&str>) -> Result<Vec<(String, String)>>;

    /// Extract a single session by ID from source.
    fn extract_one(&self, source: &Path, id: &str) -> Result<String>;

    /// Paths the daemon should watch for changes (files or directories).
    /// Called with the resolved source path.
    fn watch_paths(&self, source: &Path) -> Vec<PathBuf>;
}

// ---------------------------------------------------------------------------
// Domain models
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transcript {
    pub meta: TranscriptMeta,
    pub turns: Vec<TranscriptTurn>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptMeta {
    pub id: String,
    pub source: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    pub model_provider: String,
    pub model_name: String,
    pub folder_paths: Vec<String>,
    pub branch: Option<String>,
    pub thinking_enabled: Option<bool>,
    pub thread_version: Option<String>,
    // Aggregates (computed from turns)
    pub turns_user: usize,
    pub turns_agent: usize,
    pub turns_total: usize,
    pub tokens_in_total: u64,
    pub tokens_out_total: u64,
    pub tokens_cache_read_total: u64,
    pub tokens_cache_write_total: u64,
    pub tools_used: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptTurn {
    pub number: usize,
    pub role: TurnRole,
    pub content: Vec<ContentBlock>,
    pub token_usage: Option<TurnTokenUsage>,
    pub tools_invoked: Vec<ToolInvocation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TurnRole {
    User,
    Agent,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ContentBlock {
    Text(String),
    Thinking(String),
    ToolUse {
        name: String,
        input_summary: String,
    },
    ToolResult {
        name: String,
        is_error: bool,
        output_summary: String,
    },
    Mention {
        uri: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnTokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInvocation {
    pub name: String,
    pub is_error: bool,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_safe_truncate_ascii() {
        assert_eq!(safe_truncate("hello world", 5), "hello");
        assert_eq!(safe_truncate("hi", 10), "hi");
    }

    #[test]
    fn test_safe_truncate_unicode() {
        // "café" is 5 bytes (é = 2 bytes)
        let s = "café";
        assert_eq!(s.len(), 5);
        // Truncating at 4 would split é, should back up to 3
        assert_eq!(safe_truncate(s, 4), "caf");
        assert_eq!(safe_truncate(s, 5), "café");
    }

    #[test]
    fn test_format_tokens() {
        assert_eq!(format_tokens(0), "0");
        assert_eq!(format_tokens(999), "999");
        assert_eq!(format_tokens(1000), "1.0K");
        assert_eq!(format_tokens(1500), "1.5K");
        assert_eq!(format_tokens(1_000_000), "1.0M");
        assert_eq!(format_tokens(2_500_000), "2.5M");
    }
}
