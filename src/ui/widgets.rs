//! Frame composition: responsive surfaces, instrument panels, braille
//! graphs, the GPU overview strip, and the process/detail/help overlays.
//!
//! Every renderer projects the adopted [`RenderModel`]; unavailable
//! observations render compact markers here while detail and help expose the
//! exact canonical state phrase. Overlay failures never alter health.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph};

use crate::model::{Observation, ObservationState, PhysicalGpu, Timestamp};
use crate::source::Reading;
use crate::source::process::ProcessRow;
use crate::state::{ProcessOverlay, RenderModel};

use super::format;
use super::layout::{self, Surface};
use super::theme::Styler;
use super::{UiState, primary_partition};

/// Approved six-row block logo, verbatim from the prototype.
const LOGO: [&str; 6] = [
    " ██████╗ ██████╗ ██╗   ██╗███████╗██╗      ██████╗ ",
    "██╔════╝ ██╔══██╗██║   ██║██╔════╝██║     ██╔═══██╗",
    "██║  ███╗██████╔╝██║   ██║█████╗  ██║     ██║   ██║",
    "██║   ██║██╔══██╗██║   ██║██╔══╝  ██║     ██║   ██║",
    "╚██████╔╝██║  ██║╚██████╔╝██║     ███████╗╚██████╔╝",
    " ╚═════╝ ╚═╝  ╚═╝ ╚═════╝ ╚═╝     ╚══════╝ ╚═════╝ ",
];

/// Approved tagline, exact.
const TAGLINE: &str = "Power in, tokens out.";

/// Centered line shown while no AMD GPU is discoverable.
const NO_GPU: &str = "no AMD GPU currently detected";

/// Explicit selection marker; survives no-color mode textually.
const SELECTED_MARKER: &str = "▶";

/// Everything the surface renderers need for the selected GPU.
struct View<'a> {
    state: &'a UiState,
    styler: Styler,
    model: &'a RenderModel,
    gpu: &'a PhysicalGpu,
    activity_history: &'a [Option<f64>],
    memory_history: &'a [Option<f64>],
    peak: Option<f64>,
    surface: Surface,
}

/// Renders one full frame from session state. Pure projection: no canonical
/// data is created, defaulted, or mutated here.
pub(super) fn draw(frame: &mut Frame<'_>, state: &UiState) {
    let styler = Styler::new(state.theme, state.color_enabled);
    let area = frame.area();
    frame.render_widget(Block::default().style(styler.background()), area);

    let surface = layout::effective(state.mode_preference, area);
    let model = state
        .model
        .as_ref()
        .filter(|model| !model.snapshot.gpus.is_empty());
    match model {
        None => {
            render_no_gpu(frame, &styler, area);
            if state.show_help {
                render_help(frame, &styler, state, surface, area);
            }
        }
        Some(model) => {
            let index = state.selected_index().unwrap_or(0);
            let gpu = &model.snapshot.gpus[index];
            let render = model.gpus.iter().find(|render| render.id == gpu.id);
            let view = View {
                state,
                styler,
                model,
                gpu,
                activity_history: render
                    .map_or(&[][..], |render| render.activity_history.as_slice()),
                memory_history: render.map_or(&[][..], |render| render.memory_history.as_slice()),
                peak: render.and_then(|render| render.session_peak_activity),
                surface,
            };
            match surface {
                Surface::Mode => {
                    render_mode(frame, &view, layout::centered(area, area.width.min(82), 32));
                }
                Surface::Compact => {
                    render_compact(frame, &view, layout::centered(area, area.width.min(74), 15));
                }
                Surface::Mini => {
                    render_mini(frame, &view, layout::centered(area, area.width.min(74), 9));
                }
                Surface::Tiny => render_tiny(frame, &view, area),
            }
            if state.show_help {
                render_help(frame, &styler, state, surface, area);
            } else if state.show_processes {
                render_processes(frame, &view, area);
            } else if state.show_detail {
                render_detail(frame, &view, area);
            }
        }
    }

    if let Some(text) = state.notice_line() {
        render_notice(frame, &styler, area, text);
    }
}

