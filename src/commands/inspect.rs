use anyhow::{Context, Result};
use rusqlite::Connection;
use transcriptd_store::get_transcript;

use crate::format::record_to_json;

pub fn cmd_inspect(conn: &Connection, id: &str) -> Result<()> {
    let rec = get_transcript(conn, id)?.with_context(|| format!("transcript {id} not found"))?;
    println!("{}", record_to_json(&rec));
    Ok(())
}
