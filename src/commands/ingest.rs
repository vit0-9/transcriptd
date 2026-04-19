use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Result;
use rusqlite::Connection;
use transcriptd_core::TranscriptExtractor;
use transcriptd_store::upsert_transcript;

use crate::extractors::all_extractors;
use crate::parse::parse_md_to_record;

pub fn cmd_ingest(
    conn: &Connection,
    source_filter: &str,
    overrides: &HashMap<String, String>,
    since: Option<&str>,
) -> Result<()> {
    let extractors = all_extractors();
    let mut total = 0usize;

    for ext in &extractors {
        if source_filter != "all" && ext.name() != source_filter {
            continue;
        }
        let src_path = match resolve_source_path(
            ext.as_ref(),
            overrides.get(ext.name()).map(|s| s.as_str()),
        ) {
            Some(p) => p,
            None => {
                eprintln!("[skip] {} \u{2013} source path not found", ext.name());
                continue;
            }
        };

        eprintln!("[ingest] {} from {}", ext.name(), src_path.display());
        let pairs = ext.extract_all(&src_path, since)?;
        let count = pairs.len();
        for (id, md) in &pairs {
            let rec = parse_md_to_record(id, ext.name(), md);
            upsert_transcript(conn, &rec)?;
        }
        eprintln!("[ingest] {} \u{2013} {} transcripts", ext.name(), count);
        total += count;
    }

    eprintln!("[ingest] total: {total} transcripts");

    // Self-healing: deduplicate any legacy-format rows (ADR-001)
    eprintln!("[ingest] running auto-dedupe...");
    transcriptd_store::dedupe_transcripts(conn, false)?;

    Ok(())
}

#[allow(dead_code)]
pub fn cmd_import(conn: &Connection, dir: &str, source: &str) -> Result<()> {
    let count = ingest_markdowns(conn, dir, source)?;
    eprintln!("[import] {count} files from {dir}");
    Ok(())
}

fn resolve_source_path(
    ext: &dyn TranscriptExtractor,
    cli_override: Option<&str>,
) -> Option<PathBuf> {
    if let Some(p) = cli_override {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return Some(pb);
        }
        eprintln!("[warn] override path does not exist: {p}");
        return None;
    }
    ext.default_source_path().filter(|p| p.exists())
}

#[allow(dead_code)]
fn ingest_markdowns(conn: &Connection, dir: &str, source: &str) -> Result<usize> {
    let mut count = 0;
    for entry in walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "md") {
            let content = std::fs::read_to_string(path)?;
            let id = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let rec = parse_md_to_record(&id, source, &content);
            upsert_transcript(conn, &rec)?;
            count += 1;
        }
    }
    Ok(count)
}
