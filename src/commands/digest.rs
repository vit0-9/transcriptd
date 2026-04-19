use std::collections::HashMap;

use anyhow::{Context, Result};
use chrono::{Local, NaiveDate};
use rusqlite::Connection;
use transcriptd_store::TranscriptRecord;

use crate::format::format_tokens;

pub fn cmd_digest(conn: &Connection, period: &str, format: &str) -> Result<()> {
    let today = Local::now().date_naive();
    let (start, end) = match period {
        "today" => (today, today),
        "yesterday" => {
            let d = today - chrono::Duration::days(1);
            (d, d)
        }
        "week" => (today - chrono::Duration::days(7), today),
        "month" => (today - chrono::Duration::days(30), today),
        other => {
            let d = NaiveDate::parse_from_str(other, "%Y-%m-%d")
                .with_context(|| format!("invalid period: {other}"))?;
            (d, d)
        }
    };

    let start_str = start.format("%Y-%m-%d").to_string();
    let end_str = (end + chrono::Duration::days(1))
        .format("%Y-%m-%d")
        .to_string();

    // Query transcripts in range
    let mut stmt = conn.prepare(
        "SELECT * FROM transcripts \
         WHERE created_at >= :start AND created_at < :end \
         ORDER BY created_at DESC",
    )?;
    let rows = stmt.query_map(
        rusqlite::named_params! { ":start": start_str, ":end": end_str },
        |row| {
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
                tags: serde_json::from_str(
                    &row.get::<_, String>("tags").unwrap_or_else(|_| "[]".into()),
                )
                .unwrap_or_default(),
                tools_used: serde_json::from_str(
                    &row.get::<_, String>("tools_used")
                        .unwrap_or_else(|_| "[]".into()),
                )
                .unwrap_or_default(),
                folder_paths: serde_json::from_str(
                    &row.get::<_, String>("folder_paths")
                        .unwrap_or_else(|_| "[]".into()),
                )
                .unwrap_or_default(),
                branch: row.get("branch")?,
                thread_version: row.get("thread_version")?,
                body_text: row
                    .get::<_, Option<String>>("body_text")?
                    .unwrap_or_default(),
                created_at: row.get("created_at")?,
                updated_at: row.get("updated_at")?,
            })
        },
    )?;

    let records: Vec<TranscriptRecord> = rows.filter_map(|r| r.ok()).collect();

    // Aggregate by source
    let mut by_source: HashMap<String, Vec<&TranscriptRecord>> = HashMap::new();
    for rec in &records {
        by_source.entry(rec.source.clone()).or_default().push(rec);
    }

    // Top tools
    let mut tool_counts: HashMap<String, usize> = HashMap::new();
    for rec in &records {
        for tool in &rec.tools_used {
            *tool_counts.entry(tool.clone()).or_default() += 1;
        }
    }
    let mut top_tools: Vec<(String, usize)> = tool_counts.into_iter().collect();
    top_tools.sort_by(|a, b| b.1.cmp(&a.1));
    top_tools.truncate(10);

    let total_tokens_in: i64 = records.iter().map(|r| r.tokens_in).sum();
    let total_tokens_out: i64 = records.iter().map(|r| r.tokens_out).sum();

    if format == "json" {
        let j = serde_json::json!({
            "period": period,
            "start": start_str,
            "end": end_str,
            "total_transcripts": records.len(),
            "total_tokens_in": total_tokens_in,
            "total_tokens_out": total_tokens_out,
            "by_source": by_source.iter().map(|(src, recs)| {
                serde_json::json!({
                    "source": src,
                    "count": recs.len(),
                    "tokens_in": recs.iter().map(|r| r.tokens_in).sum::<i64>(),
                    "tokens_out": recs.iter().map(|r| r.tokens_out).sum::<i64>(),
                })
            }).collect::<Vec<_>>(),
            "top_tools": top_tools,
        });
        println!("{}", serde_json::to_string_pretty(&j)?);
    } else {
        // Markdown output
        println!("# Digest: {period}");
        println!();
        println!("Period: {} to {}", start, end);
        println!("Total transcripts: {}", records.len());
        println!(
            "Total tokens: {} in / {} out",
            format_tokens(total_tokens_in),
            format_tokens(total_tokens_out)
        );
        println!();
        println!("## By Source");
        println!();
        println!("| Source | Count | Tokens In | Tokens Out |");
        println!("|--------|------:|----------:|-----------:|");
        for (src, recs) in &by_source {
            let ti: i64 = recs.iter().map(|r| r.tokens_in).sum();
            let to: i64 = recs.iter().map(|r| r.tokens_out).sum();
            println!(
                "| {} | {} | {} | {} |",
                src,
                recs.len(),
                format_tokens(ti),
                format_tokens(to)
            );
        }
        println!();
        println!("## Top Tools");
        println!();
        println!("| Tool | Uses |");
        println!("|------|-----:|");
        for (tool, cnt) in &top_tools {
            println!("| {} | {} |", tool, cnt);
        }
        if !records.is_empty() {
            println!();
            println!("## Transcripts");
            println!();
            for rec in &records {
                println!(
                    "- **{}** [{}] {} ({} turns, {}+{} tok)",
                    &rec.id[..8.min(rec.id.len())],
                    rec.source,
                    rec.title,
                    rec.turns_total,
                    format_tokens(rec.tokens_in),
                    format_tokens(rec.tokens_out),
                );
            }
        }
    }

    Ok(())
}
