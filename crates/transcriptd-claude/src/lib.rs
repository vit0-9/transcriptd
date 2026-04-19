use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashSet;
use std::fs;
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

/// Decode encoded directory name back to filesystem path.
/// `-Users-foo-bar` → `/Users/foo/bar`
pub fn decode_dir_name(name: &str) -> String {
    let mut path = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch == '-' {
            path.push('/');
        } else {
            path.push(ch);
        }
    }
    path
}

// ── JSONL line types ─────────────────────────────────────────────────

#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
pub enum JsonlLine {
    #[serde(rename = "user")]
    User {
        message: Option<MessageEnvelope>,
        uuid: Option<String>,
        timestamp: Option<String>,
        cwd: Option<String>,
        #[serde(rename = "toolUseResult")]
        tool_use_result: Option<bool>,
        #[serde(rename = "sessionId")]
        session_id: Option<String>,
        #[serde(rename = "parentUuid")]
        parent_uuid: Option<String>,
    },
    #[serde(rename = "assistant")]
    Assistant {
        message: Option<MessageEnvelope>,
        uuid: Option<String>,
        timestamp: Option<String>,
        #[serde(rename = "parentUuid")]
        parent_uuid: Option<String>,
        cwd: Option<String>,
    },
    #[serde(rename = "system")]
    System {
        subtype: Option<String>,
        content: Option<serde_json::Value>,
        message: Option<MessageEnvelope>,
        timestamp: Option<String>,
        uuid: Option<String>,
        #[serde(rename = "sessionId")]
        session_id: Option<String>,
        cwd: Option<String>,
        #[serde(rename = "gitBranch")]
        git_branch: Option<String>,
    },
    #[serde(rename = "file-history-snapshot")]
    FileHistorySnapshot {
        #[serde(flatten)]
        _rest: serde_json::Value,
    },
    #[serde(rename = "last-prompt")]
    LastPrompt {
        #[serde(flatten)]
        _rest: serde_json::Value,
    },
}

#[derive(Deserialize, Debug)]
pub struct MessageEnvelope {
    pub role: Option<String>,
    pub content: Option<MessageContent>,
}

#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: Option<String>,
        name: Option<String>,
        input: Option<serde_json::Value>,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: Option<String>,
        content: Option<ToolResultContent>,
        is_error: Option<bool>,
    },
    #[serde(rename = "tool_reference")]
    ToolReference { tool_name: Option<String> },
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub enum ToolResultContent {
    Text(String),
    Blocks(Vec<ToolResultBlock>),
}

#[derive(Deserialize, Debug)]
pub struct ToolResultBlock {
    #[serde(rename = "type")]
    pub block_type: Option<String>,
    pub text: Option<String>,
    pub tool_name: Option<String>,
}

// ── parsed session ───────────────────────────────────────────────────

pub struct Session {
    pub id: String,
    pub project_path: String,
    pub turns: Vec<Turn>,
    pub first_timestamp: Option<String>,
    pub last_timestamp: Option<String>,
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
}

pub struct Turn {
    pub role: TurnRole,
    pub content: String,
    pub timestamp: Option<String>,
    pub tools: Vec<String>,
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

pub fn default_claude_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("projects"))
}

pub fn count_sessions(claude_dir: &str) -> Result<usize> {
    let mut count = 0usize;
    for entry in WalkDir::new(claude_dir).min_depth(2).max_depth(2) {
        let entry = entry?;
        if entry.path().extension().is_some_and(|e| e == "jsonl") {
            count += 1;
        }
    }
    Ok(count)
}

/// Extract single JSONL file -> rendered markdown.
pub fn extract_one(jsonl_path: &str) -> Result<String> {
    let path = Path::new(jsonl_path);
    let stem = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let project_dir_name = path
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let project_path = decode_dir_name(&project_dir_name);
    let session = parse_session(&stem, &project_path, jsonl_path)?;
    Ok(render_markdown(&session))
}

