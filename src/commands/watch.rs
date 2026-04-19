use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{Result, bail};
use notify_debouncer_mini::{DebouncedEventKind, new_debouncer};
use transcriptd_store::{init_db, upsert_transcript};

use crate::commands::ingest::cmd_ingest;
use crate::extractors::all_extractors;
use crate::parse::parse_md_to_record;

pub fn cmd_watch(db_path: &Path, source_filter: &str, debounce_secs: u64) -> Result<()> {
    let extractors = all_extractors();
    let (tx, rx) = mpsc::channel();

    let mut debouncer = new_debouncer(Duration::from_secs(debounce_secs), tx)?;

    // Build (extractor_index, resolved_path, watch_paths) map
    let mut watch_map: Vec<(usize, PathBuf, Vec<PathBuf>)> = Vec::new();

    for (i, ext) in extractors.iter().enumerate() {
        if source_filter != "all" && ext.name() != source_filter {
            continue;
        }
        let src_path = match ext.default_source_path() {
            Some(p) if p.exists() => p,
            _ => {
                eprintln!("[watch] {} \u{2013} source not found, skipping", ext.name());
                continue;
            }
        };
        let wpaths = ext.watch_paths(&src_path);
        for wp in &wpaths {
            if wp.exists() {
                debouncer
                    .watcher()
                    .watch(wp, notify::RecursiveMode::Recursive)?;
                eprintln!("[watch] watching {}", wp.display());
            }
        }
        watch_map.push((i, src_path, wpaths));
    }

    if watch_map.is_empty() {
        bail!("no sources found to watch");
    }

    // Initial ingest
    {
        let conn = init_db(db_path)?;
        let overrides = HashMap::new();
        cmd_ingest(&conn, source_filter, &overrides, None)?;
    }

    eprintln!("[watch] listening for changes (debounce={debounce_secs}s)...");

    loop {
        match rx.recv() {
            Ok(Ok(events)) => {
                let changed_paths: Vec<PathBuf> = events
                    .iter()
                    .filter(|e| e.kind == DebouncedEventKind::Any)
                    .map(|e| e.path.clone())
                    .collect();
                if changed_paths.is_empty() {
                    continue;
                }
                let conn = init_db(db_path)?;
                for (idx, src_path, wpaths) in &watch_map {
                    let relevant = changed_paths
                        .iter()
                        .any(|cp| wpaths.iter().any(|wp| cp.starts_with(wp)));
                    if !relevant {
                        continue;
                    }
                    let ext = &extractors[*idx];
                    eprintln!(
                        "[watch] change detected for {}, re-ingesting\u{2026}",
                        ext.name()
                    );
                    match ext.extract_all(src_path, None) {
                        Ok(pairs) => {
                            for (id, md) in &pairs {
                                let rec = parse_md_to_record(id, ext.name(), md);
                                if let Err(e) = upsert_transcript(&conn, &rec) {
                                    eprintln!("[watch] upsert error: {e}");
                                }
                            }
                            eprintln!(
                                "[watch] {} \u{2013} {} transcripts",
                                ext.name(),
                                pairs.len()
                            );
                        }
                        Err(e) => {
                            eprintln!("[watch] extract error for {}: {e}", ext.name());
                        }
                    }
                }
            }
            Ok(Err(e)) => {
                eprintln!("[watch] notify error: {e}");
            }
            Err(e) => {
                eprintln!("[watch] channel closed: {e}");
                break;
            }
        }
    }

    Ok(())
}
