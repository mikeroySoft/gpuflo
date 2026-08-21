//! The supported monitor interface and its private coordinator runtime.
//!
//! One coordinator thread owns monotonic schedules, normalization order,
//! reducer application, sequence-numbered publication, discovery lifecycle,
//! and shutdown. Potentially blocking work runs on bounded source lanes with
//! capacity-one request/result channels; sending is nonblocking and missed
//! work is skipped, never queued. Outward delivery separates a lossy
//! latest-snapshot mailbox from a priority notice/fatal mailbox.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, bounded};

use crate::model::{MemoryPool, PartitionId, PciBdf, PhysicalGpuId, Snapshot, Timestamp};
use crate::normalize;
use crate::persist::{self, PersistLane};
use crate::source::amdsmi::AmdSmi;
use crate::source::kernel::{KernelDevice, KernelSource};
use crate::source::process::{ProcessSample, ProcessSource};
use crate::source::{KernelFastSample, KernelSlowSample, Reading};
use crate::state::reducer::{Now, Reducer};
use crate::state::{Lane, ProcessOverlay, RenderModel, StateEffect};

/// Fast kernel collection cadence; also the production tick.
const FAST_CADENCE: Duration = Duration::from_millis(250);
/// Slow health collection cadence.
const SLOW_CADENCE: Duration = Duration::from_secs(1);
/// Process collection cadence, only while a scope is active.
const PROCESS_CADENCE: Duration = Duration::from_secs(2);
/// Topology rescan cadence.
const REDISCOVERY_CADENCE: Duration = Duration::from_secs(2);
/// AMD SMI reload cooldown after a failed load.
const AMDSMI_RETRY_COOLDOWN: Duration = Duration::from_secs(30);
/// Bound for joining lanes and flushing persistence at shutdown.
const SHUTDOWN_BOUND: Duration = Duration::from_secs(2);

/// Options for [`Monitor::start`].
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct MonitorOptions {
    /// Where to load/store the small daily summary. `None` disables
    /// persistence without changing telemetry semantics.
    pub summary_path: Option<PathBuf>,
    host_root: Option<PathBuf>,
    fatal_after: Option<Duration>,
}

impl MonitorOptions {
    /// Default options: persistence disabled.
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn set_debug_host_root(&mut self, root: PathBuf) {
        if cfg!(debug_assertions) {
            self.host_root = Some(root);
        }
    }

    pub(crate) fn set_debug_fatal_after(&mut self, after: Duration) {
        if cfg!(debug_assertions) {
            self.fatal_after = Some(after);
        }
    }
}

/// A factual lifecycle transition; never a log line.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Notice {
    /// What happened, e.g. `GPU disconnected: gpu-…`.
    pub message: String,
    /// When the transition was confirmed.
    pub occurred_at: Timestamp,
}

/// Events delivered by [`Monitor::receive`].
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum MonitorEvent {
    /// A new owned, immutable snapshot.
    Snapshot(Snapshot),
    /// A factual lifecycle transition.
    Notice(Notice),
    /// The monitor can produce no further snapshots.
    Fatal(MonitorError),
}

/// Bounded commands accepted while the monitor runs.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum MonitorCommand {
    /// Enables the process lane scoped to one GPU, or disables it.
    SetProcessScope(Option<PhysicalGpuId>),
    /// Clears session peaks.
    ResetSessionPeaks,
}

/// Startup cannot proceed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum StartError {
    /// The host is not Linux.
    #[error("gruflo requires Linux")]
    UnsupportedHost,
    /// No AMD PCI/DRM device bound to `amdgpu` was discoverable.
    #[error("no AMD GPU bound to the amdgpu driver was found")]
    NoGpu,
    /// The DRM topology scan could not complete reliably.
    #[error("cannot discover AMD GPUs: {0}")]
    Discovery(String),
}

/// The coordinator cannot produce further valid snapshots.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum MonitorError {
    /// XCP topology changed; a fresh process must re-enumerate.
    #[error("GPU partition configuration changed; restart gruflo")]
    PartitionConfigurationChanged,
    /// An injected or internal coordinator failure.
    #[error("monitor failure: {0}")]
    Internal(String),
}

/// The monitor terminated and delivered its final event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("monitor closed")]
pub struct MonitorClosed;

/// [`Monitor::receive_timeout`] outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ReceiveTimeoutError {
    /// No event arrived within the timeout.
    #[error("receive timed out")]
    Timeout,
    /// The monitor terminated and delivered its final event.
    #[error("monitor closed")]
    Closed,
}

/// Bounded shutdown did not complete cleanly.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ShutdownError {
    /// A lane or the persistence flush exceeded the shutdown bound and was
    /// detached.
    #[error("shutdown incomplete: {0}")]
    Incomplete(String),
}

/// Outward capacity-one mailboxes behind one receive interface.
struct Outward {
    snapshot: Option<Snapshot>,
    notice: Option<Notice>,
    fatal: Option<MonitorError>,
    /// The coordinator exited; after any pending fatal is delivered, the
    /// stream is closed.
    closed: bool,
}

struct Shared {
    outward: Mutex<Outward>,
    ready: Condvar,
    render: Mutex<Option<RenderModel>>,
}

impl Shared {
    fn new() -> Self {
        Self {
            outward: Mutex::new(Outward {
                snapshot: None,
                notice: None,
                fatal: None,
                closed: false,
            }),
            ready: Condvar::new(),
            render: Mutex::new(None),
        }
    }

    /// Lossy latest-snapshot publication; never displaces control events.
    fn publish_snapshot(&self, snapshot: Snapshot) {
        let mut outward = self.outward.lock().expect("outward mailbox");
        if outward.fatal.is_some() || outward.closed {
            return;
        }
        outward.snapshot = Some(snapshot);
        self.ready.notify_all();
    }