/// Walk all project dirs, return vec of (filename, markdown).
pub fn extract_all(claude_dir: &str, since: Option<&str>) -> Result<Vec<(String, String)>> {
    let mut results = Vec::new();

    for proj_entry in WalkDir::new(claude_dir).min_depth(1).max_depth(1) {
        let proj_entry = proj_entry?;
        if !proj_entry.file_type().is_dir() {
            continue;
        }
        let proj_dir_name = proj_entry.file_name().to_string_lossy().to_string();
        let project_path = decode_dir_name(&proj_dir_name);

        for file_entry in WalkDir::new(proj_entry.path()).min_depth(1).max_depth(1) {
            let file_entry = file_entry?;
            let fpath = file_entry.path();
            if fpath.extension().is_none_or(|e| e != "jsonl") {
                continue;
            }
            let stem = fpath
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let jsonl_str = fpath.to_string_lossy().to_string();

            match parse_session(&stem, &project_path, &jsonl_str) {
                Ok(session) => {
                    if let Some(since_ts) = since
                        && let Some(ref last) = session.last_timestamp
                        && last.as_str() < since_ts
                    {
                        continue;
                    }
                    let md = render_markdown(&session);
                    results.push((stem.clone(), md));
                }
                Err(e) => {
                    warn_once(&stem, &format!("{e}"));
                }
            }
        }
    }
    Ok(results)
}

// ── dry-run summary ──────────────────────────────────────────────────

pub struct SessionSummary {
    pub id: String,
    pub title: String,
    pub user_turns: usize,
    pub agent_turns: usize,
    pub tools: Vec<String>,
    pub first_ts: Option<String>,
    pub last_ts: Option<String>,
}

pub fn summarize_one(jsonl_path: &str) -> Result<SessionSummary> {
    let path = Path::new(jsonl_path);
    let stem = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let project_dir_name = path
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let project_path = decode_dir_name(&project_dir_name);
    let session = parse_session(&stem, &project_path, jsonl_path)?;

    let conv: Vec<&Turn> = session
        .turns
        .iter()
        .filter(|t| t.role == TurnRole::User || t.role == TurnRole::Agent)
        .collect();

    let title = conv
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
        .unwrap_or_else(|| "empty".to_string());

    let user_turns = conv.iter().filter(|t| t.role == TurnRole::User).count();
    let agent_turns = conv.iter().filter(|t| t.role == TurnRole::Agent).count();

    let mut tools_set: HashSet<String> = HashSet::new();
    for t in &session.turns {
        for tool in &t.tools {
            tools_set.insert(tool.clone());
        }
    }
    let mut tools: Vec<String> = tools_set.into_iter().collect();
    tools.sort();

    Ok(SessionSummary {
        id: stem,
        title,
        user_turns,
        agent_turns,
        tools,
        first_ts: session.first_timestamp,
        last_ts: session.last_timestamp,
    })
}

// ── parsing ──────────────────────────────────────────────────────────