// ---------------------------------------------------------------------------
// Surfaces
// ---------------------------------------------------------------------------

fn render_mode(frame: &mut Frame<'_>, view: &View<'_>, area: Rect) {
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

    render_logo(frame, view, rows[0]);
    frame.render_widget(
        Paragraph::new(TAGLINE)
            .alignment(Alignment::Center)
            .style(view.styler.fg(view.styler.palette.muted)),
        rows[1],
    );
    render_strip(frame, view, rows[3]);
    render_metric_panel(frame, view, rows[5], Metric::Activity);
    render_metric_panel(frame, view, rows[7], Metric::Memory);
    render_support(frame, view, rows[9]);
    render_health(frame, view, rows[10]);
    render_hints(frame, view, rows[12]);
}

fn render_compact(frame: &mut Frame<'_>, view: &View<'_>, area: Rect) {
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

    render_header(frame, view, rows[0]);
    render_strip(frame, view, rows[2]);
    render_metric_panel(frame, view, rows[4], Metric::Activity);
    render_metric_panel(frame, view, rows[6], Metric::Memory);
    render_support(frame, view, rows[8]);
    render_health(frame, view, rows[9]);
    render_hints(frame, view, rows[10]);
}

fn render_mini(frame: &mut Frame<'_>, view: &View<'_>, area: Rect) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Length(1),
            Constraint::Length(4),
        ])
        .split(area);
    render_metric_panel(frame, view, rows[0], Metric::Activity);
    render_metric_panel(frame, view, rows[2], Metric::Memory);
}

fn render_tiny(frame: &mut Frame<'_>, view: &View<'_>, area: Rect) {
    let styler = &view.styler;
    let palette = styler.palette;
    let gpu = view.gpu;
    let sampled_at = view.model.snapshot.sampled_at;
    let label = format!("{} · {}", short_name(&gpu.name), gpu.index);

    let mut spans = vec![
        Span::styled("gruflo", styler.bold(palette.fg)),
        Span::styled(format!("  {label}  "), styler.fg(palette.muted)),
    ];
    match primary_partition(gpu) {
        Some(partition) => match &partition.activity_percent {
            Observation::Value { value, .. } => {
                spans.push(Span::styled(
                    format!("activity {value:>2.0}%"),
                    styler.bold(palette.accent),
                ));
                let memory = &partition.memory;
                let pool = format::pool_label(memory.pool.as_str());
                let memory_text = match memory.occupancy_percent.current() {
                    Some(occupancy) => format!("   {pool} {occupancy:>2.0}%"),
                    None => format!("   {pool} —"),
                };
                spans.push(Span::styled(memory_text, styler.fg(palette.fg)));
            }
            Observation::Unavailable { state, observed_at } => {
                let text = if *state == ObservationState::ASLEEP {
                    "GPU asleep".to_owned()
                } else {
                    format::unavailable_phrase(state, *observed_at, sampled_at)
                };
                spans.push(Span::styled(text, styler.fg(palette.dim)));
            }
        },
        None => spans.push(Span::styled("—", styler.fg(palette.dim))),
    }

    let row = Rect::new(
        area.x,
        area.y + area.height.saturating_sub(1) / 2,
        area.width,
        1,
    );
    frame.render_widget(
        Paragraph::new(Line::from(spans)).alignment(Alignment::Center),
        row,
    );
}

fn render_no_gpu(frame: &mut Frame<'_>, styler: &Styler, area: Rect) {
    let row = Rect::new(
        area.x,
        area.y + area.height.saturating_sub(1) / 2,
        area.width,
        1,
    );
    frame.render_widget(
        Paragraph::new(Line::styled(NO_GPU, styler.fg(styler.palette.dim)))
            .alignment(Alignment::Center),
        row,
    );
}