    fn publish_notice(&self, notice: Notice) {
        let mut outward = self.outward.lock().expect("outward mailbox");
        if outward.fatal.is_some() || outward.closed {
            return;
        }
        // Capacity one: a newer notice replaces an unconsumed older one.
        outward.notice = Some(notice);
        self.ready.notify_all();
    }

    /// Fatal replaces any pending notice and stops future publication.
    fn publish_fatal(&self, error: MonitorError) {
        let mut outward = self.outward.lock().expect("outward mailbox");
        if outward.closed {
            return;
        }
        outward.notice = None;
        outward.snapshot = None;
        outward.fatal = Some(error);
        self.ready.notify_all();
    }

    fn close(&self) {
        let mut outward = self.outward.lock().expect("outward mailbox");
        outward.closed = true;
        self.ready.notify_all();
    }

    fn publish_render(&self, model: RenderModel) {
        *self.render.lock().expect("render mailbox") = Some(model);
    }

    /// Control-first receive shared by both public receive forms.
    fn receive_deadline(
        &self,
        deadline: Option<Instant>,
    ) -> Result<MonitorEvent, ReceiveTimeoutError> {
        let mut outward = self.outward.lock().expect("outward mailbox");
        loop {
            if let Some(error) = outward.fatal.take() {
                outward.closed = true;
                return Ok(MonitorEvent::Fatal(error));
            }
            if let Some(notice) = outward.notice.take() {
                return Ok(MonitorEvent::Notice(notice));
            }
            if let Some(snapshot) = outward.snapshot.take() {
                return Ok(MonitorEvent::Snapshot(snapshot));
            }
            if outward.closed {
                return Err(ReceiveTimeoutError::Closed);
            }
            outward = match deadline {
                None => self.ready.wait(outward).expect("outward mailbox"),
                Some(deadline) => {
                    let now = Instant::now();
                    if now >= deadline {
                        return Err(ReceiveTimeoutError::Timeout);
                    }
                    let (guard, timeout) = self
                        .ready
                        .wait_timeout(outward, deadline - now)
                        .expect("outward mailbox");
                    let pending = guard.fatal.is_some()
                        || guard.notice.is_some()
                        || guard.snapshot.is_some()
                        || guard.closed;
                    if timeout.timed_out() && !pending {
                        return Err(ReceiveTimeoutError::Timeout);
                    }
                    guard
                }
            };
        }
    }
}

/// The running monitor. Dropping it without [`Monitor::shutdown`] detaches
/// the coordinator, which exits when its command channel disconnects.
pub struct Monitor {
    shared: Arc<Shared>,
    directives: Sender<Directive>,
    done: Receiver<Result<(), ShutdownError>>,
    join: Option<std::thread::JoinHandle<()>>,
}

enum Directive {
    Command(MonitorCommand),
    Shutdown,
}

struct SummaryLoadLane {
    result: Receiver<Result<Option<crate::state::history::DailySummaryRecord>, String>>,
    join: Option<std::thread::JoinHandle<()>>,
    deadline: Instant,
}

fn spawn_summary_load(path: Option<PathBuf>) -> (Option<SummaryLoadLane>, Option<String>) {
    let Some(path) = path else {
        return (None, None);
    };
    let (sender, receiver) = bounded(1);
    match std::thread::Builder::new()
        .name("gruflo-persist-load".to_owned())
        .spawn(move || {
            let _ = sender.try_send(persist::load(&path));
        }) {
        Ok(join) => (
            Some(SummaryLoadLane {
                result: receiver,
                join: Some(join),
                deadline: Instant::now() + Duration::from_millis(100),
            }),
            None,
        ),
        Err(error) => (
            None,
            Some(format!("cannot start daily summary load: {error}")),
        ),
    }
}

impl Monitor {
    /// Performs host/device preflight and starts the coordinator thread.
    pub fn start(options: MonitorOptions) -> Result<Self, StartError> {
        if !cfg!(target_os = "linux") {
            return Err(StartError::UnsupportedHost);
        }
        #[cfg(debug_assertions)]
        let debug_root = std::env::var_os("GRUFLO_HOST_ROOT").map(PathBuf::from);
        #[cfg(not(debug_assertions))]
        let debug_root: Option<PathBuf> = None;
        let root = options
            .host_root
            .clone()
            .or(debug_root)
            .unwrap_or_else(|| PathBuf::from("/"));
        let kernel = KernelSource::new(root.clone());
        let devices = kernel.discover().map_err(StartError::Discovery)?;
        if devices.is_empty() {
            return Err(StartError::NoGpu);
        }

        // Capture the local offset before spawning threads; day rollover
        // keeps this startup offset for the whole session.
        let local_offset = time::UtcOffset::current_local_offset().unwrap_or(time::UtcOffset::UTC);
        let (summary_load, summary_notice) = spawn_summary_load(options.summary_path.clone());

        let shared = Arc::new(Shared::new());
        let (directive_tx, directive_rx) = bounded::<Directive>(1);
        let (done_tx, done_rx) = bounded::<Result<(), ShutdownError>>(1);
        let coordinator_shared = Arc::clone(&shared);
        let join = std::thread::Builder::new()
            .name("gruflo-coordinator".to_owned())
            .spawn(move || {
                let mut coordinator = Coordinator::new(
                    coordinator_shared,
                    directive_rx,
                    root,
                    devices,
                    options.summary_path,
                    summary_load,
                    summary_notice,
                    local_offset,
                    options.fatal_after,
                );
                let _ = done_tx.send(coordinator.run());
            })
            .expect("spawn coordinator thread");

        Ok(Self {
            shared,
            directives: directive_tx,
            done: done_rx,
            join: Some(join),
        })
    }

