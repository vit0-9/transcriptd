use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension};

// -- Public data types -------------------------------------------------------

#[derive(Debug, Clone)]
pub struct TranscriptRecord {
    pub id: String,
    pub source: String,
    pub title: String,
    pub status: String,
    pub model_provider: String,
    pub model_name: String,
    pub turns_user: i32,
    pub turns_agent: i32,
    pub turns_total: i32,
    pub tokens_in: i64,
    pub tokens_out: i64,
    pub tokens_cache_read: i64,
    pub tokens_cache_write: i64,
    pub word_count: i32,
    pub thinking_enabled: bool,
    pub tags: Vec<String>,
    pub tools_used: Vec<String>,
    pub folder_paths: Vec<String>,
    pub branch: Option<String>,
    pub thread_version: Option<String>,
    pub body_text: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct TurnRecord {
    pub transcript_id: String,
    pub turn_number: i32,
    pub role: String,
    pub tokens_in: i64,
    pub tokens_out: i64,
    pub tokens_cache_read: i64,
    pub tokens_cache_write: i64,
    pub content_length: i32,
    pub has_thinking: bool,
    pub tools: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct StoreStats {
    pub total_transcripts: i64,
    pub total_turns: i64,
    pub total_tokens_in: i64,
    pub total_tokens_out: i64,
    pub sources: Vec<(String, i64)>,
    pub top_tools: Vec<(String, i64)>,
}

// -- Schema ------------------------------------------------------------------

const SCHEMA_V1: &str = "
PRAGMA journal_mode=WAL;
PRAGMA foreign_keys=ON;

CREATE TABLE IF NOT EXISTS transcripts (
    id TEXT PRIMARY KEY,
    source TEXT NOT NULL,
    title TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'done',
    model_provider TEXT,
    model_name TEXT,
    turns_user INTEGER DEFAULT 0,
    turns_agent INTEGER DEFAULT 0,
    turns_total INTEGER DEFAULT 0,
    tokens_in INTEGER DEFAULT 0,
    tokens_out INTEGER DEFAULT 0,
    tokens_cache_read INTEGER DEFAULT 0,
    tokens_cache_write INTEGER DEFAULT 0,
    word_count INTEGER DEFAULT 0,
    thinking_enabled INTEGER DEFAULT 0,
    tags TEXT DEFAULT '[]',
    tools_used TEXT DEFAULT '[]',
    folder_paths TEXT DEFAULT '[]',
    branch TEXT,
    thread_version TEXT,
    body_text TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    indexed_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS turns (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    transcript_id TEXT NOT NULL REFERENCES transcripts(id) ON DELETE CASCADE,
    turn_number INTEGER NOT NULL,
    role TEXT NOT NULL,
    tokens_in INTEGER DEFAULT 0,
    tokens_out INTEGER DEFAULT 0,
    tokens_cache_read INTEGER DEFAULT 0,
    tokens_cache_write INTEGER DEFAULT 0,
    tokens_total INTEGER DEFAULT 0,
    content_length INTEGER DEFAULT 0,
    has_thinking INTEGER DEFAULT 0,
    tool_count INTEGER DEFAULT 0,
    tools TEXT DEFAULT '[]',
    UNIQUE(transcript_id, turn_number)
);

CREATE TABLE IF NOT EXISTS tool_usage (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    transcript_id TEXT NOT NULL REFERENCES transcripts(id) ON DELETE CASCADE,
    tool_name TEXT NOT NULL,
    invocation_count INTEGER DEFAULT 1,
    UNIQUE(transcript_id, tool_name)
);

CREATE TABLE IF NOT EXISTS secrets_detected (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    transcript_id TEXT NOT NULL REFERENCES transcripts(id) ON DELETE CASCADE,
    turn_number INTEGER,
    secret_type TEXT NOT NULL,
    secret_hash TEXT NOT NULL,
    line_hint TEXT,
    detected_at TEXT NOT NULL DEFAULT (datetime('now')),
    resolved INTEGER DEFAULT 0,
    resolved_at TEXT
);

CREATE VIRTUAL TABLE IF NOT EXISTS transcripts_fts USING fts5(
    title, body_text, tools_used, tags,
    content=transcripts,
    content_rowid=rowid
);

CREATE TRIGGER IF NOT EXISTS transcripts_ai AFTER INSERT ON transcripts BEGIN
    INSERT INTO transcripts_fts(rowid, title, body_text, tools_used, tags)
    VALUES (new.rowid, new.title, new.body_text, new.tools_used, new.tags);
END;

CREATE TRIGGER IF NOT EXISTS transcripts_ad AFTER DELETE ON transcripts BEGIN
    INSERT INTO transcripts_fts(transcripts_fts, rowid, title, body_text, tools_used, tags)
    VALUES('delete', old.rowid, old.title, old.body_text, old.tools_used, old.tags);
END;

CREATE TRIGGER IF NOT EXISTS transcripts_au AFTER UPDATE ON transcripts BEGIN
    INSERT INTO transcripts_fts(transcripts_fts, rowid, title, body_text, tools_used, tags)
    VALUES('delete', old.rowid, old.title, old.body_text, old.tools_used, old.tags);
    INSERT INTO transcripts_fts(rowid, title, body_text, tools_used, tags)
    VALUES (new.rowid, new.title, new.body_text, new.tools_used, new.tags);
END;

CREATE INDEX IF NOT EXISTS idx_transcripts_source ON transcripts(source);
CREATE INDEX IF NOT EXISTS idx_transcripts_created ON transcripts(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_transcripts_model ON transcripts(model_provider);
CREATE INDEX IF NOT EXISTS idx_turns_transcript ON turns(transcript_id);
CREATE INDEX IF NOT EXISTS idx_tool_usage_transcript ON tool_usage(transcript_id);
CREATE INDEX IF NOT EXISTS idx_tool_usage_name ON tool_usage(tool_name);
CREATE INDEX IF NOT EXISTS idx_secrets_transcript ON secrets_detected(transcript_id);
CREATE INDEX IF NOT EXISTS idx_secrets_unresolved ON secrets_detected(resolved) WHERE resolved = 0;

CREATE TABLE IF NOT EXISTS schema_version (version INTEGER PRIMARY KEY);
INSERT OR IGNORE INTO schema_version VALUES (1);
";

const SCHEMA_V2: &str = "
-- V2: Add tool_invocations for per-call forensics, mode/originator on transcripts
CREATE TABLE IF NOT EXISTS tool_invocations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    transcript_id TEXT NOT NULL REFERENCES transcripts(id) ON DELETE CASCADE,
    turn_number INTEGER,
    tool_name TEXT NOT NULL,
    call_id TEXT,
    is_error INTEGER DEFAULT 0,
    error_summary TEXT,
    invoked_at TEXT
);

ALTER TABLE transcripts ADD COLUMN mode TEXT DEFAULT 'agent';
ALTER TABLE transcripts ADD COLUMN originator TEXT;

CREATE INDEX IF NOT EXISTS idx_tool_inv_transcript ON tool_invocations(transcript_id);
CREATE INDEX IF NOT EXISTS idx_tool_inv_name ON tool_invocations(tool_name);
CREATE INDEX IF NOT EXISTS idx_tool_inv_errors ON tool_invocations(is_error) WHERE is_error = 1;

INSERT OR REPLACE INTO schema_version VALUES (2);
";

// -- Database initialization -------------------------------------------------

pub fn init_db(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)
        .with_context(|| format!("failed to open database at {}", path.display()))?;

    let current_version = conn
        .query_row(
            "SELECT version FROM schema_version ORDER BY version DESC LIMIT 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .unwrap_or(None)
        .unwrap_or(0);

    if current_version < 1 {
        conn.execute_batch(SCHEMA_V1)
            .context("failed to run schema migration v1")?;
    }

    if current_version < 2 {
        conn.execute_batch(SCHEMA_V2)
            .context("failed to run schema migration v2")?;
    }

    Ok(conn)
}

// -- Helpers -----------------------------------------------------------------

fn vec_to_json(v: &[String]) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "[]".to_string())
}

fn json_to_vec(s: &str) -> Vec<String> {
    serde_json::from_str(s).unwrap_or_default()
}

fn row_to_transcript(row: &rusqlite::Row<'_>) -> rusqlite::Result<TranscriptRecord> {
    Ok(TranscriptRecord {
        id: row.get("id")?,
        source: row.get("source")?,
        title: row.get("title")?,
        status: row.get("status")?,
        model_provider: row
            .get::<_, Option<String>>("model_provider")?
            .unwrap_or_default(),
        model_name: row
            .get::<_, Option<String>>("model_name")?
            .unwrap_or_default(),
        turns_user: row.get("turns_user")?,
        turns_agent: row.get("turns_agent")?,
        turns_total: row.get("turns_total")?,
        tokens_in: row.get("tokens_in")?,
        tokens_out: row.get("tokens_out")?,
        tokens_cache_read: row.get("tokens_cache_read")?,
        tokens_cache_write: row.get("tokens_cache_write")?,
        word_count: row.get("word_count")?,
        thinking_enabled: row.get::<_, i32>("thinking_enabled")? != 0,
        tags: json_to_vec(
            &row.get::<_, String>("tags")
                .unwrap_or_else(|_| "[]".to_string()),
        ),
        tools_used: json_to_vec(
            &row.get::<_, String>("tools_used")
                .unwrap_or_else(|_| "[]".to_string()),
        ),
        folder_paths: json_to_vec(
            &row.get::<_, String>("folder_paths")
                .unwrap_or_else(|_| "[]".to_string()),
        ),
        branch: row.get("branch")?,
        thread_version: row.get("thread_version")?,
        body_text: row
            .get::<_, Option<String>>("body_text")?
            .unwrap_or_default(),
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

// -- Upsert transcript -------------------------------------------------------

pub fn upsert_transcript(conn: &Connection, rec: &TranscriptRecord) -> Result<()> {
    conn.execute(
        "INSERT INTO transcripts (
            id, source, title, status, model_provider, model_name,
            turns_user, turns_agent, turns_total,
            tokens_in, tokens_out, tokens_cache_read, tokens_cache_write,
            word_count, thinking_enabled,
            tags, tools_used, folder_paths,
            branch, thread_version, body_text,
            created_at, updated_at
        ) VALUES (
            :id, :source, :title, :status, :model_provider, :model_name,
            :turns_user, :turns_agent, :turns_total,
            :tokens_in, :tokens_out, :tokens_cache_read, :tokens_cache_write,
            :word_count, :thinking_enabled,
            :tags, :tools_used, :folder_paths,
            :branch, :thread_version, :body_text,
            :created_at, :updated_at
        ) ON CONFLICT(id) DO UPDATE SET
            source = excluded.source,
            title = excluded.title,
            status = excluded.status,
            model_provider = excluded.model_provider,
            model_name = excluded.model_name,
            turns_user = excluded.turns_user,
            turns_agent = excluded.turns_agent,
            turns_total = excluded.turns_total,
            tokens_in = excluded.tokens_in,
            tokens_out = excluded.tokens_out,
            tokens_cache_read = excluded.tokens_cache_read,
            tokens_cache_write = excluded.tokens_cache_write,
            word_count = excluded.word_count,
            thinking_enabled = excluded.thinking_enabled,
            tags = excluded.tags,
            tools_used = excluded.tools_used,
            folder_paths = excluded.folder_paths,
            branch = excluded.branch,
            thread_version = excluded.thread_version,
            body_text = excluded.body_text,
            created_at = excluded.created_at,
            updated_at = excluded.updated_at,
            indexed_at = datetime('now')",
        rusqlite::named_params! {
            ":id": rec.id,
            ":source": rec.source,
            ":title": rec.title,
            ":status": rec.status,
            ":model_provider": rec.model_provider,
            ":model_name": rec.model_name,
            ":turns_user": rec.turns_user,
            ":turns_agent": rec.turns_agent,
            ":turns_total": rec.turns_total,
            ":tokens_in": rec.tokens_in,
            ":tokens_out": rec.tokens_out,
            ":tokens_cache_read": rec.tokens_cache_read,
            ":tokens_cache_write": rec.tokens_cache_write,
            ":word_count": rec.word_count,
            ":thinking_enabled": rec.thinking_enabled as i32,
            ":tags": vec_to_json(&rec.tags),
            ":tools_used": vec_to_json(&rec.tools_used),
            ":folder_paths": vec_to_json(&rec.folder_paths),
            ":branch": rec.branch,
            ":thread_version": rec.thread_version,
            ":body_text": rec.body_text,
            ":created_at": rec.created_at,
            ":updated_at": rec.updated_at,
        },
    )
    .context("failed to upsert transcript")?;

    Ok(())
}

// -- Upsert turns ------------------------------------------------------------

pub fn upsert_turns(conn: &Connection, transcript_id: &str, turns: &[TurnRecord]) -> Result<()> {
    let tx = conn
        .unchecked_transaction()
        .context("failed to begin transaction")?;

    tx.execute(
        "DELETE FROM turns WHERE transcript_id = :tid",
        rusqlite::named_params! { ":tid": transcript_id },
    )?;

    for t in turns {
        let tokens_total = t.tokens_in + t.tokens_out + t.tokens_cache_read + t.tokens_cache_write;
        let tool_count = t.tools.len() as i32;
        tx.execute(
            "INSERT INTO turns (
                transcript_id, turn_number, role,
                tokens_in, tokens_out, tokens_cache_read, tokens_cache_write, tokens_total,
                content_length, has_thinking, tool_count, tools
            ) VALUES (
                :transcript_id, :turn_number, :role,
                :tokens_in, :tokens_out, :tokens_cache_read, :tokens_cache_write, :tokens_total,
                :content_length, :has_thinking, :tool_count, :tools
            )",
            rusqlite::named_params! {
                ":transcript_id": t.transcript_id,
                ":turn_number": t.turn_number,
                ":role": t.role,
                ":tokens_in": t.tokens_in,
                ":tokens_out": t.tokens_out,
                ":tokens_cache_read": t.tokens_cache_read,
                ":tokens_cache_write": t.tokens_cache_write,
                ":tokens_total": tokens_total,
                ":content_length": t.content_length,
                ":has_thinking": t.has_thinking as i32,
                ":tool_count": tool_count,
                ":tools": vec_to_json(&t.tools),
            },
        )?;
    }

    tx.commit().context("failed to commit turns upsert")?;
    Ok(())
}

// -- Upsert tool usage -------------------------------------------------------

pub fn upsert_tool_usage(conn: &Connection, transcript_id: &str, tools: &[String]) -> Result<()> {
    let tx = conn
        .unchecked_transaction()
        .context("failed to begin transaction")?;

    tx.execute(
        "DELETE FROM tool_usage WHERE transcript_id = :tid",
        rusqlite::named_params! { ":tid": transcript_id },
    )?;

    let mut counts: std::collections::HashMap<&str, i32> = std::collections::HashMap::new();
    for tool in tools {
        *counts.entry(tool.as_str()).or_insert(0) += 1;
    }

    for (name, count) in &counts {
        tx.execute(
            "INSERT INTO tool_usage (transcript_id, tool_name, invocation_count)
             VALUES (:tid, :name, :count)",
            rusqlite::named_params! {
                ":tid": transcript_id,
                ":name": name,
                ":count": count,
            },
        )?;
    }

    tx.commit().context("failed to commit tool_usage upsert")?;
    Ok(())
}

// -- FTS5 search -------------------------------------------------------------

pub fn search(
    conn: &Connection,
    query: &str,
    limit: usize,
    offset: usize,
) -> Result<Vec<TranscriptRecord>> {
    let mut stmt = conn.prepare(
        "SELECT t.*
         FROM transcripts t
         JOIN transcripts_fts f ON t.rowid = f.rowid
         WHERE transcripts_fts MATCH :query
         ORDER BY rank
         LIMIT :limit OFFSET :offset",
    )?;

    let rows = stmt.query_map(
        rusqlite::named_params! {
            ":query": query,
            ":limit": limit as i64,
            ":offset": offset as i64,
        },
        row_to_transcript,
    )?;

    let mut results = Vec::new();
    for row in rows {
        results.push(row?);
    }
    Ok(results)
}

// -- Get single transcript ---------------------------------------------------

pub fn get_transcript(conn: &Connection, id: &str) -> Result<Option<TranscriptRecord>> {
    let mut stmt = conn.prepare("SELECT * FROM transcripts WHERE id = :id")?;

    let result = stmt
        .query_row(rusqlite::named_params! { ":id": id }, row_to_transcript)
        .optional()?;

    Ok(result)
}

// -- Store stats -------------------------------------------------------------

pub fn get_stats(conn: &Connection) -> Result<StoreStats> {
    let total_transcripts: i64 =
        conn.query_row("SELECT COUNT(*) FROM transcripts", [], |r| r.get(0))?;

    let total_turns: i64 = conn.query_row("SELECT COUNT(*) FROM turns", [], |r| r.get(0))?;

    let total_tokens_in: i64 = conn.query_row(
        "SELECT COALESCE(SUM(tokens_in), 0) FROM transcripts",
        [],
        |r| r.get(0),
    )?;

    let total_tokens_out: i64 = conn.query_row(
        "SELECT COALESCE(SUM(tokens_out), 0) FROM transcripts",
        [],
        |r| r.get(0),
    )?;

    let mut stmt = conn.prepare(
        "SELECT source, COUNT(*) as cnt FROM transcripts GROUP BY source ORDER BY cnt DESC",
    )?;
    let sources: Vec<(String, i64)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .filter_map(|r| r.ok())
        .collect();

    let mut stmt = conn.prepare(
        "SELECT tool_name, SUM(invocation_count) as total
         FROM tool_usage GROUP BY tool_name ORDER BY total DESC LIMIT 20",
    )?;
    let top_tools: Vec<(String, i64)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .filter_map(|r| r.ok())
        .collect();

    Ok(StoreStats {
        total_transcripts,
        total_turns,
        total_tokens_in,
        total_tokens_out,
        sources,
        top_tools,
    })
}

// -- List transcripts --------------------------------------------------------

pub fn list_transcripts(
    conn: &Connection,
    source: Option<&str>,
    limit: usize,
    offset: usize,
    sort: &str,
) -> Result<Vec<TranscriptRecord>> {
    let order_clause = match sort {
        "tokens_in" => "tokens_in DESC",
        "turns_total" => "turns_total DESC",
        _ => "created_at DESC",
    };

    let sql = if source.is_some() {
        format!(
            "SELECT * FROM transcripts WHERE source = :source ORDER BY {} LIMIT :limit OFFSET :offset",
            order_clause
        )
    } else {
        format!(
            "SELECT * FROM transcripts ORDER BY {} LIMIT :limit OFFSET :offset",
            order_clause
        )
    };

    let mut stmt = conn.prepare(&sql)?;

    let rows = if let Some(src) = source {
        stmt.query_map(
            rusqlite::named_params! {
                ":source": src,
                ":limit": limit as i64,
                ":offset": offset as i64,
            },
            row_to_transcript,
        )?
    } else {
        stmt.query_map(
            rusqlite::named_params! {
                ":limit": limit as i64,
                ":offset": offset as i64,
            },
            row_to_transcript,
        )?
    };

    let mut results = Vec::new();
    for row in rows {
        results.push(row?);
    }
    Ok(results)
}

// -- Dashboard helpers -------------------------------------------------------

/// Recent transcripts (for dashboard recent list)
pub fn recent_transcripts(conn: &Connection, limit: usize) -> Result<Vec<TranscriptRecord>> {
    list_transcripts(conn, None, limit, 0, "created_at")
}

/// Lightweight recent transcripts for the dashboard (no body_text).
pub fn recent_transcripts_lite(conn: &Connection, limit: usize) -> Result<Vec<TranscriptRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, source, title, status, model_provider, model_name,
                turns_user, turns_agent, turns_total,
                tokens_in, tokens_out, tokens_cache_read, tokens_cache_write,
                word_count, thinking_enabled, tags, tools_used, folder_paths,
                branch, thread_version, created_at, updated_at
         FROM transcripts
         ORDER BY created_at DESC
         LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit as i64], |row| {
        Ok(TranscriptRecord {
            id: row.get(0)?,
            source: row.get(1)?,
            title: row.get(2)?,
            status: row.get(3)?,
            model_provider: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
            model_name: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
            turns_user: row.get(6)?,
            turns_agent: row.get(7)?,
            turns_total: row.get(8)?,
            tokens_in: row.get(9)?,
            tokens_out: row.get(10)?,
            tokens_cache_read: row.get(11)?,
            tokens_cache_write: row.get(12)?,
            word_count: row.get(13)?,
            thinking_enabled: row.get::<_, i32>(14)? != 0,
            tags: json_to_vec(
                &row.get::<_, String>(15)
                    .unwrap_or_else(|_| "[]".to_string()),
            ),
            tools_used: json_to_vec(
                &row.get::<_, String>(16)
                    .unwrap_or_else(|_| "[]".to_string()),
            ),
            folder_paths: json_to_vec(
                &row.get::<_, String>(17)
                    .unwrap_or_else(|_| "[]".to_string()),
            ),
            branch: row.get(18)?,
            thread_version: row.get(19)?,
            body_text: String::new(), // not loaded for dashboard
            created_at: row.get(20)?,
            updated_at: row.get(21)?,
        })
    })?;
    let mut results = Vec::new();
    for row in rows {
        results.push(row?);
    }
    Ok(results)
}

