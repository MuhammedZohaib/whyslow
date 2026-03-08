use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io;
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::Utc;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Cell, Clear, Paragraph, Row, Sparkline, Table, TableState, Wrap,
};
use ratatui::{Frame, Terminal};

use crate::collect;
use crate::diagnose;
use crate::model::{CollectionWindow, Diagnosis, Report, RunConfig, Sample};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MainView {
    Diagnosis,
    Disk,
    Process,
    Network,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProcessFilter {
    All,
    Browser,
    Dev,
    Marker,
}

impl ProcessFilter {
    fn next(self) -> Self {
        match self {
            Self::All => Self::Browser,
            Self::Browser => Self::Dev,
            Self::Dev => Self::Marker,
            Self::Marker => Self::All,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Browser => "browser",
            Self::Dev => "dev",
            Self::Marker => "marker",
        }
    }
}

pub fn run_watch(mut config: RunConfig, watch_seconds: u64) -> Result<()> {
    config.watch_seconds = Some(watch_seconds);

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = watch_loop(&mut terminal, &config, watch_seconds.max(1));

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

struct WatchState {
    report: Option<Report>,
    latest_sample: Option<Sample>,
    last_error: Option<String>,
    last_updated: Option<chrono::DateTime<Utc>>,
    next_sample_at: Instant,
    next_analysis_at: Instant,
    runs: usize,
    samples_collected: usize,
    buffered_samples: usize,
    cpu_trend: VecDeque<u64>,
    mem_trend: VecDeque<u64>,
    disk_trend: VecDeque<u64>,
    net_trend_mbps: VecDeque<u64>,
    trend_capacity: usize,
    main_view: MainView,
    process_filter: ProcessFilter,
    help_open: bool,
    detail_open: bool,
}

impl WatchState {
    fn new(_window_capacity: usize, trend_capacity: usize) -> Self {
        Self {
            report: None,
            latest_sample: None,
            last_error: None,
            last_updated: None,
            next_sample_at: Instant::now(),
            next_analysis_at: Instant::now(),
            runs: 0,
            samples_collected: 0,
            buffered_samples: 0,
            cpu_trend: VecDeque::with_capacity(trend_capacity),
            mem_trend: VecDeque::with_capacity(trend_capacity),
            disk_trend: VecDeque::with_capacity(trend_capacity),
            net_trend_mbps: VecDeque::with_capacity(trend_capacity),
            trend_capacity,
            main_view: MainView::Diagnosis,
            process_filter: ProcessFilter::All,
            help_open: false,
            detail_open: false,
        }
    }

    fn push_trend(&mut self, sample: &Sample) {
        push_metric(
            &mut self.cpu_trend,
            sample.cpu_total_percent.map(|v| v.clamp(0.0, 100.0) as u64),
            self.trend_capacity,
        );

        let mem = match (sample.memory_used_bytes, sample.memory_total_bytes) {
            (Some(used), Some(total)) if total > 0 => {
                Some(((used as f64 * 100.0 / total as f64) as u64).min(100))
            }
            _ => None,
        };
        push_metric(&mut self.mem_trend, mem, self.trend_capacity);

        push_metric(
            &mut self.disk_trend,
            sample.disk.busy_percent.map(|v| v.clamp(0.0, 100.0) as u64),
            self.trend_capacity,
        );

        let net_total_mbps = (sample.network.down_bytes_per_sec.unwrap_or(0.0)
            + sample.network.up_bytes_per_sec.unwrap_or(0.0))
            / (1024.0 * 1024.0);
        self.net_trend_mbps
            .push_back(net_total_mbps.round().max(0.0) as u64);
        while self.net_trend_mbps.len() > self.trend_capacity {
            self.net_trend_mbps.pop_front();
        }
    }
}

fn watch_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    config: &RunConfig,
    watch_seconds: u64,
) -> Result<()> {
    let interval_ms = config.interval_ms.max(100);
    let sample_interval = Duration::from_millis(interval_ms);
    let diagnosis_interval = Duration::from_secs(watch_seconds);
    let window_capacity = ((config.sample_window_secs * 1000) / interval_ms).max(1) as usize;

    // Capture a broader process set for the exact-process panel in watch mode.
    let mut collector = collect::LiveCollector::new(config.top_n.max(64), interval_ms);
    let host = collector.host_info();
    let mut ring: VecDeque<Sample> = VecDeque::with_capacity(window_capacity + 1);
    let mut state = WatchState::new(window_capacity, 120);

    loop {
        let now = Instant::now();
        let mut sampled = false;

        if now >= state.next_sample_at {
            let sample = collector.sample_once();
            state.push_trend(&sample);
            state.latest_sample = Some(sample.clone());
            ring.push_back(sample);
            if ring.len() > window_capacity {
                ring.pop_front();
            }

            state.samples_collected += 1;
            state.buffered_samples = ring.len();
            state.next_sample_at = now + sample_interval;
            sampled = true;
        }

        let analysis_due = sampled && (state.report.is_none() || now >= state.next_analysis_at);

        if analysis_due && !ring.is_empty() {
            let samples: Vec<Sample> = ring.iter().cloned().collect();
            let started_at = samples
                .first()
                .map(|s| s.timestamp)
                .unwrap_or_else(Utc::now);
            let ended_at = samples.last().map(|s| s.timestamp).unwrap_or_else(Utc::now);

            let window = CollectionWindow {
                started_at,
                ended_at,
                interval_ms,
                host: host.clone(),
                unavailable_metrics: collect::unavailable_metrics_from_samples(&samples),
                samples,
            };

            let analysis = diagnose::analyze(config, &window);
            let report = crate::report::build_report(config.clone(), window, analysis);
            let export_error = export_watch_report(config, &report)
                .err()
                .map(|e| format!("export failed: {e}"));

            state.report = Some(report);
            state.last_updated = Some(Utc::now());
            state.last_error = export_error;
            state.runs += 1;
            state.next_analysis_at = now + diagnosis_interval;
        }

        terminal.draw(|frame| draw_ui(frame, &state, interval_ms, watch_seconds))?;

        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') => break,
                        KeyCode::Esc => {
                            if state.help_open {
                                state.help_open = false;
                            } else if state.detail_open {
                                state.detail_open = false;
                            } else {
                                break;
                            }
                        }
                        KeyCode::Char('r') => {
                            state.next_sample_at = Instant::now();
                            state.next_analysis_at = Instant::now();
                        }
                        KeyCode::Char('d') => state.main_view = MainView::Disk,
                        KeyCode::Char('p') => state.main_view = MainView::Process,
                        KeyCode::Char('n') => state.main_view = MainView::Network,
                        KeyCode::Char('h') => state.help_open = !state.help_open,
                        KeyCode::Char('f') => state.process_filter = state.process_filter.next(),
                        KeyCode::Enter => {
                            if state.report.is_some() {
                                state.detail_open = !state.detail_open;
                            }
                        }
                        _ => {
                            state.main_view = MainView::Diagnosis;
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

fn draw_ui(frame: &mut Frame<'_>, state: &WatchState, interval_ms: u64, watch_seconds: u64) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Length(14),
            Constraint::Min(12),
            Constraint::Length(7),
            Constraint::Length(3),
        ])
        .split(frame.area());

    let next_sample_ms = state
        .next_sample_at
        .saturating_duration_since(Instant::now())
        .as_millis();
    let next_analysis_s = state
        .next_analysis_at
        .saturating_duration_since(Instant::now())
        .as_secs();

    let (primary, health, sev_color) = status_badge(state.report.as_ref());
    let (duration_text, confidence_text) = state
        .report
        .as_ref()
        .and_then(|r| r.diagnoses.first())
        .map(|d| {
            let dur = d
                .duration_seconds
                .map(|secs| format!("{secs:.0}s"))
                .unwrap_or_else(|| "n/a".to_string());
            let conf = format!("{:.0}%", d.confidence * 100.0);
            (dur, conf)
        })
        .unwrap_or_else(|| ("n/a".to_string(), "n/a".to_string()));

    let title = vec![
        Line::from(vec![
            Span::styled(
                format!("System Bottleneck: {}  ", primary),
                Style::default().fg(sev_color).add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(
                "Duration: {} | Confidence: {} | System Health: {}",
                duration_text, confidence_text, health
            )),
        ]),
        Line::from(format!(
            "sample={}ms | diagnosis={}s | next sample={}ms | next diagnosis={}s | view={}",
            interval_ms,
            watch_seconds,
            next_sample_ms,
            next_analysis_s,
            view_label(state.main_view)
        )),
    ];

    let header = Paragraph::new(title)
        .block(Block::default().borders(Borders::ALL).title("whyslow"))
        .wrap(Wrap { trim: true });
    frame.render_widget(header, chunks[0]);

    draw_summary_and_trends(frame, chunks[1], state);

    match state.main_view {
        MainView::Diagnosis => draw_diagnosis_view(frame, chunks[2], state),
        MainView::Disk => draw_disk_view(frame, chunks[2], state.latest_sample.as_ref()),
        MainView::Process => draw_process_view(
            frame,
            chunks[2],
            state.latest_sample.as_ref(),
            state.process_filter,
        ),
        MainView::Network => draw_network_view(
            frame,
            chunks[2],
            state.latest_sample.as_ref(),
            &state.net_trend_mbps,
        ),
    }

    draw_actions_panel(frame, chunks[3], state.report.as_ref());

    let status = Paragraph::new(vec![
        Line::from(format!(
            "Navigation: p processes | d disk | n network | default diagnosis | filter={} (f)",
            state.process_filter.label()
        )),
        Line::from("Diagnostics: Enter details | r refresh | h help | q or Esc quit"),
    ])
    .style(Style::default().fg(Color::Gray))
    .block(Block::default().borders(Borders::TOP).title("Keys"));
    frame.render_widget(status, chunks[4]);

    if state.help_open {
        draw_help_popup(frame);
    }

    if state.detail_open {
        if let Some(report) = state.report.as_ref() {
            draw_detail_popup(frame, report.diagnoses.first());
        }
    }
}

fn draw_summary_and_trends(frame: &mut Frame<'_>, area: Rect, state: &WatchState) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(52), Constraint::Percentage(48)])
        .split(area);

    let mut summary_lines = Vec::new();
    if let Some(report) = &state.report {
        let host = &report.host;
        summary_lines.push(Line::from(format!(
            "Host: {} | {} {}",
            host.hostname
                .clone()
                .unwrap_or_else(|| "unknown-host".to_string()),
            host.os_name
                .clone()
                .unwrap_or_else(|| "unknown-os".to_string()),
            host.os_version
                .clone()
                .unwrap_or_else(|| "unknown-version".to_string())
        )));

        summary_lines.push(Line::from(format!(
            "Kernel: {} | Uptime: {}",
            host.kernel_version
                .clone()
                .unwrap_or_else(|| "n/a".to_string()),
            host.uptime_secs
                .map(format_uptime)
                .unwrap_or_else(|| "n/a".to_string())
        )));

        summary_lines.push(Line::from(format!(
            "Hardware: CPU phys {} / logical {} | RAM {}",
            host.cpu_physical_core_count
                .map(|v| v.to_string())
                .unwrap_or_else(|| "n/a".to_string()),
            host.cpu_logical_core_count
                .map(|v| v.to_string())
                .unwrap_or_else(|| host.cpu_core_count.to_string()),
            host.total_memory_bytes
                .map(format_gib)
                .unwrap_or_else(|| "n/a".to_string())
        )));

        if let Some(sample) = state.latest_sample.as_ref() {
            let current_mem = current_memory_percent(sample);
            let proc_count = sample
                .process_count
                .map(|v| v.to_string())
                .unwrap_or_else(|| "n/a".to_string());

            summary_lines.push(Line::from(format!(
                "Live: CPU {} {} | Mem {} {} | Disk {} {} | Procs {}",
                fmt_opt_percent(sample.cpu_total_percent),
                trend_marker(&state.cpu_trend),
                fmt_opt_percent(current_mem),
                trend_marker(&state.mem_trend),
                fmt_opt_percent(sample.disk.busy_percent),
                trend_marker(&state.disk_trend),
                proc_count
            )));

            summary_lines.push(Line::from(format!(
                "Network: Down {} | Up {} | Active interfaces {}",
                fmt_opt_mbps(sample.network.down_bytes_per_sec),
                fmt_opt_mbps(sample.network.up_bytes_per_sec),
                sample
                    .network
                    .active_interface_count
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "n/a".to_string())
            )));

            summary_lines.push(Line::from("System Pressure"));
            summary_lines.push(Line::from(pressure_bar("CPU", sample.cpu_total_percent)));
            summary_lines.push(Line::from(pressure_bar("Memory", current_mem)));
            summary_lines.push(Line::from(pressure_bar("Disk", sample.disk.busy_percent)));
            summary_lines.push(Line::from(pressure_bar(
                "Network",
                network_pressure_percent(sample),
            )));
            summary_lines.push(Line::from(busiest_disk_line(sample)));
        }

        if let Some(top) = report.diagnoses.first() {
            summary_lines.push(Line::from(format!(
                "Primary: {} ({:.0}% confidence, {:.0}s)",
                top.kind.as_str(),
                top.confidence * 100.0,
                top.duration_seconds.unwrap_or(0.0)
            )));
        } else {
            summary_lines.push(Line::from("Primary: none"));
        }

        summary_lines.push(Line::from(format!(
            "Window: buffered {} / {} | analyses {}",
            state.buffered_samples, report.sample_count, state.runs
        )));

        if !report.unavailable_metrics.is_empty() {
            summary_lines.push(Line::from(format!(
                "Partial metrics: {}",
                report.unavailable_metrics.join(", ")
            )));
        }
    } else {
        summary_lines.push(Line::from("Collecting live samples..."));
    }

    if let Some(updated) = state.last_updated {
        summary_lines.push(Line::from(format!(
            "Last diagnosis update: {} UTC",
            updated.format("%H:%M:%S")
        )));
    }

    if let Some(error) = &state.last_error {
        summary_lines.push(Line::from(format!("Last error: {error}")));
    }

    let summary = Paragraph::new(summary_lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("System Context"),
        )
        .wrap(Wrap { trim: true });
    frame.render_widget(summary, columns[0]);

    let trend_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(33),
            Constraint::Percentage(33),
            Constraint::Percentage(34),
        ])
        .split(columns[1]);

    draw_trend_card(
        frame,
        trend_rows[0],
        "CPU trend (60s)",
        &state.cpu_trend,
        Color::LightRed,
        "%",
        100,
    );
    draw_trend_card(
        frame,
        trend_rows[1],
        "Memory trend (60s)",
        &state.mem_trend,
        Color::Yellow,
        "%",
        100,
    );
    draw_trend_card(
        frame,
        trend_rows[2],
        "Disk busy trend (60s)",
        &state.disk_trend,
        Color::LightBlue,
        "%",
        100,
    );
}

