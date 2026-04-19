use std::collections::VecDeque;
use std::io::{self, BufRead, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::prelude::*;
use ratatui::widgets::*;

use transcriptd_store::{self, StoreStats, TranscriptRecord};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn fmt_tokens(n: i64) -> String {
    if n >= 1_000_000_000 {
        format!("{:.1}B", n as f64 / 1_000_000_000.0)
    } else if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{t}…")
    }
}

fn source_color(source: &str) -> Color {
    match source {
        "zed" => Color::Blue,
        "claude-code" => Color::Rgb(204, 120, 50),
        "vscode-copilot" => Color::Cyan,
        "codex" => Color::Green,
        "cursor" => Color::Magenta,
        _ => Color::White,
    }
}

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

struct App {
    // Aggregate stats
    stats: StoreStats,
    // Today stats
    today_sessions: i64,
    today_tokens_in: i64,
    today_tokens_out: i64,
    today_errors: i64,
    // Recent sessions
    recent: Vec<TranscriptRecord>,
    scroll_offset: usize,
    // Charts
    daily_tokens: Vec<(String, i64, i64)>,
    hourly_tokens: Vec<(i32, i64, i64)>,
    // Errors
    recent_errors: Vec<(String, String, String, String)>, // tool, summary, time, transcript_id
    // Service status
    service_running: bool,
    service_pid: Option<i32>,
    mcp_running: bool,
    mcp_pid: Option<i32>,
    // Burn rate tracking
    prev_tokens_in: i64,
    prev_time: Instant,
    burn_rate: f64,
    // Log tail
    log_lines: VecDeque<String>,
    log_pos: u64,
    // UI
    started_at: Instant,
}

impl App {
    fn new() -> Self {
        Self {
            stats: StoreStats {
                total_transcripts: 0,
                total_turns: 0,
                total_tokens_in: 0,
                total_tokens_out: 0,
                sources: vec![],
                top_tools: vec![],
            },
            today_sessions: 0,
            today_tokens_in: 0,
            today_tokens_out: 0,
            today_errors: 0,
            recent: vec![],
            scroll_offset: 0,
            daily_tokens: vec![],
            hourly_tokens: vec![],
            recent_errors: vec![],
            service_running: false,
            service_pid: None,
            mcp_running: false,
            mcp_pid: None,
            prev_tokens_in: 0,
            prev_time: Instant::now(),
            burn_rate: 0.0,
            log_lines: VecDeque::with_capacity(50),
            log_pos: 0,
            started_at: Instant::now(),
        }
    }

    fn refresh(&mut self, db_path: &Path) -> Result<()> {
        let conn = transcriptd_store::init_db(db_path)?;

        // Core stats
        self.stats = transcriptd_store::get_stats(&conn)?;
        if let Ok((ts, ti, to)) = transcriptd_store::today_stats(&conn) {
            self.today_sessions = ts;
            self.today_tokens_in = ti;
            self.today_tokens_out = to;
        }

        // Errors (defensive — table may not exist in older DBs)
        self.today_errors = transcriptd_store::today_error_count(&conn).unwrap_or(0);
        self.recent_errors = transcriptd_store::recent_tool_errors(&conn, 10).unwrap_or_default();

        // Recent sessions (lightweight — no body_text)
        self.recent = transcriptd_store::recent_transcripts_lite(&conn, 100)?;
        if !self.recent.is_empty() {
            self.scroll_offset = self.scroll_offset.min(self.recent.len().saturating_sub(1));
        } else {
            self.scroll_offset = 0;
        }

        // Charts
        self.daily_tokens = transcriptd_store::daily_token_counts(&conn, 14)?;
        self.hourly_tokens = transcriptd_store::hourly_tokens_today(&conn).unwrap_or_default();

        // Burn rate
        let elapsed = self.prev_time.elapsed().as_secs_f64() / 60.0;
        let delta = self.stats.total_tokens_in - self.prev_tokens_in;
        if elapsed > 0.1 && self.prev_tokens_in > 0 && delta > 0 {
            self.burn_rate = delta as f64 / elapsed;
        }
        self.prev_tokens_in = self.stats.total_tokens_in;
        self.prev_time = Instant::now();

        // Service status
        let (svc_run, svc_pid) = check_pid(&crate::config::pid_file_path());
        self.service_running = svc_run;
        self.service_pid = svc_pid;
        let (mcp_run, mcp_pid) = check_pid(&crate::config::mcp_pid_file_path());
        self.mcp_running = mcp_run;
        self.mcp_pid = mcp_pid;

        // Tail the log file
        self.tail_log();

        Ok(())
    }

