use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Json};
use axum::routing::{get, post};
use futures::stream::Stream;
use notify_debouncer_mini::{DebouncedEventKind, new_debouncer};
use tokio::io::AsyncWriteExt;
use tokio::net::UnixListener;
use tokio::sync::broadcast;
use tower_http::cors::CorsLayer;
use tracing::{error, info, warn};

use crate::config::Config;
use crate::ipc::{self, DailyEntry, RecentEntry, ServerMsg};
use transcriptd_core::TranscriptExtractor;

// ---------------------------------------------------------------------------
// Shared state (used by service daemon for SSE)
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct ServiceState {
    db_path: PathBuf,
    api_key: String,
    tx: broadcast::Sender<ServerMsg>,
}

// ---------------------------------------------------------------------------
// MCP HTTP state (lightweight, no broadcast channel)
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct McpState {
    db_path: PathBuf,
    api_key: String,
}

// ===========================================================================
// Service daemon: file watcher + IPC + SSE
// ===========================================================================

pub async fn run_service(db_path: &Path, cfg: &Config) -> Result<()> {
    let db_path = db_path.to_path_buf();
    let socket_path = cfg.socket_path.clone();

    // Ensure socket parent dir exists with 0o700
    if let Some(parent) = Path::new(&socket_path).parent() {
        std::fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }

    // Clean up stale socket
    let _ = std::fs::remove_file(&socket_path);

    // Broadcast channel for events (IPC clients + SSE subscribers)
    let (tx, _) = broadcast::channel::<ServerMsg>(256);

    let state = ServiceState {
        db_path: db_path.clone(),
        api_key: cfg.api_key.clone(),
        tx: tx.clone(),
    };

    // -- Task 1: SSE + health HTTP (for web dashboard / monitoring) --
    let app = Router::new()
        .route("/health", get(service_health_handler))
        .route("/sse", get(sse_handler))
        .layer(CorsLayer::permissive())
        .with_state(state.clone());

    // Service listens on mcp_port + 1 to avoid conflict with MCP HTTP
    let svc_port = cfg.mcp_port + 1;
    let http_addr = format!("127.0.0.1:{svc_port}");
    let listener = tokio::net::TcpListener::bind(&http_addr).await?;
    info!("service http listening on http://{http_addr}");

    let http_handle = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            error!("http server error: {e}");
        }
    });

    // -- Task 2: Unix socket IPC for dashboard --
    let ipc_tx = tx.clone();
    let ipc_db = db_path.clone();
    let ipc_sock = socket_path.clone();
    let ipc_handle = tokio::spawn(async move {
        if let Err(e) = run_ipc_listener(&ipc_sock, ipc_tx, &ipc_db).await {
            error!("ipc listener error: {e}");
        }
    });

    // -- Task 3: File watcher (blocking, in spawn_blocking) --
    let watcher_tx = tx.clone();
    let watcher_db = db_path.clone();
    let watcher_handle = tokio::task::spawn_blocking(move || {
        if let Err(e) = run_watcher(&watcher_db, &watcher_tx) {
            error!("watcher error: {e}");
        }
    });

    info!("service daemon started -- press Ctrl+C to stop");

    // Wait for shutdown signal
    tokio::signal::ctrl_c().await?;
    info!("shutting down service...");

    // Cleanup
    http_handle.abort();
    ipc_handle.abort();
    watcher_handle.abort();
    let _ = std::fs::remove_file(&socket_path);

    Ok(())
}

// ===========================================================================
// MCP HTTP daemon: just /mcp + /health (stateless, reads DB directly)
// ===========================================================================

pub async fn run_mcp_http(db_path: &Path, cfg: &Config) -> Result<()> {
    let state = McpState {
        db_path: db_path.to_path_buf(),
        api_key: cfg.api_key.clone(),
    };

    let app = Router::new()
        .route("/health", get(mcp_health_handler))
        .route("/mcp", post(mcp_handler))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let http_addr = format!("127.0.0.1:{}", cfg.mcp_port);
    let listener = tokio::net::TcpListener::bind(&http_addr).await?;
    info!("mcp http listening on http://{http_addr}");

    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            error!("mcp http server error: {e}");
        }
    });

    info!("mcp daemon started -- press Ctrl+C to stop");

    tokio::signal::ctrl_c().await?;
    info!("shutting down mcp...");

    Ok(())
}

// ---------------------------------------------------------------------------
// HTTP handlers: service daemon
// ---------------------------------------------------------------------------

async fn service_health_handler() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "role": "service",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

/// SSE state machine: drain snapshot first, then stream live broadcast events.
enum SsePhase {
    Snapshot(Vec<ServerMsg>),
    Live,
}