fn draw_trend_card(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    trend: &VecDeque<u64>,
    color: Color,
    unit: &str,
    default_max: u64,
) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(area);

    let data: Vec<u64> = trend.iter().copied().collect();
    let max_value = data
        .iter()
        .copied()
        .max()
        .unwrap_or(default_max)
        .max(default_max);

    let spark = Sparkline::default()
        .block(Block::default().borders(Borders::ALL).title(title))
        .data(&data)
        .max(max_value)
        .style(Style::default().fg(color));
    frame.render_widget(spark, rows[0]);

    let (avg, min, max) = trend_stats(trend);
    let stats = Paragraph::new(format!(
        "avg {:.0}{unit}   min {:.0}{unit}   max {:.0}{unit}",
        avg, min, max
    ))
    .style(Style::default().fg(Color::Gray));
    frame.render_widget(stats, rows[1]);
}

fn trend_stats(trend: &VecDeque<u64>) -> (f64, f64, f64) {
    if trend.is_empty() {
        return (0.0, 0.0, 0.0);
    }

    let mut min_v = u64::MAX;
    let mut max_v = 0_u64;
    let mut total = 0_u64;

    for value in trend {
        min_v = min_v.min(*value);
        max_v = max_v.max(*value);
        total += *value;
    }

    let avg = total as f64 / trend.len() as f64;
    (avg, min_v as f64, max_v as f64)
}