// ---------------------------------------------------------------------------
// Rows and panels
// ---------------------------------------------------------------------------

fn render_logo(frame: &mut Frame<'_>, view: &View<'_>, area: Rect) {
    let styler = &view.styler;
    let palette = styler.palette;
    let breath = ((view.state.started.elapsed().as_secs_f64() * 1.6).sin() + 1.0) * 0.5;
    let lines = LOGO
        .iter()
        .enumerate()
        .map(|(row, text)| {
            let vertical = row as f64 / (LOGO.len() - 1) as f64;
            let color = if vertical < 0.5 {
                format::rgb_lerp(palette.fg, palette.accent, vertical * 2.0)
            } else {
                format::rgb_lerp(palette.accent, palette.warning, (vertical - 0.5) * 2.0)
            };
            Line::styled(
                *text,
                styler.bold(format::rgb_lerp(palette.dim, color, 0.82 + breath * 0.18)),
            )
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), area);
}

fn render_header(frame: &mut Frame<'_>, view: &View<'_>, area: Rect) {
    let styler = &view.styler;
    let palette = styler.palette;
    let pulse = ((view.state.started.elapsed().as_secs_f64() * 2.4).sin() + 1.0) * 0.5;
    let line = Line::from(vec![
        Span::styled(
            "●",
            styler.fg(format::rgb_lerp(palette.warning, palette.accent, pulse)),
        ),
        Span::raw("  "),
        Span::styled("gruflo", styler.bold(palette.fg)),
        Span::styled("  ᦬", styler.fg(palette.muted)),
    ]);
    frame.render_widget(Paragraph::new(line).alignment(Alignment::Center), area);
}

/// Scrolling overview strip: every physical GPU with an explicit `▶` marker
/// and highlight on the selection; `‹`/`›` mark clipped neighbors.
fn render_strip(frame: &mut Frame<'_>, view: &View<'_>, area: Rect) {
    let styler = &view.styler;
    let palette = styler.palette;
    let gpus = &view.model.snapshot.gpus;
    let selected = gpus
        .iter()
        .position(|gpu| gpu.id == view.gpu.id)
        .unwrap_or(0);
    let available = area.width as usize;

    let mut labels: Vec<String> = gpus
        .iter()
        .enumerate()
        .map(|(index, gpu)| {
            let marker = if index == selected {
                SELECTED_MARKER
            } else {
                ""
            };
            let space = if index == selected { " " } else { "" };
            format!(
                " {marker}{space}{} · {} {} ",
                short_name(&gpu.name),
                gpu.index,
                strip_value(gpu)
            )
        })
        .collect();

    let window = |start: usize| -> usize {
        let mut end = start;
        let mut used = 0;
        while end < labels.len() {
            let extra = labels[end].chars().count() + usize::from(end > start) * 2;
            if used + extra > available && end > selected {
                break;
            }
            used += extra;
            end += 1;
        }
        end
    };
    let mut start = selected.saturating_sub(1);
    let mut end = window(start);
    while selected >= end {
        start += 1;
        end = window(start);
    }

    let mut spans = Vec::new();
    if start > 0 {
        spans.push(Span::styled("‹ ", styler.fg(palette.dim)));
    }
    for (index, label) in labels.iter_mut().enumerate().take(end).skip(start) {
        if index > start {
            spans.push(Span::styled("  ", styler.fg(palette.dim)));
        }
        let style = if index == selected {
            styler.selected()
        } else {
            styler.fg(palette.muted)
        };
        spans.push(Span::styled(std::mem::take(label), style));
    }
    if end < labels.len() {
        spans.push(Span::styled(" ›", styler.fg(palette.dim)));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).alignment(Alignment::Center),
        area,
    );
}

