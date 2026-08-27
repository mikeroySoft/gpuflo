//! In-crate UI tests over `TestBackend`: representative frames per surface,
//! a bounded size sweep, the no-color contract, braille gap packing, spring
//! stability, selection identity, and overlay honesty.

use std::collections::HashMap;

use crossterm::event::KeyCode;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};
use time::macros::datetime;

use crate::cli::GpuSelector;
use crate::config::{ModePreference, PresentationOptions, Theme};
use crate::model::{
    Health, HealthCategory, Memory, MemoryPool, Observation, ObservationState, Partition,
    PartitionId, PciBdf, PhysicalGpu, PhysicalGpuId, Power, Snapshot, Temperature, Timestamp,
};
use crate::source::Reading;
use crate::source::process::ProcessRow;
use crate::state::{ProcessOverlay, RenderGpu, RenderModel};

use super::layout::{self, Surface};
use super::{Action, FRAME_INTERVAL, UiState, format, widgets};

const GIB: u64 = 1024 * 1024 * 1024;

/// Snapshot assembly time shared by every fixture observation.
fn sampled_at() -> Timestamp {
    Timestamp::from_odt(datetime!(2026-08-21 12:00:00.000 UTC))
}

fn value_f(value: f64) -> Observation<f64> {
    Observation::value(value, sampled_at())
}

fn value_u(value: u64) -> Observation<u64> {
    Observation::value(value, sampled_at())
}

fn asleep_f() -> Observation<f64> {
    Observation::unavailable(ObservationState::ASLEEP)
}

/// Healthy discrete GPU: every observation fresh.
fn healthy_gpu() -> PhysicalGpu {
    PhysicalGpu {
        id: PhysicalGpuId::new("gpu-a"),
        index: 0,
        bdf: PciBdf::parse("0000:03:00.0").expect("bdf"),
        name: "AMD Instinct MI300X".to_owned(),
        uuid: Some("uuid-a".to_owned()),
        serial: Some("serial-a".to_owned()),
        health: Health {
            category: HealthCategory::NONE,
            message: "no active limits or faults".to_owned(),
            observed_at: sampled_at(),
        },
        temperature: Temperature {
            hotspot_celsius: value_f(74.0),
            limit_celsius: value_f(95.0),
        },
        power: Power {
            socket_watts: value_f(544.0),
            cap_watts: value_f(750.0),
        },
        partitions: vec![Partition {
            id: PartitionId::new("gpu-a-p0"),
            index: 0,
            is_primary: true,
            activity_percent: value_f(88.0),
            memory: Memory {
                pool: MemoryPool::VRAM,
                used_bytes: value_u(182 * GIB),
                total_bytes: value_u(192 * GIB),
                occupancy_percent: value_f(95.0),
            },
            gfx_clock_mhz: value_f(1710.0),
            memory_controller_activity_percent: value_f(41.0),
        }],
    }
}

/// GPU with unavailable observations: asleep, stale (4.2 s old), and
/// unsupported hardware, under a telemetry health condition.
fn sleepy_gpu() -> PhysicalGpu {
    let last_good = Timestamp::from_odt(datetime!(2026-08-21 11:59:55.800 UTC));
    PhysicalGpu {
        id: PhysicalGpuId::new("gpu-b"),
        index: 1,
        bdf: PciBdf::parse("0000:c3:00.0").expect("bdf"),
        name: "AMD Radeon RX 7900 XTX".to_owned(),
        uuid: None,
        serial: None,
        health: Health {
            category: HealthCategory::TELEMETRY,
            message: "GPU asleep".to_owned(),
            observed_at: sampled_at(),
        },
        temperature: Temperature {
            hotspot_celsius: Observation::stale(last_good),
            limit_celsius: asleep_f(),
        },
        power: Power {
            socket_watts: asleep_f(),
            cap_watts: value_f(355.0),
        },
        partitions: vec![Partition {
            id: PartitionId::new("gpu-b-p0"),
            index: 0,
            is_primary: true,
            activity_percent: asleep_f(),
            memory: Memory {
                pool: MemoryPool::VRAM,
                used_bytes: Observation::unavailable(ObservationState::ASLEEP),
                total_bytes: value_u(24 * GIB),
                occupancy_percent: asleep_f(),
            },
            gfx_clock_mhz: Observation::stale(last_good),
            memory_controller_activity_percent: Observation::unavailable(
                ObservationState::UNSUPPORTED_HARDWARE,
            ),
        }],
    }
}

