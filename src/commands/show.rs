use anyhow::{Context, Result};
use rusqlite::Connection;
use transcriptd_store::get_transcript;

use crate::format::{format_tokens, record_to_json};

pub fn cmd_show(conn: &Connection, id: &str, format: &str) -> Result<()> {
    let rec = get_transcript(conn, id)?.with_context(|| format!("transcript {id} not found"))?;
    match format {
        "json" => {
            println!("{}", record_to_json(&rec));
        }
        "markdown" => {
            println!("# {}", rec.title);
            println!();
            println!("- **ID:** {}", rec.id);
            println!("- **Source:** {}", rec.source);
            println!("- **Model:** {}/{}", rec.model_provider, rec.model_name);
            println!("- **Created:** {}", rec.created_at);
            println!("- **Updated:** {}", rec.updated_at);
            println!(
                "- **Turns:** {} (user: {}, agent: {})",
                rec.turns_total, rec.turns_user, rec.turns_agent
            );
            println!(
                "- **Tokens:** {} in / {} out",
                format_tokens(rec.tokens_in),
                format_tokens(rec.tokens_out)
            );
            if !rec.tools_used.is_empty() {
                println!("- **Tools:** {}", rec.tools_used.join(", "));
            }
            if !rec.folder_paths.is_empty() {
                println!("- **Workspace:** {}", rec.folder_paths.join(", "));
            }
            println!();
            println!("---");
            println!();
            println!("{}", rec.body_text);
        }
        _ => {
            println!("ID:       {}", rec.id);
            println!("Source:   {}", rec.source);
            println!("Title:    {}", rec.title);
            println!("Created:  {}", rec.created_at);
            println!("Updated:  {}", rec.updated_at);
            println!("Model:    {}/{}", rec.model_provider, rec.model_name);
            println!(
                "Turns:    {} (user:{} agent:{})",
                rec.turns_total, rec.turns_user, rec.turns_agent
            );
            println!(
                "Tokens:   in:{} out:{} cache_r:{} cache_w:{}",
                format_tokens(rec.tokens_in),
                format_tokens(rec.tokens_out),
                format_tokens(rec.tokens_cache_read),
                format_tokens(rec.tokens_cache_write),
            );
            if !rec.tools_used.is_empty() {
                println!("Tools:    {}", rec.tools_used.join(", "));
            }
            if !rec.folder_paths.is_empty() {
                println!("Folders:  {}", rec.folder_paths.join(", "));
            }
            if let Some(b) = &rec.branch {
                println!("Branch:   {b}");
            }
            println!("---");
            println!("{}", rec.body_text);
        }
    }
    Ok(())
}