/// Strip value: fresh activity, `sleep` for an asleep GPU, `—` otherwise.
fn strip_value(gpu: &PhysicalGpu) -> String {
    match primary_partition(gpu).map(|partition| &partition.activity_percent) {
        Some(Observation::Value { value, .. }) => format!("{value:>2.0}%"),
        Some(Observation::Unavailable { state, .. }) if *state == ObservationState::ASLEEP => {
            "sleep".to_owned()
        }
        _ => "—".to_owned(),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Metric {
    Activity,
    Memory,
}

fn render_metric_panel(frame: &mut Frame<'_>, view: &View<'_>, area: Rect, metric: Metric) {
    let styler = &view.styler;
    let palette = styler.palette;
    let pulse = ((view.state.started.elapsed().as_secs_f64() * 2.4).sin() + 1.0) * 0.5;
    let (title, border_color, graph_color) = match metric {
        Metric::Activity => (
            " GPU activity ",
            format::rgb_lerp(palette.warning, palette.accent, pulse),
            palette.graph,
        ),
        Metric::Memory => (" Memory occupancy ", palette.border, palette.warning),
    };
    let block = Block::default()
        .title(Span::styled(title, styler.bold(palette.fg)))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(styler.fg(border_color))
        .padding(Padding::horizontal(2));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let history = match metric {
        Metric::Activity => view.activity_history,
        Metric::Memory => view.memory_history,
    };

    if view.surface == Surface::Mini {
        render_graph(frame, view, history, graph_color, inner);
        return;
    }

    let (left, left_style, right, right_style) = match metric {
        Metric::Activity => activity_row(view),
        Metric::Memory => memory_row(view),
    };
    render_value_row(frame, inner, &left, left_style, &right, right_style);

    if view.surface == Surface::Mode && inner.height > 2 {
        let graph_area = Rect::new(inner.x, inner.y + 2, inner.width, inner.height - 2);
        render_graph(frame, view, history, graph_color, graph_area);
    }
}

/// Activity value row: spring-smoothed displayed value with a trend arrow,
/// session peak on the right. Missing data names its state, never zero.
fn activity_row(view: &View<'_>) -> (String, Style, String, Style) {
    let styler = &view.styler;
    let palette = styler.palette;
    let right = match view.peak {
        Some(peak) => format!("peak: {peak:.0}%"),
        None => "peak: —".to_owned(),
    };
    let observation = primary_partition(view.gpu).map(|partition| &partition.activity_percent);
    let (left, left_style) = match observation {
        Some(Observation::Value { value, .. }) => {
            let shown = view.state.shown_activity(&view.gpu.id).unwrap_or(*value);
            (
                format!(
                    "{shown:>3.0}%  {}",
                    format::trend(view.activity_history, *value)
                ),
                styler.bold(palette.accent),
            )
        }
        Some(Observation::Unavailable { state, .. }) if *state == ObservationState::ASLEEP => {
            ("asleep".to_owned(), styler.fg(palette.dim))
        }
        _ => ("—".to_owned(), styler.fg(palette.dim)),
    };
    (left, left_style, right, styler.fg(palette.muted))
}

/// Memory value row: used/total plus occupancy with the pool name. Partial
/// availability renders the fresh pieces; nothing fresh names the state.
fn memory_row(view: &View<'_>) -> (String, Style, String, Style) {
    let styler = &view.styler;
    let palette = styler.palette;
    let Some(memory) = primary_partition(view.gpu).map(|partition| &partition.memory) else {
        return (
            "—".to_owned(),
            styler.fg(palette.dim),
            String::new(),
            styler.fg(palette.muted),
        );
    };
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    let pool = format::pool_label(memory.pool.as_str());
    let mut left = String::new();
    if let (Some(used), Some(total)) = (memory.used_bytes.current(), memory.total_bytes.current()) {
        left = format!("{:.1} / {:.0} GiB", *used as f64 / GIB, *total as f64 / GIB);
    }
    if let Some(occupancy) = memory.occupancy_percent.current() {
        if !left.is_empty() {
            left.push_str("  ·  ");
        }
        left.push_str(&format!("{occupancy:.0}%"));
    }
    if !left.is_empty() {
        return (
            left,
            styler.bold(palette.warning),
            pool.to_owned(),
            styler.fg(palette.muted),
        );
    }
    let state = memory
        .occupancy_percent
        .state()
        .or_else(|| memory.used_bytes.state())
        .or_else(|| memory.total_bytes.state());
    if state == Some(&ObservationState::ASLEEP) {
        (
            format!("{pool} unavailable"),
            styler.fg(palette.dim),
            "GPU asleep".to_owned(),
            styler.fg(palette.dim),
        )
    } else {
        (
            format!("{pool} —"),
            styler.fg(palette.dim),
            String::new(),
            styler.fg(palette.dim),
        )
    }
}

fn render_value_row(
    frame: &mut Frame<'_>,
    area: Rect,
    left: &str,
    left_style: Style,
    right: &str,
    right_style: Style,
) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .split(Rect::new(area.x, area.y, area.width, 1));
    frame.render_widget(
        Paragraph::new(Line::styled(left.to_owned(), left_style)),
        columns[0],
    );
    frame.render_widget(
        Paragraph::new(Line::styled(right.to_owned(), right_style)).alignment(Alignment::Right),
        columns[1],
    );
}

fn render_graph(
    frame: &mut Frame<'_>,
    view: &View<'_>,
    history: &[Option<f64>],
    color: ratatui::style::Color,
    area: Rect,
) {
    let styler = &view.styler;
    let lines = format::braille_graph(
        history,
        area.width as usize,
        area.height as usize,
        100.0,
        view.state.graph_fraction(),
    )
    .into_iter()
    .enumerate()
    .map(|(row, text)| {
        let fade = 1.0 - row as f64 / area.height.max(1) as f64;
        Line::styled(
            text,
            styler.fg(format::rgb_lerp(
                styler.palette.dim,
                color,
                0.55 + fade * 0.45,
            )),
        )
    })
    .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines), area);
}