/// Two-GPU render model: fresh histories with one gap for gpu-a, all-gap
/// histories for gpu-b.
fn model() -> RenderModel {
    let mut activity_history = vec![Some(88.0); 240];
    activity_history[200] = None;
    activity_history[201] = None;
    RenderModel {
        snapshot: Snapshot::new(sampled_at(), None, vec![healthy_gpu(), sleepy_gpu()]),
        gpus: vec![
            RenderGpu {
                id: PhysicalGpuId::new("gpu-a"),
                activity_history,
                memory_history: vec![Some(95.0); 240],
                session_peak_activity: Some(97.0),
            },
            RenderGpu {
                id: PhysicalGpuId::new("gpu-b"),
                activity_history: vec![None; 240],
                memory_history: vec![None; 240],
                session_peak_activity: None,
            },
        ],
        processes: None,
    }
}

fn state_with_model(color_enabled: bool) -> UiState {
    let mut state = UiState::new(
        PresentationOptions {
            theme: Theme::Buffalo,
            mode_preference: ModePreference::Auto,
            color_enabled,
            truecolor: true,
        },
        None,
    );
    let _ = state.adopt_model(model());
    state
}

fn render(state: &UiState, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal
        .draw(|frame| widgets::draw(frame, state))
        .expect("draw");
    terminal.backend().buffer().clone()
}

fn buffer_text(buffer: &Buffer) -> String {
    let mut text = String::new();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            text.push_str(buffer.cell((x, y)).map_or(" ", |cell| cell.symbol()));
        }
        text.push('\n');
    }
    text
}

// ---------------------------------------------------------------------------
// Representative frames per surface
// ---------------------------------------------------------------------------

#[test]
fn mode_frame_shows_logo_tagline_health_and_selection() {
    let state = state_with_model(true);
    let text = buffer_text(&render(&state, 120, 40));
    assert!(
        text.contains("██████╗ ██████╗ ██╗   ██╗███████╗██╗      ██████╗"),
        "logo row missing"
    );
    assert!(
        text.contains("██║   ██║██╔═══╝ ██║   ██║██╔══╝"),
        "GPUFLO P glyph missing"
    );
    assert!(text.contains(state.tagline), "selected tagline missing");
    assert!(
        !text.contains("no active limits or faults"),
        "normal health must leave the main health row empty"
    );
    assert!(text.contains("▶"), "selection marker missing");
    assert!(text.contains("MI300X"), "selected GPU label missing");
    assert!(text.contains("peak: 97%"), "session peak missing");
    assert!(
        text.contains("hotspot 74 / 95°C"),
        "support row missing: {text}"
    );
}

#[test]
fn launch_tagline_remains_stable_across_frames() {
    let mut state = state_with_model(true);
    let launch_tagline = state.tagline;
    let first = buffer_text(&render(&state, 120, 40));
    state.animate(FRAME_INTERVAL.as_secs_f64());
    let second = buffer_text(&render(&state, 120, 40));

    assert_eq!(state.tagline, launch_tagline);
    assert!(first.contains(launch_tagline));
    assert!(second.contains(launch_tagline));
}

#[test]
fn compact_frame_shows_panels_health_and_selection() {
    let state = state_with_model(true);
    let text = buffer_text(&render(&state, 80, 24));
    assert!(text.contains("GPU activity"));
    assert!(text.contains("Memory occupancy"));
    assert!(!text.contains("no active limits or faults"));
    assert!(text.contains("▶"));
    assert!(!text.contains("██████╗"), "compact must omit the logo");
}

#[test]
fn mini_frame_shows_graph_panels_only() {
    let state = state_with_model(true);
    let text = buffer_text(&render(&state, 60, 16));
    assert!(text.contains("GPU activity"));
    assert!(text.contains("Memory occupancy"));
    assert!(
        !text.contains("no active limits or faults"),
        "mini must omit the health row"
    );
}