fn pressure_bar(label: &str, percent: Option<f32>) -> String {
    let Some(value) = percent else {
        return format!("{label:<8} [..........] n/a");
    };

    let clamped = value.clamp(0.0, 100.0);
    let filled = ((clamped / 10.0).round() as usize).min(10);
    let bar = format!("{}{}", "#".repeat(filled), ".".repeat(10 - filled));
    format!("{label:<8} [{bar}] {:>3.0}%", clamped)
}

fn network_pressure_percent(sample: &Sample) -> Option<f32> {
    let down = sample.network.down_bytes_per_sec?;
    let up = sample.network.up_bytes_per_sec?;
    let mb_per_sec = (down + up) / (1024.0 * 1024.0);
    Some(((mb_per_sec / 20.0) * 100.0).clamp(0.0, 100.0) as f32)
}

fn current_memory_percent(sample: &Sample) -> Option<f32> {
    match (sample.memory_used_bytes, sample.memory_total_bytes) {
        (Some(used), Some(total)) if total > 0 => Some((used as f64 * 100.0 / total as f64) as f32),
        _ => None,
    }
}

fn trend_marker(buf: &VecDeque<u64>) -> &'static str {
    if buf.len() < 4 {
        return "=";
    }

    let first = *buf.front().unwrap_or(&0) as i64;
    let last = *buf.back().unwrap_or(&0) as i64;
    let delta = last - first;

    if delta >= 6 {
        "^"
    } else if delta <= -6 {
        "v"
    } else {
        "="
    }
}