/// Support row: hotspot against its limit, power against its cap, GFX clock.
/// Each unavailable segment renders a dim marker; detail keeps exact states.
fn render_support(frame: &mut Frame<'_>, view: &View<'_>, area: Rect) {
    let styler = &view.styler;
    let palette = styler.palette;
    let gpu = view.gpu;
    let partition = primary_partition(gpu);

    let hotspot = gpu.temperature.hotspot_celsius.current().map(|hotspot| {
        match gpu.temperature.limit_celsius.current() {
            Some(limit) => format!("hotspot {hotspot:.0} / {limit:.0}°C"),
            None => format!("hotspot {hotspot:.0}°C"),
        }
    });
    let power = gpu
        .power
        .socket_watts
        .current()
        .map(|watts| match gpu.power.cap_watts.current() {
            Some(cap) => format!("power {watts:.0} / {cap:.0} W"),
            None => format!("power {watts:.0} W"),
        });
    let clock = partition
        .and_then(|partition| partition.gfx_clock_mhz.current())
        .map(|mhz| format!("GFX {mhz:.0} MHz"));

    let segments = [(hotspot, "hotspot —"), (power, "power —"), (clock, "GFX —")];
    let mut spans = Vec::new();
    for (index, (text, fallback)) in segments.into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled("   ·   ", styler.fg(palette.dim)));
        }
        match text {
            Some(text) => spans.push(Span::styled(text, styler.fg(palette.fg))),
            None => spans.push(Span::styled(fallback, styler.fg(palette.dim))),
        }
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).alignment(Alignment::Center),
        area,
    );
}

