use anyhow::Result;
use comfy_table::{ContentArrangement, Table, presets};
use rusqlite::Connection;
use transcriptd_store::list_transcripts;

use crate::format::{format_tokens, records_to_json};

pub fn cmd_list(
    conn: &Connection,
    source: Option<&str>,
    limit: usize,
    offset: usize,
    sort: &str,
    format: &str,
) -> Result<()> {
    let results = list_transcripts(conn, source, limit, offset, sort)?;
    if format == "json" {
        println!("{}", records_to_json(&results));
    } else {
        if results.is_empty() {
            println!("No transcripts.");
            return Ok(());
        }
        // Build rowid lookup for short IDs
        let ids: Vec<&str> = results.iter().map(|r| r.id.as_str()).collect();
        let rowid_map = crate::format::get_rowids(conn, &ids);

        let mut table = Table::new();
        table
            .load_preset(presets::UTF8_FULL_CONDENSED)
            .set_content_arrangement(ContentArrangement::Dynamic)
            .set_header(vec!["#", "Source", "Title", "Turns", "Tokens", "Date"]);
        for rec in &results {
            let short = rowid_map
                .get(rec.id.as_str())
                .map(|r| format!("#{r}"))
                .unwrap_or_else(|| rec.id.clone());
            table.add_row(vec![
                short,
                rec.source.clone(),
                truncate_title(&rec.title, 50),
                rec.turns_total.to_string(),
                format!(
                    "{}/{}",
                    format_tokens(rec.tokens_in),
                    format_tokens(rec.tokens_out)
                ),
                rec.created_at[..10.min(rec.created_at.len())].to_string(),
            ]);
        }
        println!("{table}");
        println!("({} listed)", results.len());
    }
    Ok(())
}

fn truncate_title(title: &str, max: usize) -> String {
    if title.len() <= max {
        title.to_string()
    } else {
        format!("{}…", &title[..max - 1])
    }
}
