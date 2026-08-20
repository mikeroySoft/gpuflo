use std::{
    collections::VecDeque,
    env,
    io::{self, Stdout},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use crossterm::{
    cursor::{Hide, Show},
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Padding, Paragraph},
};

const BG: Color = Color::Rgb(20, 20, 19);
const CREAM: Color = Color::Rgb(244, 225, 192);
const MUTED: Color = Color::Rgb(166, 146, 120);
const DIM: Color = Color::Rgb(102, 89, 74);
const RUST: Color = Color::Rgb(190, 76, 37);
const AMBER: Color = Color::Rgb(245, 158, 11);
const RED: Color = Color::Rgb(239, 68, 68);
const SAMPLE_INTERVAL: Duration = Duration::from_millis(250);
const FRAME_INTERVAL: Duration = Duration::from_millis(125);
const HISTORY_CAPACITY: usize = 240;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ViewMode {
    Mode,
    Compact,
    Mini,
    Tiny,
}

impl ViewMode {
    fn name(self) -> &'static str {
        match self {
            Self::Mode => "mode",
            Self::Compact => "compact",
            Self::Mini => "mini",
            Self::Tiny => "tiny",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ModeChoice {
    Auto,
    Mode,
    Compact,
    Mini,
    Tiny,
}

impl ModeChoice {
    fn next(self) -> Self {
        match self {
            Self::Auto => Self::Mode,
            Self::Mode => Self::Compact,
            Self::Compact => Self::Mini,
            Self::Mini => Self::Tiny,
            Self::Tiny => Self::Auto,
        }
    }

    fn forced(self) -> Option<ViewMode> {
        match self {
            Self::Auto => None,
            Self::Mode => Some(ViewMode::Mode),
            Self::Compact => Some(ViewMode::Compact),
            Self::Mini => Some(ViewMode::Mini),
            Self::Tiny => Some(ViewMode::Tiny),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scenario {
    Cooking,
    Throttle,
    Asleep,
}

impl Scenario {
    fn next(self) -> Self {
        match self {
            Self::Cooking => Self::Throttle,
            Self::Throttle => Self::Asleep,
            Self::Asleep => Self::Cooking,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Cooking => "cooking",
            Self::Throttle => "throttle",
            Self::Asleep => "asleep",
        }
    }
}

struct Gpu {
    label: &'static str,
    memory_pool: &'static str,
    memory_total_gib: f64,
    raw_activity: f64,
    shown_activity: f64,
    activity_velocity: f64,
    memory_percent: f64,
    hotspot_c: f64,
    power_w: f64,
    power_cap_w: f64,
    clock_mhz: f64,
    peak_activity: f64,
    activity_history: VecDeque<f64>,
    memory_history: VecDeque<f64>,
}

impl Gpu {
    fn new(label: &'static str, memory_pool: &'static str, memory_total_gib: f64) -> Self {
        Self {
            label,
            memory_pool,
            memory_total_gib,
            raw_activity: 0.0,
            shown_activity: 0.0,
            activity_velocity: 0.0,
            memory_percent: 0.0,
            hotspot_c: 0.0,
            power_w: 0.0,
            power_cap_w: 0.0,
            clock_mhz: 0.0,
            peak_activity: 0.0,
            activity_history: VecDeque::with_capacity(HISTORY_CAPACITY),
            memory_history: VecDeque::with_capacity(HISTORY_CAPACITY),
        }
    }

    fn push_sample(&mut self) {
        if self.activity_history.len() == HISTORY_CAPACITY {
            self.activity_history.pop_front();
        }
        if self.memory_history.len() == HISTORY_CAPACITY {
            self.memory_history.pop_front();
        }
        self.activity_history.push_back(self.raw_activity);
        self.memory_history.push_back(self.memory_percent);
        self.peak_activity = self.peak_activity.max(self.raw_activity);
    }
}

struct App {
    gpus: Vec<Gpu>,
    selected: usize,
    scenario: Scenario,
    mode_choice: ModeChoice,
    sample_number: u64,
    started: Instant,
    last_sample: Instant,
}

impl App {
    fn new() -> Self {
        let now = Instant::now();
        let mut app = Self {
            gpus: vec![
                Gpu::new("MI300X · 0", "HBM", 192.0),
                Gpu::new("7900 XTX · 1", "VRAM", 24.0),
                Gpu::new("780M · 2", "shared", 64.0),
            ],
            selected: 0,
            scenario: Scenario::Cooking,
            mode_choice: ModeChoice::Auto,
            sample_number: 0,
            started: now,
            last_sample: now,
        };
        app.sample();
        for gpu in &mut app.gpus {
            gpu.shown_activity = gpu.raw_activity;
        }
        app
    }

    fn sample(&mut self) {
        let t = self.sample_number as f64 * 0.25;
        for (index, gpu) in self.gpus.iter_mut().enumerate() {
            let phase = index as f64 * 1.7;
            match self.scenario {
                Scenario::Cooking => {
                    let base = [88.0, 66.0, 43.0][index];
                    gpu.raw_activity = (base
                        + 8.0 * (t * 0.72 + phase).sin()
                        + 4.0 * (t * 2.35 + phase * 0.4).sin())
                    .clamp(2.0, 99.0);
                    gpu.memory_percent = ([92.0, 81.0, 48.0][index]
                        + 1.2 * (t * 0.19 + phase).sin())
                    .clamp(1.0, 99.0);
                    gpu.hotspot_c = [79.0, 75.0, 67.0][index] + 2.0 * (t * 0.31 + phase).sin();
                    gpu.power_w = [544.0, 318.0, 34.0][index] + 12.0 * (t * 0.47 + phase).sin();
                    gpu.power_cap_w = [750.0, 355.0, 45.0][index];
                    gpu.clock_mhz =
                        [1710.0, 2440.0, 2680.0][index] + 45.0 * (t * 0.39 + phase).sin();
                }
                Scenario::Throttle => {
                    gpu.raw_activity = (95.0 + 3.0 * (t * 0.8 + phase).sin()).clamp(0.0, 100.0);
                    gpu.memory_percent = [94.0, 88.0, 62.0][index];
                    gpu.hotspot_c = [94.0, 96.0, 91.0][index];
                    gpu.power_cap_w = [750.0, 355.0, 45.0][index];
                    gpu.power_w = gpu.power_cap_w - [8.0, 2.0, 1.0][index];
                    gpu.clock_mhz = [1280.0, 1975.0, 2210.0][index];
                }
                Scenario::Asleep => {
                    gpu.raw_activity = 0.0;
                    gpu.memory_percent = 0.0;
                    gpu.hotspot_c = 0.0;
                    gpu.power_w = 0.0;
                    gpu.power_cap_w = [750.0, 355.0, 45.0][index];
                    gpu.clock_mhz = 0.0;
                }
            }
            gpu.push_sample();
        }
        self.sample_number += 1;
        self.last_sample = Instant::now();
    }

    fn animate(&mut self) {
        for gpu in &mut self.gpus {
            gpu.shown_activity = spring(
                gpu.shown_activity,
                gpu.raw_activity,
                &mut gpu.activity_velocity,
                FRAME_INTERVAL.as_secs_f64(),
            );
        }
    }

    fn select_previous(&mut self) {
        self.selected = (self.selected + self.gpus.len() - 1) % self.gpus.len();
    }

    fn select_next(&mut self) {
        self.selected = (self.selected + 1) % self.gpus.len();
    }

    fn graph_fraction(&self) -> f64 {
        (Instant::now()
            .saturating_duration_since(self.last_sample)
            .as_secs_f64()
            / SAMPLE_INTERVAL.as_secs_f64())
        .clamp(0.0, 1.0)
    }

    fn effective_mode(&self, area: Rect) -> ViewMode {
        let automatic = automatic_mode(area);
        let Some(forced) = self.mode_choice.forced() else {
            return automatic;
        };
        if mode_fits(forced, area) {
            forced
        } else {
            automatic
        }
    }
}

fn main() -> Result<()> {
    if !env::args().skip(1).any(|arg| arg == "--demo") {
        bail!("run the terminal prototype with: cargo run -- --demo");
    }
    run_terminal()
}

fn run_terminal() -> Result<()> {
    enable_raw_mode().context("enable terminal raw mode")?;
    let mut stdout = io::stdout();
    if let Err(error) = execute!(stdout, EnterAlternateScreen, Hide) {
        let _ = disable_raw_mode();
        return Err(error).context("enter alternate screen");
    }

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = match Terminal::new(backend) {
        Ok(terminal) => terminal,
        Err(error) => {
            let _ = execute!(io::stdout(), Show, LeaveAlternateScreen);
            let _ = disable_raw_mode();
            return Err(error).context("create terminal backend");
        }
    };

    let result = run(&mut terminal);
    let restore_result = restore_terminal(&mut terminal);
    result.and(restore_result)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    let raw_result = disable_raw_mode().context("disable terminal raw mode");
    let screen_result = execute!(terminal.backend_mut(), Show, LeaveAlternateScreen)
        .context("leave alternate screen");
    let cursor_result = terminal.show_cursor().context("show terminal cursor");
    raw_result.and(screen_result).and(cursor_result)
}

fn run(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    let mut app = App::new();
    let mut next_sample = Instant::now() + SAMPLE_INTERVAL;
    let mut next_frame = Instant::now();

    loop {
        let now = Instant::now();
        if now >= next_sample {
            app.sample();
            next_sample = now + SAMPLE_INTERVAL;
        }
        if now >= next_frame {
            app.animate();
            terminal.draw(|frame| render(frame, &app))?;
            next_frame = now + FRAME_INTERVAL;
        }

        let wait = next_frame
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(25));
        if !event::poll(wait)? {
            continue;
        }

        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Left | KeyCode::Char('h') => app.select_previous(),
                KeyCode::Right | KeyCode::Char('l') => app.select_next(),
                KeyCode::Char('m') => app.mode_choice = app.mode_choice.next(),
                KeyCode::Char('s') => {
                    app.scenario = app.scenario.next();
                    app.sample();
                }
                _ => {}
            },
            Event::Resize(_, _) => {
                next_frame = Instant::now();
            }
            _ => {}
        }
    }
    Ok(())
}

fn automatic_mode(area: Rect) -> ViewMode {
    if area.width >= 72 && area.height >= 34 {
        ViewMode::Mode
    } else if area.width >= 62 && area.height >= 17 {
        ViewMode::Compact
    } else if area.width >= 48 && area.height >= 11 {
        ViewMode::Mini
    } else {
        ViewMode::Tiny
    }
}

fn mode_fits(mode: ViewMode, area: Rect) -> bool {
    match mode {
        ViewMode::Mode => area.width >= 72 && area.height >= 34,
        ViewMode::Compact => area.width >= 62 && area.height >= 17,
        ViewMode::Mini => area.width >= 48 && area.height >= 11,
        ViewMode::Tiny => true,
    }
}

fn render(frame: &mut Frame<'_>, app: &App) {
    let area = frame.area();
    frame.render_widget(Block::default().style(Style::default().bg(BG)), area);
    let mode = app.effective_mode(area);
    match mode {
        ViewMode::Mode => render_mode(frame, app, centered(area, area.width.min(82), 32)),
        ViewMode::Compact => render_compact(frame, app, centered(area, area.width.min(74), 15)),
        ViewMode::Mini => render_mini(frame, app, centered(area, area.width.min(74), 9)),
        ViewMode::Tiny => render_tiny(frame, app, area),
    }
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width.min(area.width),
        height.min(area.height),
    )
}

fn render_mode(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(6),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(8),
            Constraint::Length(1),
            Constraint::Length(8),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);

