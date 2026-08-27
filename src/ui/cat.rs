//! Optional decorative surface: an ASCII cat napping on a warm GPU.
//!
//! Strictly presentation. It reads temperature and activity only to decide
//! whether to draw, never renders when doing so would touch the centered
//! instrument box or the transient notice row, and contributes nothing to
//! health, telemetry, or output. It settles into a different blank corner
//! every so often, deterministically per process so a frame redraw never
//! makes it jump.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::model::PhysicalGpu;

use super::primary_partition;
use super::theme::Styler;

/// Hotspot temperature at or above which the GPU counts as warm. Below this
/// — or with no temperature reading at all — nonzero activity counts as
/// warm instead.
const WARM_HOTSPOT_CELSIUS: f64 = 40.0;

/// How long each "z" breath frame holds, in seconds.
const BREATH_SECONDS: f64 = 1.2;

/// How long the cat stays in one spot before picking a new one.
const RESETTLE_SECONDS: f64 = 150.0;

/// Whether the selected GPU is warm enough for the cat to curl up on.
pub(super) fn gpu_is_warm(gpu: &PhysicalGpu) -> bool {
    if let Some(hotspot) = gpu.temperature.hotspot_celsius.current() {
        return *hotspot >= WARM_HOTSPOT_CELSIUS;
    }
    primary_partition(gpu)
        .and_then(|partition| partition.activity_percent.current())
        .is_some_and(|activity| *activity > 0.0)
}

/// The curled-cat sprite, with the "z"s breathing through three lengths.
fn sprite(breath: usize) -> [String; 3] {
    let zzz = ["z", "zz", "zzz"][breath % 3];
    [
        format!("　　　　  {zzz}"),
        "  γ ⌒ヽﾊ,,ﾊ".to_owned(),
        "（\"_）(-ｪ-,,）".to_owned(),
    ]
}

/// Cell footprint of the widest breath frame, so the chosen spot never
/// shifts as the "z"s lengthen.
fn footprint() -> (u16, u16) {
    let lines = sprite(2);
    let width = lines.iter().map(|line| line.width()).max().unwrap_or(0) as u16;
    (width, lines.len() as u16)
}

/// The sprite as styled lines: drifting `z`s dim, the curled outline muted,
/// and the closed eyes in the theme's accent color. Every color is a theme
/// role, so it collapses cleanly under `--no-color`.
fn styled_lines(styler: &Styler, breath: usize) -> Vec<Line<'static>> {
    let [zzz_line, ears_line, face_line] = sprite(breath);
    let dim = styler.fg(styler.palette.dim);
    let muted = styler.fg(styler.palette.muted);
    let warm = styler.fg(styler.palette.accent);

    let face = match face_line.split_once("-ｪ-") {
        Some((prefix, suffix)) => Line::from(vec![
            Span::styled(prefix.to_owned(), dim),
            Span::styled("-ｪ-".to_owned(), warm),
            Span::styled(suffix.to_owned(), dim),
        ]),
        None => Line::styled(face_line, dim),
    };

    vec![
        Line::styled(zzz_line, dim),
        Line::styled(ears_line, muted),
        face,
    ]
}

/// One blank-space resting spot, anchored to a corner or side margin.
#[derive(Debug, Clone, Copy)]
enum Spot {
    TopLeft,
    TopRight,
    MidLeft,
    MidRight,
    BottomLeft,
    BottomRight,
}

const SPOTS: [Spot; 6] = [
    Spot::TopLeft,
    Spot::TopRight,
    Spot::MidLeft,
    Spot::MidRight,
    Spot::BottomLeft,
    Spot::BottomRight,
];

impl Spot {
    /// This spot's rect for a `width`x`height` sprite in `area`, keeping the
    /// very top row clear for the transient notice line. `None` when it
    /// doesn't fit at all.
    fn rect(self, area: Rect, width: u16, height: u16) -> Option<Rect> {
        let top = area.y.saturating_add(1);
        let usable_height = area.height.saturating_sub(1);
        if area.width < width || usable_height < height {
            return None;
        }
        let x = match self {
            Self::TopLeft | Self::MidLeft | Self::BottomLeft => area.x,
            Self::TopRight | Self::MidRight | Self::BottomRight => area.x + area.width - width,
        };
        let y = match self {
            Self::TopLeft | Self::TopRight => top,
            Self::MidLeft | Self::MidRight => top + (usable_height - height) / 2,
            Self::BottomLeft | Self::BottomRight => area.y + area.height - height,
        };
        Some(Rect::new(x, y, width, height))
    }
}

/// A deterministic index into `SPOTS`, stable for `RESETTLE_SECONDS` at a
/// time and distinct per process so concurrent instances don't overlap.
fn chosen_start(elapsed_seconds: f64) -> usize {
    let bucket = (elapsed_seconds / RESETTLE_SECONDS) as u64;
    let mut hasher = DefaultHasher::new();
    (std::process::id(), bucket).hash(&mut hasher);
    (hasher.finish() as usize) % SPOTS.len()
}

/// Draws the sprite in whichever blank spot is currently chosen, skipping
/// entirely — trying every other spot first — when none is free of
/// `reserved` (the centered instrument box). Nothing is ever clipped.
pub(super) fn render(
    frame: &mut Frame<'_>,
    styler: &Styler,
    area: Rect,
    reserved: Rect,
    elapsed_seconds: f64,
) {
    let breath = (elapsed_seconds / BREATH_SECONDS) as usize;
    let (width, height) = footprint();
    let start = chosen_start(elapsed_seconds);
    let rect = (0..SPOTS.len())
        .filter_map(|offset| SPOTS[(start + offset) % SPOTS.len()].rect(area, width, height))
        .find(|rect| !rect.intersects(reserved));
    let Some(rect) = rect else {
        return;
    };
    frame.render_widget(Paragraph::new(styled_lines(styler, breath)), rect);
}