    fn tail_log(&mut self) {
        let log_path = crate::config::log_file_path();
        let Ok(mut file) = std::fs::File::open(&log_path) else {
            return;
        };
        let meta = file.metadata().ok();
        let file_size = meta.map(|m| m.len()).unwrap_or(0);

        // If file was truncated, reset position
        if file_size < self.log_pos {
            self.log_pos = 0;
            self.log_lines.clear();
        }

        // If first time, jump to last 4KB
        if self.log_pos == 0 && file_size > 4096 {
            self.log_pos = file_size - 4096;
        }

        if file_size <= self.log_pos {
            return;
        }

        if file.seek(SeekFrom::Start(self.log_pos)).is_err() {
            return;
        }

        let reader = io::BufReader::new(&file);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            if line.is_empty() {
                continue;
            }
            self.log_lines.push_back(line);
            while self.log_lines.len() > 50 {
                self.log_lines.pop_front();
            }
        }

        self.log_pos = file_size;
    }

    fn scroll_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
    }

    fn scroll_down(&mut self) {
        if !self.recent.is_empty() {
            self.scroll_offset = (self.scroll_offset + 1).min(self.recent.len().saturating_sub(1));
        }
    }
}

fn check_pid(pid_path: &PathBuf) -> (bool, Option<i32>) {
    if !pid_path.exists() {
        return (false, None);
    }
    let Ok(contents) = std::fs::read_to_string(pid_path) else {
        return (false, None);
    };
    let Ok(pid) = contents.trim().parse::<i32>() else {
        return (false, None);
    };
    #[cfg(unix)]
    {
        let alive = unsafe { libc::kill(pid, 0) == 0 };
        (alive, Some(pid))
    }
    #[cfg(not(unix))]
    {
        (false, Some(pid))
    }
}

fn fmt_uptime(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub fn run(db_path: &Path) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_loop(&mut terminal, db_path);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn run_loop(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, db_path: &Path) -> Result<()> {
    let mut app = App::new();
    app.refresh(db_path)?;

    let mut tick = Instant::now();

    loop {
        terminal.draw(|f| draw(f, &app))?;

        if event::poll(Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                    KeyCode::Char('c')
                        if key
                            .modifiers
                            .contains(crossterm::event::KeyModifiers::CONTROL) =>
                    {
                        return Ok(());
                    }
                    KeyCode::Up | KeyCode::Char('k') => app.scroll_up(),
                    KeyCode::Down | KeyCode::Char('j') => app.scroll_down(),
                    KeyCode::Char('r') => app.refresh(db_path)?,
                    _ => {}
                }
            }
        }

        // Auto-refresh every 2 seconds
        if tick.elapsed() >= Duration::from_secs(2) {
            app.refresh(db_path)?;
            tick = Instant::now();
        }
    }
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

fn draw(f: &mut Frame, app: &App) {
    let area = f.area();

    // Title bar (1 line) + status row + sessions + charts + log
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // title bar
            Constraint::Length(7), // status row: service | today | errors
            Constraint::Min(8),    // recent sessions (takes remaining)
            Constraint::Length(6), // charts row: hourly | daily
            Constraint::Length(7), // log tail
        ])
        .split(area);

    draw_title_bar(f, rows[0], app);
    draw_status_row(f, rows[1], app);
    draw_recent(f, rows[2], &app.recent, app.scroll_offset);
    draw_charts_row(f, rows[3], app);
    draw_log(f, rows[4], app);
}

// -- Title bar ---------------------------------------------------------------

fn draw_title_bar(f: &mut Frame, area: Rect, app: &App) {
    let uptime = fmt_uptime(app.started_at.elapsed());
    let left = Span::styled(
        " transcriptd dashboard ",
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    );
    let right = Span::styled(
        format!(" ↑↓ scroll  r refresh  q quit  {uptime} "),
        Style::default().fg(Color::DarkGray),
    );
    let line = Line::from(vec![
        left,
        Span::raw(" "),
        Span::styled(
            format!("{} transcripts", app.stats.total_transcripts),
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        right,
    ]);
    f.render_widget(Paragraph::new(line), area);
}

// -- Status row: Service | Today | Errors ------------------------------------

fn draw_status_row(f: &mut Frame, area: Rect, app: &App) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(33),
            Constraint::Percentage(34),
            Constraint::Percentage(33),
        ])
        .split(area);

    draw_service_panel(f, cols[0], app);
    draw_today_panel(f, cols[1], app);
    draw_errors_panel(f, cols[2], app);
}