fn parse_session(id: &str, project_path: &str, jsonl_path: &str) -> Result<Session> {
    let raw = fs::read_to_string(jsonl_path).with_context(|| format!("reading {jsonl_path}"))?;

    let mut turns: Vec<Turn> = Vec::new();
    let mut first_ts: Option<String> = None;
    let mut last_ts: Option<String> = None;
    let mut cwd: Option<String> = None;
    let mut git_branch: Option<String> = None;
    let mut tool_names: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let parsed: JsonlLine = match serde_json::from_str(line) {
            Ok(p) => p,
            Err(_) => continue,
        };

        match parsed {
            JsonlLine::FileHistorySnapshot { .. } | JsonlLine::LastPrompt { .. } => continue,

            JsonlLine::System {
                timestamp,
                cwd: sys_cwd,
                git_branch: branch,
                ..
            } => {
                if let Some(ts) = &timestamp {
                    update_timestamps(&mut first_ts, &mut last_ts, ts);
                }
                if cwd.is_none() {
                    cwd = sys_cwd;
                }
                if git_branch.is_none() {
                    git_branch = branch;
                }
            }

            JsonlLine::User {
                message,
                timestamp,
                cwd: user_cwd,
                tool_use_result,
                ..
            } => {
                if let Some(ts) = &timestamp {
                    update_timestamps(&mut first_ts, &mut last_ts, ts);
                }
                if cwd.is_none() {
                    cwd = user_cwd;
                }

                let is_tool_result = tool_use_result.unwrap_or(false);
                if is_tool_result {
                    if let Some(msg) = message {
                        let rendered = render_tool_result_content(&msg, &tool_names);
                        if !rendered.is_empty() {
                            if let Some(last) = turns.last_mut()
                                && last.role == TurnRole::Agent
                            {
                                last.content.push('\n');
                                last.content.push_str(&rendered);
                                continue;
                            }
                            turns.push(Turn {
                                role: TurnRole::System,
                                content: rendered,
                                timestamp: timestamp.clone(),
                                tools: vec![],
                            });
                        }
                    }
                } else if let Some(msg) = message {
                    let text = render_user_content(&msg);
                    if !text.is_empty() {
                        turns.push(Turn {
                            role: TurnRole::User,
                            content: text,
                            timestamp: timestamp.clone(),
                            tools: vec![],
                        });
                    }
                }
            }

            JsonlLine::Assistant {
                message,
                timestamp,
                cwd: asst_cwd,
                ..
            } => {
                if let Some(ts) = &timestamp {
                    update_timestamps(&mut first_ts, &mut last_ts, ts);
                }
                if cwd.is_none() {
                    cwd = asst_cwd;
                }
                if let Some(msg) = message {
                    let (text, tools) = render_assistant_content(&msg, &mut tool_names);
                    if !text.is_empty() {
                        turns.push(Turn {
                            role: TurnRole::Agent,
                            content: text,
                            timestamp: timestamp.clone(),
                            tools,
                        });
                    }
                }
            }
        }
    }

    Ok(Session {
        id: id.to_string(),
        project_path: project_path.to_string(),
        turns,
        first_timestamp: first_ts,
        last_timestamp: last_ts,
        cwd,
        git_branch,
    })
}

fn update_timestamps(first: &mut Option<String>, last: &mut Option<String>, ts: &str) {
    if first.is_none() || first.as_deref().is_some_and(|f| ts < f) {
        *first = Some(ts.to_string());
    }
    if last.is_none() || last.as_deref().is_some_and(|l| ts > l) {
        *last = Some(ts.to_string());
    }
}

fn render_user_content(msg: &MessageEnvelope) -> String {
    match &msg.content {
        Some(MessageContent::Text(s)) => s.clone(),
        Some(MessageContent::Blocks(blocks)) => {
            let mut out = String::new();
            for b in blocks {
                if let ContentBlock::Text { text } = b {
                    out.push_str(text);
                }
            }
            out
        }
        None => String::new(),
    }
}

