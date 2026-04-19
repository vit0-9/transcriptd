use anyhow::{Context, Result, bail};
use chrono::TimeZone;
use rusqlite::{Connection, OpenFlags};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

// ── helpers ──────────────────────────────────────────────────────────

static WARNED_IDS: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

fn warn_once(id: &str, msg: &str) {
    if let Ok(mut set) = WARNED_IDS.lock() {
        if set.insert(id.to_string()) {
            eprintln!("SKIP {id}: {msg}");
        }
    }
}

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

// ── session index (from state.vscdb) ─────────────────────────────────

#[derive(Deserialize, Debug)]
pub struct SessionIndex {
    pub version: u32,
    pub entries: HashMap<String, SessionIndexEntry>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SessionIndexEntry {
    pub session_id: String,
    pub title: Option<String>,
    pub last_message_date: Option<i64>,
}

// ── session JSON structs ─────────────────────────────────────────────

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub version: Option<u32>,
    pub session_id: String,
    pub creation_date: Option<i64>,
    pub last_message_date: Option<i64>,
    pub custom_title: Option<String>,
    #[serde(default)]
    pub requests: Vec<Request>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub request_id: Option<String>,
    pub message: Option<RequestMessage>,
    #[serde(default)]
    pub response: Vec<serde_json::Value>,
    pub model_id: Option<String>,
    pub timestamp: Option<i64>,
    pub agent: Option<serde_json::Value>,
}

#[derive(Deserialize, Debug)]
pub struct RequestMessage {
    pub text: Option<String>,
    pub parts: Option<Vec<serde_json::Value>>,
}

#[derive(Deserialize, Debug)]
pub struct WorkspaceJson {
    pub folder: Option<String>,
}

// ── public API ───────────────────────────────────────────────────────