/// One factual health sentence with its severity marker: `●` normal muted,
/// `!` fault/throttle/limit in fault color, `○` telemetry dim.
fn render_health(frame: &mut Frame<'_>, view: &View<'_>, area: Rect) {
    let styler = &view.styler;
    let palette = styler.palette;
    let health = &view.gpu.health;
    let (marker, color) = match health.category.as_str() {
        "fault" | "throttle" | "limit" => ("!", palette.fault),
        "telemetry" => ("○", palette.dim),
        "memory_pressure" => ("!", palette.warning),
        _ => ("●", palette.muted),
    };
    let line = Line::from(vec![
        Span::styled(marker, styler.bold(color)),
        Span::raw("  "),
        Span::styled(health.message.clone(), styler.fg(color)),
    ]);
    frame.render_widget(Paragraph::new(line).alignment(Alignment::Center), area);
}

fn render_hints(frame: &mut Frame<'_>, view: &View<'_>, area: Rect) {
    let styler = &view.styler;
    let palette = styler.palette;
    let hints = [
        "←/→ GPU".to_owned(),
        format!("t {}", view.state.theme.name()),
        format!("m {}", view.surface.name()),
        "p processes".to_owned(),
        "d detail".to_owned(),
        "? help".to_owned(),
        "q quit".to_owned(),
    ];
    let mut spans = Vec::new();
    for (index, hint) in hints.into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled(" · ", styler.fg(palette.dim)));
        }
        spans.push(Span::styled(hint, styler.fg(palette.muted)));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).alignment(Alignment::Center),
        area,
    );
}

fn render_notice(frame: &mut Frame<'_>, styler: &Styler, area: Rect, text: &str) {
    if area.height == 0 {
        return;
    }
    let row = Rect::new(area.x, area.y, area.width, 1);
    frame.render_widget(
        Paragraph::new(Line::styled(
            text.to_owned(),
            styler.fg(styler.palette.warning),
        ))
        .alignment(Alignment::Center),
        row,
    );
}

// ---------------------------------------------------------------------------
// Overlays
// ---------------------------------------------------------------------------

/// Clears and frames a centered overlay box, returning its inner area.
fn overlay_box(
    frame: &mut Frame<'_>,
    styler: &Styler,
    area: Rect,
    width: u16,
    height: u16,
    title: &'static str,
) -> Rect {
    let rect = layout::centered(area, width.min(area.width), height.min(area.height));
    frame.render_widget(Clear, rect);
    let block = Block::default()
        .title(Span::styled(title, styler.bold(styler.palette.fg)))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(styler.fg(styler.palette.border))
        .style(styler.background())
        .padding(Padding::horizontal(2));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    inner
}

/// Process overlay: honest attribution only. fdinfo and KFD memory are two
/// unreconciled accounting systems; rows keep the provided sort order.
fn render_processes(frame: &mut Frame<'_>, view: &View<'_>, area: Rect) {
    let styler = &view.styler;
    let palette = styler.palette;
    let mut lines: Vec<Line<'_>> = Vec::new();
    match &view.model.processes {
        None => lines.push(Line::styled("scanning processes…", styler.fg(palette.dim))),
        Some(overlay) if overlay.rows.is_empty() => lines.push(Line::styled(
            "no attributable processes",
            styler.fg(palette.dim),
        )),
        Some(overlay) => {
            lines.push(Line::styled(
                format!(
                    "{:>7}  {:<16}  {:<8}  {:>10}  {:>16}  {}",
                    "PID", "name", "GPU", "VRAM", "KFD", "container"
                ),
                styler.fg(palette.muted),
            ));
            for row in &overlay.rows {
                lines.push(Line::styled(
                    process_line(row, overlay, view.model),
                    styler.fg(palette.fg),
                ));
            }
        }
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "attributed memory only · the kernel exposes no per-process GPU utilization",
        styler.fg(palette.dim),
    ));

    let height = (lines.len() as u16).saturating_add(2);
    let inner = overlay_box(frame, styler, area, 86, height, " processes ");
    frame.render_widget(Paragraph::new(lines), inner);
}

