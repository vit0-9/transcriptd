use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

// ── helpers ──────────────────────────────────────────────────────────

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

// ── types ────────────────────────────────────────────────────────────

#[derive(Deserialize, Debug)]
pub struct ZedThread {
    pub title: Option<String>,
    pub messages: Vec<ZedMessage>,
    pub updated_at: Option<String>,
    pub model: Option<ZedModel>,
    pub request_token_usage: Option<HashMap<String, TokenUsage>>,
    #[serde(default)]
    pub cumulative_token_usage: serde_json::Value,
    pub version: Option<String>,
    pub thinking_enabled: Option<bool>,
    pub thinking_effort: Option<String>,
    pub speed: Option<serde_json::Value>,
    pub detailed_summary: Option<serde_json::Value>,
    #[serde(default)]
    pub initial_project_snapshot: serde_json::Value,
}

#[derive(Deserialize, Debug)]
pub struct ZedModel {
    pub provider: Option<String>,
    pub model: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct TokenUsage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
}

#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub enum ZedMessage {
    Tagged(HashMap<String, ZedMessageBody>),
    Signal(String),
}

#[derive(Deserialize, Debug)]
pub struct ZedMessageBody {
    pub id: Option<String>,
    pub content: Option<Vec<ContentItem>>,
    pub tool_results: Option<HashMap<String, ToolResult>>,
    #[serde(default)]
    pub reasoning_details: serde_json::Value,
}

#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub enum ContentItem {
    Tagged(HashMap<String, serde_json::Value>),
}

#[derive(Deserialize, Debug)]
pub struct ToolResult {
    pub tool_use_id: Option<String>,
    pub tool_name: Option<String>,
    pub is_error: Option<bool>,
    pub content: Option<serde_json::Value>,
    pub output: Option<serde_json::Value>,
}

pub struct ThreadRow {
    pub id: String,
    pub summary: String,
    pub updated_at: String,
    pub created_at: Option<String>,
    pub data: Vec<u8>,
    pub folder_paths: Option<String>,
    pub worktree_branch: Option<String>,
}

