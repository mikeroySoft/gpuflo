//! Interactive terminal instrument: render loop, input map, and session
//! presentation state.
//!
//! The loop renders every 125 ms and pulls the latest [`RenderModel`] each
//! iteration; rendering is a pure projection of that model. Animation —
//! spring-smoothed activity, breathing gradients, sub-cell graph
//! interpolation — owns display state only and never mutates canonical
//! observations, histories, or health.

mod cat;
mod format;
mod layout;
mod taglines;
mod theme;
mod widgets;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;

use crate::cli::GpuSelector;
use crate::config::{ModePreference, PresentationOptions, Theme};
use crate::model::{Partition, PhysicalGpu, PhysicalGpuId};
use crate::monitor::{Monitor, MonitorCommand, MonitorError, MonitorEvent, ReceiveTimeoutError};
use crate::state::RenderModel;

/// Render cadence; animation only, decoupled from the 250 ms production tick.
const FRAME_INTERVAL: Duration = Duration::from_millis(125);
/// Production tick length, used for sub-cell graph interpolation.
const TICK_INTERVAL: Duration = Duration::from_millis(250);
/// How long a transient notice stays visible.
const NOTICE_TTL: Duration = Duration::from_secs(4);

/// Why the interactive surface stopped.
pub(crate) enum UiOutcome {
    /// The user quit with `q` or Escape.
    Quit,
    /// The process received an interrupt signal.
    Interrupted,
    /// The monitor can produce no further snapshots.
    MonitorFatal(MonitorError),
}

/// Runs the interactive instrument until quit, interrupt, or monitor fatal.
/// The caller owns terminal setup and restoration.
pub(crate) fn run(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    monitor: &Monitor,
    presentation: PresentationOptions,
    initial_gpu: Option<GpuSelector>,
    interrupted: &AtomicBool,
) -> io::Result<UiOutcome> {
    let mut state = UiState::new(presentation, initial_gpu);
    let mut next_frame = Instant::now();

    loop {
        if interrupted.load(Ordering::SeqCst) {
            return Ok(UiOutcome::Interrupted);
        }

        // Drain pending monitor events without blocking. Snapshots are
        // ignored here: the render model below carries the same canonical
        // data in presentation shape.
        loop {
            match monitor.receive_timeout(Duration::ZERO) {
                Ok(MonitorEvent::Notice(notice)) => state.show_notice(notice.message),
                Ok(MonitorEvent::Fatal(error)) => return Ok(UiOutcome::MonitorFatal(error)),
                Ok(_) => {}
                Err(ReceiveTimeoutError::Timeout) => break,
                Err(ReceiveTimeoutError::Closed) => {
                    return Ok(UiOutcome::MonitorFatal(MonitorError::Internal(
                        "monitor closed".to_owned(),
                    )));
                }
            }
        }

        if let Some(model) = monitor.take_render_model()
            && let Some(scope) = state.adopt_model(model)
        {
            // The open process overlay follows the effective selection.
            let _ = monitor.command(MonitorCommand::SetProcessScope(scope));
        }

        let now = Instant::now();
        if now >= next_frame {
            state.animate(FRAME_INTERVAL.as_secs_f64());
            terminal.draw(|frame| widgets::draw(frame, &state))?;
            next_frame = now + FRAME_INTERVAL;
        }

        let wait = next_frame
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(25));
        if !event::poll(wait)? {
            continue;
        }
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                let area = terminal.size()?;
                match state.handle_key_in_area(key.code, area.into()) {
                    Action::Quit => return Ok(UiOutcome::Quit),
                    Action::SetProcessScope(scope) => {
                        let _ = monitor.command(MonitorCommand::SetProcessScope(scope));
                    }
                    Action::None => {}
                }
            }
            Event::Resize(_, _) => next_frame = Instant::now(),
            _ => {}
        }
    }
}

/// What a handled key asks the loop to do beyond mutating session state.
#[derive(Debug, PartialEq, Eq)]
enum Action {
    None,
    Quit,
    /// Send [`MonitorCommand::SetProcessScope`] with this scope.
    SetProcessScope(Option<PhysicalGpuId>),
}

/// Spring display state for one GPU's shown activity; UI-only, never data.
struct SpringState {
    shown: f64,
    velocity: f64,
}

