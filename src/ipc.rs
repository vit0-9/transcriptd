use serde::{Deserialize, Serialize};

/// Messages from daemon to clients (JSON-lines over Unix socket)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ServerMsg {
    /// Full stats snapshot
    #[serde(rename = "stats")]
    Stats {
        total_transcripts: i64,
        total_turns: i64,
        total_tokens_in: i64,
        total_tokens_out: i64,
        sources: Vec<(String, i64)>,
        top_tools: Vec<(String, i64)>,
    },
    /// An ingest event occurred
    #[serde(rename = "event")]
    Event {
        kind: String, // "ingested", "watching", "error"
        source: String,
        detail: String,
        timestamp: String,
    },
    /// Recent transcripts snapshot
    #[serde(rename = "recent")]
    Recent { transcripts: Vec<RecentEntry> },
    /// Daily token data for sparkline
    #[serde(rename = "daily")]
    Daily { entries: Vec<DailyEntry> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentEntry {
    pub id: String,
    pub source: String,
    pub title: String,
    pub turns_total: i32,
    pub tokens_in: i64,
    pub tokens_out: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyEntry {
    pub date: String,
    pub tokens_in: i64,
    pub tokens_out: i64,
    pub sessions: i64,
}

/// Serialize a ServerMsg as a JSON line (with trailing newline)
pub fn encode(msg: &ServerMsg) -> String {
    let mut s = serde_json::to_string(msg).unwrap_or_default();
    s.push('\n');
    s
}

/// Deserialize a JSON line into a ServerMsg
#[allow(dead_code)]
pub fn decode(line: &str) -> Option<ServerMsg> {
    serde_json::from_str(line.trim()).ok()
}
