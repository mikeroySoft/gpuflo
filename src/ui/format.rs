//! Pure presentation math and text: spring animation, braille packing over
//! observation gaps, trend arrows, color interpolation, IEC byte counts, and
//! canonical unavailable-state text. No function here mutates canonical data.

use ratatui::style::Color;

use crate::model::{ObservationState, Timestamp, state_phrase};
use crate::source::Reading;

/// Critically-damped-ish spring toward `target`; velocity state lives in the
/// UI only. Non-finite inputs are absorbed: the result is always finite and
/// the velocity is reset rather than propagating NaN into later frames.
pub(super) fn spring(current: f64, target: f64, velocity: &mut f64, dt: f64) -> f64 {
    if !velocity.is_finite() {
        *velocity = 0.0;
    }
    let current = if current.is_finite() {
        current
    } else {
        *velocity = 0.0;
        if target.is_finite() { target } else { 0.0 }
    };
    if !target.is_finite() || !dt.is_finite() || dt <= 0.0 {
        *velocity = 0.0;
        return current;
    }
    let force = 180.0 * (target - current);
    *velocity += force * dt;
    *velocity *= (-12.0 * dt).exp();
    if velocity.abs() < 0.0001 {
        *velocity = 0.0;
    }
    current + *velocity * dt
}

/// Linearly interpolated history sample at a fractional index. `None` slots
/// are observation gaps: interpolation never crosses one, and a fractional
/// position adjacent to a gap is itself a gap.
pub(super) fn sample_at(samples: &[Option<f64>], index: f64) -> Option<f64> {
    if samples.is_empty() || index < 0.0 {
        return None;
    }
    let last = samples.len() - 1;
    if index >= last as f64 {
        return samples[last].filter(|value| value.is_finite());
    }
    let lower = index.floor() as usize;
    let fraction = index - lower as f64;
    let first = samples[lower].filter(|value| value.is_finite());
    let second = samples[lower + 1].filter(|value| value.is_finite());
    match (first, second) {
        (Some(a), Some(b)) => Some(a * (1.0 - fraction) + b * fraction),
        (Some(a), _) if fraction == 0.0 => Some(a),
        _ => None,
    }
}

/// Renders the trailing window of `samples` as a 2×4-dot braille area chart,
/// filled from the bottom. `fraction` (0..1 of one production tick) slides
/// the window for sub-cell interpolation. A gap slot renders as an empty
/// braille column and is never interpolated across.
pub(super) fn braille_graph(
    samples: &[Option<f64>],
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

    let mut values: Vec<Option<f64>> = vec![None; dots_x];
    for (x, value) in values.iter_mut().enumerate() {
        let index = samples.len() as f64 - dots_x as f64 + x as f64 + fraction;
        *value = sample_at(samples, index);
    }

    // Weighted smoothing over present neighbors only; a gap column stays a
    // gap and never borrows neighboring values.
    let mut smoothed: Vec<Option<f64>> = vec![None; dots_x];
    for (x, output) in smoothed.iter_mut().enumerate() {
        if values[x].is_none() {
            continue;
        }
        let mut sum = 0.0;
        let mut weights = 0.0;
        for offset in -2_isize..=2 {
            let index = x as isize + offset;
            if !(0..dots_x as isize).contains(&index) {
                continue;
            }
            let Some(value) = values[index as usize] else {
                continue;
            };
            let weight = match offset {
                0 => 3.0,
                -1 | 1 => 2.0,
                _ => 1.0,
            };
            sum += value * weight;
            weights += weight;
        }
        *output = Some(sum / weights);
    }

    for (x, value) in smoothed.into_iter().enumerate() {
        let Some(value) = value else { continue };
        let ratio = if max_value > 0.0 {
            (value / max_value).clamp(0.0, 1.0)
        } else {
            0.0
        };
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

/// Rising, steady, or falling arrow comparing `current` against the fresh
/// observation six production ticks back. A gap yields the steady arrow.
pub(super) fn trend(history: &[Option<f64>], current: f64) -> &'static str {
    if history.len() < 6 {
        return "→";
    }
    let Some(start) = history[history.len() - 6] else {
        return "→";
    };
    let delta = current - start;
    if delta > 4.0 {
        "↗"
    } else if delta < -4.0 {
        "↘"
    } else {
        "→"
    }
}

/// Linear RGB interpolation; non-RGB endpoints resolve to `to`, and a
/// non-finite `amount` saturates instead of propagating NaN.
pub(super) fn rgb_lerp(from: Color, to: Color, amount: f64) -> Color {
    let (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) = (from, to) else {
        return to;
    };
    let t = if amount.is_finite() {
        amount.clamp(0.0, 1.0)
    } else {
        1.0
    };
    Color::Rgb(
        (r1 as f64 + (r2 as f64 - r1 as f64) * t).round() as u8,
        (g1 as f64 + (g2 as f64 - g1 as f64) * t).round() as u8,
        (b1 as f64 + (b2 as f64 - b1 as f64) * t).round() as u8,
    )
}

/// IEC byte count with one decimal, e.g. `1.5 GiB`, `512.0 MiB`, `640 B`.
pub(super) fn iec_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Canonical phrase for an unavailable observation. A stale observation
/// renders its age against snapshot assembly time as whole-plus-tenths
/// seconds, e.g. `stale 4.2s`, matching the non-interactive outputs.
pub(super) fn unavailable_phrase(
    state: &ObservationState,
    observed_at: Option<Timestamp>,
    sampled_at: Timestamp,
) -> String {
    if *state == ObservationState::STALE
        && let Some(last_good) = observed_at
    {
        let age = (sampled_at.as_odt() - last_good.as_odt())
            .as_seconds_f64()
            .max(0.0);
        return format!("stale {age:.1}s");
    }
    state_phrase(state).to_owned()
}

/// Exact evidence phrase for a process-source reading without a value.
pub(super) fn reading_phrase<T>(reading: &Reading<T>) -> &'static str {
    match reading {
        Reading::Value(_) => "",
        Reading::Absent => "absent",
        Reading::Sentinel => "unavailable",
        Reading::Asleep => "asleep",
        Reading::PermissionDenied => "permission denied",
        Reading::UnsupportedDriver => "unsupported driver version",
        Reading::Malformed => "malformed",
        Reading::Error => "source error",
    }
}

/// Human label for a memory pool string: known pools get their display case,
/// unknown pools pass through as-is.
pub(super) fn pool_label(pool: &str) -> &str {
    match pool {
        "vram" => "VRAM",
        "shared" => "shared",
        "gtt" => "GTT",
        other => other,
    }
}