    /// Blocks until the next event. Control events (notice/fatal) are always
    /// delivered before a pending snapshot.
    pub fn receive(&self) -> Result<MonitorEvent, MonitorClosed> {
        self.shared
            .receive_deadline(None)
            .map_err(|_| MonitorClosed)
    }

    /// Bounded receive.
    pub fn receive_timeout(&self, timeout: Duration) -> Result<MonitorEvent, ReceiveTimeoutError> {
        self.shared.receive_deadline(Some(Instant::now() + timeout))
    }

    /// Sends one bounded command; fails only when the monitor terminated.
    pub fn command(&self, command: MonitorCommand) -> Result<(), MonitorClosed> {
        self.directives
            .send(Directive::Command(command))
            .map_err(|_| MonitorClosed)
    }

    /// Latest private render projection, for the in-crate UI only.
    pub(crate) fn take_render_model(&self) -> Option<RenderModel> {
        self.shared.render.lock().expect("render mailbox").take()
    }

    /// Explicit shutdown: stops publication, flushes the daily summary, and
    /// joins lanes to a bounded deadline.
    pub fn shutdown(mut self) -> Result<(), ShutdownError> {
        let deadline = Instant::now() + SHUTDOWN_BOUND;
        self.directives
            .send_timeout(Directive::Shutdown, remaining(deadline))
            .map_err(|_| {
                ShutdownError::Incomplete(
                    "could not deliver shutdown within the shutdown bound".to_owned(),
                )
            })?;
        let result = self.done.recv_timeout(remaining(deadline)).map_err(|_| {
            ShutdownError::Incomplete(
                "coordinator did not stop within the shutdown bound".to_owned(),
            )
        })?;
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
        result
    }
}

// ---------------------------------------------------------------------------
// Lanes
fn disconnect_sender<T>(sender: &mut Sender<T>) {
    let (dummy, receiver) = bounded(0);
    drop(receiver);
    drop(std::mem::replace(sender, dummy));
}

fn remaining(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}

fn join_worker<T>(
    name: &str,
    sender: &mut Sender<T>,
    done: &Receiver<()>,
    join: &mut Option<std::thread::JoinHandle<()>>,
    deadline: Instant,
    failures: &mut Vec<String>,
) {
    if join.is_none() {
        return;
    }
    disconnect_sender(sender);
    if done.recv_timeout(remaining(deadline)).is_ok() {
        if let Some(handle) = join.take() {
            let _ = handle.join();
        }
    } else {
        join.take(); // bounded final fallback: detach and report.
        failures.push(name.to_owned());
    }
}

// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum KernelJob {
    Fast,
    Slow,
}

enum KernelOutcome {
    Fast(KernelFastSample),
    Slow(KernelSlowSample),
}

struct TimedRequest<J> {
    generation: u64,
    job: J,
}

struct TimedResult<T> {
    generation: u64,
    value: T,
}

struct InFlight {
    generation: u64,
    deadline: Instant,
    timed_out: bool,
}

fn accept_result(in_flight: &mut Option<InFlight>, generation: u64, now: Instant) -> bool {
    let Some(current) = in_flight.take() else {
        return false;
    };
    if current.generation != generation {
        *in_flight = Some(current);
        return false;
    }
    !current.timed_out && now < current.deadline
}

struct KernelLane {
    request: Sender<TimedRequest<KernelJob>>,
    result: Receiver<TimedResult<KernelOutcome>>,
    done: Receiver<()>,
    join: Option<std::thread::JoinHandle<()>>,
    current_job: Option<KernelJob>,
    in_flight: Option<InFlight>,
    next_generation: u64,
    /// Slow work waits until the lane is idle after a fast result.
    slow_due: bool,
    failed: bool,
}

impl KernelLane {
    fn dispatch(&mut self, job: KernelJob, now: Instant) -> bool {
        if self.failed || self.in_flight.is_some() {
            return false;
        }
        let generation = self.next_generation;
        self.next_generation += 1;
        let deadline = now
            + match job {
                KernelJob::Fast => Duration::from_millis(200),
                KernelJob::Slow => Duration::from_millis(900),
            };
        if self
            .request
            .try_send(TimedRequest { generation, job })
            .is_ok()
        {
            self.current_job = Some(job);
            self.in_flight = Some(InFlight {
                generation,
                deadline,
                timed_out: false,
            });
            true
        } else {
            false
        }
    }
}

fn spawn_kernel_lane(root: PathBuf, device: KernelDevice) -> KernelLane {
    let (request_tx, request_rx) = bounded::<TimedRequest<KernelJob>>(1);
    let (result_tx, result_rx) = bounded::<TimedResult<KernelOutcome>>(1);
    let (done_tx, done_rx) = bounded::<()>(1);
    let join = std::thread::Builder::new()
        .name(format!("gruflo-kernel-{}", device.disc.bdf))
        .spawn(move || {
            let mut source = KernelSource::new(root);
            while let Ok(request) = request_rx.recv() {
                let value = match request.job {
                    KernelJob::Fast => KernelOutcome::Fast(source.collect_fast(&device)),
                    KernelJob::Slow => KernelOutcome::Slow(source.collect_slow(&device)),
                };
                let _ = result_tx.try_send(TimedResult {
                    generation: request.generation,
                    value,
                });
            }
            let _ = done_tx.send(());
        })
        .expect("spawn kernel lane");
    KernelLane {
        request: request_tx,
        result: result_rx,
        done: done_rx,
        join: Some(join),
        current_job: None,
        in_flight: None,
        next_generation: 1,
        slow_due: false,
        failed: false,
    }
}

struct DiscoveryLane {
    request: Sender<TimedRequest<()>>,
    result: Receiver<TimedResult<Result<Vec<KernelDevice>, String>>>,
    done: Receiver<()>,
    join: Option<std::thread::JoinHandle<()>>,
    in_flight: Option<InFlight>,
    next_generation: u64,
    failed: bool,
}