fn busiest_disk_line(sample: &Sample) -> String {
    if sample.disk_devices.is_empty() {
        return "Busiest disk: n/a".to_string();
    }

    let mut best: Option<&crate::model::DiskDeviceSample> = None;
    let mut best_score = -1.0_f64;

    for disk in &sample.disk_devices {
        let busy = disk.busy_percent.unwrap_or(0.0) as f64;
        let throughput_mb = (disk.read_bytes_per_sec.unwrap_or(0.0)
            + disk.write_bytes_per_sec.unwrap_or(0.0))
            / (1024.0 * 1024.0);
        let latency = disk.avg_latency_ms.unwrap_or(0.0) as f64;
        let score = busy + (throughput_mb * 0.45) + (latency * 0.30);

        if score > best_score {
            best_score = score;
            best = Some(disk);
        }
    }

    let Some(disk) = best else {
        return "Busiest disk: n/a".to_string();
    };

    format!(
        "Busiest disk: {} busy {} | R {} W {} | Lat {}",
        disk.label,
        fmt_opt_percent(disk.busy_percent),
        fmt_opt_mbps(disk.read_bytes_per_sec),
        fmt_opt_mbps(disk.write_bytes_per_sec),
        fmt_opt_ms(disk.avg_latency_ms)
    )
}
fn draw_diagnosis_view(frame: &mut Frame<'_>, area: Rect, state: &WatchState) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
        .split(area);

    let mut rows = Vec::new();
    if let Some(report) = &state.report {
        for diagnosis in &report.diagnoses {
            let reason = diagnosis
                .evidence
                .first()
                .map(|e| format!("{}: {}", e.label, e.detail))
                .unwrap_or_else(|| "n/a".to_string());

            rows.push(Row::new(vec![
                Cell::from(diagnosis.kind.as_str().to_string()),
                Cell::from(format!("{:.0}%", diagnosis.confidence * 100.0)),
                Cell::from(
                    diagnosis
                        .duration_seconds
                        .map(|v| format!("{v:.0}s"))
                        .unwrap_or_else(|| "n/a".to_string()),
                ),
                Cell::from(reason),
            ]));
        }
    }

    if rows.is_empty() {
        rows.push(Row::new(vec![
            Cell::from("n/a"),
            Cell::from("n/a"),
            Cell::from("n/a"),
            Cell::from("waiting for diagnosis"),
        ]));
    }

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(24),
            Constraint::Percentage(12),
            Constraint::Percentage(14),
            Constraint::Percentage(50),
        ],
    )
    .header(
        Row::new(vec![
            "Diagnosis",
            "Confidence",
            "Duration",
            "Why this diagnosis",
        ])
        .style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title("Likely Bottlenecks"),
    );

    frame.render_widget(table, cols[0]);
    draw_offending_panel(frame, cols[1], state.latest_sample.as_ref());
}