pub struct TokenStats {
    pub total_in: u64,
    pub total_out: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

pub struct TurnMeta {
    pub number: usize,
    pub role: String,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub tools: Vec<String>,
    pub has_thinking: bool,
    pub content_len: usize,
}

// ── core functions ───────────────────────────────────────────────────

pub fn decompress_zstd(data: &[u8]) -> Result<Vec<u8>> {
    Ok(zstd::decode_all(data)?)
}

pub fn parse_thread(json_bytes: &[u8]) -> Result<ZedThread> {
    Ok(serde_json::from_slice(json_bytes)?)
}

pub fn aggregate_tokens(usage: &Option<HashMap<String, TokenUsage>>) -> TokenStats {
    let mut s = TokenStats {
        total_in: 0,
        total_out: 0,
        cache_read: 0,
        cache_write: 0,
    };
    if let Some(map) = usage {
        for u in map.values() {
            s.total_in +=
                u.input_tokens + u.cache_creation_input_tokens + u.cache_read_input_tokens;
            s.total_out += u.output_tokens;
            s.cache_read += u.cache_read_input_tokens;
            s.cache_write += u.cache_creation_input_tokens;
        }
    }
    s
}

pub fn extract_tools(thread: &ZedThread) -> Vec<String> {
    let mut tools = HashSet::new();
    for msg in &thread.messages {
        let map = match msg {
            ZedMessage::Tagged(map) => map,
            ZedMessage::Signal(_) => continue,
        };
        for body in map.values() {
            if let Some(content) = &body.content {
                for item in content {
                    let ContentItem::Tagged(m) = item;
                    if let Some(tu) = m.get("ToolUse")
                        && let Some(name) = tu.get("name").and_then(|v| v.as_str())
                    {
                        tools.insert(name.to_string());
                    }
                }
            }
        }
    }
    let mut v: Vec<String> = tools.into_iter().collect();
    v.sort();
    v
}

/// Extract all mention URIs from a thread (files, directories, threads referenced by user).
pub fn extract_mentions(thread: &ZedThread) -> Vec<String> {
    let mut mentions = Vec::new();
    for msg in &thread.messages {
        let map = match msg {
            ZedMessage::Tagged(map) => map,
            ZedMessage::Signal(_) => continue,
        };
        if let Some(body) = map.get("User")
            && let Some(content) = &body.content
        {
            for item in content {
                let ContentItem::Tagged(m) = item;
                if let Some(mention) = m.get("Mention")
                    && let Some(obj) = mention.as_object()
                    && let Some(uri_val) = obj.get("uri")
                {
                    if let Some(inner) = uri_val.as_object() {
                        if let Some(p) = inner
                            .get("File")
                            .and_then(|v| v.get("abs_path"))
                            .and_then(|v| v.as_str())
                        {
                            mentions.push(format!("file://{p}"));
                        } else if let Some(p) = inner
                            .get("Directory")
                            .and_then(|v| v.get("abs_path"))
                            .and_then(|v| v.as_str())
                        {
                            mentions.push(format!("dir://{p}"));
                        } else if let Some(t) = inner.get("Thread").and_then(|v| v.as_object()) {
                            let name = t.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                            mentions.push(format!("thread://{name}"));
                        } else if let Some(p) = inner
                            .get("Selection")
                            .and_then(|v| v.get("abs_path"))
                            .and_then(|v| v.as_str())
                        {
                            mentions.push(format!("selection://{p}"));
                        } else if let Some(url) = inner
                            .get("Fetch")
                            .and_then(|v| v.get("url"))
                            .and_then(|v| v.as_str())
                        {
                            mentions.push(url.to_string());
                        }
                    } else if let Some(s) = uri_val.as_str() {
                        mentions.push(s.to_string());
                    }
                }
            }
        }
    }
    mentions
}

pub fn count_turns(thread: &ZedThread) -> (usize, usize) {
    let (mut u, mut a) = (0, 0);
    for msg in &thread.messages {
        let map = match msg {
            ZedMessage::Tagged(map) => map,
            ZedMessage::Signal(_) => continue,
        };
        if map.contains_key("User") {
            u += 1;
        }
        if map.contains_key("Agent") {
            a += 1;
        }
    }
    (u, a)
}

pub fn build_turn_token_map(thread: &ZedThread) -> HashMap<String, (usize, &TokenUsage)> {
    let mut map = HashMap::new();
    let mut turn = 0;
    for msg in &thread.messages {
        match msg {
            ZedMessage::Tagged(m) => {
                if let Some(body) = m.get("User") {
                    turn += 1;
                    if let Some(id) = &body.id
                        && let Some(usage_map) = &thread.request_token_usage
                        && let Some(usage) = usage_map.get(id)
                    {
                        map.insert(id.clone(), (turn, usage));
                    }
                }
                if m.contains_key("Agent") {
                    turn += 1;
                }
            }
            ZedMessage::Signal(_) => {}
        }
    }
    map
}

pub fn collect_turn_metas(thread: &ZedThread) -> Vec<TurnMeta> {
    let mut metas = Vec::new();
    let mut turn = 0;

    let mut user_tokens: HashMap<String, &TokenUsage> = HashMap::new();
    if let Some(usage_map) = &thread.request_token_usage {
        for msg in &thread.messages {
            if let ZedMessage::Tagged(m) = msg
                && let Some(body) = m.get("User")
                && let Some(id) = &body.id
                && let Some(usage) = usage_map.get(id)
            {
                user_tokens.insert(id.clone(), usage);
            }
        }
    }

    let mut _last_user_id: Option<String> = None;

    for msg in &thread.messages {
        match msg {
            ZedMessage::Signal(s) => {
                turn += 1;
                metas.push(TurnMeta {
                    number: turn,
                    role: "system".to_string(),
                    tokens_in: 0,
                    tokens_out: 0,
                    cache_read: 0,
                    cache_write: 0,
                    tools: vec![],
                    has_thinking: false,
                    content_len: s.len(),
                });
            }
            ZedMessage::Tagged(m) => {
                if let Some(body) = m.get("User") {
                    turn += 1;
                    _last_user_id = body.id.clone();
                    let tu = body.id.as_ref().and_then(|id| user_tokens.get(id));
                    let (tin, tout, cr, cw) = if let Some(u) = tu {
                        (
                            u.input_tokens
                                + u.cache_creation_input_tokens
                                + u.cache_read_input_tokens,
                            u.output_tokens,
                            u.cache_read_input_tokens,
                            u.cache_creation_input_tokens,
                        )
                    } else {
                        (0, 0, 0, 0)
                    };
                    let content_len = body
                        .content
                        .as_ref()
                        .map(|c| {
                            c.iter()
                                .map(|item| {
                                    let ContentItem::Tagged(m) = item;
                                    m.get("Text")
                                        .and_then(|v| v.as_str())
                                        .map(|s| s.len())
                                        .unwrap_or(0)
                                })
                                .sum()
                        })
                        .unwrap_or(0);
                    metas.push(TurnMeta {
                        number: turn,
                        role: "user".to_string(),
                        tokens_in: tin,
                        tokens_out: tout,
                        cache_read: cr,
                        cache_write: cw,
                        tools: vec![],
                        has_thinking: false,
                        content_len,
                    });
                }
                if let Some(body) = m.get("Agent") {
                    turn += 1;
                    let mut tools = Vec::new();
                    let mut has_thinking = false;
                    let mut content_len = 0usize;
                    if let Some(content) = &body.content {
                        for item in content {
                            let ContentItem::Tagged(m) = item;
                            if m.contains_key("Thinking") {
                                has_thinking = true;
                            }
                            if let Some(tu) = m.get("ToolUse")
                                && let Some(name) = tu.get("name").and_then(|v| v.as_str())
                            {
                                tools.push(name.to_string());
                            }
                            if let Some(text) = m.get("Text").and_then(|v| v.as_str()) {
                                content_len += text.len();
                            }
                        }
                    }
                    metas.push(TurnMeta {
                        number: turn,
                        role: "agent".to_string(),
                        tokens_in: 0,
                        tokens_out: 0,
                        cache_read: 0,
                        cache_write: 0,
                        tools,
                        has_thinking,
                        content_len,
                    });
                }
            }
        }
    }
    metas
}

pub fn render_content_item(item: &ContentItem) -> String {
    let ContentItem::Tagged(m) = item;
    if let Some(text) = m.get("Text").and_then(|v| v.as_str()) {
        return text.to_string();
    }
    if let Some(thinking) = m.get("Thinking")
        && let Some(obj) = thinking.as_object()
        && let Some(text) = obj.get("text").and_then(|v| v.as_str())
    {
        return format!("<details>\n<summary>thinking</summary>\n\n{text}\n</details>");
    }
    if let Some(tu) = m.get("ToolUse") {
        let name = tu.get("name").and_then(|v| v.as_str()).unwrap_or("unknown");
        let input = tu
            .get("input")
            .map(|v| {
                if let Some(obj) = v.as_object() {
                    obj.iter()
                        .map(|(k, v)| {
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
                            format!("  {k}: {vs}")
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                } else {
                    String::new()
                }
            })
            .unwrap_or_default();
        return format!("> **tool: {name}**\n{input}");
    }
    if let Some(mention) = m.get("Mention")
        && let Some(obj) = mention.as_object()
    {
        let uri_display = if let Some(uri_val) = obj.get("uri") {
            if let Some(s) = uri_val.as_str() {
                s.to_string()
            } else if let Some(inner) = uri_val.as_object() {
                // Tagged enum: {"File":{"abs_path":"/..."}}, {"Directory":{"abs_path":"/..."}},
                //              {"Thread":{"id":"...","name":"..."}}, {"Selection":{"abs_path":"/..."}}
                if let Some(file) = inner
                    .get("File")
                    .and_then(|v| v.get("abs_path"))
                    .and_then(|v| v.as_str())
                {
                    format!("file://{file}")
                } else if let Some(dir) = inner
                    .get("Directory")
                    .and_then(|v| v.get("abs_path"))
                    .and_then(|v| v.as_str())
                {
                    format!("dir://{dir}")
                } else if let Some(thread) = inner.get("Thread").and_then(|v| v.as_object()) {
                    let name = thread.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                    let id = thread.get("id").and_then(|v| v.as_str()).unwrap_or("");
                    format!("thread://{name} ({id})")
                } else if let Some(sel) = inner
                    .get("Selection")
                    .and_then(|v| v.get("abs_path"))
                    .and_then(|v| v.as_str())
                {
                    format!("selection://{sel}")
                } else if let Some(fetch) = inner
                    .get("Fetch")
                    .and_then(|v| v.get("url"))
                    .and_then(|v| v.as_str())
                {
                    fetch.to_string()
                } else {
                    uri_val.to_string()
                }
            } else {
                String::new()
            }
        } else {
            String::new()
        };
        let content_preview = obj
            .get("content")
            .and_then(|v| v.as_str())
            .map(|c| {
                if c.len() > 200 {
                    format!("{}...", safe_truncate(c, 200))
                } else {
                    c.to_string()
                }
            })
            .unwrap_or_default();
        if content_preview.is_empty() {
            return format!("[mention: {uri_display}]");
        }
        return format!("[mention: {uri_display}]\n> {content_preview}");
    }
    String::new()
}

pub fn render_tool_results(results: &HashMap<String, ToolResult>) -> String {
    let mut out = String::new();
    for tr in results.values() {
        let name = tr.tool_name.as_deref().unwrap_or("?");
        let err_marker = if tr.is_error.unwrap_or(false) {
            " ERROR"
        } else {
            ""
        };
        let content_str = tr
            .output
            .as_ref()
            .or(tr.content.as_ref())
            .map(|v| {
                if let Some(obj) = v.as_object()
                    && let Some(text) = obj.get("Text").and_then(|v2| v2.as_str())
                {
                    return if text.len() > 500 {
                        format!("{}...[truncated]", safe_truncate(text, 500))
                    } else {
                        text.to_string()
                    };
                }
                let j = v.to_string();
                if j.len() > 500 {
                    format!("{}...[truncated]", safe_truncate(&j, 500))
                } else {
                    j
                }
            })
            .unwrap_or_default();
        out.push_str(&format!(
            "\n> **result: {name}**{err_marker}\n> {content_str}\n"
        ));
    }
    out
}

pub fn render_markdown(row: &ThreadRow, thread: &ZedThread) -> Result<String> {
    let (user_turns, agent_turns) = count_turns(thread);
    let tokens = aggregate_tokens(&thread.request_token_usage);
    let tools = extract_tools(thread);
    let mentions = extract_mentions(thread);

    let model_provider = thread
        .model
        .as_ref()
        .and_then(|m| m.provider.as_deref())
        .unwrap_or("unknown");
    let model_name = thread
        .model
        .as_ref()
        .and_then(|m| m.model.as_deref())
        .unwrap_or("unknown");

    let folder_paths_yaml = row
        .folder_paths
        .as_ref()
        .map(|fp| {
            fp.split(',')
                .map(|p| format!("  - \"{}\"", p.trim()))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_else(|| "  - \"/unknown\"".to_string());

    let tools_yaml = if tools.is_empty() {
        "[]".to_string()
    } else {
        format!("[{}]", tools.join(", "))
    };

    let mentions_yaml = if mentions.is_empty() {
        String::new()
    } else {
        let items: Vec<String> = mentions
            .iter()
            .map(|m| format!("  - \"{}\"", m.replace('"', "\\\"")))
            .collect();
        format!("mentions:\n{}\n", items.join("\n"))
    };

    let title = thread
        .title
        .as_deref()
        .unwrap_or(&row.summary)
        .replace('"', "\\\"");
    let created = row.created_at.as_deref().unwrap_or(&row.updated_at);
    let updated = &row.updated_at;
    let total_turns = user_turns + agent_turns;

    let thinking_line = thread
        .thinking_enabled
        .map(|t| format!("thinking_enabled: {t}\n"))
        .unwrap_or_default();
    let version_line = thread
        .version
        .as_ref()
        .map(|v| format!("thread_version: \"{v}\"\n"))
        .unwrap_or_default();
    let branch_line = row
        .worktree_branch
        .as_ref()
        .map(|b| format!("branch: \"{b}\"\n"))
        .unwrap_or_default();

    let turn_metas = collect_turn_metas(thread);
    let turns_detail = if turn_metas
        .iter()
        .any(|t| t.tokens_in > 0 || !t.tools.is_empty())
    {
        let mut s = String::from("turns_detail:\n");
        for t in &turn_metas {
            s.push_str(&format!("  - turn: {}\n    role: {}\n", t.number, t.role));
            if t.tokens_in > 0 || t.tokens_out > 0 {
                s.push_str(&format!(
                    "    tokens_in: {}\n    tokens_out: {}\n",
                    t.tokens_in, t.tokens_out
                ));
                s.push_str(&format!(
                    "    cache_read: {}\n    cache_write: {}\n",
                    t.cache_read, t.cache_write
                ));
            }
            if !t.tools.is_empty() {
                s.push_str(&format!("    tools: [{}]\n", t.tools.join(", ")));
            }
            if t.has_thinking {
                s.push_str("    has_thinking: true\n");
            }
            s.push_str(&format!("    content_chars: {}\n", t.content_len));
        }
        s
    } else {
        String::new()
    };

    let mut md = format!(
        "---
schema_version: 1
id: zed-{id}
type: transcript
source: zed
status: done
title: \"{title}\"
created_at: {created}
updated_at: {updated}
strategic_parent: /unknown
model:
  provider: {model_provider}
  model: {model_name}
turns_user: {user_turns}
turns_agent: {agent_turns}
turns_total: {total_turns}
tokens_in: {tokens_in}
tokens_out: {tokens_out}
tokens_cache_read: {cache_read}
tokens_cache_write: {cache_write}
tools_used: {tools_yaml}
folder_paths:
{folder_paths_yaml}
{thinking_line}{version_line}{branch_line}{mentions_yaml}{turns_detail}---

# {title}

",
        id = row.id,
        title = title,
        created = created,
        updated = updated,
        model_provider = model_provider,
        model_name = model_name,
        user_turns = user_turns,
        agent_turns = agent_turns,
        total_turns = total_turns,
        tokens_in = tokens.total_in,
        tokens_out = tokens.total_out,
        cache_read = tokens.cache_read,
        cache_write = tokens.cache_write,
        tools_yaml = tools_yaml,
        folder_paths_yaml = folder_paths_yaml,
        thinking_line = thinking_line,
        version_line = version_line,
        branch_line = branch_line,
        mentions_yaml = mentions_yaml,
        turns_detail = turns_detail,
    );

    let mut turn_num = 0;
    for msg in &thread.messages {
        let map = match msg {
            ZedMessage::Tagged(map) => map,
            ZedMessage::Signal(_) => continue,
        };
        if let Some(body) = map.get("User") {
            turn_num += 1;
            let turn_cost = body
                .id
                .as_ref()
                .and_then(|uid| thread.request_token_usage.as_ref()?.get(uid));
            if let Some(tc) = turn_cost {
                let tin =
                    tc.input_tokens + tc.cache_creation_input_tokens + tc.cache_read_input_tokens;
                md.push_str(&format!("## Turn {turn_num} -- user\n\n<!-- tokens: in={} out={} cache_read={} cache_write={} -->\n\n", tin, tc.output_tokens, tc.cache_read_input_tokens, tc.cache_creation_input_tokens));
            } else {
                md.push_str(&format!("## Turn {turn_num} -- user\n\n"));
            }
            if let Some(content) = &body.content {
                for item in content {
                    let rendered = render_content_item(item);
                    if !rendered.is_empty() {
                        md.push_str(&rendered);
                        md.push_str("\n\n");
                    }
                }
            }
        }
        if let Some(body) = map.get("Agent") {
            turn_num += 1;
            md.push_str(&format!("## Turn {turn_num} -- agent\n\n"));
            if let Some(content) = &body.content {
                for item in content {
                    let rendered = render_content_item(item);
                    if !rendered.is_empty() {
                        md.push_str(&rendered);
                        md.push_str("\n\n");
                    }
                }
            }
            if let Some(results) = &body.tool_results {
                md.push_str(&render_tool_results(results));
                md.push('\n');
            }
        }
    }
    Ok(md)
}

// ── high-level API ───────────────────────────────────────────────────

fn open_db(db_path: &str) -> Result<Connection> {
    Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("Failed to open {db_path}"))
}

fn query_rows(conn: &Connection, extra_where: Option<&str>, order: &str) -> Result<Vec<ThreadRow>> {
    let mut query = String::from(
        "SELECT id, summary, updated_at, created_at, data, folder_paths, worktree_branch FROM threads",
    );
    if let Some(w) = extra_where {
        query.push_str(" WHERE ");
        query.push_str(w);
    }
    query.push_str(" ORDER BY ");
    query.push_str(order);

    let mut stmt = conn.prepare(&query)?;
    let rows = stmt.query_map([], |row| {
        Ok(ThreadRow {
            id: row.get(0)?,
            summary: row.get(1)?,
            updated_at: row.get(2)?,
            created_at: row.get(3)?,
            data: row.get(4)?,
            folder_paths: row.get(5)?,
            worktree_branch: row.get(6)?,
        })
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

/// Extract all threads from a Zed threads.db, return rendered markdown strings.
/// Each item is (thread_id, rendered_markdown).
pub fn extract_all(db_path: &str, since: Option<&str>) -> Result<Vec<(String, String)>> {
    let conn = open_db(db_path)?;
    let wh = since.map(|s| format!("updated_at > '{s}'"));
    let rows = query_rows(&conn, wh.as_deref(), "updated_at ASC")?;

    let mut results = Vec::new();
    for row in rows {
        let json_bytes = decompress_zstd(&row.data)?;
        let thread = parse_thread(&json_bytes)?;
        let md = render_markdown(&row, &thread)?;
        results.push((row.id.clone(), md));
    }
    Ok(results)
}

/// Extract a single thread by ID.
pub fn extract_one(db_path: &str, thread_id: &str) -> Result<String> {
    let conn = open_db(db_path)?;
    let wh = format!("id = '{thread_id}'");
    let rows = query_rows(&conn, Some(&wh), "updated_at ASC")?;
    let row = rows
        .into_iter()
        .next()
        .with_context(|| format!("Thread {thread_id} not found"))?;
    let json_bytes = decompress_zstd(&row.data)?;
    let thread = parse_thread(&json_bytes)?;
    render_markdown(&row, &thread)
}

/// Count total threads in DB.
pub fn count_threads(db_path: &str) -> Result<usize> {
    let conn = open_db(db_path)?;
    let n: i64 = conn.query_row("SELECT count(*) FROM threads", [], |r| r.get(0))?;
    Ok(n as usize)
}

/// Find the Zed threads.db path on this system (platform-aware).
pub fn default_threads_db() -> Option<PathBuf> {
    // Zed stores threads in a dedicated SQLite DB
    // macOS: ~/Library/Application Support/Zed/threads/threads.db
    // Linux: ~/.local/share/Zed/threads/threads.db (or XDG_DATA_HOME)
    dirs::data_dir().map(|d| d.join("Zed").join("threads").join("threads.db"))
}

// -- TranscriptExtractor trait impl ------------------------------------------

use transcriptd_core::TranscriptExtractor;

pub struct ZedExtractor;

impl TranscriptExtractor for ZedExtractor {
    fn name(&self) -> &str {
        "zed"
    }

    fn default_source_path(&self) -> Option<PathBuf> {
        default_threads_db()
    }

    fn count(&self, source: &Path) -> Result<usize> {
        count_threads(&source.to_string_lossy())
    }

    fn extract_all(&self, source: &Path, since: Option<&str>) -> Result<Vec<(String, String)>> {
        extract_all(&source.to_string_lossy(), since)
    }

    fn extract_one(&self, source: &Path, id: &str) -> Result<String> {
        extract_one(&source.to_string_lossy(), id)
    }

    fn watch_paths(&self, source: &Path) -> Vec<PathBuf> {
        // Watch the threads.db file itself and its WAL
        let mut paths = vec![source.to_path_buf()];
        let wal = source.with_extension("db-wal");
        if wal.exists() {
            paths.push(wal);
        }
        paths
    }
}