/// Daily token counts for the last N days (for sparkline chart)
pub fn daily_token_counts(conn: &Connection, days: usize) -> Result<Vec<(String, i64, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT date(created_at) as day,
                COALESCE(SUM(tokens_in), 0),
                COALESCE(SUM(tokens_out), 0)
         FROM transcripts
         WHERE created_at >= date('now', :offset)
         GROUP BY day
         ORDER BY day ASC",
    )?;

    let offset_str = format!("-{days} days");
    let rows = stmt.query_map(rusqlite::named_params! { ":offset": offset_str }, |row| {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?))
    })?;

    let mut results = Vec::new();
    for row in rows {
        results.push(row?);
    }
    Ok(results)
}

/// Transcript count per day for the last N days (for activity sparkline)
pub fn daily_session_counts(conn: &Connection, days: usize) -> Result<Vec<(String, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT date(created_at) as day, COUNT(*) as cnt
         FROM transcripts
         WHERE created_at >= date('now', :offset)
         GROUP BY day
         ORDER BY day ASC",
    )?;

    let offset_str = format!("-{days} days");
    let rows = stmt.query_map(rusqlite::named_params! { ":offset": offset_str }, |row| {
        Ok((row.get(0)?, row.get(1)?))
    })?;

    let mut results = Vec::new();
    for row in rows {
        results.push(row?);
    }
    Ok(results)
}