fn draw_offending_panel(frame: &mut Frame<'_>, area: Rect, sample: Option<&Sample>) {
    let processes = sample
        .map(|s| ranked_processes(s, ProcessFilter::All))
        .unwrap_or_default();

    let mut rows = Vec::new();
    for proc in processes.into_iter().take(10) {
        let io =
            proc.io_read_bytes_per_sec.unwrap_or(0.0) + proc.io_write_bytes_per_sec.unwrap_or(0.0);
        rows.push(Row::new(vec![
            Cell::from(proc.pid.to_string()),
            Cell::from(proc.name),
            Cell::from(fmt_opt_percent(proc.cpu_percent)),
            Cell::from(
                proc.memory_bytes
                    .map(format_bytes_human)
                    .unwrap_or_else(|| "n/a".to_string()),
            ),
            Cell::from(format_mbps(io)),
        ]));
    }

    if rows.is_empty() {
        rows.push(Row::new(vec![
            Cell::from("n/a"),
            Cell::from("waiting for samples"),
            Cell::from("n/a"),
            Cell::from("n/a"),
            Cell::from("n/a"),
        ]));
    }

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(14),
            Constraint::Percentage(34),
            Constraint::Percentage(14),
            Constraint::Percentage(20),
            Constraint::Percentage(18),
        ],
    )
    .header(
        Row::new(vec!["PID", "Name", "CPU", "Mem", "Disk R/W"]).style(
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title("Top Offending Processes"),
    );

    frame.render_widget(table, area);
}

fn draw_disk_view(frame: &mut Frame<'_>, area: Rect, sample: Option<&Sample>) {
    let mut rows = Vec::new();
    if let Some(sample) = sample {
        for disk in &sample.disk_devices {
            let used = match (disk.total_bytes, disk.available_bytes) {
                (Some(total), Some(avail)) if total > 0 => {
                    format!(
                        "{:.1}%",
                        ((total - avail) as f64 * 100.0 / total as f64).clamp(0.0, 100.0)
                    )
                }
                _ => "n/a".to_string(),
            };

            rows.push(Row::new(vec![
                Cell::from(disk.label.clone()),
                Cell::from(fmt_opt_percent(disk.busy_percent)),
                Cell::from(fmt_opt_mbps(disk.read_bytes_per_sec)),
                Cell::from(fmt_opt_mbps(disk.write_bytes_per_sec)),
                Cell::from(fmt_opt_ms(disk.avg_latency_ms)),
                Cell::from(used),
            ]));
        }
    }

    if rows.is_empty() {
        rows.push(Row::new(vec![
            Cell::from("n/a"),
            Cell::from("n/a"),
            Cell::from("n/a"),
            Cell::from("n/a"),
            Cell::from("n/a"),
            Cell::from("n/a"),
        ]));
    }

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(20),
            Constraint::Percentage(12),
            Constraint::Percentage(18),
            Constraint::Percentage(18),
            Constraint::Percentage(16),
            Constraint::Percentage(16),
        ],
    )
    .header(
        Row::new(vec!["Drive", "Busy", "Read", "Write", "Latency", "Used"]).style(
            Style::default()
                .fg(Color::LightBlue)
                .add_modifier(Modifier::BOLD),
        ),
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title("Disk Breakdown"),
    );

    frame.render_widget(table, area);
}

