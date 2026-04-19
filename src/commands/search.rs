use anyhow::Result;
use comfy_table::{ContentArrangement, Table, presets};
use rusqlite::Connection;
use transcriptd_store::search;

use crate::format::{format_tokens, records_to_json};

pub fn cmd_search(
    conn: &Connection,
    query: &str,
    limit: usize,
    offset: usize,
    format: &str,
) -> Result<()> {
    let results = search(conn, query, limit, offset)?;
    if format == "json" {
        println!("{}", records_to_json(&results));
    } else {
        if results.is_empty() {
            println!("No results.");
            return Ok(());
        }
        let mut table = Table::new();
        table
            .load_preset(presets::UTF8_FULL_CONDENSED)
            .set_content_arrangement(ContentArrangement::Dynamic)
            .set_header(vec!["ID", "Source", "Title", "Turns", "Tokens", "Date"]);
        for rec in &results {
            table.add_row(vec![
                rec.id.clone(),
                rec.source.clone(),
                rec.title.clone(),
                rec.turns_total.to_string(),
                format!(
                    "{}/{}",
                    format_tokens(rec.tokens_in),
                    format_tokens(rec.tokens_out)
                ),
                rec.created_at.clone(),
            ]);
        }
        println!("{table}");
        println!("({} results)", results.len());
    }
    Ok(())
}