fn process_line(row: &ProcessRow, overlay: &ProcessOverlay, model: &RenderModel) -> String {
    let name = match &row.name {
        Reading::Value(name) => clip(name, 16),
        other => format::reading_phrase(other).to_owned(),
    };
    let gpu = row
        .bdf
        .as_ref()
        .and_then(|bdf| overlay.gpu_by_bdf.get(bdf))
        .and_then(|id| model.snapshot.gpus.iter().find(|gpu| gpu.id == *id))
        .map(|gpu| format!("GPU {}", gpu.index))
        .unwrap_or_else(|| "unknown".to_owned());
    let vram = memory_cell(&row.fdinfo_vram_bytes);
    let kfd = format!("KFD {}", memory_cell(&row.kfd_vram_bytes));
    let container = row
        .container
        .as_deref()
        .map(|container| clip(container, 12))
        .unwrap_or_else(|| "—".to_owned());
    format!(
        "{:>7}  {name:<16}  {gpu:<8}  {vram:>10}  {kfd:>16}  {container}",
        row.pid
    )
}

/// IEC memory cell: value, `—` for structural absence, or the exact
/// evidence phrase.
fn memory_cell(reading: &Reading<u64>) -> String {
    match reading {
        Reading::Value(bytes) => format::iec_bytes(*bytes),
        Reading::Absent => "—".to_owned(),
        other => format::reading_phrase(other).to_owned(),
    }
}

/// Detail overlay: selected-GPU secondary data with the exact canonical
/// state phrase for every unavailable metric.
fn render_detail(frame: &mut Frame<'_>, view: &View<'_>, area: Rect) {
    let styler = &view.styler;
    let gpu = view.gpu;
    let sampled_at = view.model.snapshot.sampled_at;
    let mut lines = Vec::new();

    lines.push(detail_line(
        styler,
        "gpu",
        format!("{} · GPU {}", gpu.name, gpu.index),
        false,
    ));
    lines.push(detail_line(styler, "id", gpu.id.to_string(), false));
    lines.push(detail_line(styler, "bdf", gpu.bdf.to_string(), false));
    if let Some(uuid) = &gpu.uuid {
        lines.push(detail_line(styler, "uuid", uuid.clone(), false));
    }
    if let Some(serial) = &gpu.serial {
        lines.push(detail_line(styler, "serial", serial.clone(), false));
    }
    lines.push(Line::raw(""));

    let push_f64 = |lines: &mut Vec<Line<'static>>,
                    label: &'static str,
                    observation: &Observation<f64>,
                    unit: &'static str| {
        let (text, unavailable) = f64_or_phrase(observation, unit, sampled_at);
        lines.push(detail_line(styler, label, text, unavailable));
    };

    if let Some(partition) = primary_partition(gpu) {
        push_f64(&mut lines, "activity", &partition.activity_percent, "%");
        lines.push(detail_line(
            styler,
            "pool",
            format::pool_label(partition.memory.pool.as_str()).to_owned(),
            false,
        ));
        let (used, used_missing) = bytes_or_phrase(&partition.memory.used_bytes, sampled_at);
        lines.push(detail_line(styler, "memory used", used, used_missing));
        let (total, total_missing) = bytes_or_phrase(&partition.memory.total_bytes, sampled_at);
        lines.push(detail_line(styler, "memory total", total, total_missing));
        push_f64(
            &mut lines,
            "occupancy",
            &partition.memory.occupancy_percent,
            "%",
        );
        push_f64(&mut lines, "GFX clock", &partition.gfx_clock_mhz, " MHz");
        push_f64(
            &mut lines,
            "memory-controller",
            &partition.memory_controller_activity_percent,
            "%",
        );
    }
    push_f64(
        &mut lines,
        "hotspot",
        &gpu.temperature.hotspot_celsius,
        "°C",
    );
    push_f64(
        &mut lines,
        "hotspot limit",
        &gpu.temperature.limit_celsius,
        "°C",
    );
    push_f64(&mut lines, "power", &gpu.power.socket_watts, " W");
    push_f64(&mut lines, "power cap", &gpu.power.cap_watts, " W");

    let height = (lines.len() as u16).saturating_add(2);
    let inner = overlay_box(frame, styler, area, 64, height, " detail ");
    frame.render_widget(Paragraph::new(lines), inner);
}

