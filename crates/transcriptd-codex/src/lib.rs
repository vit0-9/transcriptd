use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use walkdir::WalkDir;

// ── helpers ──────────────────────────────────────────────────────────

static WARNED_IDS: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

fn warn_once(id: &str, msg: &str) {
    if let Ok(mut set) = WARNED_IDS.lock()
        && set.insert(id.to_string()) {
            eprintln!("SKIP {id}: {msg}");
        }
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

// ── JSONL event types ────────────────────────────────────────────────

#[derive(Deserialize, Debug)]
pub struct JsonlLine {
    pub timestamp: Option<String>,
    #[serde(rename = "type")]
    pub line_type: String,
    pub payload: serde_json::Value,
}

#[derive(Deserialize, Debug)]
pub struct SessionMeta {
    pub id: String,
    pub timestamp: Option<String>,
    pub cwd: Option<String>,
    pub originator: Option<String>,
    pub cli_version: Option<String>,
    pub model_provider: Option<String>,
    pub git: Option<GitInfo>,
}

#[derive(Deserialize, Debug)]
pub struct GitInfo {
    pub commit_hash: Option<String>,
    pub branch: Option<String>,
    pub repository_url: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct TurnContext {
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
}

// ── parsed session ───────────────────────────────────────────────────

pub struct Session {
    pub meta: SessionMeta,
    pub turns: Vec<Turn>,
}

pub struct Turn {
    pub role: TurnRole,
    pub content: String,
    pub timestamp: Option<String>,
    pub tools: Vec<ToolCall>,
}

pub struct ToolCall {
    pub name: String,
    pub is_error: bool,
}

#[derive(Clone, Copy, PartialEq)]
pub enum TurnRole {
    User,
    Agent,
    System,
}

impl TurnRole {
    pub fn label(&self) -> &'static str {
        match self {
            TurnRole::User => "user",
            TurnRole::Agent => "agent",
            TurnRole::System => "system",
        }
    }
}

// ── public API ───────────────────────────────────────────────────────

pub fn default_codex_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".codex").join("sessions"))
}

pub fn count_sessions(codex_dir: &str) -> Result<usize> {
    let mut count = 0usize;
    for entry in WalkDir::new(codex_dir).min_depth(1) {
        let entry = entry?;
        if entry.path().extension().is_some_and(|e| e == "jsonl") {
            count += 1;
        }
    }
    Ok(count)
}

pub fn extract_one(jsonl_path: &str) -> Result<String> {
    let session = parse_session(jsonl_path)?;
    Ok(render_markdown(&session))
}

pub fn extract_all(codex_dir: &str, since: Option<&str>) -> Result<Vec<(String, String)>> {
    let mut results = Vec::new();

    for entry in WalkDir::new(codex_dir).min_depth(1) {
        let entry = entry?;
        let fpath = entry.path();
        if fpath.extension().is_none_or(|e| e != "jsonl") {
            continue;
        }

        let stem = fpath
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let jsonl_str = fpath.to_string_lossy().to_string();

        match parse_session(&jsonl_str) {
            Ok(session) => {
                if let Some(since_ts) = since {
                    let last_ts = session
                        .turns
                        .last()
                        .and_then(|t| t.timestamp.as_deref())
                        .or(session.meta.timestamp.as_deref());
                    if let Some(ts) = last_ts
                        && ts < since_ts {
                            continue;
                        }
                }

                let md = render_markdown(&session);
                results.push((stem.clone(), md));
            }
            Err(e) => {
                warn_once(&stem, &format!("{e}"));
            }
        }
    }

    results.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(results)
}

// ── parsing ──────────────────────────────────────────────────────────

