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

/// Hard product bound for the first exportable snapshot.
const FIRST_SNAPSHOT_BOUND: Duration = Duration::from_secs(1);

/// Parses the environment and arguments, runs the selected surface, and
/// returns the process exit code.
pub(crate) fn run_from_env() -> u8 {
    let invocation = match cli::parse(std::env::args_os().skip(1)) {
        Ok(invocation) => invocation,
        Err(error) => {
            eprintln!("gpuflo: {error}");
            eprintln!("run `gpuflo --help` for usage");
            return EXIT_USAGE;
        }
    };
    let options = match invocation {
        Invocation::Help => {
            print!("{}", cli::HELP);
            return EXIT_OK;
        }
        Invocation::Version => {
            println!("gpuflo {}", env!("CARGO_PKG_VERSION"));
            return EXIT_OK;
        }
        Invocation::Run(options) => options,
    };

    let environment = Environment::from_process();
    let presentation = match config::resolve(&environment, &options) {
        Ok(presentation) => presentation,
        Err(error) => {
            eprintln!("gpuflo: {error}");
            return EXIT_USAGE;
        }
    };

    let mut monitor_options = MonitorOptions::new();
    monitor_options.summary_path = environment.summary_path();
    apply_debug_seams(&mut monitor_options);

    let monitor = match Monitor::start(monitor_options) {
        Ok(monitor) => monitor,
        Err(error) => {
            eprintln!("gpuflo: {error}");
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
        OutputMode::Interactive => {
            if options.gpu.is_some() {
                let snapshot = match first_snapshot(&monitor) {
                    Ok(snapshot) => snapshot,
                    Err(error) => {
                        eprintln!("gpuflo: {error}");
                        let _ = monitor.shutdown();
                        return EXIT_FATAL;
                    }
                };
                if let Err(error) = select_gpu(&snapshot, &options.gpu) {
                    eprintln!("gpuflo: {error}");
                    let _ = monitor.shutdown();
                    return EXIT_USAGE;
                }
            }
            interactive(monitor, presentation, options.gpu)
        }
    }
}

/// Test-only host seams; release builds ignore these variables entirely.
fn apply_debug_seams(options: &mut MonitorOptions) {
    if !cfg!(debug_assertions) {
        return;
    }
    if let Some(root) = std::env::var_os("GPUFLO_HOST_ROOT") {
        options.set_debug_host_root(std::path::PathBuf::from(root));
    }
    if let Ok(ms) = std::env::var("GPUFLO_FATAL_AFTER_MS")
        && let Ok(ms) = ms.parse::<u64>()
    {
        options.set_debug_fatal_after(Duration::from_millis(ms));
    }
}

/// Waits for the first exportable snapshot (after the priming sample).
fn first_snapshot(monitor: &Monitor) -> Result<Snapshot, String> {
    let deadline = std::time::Instant::now() + FIRST_SNAPSHOT_BOUND;
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        match monitor.receive_timeout(remaining) {
            Ok(MonitorEvent::Snapshot(snapshot)) => return Ok(snapshot),
            Ok(MonitorEvent::Notice(notice)) => eprintln!("gpuflo: {}", notice.message),
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
            eprintln!("gpuflo: {message}");
            let _ = monitor.shutdown();
            return EXIT_FATAL;
        }
    };
    if matches!(options.output, OutputMode::Tiny)
        && let Err(message) = select_gpu(&snapshot, &options.gpu)
    {
        eprintln!("gpuflo: {message}");
        let _ = monitor.shutdown();
        return if snapshot.gpus.is_empty() {
            EXIT_FATAL
        } else {
            EXIT_USAGE
        };
    }
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let result = write(&mut out, &snapshot, options).and_then(|()| out.flush());
    let (code, broken_pipe) = match result {
        Ok(()) => (EXIT_OK, false),
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => (EXIT_OK, true),
        Err(error) => {
            eprintln!("gpuflo: cannot write output: {error}");
            (EXIT_FATAL, false)
        }
    };
    match monitor.shutdown() {
        Ok(()) => code,
        Err(_) if broken_pipe => EXIT_OK,
        Err(error) => {
            eprintln!("gpuflo: {error}");
            EXIT_FATAL
        }
    }
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
    if let Err(error) =
        signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&sigint))
    {
        eprintln!("gpuflo: cannot install SIGINT handler: {error}");
        let _ = monitor.shutdown();
        return EXIT_FATAL;
    }
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let mut broken_pipe = false;
    let code = loop {
        if sigint.load(Ordering::SeqCst) {
            break EXIT_SIGINT;
        }
        match monitor.receive_timeout(Duration::from_millis(250)) {
            Ok(MonitorEvent::Snapshot(snapshot)) => {
                match output::write_ndjson_line(&mut out, &snapshot) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => {
                        broken_pipe = true;
                        break EXIT_OK;
                    }
                    Err(error) => {
                        eprintln!("gpuflo: cannot write output: {error}");
                        break EXIT_FATAL;
                    }
                }
            }
            Ok(MonitorEvent::Notice(notice)) => eprintln!("gpuflo: {}", notice.message),
            Ok(MonitorEvent::Fatal(error)) => {
                eprintln!("gpuflo: {error}");
                break EXIT_FATAL;
            }
            Err(crate::monitor::ReceiveTimeoutError::Timeout) => {}
            Err(crate::monitor::ReceiveTimeoutError::Closed) => break EXIT_FATAL,
        }
    };
    match monitor.shutdown() {
        Ok(()) => code,
        Err(_) if broken_pipe => EXIT_OK,
        Err(error) => {
            eprintln!("gpuflo: {error}");
            EXIT_FATAL
        }
    }
}