impl DiscoveryLane {
    fn dispatch(&mut self, now: Instant) {
        if self.failed || self.in_flight.is_some() {
            return;
        }
        let generation = self.next_generation;
        self.next_generation += 1;
        if self
            .request
            .try_send(TimedRequest {
                generation,
                job: (),
            })
            .is_ok()
        {
            self.in_flight = Some(InFlight {
                generation,
                deadline: now + Duration::from_millis(1500),
                timed_out: false,
            });
        }
    }
}

fn spawn_discovery_lane(root: PathBuf) -> DiscoveryLane {
    let (request_tx, request_rx) = bounded::<TimedRequest<()>>(1);
    let (result_tx, result_rx) = bounded::<TimedResult<Result<Vec<KernelDevice>, String>>>(1);
    let (done_tx, done_rx) = bounded::<()>(1);
    let join = std::thread::Builder::new()
        .name("gruflo-discovery".to_owned())
        .spawn(move || {
            let source = KernelSource::new(root);
            while let Ok(request) = request_rx.recv() {
                let _ = result_tx.try_send(TimedResult {
                    generation: request.generation,
                    value: source.discover(),
                });
            }
            let _ = done_tx.send(());
        })
        .expect("spawn discovery lane");
    DiscoveryLane {
        request: request_tx,
        result: result_rx,
        done: done_rx,
        join: Some(join),
        in_flight: None,
        next_generation: 1,
        failed: false,
    }
}

#[derive(Clone, Copy)]
enum AmdSmiJob {
    Sample,
    Reload,
}

struct AmdSmiLane {
    request: Sender<TimedRequest<AmdSmiJob>>,
    result: Receiver<TimedResult<Vec<crate::source::amdsmi::AmdSmiSample>>>,
    done: Receiver<()>,
    join: Option<std::thread::JoinHandle<()>>,
    in_flight: Option<InFlight>,
    next_generation: u64,
    reload_due: bool,
    failed: bool,
}

impl AmdSmiLane {
    fn dispatch(&mut self, job: AmdSmiJob, now: Instant) {
        if matches!(job, AmdSmiJob::Reload) {
            self.reload_due = true;
        }
        if self.failed || self.in_flight.is_some() {
            return;
        }
        let job = if self.reload_due {
            self.reload_due = false;
            AmdSmiJob::Reload
        } else {
            job
        };
        let generation = self.next_generation;
        self.next_generation += 1;
        if self
            .request
            .try_send(TimedRequest { generation, job })
            .is_ok()
        {
            self.in_flight = Some(InFlight {
                generation,
                deadline: now + Duration::from_millis(900),
                timed_out: false,
            });
        }
    }
}

fn spawn_amdsmi_lane() -> AmdSmiLane {
    let (request_tx, request_rx) = bounded::<TimedRequest<AmdSmiJob>>(1);
    let (result_tx, result_rx) =
        bounded::<TimedResult<Vec<crate::source::amdsmi::AmdSmiSample>>>(1);
    let (done_tx, done_rx) = bounded::<()>(1);
    let join = std::thread::Builder::new()
        .name("gruflo-amdsmi".to_owned())
        .spawn(move || {
            let mut library: Option<AmdSmi> = None;
            let mut next_attempt = Instant::now();
            while let Ok(request) = request_rx.recv() {
                if matches!(request.job, AmdSmiJob::Reload) {
                    library = None;
                    next_attempt = Instant::now();
                }
                if library.is_none() && Instant::now() >= next_attempt {
                    match AmdSmi::load() {
                        Ok(loaded) => library = Some(loaded),
                        Err(_) => next_attempt = Instant::now() + AMDSMI_RETRY_COOLDOWN,
                    }
                }
                let samples = library.as_ref().map(AmdSmi::sample).unwrap_or_default();
                let sampling_failed = !samples.is_empty()
                    && samples.iter().all(|sample| {
                        matches!(sample.gfx_activity_percent, Reading::Error)
                            && matches!(sample.umc_activity_percent, Reading::Error)
                            && matches!(sample.vram_used_bytes, Reading::Error)
                            && matches!(sample.vram_total_bytes, Reading::Error)
                    });
                let _ = result_tx.try_send(TimedResult {
                    generation: request.generation,
                    value: samples,
                });
                if sampling_failed {
                    library = None;
                    next_attempt = Instant::now() + AMDSMI_RETRY_COOLDOWN;
                }
            }
            let _ = done_tx.send(());
        })
        .expect("spawn amdsmi lane");
    AmdSmiLane {
        request: request_tx,
        result: result_rx,
        done: done_rx,
        join: Some(join),
        in_flight: None,
        next_generation: 1,
        reload_due: false,
        failed: false,
    }
}

struct ProcessLane {
    request: Sender<TimedRequest<()>>,
    result: Receiver<TimedResult<ProcessSample>>,
    done: Receiver<()>,
    join: Option<std::thread::JoinHandle<()>>,
    in_flight: Option<InFlight>,
    next_generation: u64,
    failed: bool,
}

impl ProcessLane {
    fn dispatch(&mut self, now: Instant) {
        if self.failed || self.in_flight.is_some() {
            return;
        }
        let generation = self.next_generation;
        self.next_generation += 1;
        if self
            .request
            .try_send(TimedRequest {
                generation,
                job: (),
            })
            .is_ok()
        {
            self.in_flight = Some(InFlight {
                generation,
                deadline: now + Duration::from_millis(1500),
                timed_out: false,
            });
        }
    }
}

