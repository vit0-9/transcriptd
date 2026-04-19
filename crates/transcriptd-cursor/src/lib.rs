use anyhow::{Context, Result};
use chrono::TimeZone;
use rusqlite::{Connection, OpenFlags};
use serde::Deserialize;
use std::path::{Path, PathBuf};

// ── helpers ──────────────────────────────────────────────────────────

fn ms_to_iso(ms: i64) -> String {
    chrono::Utc
        .timestamp_millis_opt(ms)
        .single()
        .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string())
}

fn safe_truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

// ── data types ───────────────────────────────────────────────────────

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct ComposerHeaders {
    all_composers: Vec<ComposerHeader>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct ComposerHeader {
    composer_id: String,
    last_updated_at: Option<i64>,
    created_at: Option<i64>,
    unified_mode: Option<String>,
    is_archived: Option<bool>,
    is_draft: Option<bool>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct ComposerData {
    composer_id: String,
    full_conversation_headers_only: Option<Vec<BubbleHeader>>,
    status: Option<String>,
    created_at: Option<i64>,
    last_updated_at: Option<i64>,
    unified_mode: Option<String>,
    name: Option<String>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct BubbleHeader {
    bubble_id: String,
    #[serde(rename = "type")]
    bubble_type: u32, // 1 = user, 2 = assistant
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct BubbleData {
    #[serde(rename = "type")]
    bubble_type: Option<u32>,
    text: Option<String>,
    raw_text: Option<String>,
    token_count: Option<TokenCount>,
    bubble_id: Option<String>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct TokenCount {
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
}

// ── parsed session ───────────────────────────────────────────────────

struct Session {
    id: String,
    title: String,
    mode: String,
    created_at: String,
    updated_at: String,
    turns: Vec<Turn>,
}

struct Turn {
    role: &'static str,
    content: String,
}

// ── public API ───────────────────────────────────────────────────────

/// Default Cursor globalStorage dir per platform.
pub fn default_cursor_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        dirs::home_dir().map(|h| h.join("Library/Application Support/Cursor/User/globalStorage"))
    }
    #[cfg(target_os = "linux")]
    {
        dirs::home_dir().map(|h| h.join(".config/Cursor/User/globalStorage"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

pub fn count_sessions(cursor_dir: &str) -> Result<usize> {
    let headers = read_composer_headers(Path::new(cursor_dir))?;
    Ok(headers
        .iter()
        .filter(|h| !h.is_draft.unwrap_or(false))
        .count())
}

pub fn extract_all(cursor_dir: &str, since: Option<&str>) -> Result<Vec<(String, String)>> {
    let dir = Path::new(cursor_dir);
    let headers = read_composer_headers(dir)?;

    let since_ms: Option<i64> = since.and_then(|s| {
        chrono::DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|dt| dt.timestamp_millis())
    });

    let db_path = dir.join("state.vscdb");
    let conn = Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("opening {}", db_path.display()))?;

    let mut results = Vec::new();

    for header in &headers {
        if header.is_draft.unwrap_or(false) {
            continue;
        }

        if let Some(cutoff) = since_ms {
            if header.last_updated_at.is_some_and(|ts| ts < cutoff) {
                continue;
            }
        }

        match build_session(&conn, header) {
            Ok(session) => {
                let md = render_markdown(&session);
                results.push((session.id.clone(), md));
            }
            Err(e) => {
                eprintln!("SKIP cursor {}: {e}", header.composer_id);
            }
        }
    }

    results.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(results)
}

pub fn extract_one_by_id(cursor_dir: &str, id: &str) -> Result<String> {
    let dir = Path::new(cursor_dir);
    let db_path = dir.join("state.vscdb");
    let conn = Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;

    let headers = read_composer_headers(dir)?;
    let header = headers
        .iter()
        .find(|h| h.composer_id == id)
        .ok_or_else(|| anyhow::anyhow!("cursor session {id} not found"))?;

    let session = build_session(&conn, header)?;
    Ok(render_markdown(&session))
}

// ── internal ─────────────────────────────────────────────────────────

fn read_composer_headers(dir: &Path) -> Result<Vec<ComposerHeader>> {
    let db_path = dir.join("state.vscdb");
    if !db_path.exists() {
        return Ok(Vec::new());
    }
    let conn = Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let raw: String = conn
        .query_row(
            "SELECT value FROM ItemTable WHERE key = 'composer.composerHeaders'",
            [],
            |row| row.get(0),
        )
        .unwrap_or_default();

    if raw.is_empty() {
        return Ok(Vec::new());
    }
    let headers: ComposerHeaders = serde_json::from_str(&raw)?;
    Ok(headers.all_composers)
}

fn build_session(conn: &Connection, header: &ComposerHeader) -> Result<Session> {
    let cid = &header.composer_id;

    // Read composerData from cursorDiskKV
    let key = format!("composerData:{cid}");
    let raw: String = conn
        .query_row(
            "SELECT value FROM cursorDiskKV WHERE key = ?",
            [&key],
            |row| row.get(0),
        )
        .with_context(|| format!("reading composerData for {cid}"))?;

    let data: ComposerData = serde_json::from_str(&raw)?;

    let bubble_headers = data.full_conversation_headers_only.unwrap_or_default();

    let mut turns = Vec::new();

    for bh in &bubble_headers {
        let bubble_key = format!("bubbleId:{cid}:{}", bh.bubble_id);
        let bubble_raw: String = match conn.query_row(
            "SELECT value FROM cursorDiskKV WHERE key = ?",
            [&bubble_key],
            |row| row.get(0),
        ) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let bubble: BubbleData = match serde_json::from_str(&bubble_raw) {
            Ok(b) => b,
            Err(_) => continue,
        };

        let role = match bubble.bubble_type.unwrap_or(bh.bubble_type) {
            1 => "user",
            2 => "agent",
            _ => "system",
        };

        let content = bubble.text.or(bubble.raw_text).unwrap_or_default();

        turns.push(Turn { role, content });
    }

    let created_ms = data.created_at.or(header.created_at).unwrap_or(0);
    let updated_ms = data
        .last_updated_at
        .or(header.last_updated_at)
        .unwrap_or(created_ms);

    let title = data
        .name
        .filter(|n| !n.is_empty())
        .or_else(|| {
            turns.iter().find(|t| t.role == "user").map(|t| {
                let s = t.content.trim();
                if s.len() > 80 {
                    format!("{}...", safe_truncate(s, 80))
                } else {
                    s.to_string()
                }
            })
        })
        .unwrap_or_else(|| "Untitled".to_string());

    Ok(Session {
        id: cid.clone(),
        title,
        mode: data
            .unified_mode
            .or(header.unified_mode.clone())
            .unwrap_or_else(|| "agent".to_string()),
        created_at: ms_to_iso(created_ms),
        updated_at: ms_to_iso(updated_ms),
        turns,
    })
}

fn render_markdown(session: &Session) -> String {
    let turns_user = session.turns.iter().filter(|t| t.role == "user").count();
    let turns_agent = session.turns.iter().filter(|t| t.role == "agent").count();
    let turns_total = turns_user + turns_agent;

    let title_esc = session.title.replace('"', "\\\"");

    let mut md = format!(
        "---\n\
         schema_version: 1\n\
         id: cursor-{}\n\
         type: transcript\n\
         source: cursor\n\
         status: done\n\
         title: \"{title_esc}\"\n\
         created_at: {}\n\
         updated_at: {}\n\
         mode: {}\n\
         turns_user: {turns_user}\n\
         turns_agent: {turns_agent}\n\
         turns_total: {turns_total}\n\
         tools_used: []\n\
         ---\n\n\
         # {title_esc}\n\n",
        session.id, session.created_at, session.updated_at, session.mode
    );

    let mut tn = 0u32;
    for turn in &session.turns {
        tn += 1;
        md.push_str(&format!("## Turn {tn} -- {}\n\n", turn.role));
        if !turn.content.is_empty() {
            md.push_str(&turn.content);
            if !turn.content.ends_with('\n') {
                md.push('\n');
            }
            md.push('\n');
        }
    }

    md
}

// -- TranscriptExtractor trait impl ------------------------------------------

use transcriptd_core::TranscriptExtractor;

pub struct CursorExtractor;

impl TranscriptExtractor for CursorExtractor {
    fn name(&self) -> &str {
        "cursor"
    }

    fn default_source_path(&self) -> Option<PathBuf> {
        default_cursor_dir()
    }

    fn count(&self, source: &Path) -> Result<usize> {
        count_sessions(&source.to_string_lossy())
    }

    fn extract_all(&self, source: &Path, since: Option<&str>) -> Result<Vec<(String, String)>> {
        extract_all(&source.to_string_lossy(), since)
    }

    fn extract_one(&self, source: &Path, id: &str) -> Result<String> {
        extract_one_by_id(&source.to_string_lossy(), id)
    }

    fn watch_paths(&self, source: &Path) -> Vec<PathBuf> {
        vec![source.join("state.vscdb")]
    }
}