#[test]
fn narrow_frame_falls_to_tiny() {
    let state = state_with_model(true);
    let text = buffer_text(&render(&state, 40, 8));
    assert!(text.contains("gpuflo"));
    assert!(text.contains("activity 88%"));
}

#[test]
fn one_line_terminal_renders_tiny() {
    let state = state_with_model(true);
    let text = buffer_text(&render(&state, 20, 1));
    assert!(text.contains("gpuflo"));
}

#[test]
fn tiny_names_the_state_when_asleep() {
    let mut state = state_with_model(true);
    let _ = state.handle_key(KeyCode::Right); // gpu-b
    let text = buffer_text(&render(&state, 40, 8));
    assert!(text.contains("GPU asleep"), "asleep text missing: {text}");
    assert!(!text.contains(" 0%"), "missing data must never render 0");
}

#[test]
fn empty_model_keeps_running_with_message() {
    let mut state = state_with_model(true);
    let mut empty = model();
    empty.snapshot.gpus.clear();
    empty.gpus.clear();
    let _ = state.adopt_model(empty);
    let text = buffer_text(&render(&state, 80, 24));
    assert!(text.contains("no AMD GPU currently detected"));
}

// ---------------------------------------------------------------------------
// Bounded size sweep
// ---------------------------------------------------------------------------

#[test]
fn every_bounded_size_renders_without_panic() {
    let mut state = state_with_model(true);
    for width in 40..=90u16 {
        for height in 8..=40u16 {
            // Alternate selection and overlays to sweep both GPUs' paths.
            let _ = state.handle_key(KeyCode::Right);
            state.show_detail = (width + height) % 3 == 0;
            state.show_help = (width + height) % 5 == 0;
            state.show_processes = (width + height) % 7 == 0;
            state.animate(0.125);
            render(&state, width, height);
        }
    }
}

// ---------------------------------------------------------------------------
// No-color contract
// ---------------------------------------------------------------------------