fn parse_session(jsonl_path: &str) -> Result<Session> {
    let raw =
        std::fs::read_to_string(jsonl_path).with_context(|| format!("reading {jsonl_path}"))?;

    let mut meta: Option<SessionMeta> = None;
    let mut turns: Vec<Turn> = Vec::new();
    let mut model: Option<String> = None;

    // Track pending function calls for pairing with outputs
    let mut pending_calls: HashMap<String, String> = HashMap::new(); // call_id -> name

    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let entry: JsonlLine = match serde_json::from_str(line) {
            Ok(e) => e,
            Err(_) => continue,
        };

        match entry.line_type.as_str() {
            "session_meta" => {
                meta = serde_json::from_value(entry.payload).ok();
            }

            "turn_context" => {
                if let Ok(tc) = serde_json::from_value::<TurnContext>(entry.payload)
                    && model.is_none() {
                        model = tc.model;
                    }
            }

            "event_msg" => {
                let payload = &entry.payload;
                let evt_type = payload.get("type").and_then(|v| v.as_str());
                match evt_type {
                    Some("user_message") => {
                        let message = payload
                            .get("message")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        if !message.is_empty() {
                            turns.push(Turn {
                                role: TurnRole::User,
                                content: message,
                                timestamp: entry.timestamp.clone(),
                                tools: Vec::new(),
                            });
                        }
                    }
                    Some("agent_message") => {
                        let message = payload
                            .get("message")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        if !message.is_empty() {
                            // Append to existing agent turn or create new
                            if let Some(last) = turns.last_mut()
                                && last.role == TurnRole::Agent {
                                    last.content.push('\n');
                                    last.content.push_str(&message);
                                    continue;
                                }
                            turns.push(Turn {
                                role: TurnRole::Agent,
                                content: message,
                                timestamp: entry.timestamp.clone(),
                                tools: Vec::new(),
                            });
                        }
                    }
                    _ => {} // token_count, agent_reasoning handled below
                }
            }

            "response_item" => {
                let payload = &entry.payload;
                let item_type = payload.get("type").and_then(|v| v.as_str());

                match item_type {
                    Some("message") => {
                        let role_str = payload.get("role").and_then(|v| v.as_str());
                        let content = payload
                            .get("content")
                            .and_then(|v| v.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|block| {
                                        let bt = block.get("type").and_then(|v| v.as_str())?;
                                        match bt {
                                            "input_text" | "output_text" => block
                                                .get("text")
                                                .and_then(|v| v.as_str())
                                                .map(String::from),
                                            _ => None,
                                        }
                                    })
                                    .collect::<Vec<_>>()
                                    .join("\n")
                            })
                            .unwrap_or_default();

                        if content.is_empty() {
                            continue;
                        }

                        let role = match role_str {
                            Some("user") => TurnRole::User,
                            Some("assistant") => TurnRole::Agent,
                            _ => TurnRole::Agent,
                        };

                        turns.push(Turn {
                            role,
                            content,
                            timestamp: entry.timestamp.clone(),
                            tools: Vec::new(),
                        });
                    }

                    Some("function_call") => {
                        let name = payload
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                            .to_string();
                        let call_id = payload
                            .get("call_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        pending_calls.insert(call_id, name.clone());

                        // Ensure there's an agent turn to attach tools to
                        if turns.last().is_none_or(|t| t.role != TurnRole::Agent) {
                            turns.push(Turn {
                                role: TurnRole::Agent,
                                content: String::new(),
                                timestamp: entry.timestamp.clone(),
                                tools: Vec::new(),
                            });
                        }
                        if let Some(last) = turns.last_mut() {
                            last.tools.push(ToolCall {
                                name,
                                is_error: false,
                            });
                        }
                    }

                    Some("function_call_output") => {
                        let call_id = payload
                            .get("call_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let output = payload.get("output").and_then(|v| v.as_str()).unwrap_or("");
                        let is_error = output.starts_with("Error") || output.starts_with("error");

                        if is_error
                            && let Some(name) = pending_calls.get(call_id) {
                                // Mark the tool call as errored
                                for turn in turns.iter_mut().rev() {
                                    for tc in &mut turn.tools {
                                        if tc.name == *name && !tc.is_error {
                                            tc.is_error = true;
                                            break;
                                        }
                                    }
                                    break;
                                }
                            }
                    }

                    Some("custom_tool_call") => {
                        let name = payload
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                            .to_string();
                        let status = payload.get("status").and_then(|v| v.as_str()).unwrap_or("");
                        let is_error = status == "failed" || status == "error";

                        if turns.last().is_none_or(|t| t.role != TurnRole::Agent) {
                            turns.push(Turn {
                                role: TurnRole::Agent,
                                content: String::new(),
                                timestamp: entry.timestamp.clone(),
                                tools: Vec::new(),
                            });
                        }
                        if let Some(last) = turns.last_mut() {
                            last.tools.push(ToolCall { name, is_error });
                        }
                    }

                    _ => {} // reasoning, custom_tool_call_output
                }
            }

            _ => {} // compacted, etc.
        }
    }

    let meta = meta.unwrap_or(SessionMeta {
        id: Path::new(jsonl_path)
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
        timestamp: None,
        cwd: None,
        originator: None,
        cli_version: None,
        model_provider: None,
        git: None,
    });

    Ok(Session { meta, turns })
}

// ── markdown rendering ───────────────────────────────────────────────