fn draw_service_panel(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::bordered().title(" Service ").title_style(
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    );
    let inner = block.inner(area);
    f.render_widget(block, area);

    let svc_line = if app.service_running {
        let pid = app
            .service_pid
            .map(|p| format!(" pid {p}"))
            .unwrap_or_default();
        Line::from(vec![
            Span::styled("● ", Style::default().fg(Color::Green)),
            Span::styled(
                "Watcher",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" running{pid}"),
                Style::default().fg(Color::DarkGray),
            ),
        ])
    } else {
        Line::from(vec![
            Span::styled("○ ", Style::default().fg(Color::DarkGray)),
            Span::styled("Watcher stopped", Style::default().fg(Color::DarkGray)),
        ])
    };

    let mcp_line = if app.mcp_running {
        let pid = app.mcp_pid.map(|p| format!(" pid {p}")).unwrap_or_default();
        Line::from(vec![
            Span::styled("● ", Style::default().fg(Color::Green)),
            Span::styled(
                "MCP HTTP",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" :3100{pid}"), Style::default().fg(Color::DarkGray)),
        ])
    } else {
        Line::from(vec![
            Span::styled("○ ", Style::default().fg(Color::DarkGray)),
            Span::styled("MCP HTTP stopped", Style::default().fg(Color::DarkGray)),
        ])
    };

    let db_line = Line::from(vec![
        Span::styled("  DB: ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!(
                "{} sessions, {} turns",
                app.stats.total_transcripts, app.stats.total_turns
            ),
            Style::default().fg(Color::White),
        ),
    ]);

    let tok_line = Line::from(vec![
        Span::styled("  Tokens: ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!(
                "{} in / {} out",
                fmt_tokens(app.stats.total_tokens_in),
                fmt_tokens(app.stats.total_tokens_out)
            ),
            Style::default().fg(Color::White),
        ),
    ]);

    f.render_widget(
        Paragraph::new(vec![svc_line, mcp_line, Line::from(""), db_line, tok_line]),
        inner,
    );
}