    render_logo(frame, app, rows[0]);
    frame.render_widget(
        Paragraph::new("Power in, tokens out.")
            .alignment(Alignment::Center)
            .style(Style::default().fg(MUTED)),
        rows[1],
    );
    render_gpu_strip(frame, app, rows[3]);
    render_metric_panel(frame, app, rows[5], MetricKind::Activity, ViewMode::Mode);
    render_metric_panel(frame, app, rows[7], MetricKind::Memory, ViewMode::Mode);
    render_support(frame, app, rows[9]);
    render_health(frame, app, rows[10]);
    render_hints(frame, app, ViewMode::Mode, rows[12]);
}

fn render_compact(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);

    render_header(frame, app, rows[0]);
    render_gpu_strip(frame, app, rows[2]);
    render_metric_panel(frame, app, rows[4], MetricKind::Activity, ViewMode::Compact);
    render_metric_panel(frame, app, rows[6], MetricKind::Memory, ViewMode::Compact);
    render_support(frame, app, rows[8]);
    render_health(frame, app, rows[9]);
    render_hints(frame, app, ViewMode::Compact, rows[10]);
}

fn render_mini(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Length(1),
            Constraint::Length(4),
        ])
        .split(area);
    render_metric_panel(frame, app, rows[0], MetricKind::Activity, ViewMode::Mini);
    render_metric_panel(frame, app, rows[2], MetricKind::Memory, ViewMode::Mini);
}