/// Interactive terminal flow.
fn interactive(
    monitor: Monitor,
    presentation: crate::config::PresentationOptions,
    initial_gpu: Option<GpuSelector>,
) -> u8 {
    let sigint = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));
    for (signal, flag) in [
        (signal_hook::consts::SIGINT, Arc::clone(&sigint)),
        (signal_hook::consts::SIGINT, Arc::clone(&stop)),
        (signal_hook::consts::SIGTERM, Arc::clone(&stop)),
        (signal_hook::consts::SIGHUP, Arc::clone(&stop)),
    ] {
        if let Err(error) = signal_hook::flag::register(signal, flag) {
            eprintln!("gpuflo: cannot install signal handler: {error}");
            let _ = monitor.shutdown();
            return EXIT_FATAL;
        }
    }

    let owner = std::thread::current().id();
    let original_hook: Arc<dyn Fn(&std::panic::PanicHookInfo<'_>) + Send + Sync + 'static> =
        Arc::from(std::panic::take_hook());
    let panic_hook = Arc::clone(&original_hook);
    std::panic::set_hook(Box::new(move |info| {
        // Only the terminal-owning thread may change terminal modes. Worker
        // panics remain ordinary diagnostics and are supervised by monitor.
        if std::thread::current().id() == owner {
            let _ = crossterm::execute!(std::io::stdout(), crossterm::cursor::Show);
            let _ = crossterm::terminal::disable_raw_mode();
            let _ =
                crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen);
        }
        panic_hook(info);
    }));

    let mut guard = match TerminalGuard::acquire(CrosstermOps) {
        Ok(guard) => guard,
        Err(error) => {
            let _ = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| original_hook(info)));
            eprintln!("gpuflo: cannot acquire the terminal: {error}");
            let _ = monitor.shutdown();
            return EXIT_FATAL;
        }
    };
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let backend = ratatui::backend::CrosstermBackend::new(std::io::stdout());
        match ratatui::Terminal::new(backend) {
            Ok(mut terminal) => ui::run(&mut terminal, &monitor, presentation, initial_gpu, &stop),
            Err(error) => Err(error),
        }
    }));

    // Restoration precedes every diagnostic, shutdown, flush, and join.
    let restore_failed = guard.restore().is_err();
    drop(guard);
    let _ = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| original_hook(info)));

    let code = match outcome {
        Ok(Ok(ui::UiOutcome::Quit)) => EXIT_OK,
        Ok(Ok(ui::UiOutcome::Interrupted)) => {
            if sigint.load(Ordering::SeqCst) {
                EXIT_SIGINT
            } else {
                EXIT_OK
            }
        }
        Ok(Ok(ui::UiOutcome::MonitorFatal(error))) => {
            eprintln!("gpuflo: {error}");
            EXIT_FATAL
        }
        Ok(Err(error)) => {
            eprintln!("gpuflo: interface failure: {error}");
            EXIT_FATAL
        }
        Err(_) => EXIT_FATAL, // panic hook already printed after restoration.
    };
    let shutdown_error = monitor.shutdown().err();
    if let Some(error) = &shutdown_error {
        eprintln!("gpuflo: {error}");
    }
    if restore_failed || shutdown_error.is_some() {
        return if code == EXIT_SIGINT {
            EXIT_SIGINT
        } else {
            EXIT_FATAL
        };
    }
    code
}