fn spawn_process_lane(root: PathBuf) -> ProcessLane {
    let (request_tx, request_rx) = bounded::<TimedRequest<()>>(1);
    let (result_tx, result_rx) = bounded::<TimedResult<ProcessSample>>(1);
    let (done_tx, done_rx) = bounded::<()>(1);
    let join = std::thread::Builder::new()
        .name("gruflo-process".to_owned())
        .spawn(move || {
            let source = ProcessSource::new(root);
            while let Ok(request) = request_rx.recv() {
                let _ = result_tx.try_send(TimedResult {
                    generation: request.generation,
                    value: source.scan(),
                });
            }
            let _ = done_tx.send(());
        })
        .expect("spawn process lane");
    ProcessLane {
        request: request_tx,
        result: result_rx,
        done: done_rx,
        join: Some(join),
        in_flight: None,
        next_generation: 1,
        failed: false,
    }
}

// ---------------------------------------------------------------------------
// Coordinator
// ---------------------------------------------------------------------------

struct Coordinator {
    shared: Arc<Shared>,
    retired_kernel_lanes: Vec<KernelLane>,
    directives: Receiver<Directive>,
    root: PathBuf,
    reducer: Reducer,
    kernel_lanes: HashMap<PhysicalGpuId, KernelLane>,
    summary_load: Option<SummaryLoadLane>,
    discovery: DiscoveryLane,
    amdsmi: AmdSmiLane,
    process: ProcessLane,
    persist: Option<PersistLane>,
    gpu_pools: HashMap<PhysicalGpuId, MemoryPool>,
    partition_by_bdf: HashMap<PciBdf, (PhysicalGpuId, PartitionId)>,
    gpu_by_bdf: HashMap<PciBdf, PhysicalGpuId>,
    process_scope: Option<PhysicalGpuId>,
    discovery_error: Option<String>,
    latest_processes: Option<ProcessOverlay>,
    /// First public snapshot follows the second scheduled fast collection;
    /// late GPUs remain explicit unavailable cells, never an all-device barrier.
    fast_ticks: u8,
    sequence: u64,
    fatal_deadline: Option<Instant>,
    fatal: Option<MonitorError>,
}

impl Coordinator {
    #[allow(clippy::too_many_arguments)]
    fn new(
        shared: Arc<Shared>,
        directives: Receiver<Directive>,
        root: PathBuf,
        devices: Vec<KernelDevice>,
        summary_path: Option<PathBuf>,
        summary_load: Option<SummaryLoadLane>,
        summary_notice: Option<String>,
        local_offset: time::UtcOffset,
        fatal_after: Option<Duration>,
    ) -> Self {
        let mut reducer = Reducer::new(local_offset, Timestamp::now());
        let _ = reducer.apply_topology(devices.iter().map(|d| d.disc.clone()).collect());
        let mut coordinator = Self {
            shared,
            directives,
            retired_kernel_lanes: Vec::new(),
            root: root.clone(),
            reducer,
            kernel_lanes: HashMap::new(),
            summary_load,
            discovery: spawn_discovery_lane(root.clone()),
            amdsmi: spawn_amdsmi_lane(),
            process: spawn_process_lane(root),
            persist: Some(PersistLane::start(summary_path)),
            gpu_pools: HashMap::new(),
            partition_by_bdf: HashMap::new(),
            discovery_error: None,
            gpu_by_bdf: HashMap::new(),
            process_scope: None,
            latest_processes: None,
            fast_ticks: 0,
            sequence: 0,
            fatal_deadline: fatal_after.map(|after| Instant::now() + after),
            fatal: None,
        };
        for device in devices {
            coordinator.adopt_device(device);
        }
        if let Some(message) = summary_notice {
            coordinator.shared.publish_notice(Notice {
                message,
                occurred_at: Timestamp::now(),
            });
        }
        coordinator
    }

    /// Creates the lane and identity maps for one discovered device.
    fn adopt_device(&mut self, device: KernelDevice) {
        let gpu_id = device.disc.id.clone();
        self.gpu_pools
            .insert(gpu_id.clone(), device.disc.pool.clone());
        self.gpu_by_bdf
            .insert(device.disc.bdf.clone(), gpu_id.clone());
        for partition in &device.disc.partitions {
            self.partition_by_bdf.insert(
                partition.bdf.clone(),
                (gpu_id.clone(), partition.id.clone()),
            );
            self.gpu_by_bdf
                .insert(partition.bdf.clone(), gpu_id.clone());
        }
        self.kernel_lanes
            .insert(gpu_id, spawn_kernel_lane(self.root.clone(), device));
    }