/// Default VSCode workspaceStorage dir per platform.
pub fn default_vscode_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        dirs::home_dir().map(|h| h.join("Library/Application Support/Code/User/workspaceStorage"))
    }
    #[cfg(target_os = "linux")]
    {
        dirs::home_dir().map(|h| h.join(".config/Code/User/workspaceStorage"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

/// Reconstruct a `Session` from a `.jsonl` chat-sessions file.
///
/// Format: kind 0 = session metadata, kind 1 = context/attachments,
/// kind 2 = incremental request-array snapshots.
/// We take the metadata from kind 0 and the **last** kind 2 line for
/// the final request state.
fn parse_jsonl_session(path: &Path) -> Result<Session> {
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let reader = std::io::BufReader::new(file);

    let mut session_meta: Option<serde_json::Value> = None;
    let mut last_requests: Option<Vec<serde_json::Value>> = None;

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let entry: serde_json::Value = serde_json::from_str(&line)
            .with_context(|| format!("parsing jsonl line in {}", path.display()))?;

        match entry.get("kind").and_then(|v| v.as_u64()) {
            Some(0) => {
                session_meta = entry.get("v").cloned();
            }
            Some(2) => {
                if let Some(arr) = entry.get("v").and_then(|v| v.as_array()) {
                    last_requests = Some(arr.clone());
                }
            }
            _ => {} // kind 1 = context, skip for now
        }
    }

    let meta = session_meta.unwrap_or_else(|| serde_json::json!({}));

    // Build Session from the combined data
    let mut session_value = meta;
    if let Some(requests) = last_requests {
        session_value["requests"] = serde_json::Value::Array(requests);
    }

    let session: Session = serde_json::from_value(session_value).with_context(|| {
        format!(
            "deserializing reconstructed session from {}",
            path.display()
        )
    })?;

    Ok(session)
}

/// Returns true if the file extension is `.json` or `.jsonl`.
fn is_chat_session_file(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e == "json" || e == "jsonl")
}

/// Count all sessions across all workspaces.
pub fn count_sessions(vscode_dir: &str) -> Result<usize> {
    let base = Path::new(vscode_dir);
    if !base.is_dir() {
        bail!("not a directory: {}", vscode_dir);
    }
    let mut total = 0usize;
    for entry in std::fs::read_dir(base)? {
        let entry = entry?;
        let cs = entry.path().join("chatSessions");
        if cs.is_dir() {
            for f in std::fs::read_dir(&cs)? {
                let f = f?;
                if is_chat_session_file(&f.path()) {
                    total += 1;
                }
            }
        }
    }
    Ok(total)
}

/// Extract all sessions. Returns Vec<(relative_path, markdown)>.
pub fn extract_all(vscode_dir: &str, since: Option<&str>) -> Result<Vec<(String, String)>> {
    let base = Path::new(vscode_dir);
    if !base.is_dir() {
        bail!("not a directory: {}", vscode_dir);
    }

    let since_ms: Option<i64> = since.and_then(|s| {
        chrono::DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|dt| dt.timestamp_millis())
    });

    let mut results = Vec::new();

    for ws_entry in std::fs::read_dir(base)? {
        let ws_entry = ws_entry?;
        let ws_path = ws_entry.path();
        if !ws_path.is_dir() {
            continue;
        }

        let cs_dir = ws_path.join("chatSessions");
        if !cs_dir.is_dir() {
            continue;
        }

        // Session index from state.vscdb for titles
        let index = read_session_index(&ws_path).unwrap_or_default();
        let workspace_folder = read_workspace_folder(&ws_path);

        for f in std::fs::read_dir(&cs_dir)? {
            let f = f?;
            let path = f.path();
            if !is_chat_session_file(&path) {
                continue;
            }

            let session_id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();

            // Pre-filter by index date
            if let Some(cutoff) = since_ms
                && let Some(entry) = index.get(&session_id)
                && entry.last_message_date.is_some_and(|d| d < cutoff)
            {
                continue;
            }

            let session: Session = if path.extension().is_some_and(|e| e == "jsonl") {
                match parse_jsonl_session(&path) {
                    Ok(s) => s,
                    Err(e) => {
                        warn_once(&session_id, &format!("jsonl: {e}"));
                        continue;
                    }
                }
            } else {
                let json_bytes = match std::fs::read(&path) {
                    Ok(b) => b,
                    Err(e) => {
                        warn_once(&session_id, &format!("read: {e}"));
                        continue;
                    }
                };

                match serde_json::from_slice(&json_bytes) {
                    Ok(s) => s,
                    Err(e) => {
                        warn_once(&session_id, &format!("json: {e}"));
                        continue;
                    }
                }
            };

            // Post-filter by session date
            if let Some(cutoff) = since_ms
                && session.last_message_date.is_some_and(|d| d < cutoff)
            {
                continue;
            }

            let index_title = index.get(&session_id).and_then(|e| e.title.clone());
            let md = render_session(
                &session,
                index_title.as_deref(),
                workspace_folder.as_deref(),
            );

            results.push((session_id.clone(), md));
        }
    }

    results.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(results)
}

/// Extract single session file to markdown.
pub fn extract_one(session_path: &str) -> Result<String> {
    let path = Path::new(session_path);

    let session: Session = if path.extension().is_some_and(|e| e == "jsonl") {
        parse_jsonl_session(path)?
    } else {
        let json_bytes = std::fs::read(path).with_context(|| format!("reading {session_path}"))?;
        serde_json::from_slice(&json_bytes).with_context(|| format!("parsing {session_path}"))?
    };

    let index_title = path.parent().and_then(|p| p.parent()).and_then(|ws| {
        let sid = path.file_stem()?.to_str()?;
        let idx = read_session_index(ws).ok()?;
        idx.get(sid)?.title.clone()
    });

    let workspace_folder = path
        .parent()
        .and_then(|p| p.parent())
        .and_then(read_workspace_folder);

    Ok(render_session(
        &session,
        index_title.as_deref(),
        workspace_folder.as_deref(),
    ))
}

// ── internal ─────────────────────────────────────────────────────────