fn draw_process_view(
    frame: &mut Frame<'_>,
    area: Rect,
    sample: Option<&Sample>,
    filter: ProcessFilter,
) {
    let processes = sample
        .map(|s| ranked_processes(s, filter))
        .unwrap_or_default();

    let mut rows = Vec::new();
    for proc in processes.into_iter().take(18) {
        let io =
            proc.io_read_bytes_per_sec.unwrap_or(0.0) + proc.io_write_bytes_per_sec.unwrap_or(0.0);
        rows.push(Row::new(vec![
            Cell::from(proc.pid.to_string()),
            Cell::from(proc.name),
            Cell::from(fmt_opt_percent(proc.cpu_percent)),
            Cell::from(
                proc.memory_bytes
                    .map(format_bytes_human)
                    .unwrap_or_else(|| "n/a".to_string()),
            ),
            Cell::from(format_mbps(io)),
        ]));
    }

    if rows.is_empty() {
        rows.push(Row::new(vec![
            Cell::from("n/a"),
            Cell::from("no processes for filter"),
            Cell::from("n/a"),
            Cell::from("n/a"),
            Cell::from("n/a"),
        ]));
    }

    let mut state = TableState::default();
    state.select(Some(0));

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(12),
            Constraint::Percentage(38),
            Constraint::Percentage(14),
            Constraint::Percentage(18),
            Constraint::Percentage(18),
        ],
    )
    .header(
        Row::new(vec!["PID", "Name", "CPU", "Mem", "I/O"]).style(
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!("Top Processes (filter: {})", filter.label())),
    );

    frame.render_stateful_widget(table, area, &mut state);
}

fn draw_network_view(
    frame: &mut Frame<'_>,
    area: Rect,
    sample: Option<&Sample>,
    net_trend_mbps: &VecDeque<u64>,
) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(7), Constraint::Min(5)])
        .split(area);

    let mut lines = Vec::new();
    if let Some(sample) = sample {
        lines.push(Line::from(format!(
            "Down: {}",
            fmt_opt_mbps(sample.network.down_bytes_per_sec)
        )));
        lines.push(Line::from(format!(
            "Up:   {}",
            fmt_opt_mbps(sample.network.up_bytes_per_sec)
        )));
        lines.push(Line::from(format!(
            "Interfaces: {} (active {})",
            sample
                .network
                .interface_count
                .map(|v| v.to_string())
                .unwrap_or_else(|| "n/a".to_string()),
            sample
                .network
                .active_interface_count
                .map(|v| v.to_string())
                .unwrap_or_else(|| "n/a".to_string())
        )));

        let top_proxy = ranked_processes(sample, ProcessFilter::All)
            .into_iter()
            .max_by(|a, b| {
                let a_io = a.io_read_bytes_per_sec.unwrap_or(0.0)
                    + a.io_write_bytes_per_sec.unwrap_or(0.0);
                let b_io = b.io_read_bytes_per_sec.unwrap_or(0.0)
                    + b.io_write_bytes_per_sec.unwrap_or(0.0);
                a_io.partial_cmp(&b_io).unwrap_or(std::cmp::Ordering::Equal)
            });

        if let Some(proc) = top_proxy {
            let io = proc.io_read_bytes_per_sec.unwrap_or(0.0)
                + proc.io_write_bytes_per_sec.unwrap_or(0.0);
            lines.push(Line::from(format!(
                "Top network process (I/O proxy): {} {}",
                proc.name,
                format_mbps(io)
            )));
        } else {
            lines.push(Line::from("Top network process: n/a"));
        }
    } else {
        lines.push(Line::from("Collecting network samples..."));
    }

    let network = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("Network"))
        .wrap(Wrap { trim: true });
    frame.render_widget(network, rows[0]);

    draw_trend_card(
        frame,
        rows[1],
        "Network throughput trend (MB/s)",
        net_trend_mbps,
        Color::Cyan,
        "MB/s",
        1,
    );
}

