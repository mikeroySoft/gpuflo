//! Binary glue: configuration, surface selection, exit-code mapping.
//!
//! This is the only layer that maps typed errors and outcomes to stderr and
//! process exit codes. It is public solely so `src/main.rs` can call it; it
//! is not part of the supported reuse interface.

use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::cli::{self, CliOptions, GpuSelector, Invocation, OutputMode};
use crate::config::{self, Environment};
use crate::model::{PhysicalGpu, Snapshot};
use crate::monitor::{Monitor, MonitorEvent, MonitorOptions};
use crate::output;
use crate::terminal::{CrosstermOps, TerminalGuard};
use crate::ui;

const EXIT_OK: u8 = 0;
const EXIT_FATAL: u8 = 1;
const EXIT_USAGE: u8 = 2;
const EXIT_SIGINT: u8 = 130;

/// Bound for the first exportable snapshot; the product budget is 1 s, the
/// bound leaves headroom for loaded hosts before declaring failure.
const FIRST_SNAPSHOT_BOUND: Duration = Duration::from_secs(5);

/// Parses the environment and arguments, runs the selected surface, and
/// returns the process exit code.
pub(crate) fn run_from_env() -> u8 {
    let invocation = match cli::parse(std::env::args_os().skip(1)) {
        Ok(invocation) => invocation,
        Err(error) => {
            eprintln!("gruflo: {error}");
            eprintln!("run `gruflo --help` for usage");
            return EXIT_USAGE;
        }
    };
    let options = match invocation {
        Invocation::Help => {
            print!("{}", cli::HELP);
            return EXIT_OK;
        }
        Invocation::Version => {
            println!("gruflo {}", env!("CARGO_PKG_VERSION"));
            return EXIT_OK;
        }
        Invocation::Run(options) => options,
    };

    let environment = Environment::from_process();
    let presentation = match config::resolve(&environment, &options) {
        Ok(presentation) => presentation,
        Err(error) => {
            eprintln!("gruflo: {error}");
            return EXIT_USAGE;
        }
    };

    let mut monitor_options = MonitorOptions::new();
    monitor_options.summary_path = environment.summary_path();
    apply_debug_seams(&mut monitor_options);

    let monitor = match Monitor::start(monitor_options) {
        Ok(monitor) => monitor,
        Err(error) => {
            eprintln!("gruflo: {error}");
            return EXIT_FATAL;
        }
    };

    match options.output {
        OutputMode::Once => one_shot(monitor, &options, write_once),
        OutputMode::Json => one_shot(monitor, &options, |out, snapshot, _| {
            let mut snapshot = snapshot.clone();
            snapshot.sequence = None;
            output::write_json(out, &snapshot)
        }),
        OutputMode::Tiny => one_shot(monitor, &options, write_tiny),
        OutputMode::JsonStream => json_stream(monitor),
        OutputMode::Interactive => interactive(monitor, presentation, options.gpu),
    }
}

/// Test-only host seams; release builds ignore these variables entirely.
fn apply_debug_seams(options: &mut MonitorOptions) {
    if !cfg!(debug_assertions) {
        return;
    }
    if let Some(root) = std::env::var_os("GRUFLO_HOST_ROOT") {
        options.host_root = Some(std::path::PathBuf::from(root));
    }
    if let Ok(ms) = std::env::var("GRUFLO_FATAL_AFTER_MS")
        && let Ok(ms) = ms.parse::<u64>()
    {
        options.fatal_after = Some(Duration::from_millis(ms));
    }
}

/// Waits for the first exportable snapshot (after the priming sample).
fn first_snapshot(monitor: &Monitor) -> Result<Snapshot, String> {
    let deadline = std::time::Instant::now() + FIRST_SNAPSHOT_BOUND;
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        match monitor.receive_timeout(remaining) {
            Ok(MonitorEvent::Snapshot(snapshot)) => return Ok(snapshot),
            Ok(MonitorEvent::Notice(_)) => continue,
            Ok(MonitorEvent::Fatal(error)) => return Err(error.to_string()),
            Err(_) => return Err("no snapshot could be produced".to_owned()),
        }
    }
}

/// Resolves `--gpu` against display index, stable id, or BDF.
fn select_gpu<'s>(
    snapshot: &'s Snapshot,
    selector: &Option<GpuSelector>,
) -> Result<&'s PhysicalGpu, String> {
    let Some(selector) = selector else {
        return snapshot
            .gpus
            .first()
            .ok_or_else(|| "no GPU discovered".to_owned());
    };
    let found = snapshot.gpus.iter().find(|gpu| match selector {
        GpuSelector::Index(index) => gpu.index == *index,
        GpuSelector::Bdf(bdf) => gpu.bdf == *bdf,
        GpuSelector::Id(id) => gpu.id.as_str() == id,
    });
    found.ok_or_else(|| {
        let available: Vec<String> = snapshot
            .gpus
            .iter()
            .map(|gpu| format!("{} ({}, {})", gpu.index, gpu.id, gpu.bdf))
            .collect();
        format!(
            "no GPU matches the selector; available: {}",
            available.join(", ")
        )
    })
}