fn render_tiny(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let gpu = &app.gpus[app.selected];
    let line = if app.scenario == Scenario::Asleep {
        Line::from(vec![
            Span::styled(
                "gruflo",
                Style::default().fg(CREAM).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  {}  ", gpu.label), Style::default().fg(MUTED)),
            Span::styled("GPU asleep", Style::default().fg(DIM)),
        ])
    } else {
        Line::from(vec![
            Span::styled(
                "gruflo",
                Style::default().fg(CREAM).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  {}  ", gpu.label), Style::default().fg(MUTED)),
            Span::styled(
                format!("activity {:>2.0}%", gpu.raw_activity),
                Style::default().fg(AMBER).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("   {} {:>2.0}%", gpu.memory_pool, gpu.memory_percent),
                Style::default().fg(CREAM),
            ),
        ])
    };
    let row = Rect::new(
        area.x,
        area.y + area.height.saturating_sub(1) / 2,
        area.width,
        1,
    );
    frame.render_widget(
        Paragraph::new(line)
            .alignment(Alignment::Center)
            .style(Style::default().bg(BG)),
        row,
    );
}

fn render_header(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let pulse = ((app.started.elapsed().as_secs_f64() * 2.4).sin() + 1.0) * 0.5;
    let dot = rgb_lerp(RUST, AMBER, pulse);
    let line = Line::from(vec![
        Span::styled("●", Style::default().fg(dot)),
        Span::raw("  "),
        Span::styled(
            "gruflo",
            Style::default().fg(CREAM).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ᦬", Style::default().fg(MUTED)),
    ]);
    frame.render_widget(Paragraph::new(line).alignment(Alignment::Center), area);
}

fn render_gpu_strip(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let available = area.width as usize;
    let mut labels: Vec<String> = app
        .gpus
        .iter()
        .map(|gpu| {
            let value = if app.scenario == Scenario::Asleep {
                "sleep".to_owned()
            } else {
                format!("{:>2.0}%", gpu.raw_activity)
            };
            format!(" {} {} ", gpu.label, value)
        })
        .collect();

    let mut start = app.selected.saturating_sub(1);
    let mut end = start;
    let mut used = 0;
    while end < labels.len() {
        let extra = labels[end].chars().count() + usize::from(end > start) * 2;
        if used + extra > available && end > app.selected {
            break;
        }
        used += extra;
        end += 1;
    }
    while app.selected >= end {
        start += 1;
        end = start;
        used = 0;
        while end < labels.len() {
            let extra = labels[end].chars().count() + usize::from(end > start) * 2;
            if used + extra > available && end > app.selected {
                break;
            }
            used += extra;
            end += 1;
        }
    }

    let mut spans = Vec::new();
    if start > 0 {
        spans.push(Span::styled("‹ ", Style::default().fg(DIM)));
    }
    for index in start..end {
        if index > start {
            spans.push(Span::styled("  ", Style::default().fg(DIM)));
        }
        let style = if index == app.selected {
            Style::default()
                .fg(BG)
                .bg(AMBER)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(MUTED)
        };
        spans.push(Span::styled(std::mem::take(&mut labels[index]), style));
    }
    if end < labels.len() {
        spans.push(Span::styled(" ›", Style::default().fg(DIM)));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).alignment(Alignment::Center),
        area,
    );
}

#[derive(Clone, Copy)]
enum MetricKind {
    Activity,
    Memory,
}

fn render_metric_panel(
    frame: &mut Frame<'_>,
    app: &App,
    area: Rect,
    kind: MetricKind,
    mode: ViewMode,
) {
    let gpu = &app.gpus[app.selected];
    let pulse = ((app.started.elapsed().as_secs_f64() * 2.4).sin() + 1.0) * 0.5;
    let (title, border, graph_color) = match kind {
        MetricKind::Activity => (" GPU activity ", rgb_lerp(RUST, AMBER, pulse), AMBER),
        MetricKind::Memory => (" Memory occupancy ", Color::Rgb(114, 91, 64), RUST),
    };
    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default().fg(CREAM).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border))
        .padding(Padding::horizontal(2));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if mode == ViewMode::Mini {
        let history = match kind {
            MetricKind::Activity => &gpu.activity_history,
            MetricKind::Memory => &gpu.memory_history,
        };
        render_graph(
            frame,
            history,
            100.0,
            app.graph_fraction(),
            graph_color,
            inner,
        );
        return;
    }

    let (left, right) = match kind {
        MetricKind::Activity if app.scenario == Scenario::Asleep => {
            ("asleep".to_owned(), "peak: —".to_owned())
        }
        MetricKind::Activity => (
            format!("{:>3.0}%  {}", gpu.shown_activity, trend(gpu)),
            format!("peak: {:.0}%", gpu.peak_activity),
        ),
        MetricKind::Memory if app.scenario == Scenario::Asleep => (
            format!("{} unavailable", gpu.memory_pool),
            "GPU asleep".to_owned(),
        ),
        MetricKind::Memory => {
            let used = gpu.memory_total_gib * gpu.memory_percent / 100.0;
            (
                format!(
                    "{used:.1} / {:.0} GiB  ·  {:.0}%",
                    gpu.memory_total_gib, gpu.memory_percent
                ),
                gpu.memory_pool.to_owned(),
            )
        }
    };
    render_value_row(frame, inner, &left, &right, graph_color);

    if mode == ViewMode::Mode && inner.height > 2 {
        let graph_area = Rect::new(inner.x, inner.y + 2, inner.width, inner.height - 2);
        let history = match kind {
            MetricKind::Activity => &gpu.activity_history,
            MetricKind::Memory => &gpu.memory_history,
        };
        render_graph(
            frame,
            history,
            100.0,
            app.graph_fraction(),
            graph_color,
            graph_area,
        );
    }
}