// -- Dashboard live-data queries ---------------------------------------------

/// Today's aggregate: sessions, tokens_in, tokens_out
pub fn today_stats(conn: &Connection) -> Result<(i64, i64, i64)> {
    let (sessions, tok_in, tok_out) = conn.query_row(
        "SELECT COUNT(*),
                COALESCE(SUM(tokens_in), 0),
                COALESCE(SUM(tokens_out), 0)
         FROM transcripts
         WHERE date(created_at) = date('now')",
        [],
        |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
            ))
        },
    )?;
    Ok((sessions, tok_in, tok_out))
}

/// Recent tool errors (from tool_invocations where is_error = 1).
/// Returns (tool_name, error_summary, invoked_at, transcript_id).
pub fn recent_tool_errors(
    conn: &Connection,
    limit: usize,
) -> Result<Vec<(String, String, String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT tool_name,
                COALESCE(error_summary, '(no details)'),
                COALESCE(invoked_at, ''),
                transcript_id
         FROM tool_invocations
         WHERE is_error = 1
         ORDER BY id DESC
         LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit as i64], |row| {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
    })?;
    let mut results = Vec::new();
    for row in rows {
        results.push(row?);
    }
    Ok(results)
}

/// Total tool error count today.
pub fn today_error_count(conn: &Connection) -> Result<i64> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tool_invocations
         WHERE is_error = 1 AND date(invoked_at) = date('now')",
        [],
        |r| r.get(0),
    )?;
    Ok(count)
}