fn detail_line(styler: &Styler, label: &'static str, value: String, dimmed: bool) -> Line<'static> {
    let palette = styler.palette;
    let value_style = if dimmed {
        styler.fg(palette.dim)
    } else {
        styler.fg(palette.fg)
    };
    Line::from(vec![
        Span::styled(format!("{label:<20}"), styler.fg(palette.muted)),
        Span::styled(value, value_style),
    ])
}

/// Value with unit, or the exact canonical phrase and a dim flag.
fn f64_or_phrase(
    observation: &Observation<f64>,
    unit: &str,
    sampled_at: Timestamp,
) -> (String, bool) {
    match observation {
        Observation::Value { value, .. } => (format!("{value:.0}{unit}"), false),
        Observation::Unavailable { state, observed_at } => (
            format::unavailable_phrase(state, *observed_at, sampled_at),
            true,
        ),
    }
}

fn bytes_or_phrase(observation: &Observation<u64>, sampled_at: Timestamp) -> (String, bool) {
    match observation {
        Observation::Value { value, .. } => (format::iec_bytes(*value), false),
        Observation::Unavailable { state, observed_at } => (
            format::unavailable_phrase(state, *observed_at, sampled_at),
            true,
        ),
    }
}

/// Help overlay: input map, active theme, preferred versus effective
/// surface, and the attribution limitation note.
fn render_help(
    frame: &mut Frame<'_>,
    styler: &Styler,
    state: &UiState,
    surface: Surface,
    area: Rect,
) {
    let palette = styler.palette;
    let keys = [
        ("←/→ · h/l", "select GPU"),
        ("t", "cycle theme (session only)"),
        ("m", "cycle surface preference (session only)"),
        ("p", "toggle process overlay"),
        ("d", "toggle detail overlay"),
        ("?", "toggle this help"),
        ("q · Esc", "quit"),
    ];
    let mut lines: Vec<Line<'_>> = keys
        .into_iter()
        .map(|(key, action)| {
            Line::from(vec![
                Span::styled(format!("{key:<12}"), styler.fg(palette.muted)),
                Span::styled(action, styler.fg(palette.fg)),
            ])
        })
        .collect();
    lines.push(Line::raw(""));
    lines.push(Line::from(vec![
        Span::styled(format!("{:<12}", "theme"), styler.fg(palette.muted)),
        Span::styled(state.theme.name(), styler.fg(palette.fg)),
    ]));
    lines.push(Line::from(vec![
        Span::styled(format!("{:<12}", "surface"), styler.fg(palette.muted)),
        Span::styled(
            format!(
                "preferred {} · effective {}",
                state.mode_preference.name(),
                surface.name()
            ),
            styler.fg(palette.fg),
        ),
    ]));
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "process memory attribution only; no per-process utilization",
        styler.fg(palette.dim),
    ));

    let height = (lines.len() as u16).saturating_add(2);
    let inner = overlay_box(frame, styler, area, 66, height, " help ");
    frame.render_widget(Paragraph::new(lines), inner);
}

// ---------------------------------------------------------------------------
// Text helpers
// ---------------------------------------------------------------------------

/// Compact model label: known marketing prefixes removed, capped for the
/// overview strip.
fn short_name(name: &str) -> String {
    let mut trimmed = name.trim();
    loop {
        let mut stripped = false;
        for prefix in ["AMD ", "Radeon ", "Instinct "] {
            if let Some(rest) = trimmed.strip_prefix(prefix) {
                trimmed = rest.trim_start();
                stripped = true;
            }
        }
        if !stripped {
            break;
        }
    }
    if trimmed.is_empty() {
        trimmed = name.trim();
    }
    clip(trimmed, 16)
}

/// Truncates to `max` characters with a trailing ellipsis.
fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}