fn render_value_row(frame: &mut Frame<'_>, area: Rect, left: &str, right: &str, accent: Color) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .split(Rect::new(area.x, area.y, area.width, 1));
    frame.render_widget(
        Paragraph::new(Line::styled(
            left.to_owned(),
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        )),
        columns[0],
    );
    frame.render_widget(
        Paragraph::new(Line::styled(right.to_owned(), Style::default().fg(MUTED)))
            .alignment(Alignment::Right),
        columns[1],
    );
}

fn render_graph(
    frame: &mut Frame<'_>,
    history: &VecDeque<f64>,
    max_value: f64,
    fraction: f64,
    color: Color,
    area: Rect,
) {
    let lines = braille_graph(
        history,
        area.width as usize,
        area.height as usize,
        max_value,
        fraction,
    )
    .into_iter()
    .enumerate()
    .map(|(row, text)| {
        let fade = 1.0 - row as f64 / area.height.max(1) as f64;
        Line::styled(
            text,
            Style::default().fg(rgb_lerp(DIM, color, 0.55 + fade * 0.45)),
        )
    })
    .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_support(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let gpu = &app.gpus[app.selected];
    let line = if app.scenario == Scenario::Asleep {
        Line::styled(
            "hotspot —   power —   GFX clock —",
            Style::default().fg(DIM),
        )
    } else {
        Line::from(vec![
            Span::styled(
                format!("hotspot {:.0} / 95°C", gpu.hotspot_c),
                Style::default().fg(CREAM),
            ),
            Span::styled("   ·   ", Style::default().fg(DIM)),
            Span::styled(
                format!("power {:.0} / {:.0} W", gpu.power_w, gpu.power_cap_w),
                Style::default().fg(CREAM),
            ),
            Span::styled("   ·   ", Style::default().fg(DIM)),
            Span::styled(
                format!("GFX {:.0} MHz", gpu.clock_mhz),
                Style::default().fg(CREAM),
            ),
        ])
    };
    frame.render_widget(Paragraph::new(line).alignment(Alignment::Center), area);
}

fn render_health(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let gpu = &app.gpus[app.selected];
    let (marker, text, color) = match app.scenario {
        Scenario::Cooking => ("●", "no active limits or faults".to_owned(), MUTED),
        Scenario::Throttle => (
            "!",
            format!("thermal throttle · hotspot {:.0} / 95°C", gpu.hotspot_c),
            RED,
        ),
        Scenario::Asleep => ("○", "GPU asleep".to_owned(), DIM),
    };
    let line = Line::from(vec![
        Span::styled(
            marker,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(text, Style::default().fg(color)),
    ]);
    frame.render_widget(Paragraph::new(line).alignment(Alignment::Center), area);
}

fn render_logo(frame: &mut Frame<'_>, app: &App, area: Rect) {
    const LOGO: [&str; 6] = [
        " ██████╗ ██████╗ ██╗   ██╗███████╗██╗      ██████╗ ",
        "██╔════╝ ██╔══██╗██║   ██║██╔════╝██║     ██╔═══██╗",
        "██║  ███╗██████╔╝██║   ██║█████╗  ██║     ██║   ██║",
        "██║   ██║██╔══██╗██║   ██║██╔══╝  ██║     ██║   ██║",
        "╚██████╔╝██║  ██║╚██████╔╝██║     ███████╗╚██████╔╝",
        " ╚═════╝ ╚═╝  ╚═╝ ╚═════╝ ╚═╝     ╚══════╝ ╚═════╝ ",
    ];
    let breath = ((app.started.elapsed().as_secs_f64() * 1.6).sin() + 1.0) * 0.5;
    let lines = LOGO
        .iter()
        .enumerate()
        .map(|(row, text)| {
            let vertical = row as f64 / (LOGO.len() - 1) as f64;
            let color = if vertical < 0.5 {
                rgb_lerp(CREAM, AMBER, vertical * 2.0)
            } else {
                rgb_lerp(AMBER, RUST, (vertical - 0.5) * 2.0)
            };
            Line::styled(
                *text,
                Style::default()
                    .fg(rgb_lerp(DIM, color, 0.82 + breath * 0.18))
                    .add_modifier(Modifier::BOLD),
            )
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), area);
}

fn render_hints(frame: &mut Frame<'_>, app: &App, mode: ViewMode, area: Rect) {
    let actual = mode.name();
    let line = Line::from(vec![
        key_hint("←/→", "GPU"),
        Span::styled("  ·  ", Style::default().fg(DIM)),
        key_hint("s", app.scenario.name()),
        Span::styled("  ·  ", Style::default().fg(DIM)),
        key_hint("m", actual),
        Span::styled("  ·  ", Style::default().fg(DIM)),
        key_hint("q", "quit"),
    ]);
    frame.render_widget(Paragraph::new(line).alignment(Alignment::Center), area);
}

fn key_hint(key: &'static str, description: &'static str) -> Span<'static> {
    Span::styled(format!("{key} {description}"), Style::default().fg(MUTED))
}

fn trend(gpu: &Gpu) -> &'static str {
    if gpu.activity_history.len() < 6 {
        return "→";
    }
    let start = gpu.activity_history[gpu.activity_history.len() - 6];
    let delta = gpu.raw_activity - start;
    if delta > 4.0 {
        "↗"
    } else if delta < -4.0 {
        "↘"
    } else {
        "→"
    }
}