fn read_session_index(ws_path: &Path) -> Result<HashMap<String, SessionIndexEntry>> {
    let db_path = ws_path.join("state.vscdb");
    if !db_path.exists() {
        return Ok(HashMap::new());
    }
    let conn = Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let raw: String = conn
        .query_row(
            "SELECT value FROM ItemTable WHERE key = 'chat.ChatSessionStore.index'",
            [],
            |row| row.get(0),
        )
        .unwrap_or_default();

    if raw.is_empty() {
        return Ok(HashMap::new());
    }
    let idx: SessionIndex = serde_json::from_str(&raw)?;
    Ok(idx.entries)
}

fn read_workspace_folder(ws_path: &Path) -> Option<String> {
    let wj_path = ws_path.join("workspace.json");
    let data = std::fs::read_to_string(&wj_path).ok()?;
    let wj: WorkspaceJson = serde_json::from_str(&data).ok()?;
    wj.folder.map(|f| f.replace("file://", ""))
}

fn render_session(
    session: &Session,
    index_title: Option<&str>,
    workspace_folder: Option<&str>,
) -> String {
    let sid = &session.session_id;
    let created_ms = session.creation_date.unwrap_or(0);
    let updated_ms = session.last_message_date.unwrap_or(created_ms);
    let created = ms_to_iso(created_ms);
    let updated = ms_to_iso(updated_ms);

    // Title: customTitle > index title > first message > "Untitled"
    let title = session
        .custom_title
        .as_deref()
        .or(index_title)
        .or_else(|| {
            session
                .requests
                .first()
                .and_then(|r| r.message.as_ref())
                .and_then(|m| m.text.as_deref())
                .map(|t| safe_truncate(t, 80))
        })
        .unwrap_or("Untitled");

    // Model from first request
    let model_id = session
        .requests
        .first()
        .and_then(|r| r.model_id.as_deref())
        .unwrap_or("unknown");

    let (provider, model) = if let Some(pos) = model_id.find('/') {
        (&model_id[..pos], &model_id[pos + 1..])
    } else {
        ("copilot", model_id)
    };

    let turns_user = session.requests.len();
    let turns_agent = session
        .requests
        .iter()
        .filter(|r| !r.response.is_empty())
        .count();

    // Collect tools
    let mut tools_set = HashSet::new();
    for req in &session.requests {
        for part in &req.response {
            if let Some(tid) = part.get("toolId").and_then(|v| v.as_str()) {
                tools_set.insert(tid.to_string());
            }
        }
    }
    let mut tools: Vec<String> = tools_set.into_iter().collect();
    tools.sort();
    let tools_yaml = if tools.is_empty() {
        "[]".to_string()
    } else {
        format!("[{}]", tools.join(", "))
    };

    // Collect mentions from user message parts (kind: "dynamic")
    let mut mentions = Vec::new();
    for req in &session.requests {
        if let Some(msg) = &req.message {
            if let Some(parts) = &msg.parts {
                for part in parts {
                    if let Some(obj) = part.as_object() {
                        if obj.get("kind").and_then(|v| v.as_str()) == Some("dynamic") {
                            if let Some(id) = obj.get("id").and_then(|v| v.as_str()) {
                                mentions.push(id.to_string());
                            }
                        }
                    }
                }
            }
        }
    }
    let mentions_yaml = if mentions.is_empty() {
        String::new()
    } else {
        let items: Vec<String> = mentions
            .iter()
            .map(|m| format!("  - \"{}\"", m.replace('"', "\\\"")))
            .collect();
        format!("mentions:\n{}\n", items.join("\n"))
    };

    let folder_yaml = workspace_folder
        .map(|f| format!("  - \"{f}\""))
        .unwrap_or_else(|| "  - \"/unknown\"".to_string());

    // turns_detail
    let mut turns_detail = String::from("turns_detail:\n");
    let mut tn = 0u32;
    for req in &session.requests {
        tn += 1;
        let ulen = req
            .message
            .as_ref()
            .and_then(|m| m.text.as_ref())
            .map(|t| t.len())
            .unwrap_or(0);
        turns_detail.push_str(&format!(
            "  - turn: {tn}\n    role: user\n    content_chars: {ulen}\n"
        ));

        if !req.response.is_empty() {
            tn += 1;
            let mut at = Vec::new();
            let mut has_think = false;
            let mut cchars = 0usize;
            for part in &req.response {
                let kind = part.get("kind").and_then(|v| v.as_str());
                match kind {
                    Some("thinking") => {
                        has_think = true;
                        if let Some(v) = part.get("value").and_then(|v| v.as_str()) {
                            cchars += v.len();
                        }
                    }
                    Some("toolInvocationSerialized") => {
                        if let Some(tid) = part.get("toolId").and_then(|v| v.as_str()) {
                            at.push(tid.to_string());
                        }
                    }
                    None => {
                        if let Some(v) = part.get("value").and_then(|v| v.as_str()) {
                            cchars += v.len();
                        }
                    }
                    _ => {}
                }
            }
            turns_detail.push_str(&format!("  - turn: {tn}\n    role: agent\n"));
            if !at.is_empty() {
                turns_detail.push_str(&format!("    tools: [{}]\n", at.join(", ")));
            }
            if has_think {
                turns_detail.push_str("    has_thinking: true\n");
            }
            turns_detail.push_str(&format!("    content_chars: {cchars}\n"));
        }
    }

    let title_esc = title.replace('"', "\\\"");
    let total = turns_user + turns_agent;

    let mut md = format!(
        "---\n\
         schema_version: 1\n\
         id: vscode-{sid}\n\
         type: transcript\n\
         source: vscode-copilot\n\
         status: done\n\
         title: \"{title_esc}\"\n\
         created_at: {created}\n\
         updated_at: {updated}\n\
         model:\n\
         \x20 provider: {provider}\n\
         \x20 model: {model}\n\
         turns_user: {turns_user}\n\
         turns_agent: {turns_agent}\n\
         turns_total: {total}\n\
         tools_used: {tools_yaml}\n\
         folder_paths:\n\
         {folder_yaml}\n\
         {mentions_yaml}\
         {turns_detail}\
         ---\n\n\
         # {title_esc}\n\n"
    );

    // Render turns
    let mut tn = 0u32;
    for req in &session.requests {
        tn += 1;
        let user_text = req
            .message
            .as_ref()
            .and_then(|m| m.text.as_deref())
            .unwrap_or("");
        md.push_str(&format!("## Turn {tn} -- user\n\n{user_text}\n\n"));

        // Render user mentions from parts
        if let Some(msg) = &req.message {
            if let Some(parts) = &msg.parts {
                for part in parts {
                    if let Some(obj) = part.as_object() {
                        if obj.get("kind").and_then(|v| v.as_str()) == Some("dynamic") {
                            let id = obj.get("id").and_then(|v| v.as_str()).unwrap_or("");
                            let text = obj.get("text").and_then(|v| v.as_str()).unwrap_or("");
                            if !id.is_empty() {
                                md.push_str(&format!("[mention: {id}] {text}\n\n"));
                            }
                        }
                    }
                }
            }
        }

        if !req.response.is_empty() {
            tn += 1;
            md.push_str(&format!("## Turn {tn} -- agent\n\n"));

            for part in &req.response {
                let kind = part.get("kind").and_then(|v| v.as_str());
                match kind {
                    Some("thinking") => {
                        if let Some(v) = part.get("value").and_then(|v| v.as_str()) {
                            md.push_str(&format!(
                                "<details>\n<summary>thinking</summary>\n\n{v}\n</details>\n\n"
                            ));
                        }
                    }
                    Some("toolInvocationSerialized") => {
                        let tid = part
                            .get("toolId")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown");
                        let msg = part
                            .get("pastTenseMessage")
                            .or_else(|| part.get("invocationMessage"))
                            .and_then(|v| v.get("value"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        md.push_str(&format!("> **tool: {tid}**\n> {msg}\n\n"));

                        // Render tool result if available
                        if let Some(rd) = part.get("resultDetails").and_then(|v| v.as_object()) {
                            let is_error =
                                rd.get("isError").and_then(|v| v.as_bool()).unwrap_or(false);
                            let err_marker = if is_error { " ERROR" } else { "" };
                            let output_str = rd
                                .get("output")
                                .and_then(|v| v.as_array())
                                .map(|arr| {
                                    arr.iter()
                                        .filter_map(|item| {
                                            if item
                                                .get("isText")
                                                .and_then(|v| v.as_bool())
                                                .unwrap_or(false)
                                            {
                                                item.get("value").and_then(|v| v.as_str()).map(
                                                    |s| {
                                                        if s.len() > 500 {
                                                            format!(
                                                                "{}...[truncated]",
                                                                safe_truncate(s, 500)
                                                            )
                                                        } else {
                                                            s.to_string()
                                                        }
                                                    },
                                                )
                                            } else {
                                                None
                                            }
                                        })
                                        .collect::<Vec<_>>()
                                        .join("\n")
                                })
                                .unwrap_or_default();
                            if !output_str.is_empty() || is_error {
                                md.push_str(&format!("> **result: {tid}**{err_marker}\n"));
                                for line in output_str.lines().take(10) {
                                    md.push_str(&format!("> {line}\n"));
                                }
                                md.push('\n');
                            }
                        }
                    }
                    Some("inlineReference") => {
                        if let Some(r) = part.get("inlineReference") {
                            let p = r.get("path").and_then(|v| v.as_str()).unwrap_or("");
                            md.push_str(&format!("[ref: {p}]\n"));
                        }
                    }
                    Some("textEditGroup") => {
                        if let Some(uri) = part.get("uri") {
                            let p = uri
                                .get("path")
                                .or_else(|| uri.get("fsPath"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("?");
                            md.push_str(&format!("> **edit: {p}**\n\n"));
                        }
                    }
                    Some("progressMessage") | Some("progressTaskSerialized") => {
                        if let Some(c) = part.get("content") {
                            let m = c.get("value").and_then(|v| v.as_str()).unwrap_or("");
                            if !m.is_empty() {
                                md.push_str(&format!("<!-- progress: {m} -->\n"));
                            }
                        }
                    }
                    None => {
                        if let Some(v) = part.get("value").and_then(|v| v.as_str()) {
                            md.push_str(v);
                        }
                    }
                    _ => {} // skip prepareToolInvocation, mcpServersStarting, undoStop, codeblockUri, etc.
                }
            }
            // ensure trailing double-newline
            if !md.ends_with("\n\n") {
                if md.ends_with('\n') {
                    md.push('\n');
                } else {
                    md.push_str("\n\n");
                }
            }
        }
    }

    md
}

// -- TranscriptExtractor trait impl ------------------------------------------

use transcriptd_core::TranscriptExtractor;

pub struct VscodeExtractor;

impl TranscriptExtractor for VscodeExtractor {
    fn name(&self) -> &str {
        "vscode-copilot"
    }

    fn default_source_path(&self) -> Option<PathBuf> {
        default_vscode_dir()
    }

    fn count(&self, source: &Path) -> Result<usize> {
        count_sessions(&source.to_string_lossy())
    }

    fn extract_all(&self, source: &Path, since: Option<&str>) -> Result<Vec<(String, String)>> {
        extract_all(&source.to_string_lossy(), since)
    }

    fn extract_one(&self, source: &Path, id: &str) -> Result<String> {
        // id is session ID; search chatSessions dirs for matching JSON or JSONL
        for ws_entry in std::fs::read_dir(source)? {
            let ws_entry = ws_entry?;
            let cs_dir = ws_entry.path().join("chatSessions");
            if cs_dir.is_dir() {
                let json_path = cs_dir.join(format!("{id}.json"));
                if json_path.exists() {
                    return extract_one(&json_path.to_string_lossy());
                }
                let jsonl_path = cs_dir.join(format!("{id}.jsonl"));
                if jsonl_path.exists() {
                    return extract_one(&jsonl_path.to_string_lossy());
                }
            }
        }
        anyhow::bail!("session {id} not found in {}", source.display())
    }

    fn watch_paths(&self, source: &Path) -> Vec<PathBuf> {
        // Watch the entire workspaceStorage directory
        vec![source.to_path_buf()]
    }
}