#[test]
fn no_color_keeps_distinctions_without_any_styling() {
    let state = state_with_model(false);
    let buffer = render(&state, 120, 40);
    let text = buffer_text(&buffer);
    assert!(text.contains("▶"), "selection must survive textually");
    assert!(!text.contains("no active limits or faults"));
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            let cell = buffer.cell((x, y)).expect("cell");
            assert_eq!(cell.fg, Color::Reset, "fg styled at {x},{y}");
            assert_eq!(cell.bg, Color::Reset, "bg styled at {x},{y}");
            assert_eq!(
                cell.modifier,
                Modifier::empty(),
                "emphasis styled at {x},{y}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Braille packing
// ---------------------------------------------------------------------------

#[test]
fn braille_packs_bottom_up_and_keeps_gaps_blank() {
    // Full-scale values around a two-dot gap covering one whole cell.
    let mut history = vec![Some(100.0); 8];
    history[4] = None;
    history[5] = None;
    let lines = format::braille_graph(&history, 4, 2, 100.0, 0.0);
    assert_eq!(lines, vec!["⣿⣿⠀⣿".to_owned(), "⣿⣿⠀⣿".to_owned()]);

    // Low values fill from the bottom; the top row stays empty.
    let low = vec![Some(25.0); 8];
    let lines = format::braille_graph(&low, 4, 2, 100.0, 0.0);
    assert_eq!(lines, vec!["⠀⠀⠀⠀".to_owned(), "⣿⣿⣿⣿".to_owned()]);

    // Rendered buffer cells carry the same packing.
    let mut terminal = Terminal::new(TestBackend::new(4, 2)).expect("terminal");
    terminal
        .draw(|frame| {
            let lines: Vec<ratatui::text::Line<'_>> =
                format::braille_graph(&history, 4, 2, 100.0, 0.0)
                    .into_iter()
                    .map(ratatui::text::Line::raw)
                    .collect();
            frame.render_widget(ratatui::widgets::Paragraph::new(lines), frame.area());
        })
        .expect("draw");
    let buffer = terminal.backend().buffer();
    assert_eq!(buffer.cell((1, 0)).expect("cell").symbol(), "⣿");
    assert_eq!(buffer.cell((2, 0)).expect("cell").symbol(), "\u{2800}");
    assert_eq!(buffer.cell((2, 1)).expect("cell").symbol(), "\u{2800}");
    assert_eq!(buffer.cell((3, 1)).expect("cell").symbol(), "⣿");
}

#[test]
fn sample_at_never_interpolates_across_gaps() {
    let samples = [Some(10.0), None, Some(30.0)];
    assert_eq!(format::sample_at(&samples, 0.0), Some(10.0));
    assert_eq!(format::sample_at(&samples, 0.5), None);
    assert_eq!(format::sample_at(&samples, 1.0), None);
    assert_eq!(format::sample_at(&samples, 2.0), Some(30.0));
}

// ---------------------------------------------------------------------------
// Spring
// ---------------------------------------------------------------------------

#[test]
fn spring_survives_non_finite_inputs() {
    let mut velocity = 0.0;
    let shown = format::spring(f64::NAN, 50.0, &mut velocity, 0.125);
    assert!(shown.is_finite() && velocity.is_finite());

    let mut velocity = 0.0;
    let shown = format::spring(10.0, f64::INFINITY, &mut velocity, 0.125);
    assert_eq!(shown, 10.0);
    assert_eq!(velocity, 0.0);

    let mut velocity = f64::NAN;
    let shown = format::spring(10.0, f64::NEG_INFINITY, &mut velocity, 0.125);
    assert!(shown.is_finite());
    assert_eq!(velocity, 0.0);

    let mut velocity = 0.0;
    let shown = format::spring(f64::INFINITY, f64::NAN, &mut velocity, 0.125);
    assert!(shown.is_finite());

    let mut velocity = 0.0;
    let shown = format::spring(40.0, 60.0, &mut velocity, f64::NAN);
    assert_eq!(shown, 40.0);
}

#[test]
fn spring_converges_to_target() {
    let mut shown = 0.0;
    let mut velocity = 0.0;
    for _ in 0..400 {
        shown = format::spring(shown, 100.0, &mut velocity, 0.125);
        assert!(shown.is_finite(), "spring diverged: {shown}");
    }
    assert!((shown - 100.0).abs() < 0.5, "did not converge: {shown}");
}

// ---------------------------------------------------------------------------
// Surface policy
// ---------------------------------------------------------------------------

#[test]
fn forced_surface_falls_back_to_richest_fitting() {
    let small = Rect::new(0, 0, 70, 20); // fits compact, not mode
    assert_eq!(
        layout::effective(ModePreference::Mode, small),
        Surface::Compact
    );
    assert_eq!(
        layout::effective(ModePreference::Tiny, small),
        Surface::Tiny
    );
    let large = Rect::new(0, 0, 120, 40);
    assert_eq!(
        layout::effective(ModePreference::Auto, large),
        Surface::Mode
    );
    assert_eq!(
        layout::effective(ModePreference::Mini, large),
        Surface::Mini
    );
}

// ---------------------------------------------------------------------------
// Selection and keys
// ---------------------------------------------------------------------------

#[test]
fn selection_tracks_identity_and_restores_returning_gpu() {
    let mut state = state_with_model(true);
    let _ = state.handle_key(KeyCode::Right);
    assert_eq!(state.selected.as_ref().expect("selected").as_str(), "gpu-b");

    // gpu-b disappears: fall back to the nearest survivor.
    let mut shrunk = model();
    shrunk.snapshot.gpus.remove(1);
    shrunk.gpus.remove(1);
    let _ = state.adopt_model(shrunk);
    assert_eq!(state.selected.as_ref().expect("selected").as_str(), "gpu-a");

    // gpu-b returns: the remembered selection is restored.
    let _ = state.adopt_model(model());
    assert_eq!(state.selected.as_ref().expect("selected").as_str(), "gpu-b");
}

#[test]
fn initial_gpu_selector_resolves_against_first_model() {
    let mut state = UiState::new(PresentationOptions::default(), Some(GpuSelector::Index(1)));
    let _ = state.adopt_model(model());
    assert_eq!(state.selected.as_ref().expect("selected").as_str(), "gpu-b");

    let mut state = UiState::new(
        PresentationOptions::default(),
        Some(GpuSelector::Id("gpu-a".to_owned())),
    );
    let _ = state.adopt_model(model());
    assert_eq!(state.selected.as_ref().expect("selected").as_str(), "gpu-a");
}

#[test]
fn process_key_scopes_to_the_selected_gpu() {
    let mut state = state_with_model(true);
    assert_eq!(
        state.handle_key(KeyCode::Char('p')),
        Action::SetProcessScope(Some(PhysicalGpuId::new("gpu-a")))
    );
    // Selection change while open retargets the scope.
    assert_eq!(
        state.handle_key(KeyCode::Right),
        Action::SetProcessScope(Some(PhysicalGpuId::new("gpu-b")))
    );
    assert_eq!(
        state.handle_key(KeyCode::Esc),
        Action::SetProcessScope(None)
    );
}

#[test]
fn keys_cycle_session_presentation_and_quit() {
    let mut state = state_with_model(true);
    assert_eq!(state.handle_key(KeyCode::Char('t')), Action::None);
    assert_eq!(state.theme, Theme::Nord);
    let before = layout::effective(state.mode_preference, Rect::new(0, 0, 120, 40));
    assert_eq!(state.handle_key(KeyCode::Char('m')), Action::None);
    assert_eq!(state.mode_preference, ModePreference::Compact);
    assert_ne!(
        before,
        layout::effective(state.mode_preference, Rect::new(0, 0, 120, 40))
    );
    assert_eq!(state.handle_key(KeyCode::Char('q')), Action::Quit);
    assert_eq!(state.handle_key(KeyCode::Esc), Action::Quit);
}

#[test]
fn escape_closes_topmost_modal_before_quitting() {
    let mut state = state_with_model(true);
    assert_eq!(state.handle_key(KeyCode::Char('d')), Action::None);
    assert_eq!(
        state.handle_key(KeyCode::Char('p')),
        Action::SetProcessScope(Some(PhysicalGpuId::new("gpu-a")))
    );
    assert_eq!(state.handle_key(KeyCode::Char('?')), Action::None);
    assert_eq!(state.handle_key(KeyCode::Esc), Action::None);
    assert!(!state.show_help && state.show_processes && state.show_detail);
    assert_eq!(
        state.handle_key(KeyCode::Esc),
        Action::SetProcessScope(None)
    );
    assert!(!state.show_processes && state.show_detail);
    assert_eq!(state.handle_key(KeyCode::Esc), Action::None);
    assert!(!state.show_detail);
    assert_eq!(state.handle_key(KeyCode::Esc), Action::Quit);
}

// ---------------------------------------------------------------------------
// Overlays
// ---------------------------------------------------------------------------

#[test]
fn process_columns_share_available_width_evenly() {
    let widths = widgets::process_column_widths(106);
    assert_eq!(widths.iter().sum::<usize>() + 12, 106);
    let data = &widths[2..];
    assert!(data.iter().max().unwrap() - data.iter().min().unwrap() <= 1);
    assert_eq!(widths[0], 7);
    assert_eq!(widths[1], 17);
}

#[test]
fn process_overlay_lists_honest_attribution() {
    let mut state = state_with_model(true);
    state.show_processes = true;
    let mut with_processes = model();
    let mut secondary = with_processes.snapshot.gpus[0].partitions[0].clone();
    secondary.id = PartitionId::new("gpu-a-p1");
    secondary.index = 1;
    secondary.is_primary = false;
    with_processes.snapshot.gpus[0].partitions.push(secondary);
    let bdf = PciBdf::parse("0000:03:00.0").expect("bdf");
    let mut gpu_by_bdf = HashMap::new();
    gpu_by_bdf.insert(bdf.clone(), PhysicalGpuId::new("gpu-a"));
    let partition_by_bdf = HashMap::from([(bdf.clone(), PartitionId::new("gpu-a-p0"))]);
    with_processes.processes = Some(ProcessOverlay {
        scanned_at: sampled_at(),
        fdinfo_status: Reading::PermissionDenied,
        kfd_status: Reading::Value(()),
        rows: vec![
            ProcessRow {
                pid: 4242,
                name: Reading::Value("llama-server".to_owned()),
                bdf: Some(bdf),
                fdinfo_vram_bytes: Reading::Value(1_610_612_736),
                fdinfo_gtt_bytes: Reading::Absent,
                kfd_vram_bytes: Reading::Value(1_073_741_824),
                container: Some("deadbeefcafe".to_owned()),
            },
            ProcessRow {
                pid: 77,
                name: Reading::PermissionDenied,
                bdf: None,
                fdinfo_vram_bytes: Reading::PermissionDenied,
                fdinfo_gtt_bytes: Reading::Absent,
                kfd_vram_bytes: Reading::Absent,
                container: None,
            },
        ],
        gpu_by_bdf,
        partition_by_bdf,
    });
    let _ = state.adopt_model(with_processes);
    let text = buffer_text(&render(&state, 120, 40));
    assert!(text.contains("4242"));
    assert!(text.contains("llama-server"));
    assert!(text.contains("GPU 0/XCP 0"));
    assert!(text.contains("1.5 GiB"));
    assert!(text.contains("KFD") && text.contains("1.0 GiB"));
    assert!(text.contains("unknown"));
    assert!(text.contains("permission denied"));
    assert!(!text.contains("fdinfo permission denied"));
    assert!(text.contains("deadbeefcafe"));
    assert!(!text.contains("attributed memory only"));
}

#[test]
fn single_gpu_labels_omit_index_suffix() {
    let mut state = UiState::new(PresentationOptions::default(), None);
    let mut one = model();
    one.snapshot.gpus.truncate(1);
    one.gpus.truncate(1);
    let _ = state.adopt_model(one);
    let text = buffer_text(&render(&state, 120, 40));
    assert!(text.contains("AMD Instinct MI300X"));
    assert!(!text.contains("AMD Instinct MI300X · 0"));
}

#[test]
fn thermal_throttle_is_detail_only() {
    let mut state = state_with_model(true);
    let mut thermal = model();
    thermal.snapshot.gpus[0].health.category = HealthCategory::THROTTLE;
    thermal.snapshot.gpus[0].health.message = "thermal throttle active".to_owned();
    let _ = state.adopt_model(thermal);
    let main = buffer_text(&render(&state, 120, 40));
    assert!(!main.contains("thermal throttle active"));
    state.show_detail = true;
    let detail = buffer_text(&render(&state, 120, 40));
    assert!(detail.contains("thermal throttle active"));
}

#[test]
fn active_fault_still_renders_on_main_screen() {
    let mut state = state_with_model(true);
    let mut fault = model();
    fault.snapshot.gpus[0].health.category = HealthCategory::FAULT;
    fault.snapshot.gpus[0].health.message = "2 uncorrectable ECC errors".to_owned();
    let _ = state.adopt_model(fault);
    let main = buffer_text(&render(&state, 120, 40));
    assert!(main.contains("2 uncorrectable ECC errors"));
}
#[test]
fn compact_header_uses_only_supported_glyphs() {
    let state = state_with_model(true);
    let text = buffer_text(&render(&state, 80, 24));
    assert!(!text.contains('᦬'));
}

#[test]
fn detail_overlay_names_exact_observation_states() {
    let mut state = state_with_model(true);
    let _ = state.handle_key(KeyCode::Right); // gpu-b
    state.show_detail = true;
    let text = buffer_text(&render(&state, 120, 40));
    assert!(text.contains("asleep"), "exact asleep phrase missing");
    assert!(text.contains("stale 4.2s"), "stale age missing: {text}");
    assert!(
        text.contains("unsupported hardware"),
        "exact unsupported phrase missing"
    );
    assert!(text.contains("0000:c3:00.0"));
}

#[test]
fn help_overlay_shows_theme_and_preferred_vs_effective_surface() {
    let mut state = state_with_model(true);
    state.mode_preference = ModePreference::Mode;
    state.show_help = true;
    let text = buffer_text(&render(&state, 80, 24)); // mode does not fit here
    assert!(text.contains("buffalo"));
    assert!(text.contains("preferred mode · effective compact"));
    assert!(text.contains("select GPU"));
}