fn spring(current: f64, target: f64, velocity: &mut f64, dt: f64) -> f64 {
    let force = 180.0 * (target - current);
    *velocity += force * dt;
    *velocity *= (-12.0 * dt).exp();
    if velocity.abs() < 0.0001 {
        *velocity = 0.0;
    }
    current + *velocity * dt
}

fn rgb_lerp(from: Color, to: Color, amount: f64) -> Color {
    let (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) = (from, to) else {
        return to;
    };
    let t = amount.clamp(0.0, 1.0);
    Color::Rgb(
        (r1 as f64 + (r2 as f64 - r1 as f64) * t).round() as u8,
        (g1 as f64 + (g2 as f64 - g1 as f64) * t).round() as u8,
        (b1 as f64 + (b2 as f64 - b1 as f64) * t).round() as u8,
    )
}

fn braille_graph(
    samples: &VecDeque<f64>,
    width: usize,
    height: usize,
    max_value: f64,
    fraction: f64,
) -> Vec<String> {
    if width == 0 || height == 0 {
        return Vec::new();
    }
    let dots_x = width * 2;
    let dots_y = height * 4;
    let mut grid = vec![vec!['\u{2800}'; width]; height];
    let mut values = vec![0.0; dots_x];

    for (x, value) in values.iter_mut().enumerate() {
        let index = samples.len() as f64 - dots_x as f64 + x as f64 + fraction;
        *value = sample_at(samples, index);
    }

    let mut smoothed = vec![0.0; dots_x];
    for (x, output) in smoothed.iter_mut().enumerate() {
        let mut sum = 0.0;
        let mut weights = 0.0;
        for offset in -2..=2 {
            let index = x as isize + offset;
            if !(0..dots_x as isize).contains(&index) {
                continue;
            }
            let weight = match offset {
                0 => 3.0,
                -1 | 1 => 2.0,
                _ => 1.0,
            };
            sum += values[index as usize] * weight;
            weights += weight;
        }
        *output = sum / weights;
    }

    for (x, value) in smoothed.into_iter().enumerate() {
        let ratio = (value / max_value).clamp(0.0, 1.0);
        let height_dots = ratio * (2.0 - ratio) * dots_y as f64;
        let column = x / 2;
        let dx = x % 2;
        for y_dot in 0..dots_y {
            if y_dot as f64 >= height_dots {
                break;
            }
            let row = height - 1 - y_dot / 4;
            let dy = 3 - y_dot % 4;
            let mask = match (dx, dy) {
                (0, 0) => 1 << 0,
                (0, 1) => 1 << 1,
                (0, 2) => 1 << 2,
                (0, 3) => 1 << 6,
                (1, 0) => 1 << 3,
                (1, 1) => 1 << 4,
                (1, 2) => 1 << 5,
                (1, 3) => 1 << 7,
                _ => 0,
            };
            grid[row][column] =
                char::from_u32(grid[row][column] as u32 | mask).unwrap_or('\u{2800}');
        }
    }

    grid.into_iter()
        .map(|line| line.into_iter().collect())
        .collect()
}

fn sample_at(samples: &VecDeque<f64>, index: f64) -> f64 {
    if samples.is_empty() || index < 0.0 {
        return 0.0;
    }
    if index >= samples.len().saturating_sub(1) as f64 {
        return *samples.back().unwrap_or(&0.0);
    }
    let lower = index.floor() as usize;
    let fraction = index - lower as f64;
    let first = samples[lower];
    let second = samples[lower + 1];
    first * (1.0 - fraction) + second * fraction
}