async fn sse_handler(
    State(state): State<ServiceState>,
    headers: HeaderMap,
) -> Result<Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>>, StatusCode> {
    // Auth check
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let expected = format!("Bearer {}", state.api_key);
    if auth != expected {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let rx = state.tx.subscribe();

    // Build snapshot (reversed so we can pop from back in order)
    let mut snapshot = build_snapshot(&state.db_path);
    snapshot.reverse();

    let stream = futures::stream::unfold(
        (SsePhase::Snapshot(snapshot), rx),
        |(phase, mut rx)| async move {
            match phase {
                SsePhase::Snapshot(mut snap) => {
                    if let Some(msg) = snap.pop() {
                        if let Ok(json) = serde_json::to_string(&msg) {
                            return Some((
                                Ok(Event::default().data(json)),
                                (SsePhase::Snapshot(snap), rx),
                            ));
                        }
                        return Some((
                            Ok(Event::default().comment("skip")),
                            (SsePhase::Snapshot(snap), rx),
                        ));
                    }
                    Some((
                        Ok(Event::default().comment("snapshot complete")),
                        (SsePhase::Live, rx),
                    ))
                }
                SsePhase::Live => loop {
                    match rx.recv().await {
                        Ok(msg) => {
                            if let Ok(json) = serde_json::to_string(&msg) {
                                return Some((
                                    Ok(Event::default().data(json)),
                                    (SsePhase::Live, rx),
                                ));
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!("sse client lagged, skipped {n} events");
                            continue;
                        }
                        Err(broadcast::error::RecvError::Closed) => return None,
                    }
                },
            }
        },
    );

    Ok(Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("ping"),
    ))
}

// ---------------------------------------------------------------------------
// HTTP handlers: MCP daemon
// ---------------------------------------------------------------------------

async fn mcp_health_handler() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "role": "mcp",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

async fn mcp_handler(
    State(state): State<McpState>,
    headers: HeaderMap,
    body: String,
) -> impl IntoResponse {
    // Auth check
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let expected = format!("Bearer {}", state.api_key);
    if auth != expected {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "unauthorized"})),
        );
    }

    // Parse JSON-RPC
    let request: serde_json::Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": {"code": -32700, "message": format!("Parse error: {e}")}
                })),
            );
        }
    };

    let response = crate::mcp::handle_request(&state.db_path, &request);
    (StatusCode::OK, Json(response))
}

// ---------------------------------------------------------------------------
// IPC Unix socket listener
// ---------------------------------------------------------------------------