fn render_markdown(session: &Session) -> String {
    let m = &session.meta;
    let id = &m.id;

    let first_ts = session
        .turns
        .first()
        .and_then(|t| t.timestamp.as_deref())
        .or(m.timestamp.as_deref())
        .unwrap_or("1970-01-01T00:00:00Z");
    let last_ts = session
        .turns
        .last()
        .and_then(|t| t.timestamp.as_deref())
        .or(m.timestamp.as_deref())
        .unwrap_or(first_ts);

    let title = session
        .turns
        .iter()
        .find(|t| t.role == TurnRole::User)
        .map(|t| {
            let s = t.content.trim();
            if s.len() > 80 {
                format!("{}...", safe_truncate(s, 80))
            } else {
                s.to_string()
            }
        })
        .unwrap_or_else(|| "Untitled".to_string());

    let model_provider = m.model_provider.as_deref().unwrap_or("openai");

    let turns_user = session
        .turns
        .iter()
        .filter(|t| t.role == TurnRole::User)
        .count();
    let turns_agent = session
        .turns
        .iter()
        .filter(|t| t.role == TurnRole::Agent)
        .count();
    let turns_total = turns_user + turns_agent;

    // Collect tools
    let mut tools_set = HashSet::new();
    for turn in &session.turns {
        for tc in &turn.tools {
            tools_set.insert(tc.name.clone());
        }
    }
    let mut tools: Vec<String> = tools_set.into_iter().collect();
    tools.sort();
    let tools_yaml = if tools.is_empty() {
        "[]".to_string()
    } else {
        format!("[{}]", tools.join(", "))
    };

    let folder_yaml = m
        .cwd
        .as_deref()
        .map(|f| format!("  - \"{f}\""))
        .unwrap_or_else(|| "  - \"/unknown\"".to_string());

    let branch_yaml = m
        .git
        .as_ref()
        .and_then(|g| g.branch.as_deref())
        .map(|b| format!("branch: {b}\n"))
        .unwrap_or_default();

    let originator = m.originator.as_deref().unwrap_or("codex");
    let cli_version = m.cli_version.as_deref().unwrap_or("unknown");

    let title_esc = title.replace('"', "\\\"");

    let mut md = format!(
        "---\n\
         schema_version: 1\n\
         id: codex-{id}\n\
         type: transcript\n\
         source: codex\n\
         status: done\n\
         title: \"{title_esc}\"\n\
         created_at: {first_ts}\n\
         updated_at: {last_ts}\n\
         model:\n\
         \x20 provider: {model_provider}\n\
         \x20 model: codex\n\
         originator: {originator}\n\
         cli_version: {cli_version}\n\
         turns_user: {turns_user}\n\
         turns_agent: {turns_agent}\n\
         turns_total: {turns_total}\n\
         tools_used: {tools_yaml}\n\
         {branch_yaml}\
         folder_paths:\n\
         {folder_yaml}\n\
         ---\n\n\
         # {title_esc}\n\n"
    );

    let mut tn = 0u32;
    for turn in &session.turns {
        tn += 1;
        let role = turn.role.label();
        md.push_str(&format!("## Turn {tn} -- {role}\n\n"));

        if !turn.content.is_empty() {
            md.push_str(&turn.content);
            if !turn.content.ends_with('\n') {
                md.push('\n');
            }
            md.push('\n');
        }

        for tc in &turn.tools {
            let err = if tc.is_error { " ERROR" } else { "" };
            md.push_str(&format!("> **tool: {}**{err}\n\n", tc.name));
        }
    }

    md
}

// -- TranscriptExtractor trait impl ------------------------------------------

use transcriptd_core::TranscriptExtractor;

pub struct CodexExtractor;

impl TranscriptExtractor for CodexExtractor {
    fn name(&self) -> &str {
        "codex"
    }

    fn default_source_path(&self) -> Option<PathBuf> {
        default_codex_dir()
    }

    fn count(&self, source: &Path) -> Result<usize> {
        count_sessions(&source.to_string_lossy())
    }

    fn extract_all(&self, source: &Path, since: Option<&str>) -> Result<Vec<(String, String)>> {
        extract_all(&source.to_string_lossy(), since)
    }

    fn extract_one(&self, source: &Path, id: &str) -> Result<String> {
        // Search for the session file by ID (stem match)
        for entry in WalkDir::new(source).min_depth(1) {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let fpath = entry.path();
            if fpath.extension().is_some_and(|e| e == "jsonl") {
                let stem = fpath.file_stem().unwrap_or_default().to_string_lossy();
                if stem == id || stem.ends_with(id) {
                    return extract_one(&fpath.to_string_lossy());
                }
            }
        }
        bail!("codex session {id} not found in {}", source.display())
    }

    fn watch_paths(&self, source: &Path) -> Vec<PathBuf> {
        vec![source.to_path_buf()]
    }
}