/// Hourly token buckets for today (for an intra-day sparkline).
/// Returns up to 24 entries: (hour 0..23, tokens_in, tokens_out).
pub fn hourly_tokens_today(conn: &Connection) -> Result<Vec<(i32, i64, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT CAST(strftime('%H', created_at) AS INTEGER) AS hr,
                COALESCE(SUM(tokens_in), 0),
                COALESCE(SUM(tokens_out), 0)
         FROM transcripts
         WHERE date(created_at) = date('now')
         GROUP BY hr
         ORDER BY hr ASC",
    )?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
    let mut results = Vec::new();
    for row in rows {
        results.push(row?);
    }
    Ok(results)
}

// -- Maintenance -------------------------------------------------------------

/// Find and remove duplicate transcripts that differ only by ID prefix/format.
/// Uses the canonical_id algorithm (ADR-001) to group rows that represent the
/// same session. Keeps the row with the most data (longest body, most tokens).
pub fn dedupe_transcripts(conn: &Connection, dry_run: bool) -> Result<()> {
    // Fetch all transcripts with scoring data
    let mut stmt = conn.prepare(
        "SELECT id, source, length(body_text) as blen, tokens_in, created_at FROM transcripts ORDER BY source, id",
    )?;
    let rows: Vec<(String, String, i64, i64, String)> = stmt
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })?
        .collect::<std::result::Result<_, _>>()?;

    /// Canonicalize an ID to `{source}-{native_id}`.
    /// Mirrors parse::canonical_id — duplicated here to avoid cross-crate dep.
    fn canonical(id: &str, source: &str) -> String {
        let stem = id.rsplit('/').next().unwrap_or(id);
        let stem = stem.strip_suffix(".md").unwrap_or(stem);
        let prefix = format!("{source}-");
        let body = stem.strip_prefix(&prefix).unwrap_or(stem);
        let body = match source {
            "claude-code" => body.strip_prefix("claude-").unwrap_or(body),
            "vscode-copilot" => body.strip_prefix("vscode-").unwrap_or(body),
            _ => body,
        };
        format!("{source}-{body}")
    }

    // Group rows by their canonical ID
    let mut groups: std::collections::HashMap<String, Vec<(String, i64, i64, String)>> =
        std::collections::HashMap::new();
    for (id, source, blen, tokens, created) in &rows {
        let key = canonical(id, source);
        groups
            .entry(key)
            .or_default()
            .push((id.clone(), *blen, *tokens, created.clone()));
    }

    let mut to_delete = Vec::new();
    for (canonical_id, mut group) in &mut groups {
        if group.len() <= 1 {
            continue;
        }
        // Prefer: canonical ID first, then most tokens, then longest body, then latest created
        group.sort_by(|a, b| {
            let a_is_canonical = a.0 == *canonical_id;
            let b_is_canonical = b.0 == *canonical_id;
            b_is_canonical
                .cmp(&a_is_canonical)
                .then(b.2.cmp(&a.2)) // tokens_in desc
                .then(b.1.cmp(&a.1)) // body_len desc
                .then(b.3.cmp(&a.3)) // created_at desc
        });
        let keep = &group[0].0;
        for entry in &group[1..] {
            to_delete.push((entry.0.clone(), keep.clone()));
        }
    }

    if to_delete.is_empty() {
        println!("No duplicates found.");
        return Ok(());
    }

    to_delete.sort_by(|a, b| a.0.cmp(&b.0));
    println!("Found {} duplicate(s):", to_delete.len());
    for (dup, keep) in &to_delete {
        println!("  delete {dup}  (keeping {keep})");
    }

    if dry_run {
        println!("(dry run — no changes made)");
    } else {
        for (dup, _keep) in &to_delete {
            conn.execute("DELETE FROM turns WHERE transcript_id = ?1", [dup])?;
            conn.execute("DELETE FROM tool_usage WHERE transcript_id = ?1", [dup])?;
            conn.execute("DELETE FROM transcripts WHERE id = ?1", [dup])?;
        }
        println!("Deleted {} duplicate(s).", to_delete.len());
    }

    Ok(())
}

