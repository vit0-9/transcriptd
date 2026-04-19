use anyhow::Result;
use rusqlite::Connection;
use transcriptd_store::get_stats;

use crate::format::format_tokens;

pub fn cmd_stats(conn: &Connection, format: &str) -> Result<()> {
    let stats = get_stats(conn)?;
    if format == "json" {
        let j = serde_json::json!({
            "total_transcripts": stats.total_transcripts,
            "total_turns": stats.total_turns,
            "total_tokens_in": stats.total_tokens_in,
            "total_tokens_out": stats.total_tokens_out,
            "sources": stats.sources,
            "top_tools": stats.top_tools,
        });
        println!("{}", serde_json::to_string_pretty(&j)?);
    } else {
        println!("Transcripts: {}", stats.total_transcripts);
        println!("Turns:       {}", stats.total_turns);
        println!("Tokens in:   {}", format_tokens(stats.total_tokens_in));
        println!("Tokens out:  {}", format_tokens(stats.total_tokens_out));
        println!();
        println!("Sources:");
        for (src, cnt) in &stats.sources {
            println!("  {src}: {cnt}");
        }
        println!();
        println!("Top tools:");
        for (tool, cnt) in &stats.top_tools {
            println!("  {tool}: {cnt}");
        }
    }
    Ok(())
}
