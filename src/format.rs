use std::collections::HashMap;

use rusqlite::Connection;
use transcriptd_store::TranscriptRecord;

pub fn format_tokens(n: i64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

pub fn record_to_json(rec: &TranscriptRecord) -> String {
    serde_json::json!({
        "id": rec.id,
        "source": rec.source,
        "title": rec.title,
        "status": rec.status,
        "model_provider": rec.model_provider,
        "model_name": rec.model_name,
        "turns_user": rec.turns_user,
        "turns_agent": rec.turns_agent,
        "turns_total": rec.turns_total,
        "tokens_in": rec.tokens_in,
        "tokens_out": rec.tokens_out,
        "tokens_cache_read": rec.tokens_cache_read,
        "tokens_cache_write": rec.tokens_cache_write,
        "word_count": rec.word_count,
        "thinking_enabled": rec.thinking_enabled,
        "tags": rec.tags,
        "tools_used": rec.tools_used,
        "folder_paths": rec.folder_paths,
        "branch": rec.branch,
        "thread_version": rec.thread_version,
        "created_at": rec.created_at,
        "updated_at": rec.updated_at,
    })
    .to_string()
}

pub fn records_to_json(recs: &[TranscriptRecord]) -> String {
    let arr: Vec<serde_json::Value> = recs
        .iter()
        .map(|r| {
            serde_json::json!({
                "id": r.id,
                "source": r.source,
                "title": r.title,
                "turns_total": r.turns_total,
                "tokens_in": r.tokens_in,
                "tokens_out": r.tokens_out,
                "created_at": r.created_at,
            })
        })
        .collect();
    serde_json::to_string_pretty(&arr).unwrap_or_else(|_| "[]".to_string())
}

/// Look up rowid for a set of transcript IDs (used as short IDs).
pub fn get_rowids(conn: &Connection, ids: &[&str]) -> HashMap<String, i64> {
    let mut map = HashMap::new();
    for id in ids {
        if let Ok(rowid) = conn.query_row(
            "SELECT rowid FROM transcripts WHERE id = ?1",
            [id],
            |row| row.get::<_, i64>(0),
        ) {
            map.insert(id.to_string(), rowid);
        }
    }
    map
}