// -- Tests -------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Connection {
        init_db(Path::new(":memory:")).expect("init_db failed")
    }

    fn sample_record() -> TranscriptRecord {
        TranscriptRecord {
            id: "test-001".into(),
            source: "claude-code".into(),
            title: "Fix login bug".into(),
            status: "done".into(),
            model_provider: "anthropic".into(),
            model_name: "claude-sonnet-4-20250514".into(),
            turns_user: 3,
            turns_agent: 3,
            turns_total: 6,
            tokens_in: 5000,
            tokens_out: 12000,
            tokens_cache_read: 2000,
            tokens_cache_write: 1000,
            word_count: 850,
            thinking_enabled: true,
            tags: vec!["bugfix".into(), "auth".into()],
            tools_used: vec!["Read".into(), "Edit".into()],
            folder_paths: vec!["/home/user/project".into()],
            branch: Some("fix/login".into()),
            thread_version: None,
            body_text: "User asked to fix login bug. Agent read files and edited code.".into(),
            created_at: "2025-01-15T10:00:00Z".into(),
            updated_at: "2025-01-15T10:05:00Z".into(),
        }
    }

    #[test]
    fn test_init_db() {
        let conn = test_db();
        let version: i64 = conn
            .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 1);
    }

    #[test]
    fn test_upsert_and_get() {
        let conn = test_db();
        let rec = sample_record();
        upsert_transcript(&conn, &rec).unwrap();

        let fetched = get_transcript(&conn, "test-001").unwrap().unwrap();
        assert_eq!(fetched.title, "Fix login bug");
        assert_eq!(fetched.tokens_in, 5000);
        assert!(fetched.thinking_enabled);
        assert_eq!(fetched.tags, vec!["bugfix", "auth"]);
    }

    #[test]
    fn test_upsert_updates_existing() {
        let conn = test_db();
        let mut rec = sample_record();
        upsert_transcript(&conn, &rec).unwrap();

        rec.title = "Updated title".into();
        rec.tokens_in = 9999;
        upsert_transcript(&conn, &rec).unwrap();

        let fetched = get_transcript(&conn, "test-001").unwrap().unwrap();
        assert_eq!(fetched.title, "Updated title");
        assert_eq!(fetched.tokens_in, 9999);
    }

    #[test]
    fn test_upsert_turns() {
        let conn = test_db();
        upsert_transcript(&conn, &sample_record()).unwrap();

        let turns = vec![
            TurnRecord {
                transcript_id: "test-001".into(),
                turn_number: 1,
                role: "user".into(),
                tokens_in: 100,
                tokens_out: 0,
                tokens_cache_read: 0,
                tokens_cache_write: 0,
                content_length: 50,
                has_thinking: false,
                tools: vec![],
            },
            TurnRecord {
                transcript_id: "test-001".into(),
                turn_number: 2,
                role: "agent".into(),
                tokens_in: 0,
                tokens_out: 500,
                tokens_cache_read: 200,
                tokens_cache_write: 0,
                content_length: 300,
                has_thinking: true,
                tools: vec!["Read".into(), "Edit".into()],
            },
        ];
        upsert_turns(&conn, "test-001", &turns).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM turns WHERE transcript_id = 'test-001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn test_tool_usage() {
        let conn = test_db();
        upsert_transcript(&conn, &sample_record()).unwrap();

        let tools: Vec<String> = vec!["Read".into(), "Read".into(), "Edit".into(), "Bash".into()];
        upsert_tool_usage(&conn, "test-001", &tools).unwrap();

        let read_count: i64 = conn
            .query_row(
                "SELECT invocation_count FROM tool_usage WHERE transcript_id = 'test-001' AND tool_name = 'Read'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(read_count, 2);
    }

    #[test]
    fn test_search_fts() {
        let conn = test_db();
        upsert_transcript(&conn, &sample_record()).unwrap();

        let results = search(&conn, "login bug", 10, 0).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "test-001");
    }

    #[test]
    fn test_list_transcripts() {
        let conn = test_db();
        upsert_transcript(&conn, &sample_record()).unwrap();

        let mut rec2 = sample_record();
        rec2.id = "test-002".into();
        rec2.source = "zed".into();
        upsert_transcript(&conn, &rec2).unwrap();

        let all = list_transcripts(&conn, None, 50, 0, "created_at").unwrap();
        assert_eq!(all.len(), 2);

        let zed_only = list_transcripts(&conn, Some("zed"), 50, 0, "created_at").unwrap();
        assert_eq!(zed_only.len(), 1);
        assert_eq!(zed_only[0].source, "zed");
    }

    #[test]
    fn test_get_stats() {
        let conn = test_db();
        upsert_transcript(&conn, &sample_record()).unwrap();

        let tools: Vec<String> = vec!["Read".into(), "Edit".into()];
        upsert_tool_usage(&conn, "test-001", &tools).unwrap();

        let stats = get_stats(&conn).unwrap();
        assert_eq!(stats.total_transcripts, 1);
        assert_eq!(stats.total_tokens_in, 5000);
        assert_eq!(stats.sources.len(), 1);
        assert_eq!(stats.top_tools.len(), 2);
    }
}
