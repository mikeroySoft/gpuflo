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

use crate::model::{PartitionId, PciBdf, PhysicalGpuId, Snapshot, Timestamp};
use crate::normalize;
use crate::persist::{self, PersistLane};
use crate::source::amdsmi::AmdSmi;
use crate::source::kernel::{KernelDevice, KernelSource};
use crate::source::process::{ProcessSample, ProcessSource};
use crate::source::{KernelFastSample, KernelSlowSample};
use crate::state::reducer::{Now, Reducer};
use crate::state::{ProcessOverlay, RenderModel, StateEffect};

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
    #[doc(hidden)]
    pub host_root: Option<PathBuf>,
    #[doc(hidden)]
    pub fatal_after: Option<Duration>,
}

impl MonitorOptions {
    /// Default options: persistence disabled.
    pub fn new() -> Self {
        Self::default()
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
    done: Receiver<()>,
    join: Option<std::thread::JoinHandle<()>>,
}

enum Directive {
    Command(MonitorCommand),
    Shutdown,
}

impl Monitor {
    /// Performs host/device preflight and starts the coordinator thread.
    pub fn start(options: MonitorOptions) -> Result<Self, StartError> {
        if !cfg!(target_os = "linux") {
            return Err(StartError::UnsupportedHost);
        }
        let root = options
            .host_root
            .clone()
            .unwrap_or_else(|| PathBuf::from("/"));
        let kernel = KernelSource::new(root.clone());
        let devices = kernel.discover();
        if devices.is_empty() {
            return Err(StartError::NoGpu);
        }

        // Capture the local offset before spawning threads; day rollover
        // keeps this startup offset for the whole session.
        let local_offset = time::UtcOffset::current_local_offset().unwrap_or(time::UtcOffset::UTC);

        let shared = Arc::new(Shared::new());
        let (directive_tx, directive_rx) = bounded::<Directive>(1);
        let (done_tx, done_rx) = bounded::<()>(1);
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
                    local_offset,
                    options.fatal_after,
                );
                coordinator.run();
                let _ = done_tx.send(());
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
        let _ = self.directives.send(Directive::Shutdown);
        let finished = self.done.recv_timeout(SHUTDOWN_BOUND).is_ok();
        if finished {
            if let Some(join) = self.join.take() {
                let _ = join.join();
            }
            Ok(())
        } else {
            self.join.take(); // Detach the nonresponsive coordinator.
            Err(ShutdownError::Incomplete(
                "coordinator did not stop within the shutdown bound".to_owned(),
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// Lanes
// ---------------------------------------------------------------------------

enum KernelJob {
    Fast,
    Slow,
}

enum KernelOutcome {
    Fast(KernelFastSample),
    Slow(KernelSlowSample),
}

struct KernelLane {
    request: Sender<KernelJob>,
    result: Receiver<KernelOutcome>,
    /// Slow work waits until the lane is idle after a fast result.
    slow_due: bool,
}

fn spawn_kernel_lane(root: PathBuf, device: KernelDevice) -> KernelLane {
    let (request_tx, request_rx) = bounded::<KernelJob>(1);
    let (result_tx, result_rx) = bounded::<KernelOutcome>(1);
    std::thread::Builder::new()
        .name(format!("gruflo-kernel-{}", device.disc.bdf))
        .spawn(move || {
            let mut source = KernelSource::new(root);
            while let Ok(job) = request_rx.recv() {
                let outcome = match job {
                    KernelJob::Fast => KernelOutcome::Fast(source.collect_fast(&device)),
                    KernelJob::Slow => KernelOutcome::Slow(source.collect_slow(&device)),
                };
                // Nonblocking: an unconsumed older result wins; the
                // coordinator will request again on schedule.
                let _ = result_tx.try_send(outcome);
            }
        })
        .expect("spawn kernel lane");
    KernelLane {
        request: request_tx,
        result: result_rx,
        slow_due: false,
    }
}

struct DiscoveryLane {
    request: Sender<()>,
    result: Receiver<Vec<KernelDevice>>,
}

fn spawn_discovery_lane(root: PathBuf) -> DiscoveryLane {
    let (request_tx, request_rx) = bounded::<()>(1);
    let (result_tx, result_rx) = bounded::<Vec<KernelDevice>>(1);
    std::thread::Builder::new()
        .name("gruflo-discovery".to_owned())
        .spawn(move || {
            let source = KernelSource::new(root);
            while request_rx.recv().is_ok() {
                let _ = result_tx.try_send(source.discover());
            }
        })
        .expect("spawn discovery lane");
    DiscoveryLane {
        request: request_tx,
        result: result_rx,
    }
}

struct AmdSmiLane {
    request: Sender<()>,
    result: Receiver<Vec<crate::source::amdsmi::AmdSmiSample>>,
}

fn spawn_amdsmi_lane() -> AmdSmiLane {
    let (request_tx, request_rx) = bounded::<()>(1);
    let (result_tx, result_rx) = bounded::<Vec<crate::source::amdsmi::AmdSmiSample>>(1);
    std::thread::Builder::new()
        .name("gruflo-amdsmi".to_owned())
        .spawn(move || {
            // All library state and FFI stays inside this lane. A failed
            // load opens a cooldown; kernel lanes are never affected.
            let mut library: Option<AmdSmi> = None;
            let mut next_attempt = Instant::now();
            while request_rx.recv().is_ok() {
                if library.is_none() && Instant::now() >= next_attempt {
                    match AmdSmi::load() {
                        Ok(loaded) => library = Some(loaded),
                        Err(_) => next_attempt = Instant::now() + AMDSMI_RETRY_COOLDOWN,
                    }
                }
                let samples = library.as_ref().map(AmdSmi::sample).unwrap_or_default();
                let _ = result_tx.try_send(samples);
            }
            // Dropping the library runs shutdown exactly once.
        })
        .expect("spawn amdsmi lane");
    AmdSmiLane {
        request: request_tx,
        result: result_rx,
    }
}

struct ProcessLane {
    request: Sender<()>,
    result: Receiver<ProcessSample>,
}

fn spawn_process_lane(root: PathBuf) -> ProcessLane {
    let (request_tx, request_rx) = bounded::<()>(1);
    let (result_tx, result_rx) = bounded::<ProcessSample>(1);
    std::thread::Builder::new()
        .name("gruflo-process".to_owned())
        .spawn(move || {
            let source = ProcessSource::new(root);
            while request_rx.recv().is_ok() {
                let _ = result_tx.try_send(source.scan());
            }
        })
        .expect("spawn process lane");
    ProcessLane {
        request: request_tx,
        result: result_rx,
    }
}

// ---------------------------------------------------------------------------
// Coordinator
// ---------------------------------------------------------------------------

struct Coordinator {
    shared: Arc<Shared>,
    directives: Receiver<Directive>,
    root: PathBuf,
    reducer: Reducer,
    kernel_lanes: HashMap<PhysicalGpuId, KernelLane>,
    discovery: DiscoveryLane,
    amdsmi: AmdSmiLane,
    process: ProcessLane,
    persist: Option<PersistLane>,
    /// BDF → (gpu, partition) for AMD SMI processor mapping.
    partition_by_bdf: HashMap<PciBdf, (PhysicalGpuId, PartitionId)>,
    gpu_by_bdf: HashMap<PciBdf, PhysicalGpuId>,
    process_scope: Option<PhysicalGpuId>,
    latest_processes: Option<ProcessOverlay>,
    fast_samples_applied: u64,
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
        local_offset: time::UtcOffset,
        fatal_after: Option<Duration>,
    ) -> Self {
        let mut reducer = Reducer::new(local_offset, Timestamp::now());
        // Startup load happens once before normal accumulation.
        if let Some(path) = &summary_path
            && let Some(record) = persist::load(path)
        {
            reducer.seed_daily(&record);
        }
        // Initial topology: effects are startup facts, not transitions.
        let _ = reducer.apply_topology(devices.iter().map(|d| d.disc.clone()).collect());

        let mut coordinator = Self {
            shared,
            directives,
            root: root.clone(),
            reducer,
            kernel_lanes: HashMap::new(),
            discovery: spawn_discovery_lane(root.clone()),
            amdsmi: spawn_amdsmi_lane(),
            process: spawn_process_lane(root),
            persist: Some(PersistLane::start(summary_path)),
            partition_by_bdf: HashMap::new(),
            gpu_by_bdf: HashMap::new(),
            process_scope: None,
            latest_processes: None,
            fast_samples_applied: 0,
            sequence: 0,
            fatal_deadline: fatal_after.map(|after| Instant::now() + after),
            fatal: None,
        };
        for device in devices {
            coordinator.adopt_device(device);
        }
        coordinator
    }

    /// Creates the lane and identity maps for one discovered device.
    fn adopt_device(&mut self, device: KernelDevice) {
        let gpu_id = device.disc.id.clone();
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

    fn run(&mut self) {
        let start = Instant::now();
        let mut next_fast = start;
        let mut next_production = start + FAST_CADENCE;
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

            // 2. Drain every ready lane result.
            self.drain_results();
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
                for lane in self.kernel_lanes.values() {
                    let _ = lane.request.try_send(KernelJob::Fast);
                }
                while next_fast <= now_mono {
                    next_fast += FAST_CADENCE;
                }
            }
            if now_mono >= next_slow {
                for lane in self.kernel_lanes.values_mut() {
                    lane.slow_due = true;
                }
                let _ = self.amdsmi.request.try_send(());
                while next_slow <= now_mono {
                    next_slow += SLOW_CADENCE;
                }
            }
            if self.process_scope.is_some() && now_mono >= next_process {
                let _ = self.process.request.try_send(());
                while next_process <= now_mono {
                    next_process += PROCESS_CADENCE;
                }
            }
            if now_mono >= next_rediscovery {
                let _ = self.discovery.request.try_send(());
                while next_rediscovery <= now_mono {
                    next_rediscovery += REDISCOVERY_CADENCE;
                }
            }

            // 4. Production tick: one snapshot per tick, results or not.
            if now_mono >= next_production {
                let now = Now::current();
                self.reducer.end_tick(now);
                if self.fast_samples_applied >= 2 {
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
                while next_production <= now_mono {
                    next_production += FAST_CADENCE;
                }
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
            for lane in self.kernel_lanes.values() {
                select.recv(&lane.result);
            }
            select.recv(&self.amdsmi.result);
            select.recv(&self.process.result);
            select.recv(&self.discovery.result);
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

        self.finish();
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
        let gpu_ids: Vec<PhysicalGpuId> = self.kernel_lanes.keys().cloned().collect();
        for gpu_id in gpu_ids {
            loop {
                let Some(lane) = self.kernel_lanes.get_mut(&gpu_id) else {
                    break;
                };
                match lane.result.try_recv() {
                    Ok(KernelOutcome::Fast(sample)) => {
                        self.fast_samples_applied += 1;
                        for batch in normalize::kernel_fast(sample) {
                            self.reducer.apply_batch(batch);
                        }
                        // Fast has priority; due slow work dispatches only
                        // when the lane is idle after a fast result.
                        let Some(lane) = self.kernel_lanes.get_mut(&gpu_id) else {
                            break;
                        };
                        if lane.slow_due && lane.request.try_send(KernelJob::Slow).is_ok() {
                            lane.slow_due = false;
                        }
                    }
                    Ok(KernelOutcome::Slow(sample)) => {
                        let (batches, report) = normalize::kernel_slow(sample);
                        for batch in batches {
                            self.reducer.apply_batch(batch);
                        }
                        self.reducer.apply_health_report(report);
                    }
                    Err(_) => break,
                }
            }
        }
        if let Ok(samples) = self.amdsmi.result.try_recv() {
            for batch in normalize::amdsmi(samples, &self.partition_by_bdf) {
                self.reducer.apply_batch(batch);
            }
        }
        if let Ok(sample) = self.process.result.try_recv() {
            self.latest_processes = Some(ProcessOverlay {
                scanned_at: sample.read_wall,
                rows: sample.rows,
                gpu_by_bdf: self.gpu_by_bdf.clone(),
            });
        }
        if let Ok(devices) = self.discovery.result.try_recv() {
            self.apply_discovery(devices);
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
                    self.shared.publish_notice(Notice {
                        message: format!("GPU connected: {id}"),
                        occurred_at: Timestamp::now(),
                    });
                }
                StateEffect::GpuRemoved(id) => {
                    self.kernel_lanes.remove(&id);
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
            rows,
            gpu_by_bdf: overlay.gpu_by_bdf.clone(),
        })
    }

    /// Shutdown: close publication, drop lanes, flush persistence bounded.
    fn finish(&mut self) {
        // Dropping request senders disconnects every lane loop.
        self.kernel_lanes.clear();
        let final_record = self.reducer.daily_record();
        if let Some(persist) = self.persist.take() {
            persist.update(final_record);
            let _ = persist.shutdown(SHUTDOWN_BOUND);
        }
        self.shared.close();
    }
}