fn draw_today_panel(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::bordered().title(" Today ").title_style(
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    );
    let inner = block.inner(area);
    f.render_widget(block, area);

    let bold_val = |s: String| -> Span {
        Span::styled(
            s,
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )
    };

    let mut lines = vec![
        Line::from(vec![
            Span::raw("  Sessions   "),
            bold_val(app.today_sessions.to_string()),
        ]),
        Line::from(vec![
            Span::raw("  Tokens in  "),
            bold_val(fmt_tokens(app.today_tokens_in)),
        ]),
        Line::from(vec![
            Span::raw("  Tokens out "),
            bold_val(fmt_tokens(app.today_tokens_out)),
        ]),
    ];

    if app.burn_rate > 0.0 {
        lines.push(Line::from(vec![
            Span::raw("  Burn rate  "),
            Span::styled(
                format!("{:.0} tok/min", app.burn_rate),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
        ]));
    } else {
        lines.push(Line::from(vec![
            Span::raw("  Burn rate  "),
            Span::styled("idle", Style::default().fg(Color::DarkGray)),
        ]));
    }

    // Source breakdown for today from recent sessions
    let mut today_src: Vec<(&str, usize)> = vec![];
    let today_prefix = chrono::Local::now().format("%Y-%m-%d").to_string();
    for r in &app.recent {
        if r.created_at.starts_with(&today_prefix) {
            if let Some(entry) = today_src.iter_mut().find(|(s, _)| *s == r.source.as_str()) {
                entry.1 += 1;
            } else {
                today_src.push((&r.source, 1));
            }
        }
    }
    if !today_src.is_empty() {
        let parts: Vec<Span> = today_src
            .iter()
            .flat_map(|(src, cnt)| {
                vec![
                    Span::styled(format!(" {src}:"), Style::default().fg(source_color(src))),
                    Span::styled(format!("{cnt}"), Style::default().fg(Color::White)),
                ]
            })
            .collect();
        let mut spans = vec![Span::raw(" ")];
        spans.extend(parts);
        lines.push(Line::from(spans));
    }

    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_errors_panel(f: &mut Frame, area: Rect, app: &App) {
    let title_style = if app.today_errors > 0 {
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD)
    };

    let title = if app.today_errors > 0 {
        format!(" ⚠ {} Errors Today ", app.today_errors)
    } else {
        " ✓ No Errors ".to_string()
    };

    let block = Block::bordered().title(title).title_style(title_style);
    let inner = block.inner(area);
    f.render_widget(block, area);

    if app.recent_errors.is_empty() {
        f.render_widget(
            Paragraph::new(vec![
                Line::from(""),
                Line::from(Span::styled(
                    "  All tools executing cleanly",
                    Style::default().fg(Color::DarkGray),
                )),
            ]),
            inner,
        );
        return;
    }

    let lines: Vec<Line> = app
        .recent_errors
        .iter()
        .take(inner.height as usize)
        .map(|(tool, summary, time, _tid)| {
            let ts = if time.len() >= 16 {
                &time[11..16] // HH:MM
            } else {
                time.as_str()
            };
            Line::from(vec![
                Span::styled(format!(" {ts} "), Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{:<18}", truncate(tool, 16)),
                    Style::default().fg(Color::Red),
                ),
                Span::styled(truncate(summary, 30), Style::default().fg(Color::White)),
            ])
        })
        .collect();

    f.render_widget(Paragraph::new(lines), inner);
}

// -- Recent sessions table ---------------------------------------------------

fn draw_recent(f: &mut Frame, area: Rect, recent: &[TranscriptRecord], offset: usize) {
    let block = Block::bordered().title(" Recent Sessions ↑↓ ").title_style(
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    );

    let header = Row::new(vec![
        Cell::from("Source"),
        Cell::from("Title"),
        Cell::from("Model"),
        Cell::from("Turns"),
        Cell::from("Tokens In"),
        Cell::from("Date"),
    ])
    .style(
        Style::default()
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            .fg(Color::Cyan),
    );

    let rows: Vec<Row> = recent
        .iter()
        .skip(offset)
        .map(|r| {
            let src = r.source.as_str();
            let model = if !r.model_name.is_empty() {
                truncate(&r.model_name, 18)
            } else if !r.model_provider.is_empty() {
                truncate(&r.model_provider, 18)
            } else {
                "—".to_string()
            };
            let date_str = if r.created_at.len() >= 16 {
                r.created_at[..16].replace('T', " ")
            } else {
                r.created_at.clone()
            };

            Row::new(vec![
                Cell::from(Span::styled(
                    format!("{:<15}", truncate(src, 13)),
                    Style::default().fg(source_color(src)),
                )),
                Cell::from(truncate(&r.title, 35)),
                Cell::from(Span::styled(model, Style::default().fg(Color::DarkGray))),
                Cell::from(Span::styled(
                    format!("{:>4}", r.turns_total),
                    Style::default().fg(Color::White),
                )),
                Cell::from(Span::styled(
                    format!("{:>7}", fmt_tokens(r.tokens_in)),
                    Style::default().fg(Color::Green),
                )),
                Cell::from(Span::styled(date_str, Style::default().fg(Color::DarkGray))),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(15),
        Constraint::Min(20),
        Constraint::Length(20),
        Constraint::Length(6),
        Constraint::Length(9),
        Constraint::Length(17),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED));

    f.render_widget(table, area);
}

// -- Charts row: hourly today | 14-day trend ---------------------------------

fn draw_charts_row(f: &mut Frame, area: Rect, app: &App) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    draw_hourly_chart(f, cols[0], &app.hourly_tokens);
    draw_daily_chart(f, cols[1], &app.daily_tokens);
}

fn draw_hourly_chart(f: &mut Frame, area: Rect, hourly: &[(i32, i64, i64)]) {
    let block = Block::bordered().title(" Today (hourly) ").title_style(
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    );

    // Build a full 24-hour array
    let mut buckets = [0u64; 24];
    let mut peak_hr: usize = 0;
    let mut peak_val: u64 = 0;
    for &(hr, tin, tout) in hourly {
        let h = hr as usize;
        if h < 24 {
            let total = (tin + tout) as u64;
            buckets[h] = total;
            if total > peak_val {
                peak_val = total;
                peak_hr = h;
            }
        }
    }

    let inner = block.inner(area);
    f.render_widget(block, area);

    if peak_val == 0 {
        f.render_widget(
            Paragraph::new(Span::styled(
                "  No activity today",
                Style::default().fg(Color::DarkGray),
            )),
            inner,
        );
        return;
    }

    // Sparkline + label
    let spark_area = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: inner.height.saturating_sub(1),
    };
    let label_area = Rect {
        x: inner.x,
        y: inner.y + spark_area.height,
        width: inner.width,
        height: 1,
    };

    let spark = Sparkline::default()
        .data(&buckets)
        .style(Style::default().fg(Color::LightGreen));
    f.render_widget(spark, spark_area);

    let label = Line::from(vec![
        Span::styled(" 0h", Style::default().fg(Color::DarkGray)),
        Span::raw("      "),
        Span::styled("6h", Style::default().fg(Color::DarkGray)),
        Span::raw("      "),
        Span::styled("12h", Style::default().fg(Color::DarkGray)),
        Span::raw("     "),
        Span::styled("18h", Style::default().fg(Color::DarkGray)),
        Span::raw("    "),
        Span::styled("23h", Style::default().fg(Color::DarkGray)),
        Span::raw("  "),
        Span::styled(
            format!("peak {}:00 {}", peak_hr, fmt_tokens(peak_val as i64)),
            Style::default().fg(Color::Yellow),
        ),
    ]);
    f.render_widget(Paragraph::new(label), label_area);
}

fn draw_daily_chart(f: &mut Frame, area: Rect, daily: &[(String, i64, i64)]) {
    let block = Block::bordered().title(" 14-Day Trend ").title_style(
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    );

    let inner = block.inner(area);
    f.render_widget(block, area);

    if daily.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled(
                "  No data",
                Style::default().fg(Color::DarkGray),
            )),
            inner,
        );
        return;
    }

    let data: Vec<u64> = daily.iter().map(|(_, ti, to)| (ti + to) as u64).collect();

    let spark_area = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: inner.height.saturating_sub(1),
    };
    let label_area = Rect {
        x: inner.x,
        y: inner.y + spark_area.height,
        width: inner.width,
        height: 1,
    };

    let spark = Sparkline::default()
        .data(&data)
        .style(Style::default().fg(Color::LightBlue));
    f.render_widget(spark, spark_area);

    // Labels: first date ... last date + total
    let first = daily.first().map(|(d, _, _)| d.as_str()).unwrap_or("?");
    let last = daily.last().map(|(d, _, _)| d.as_str()).unwrap_or("?");
    let total: u64 = data.iter().sum();

    let label = Line::from(vec![
        Span::styled(format!(" {first}"), Style::default().fg(Color::DarkGray)),
        Span::raw("  "),
        Span::styled(
            format!("total: {}", fmt_tokens(total as i64)),
            Style::default().fg(Color::Yellow),
        ),
        Span::raw("  "),
        Span::styled(format!("{last}"), Style::default().fg(Color::DarkGray)),
    ]);
    f.render_widget(Paragraph::new(label), label_area);
}

// -- Log tail ----------------------------------------------------------------

fn draw_log(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::bordered().title(" Live Log ").title_style(
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    );
    let inner = block.inner(area);
    f.render_widget(block, area);

    if app.log_lines.is_empty() {
        let hint = if !app.service_running {
            "  Service not running. Start with: transcriptd service up"
        } else {
            "  Waiting for log output…"
        };
        f.render_widget(
            Paragraph::new(Span::styled(hint, Style::default().fg(Color::DarkGray))),
            inner,
        );
        return;
    }

    let max_lines = inner.height as usize;
    let lines: Vec<Line> = app
        .log_lines
        .iter()
        .rev()
        .take(max_lines)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|line| {
            // Color-code log lines by content
            let style = if line.contains("ERROR") || line.contains("error") {
                Style::default().fg(Color::Red)
            } else if line.contains("WARN") || line.contains("warn") {
                Style::default().fg(Color::Yellow)
            } else if line.contains("ingested") || line.contains("Ingested") {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            Line::from(Span::styled(
                format!(" {}", truncate(line, inner.width as usize - 2)),
                style,
            ))
        })
        .collect();

    f.render_widget(Paragraph::new(lines), inner);
}