/// Shared one-shot flow: prime, write, flush, shut down.
fn one_shot(
    monitor: Monitor,
    options: &CliOptions,
    write: impl Fn(&mut dyn Write, &Snapshot, &CliOptions) -> std::io::Result<()>,
) -> u8 {
    let snapshot = match first_snapshot(&monitor) {
        Ok(snapshot) => snapshot,
        Err(message) => {
            eprintln!("gruflo: {message}");
            let _ = monitor.shutdown();
            return EXIT_FATAL;
        }
    };
    // Selector validation happens before output production.
    if matches!(options.output, OutputMode::Tiny)
        && let Err(message) = select_gpu(&snapshot, &options.gpu)
    {
        eprintln!("gruflo: {message}");
        let _ = monitor.shutdown();
        return EXIT_USAGE;
    }
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let result = write(&mut out, &snapshot, options).and_then(|()| out.flush());
    let code = match result {
        Ok(()) => EXIT_OK,
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => EXIT_OK,
        Err(error) => {
            eprintln!("gruflo: cannot write output: {error}");
            EXIT_FATAL
        }
    };
    let _ = monitor.shutdown();
    code
}

fn write_once(
    out: &mut dyn Write,
    snapshot: &Snapshot,
    _options: &CliOptions,
) -> std::io::Result<()> {
    for gpu in &snapshot.gpus {
        output::write_once_line(out, gpu, snapshot.sampled_at)?;
    }
    Ok(())
}

fn write_tiny(
    out: &mut dyn Write,
    snapshot: &Snapshot,
    options: &CliOptions,
) -> std::io::Result<()> {
    let gpu =
        select_gpu(snapshot, &options.gpu).expect("selector validated before output production");
    output::write_tiny_line(out, gpu, snapshot.sampled_at)
}

/// Continuous compact NDJSON at production cadence.
fn json_stream(monitor: Monitor) -> u8 {
    let sigint = Arc::new(AtomicBool::new(false));
    let _ = signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&sigint));
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let code = loop {
        if sigint.load(Ordering::SeqCst) {
            break EXIT_SIGINT;
        }
        match monitor.receive_timeout(Duration::from_millis(250)) {
            Ok(MonitorEvent::Snapshot(snapshot)) => {
                match output::write_ndjson_line(&mut out, &snapshot) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => {
                        break EXIT_OK;
                    }
                    Err(error) => {
                        eprintln!("gruflo: cannot write output: {error}");
                        break EXIT_FATAL;
                    }
                }
            }
            Ok(MonitorEvent::Notice(notice)) => eprintln!("gruflo: {}", notice.message),
            Ok(MonitorEvent::Fatal(error)) => {
                eprintln!("gruflo: {error}");
                break EXIT_FATAL;
            }
            Err(crate::monitor::ReceiveTimeoutError::Timeout) => {}
            Err(crate::monitor::ReceiveTimeoutError::Closed) => break EXIT_FATAL,
        }
    };
    let _ = monitor.shutdown();
    code
}

/// Interactive flow: preflight already passed; acquire the terminal under
/// the staged guard, run the UI, restore before any diagnostic or shutdown.
fn interactive(
    monitor: Monitor,
    presentation: crate::config::PresentationOptions,
    initial_gpu: Option<GpuSelector>,
) -> u8 {
    let sigint = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));
    let _ = signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&sigint));
    let _ = signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&stop));
    let _ = signal_hook::flag::register(signal_hook::consts::SIGTERM, Arc::clone(&stop));
    let _ = signal_hook::flag::register(signal_hook::consts::SIGHUP, Arc::clone(&stop));

    // Panic hook: best-effort restoration without panicking, before the
    // default hook prints the panic message.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::cursor::Show,
            crossterm::terminal::LeaveAlternateScreen
        );
        let _ = crossterm::terminal::disable_raw_mode();
        default_hook(info);
    }));

    let mut guard = match TerminalGuard::acquire(CrosstermOps) {
        Ok(guard) => guard,
        Err(error) => {
            eprintln!("gruflo: cannot acquire the terminal: {error}");
            let _ = monitor.shutdown();
            return EXIT_FATAL;
        }
    };
    let backend = ratatui::backend::CrosstermBackend::new(std::io::stdout());
    let outcome = match ratatui::Terminal::new(backend) {
        Ok(mut terminal) => ui::run(&mut terminal, &monitor, presentation, initial_gpu, &stop),
        Err(error) => Err(error),
    };

    // Restoration precedes every diagnostic, shutdown, flush, and join.
    let restore_failed = guard.restore().is_err();
    drop(guard);
    let _ = std::panic::take_hook();

    let code = match outcome {
        Ok(ui::UiOutcome::Quit) => EXIT_OK,
        Ok(ui::UiOutcome::Interrupted) => {
            if sigint.load(Ordering::SeqCst) {
                EXIT_SIGINT
            } else {
                EXIT_OK // SIGTERM/SIGHUP: a requested, clean stop.
            }
        }
        Ok(ui::UiOutcome::MonitorFatal(error)) => {
            eprintln!("gruflo: {error}");
            EXIT_FATAL
        }
        Err(error) => {
            eprintln!("gruflo: interface failure: {error}");
            EXIT_FATAL
        }
    };
    let shutdown_failed = monitor.shutdown().is_err();
    if restore_failed || shutdown_failed {
        return EXIT_FATAL.max(code);
    }
    code
}