/// Session presentation state: everything the user can change with keys plus
/// the latest adopted render model. Never canonical product state.
pub(super) struct UiState {
    theme: Theme,
    mode_preference: ModePreference,
    color_enabled: bool,
    truecolor: bool,
    /// Opt-in: show a sleeping ASCII cat when the selected GPU is warm.
    cat_enabled: bool,
    /// Chosen once at launch and retained for the whole interactive session.
    tagline: &'static str,
    model: Option<RenderModel>,
    /// Selection tracks the stable physical GPU identity, never an index.
    selected: Option<PhysicalGpuId>,
    /// Display position of the selection in the last adopted model; used to
    /// pick the nearest survivor when the selected GPU disappears.
    selected_position: usize,
    /// A selected GPU that disappeared; restored when it returns.
    remembered: Option<PhysicalGpuId>,
    /// `--gpu` selector, resolved against the first received model.
    pending_selector: Option<GpuSelector>,
    show_processes: bool,
    show_detail: bool,
    show_help: bool,
    notice: Option<(String, Instant)>,
    started: Instant,
    model_received: Instant,
    springs: HashMap<PhysicalGpuId, SpringState>,
}

impl UiState {
    fn new(presentation: PresentationOptions, initial_gpu: Option<GpuSelector>) -> Self {
        let now = Instant::now();
        Self {
            theme: presentation.theme,
            mode_preference: presentation.mode_preference,
            color_enabled: presentation.color_enabled,
            truecolor: presentation.truecolor,
            cat_enabled: presentation.cat_enabled,
            tagline: taglines::select(),
            model: None,
            selected: None,
            selected_position: 0,
            remembered: None,
            pending_selector: initial_gpu,
            show_processes: false,
            show_detail: false,
            show_help: false,
            notice: None,
            started: now,
            model_received: now,
            springs: HashMap::new(),
        }
    }

    /// Adopts a fresh render model, maintaining identity-stable selection.
    /// Returns the new process scope when the open overlay must retarget.
    fn adopt_model(&mut self, model: RenderModel) -> Option<Option<PhysicalGpuId>> {
        let before = self.selected.clone();
        let gpus = &model.snapshot.gpus;
        if gpus.is_empty() {
            if self.selected.is_some() && self.remembered.is_none() {
                self.remembered = self.selected.clone();
            }
            self.selected = None;
        } else {
            if let Some(selector) = self.pending_selector.take() {
                self.selected =
                    Some(resolve_selector(&selector, gpus).unwrap_or_else(|| gpus[0].id.clone()));
            }
            // A remembered GPU that returned wins over any fallback.
            if let Some(remembered) = &self.remembered
                && gpus.iter().any(|gpu| gpu.id == *remembered)
            {
                self.selected = Some(remembered.clone());
                self.remembered = None;
            }
            let position = self
                .selected
                .as_ref()
                .and_then(|id| gpus.iter().position(|gpu| gpu.id == *id));
            match position {
                Some(position) => self.selected_position = position,
                None => {
                    // Selection disappeared: remember it and fall back to the
                    // nearest survivor by display order.
                    if self.remembered.is_none() {
                        self.remembered = self.selected.clone();
                    }
                    let position = self.selected_position.min(gpus.len() - 1);
                    self.selected = Some(gpus[position].id.clone());
                    self.selected_position = position;
                }
            }
        }
        self.springs
            .retain(|id, _| gpus.iter().any(|gpu| gpu.id == *id));
        self.model_received = Instant::now();
        self.model = Some(model);
        if self.show_processes && self.selected != before {
            Some(self.selected.clone())
        } else {
            None
        }
    }

    /// Advances display-only animation state by `dt` seconds.
    fn animate(&mut self, dt: f64) {
        let Some(model) = &self.model else { return };
        for gpu in &model.snapshot.gpus {
            let target = primary_partition(gpu)
                .and_then(|partition| partition.activity_percent.current().copied());
            match target {
                Some(value) if value.is_finite() => {
                    let entry = self.springs.entry(gpu.id.clone()).or_insert(SpringState {
                        shown: value,
                        velocity: 0.0,
                    });
                    entry.shown = format::spring(entry.shown, value, &mut entry.velocity, dt);
                }
                _ => {
                    // No fresh observation: freeze rather than animate toward
                    // an invented value.
                    if let Some(entry) = self.springs.get_mut(&gpu.id) {
                        entry.velocity = 0.0;
                    }
                }
            }
        }
    }

    #[cfg(test)]
    fn handle_key(&mut self, code: KeyCode) -> Action {
        self.handle_key_in_area(code, Rect::new(0, 0, 120, 40))
    }

