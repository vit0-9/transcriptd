use transcriptd_store::TranscriptRecord;

/// Canonicalize a raw id into `{source}-{uuid}` form.
///
/// See ADR-001 (docs/ADR-001-TRANSCRIPT-ID.md) for the full specification.
///
/// Input variants seen in the wild:
/// - Raw UUID: `032103b0-...`
/// - Already-prefixed: `zed-032103b0-...`
/// - With extension: `claude-abc.md`
/// - With date path: `2026/04/18/vscode-abc.md`
///
/// Output is always `{source}-{native_id}` with no path, no extension, single prefix.
/// This function is idempotent: `canonical_id(canonical_id(x, s), s) == canonical_id(x, s)`.
pub fn canonical_id(raw: &str, source: &str) -> String {
    // Strip path components
    let stem = raw.rsplit('/').next().unwrap_or(raw);
    // Strip .md extension
    let stem = stem.strip_suffix(".md").unwrap_or(stem);
    // Strip the correct source prefix if already present
    let prefix = format!("{source}-");
    let body = stem.strip_prefix(&prefix).unwrap_or(stem);
    // Handle legacy wrong prefixes from older extractors:
    // Claude extractor used to emit "claude-{uuid}" but source is "claude-code"
    // VSCode extractor used to emit "vscode-{uuid}" but source is "vscode-copilot"
    let body = match source {
        "claude-code" => body.strip_prefix("claude-").unwrap_or(body),
        "vscode-copilot" => body.strip_prefix("vscode-").unwrap_or(body),
        _ => body,
    };
    format!("{source}-{body}")
}

pub fn parse_md_to_record(id: &str, source: &str, md: &str) -> TranscriptRecord {
    let id = canonical_id(id, source);
    // Parse YAML frontmatter if present
    let (meta, body) = if let Some(stripped) = md.strip_prefix("---\n") {
        if let Some(end) = stripped.find("\n---\n") {
            let yaml_str = &stripped[..end];
            let rest = &stripped[end + 5..];
            let meta: serde_yaml::Value = serde_yaml::from_str(yaml_str).unwrap_or_default();
            (meta, rest.to_string())
        } else {
            (serde_yaml::Value::default(), md.to_string())
        }
    } else {
        (serde_yaml::Value::default(), md.to_string())
    };

    let get_str = |key: &str| -> String {
        meta.get(key)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };
    let get_i64 = |key: &str| -> i64 { meta.get(key).and_then(|v| v.as_i64()).unwrap_or(0) };
    let get_i32 = |key: &str| -> i32 { meta.get(key).and_then(|v| v.as_i64()).unwrap_or(0) as i32 };
    let get_bool = |key: &str| -> bool { meta.get(key).and_then(|v| v.as_bool()).unwrap_or(false) };
    let get_vec = |key: &str| -> Vec<String> {
        meta.get(key)
            .and_then(|v| v.as_sequence())
            .map(|seq| {
                seq.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    };

    let title = {
        let t = get_str("title");
        if t.is_empty() {
            body.lines()
                .find(|l| l.starts_with("# "))
                .map(|l| l[2..].trim().to_string())
                .unwrap_or_else(|| id.to_string())
        } else {
            t
        }
    };

    let word_count = body.split_whitespace().count() as i32;

    TranscriptRecord {
        id: id.to_string(),
        source: source.to_string(),
        title,
        status: get_str("status"),
        model_provider: get_str("model_provider"),
        model_name: get_str("model_name"),
        turns_user: get_i32("turns_user"),
        turns_agent: get_i32("turns_agent"),
        turns_total: get_i32("turns_total"),
        tokens_in: get_i64("tokens_in"),
        tokens_out: get_i64("tokens_out"),
        tokens_cache_read: get_i64("tokens_cache_read"),
        tokens_cache_write: get_i64("tokens_cache_write"),
        word_count,
        thinking_enabled: get_bool("thinking_enabled"),
        tags: get_vec("tags"),
        tools_used: get_vec("tools_used"),
        folder_paths: get_vec("folder_paths"),
        branch: meta
            .get("branch")
            .and_then(|v| v.as_str())
            .map(String::from),
        thread_version: meta
            .get("thread_version")
            .and_then(|v| v.as_str())
            .map(String::from),
        body_text: body,
        created_at: get_str("created_at"),
        updated_at: get_str("updated_at"),
    }
}