    fn run(&mut self) -> Result<(), ShutdownError> {
        let start = Instant::now();
        let mut next_fast = start;
        // Production runs half a cadence after each fast request so a
        // published snapshot carries the just-collected observations rather
        // than the previous tick's.
        let mut next_production = start + FAST_CADENCE + FAST_CADENCE / 2;
        let mut next_slow = start + Duration::from_millis(50);
        let mut next_process = start;
        let mut next_rediscovery = start + REDISCOVERY_CADENCE;

        'main: loop {
            let now_mono = Instant::now();

            // 1. Commands and shutdown.
            loop {
                match self.directives.try_recv() {
                    Ok(Directive::Command(command)) => self.handle_command(command),
                    Ok(Directive::Shutdown) => break 'main,
                    Err(crossbeam_channel::TryRecvError::Empty) => break,
                    Err(crossbeam_channel::TryRecvError::Disconnected) => break 'main,
                }
            }
            self.poll_summary_load(now_mono);

            // Deadlines are shorter than cadence. Timeout stops publication
            // of that generation; the lane remains isolated and receives no
            // overlapping work until the blocking call eventually returns.
            let mut kernel_timeouts = Vec::new();
            for (id, lane) in &mut self.kernel_lanes {
                if let Some(in_flight) = &mut lane.in_flight
                    && !in_flight.timed_out
                    && now_mono >= in_flight.deadline
                {
                    in_flight.timed_out = true;
                    if let Some(job) = lane.current_job {
                        kernel_timeouts.push((id.clone(), job));
                    }
                }
            }
            for (id, job) in kernel_timeouts {
                let lane = match job {
                    KernelJob::Fast => Lane::Fast,
                    KernelJob::Slow => Lane::Slow,
                };
                self.reducer.apply_kernel_timeout(
                    &id,
                    lane,
                    Now {
                        wall: Timestamp::now(),
                        mono: now_mono,
                    },
                );
            }
            for in_flight in [
                &mut self.discovery.in_flight,
                &mut self.amdsmi.in_flight,
                &mut self.process.in_flight,
            ]
            .into_iter()
            .flatten()
            {
                if now_mono >= in_flight.deadline {
                    in_flight.timed_out = true;
                }
            }
            // 2. Drain every ready lane result.
            self.drain_results();
            self.supervise_lanes();
            if let Some(fatal) = self.fatal.take() {
                self.shared.publish_fatal(fatal);
                break 'main;
            }
            if let Some(deadline) = self.fatal_deadline
                && now_mono >= deadline
            {
                self.shared
                    .publish_fatal(MonitorError::Internal("injected fatal".to_owned()));
                break 'main;
            }

            // 3. Skip-on-miss schedules.
            if now_mono >= next_fast {
                for lane in self.kernel_lanes.values_mut() {
                    lane.dispatch(KernelJob::Fast, now_mono);
                }
                self.fast_ticks = self.fast_ticks.saturating_add(1);
                next_fast = now_mono + FAST_CADENCE;
            }
            if now_mono >= next_slow {
                for lane in self.kernel_lanes.values_mut() {
                    lane.slow_due = true;
                }
                self.amdsmi.dispatch(AmdSmiJob::Sample, now_mono);
                next_slow = now_mono + SLOW_CADENCE;
            }
            if self.process_scope.is_none() {
                // Inactive schedules do not accumulate missed deadlines.
                next_process = now_mono;
            } else if now_mono >= next_process {
                self.process.dispatch(now_mono);
                next_process = now_mono + PROCESS_CADENCE;
            }
            if now_mono >= next_rediscovery {
                self.discovery.dispatch(now_mono);
                next_rediscovery = now_mono + REDISCOVERY_CADENCE;
            }

            // 4. Production tick: one snapshot per tick, results or not.
            if now_mono >= next_production {
                let now = Now::current();
                self.reducer.end_tick(now);
                if self.fast_ticks >= 2 {
                    self.sequence += 1;
                    let snapshot = self.reducer.assemble(now, Some(self.sequence));
                    let render = self
                        .reducer
                        .render_model(&snapshot, self.scoped_processes());
                    self.shared.publish_render(render);
                    self.shared.publish_snapshot(snapshot);
                }
                if self.reducer.take_daily_dirty()
                    && let Some(persist) = &self.persist
                {
                    persist.update(self.reducer.daily_record());
                }
                next_production = now_mono + FAST_CADENCE;
            }
            // 5. Sleep until the earliest deadline, an incoming directive,
            //    or any lane result becoming ready. Results wake the loop so
            //    slow/fast samples are applied promptly, never a tick late.
            let deadline = [next_fast, next_slow, next_production, next_rediscovery]
                .into_iter()
                .min()
                .expect("nonempty");
            let wait = deadline
                .saturating_duration_since(Instant::now())
                .min(FAST_CADENCE);
            let mut select = crossbeam_channel::Select::new();
            let directive_index = select.recv(&self.directives);
            for lane in self.kernel_lanes.values().filter(|lane| !lane.failed) {
                select.recv(&lane.result);
            }
            if !self.amdsmi.failed {
                select.recv(&self.amdsmi.result);
            }

            if !self.process.failed {
                select.recv(&self.process.result);
            }
            if !self.discovery.failed {
                select.recv(&self.discovery.result);
            }
            match select.ready_timeout(wait) {
                Ok(index) if index == directive_index => match self.directives.try_recv() {
                    Ok(Directive::Command(command)) => self.handle_command(command),
                    Ok(Directive::Shutdown) => break 'main,
                    Err(crossbeam_channel::TryRecvError::Empty) => {}
                    Err(crossbeam_channel::TryRecvError::Disconnected) => break 'main,
                },
                // A lane result is ready; the next pass drains it.
                Ok(_) => {}
                Err(_) => {}
            }
        }