async fn run_ipc_listener(
    socket_path: &str,
    tx: broadcast::Sender<ServerMsg>,
    db_path: &Path,
) -> Result<()> {
    let listener =
        UnixListener::bind(socket_path).with_context(|| format!("bind {socket_path}"))?;
    info!("ipc listening on {socket_path}");

    loop {
        let (stream, _) = listener.accept().await?;
        let mut rx = tx.subscribe();
        let db_path = db_path.to_path_buf();

        tokio::spawn(async move {
            let (_, mut writer) = stream.into_split();

            // Send snapshot
            let snapshot = build_snapshot(&db_path);
            for msg in snapshot {
                let encoded = ipc::encode(&msg);
                if writer.write_all(encoded.as_bytes()).await.is_err() {
                    return;
                }
            }

            info!("ipc client connected");

            // Stream live events
            loop {
                match rx.recv().await {
                    Ok(msg) => {
                        let encoded = ipc::encode(&msg);
                        if writer.write_all(encoded.as_bytes()).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }
}

// ---------------------------------------------------------------------------
// Snapshot builder (shared by IPC + SSE)
// ---------------------------------------------------------------------------

fn build_snapshot(db_path: &Path) -> Vec<ServerMsg> {
    let mut msgs = Vec::new();
    let conn = match transcriptd_store::init_db(db_path) {
        Ok(c) => c,
        Err(_) => return msgs,
    };

    if let Ok(stats) = transcriptd_store::get_stats(&conn) {
        msgs.push(ServerMsg::Stats {
            total_transcripts: stats.total_transcripts,
            total_turns: stats.total_turns,
            total_tokens_in: stats.total_tokens_in,
            total_tokens_out: stats.total_tokens_out,
            sources: stats.sources,
            top_tools: stats.top_tools,
        });
    }

    if let Ok(recent) = transcriptd_store::recent_transcripts(&conn, 50) {
        let entries: Vec<RecentEntry> = recent
            .iter()
            .map(|r| RecentEntry {
                id: r.id.clone(),
                source: r.source.clone(),
                title: r.title.clone(),
                turns_total: r.turns_total,
                tokens_in: r.tokens_in,
                tokens_out: r.tokens_out,
                created_at: r.created_at.clone(),
            })
            .collect();
        msgs.push(ServerMsg::Recent {
            transcripts: entries,
        });
    }

    if let Ok(daily) = transcriptd_store::daily_token_counts(&conn, 14) {
        let sessions = transcriptd_store::daily_session_counts(&conn, 14).unwrap_or_default();
        let session_map: std::collections::HashMap<String, i64> = sessions.into_iter().collect();
        let entries: Vec<DailyEntry> = daily
            .iter()
            .map(|(date, tin, tout)| DailyEntry {
                date: date.clone(),
                tokens_in: *tin,
                tokens_out: *tout,
                sessions: *session_map.get(date).unwrap_or(&0),
            })
            .collect();
        msgs.push(ServerMsg::Daily { entries });
    }

    msgs
}

// ---------------------------------------------------------------------------
// File watcher (sync -- runs in spawn_blocking)
// ---------------------------------------------------------------------------

fn run_watcher(db_path: &Path, event_tx: &broadcast::Sender<ServerMsg>) -> Result<()> {
    let extractors: Vec<Box<dyn TranscriptExtractor>> = vec![
        Box::new(transcriptd_zed::ZedExtractor),
        Box::new(transcriptd_claude::ClaudeExtractor),
        Box::new(transcriptd_vscode::VscodeExtractor),
    ];

    let (tx, rx) = std::sync::mpsc::channel();
    let mut debouncer = new_debouncer(Duration::from_secs(5), tx)?;

    let mut watch_map: Vec<(usize, PathBuf, Vec<PathBuf>)> = Vec::new();

    for (i, ext) in extractors.iter().enumerate() {
        let src_path = match ext.default_source_path() {
            Some(p) if p.exists() => p,
            _ => continue,
        };
        let wpaths = ext.watch_paths(&src_path);
        for wp in &wpaths {
            if wp.exists() {
                let _ = debouncer
                    .watcher()
                    .watch(wp, notify::RecursiveMode::Recursive);
                info!("watching {}", wp.display());
            }
        }
        watch_map.push((i, src_path, wpaths));
    }

    // Initial ingest
    {
        let conn = transcriptd_store::init_db(db_path)?;
        for (idx, src_path, _) in &watch_map {
            let ext = &extractors[*idx];
            info!("initial ingest: {}", ext.name());
            if let Ok(pairs) = ext.extract_all(src_path, None) {
                for (id, md) in &pairs {
                    let rec = crate::parse::parse_md_to_record(id, ext.name(), md);
                    let _ = transcriptd_store::upsert_transcript(&conn, &rec);
                }
                let timestamp = chrono::Local::now().to_rfc3339();
                let _ = event_tx.send(ServerMsg::Event {
                    kind: "ingested".to_string(),
                    source: ext.name().to_string(),
                    detail: format!("{} transcripts", pairs.len()),
                    timestamp,
                });
            }
        }
        // Send initial stats
        if let Ok(stats) = transcriptd_store::get_stats(&conn) {
            let _ = event_tx.send(ServerMsg::Stats {
                total_transcripts: stats.total_transcripts,
                total_turns: stats.total_turns,
                total_tokens_in: stats.total_tokens_in,
                total_tokens_out: stats.total_tokens_out,
                sources: stats.sources,
                top_tools: stats.top_tools,
            });
        }
    }

    info!("watching for changes...");

    loop {
        match rx.recv() {
            Ok(Ok(events)) => {
                let changed: Vec<PathBuf> = events
                    .iter()
                    .filter(|e| e.kind == DebouncedEventKind::Any)
                    .map(|e| e.path.clone())
                    .collect();
                if changed.is_empty() {
                    continue;
                }

                let conn = match transcriptd_store::init_db(db_path) {
                    Ok(c) => c,
                    Err(e) => {
                        error!("db error: {e}");
                        continue;
                    }
                };

                for (idx, src_path, wpaths) in &watch_map {
                    let relevant = changed
                        .iter()
                        .any(|cp| wpaths.iter().any(|wp| cp.starts_with(wp)));
                    if !relevant {
                        continue;
                    }

                    let ext = &extractors[*idx];
                    info!("re-ingesting {}", ext.name());
                    match ext.extract_all(src_path, None) {
                        Ok(pairs) => {
                            for (id, md) in &pairs {
                                let rec = crate::parse::parse_md_to_record(id, ext.name(), md);
                                let _ = transcriptd_store::upsert_transcript(&conn, &rec);
                            }
                            let timestamp = chrono::Local::now().to_rfc3339();
                            let _ = event_tx.send(ServerMsg::Event {
                                kind: "ingested".to_string(),
                                source: ext.name().to_string(),
                                detail: format!("{} transcripts", pairs.len()),
                                timestamp,
                            });
                        }
                        Err(e) => error!("{}: {e}", ext.name()),
                    }
                }

                // Updated stats
                if let Ok(stats) = transcriptd_store::get_stats(&conn) {
                    let _ = event_tx.send(ServerMsg::Stats {
                        total_transcripts: stats.total_transcripts,
                        total_turns: stats.total_turns,
                        total_tokens_in: stats.total_tokens_in,
                        total_tokens_out: stats.total_tokens_out,
                        sources: stats.sources,
                        top_tools: stats.top_tools,
                    });
                }
            }
            Ok(Err(e)) => warn!("notify error: {e}"),
            Err(e) => {
                error!("channel closed: {e}");
                break;
            }
        }
    }

    Ok(())
}