    fn handle_key_in_area(&mut self, code: KeyCode, area: Rect) -> Action {
        match code {
            KeyCode::Char('q') => Action::Quit,
            KeyCode::Esc => {
                if self.show_help {
                    self.show_help = false;
                    Action::None
                } else if self.show_processes {
                    self.show_processes = false;
                    Action::SetProcessScope(None)
                } else if self.show_detail {
                    self.show_detail = false;
                    Action::None
                } else {
                    Action::Quit
                }
            }
            KeyCode::Left | KeyCode::Char('h') => self.select_step(-1),
            KeyCode::Right | KeyCode::Char('l') => self.select_step(1),
            KeyCode::Char('t') => {
                self.theme = self.theme.next();
                Action::None
            }
            KeyCode::Char('m') => {
                self.mode_preference = match layout::effective(self.mode_preference, area) {
                    layout::Surface::Mode => ModePreference::Compact,
                    layout::Surface::Compact => ModePreference::Mini,
                    layout::Surface::Mini => ModePreference::Tiny,
                    layout::Surface::Tiny => ModePreference::Mode,
                };
                Action::None
            }
            KeyCode::Char('p') => {
                if !self.show_processes && self.selected.is_none() {
                    return Action::None;
                }
                self.show_processes = !self.show_processes;
                if self.show_processes {
                    Action::SetProcessScope(self.selected.clone())
                } else {
                    Action::SetProcessScope(None)
                }
            }
            KeyCode::Char('d') => {
                self.show_detail = !self.show_detail;
                Action::None
            }
            KeyCode::Char('?') => {
                self.show_help = !self.show_help;
                Action::None
            }
            _ => Action::None,
        }
    }

    /// Moves the selection by `delta` in display order, wrapping.
    fn select_step(&mut self, delta: isize) -> Action {
        let Some(model) = &self.model else {
            return Action::None;
        };
        let gpus = &model.snapshot.gpus;
        if gpus.is_empty() {
            return Action::None;
        }
        let len = gpus.len() as isize;
        let position = self.selected_index().unwrap_or(0) as isize;
        let next = ((position + delta) % len + len) % len;
        let next = next as usize;
        self.selected = Some(gpus[next].id.clone());
        self.selected_position = next;
        // Manual selection replaces any pending disappearance restore.
        self.remembered = None;
        if self.show_processes {
            Action::SetProcessScope(self.selected.clone())
        } else {
            Action::None
        }
    }

    /// Display position of the selected GPU in the current model.
    fn selected_index(&self) -> Option<usize> {
        let model = self.model.as_ref()?;
        let id = self.selected.as_ref()?;
        model.snapshot.gpus.iter().position(|gpu| gpu.id == *id)
    }

    /// Spring-smoothed displayed activity for one GPU, when animated.
    fn shown_activity(&self, id: &PhysicalGpuId) -> Option<f64> {
        self.springs.get(id).map(|spring| spring.shown)
    }

    /// Fraction of the current production tick elapsed, for sub-cell
    /// graph interpolation.
    fn graph_fraction(&self) -> f64 {
        (self.model_received.elapsed().as_secs_f64() / TICK_INTERVAL.as_secs_f64()).clamp(0.0, 1.0)
    }

    fn show_notice(&mut self, message: String) {
        self.notice = Some((message, Instant::now()));
    }

    /// The transient notice text while within its display window.
    fn notice_line(&self) -> Option<&str> {
        self.notice
            .as_ref()
            .filter(|(_, shown_at)| shown_at.elapsed() < NOTICE_TTL)
            .map(|(message, _)| message.as_str())
    }
}

/// The partition owning socket-reported telemetry: the primary XCP, falling
/// back to the first partition.
fn primary_partition(gpu: &PhysicalGpu) -> Option<&Partition> {
    gpu.partitions
        .iter()
        .find(|partition| partition.is_primary)
        .or_else(|| gpu.partitions.first())
}

/// Resolves `--gpu` against the first received model: index against the
/// display index, BDF against the PCI address, id against the stable id.
fn resolve_selector(selector: &GpuSelector, gpus: &[PhysicalGpu]) -> Option<PhysicalGpuId> {
    let found = match selector {
        GpuSelector::Index(index) => gpus.iter().find(|gpu| gpu.index == *index),
        GpuSelector::Bdf(bdf) => gpus.iter().find(|gpu| gpu.bdf == *bdf),
        GpuSelector::Id(id) => gpus.iter().find(|gpu| gpu.id.as_str() == id),
    };
    found.map(|gpu| gpu.id.clone())
}