fn draw_actions_panel(frame: &mut Frame<'_>, area: Rect, report: Option<&Report>) {
    let mut seen = HashSet::new();
    let mut lines = Vec::new();

    if let Some(report) = report {
        for diagnosis in &report.diagnoses {
            for suggestion in &diagnosis.suggestions {
                if seen.insert(suggestion.action.clone()) {
                    lines.push(Line::from(format!(
                        "- {} ({})",
                        suggestion.action, suggestion.rationale
                    )));
                }
            }
        }
    }

    if lines.is_empty() {
        lines.push(Line::from("- No high-confidence actions yet"));
    }

    let actions = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Suggested Actions"),
        )
        .wrap(Wrap { trim: true });
    frame.render_widget(actions, area);
}

fn draw_help_popup(frame: &mut Frame<'_>) {
    let area = centered_rect(72, 66, frame.area());
    frame.render_widget(Clear, area);

    let help_lines = vec![
        Line::from("Navigation"),
        Line::from("- d: Disk breakdown view"),
        Line::from("- p: Process offender view"),
        Line::from("- n: Network view"),
        Line::from("- any other key: Diagnosis view"),
        Line::from(""),
        Line::from("Diagnostics"),
        Line::from("- Enter: Detailed diagnosis explanation"),
        Line::from("- f: Cycle process filter (all/browser/dev/marker)"),
        Line::from("- r: Force sample + diagnosis refresh"),
        Line::from("- h: Toggle this help"),
        Line::from("- q/Esc: Quit (Esc closes popups first)"),
    ];

    let help = Paragraph::new(help_lines)
        .alignment(Alignment::Left)
        .block(Block::default().title("Help").borders(Borders::ALL))
        .wrap(Wrap { trim: true });
    frame.render_widget(help, area);
}
fn draw_detail_popup(frame: &mut Frame<'_>, diagnosis: Option<&Diagnosis>) {
    let area = centered_rect(78, 78, frame.area());
    frame.render_widget(Clear, area);

    let mut lines = Vec::new();
    if let Some(d) = diagnosis {
        lines.push(Line::from(format!(
            "{} | Confidence {:.0}% | Duration {}",
            d.kind.as_str(),
            d.confidence * 100.0,
            d.duration_seconds
                .map(|v| format!("{v:.0}s"))
                .unwrap_or_else(|| "n/a".to_string())
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(format!("Explanation: {}", d.explanation)));
        lines.push(Line::from(""));
        lines.push(Line::from("Reasons:"));
        for ev in &d.evidence {
            lines.push(Line::from(format!("- {}: {}", ev.label, ev.detail)));
        }
        lines.push(Line::from(""));
        lines.push(Line::from("Suggested Fix:"));
        for s in &d.suggestions {
            lines.push(Line::from(format!("- {} ({})", s.action, s.rationale)));
        }
    } else {
        lines.push(Line::from("No diagnosis yet."));
    }

    let detail = Paragraph::new(lines)
        .block(
            Block::default()
                .title("Detailed Diagnosis")
                .borders(Borders::ALL),
        )
        .wrap(Wrap { trim: true });
    frame.render_widget(detail, area);
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

fn process_matches_filter(proc: &crate::model::ProcessSample, filter: ProcessFilter) -> bool {
    match filter {
        ProcessFilter::All => true,
        ProcessFilter::Browser => proc.is_browser,
        ProcessFilter::Dev => proc.is_dev_tool,
        ProcessFilter::Marker => proc.is_marker,
    }
}

fn ranked_processes(sample: &Sample, filter: ProcessFilter) -> Vec<crate::model::ProcessSample> {
    let mut dedup: HashMap<u32, crate::model::ProcessSample> = HashMap::new();

    for proc in sample
        .top_processes_cpu
        .iter()
        .chain(sample.top_processes_memory.iter())
    {
        if !process_matches_filter(proc, filter) {
            continue;
        }
        dedup
            .entry(proc.pid)
            .and_modify(|existing| merge_process(existing, proc))
            .or_insert_with(|| proc.clone());
    }

    let mut processes: Vec<_> = dedup.into_values().collect();
    processes.sort_by(|a, b| {
        process_pressure_score(b)
            .partial_cmp(&process_pressure_score(a))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    processes
}
fn merge_process(
    existing: &mut crate::model::ProcessSample,
    incoming: &crate::model::ProcessSample,
) {
    if incoming.cpu_percent.unwrap_or(0.0) > existing.cpu_percent.unwrap_or(0.0) {
        existing.cpu_percent = incoming.cpu_percent;
    }
    if incoming.memory_bytes.unwrap_or(0) > existing.memory_bytes.unwrap_or(0) {
        existing.memory_bytes = incoming.memory_bytes;
    }
    if incoming.io_read_bytes_per_sec.unwrap_or(0.0) > existing.io_read_bytes_per_sec.unwrap_or(0.0)
    {
        existing.io_read_bytes_per_sec = incoming.io_read_bytes_per_sec;
    }
    if incoming.io_write_bytes_per_sec.unwrap_or(0.0)
        > existing.io_write_bytes_per_sec.unwrap_or(0.0)
    {
        existing.io_write_bytes_per_sec = incoming.io_write_bytes_per_sec;
    }
    existing.is_browser |= incoming.is_browser;
    existing.is_dev_tool |= incoming.is_dev_tool;
    existing.is_marker |= incoming.is_marker;
}

fn process_pressure_score(proc: &crate::model::ProcessSample) -> f64 {
    let cpu = proc.cpu_percent.unwrap_or(0.0) as f64;
    let mem_gib = proc.memory_bytes.unwrap_or(0) as f64 / (1024.0 * 1024.0 * 1024.0);
    let io_mb = (proc.io_read_bytes_per_sec.unwrap_or(0.0)
        + proc.io_write_bytes_per_sec.unwrap_or(0.0))
        / (1024.0 * 1024.0);

    // Weighted score for "slowdown pressure" rather than CPU-only sorting.
    (cpu * 1.3) + (mem_gib * 7.0) + (io_mb * 0.7)
}

fn format_bytes_human(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

    let b = bytes as f64;
    if b >= GIB {
        format!("{:.2} GiB", b / GIB)
    } else if b >= MIB {
        format!("{:.0} MiB", b / MIB)
    } else if b >= KIB {
        format!("{:.0} KiB", b / KIB)
    } else {
        format!("{} B", bytes)
    }
}
fn status_badge(report: Option<&Report>) -> (String, String, Color) {
    let Some(report) = report else {
        return ("NONE".to_string(), "Collecting".to_string(), Color::Gray);
    };

    let primary = report
        .diagnoses
        .first()
        .map(|d| d.kind.as_str().to_ascii_uppercase())
        .unwrap_or_else(|| "NONE".to_string());

    let mut pressure = report
        .diagnoses
        .first()
        .map(|d| d.confidence)
        .unwrap_or(0.0);
    pressure = pressure.max(report.summary.avg_cpu_percent.unwrap_or(0.0) / 100.0 * 0.6);
    pressure = pressure.max(report.summary.avg_memory_used_percent.unwrap_or(0.0) / 100.0 * 0.6);
    pressure = pressure.max(report.summary.avg_disk_busy_percent.unwrap_or(0.0) / 100.0 * 0.6);

    if pressure >= 0.80 {
        (primary, "Critical".to_string(), Color::Red)
    } else if pressure >= 0.60 {
        (primary, "High pressure".to_string(), Color::LightRed)
    } else if pressure >= 0.35 {
        (primary, "Moderate pressure".to_string(), Color::Yellow)
    } else {
        (primary, "Healthy".to_string(), Color::Green)
    }
}

fn view_label(view: MainView) -> &'static str {
    match view {
        MainView::Diagnosis => "diagnosis",
        MainView::Disk => "disk",
        MainView::Process => "process",
        MainView::Network => "network",
    }
}
fn push_metric(buf: &mut VecDeque<u64>, value: Option<u64>, cap: usize) {
    buf.push_back(value.unwrap_or(0).min(100));
    while buf.len() > cap {
        buf.pop_front();
    }
}

fn fmt_opt_percent(v: Option<f32>) -> String {
    v.map(|x| format!("{x:.1}%"))
        .unwrap_or_else(|| "n/a".to_string())
}

fn format_mbps(v: f64) -> String {
    format!("{:.1} MB/s", v / (1024.0 * 1024.0))
}

fn fmt_opt_mbps(v: Option<f64>) -> String {
    v.map(format_mbps).unwrap_or_else(|| "n/a".to_string())
}

fn fmt_opt_ms(v: Option<f32>) -> String {
    v.map(|x| format!("{x:.1} ms"))
        .unwrap_or_else(|| "n/a".to_string())
}

fn format_uptime(secs: u64) -> String {
    let hours = secs / 3600;
    let mins = (secs % 3600) / 60;
    format!("{hours}h {mins}m")
}

fn format_gib(bytes: u64) -> String {
    format!("{:.1} GiB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
}

fn export_watch_report(config: &RunConfig, report: &Report) -> Result<()> {
    let Some(path) = config.export_path.as_deref() else {
        return Ok(());
    };

    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_else(|| "json".to_string());

    let content = if ext == "md" || ext == "markdown" {
        crate::report::markdown::render(report)
    } else {
        crate::report::json::render(report)?
    };

    fs::write(path, content)?;
    Ok(())
}