fn render_assistant_content(
    msg: &MessageEnvelope,
    tool_names: &mut std::collections::HashMap<String, String>,
) -> (String, Vec<String>) {
    let mut out = String::new();
    let mut tools = Vec::new();

    match &msg.content {
        Some(MessageContent::Text(s)) => {
            out.push_str(s);
        }
        Some(MessageContent::Blocks(blocks)) => {
            for b in blocks {
                match b {
                    ContentBlock::Text { text } => {
                        if !out.is_empty() && !out.ends_with('\n') {
                            out.push_str("\n\n");
                        }
                        out.push_str(text);
                    }
                    ContentBlock::ToolUse { id, name, input } => {
                        let tool_name = name.as_deref().unwrap_or("unknown");
                        tools.push(tool_name.to_string());
                        if let Some(tid) = id {
                            tool_names.insert(tid.clone(), tool_name.to_string());
                        }
                        if !out.is_empty() && !out.ends_with('\n') {
                            out.push('\n');
                        }
                        out.push_str(&format!("\n> **tool: {tool_name}**\n"));
                        if let Some(inp) = input
                            && let Some(obj) = inp.as_object()
                        {
                            for (k, v) in obj {
                                let vs = if let Some(s) = v.as_str() {
                                    if s.len() > 200 {
                                        format!("{}...", safe_truncate(s, 200))
                                    } else {
                                        s.to_string()
                                    }
                                } else {
                                    let j = v.to_string();
                                    if j.len() > 200 {
                                        format!("{}...", safe_truncate(&j, 200))
                                    } else {
                                        j
                                    }
                                };
                                out.push_str(&format!(">   {k}: {vs}\n"));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        None => {}
    }
    (out, tools)
}

fn render_tool_result_content(
    msg: &MessageEnvelope,
    tool_names: &std::collections::HashMap<String, String>,
) -> String {
    let mut out = String::new();
    match &msg.content {
        Some(MessageContent::Blocks(blocks)) => {
            for b in blocks {
                if let ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } = b
                {
                    let name = tool_use_id
                        .as_ref()
                        .and_then(|tid| tool_names.get(tid))
                        .map(|s| s.as_str())
                        .unwrap_or("?");
                    let err_marker = if is_error.unwrap_or(false) {
                        " ERROR"
                    } else {
                        ""
                    };

                    let summary = match content {
                        Some(ToolResultContent::Text(s)) => {
                            if s.len() > 500 {
                                format!("{}...[truncated]", safe_truncate(s, 500))
                            } else {
                                s.clone()
                            }
                        }
                        Some(ToolResultContent::Blocks(blocks)) => {
                            let mut combined = String::new();
                            for rb in blocks {
                                if let Some(t) = &rb.text {
                                    if !combined.is_empty() {
                                        combined.push('\n');
                                    }
                                    combined.push_str(t);
                                }
                                if let Some(tn) = &rb.tool_name {
                                    if !combined.is_empty() {
                                        combined.push('\n');
                                    }
                                    combined.push_str(&format!("[ref: {tn}]"));
                                }
                            }
                            if combined.len() > 500 {
                                format!("{}...[truncated]", safe_truncate(&combined, 500))
                            } else {
                                combined
                            }
                        }
                        None => String::new(),
                    };

                    out.push_str(&format!("\n> **result: {name}**{err_marker}\n"));
                    for line in summary.lines() {
                        out.push_str(&format!(">   {line}\n"));
                    }
                }
            }
        }
        Some(MessageContent::Text(s)) => {
            let summary = if s.len() > 500 {
                format!("{}...[truncated]", safe_truncate(s, 500))
            } else {
                s.clone()
            };
            out.push_str(&format!("\n> **result: ?**\n>   {summary}\n"));
        }
        None => {}
    }
    out
}

// ── markdown rendering ───────────────────────────────────────────────

fn render_markdown(session: &Session) -> String {
    let conv: Vec<&Turn> = session
        .turns
        .iter()
        .filter(|t| t.role == TurnRole::User || t.role == TurnRole::Agent)
        .collect();

    let title = conv
        .iter()
        .find(|t| t.role == TurnRole::User)
        .map(|t| {
            let s = t.content.trim();
            if s.len() > 60 {
                format!("{}...", safe_truncate(s, 60))
            } else {
                s.to_string()
            }
        })
        .unwrap_or_else(|| "Claude Code session".to_string())
        .replace('"', "\\\"")
        .replace('\n', " ");

    let created = session.first_timestamp.as_deref().unwrap_or("unknown");
    let updated = session.last_timestamp.as_deref().unwrap_or("unknown");

    let (user_turns, agent_turns) = conv
        .iter()
        .fold((0usize, 0usize), |(u, a), t| match t.role {
            TurnRole::User => (u + 1, a),
            TurnRole::Agent => (u, a + 1),
            _ => (u, a),
        });

    let mut all_tools: HashSet<String> = HashSet::new();
    for t in &session.turns {
        for tool in &t.tools {
            all_tools.insert(tool.clone());
        }
    }
    let mut tools_sorted: Vec<String> = all_tools.into_iter().collect();
    tools_sorted.sort();
    let tools_yaml = if tools_sorted.is_empty() {
        "[]".to_string()
    } else {
        format!("[{}]", tools_sorted.join(", "))
    };

    let branch_line = session
        .git_branch
        .as_ref()
        .map(|b| format!("branch: \"{b}\"\n"))
        .unwrap_or_default();

    let folder_path = if session.project_path.is_empty() {
        session.cwd.as_deref().unwrap_or("/unknown").to_string()
    } else {
        session.project_path.clone()
    };

    let mut turns_detail = String::from("turns_detail:\n");
    let mut tn = 0usize;
    for t in &session.turns {
        if t.role != TurnRole::User && t.role != TurnRole::Agent {
            continue;
        }
        tn += 1;
        turns_detail.push_str(&format!("  - turn: {}\n    role: {}\n", tn, t.role.label()));
        if !t.tools.is_empty() {
            turns_detail.push_str(&format!("    tools: [{}]\n", t.tools.join(", ")));
        }
        turns_detail.push_str(&format!("    content_chars: {}\n", t.content.len()));
    }

    let total_turns = user_turns + agent_turns;

    let mut md = format!(
        "---\nschema_version: 1\nid: claude-{id}\ntype: transcript\nsource: claude-code\nstatus: done\ntitle: \"{title}\"\ncreated_at: {created}\nupdated_at: {updated}\nmodel:\n  provider: anthropic\n  model: claude\nturns_user: {user_turns}\nturns_agent: {agent_turns}\nturns_total: {total_turns}\ntools_used: {tools_yaml}\nfolder_paths:\n  - \"{folder_path}\"\n{branch_line}{turns_detail}---\n\n# {title}\n\n",
        id = session.id,
        title = title,
        created = created,
        updated = updated,
        user_turns = user_turns,
        agent_turns = agent_turns,
        total_turns = total_turns,
        tools_yaml = tools_yaml,
        folder_path = folder_path,
        branch_line = branch_line,
        turns_detail = turns_detail,
    );

    let mut tn = 0usize;
    for turn in &session.turns {
        if turn.role != TurnRole::User && turn.role != TurnRole::Agent {
            continue;
        }
        tn += 1;
        md.push_str(&format!(
            "## Turn {} -- {}\n\n{}\n\n",
            tn,
            turn.role.label(),
            turn.content.trim()
        ));
    }

    md
}

// -- TranscriptExtractor trait impl ------------------------------------------

use transcriptd_core::TranscriptExtractor;

pub struct ClaudeExtractor;

impl TranscriptExtractor for ClaudeExtractor {
    fn name(&self) -> &str {
        "claude-code"
    }

    fn default_source_path(&self) -> Option<PathBuf> {
        default_claude_dir()
    }

    fn count(&self, source: &Path) -> Result<usize> {
        count_sessions(&source.to_string_lossy())
    }

    fn extract_all(&self, source: &Path, since: Option<&str>) -> Result<Vec<(String, String)>> {
        extract_all(&source.to_string_lossy(), since)
    }

    fn extract_one(&self, source: &Path, id: &str) -> Result<String> {
        // id is the JSONL filename stem; resolve to full path within source dir
        // Walk source to find matching .jsonl file
        for entry in WalkDir::new(source).min_depth(2).max_depth(2) {
            if let Ok(entry) = entry
                && entry
                    .path()
                    .file_stem()
                    .is_some_and(|s| s.to_string_lossy() == id)
            {
                return extract_one(&entry.path().to_string_lossy());
            }
        }
        anyhow::bail!("session {id} not found in {}", source.display())
    }

    fn watch_paths(&self, source: &Path) -> Vec<PathBuf> {
        // Watch the entire projects directory for new/modified JSONL files
        vec![source.to_path_buf()]
    }
}