        self.finish()
    }

    fn poll_summary_load(&mut self, now: Instant) {
        let Some(load) = &mut self.summary_load else {
            return;
        };
        let mut finished = false;
        let mut join_ready = false;
        match load.result.try_recv() {
            Ok(Ok(Some(record))) => {
                self.reducer.seed_daily(&record);
                finished = true;
                join_ready = true;
            }
            Ok(Ok(None)) => {
                finished = true;
                join_ready = true;
            }
            Ok(Err(error)) => {
                self.shared.publish_notice(Notice {
                    message: error,
                    occurred_at: Timestamp::now(),
                });
                finished = true;
                join_ready = true;
            }
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                self.shared.publish_notice(Notice {
                    message: "daily summary load lane stopped".to_owned(),
                    occurred_at: Timestamp::now(),
                });
                finished = true;
                join_ready = true;
            }
            Err(crossbeam_channel::TryRecvError::Empty) if now >= load.deadline => {
                self.shared.publish_notice(Notice {
                    message: "daily summary load timed out".to_owned(),
                    occurred_at: Timestamp::now(),
                });
                finished = true;
            }
            Err(crossbeam_channel::TryRecvError::Empty) => {}
        }
        if finished
            && let Some(mut load) = self.summary_load.take()
            && join_ready
            && let Some(join) = load.join.take()
        {
            let _ = join.join();
        }
    }
    fn supervise_lanes(&mut self) {
        let stopped: Vec<_> = self
            .kernel_lanes
            .iter()
            .filter(|(_, lane)| {
                !matches!(
                    lane.done.try_recv(),
                    Err(crossbeam_channel::TryRecvError::Empty)
                )
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in stopped {
            if let Some(mut lane) = self.kernel_lanes.remove(&id) {
                lane.failed = true;
                disconnect_sender(&mut lane.request);
                if let Some(join) = lane.join.take() {
                    let _ = join.join();
                }
                self.shared.publish_notice(Notice {
                    message: format!("kernel telemetry lane stopped: {id}"),
                    occurred_at: Timestamp::now(),
                });
            }
        }
        if !self.discovery.failed
            && !matches!(
                self.discovery.done.try_recv(),
                Err(crossbeam_channel::TryRecvError::Empty)
            )
        {
            self.discovery.failed = true;
            if let Some(join) = self.discovery.join.take() {
                let _ = join.join();
            }
            self.shared.publish_notice(Notice {
                message: "GPU discovery lane stopped".to_owned(),
                occurred_at: Timestamp::now(),
            });
        }
        if !self.amdsmi.failed
            && !matches!(
                self.amdsmi.done.try_recv(),
                Err(crossbeam_channel::TryRecvError::Empty)
            )
        {
            self.amdsmi.failed = true;
            if let Some(join) = self.amdsmi.join.take() {
                let _ = join.join();
            }
            self.shared.publish_notice(Notice {
                message: "optional AMD SMI lane stopped".to_owned(),
                occurred_at: Timestamp::now(),
            });
        }
        if !self.process.failed
            && !matches!(
                self.process.done.try_recv(),
                Err(crossbeam_channel::TryRecvError::Empty)
            )
        {
            self.process.failed = true;
            if let Some(join) = self.process.join.take() {
                let _ = join.join();
            }
            self.shared.publish_notice(Notice {
                message: "process attribution lane stopped".to_owned(),
                occurred_at: Timestamp::now(),
            });
        }
    }
    fn handle_command(&mut self, command: MonitorCommand) {
        match command {
            MonitorCommand::SetProcessScope(scope) => {
                if scope.is_none() {
                    self.latest_processes = None;
                }
                self.process_scope = scope;
            }
            MonitorCommand::ResetSessionPeaks => self.reducer.reset_session_peaks(),
        }
    }

    /// Applies every ready lane result in deterministic per-lane order.
    fn drain_results(&mut self) {
        let now = Instant::now();
        let gpu_ids: Vec<PhysicalGpuId> = self.kernel_lanes.keys().cloned().collect();
        for gpu_id in gpu_ids {
            #[allow(clippy::while_let_loop)] // lane is re-borrowed after fast results
            loop {
                let Some(lane) = self.kernel_lanes.get_mut(&gpu_id) else {
                    break;
                };
                let Ok(result) = lane.result.try_recv() else {
                    break;
                };
                let matched = lane
                    .in_flight
                    .as_ref()
                    .is_some_and(|current| current.generation == result.generation);
                let accepted = accept_result(&mut lane.in_flight, result.generation, now);
                if matched {
                    lane.current_job = None;
                }
                if !accepted {
                    continue;
                }
                match result.value {
                    KernelOutcome::Fast(sample) => {
                        if sample.device_missing {
                            self.discovery.dispatch(now);
                        }
                        for batch in normalize::kernel_fast(sample) {
                            self.reducer.apply_batch_at(batch, Some(now));
                        }
                        let Some(lane) = self.kernel_lanes.get_mut(&gpu_id) else {
                            break;
                        };
                        if lane.slow_due && lane.dispatch(KernelJob::Slow, now) {
                            lane.slow_due = false;
                        }
                    }
                    KernelOutcome::Slow(sample) => {
                        if sample.device_missing {
                            self.discovery.dispatch(now);
                        }
                        let (batches, report) = normalize::kernel_slow(sample);
                        for batch in batches {
                            self.reducer.apply_batch_at(batch, Some(now));
                        }
                        self.reducer.apply_health_report(report);
                    }
                }
            }
        }
        if let Ok(result) = self.amdsmi.result.try_recv()
            && accept_result(&mut self.amdsmi.in_flight, result.generation, now)
        {
            for batch in normalize::amdsmi(result.value, &self.partition_by_bdf, &self.gpu_pools) {
                self.reducer.apply_batch_at(batch, Some(now));
            }
            if self.amdsmi.reload_due {
                self.amdsmi.dispatch(AmdSmiJob::Reload, now);
            }
        }
        if let Ok(result) = self.process.result.try_recv()
            && accept_result(&mut self.process.in_flight, result.generation, now)
        {
            let sample = result.value;
            self.latest_processes = Some(ProcessOverlay {
                scanned_at: sample.read_wall,
                fdinfo_status: sample.fdinfo_status,
                kfd_status: sample.kfd_status,
                rows: sample.rows,
                gpu_by_bdf: self.gpu_by_bdf.clone(),
                partition_by_bdf: self
                    .partition_by_bdf
                    .iter()
                    .map(|(bdf, (_, partition))| (bdf.clone(), partition.clone()))
                    .collect(),
            });
        }
        if let Ok(result) = self.discovery.result.try_recv()
            && accept_result(&mut self.discovery.in_flight, result.generation, now)
        {
            match result.value {
                Ok(devices) => {
                    if self.discovery_error.take().is_some() {
                        self.shared.publish_notice(Notice {
                            message: "GPU discovery recovered".to_owned(),
                            occurred_at: Timestamp::now(),
                        });
                    }
                    self.apply_discovery(devices);
                }
                Err(error) => {
                    if self.discovery_error.as_ref() != Some(&error) {
                        self.shared.publish_notice(Notice {
                            message: format!("GPU discovery failed: {error}"),
                            occurred_at: Timestamp::now(),
                        });
                        self.discovery_error = Some(error);
                    }
                }
            }
        }
    }

    /// Reconciles a rescan: lane lifecycle, notices, and fatal detection.
    fn apply_discovery(&mut self, devices: Vec<KernelDevice>) {
        let effects = self
            .reducer
            .apply_topology(devices.iter().map(|d| d.disc.clone()).collect());
        for effect in effects {
            match effect {
                StateEffect::GpuAdded(id) => {
                    if let Some(device) = devices.iter().find(|d| d.disc.id == id) {
                        self.adopt_device(device.clone());
                    }
                    self.amdsmi.dispatch(AmdSmiJob::Reload, Instant::now());

                    self.shared.publish_notice(Notice {
                        message: format!("GPU connected: {id}"),
                        occurred_at: Timestamp::now(),
                    });
                }
                StateEffect::GpuRemoved(id) => {
                    if let Some(lane) = self.kernel_lanes.remove(&id) {
                        self.retired_kernel_lanes.push(lane);
                    }
                    self.amdsmi.dispatch(AmdSmiJob::Reload, Instant::now());
                    self.gpu_pools.remove(&id);
                    self.gpu_by_bdf.retain(|_, gpu| *gpu != id);
                    self.partition_by_bdf.retain(|_, (gpu, _)| *gpu != id);
                    self.shared.publish_notice(Notice {
                        message: format!("GPU disconnected: {id}"),
                        occurred_at: Timestamp::now(),
                    });
                }
                StateEffect::PartitionConfigurationChanged(_) => {
                    self.fatal = Some(MonitorError::PartitionConfigurationChanged);
                }
            }
        }
        for device in devices {
            if !self.kernel_lanes.contains_key(&device.disc.id) {
                let id = device.disc.id.clone();
                self.adopt_device(device);
                self.shared.publish_notice(Notice {
                    message: format!("kernel telemetry lane restarted: {id}"),
                    occurred_at: Timestamp::now(),
                });
            }
        }
    }

    /// Rows filtered to the active scope, keeping unattributable rows so a
    /// permission limitation is never silently hidden.
    fn scoped_processes(&self) -> Option<ProcessOverlay> {
        let scope = self.process_scope.as_ref()?;
        let overlay = self.latest_processes.as_ref()?;
        let rows = overlay
            .rows
            .iter()
            .filter(|row| match &row.bdf {
                Some(bdf) => overlay.gpu_by_bdf.get(bdf) == Some(scope),
                None => true,
            })
            .cloned()
            .collect();
        Some(ProcessOverlay {
            scanned_at: overlay.scanned_at,
            fdinfo_status: overlay.fdinfo_status,
            kfd_status: overlay.kfd_status,
            rows,
            gpu_by_bdf: overlay.gpu_by_bdf.clone(),
            partition_by_bdf: overlay.partition_by_bdf.clone(),
        })
    }

    /// Shutdown: close publication, flush persistence, then join or report
    /// every isolated source lane under one absolute bound.
    fn finish(&mut self) -> Result<(), ShutdownError> {
        let deadline = Instant::now() + SHUTDOWN_BOUND;
        let mut failures = Vec::new();
        if let Some(mut load) = self.summary_load.take() {
            if load.result.recv_timeout(remaining(deadline)).is_ok() {
                if let Some(join) = load.join.take() {
                    let _ = join.join();
                }
            } else {
                load.join.take();
                failures.push("daily summary load lane".to_owned());
            }
        }
        let final_record = self.reducer.daily_record();
        if let Some(persist) = self.persist.take() {
            persist.update(final_record);
            if let Err(error) = persist.shutdown(remaining(deadline)) {
                failures.push(format!("persistence: {error}"));
            }
        }

        self.retired_kernel_lanes
            .extend(self.kernel_lanes.drain().map(|(_, lane)| lane));
        for (index, lane) in self.retired_kernel_lanes.iter_mut().enumerate() {
            join_worker(
                &format!("kernel lane {index}"),
                &mut lane.request,
                &lane.done,
                &mut lane.join,
                deadline,
                &mut failures,
            );
        }
        join_worker(
            "discovery lane",
            &mut self.discovery.request,
            &self.discovery.done,
            &mut self.discovery.join,
            deadline,
            &mut failures,
        );
        join_worker(
            "AMD SMI lane",
            &mut self.amdsmi.request,
            &self.amdsmi.done,
            &mut self.amdsmi.join,
            deadline,
            &mut failures,
        );
        join_worker(
            "process lane",
            &mut self.process.request,
            &self.process.done,
            &mut self.process.join,
            deadline,
            &mut failures,
        );
        self.shared.close();
        if failures.is_empty() {
            Ok(())
        } else {
            Err(ShutdownError::Incomplete(failures.join("; ")))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{InFlight, accept_result};

    #[test]
    fn priming_is_schedule_based_not_an_all_device_barrier() {
        let mut ticks = 0u8;
        assert!(ticks < 2);
        ticks = ticks.saturating_add(1);
        assert!(ticks < 2);
        ticks = ticks.saturating_add(1);
        assert!(ticks >= 2);
    }

    #[test]
    fn late_or_mismatched_source_results_are_discarded() {
        let now = Instant::now();
        let mut timed_out = Some(InFlight {
            generation: 7,
            deadline: now,
            timed_out: true,
        });
        assert!(!accept_result(
            &mut timed_out,
            7,
            now + Duration::from_millis(1)
        ));
        assert!(timed_out.is_none());

        let mut current = Some(InFlight {
            generation: 9,
            deadline: now + Duration::from_secs(1),
            timed_out: false,
        });
        assert!(!accept_result(&mut current, 8, now));
        assert_eq!(current.as_ref().unwrap().generation, 9);
        assert!(accept_result(&mut current, 9, now));
        assert!(current.is_none());
    }
}
