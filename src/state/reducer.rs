//! Deterministic transitions: freshness, precedence, topology, histories,
//! peaks, daily rollover, and health assembly. No I/O, no channels.

use std::time::Instant;

use time::UtcOffset;

use super::health::{self, HealthCandidate};
use super::history::{DailyAccumulator, DailySummaryRecord, Ring};
use super::{
    DiscoveredGpu, DiscoveredPartition, Lane, MetricBatch, MetricKey, Origin, PartitionMetric,
    RenderGpu, RenderModel, SocketMetric, SourceHealthReport, StateEffect, Value,
};
use crate::model::{
    Health, HealthCategory, Memory, Observation, ObservationState, Partition, PhysicalGpu,
    PhysicalGpuId, Snapshot, Timestamp,
};

/// One coherent pair of wall and monotonic time.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Now {
    pub wall: Timestamp,
    pub mono: Instant,
}

impl Now {
    /// Captures the current wall and monotonic clocks.
    pub fn current() -> Self {
        Self {
            wall: Timestamp::now(),
            mono: Instant::now(),
        }
    }
}

#[derive(Debug, Clone)]
enum CellState {
    /// Never observed.
    Empty,
    /// A current numeric value within its freshness window.
    Fresh {
        value: Value,
        wall: Timestamp,
        mono: Instant,
        origin: Origin,
        lane: Lane,
    },
    /// No current value.
    Unavailable {
        state: ObservationState,
        last_good: Option<Timestamp>,
        origin: Origin,
    },
}

/// One metric cell: current canonical observation plus freshness bookkeeping.
#[derive(Debug, Clone)]
struct Cell {
    state: CellState,
    /// Most recent kernel read failure while a fresh value was retained.
    last_failure: Option<(ObservationState, Timestamp)>,
    /// Time of the most recent source attempt represented by this cell.
    last_attempt_wall: Option<Timestamp>,
}

impl Cell {
    fn new() -> Self {
        Self {
            state: CellState::Empty,
            last_failure: None,
            last_attempt_wall: None,
        }
    }

    /// Applies one normalized outcome. Returns the fresh value when one was
    /// accepted (used to stage history/peaks).
    fn apply(
        &mut self,
        outcome: &Result<Value, ObservationState>,
        origin: Origin,
        lane: Lane,
        wall: Timestamp,
        mono: Instant,
    ) -> Option<Value> {
        match outcome {
            Ok(value) => {
                if origin == Origin::AmdSmi {
                    let may_enrich = match &self.state {
                        CellState::Empty => true,
                        CellState::Fresh {
                            origin: Origin::AmdSmi,
                            ..
                        } => true,
                        CellState::Unavailable {
                            state,
                            origin: Origin::Kernel,
                            ..
                        } => state == &ObservationState::UNSUPPORTED_HARDWARE,
                        _ => false,
                    };
                    if !may_enrich {
                        return None;
                    }
                }
                self.last_attempt_wall = Some(wall);
                self.state = CellState::Fresh {
                    value: *value,
                    wall,
                    mono,
                    origin,
                    lane,
                };
                self.last_failure = None;
                Some(*value)
            }
            Err(state) => {
                // Optional-source failures never mutate authoritative kernel
                // state or a prior enrichment value.
                if origin == Origin::AmdSmi {
                    return None;
                }
                self.last_attempt_wall = Some(wall);
                match &self.state {
                    CellState::Fresh {
                        origin: Origin::Kernel,
                        ..
                    } => self.last_failure = Some((state.clone(), wall)),
                    CellState::Fresh {
                        origin: Origin::AmdSmi,
                        ..
                    } if state == &ObservationState::UNSUPPORTED_HARDWARE => {}
                    CellState::Unavailable { state: current, .. }
                        if current == &ObservationState::STALE =>
                    {
                        self.last_failure = Some((state.clone(), wall));
                    }
                    _ => {
                        self.state = CellState::Unavailable {
                            state: state.clone(),
                            last_good: None,
                            origin: Origin::Kernel,
                        };
                        self.last_failure = None;
                    }
                }
                None
            }
        }
    }

    /// Applies the freshness rule at `now`: a previously good value always
    /// expires to `stale` with its last good source time. Failure evidence is
    /// retained separately for health/source diagnostics while the value is
    /// still fresh.
    fn evaluate(&mut self, now_mono: Instant) {
        if let CellState::Fresh {
            mono,
            lane,
            wall,
            origin,
            ..
        } = &self.state
            && now_mono.duration_since(*mono) >= lane.stale_after()
        {
            self.state = CellState::Unavailable {
                state: ObservationState::STALE,
                last_good: Some(*wall),
                origin: *origin,
            };
        }
    }

    fn fresh_f64(&self) -> Option<(f64, Timestamp)> {
        match &self.state {
            CellState::Fresh {
                value: Value::F64(v),
                wall,
                ..
            } => Some((*v, *wall)),
            CellState::Fresh {
                value: Value::U64(v),
                wall,
                ..
            } => Some((*v as f64, *wall)),
            _ => None,
        }
    }

    fn fresh_u64(&self) -> Option<(u64, Timestamp)> {
        match &self.state {
            CellState::Fresh {
                value: Value::U64(v),
                wall,
                ..
            } => Some((*v, *wall)),
            _ => None,
        }
    }

    fn unavailable(&self) -> Observation<f64> {
        match &self.state {
            CellState::Empty => Observation::unavailable(ObservationState::SOURCE_ERROR),
            CellState::Unavailable {
                state, last_good, ..
            } => Observation::Unavailable {
                state: state.clone(),
                observed_at: *last_good,
            },
            CellState::Fresh { .. } => unreachable!("caller checked freshness"),
        }
    }

    fn observation_f64(&self) -> Observation<f64> {
        match self.fresh_f64() {
            Some((value, wall)) => Observation::value(value, wall),
            None => self.unavailable(),
        }
    }

    fn observation_u64(&self) -> Observation<u64> {
        match self.fresh_u64() {
            Some((value, wall)) => Observation::value(value, wall),
            None => match &self.state {
                CellState::Empty => Observation::unavailable(ObservationState::SOURCE_ERROR),
                CellState::Unavailable {
                    state, last_good, ..
                } => Observation::Unavailable {
                    state: state.clone(),
                    observed_at: *last_good,
                },
                CellState::Fresh { .. } => Observation::unavailable(ObservationState::SOURCE_ERROR),
            },
        }
    }

    /// The state feeding a derived telemetry health candidate, if any.
    fn trouble(&self) -> Option<(&ObservationState, Option<Timestamp>)> {
        match &self.state {
            CellState::Empty => Some((&ObservationState::SOURCE_ERROR, self.last_attempt_wall)),
            CellState::Unavailable {
                state, last_good, ..
            } if state != &ObservationState::REPORTED_BY_PRIMARY_PARTITION => {
                Some((state, last_good.or(self.last_attempt_wall)))
            }
            CellState::Fresh { .. } => self
                .last_failure
                .as_ref()
                .map(|(state, at)| (state, Some(*at))),
            _ => None,
        }
    }

    fn record_failure(&mut self, state: ObservationState, wall: Timestamp, origin: Origin) {
        self.last_attempt_wall = Some(wall);
        match &self.state {
            CellState::Fresh { .. } => {
                self.last_failure = Some((state, wall));
            }
            CellState::Unavailable { state: current, .. }
                if current == &ObservationState::STALE =>
            {
                self.last_failure = Some((state, wall));
            }
            CellState::Empty | CellState::Unavailable { .. } => {
                self.state = CellState::Unavailable {
                    state,
                    last_good: None,
                    origin,
                };
                self.last_failure = None;
            }
        }
    }

    fn timeout(&mut self, wall: Timestamp) {
        self.last_attempt_wall = Some(wall);
        match self.state {
            CellState::Empty => {
                self.state = CellState::Unavailable {
                    state: ObservationState::SOURCE_ERROR,
                    last_good: None,
                    origin: Origin::Kernel,
                };
                self.last_failure = None;
            }
            _ => {
                self.last_failure = Some((ObservationState::SOURCE_ERROR, wall));
            }
        }
    }

    fn health_time(&self) -> Option<Timestamp> {
        match &self.state {
            CellState::Fresh { wall, .. } => Some(*wall),
            CellState::Unavailable { last_good, .. } => last_good.or(self.last_attempt_wall),
            CellState::Empty => self.last_attempt_wall,
        }
    }
}

#[derive(Debug)]
struct PartState {
    disc: DiscoveredPartition,
    activity: Cell,
    mem_used: Cell,
    mem_total: Cell,
    gfx_clock: Cell,
    mem_ctl: Cell,
    activity_ring: Ring,
    memory_ring: Ring,
    staged_activity: Option<f64>,
    staged_memory: Option<f64>,
    session_peak_activity: Option<f64>,
}

impl PartState {
    fn new(disc: DiscoveredPartition) -> Self {
        Self {
            disc,
            activity: Cell::new(),
            mem_used: Cell::new(),
            mem_total: Cell::new(),
            gfx_clock: Cell::new(),
            mem_ctl: Cell::new(),
            activity_ring: Ring::new(),
            memory_ring: Ring::new(),
            staged_activity: None,
            staged_memory: None,
            session_peak_activity: None,
        }
    }

    fn cell(&mut self, metric: PartitionMetric) -> &mut Cell {
        match metric {
            PartitionMetric::ActivityPercent => &mut self.activity,
            PartitionMetric::MemUsedBytes => &mut self.mem_used,
            PartitionMetric::MemTotalBytes => &mut self.mem_total,
            PartitionMetric::GfxClockMhz => &mut self.gfx_clock,
            PartitionMetric::MemCtlActivityPercent => &mut self.mem_ctl,
        }
    }
}

#[derive(Debug)]
struct StoredHealthReport {
    candidates: Vec<HealthCandidate>,
    observed_mono: Instant,
    lane: Lane,
}

impl StoredHealthReport {
    fn active(&self, now: Instant) -> bool {
        now.duration_since(self.observed_mono) < self.lane.stale_after()
    }
}

#[derive(Debug)]
struct GpuState {
    disc: DiscoveredGpu,
    hotspot: Cell,
    temp_limit: Cell,
    power: Cell,
    power_cap: Cell,
    partitions: Vec<PartState>,
    kernel_health: Option<StoredHealthReport>,
    amdsmi_health: Option<StoredHealthReport>,
}

impl GpuState {
    fn new(disc: DiscoveredGpu) -> Self {
        let partitions = disc
            .partitions
            .iter()
            .cloned()
            .map(PartState::new)
            .collect();
        Self {
            disc,
            hotspot: Cell::new(),
            temp_limit: Cell::new(),
            power: Cell::new(),
            power_cap: Cell::new(),
            partitions,
            kernel_health: None,
            amdsmi_health: None,
        }
    }

    fn socket_cell(&mut self, metric: SocketMetric) -> &mut Cell {
        match metric {
            SocketMetric::HotspotCelsius => &mut self.hotspot,
            SocketMetric::LimitCelsius => &mut self.temp_limit,
            SocketMetric::SocketWatts => &mut self.power,
            SocketMetric::CapWatts => &mut self.power_cap,
        }
    }
}

/// The coordinator-owned deterministic state machine.
#[derive(Debug)]
pub(crate) struct Reducer {
    gpus: Vec<GpuState>,
    daily: DailyAccumulator,
    local_offset: UtcOffset,
}

impl Reducer {
    /// Creates the reducer with the startup local offset for day rollover.
    pub fn new(local_offset: UtcOffset, now_wall: Timestamp) -> Self {
        let date = now_wall.as_odt().to_offset(local_offset).date();
        Self {
            gpus: Vec::new(),
            daily: DailyAccumulator::new(date),
            local_offset,
        }
    }

    /// Seeds today's summary from a persisted record.
    pub fn seed_daily(&mut self, record: &DailySummaryRecord) {
        self.daily.seed(record);
    }

    /// Reconciles discovered topology with owned state. Preserves existing
    /// per-GPU state across rescans; returns factual lifecycle effects.
    pub fn apply_topology(&mut self, mut discovered: Vec<DiscoveredGpu>) -> Vec<StateEffect> {
        discovered.sort_by(|a, b| a.bdf.as_str().cmp(b.bdf.as_str()));
        let mut effects = Vec::new();
        let mut next = Vec::with_capacity(discovered.len());
        for gpu in discovered {
            match self.gpus.iter().position(|g| g.disc.id == gpu.id) {
                Some(index) => {
                    let mut existing = self.gpus.remove(index);
                    let topology_changed = existing.disc.pool != gpu.pool
                        || existing.disc.partitions != gpu.partitions;
                    if topology_changed {
                        effects.push(StateEffect::PartitionConfigurationChanged(gpu.id.clone()));
                    }
                    existing.disc = gpu;
                    next.push(existing);
                }
                None => {
                    effects.push(StateEffect::GpuAdded(gpu.id.clone()));
                    next.push(GpuState::new(gpu));
                }
            }
        }
        for removed in self.gpus.drain(..) {
            effects.push(StateEffect::GpuRemoved(removed.disc.id));
        }
        self.gpus = next;
        effects
    }

    /// Applies one normalized metric batch to the addressed scope.
    #[cfg(test)]
    pub fn apply_batch(&mut self, batch: MetricBatch) {
        self.apply_batch_at(batch, None);
    }

    /// Applies a batch at a coordinator monotonic time. A result already
    /// beyond freshness is discarded before history/peak staging.
    pub fn apply_batch_at(&mut self, batch: MetricBatch, now_mono: Option<Instant>) {
        if now_mono
            .is_some_and(|now| now.duration_since(batch.observed_mono) >= batch.lane.stale_after())
        {
            return;
        }
        let Some(gpu) = self.gpus.iter_mut().find(|g| g.disc.id == batch.gpu) else {
            return; // Late result for a removed GPU: discard.
        };
        if let Some((raw, resolution)) = batch.energy_accumulator {
            self.daily
                .observe_energy(batch.gpu.as_str(), raw, resolution);
        }
        match &batch.partition {
            None => {
                for result in &batch.results {
                    let MetricKey::Socket(metric) = result.metric else {
                        continue;
                    };
                    gpu.socket_cell(metric).apply(
                        &result.outcome,
                        batch.origin,
                        batch.lane,
                        batch.observed_wall,
                        batch.observed_mono,
                    );
                }
            }
            Some(partition_id) => {
                let Some(part) = gpu
                    .partitions
                    .iter_mut()
                    .find(|p| p.disc.id == *partition_id)
                else {
                    return;
                };
                let mut used_outcome = None;
                let mut total_outcome = None;
                for result in &batch.results {
                    let MetricKey::Partition(metric) = result.metric else {
                        continue;
                    };
                    match metric {
                        PartitionMetric::MemUsedBytes => {
                            used_outcome = Some(result.outcome.clone());
                        }
                        PartitionMetric::MemTotalBytes => {
                            total_outcome = Some(result.outcome.clone());
                        }
                        _ => {
                            let accepted = part.cell(metric).apply(
                                &result.outcome,
                                batch.origin,
                                batch.lane,
                                batch.observed_wall,
                                batch.observed_mono,
                            );
                            if let (PartitionMetric::ActivityPercent, Some(Value::F64(v))) =
                                (metric, accepted)
                            {
                                part.staged_activity = Some(v);
                            }
                        }
                    }
                }
                match (used_outcome, total_outcome) {
                    (Some(Ok(Value::U64(used))), Some(Ok(Value::U64(total))))
                        if total > 0 && used <= total =>
                    {
                        let accepted_used = part.mem_used.apply(
                            &Ok(Value::U64(used)),
                            batch.origin,
                            batch.lane,
                            batch.observed_wall,
                            batch.observed_mono,
                        );
                        let accepted_total = part.mem_total.apply(
                            &Ok(Value::U64(total)),
                            batch.origin,
                            batch.lane,
                            batch.observed_wall,
                            batch.observed_mono,
                        );
                        if accepted_used.is_some() && accepted_total.is_some() {
                            part.staged_memory = Some(used as f64 / total as f64 * 100.0);
                        }
                    }
                    (Some(Err(used)), Some(Err(total))) => {
                        part.mem_used.apply(
                            &Err(used),
                            batch.origin,
                            batch.lane,
                            batch.observed_wall,
                            batch.observed_mono,
                        );
                        part.mem_total.apply(
                            &Err(total),
                            batch.origin,
                            batch.lane,
                            batch.observed_wall,
                            batch.observed_mono,
                        );
                    }
                    (None, None) => {}
                    (used, total) => {
                        let failure = used
                            .as_ref()
                            .and_then(|outcome| outcome.as_ref().err())
                            .or_else(|| total.as_ref().and_then(|outcome| outcome.as_ref().err()))
                            .cloned()
                            .unwrap_or(ObservationState::SOURCE_ERROR);
                        part.mem_used.record_failure(
                            failure.clone(),
                            batch.observed_wall,
                            batch.origin,
                        );
                        part.mem_total
                            .record_failure(failure, batch.observed_wall, batch.origin);
                    }
                }
            }
        }
    }

    /// Applies a kernel operation timeout as a non-staging failure event.
    /// Current values remain until freshness expiry; never-observed cells
    /// become explicit source errors and contribute telemetry health.
    pub fn apply_kernel_timeout(&mut self, gpu_id: &PhysicalGpuId, lane: Lane, now: Now) {
        let Some(gpu) = self.gpus.iter_mut().find(|gpu| &gpu.disc.id == gpu_id) else {
            return;
        };
        match lane {
            Lane::Fast => {
                for cell in [&mut gpu.hotspot, &mut gpu.power] {
                    cell.timeout(now.wall);
                }
                for part in &mut gpu.partitions {
                    for cell in [
                        &mut part.activity,
                        &mut part.mem_used,
                        &mut part.mem_total,
                        &mut part.mem_ctl,
                    ] {
                        cell.timeout(now.wall);
                    }
                }
            }
            Lane::Slow => {
                for cell in [&mut gpu.temp_limit, &mut gpu.power_cap] {
                    cell.timeout(now.wall);
                }
                for part in &mut gpu.partitions {
                    part.gfx_clock.timeout(now.wall);
                }
            }
        }
    }

    /// Replaces the source-backed candidate set for one GPU and origin.
    pub fn apply_health_report(&mut self, report: SourceHealthReport) {
        let Some(gpu) = self.gpus.iter_mut().find(|g| g.disc.id == report.gpu) else {
            return;
        };
        let stored = StoredHealthReport {
            candidates: report.candidates,
            observed_mono: report.observed_mono,
            lane: report.lane,
        };
        match report.origin {
            Origin::Kernel => gpu.kernel_health = Some(stored),
            Origin::AmdSmi => gpu.amdsmi_health = Some(stored),
        }
    }

    /// Clears session peaks (public `ResetSessionPeaks` command).
    pub fn reset_session_peaks(&mut self) {
        for gpu in &mut self.gpus {
            for part in &mut gpu.partitions {
                part.session_peak_activity = None;
            }
        }
    }

    /// Closes one production tick: pushes staged fresh values (or gaps) into
    /// rings, updates peaks and the daily summary, and rolls the local day.
    pub fn end_tick(&mut self, now: Now) {
        let date = now.wall.as_odt().to_offset(self.local_offset).date();
        self.daily.roll(date);
        for gpu in &mut self.gpus {
            for part in &mut gpu.partitions {
                let activity = part.staged_activity.take();
                let memory = part.staged_memory.take();
                part.activity_ring.push(activity);
                part.memory_ring.push(memory);
                if let Some(value) = activity {
                    if part.session_peak_activity.is_none_or(|peak| value > peak) {
                        part.session_peak_activity = Some(value);
                    }
                    if part.disc.is_primary {
                        self.daily.observe_activity(gpu.disc.id.as_str(), value);
                    }
                }
                if let Some(value) = memory
                    && part.disc.is_primary
                {
                    self.daily.observe_memory(gpu.disc.id.as_str(), value);
                }
            }
        }
    }

    /// Whether the daily summary changed since last asked.
    pub fn take_daily_dirty(&mut self) -> bool {
        self.daily.take_dirty()
    }

    /// The current persistable daily record.
    pub fn daily_record(&self) -> DailySummaryRecord {
        self.daily.record()
    }

    /// Number of currently confirmed physical GPUs.
    #[cfg(test)]
    pub fn gpu_count(&self) -> usize {
        self.gpus.len()
    }

    /// Evaluates freshness at `now` and assembles the all-GPU snapshot.
    pub fn assemble(&mut self, now: Now, sequence: Option<u64>) -> Snapshot {
        let mut gpus = Vec::with_capacity(self.gpus.len());
        for (gpu_index, gpu) in self.gpus.iter_mut().enumerate() {
            for cell in [
                &mut gpu.hotspot,
                &mut gpu.temp_limit,
                &mut gpu.power,
                &mut gpu.power_cap,
            ] {
                cell.evaluate(now.mono);
            }
            for part in &mut gpu.partitions {
                for cell in [
                    &mut part.activity,
                    &mut part.mem_used,
                    &mut part.mem_total,
                    &mut part.gfx_clock,
                    &mut part.mem_ctl,
                ] {
                    cell.evaluate(now.mono);
                }
            }
            let health = assemble_health(gpu, now);
            let partitions = gpu
                .partitions
                .iter()
                .enumerate()
                .map(|(index, part)| Partition {
                    id: part.disc.id.clone(),
                    index: index as u32,
                    is_primary: part.disc.is_primary,
                    activity_percent: part.activity.observation_f64(),
                    memory: Memory {
                        pool: gpu.disc.pool.clone(),
                        used_bytes: part.mem_used.observation_u64(),
                        total_bytes: part.mem_total.observation_u64(),
                        occupancy_percent: occupancy(&part.mem_used, &part.mem_total),
                    },
                    gfx_clock_mhz: part.gfx_clock.observation_f64(),
                    memory_controller_activity_percent: part.mem_ctl.observation_f64(),
                })
                .collect();
            gpus.push(PhysicalGpu {
                id: gpu.disc.id.clone(),
                index: gpu_index as u32,
                bdf: gpu.disc.bdf.clone(),
                name: gpu.disc.name.clone(),
                uuid: gpu.disc.uuid.clone(),
                serial: gpu.disc.serial.clone(),
                health,
                temperature: crate::model::Temperature {
                    hotspot_celsius: gpu.hotspot.observation_f64(),
                    limit_celsius: gpu.temp_limit.observation_f64(),
                },
                power: crate::model::Power {
                    socket_watts: gpu.power.observation_f64(),
                    cap_watts: gpu.power_cap.observation_f64(),
                },
                partitions,
            });
        }
        Snapshot::new(now.wall, sequence, gpus)
    }

    /// Projects the private presentation model aligned with `snapshot`.
    pub fn render_model(
        &self,
        snapshot: &Snapshot,
        processes: Option<super::ProcessOverlay>,
    ) -> RenderModel {
        let gpus = self
            .gpus
            .iter()
            .map(|gpu| {
                let primary = gpu
                    .partitions
                    .iter()
                    .find(|p| p.disc.is_primary)
                    .or_else(|| gpu.partitions.first());
                RenderGpu {
                    id: gpu.disc.id.clone(),
                    activity_history: primary
                        .map(|p| p.activity_ring.to_vec())
                        .unwrap_or_default(),
                    memory_history: primary.map(|p| p.memory_ring.to_vec()).unwrap_or_default(),
                    session_peak_activity: primary.and_then(|p| p.session_peak_activity),
                }
            })
            .collect();
        RenderModel {
            snapshot: snapshot.clone(),
            gpus,
            processes,
        }
    }
}

/// Derives the memory occupancy observation from used and total cells so all
/// surfaces share one occupancy definition.
fn occupancy(used: &Cell, total: &Cell) -> Observation<f64> {
    match (used.fresh_u64(), total.fresh_u64()) {
        (Some((used, at)), Some((total, _))) if total > 0 && used <= total => {
            Observation::value(used as f64 / total as f64 * 100.0, at)
        }
        (Some(_), Some(_)) => Observation::unavailable(ObservationState::SOURCE_ERROR),
        (None, _) => used.observation_f64(),
        (_, _) => total.observation_f64(),
    }
}

/// Collects source-backed and derived candidates, then selects one sentence.
fn assemble_health(gpu: &GpuState, now: Now) -> Health {
    let mut candidates: Vec<HealthCandidate> = Vec::new();
    for report in [&gpu.kernel_health, &gpu.amdsmi_health]
        .into_iter()
        .flatten()
    {
        if report.active(now.mono) {
            candidates.extend(report.candidates.iter().cloned());
        }
    }

    // Source-reported limits reached (both numbers source-reported).
    if let (Some((hotspot, at)), Some((limit, _))) =
        (gpu.hotspot.fresh_f64(), gpu.temp_limit.fresh_f64())
        && limit > 0.0
        && hotspot >= limit
    {
        candidates.push(HealthCandidate {
            category: HealthCategory::LIMIT,
            message: format!("thermal limit reached · hotspot {hotspot:.0} / {limit:.0}°C"),
            observed_at: at,
        });
    }
    if let (Some((power, at)), Some((cap, _))) = (gpu.power.fresh_f64(), gpu.power_cap.fresh_f64())
        && cap > 0.0
        && power >= cap
    {
        candidates.push(HealthCandidate {
            category: HealthCategory::LIMIT,
            message: format!("power limit active · {power:.0} / {cap:.0} W"),
            observed_at: at,
        });
    }

    // Telemetry trouble from every contracted mode observation. The memory
    // controller is optional; primary-partition ownership is structural.
    let mut contracted: Vec<&Cell> =
        vec![&gpu.hotspot, &gpu.temp_limit, &gpu.power, &gpu.power_cap];
    for part in &gpu.partitions {
        contracted.push(&part.activity);
        contracted.push(&part.mem_used);
        contracted.push(&part.mem_total);
        contracted.push(&part.gfx_clock);
    }
    let normal_observed_at = contracted
        .iter()
        .filter_map(|cell| cell.health_time())
        .max()
        .unwrap_or(now.wall);
    for cell in contracted {
        let Some((state, source_time)) = cell.trouble() else {
            continue;
        };
        let observed_at = source_time.unwrap_or(now.wall);
        let message = match state.as_str() {
            "asleep" => "GPU asleep".to_owned(),
            "permission_denied" => "telemetry permission denied".to_owned(),
            "unsupported_hardware" => "telemetry unsupported by hardware".to_owned(),
            "unsupported_driver_version" => {
                "telemetry unsupported by this driver version".to_owned()
            }
            "stale" => {
                let age = source_time
                    .map(|t| (now.wall.as_odt() - t.as_odt()).as_seconds_f64().max(0.0))
                    .unwrap_or(0.0);
                format!("telemetry stale · last sample {age:.1}s ago")
            }
            _ => "telemetry source error".to_owned(),
        };
        if !candidates
            .iter()
            .any(|c| c.category == HealthCategory::TELEMETRY && c.message == message)
        {
            candidates.push(HealthCandidate {
                category: HealthCategory::TELEMETRY,
                message,
                observed_at,
            });
        }
    }

    health::select(&candidates, normal_observed_at)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use time::macros::datetime;

    use super::*;
    use crate::model::{MemoryPool, PartitionId, PciBdf, PhysicalGpuId};
    use crate::state::MetricResult;

    struct Clock {
        base_mono: Instant,
        base_wall: time::OffsetDateTime,
    }

    impl Clock {
        fn new() -> Self {
            Self {
                base_mono: Instant::now(),
                base_wall: datetime!(2026-08-21 10:00 UTC),
            }
        }

        fn at(&self, ms: u64) -> Now {
            Now {
                wall: Timestamp::from_odt(self.base_wall + Duration::from_millis(ms)),
                mono: self.base_mono + Duration::from_millis(ms),
            }
        }
    }

    fn gpu_id() -> PhysicalGpuId {
        PhysicalGpuId::new("gpu-0000:41:00.0")
    }

    fn part_id(n: u32) -> PartitionId {
        PartitionId::new(format!("gpu-0000:41:00.0-xcp-{n}"))
    }

    fn discovered(partitions: u32) -> DiscoveredGpu {
        DiscoveredGpu {
            id: gpu_id(),
            bdf: PciBdf::parse("0000:41:00.0").unwrap(),
            name: "AMD Instinct MI300X".to_owned(),
            uuid: None,
            serial: None,
            pool: MemoryPool::VRAM,
            partitions: (0..partitions)
                .map(|n| DiscoveredPartition {
                    id: part_id(n),
                    is_primary: n == 0,
                    bdf: PciBdf::parse(&format!("0000:41:00.{n}")).unwrap(),
                })
                .collect(),
        }
    }

    fn reducer(clock: &Clock) -> Reducer {
        let mut reducer = Reducer::new(UtcOffset::UTC, clock.at(0).wall);
        let effects = reducer.apply_topology(vec![discovered(1)]);
        assert_eq!(effects, vec![StateEffect::GpuAdded(gpu_id())]);
        reducer
    }

    fn fast_batch(now: Now, origin: Origin, results: Vec<MetricResult>) -> MetricBatch {
        MetricBatch {
            gpu: gpu_id(),
            partition: Some(part_id(0)),
            origin,
            lane: Lane::Fast,
            observed_wall: now.wall,
            observed_mono: now.mono,
            results,
            energy_accumulator: None,
        }
    }

    fn activity(value: f64) -> MetricResult {
        MetricResult {
            metric: MetricKey::Partition(PartitionMetric::ActivityPercent),
            outcome: Ok(Value::F64(value)),
        }
    }

    fn activity_err(state: ObservationState) -> MetricResult {
        MetricResult {
            metric: MetricKey::Partition(PartitionMetric::ActivityPercent),
            outcome: Err(state),
        }
    }

    fn snapshot_activity(snapshot: &Snapshot) -> &Observation<f64> {
        &snapshot.gpus[0].partitions[0].activity_percent
    }

    #[test]
    fn failed_read_before_deadline_retains_value_without_history_append() {
        let clock = Clock::new();
        let mut r = reducer(&clock);
        r.apply_batch(fast_batch(
            clock.at(0),
            Origin::Kernel,
            vec![activity(80.0)],
        ));
        r.end_tick(clock.at(0));
        r.apply_batch(fast_batch(
            clock.at(250),
            Origin::Kernel,
            vec![activity_err(ObservationState::SOURCE_ERROR)],
        ));
        r.end_tick(clock.at(250));
        let snapshot = r.assemble(clock.at(500), None);
        // Value retained with its original source time.
        assert_eq!(
            snapshot_activity(&snapshot),
            &Observation::value(80.0, clock.at(0).wall)
        );
        // History has exactly one fresh slot and one gap.
        let render = r.render_model(&snapshot, None);
        assert_eq!(render.gpus[0].activity_history, vec![Some(80.0), None]);
        assert_eq!(render.gpus[0].session_peak_activity, Some(80.0));
    }

    #[test]
    fn deadline_elapsed_without_evidence_becomes_stale_with_last_time() {
        let clock = Clock::new();
        let mut r = reducer(&clock);
        r.apply_batch(fast_batch(
            clock.at(0),
            Origin::Kernel,
            vec![activity(80.0)],
        ));
        r.end_tick(clock.at(0));
        // No further results; at max(1s, 3×250ms) the value is stale.
        let snapshot = r.assemble(clock.at(1000), None);
        assert_eq!(
            snapshot_activity(&snapshot),
            &Observation::stale(clock.at(0).wall)
        );
        // Other never-observed contracted cells also keep health in telemetry.
        assert_eq!(snapshot.gpus[0].health.category, HealthCategory::TELEMETRY);
    }

    #[test]
    fn deadline_elapsed_with_asleep_evidence_is_stale_with_last_good_time() {
        let clock = Clock::new();
        let mut r = reducer(&clock);
        r.apply_batch(fast_batch(
            clock.at(0),
            Origin::Kernel,
            vec![activity(80.0)],
        ));
        r.apply_batch(fast_batch(
            clock.at(250),
            Origin::Kernel,
            vec![activity_err(ObservationState::ASLEEP)],
        ));
        let snapshot = r.assemble(clock.at(1500), None);
        assert_eq!(
            snapshot_activity(&snapshot),
            &Observation::stale(clock.at(0).wall)
        );
    }

    #[test]
    fn stale_then_fresh_appends_exactly_once_and_updates_peak() {
        let clock = Clock::new();
        let mut r = reducer(&clock);
        r.apply_batch(fast_batch(
            clock.at(0),
            Origin::Kernel,
            vec![activity(50.0)],
        ));
        r.end_tick(clock.at(0));
        let _ = r.assemble(clock.at(1500), None); // becomes stale
        r.end_tick(clock.at(1500)); // gap tick while stale
        r.apply_batch(fast_batch(
            clock.at(1750),
            Origin::Kernel,
            vec![activity(90.0)],
        ));
        r.end_tick(clock.at(1750));
        let snapshot = r.assemble(clock.at(1750), None);
        assert_eq!(
            snapshot_activity(&snapshot),
            &Observation::value(90.0, clock.at(1750).wall)
        );
        let render = r.render_model(&snapshot, None);
        assert_eq!(
            render.gpus[0].activity_history,
            vec![Some(50.0), None, Some(90.0)]
        );
        assert_eq!(render.gpus[0].session_peak_activity, Some(90.0));
    }

    #[test]
    fn fresh_kernel_value_defeats_newer_amdsmi_value() {
        let clock = Clock::new();
        let mut r = reducer(&clock);
        r.apply_batch(fast_batch(
            clock.at(0),
            Origin::Kernel,
            vec![activity(70.0)],
        ));
        r.apply_batch(fast_batch(
            clock.at(100),
            Origin::AmdSmi,
            vec![activity(10.0)],
        ));
        let snapshot = r.assemble(clock.at(200), None);
        assert_eq!(
            snapshot_activity(&snapshot),
            &Observation::value(70.0, clock.at(0).wall)
        );
        // But AMD SMI may fill a field with no fresh kernel value.
        let mut r = reducer(&clock);
        r.apply_batch(fast_batch(
            clock.at(0),
            Origin::AmdSmi,
            vec![activity(10.0)],
        ));
        let snapshot = r.assemble(clock.at(200), None);
        assert_eq!(
            snapshot_activity(&snapshot),
            &Observation::value(10.0, clock.at(0).wall)
        );
    }

    #[test]
    fn socket_metrics_live_only_on_the_physical_gpu() {
        let clock = Clock::new();
        let mut r = reducer(&clock);
        let now = clock.at(0);
        r.apply_batch(MetricBatch {
            gpu: gpu_id(),
            partition: None,
            origin: Origin::Kernel,
            lane: Lane::Fast,
            observed_wall: now.wall,
            observed_mono: now.mono,
            results: vec![MetricResult {
                metric: MetricKey::Socket(SocketMetric::SocketWatts),
                outcome: Ok(Value::F64(318.0)),
            }],
            energy_accumulator: None,
        });
        let snapshot = r.assemble(clock.at(100), None);
        assert_eq!(
            snapshot.gpus[0].power.socket_watts,
            Observation::value(318.0, now.wall)
        );
        // The JSON shape carries no socket metric on partitions at all;
        // verify no partition-scope cell was touched.
        assert_eq!(
            snapshot.gpus[0].partitions[0].activity_percent,
            Observation::unavailable(ObservationState::SOURCE_ERROR)
        );
    }

    #[test]
    fn secondary_partition_state_passes_through_unaltered() {
        let clock = Clock::new();
        let mut r = Reducer::new(UtcOffset::UTC, clock.at(0).wall);
        r.apply_topology(vec![discovered(2)]);
        let now = clock.at(0);
        r.apply_batch(MetricBatch {
            gpu: gpu_id(),
            partition: Some(part_id(1)),
            origin: Origin::Kernel,
            lane: Lane::Fast,
            observed_wall: now.wall,
            observed_mono: now.mono,
            results: vec![activity_err(
                ObservationState::REPORTED_BY_PRIMARY_PARTITION,
            )],
            energy_accumulator: None,
        });
        let snapshot = r.assemble(clock.at(100), None);
        assert_eq!(
            snapshot.gpus[0].partitions[1].activity_percent,
            Observation::unavailable(ObservationState::REPORTED_BY_PRIMARY_PARTITION)
        );
    }

    #[test]
    fn health_priority_orders_fault_over_throttle_over_limit_over_telemetry() {
        let clock = Clock::new();
        let mut r = reducer(&clock);
        let now = clock.at(0);
        // Derived limit condition: power at cap.
        r.apply_batch(MetricBatch {
            gpu: gpu_id(),
            partition: None,
            origin: Origin::Kernel,
            lane: Lane::Fast,
            observed_wall: now.wall,
            observed_mono: now.mono,
            results: vec![
                MetricResult {
                    metric: MetricKey::Socket(SocketMetric::SocketWatts),
                    outcome: Ok(Value::F64(320.0)),
                },
                MetricResult {
                    metric: MetricKey::Socket(SocketMetric::CapWatts),
                    outcome: Ok(Value::F64(320.0)),
                },
            ],
            energy_accumulator: None,
        });
        let snapshot = r.assemble(clock.at(100), None);
        assert_eq!(snapshot.gpus[0].health.category, HealthCategory::LIMIT);
        assert_eq!(
            snapshot.gpus[0].health.message,
            "power limit active · 320 / 320 W"
        );

        // A source-backed throttle outranks the limit.
        r.apply_health_report(SourceHealthReport {
            gpu: gpu_id(),
            origin: Origin::Kernel,
            observed_mono: now.mono,
            lane: Lane::Slow,
            candidates: vec![HealthCandidate {
                category: HealthCategory::THROTTLE,
                message: "thermal throttle · hotspot 94 / 95°C".to_owned(),
                observed_at: now.wall,
            }],
        });
        let snapshot = r.assemble(clock.at(200), None);
        assert_eq!(snapshot.gpus[0].health.category, HealthCategory::THROTTLE);

        // A fault outranks the throttle.
        r.apply_health_report(SourceHealthReport {
            gpu: gpu_id(),
            origin: Origin::Kernel,
            observed_mono: now.mono,
            lane: Lane::Slow,
            candidates: vec![
                HealthCandidate {
                    category: HealthCategory::THROTTLE,
                    message: "thermal throttle".to_owned(),
                    observed_at: now.wall,
                },
                HealthCandidate {
                    category: HealthCategory::FAULT,
                    message: "2 uncorrectable ECC errors".to_owned(),
                    observed_at: now.wall,
                },
            ],
        });
        let snapshot = r.assemble(clock.at(300), None);
        assert_eq!(snapshot.gpus[0].health.category, HealthCategory::FAULT);
        assert_eq!(
            snapshot.gpus[0].health.message,
            "2 uncorrectable ECC errors"
        );
    }

    #[test]
    fn one_metric_failure_preserves_independent_values() {
        let clock = Clock::new();
        let mut r = reducer(&clock);
        let now = clock.at(0);
        r.apply_batch(fast_batch(
            now,
            Origin::Kernel,
            vec![
                activity_err(ObservationState::PERMISSION_DENIED),
                MetricResult {
                    metric: MetricKey::Partition(PartitionMetric::MemUsedBytes),
                    outcome: Ok(Value::U64(96 * 1024 * 1024 * 1024)),
                },
                MetricResult {
                    metric: MetricKey::Partition(PartitionMetric::MemTotalBytes),
                    outcome: Ok(Value::U64(192 * 1024 * 1024 * 1024)),
                },
            ],
        ));
        let snapshot = r.assemble(clock.at(100), None);
        let part = &snapshot.gpus[0].partitions[0];
        assert_eq!(
            part.activity_percent,
            Observation::unavailable(ObservationState::PERMISSION_DENIED)
        );
        assert_eq!(
            part.memory.occupancy_percent,
            Observation::value(50.0, now.wall)
        );
        assert_eq!(
            part.memory.used_bytes.current(),
            Some(&(96 * 1024 * 1024 * 1024))
        );
    }

    #[test]
    fn partition_identity_change_is_fatal() {
        let clock = Clock::new();
        let mut r = reducer(&clock);
        let effects = r.apply_topology(vec![discovered(2)]);
        assert_eq!(
            effects,
            vec![StateEffect::PartitionConfigurationChanged(gpu_id())]
        );
    }

    #[test]
    fn removal_and_return_produce_notices() {
        let clock = Clock::new();
        let mut r = reducer(&clock);
        let effects = r.apply_topology(vec![]);
        assert_eq!(effects, vec![StateEffect::GpuRemoved(gpu_id())]);
        assert_eq!(r.gpu_count(), 0);
        let snapshot = r.assemble(clock.at(100), None);
        assert!(snapshot.gpus.is_empty());
        let effects = r.apply_topology(vec![discovered(1)]);
        assert_eq!(effects, vec![StateEffect::GpuAdded(gpu_id())]);
    }

    #[test]
    fn daily_summary_accumulates_and_resets_session_peaks_independently() {
        let clock = Clock::new();
        let mut r = reducer(&clock);
        r.apply_batch(fast_batch(
            clock.at(0),
            Origin::Kernel,
            vec![activity(95.0)],
        ));
        r.end_tick(clock.at(0));
        assert!(r.take_daily_dirty());
        let record = r.daily_record();
        assert_eq!(
            record.gpus[gpu_id().as_str()].activity_peak_percent,
            Some(95.0)
        );
        r.reset_session_peaks();
        let snapshot = r.assemble(clock.at(100), None);
        let render = r.render_model(&snapshot, None);
        assert_eq!(render.gpus[0].session_peak_activity, None);
        // Daily peak survives a session reset.
        assert_eq!(
            r.daily_record().gpus[gpu_id().as_str()].activity_peak_percent,
            Some(95.0)
        );
    }

    #[test]
    fn late_batch_never_enters_history_or_peaks() {
        let clock = Clock::new();
        let mut r = reducer(&clock);
        r.apply_batch_at(
            fast_batch(clock.at(0), Origin::Kernel, vec![activity(99.0)]),
            Some(clock.at(1500).mono),
        );
        r.end_tick(clock.at(1500));
        let snapshot = r.assemble(clock.at(1500), None);
        assert!(
            snapshot.gpus[0].partitions[0]
                .activity_percent
                .current()
                .is_none()
        );
        let render = r.render_model(&snapshot, None);
        assert_eq!(render.gpus[0].activity_history, vec![None]);
        assert_eq!(render.gpus[0].session_peak_activity, None);
    }

    #[test]
    fn optional_source_cannot_mask_kernel_runtime_state() {
        let clock = Clock::new();
        let mut r = reducer(&clock);
        r.apply_batch(fast_batch(
            clock.at(0),
            Origin::Kernel,
            vec![activity_err(ObservationState::UNSUPPORTED_HARDWARE)],
        ));
        r.apply_batch(fast_batch(
            clock.at(10),
            Origin::AmdSmi,
            vec![activity(50.0)],
        ));
        r.apply_batch(fast_batch(
            clock.at(15),
            Origin::Kernel,
            vec![activity_err(ObservationState::UNSUPPORTED_HARDWARE)],
        ));
        assert_eq!(
            snapshot_activity(&r.assemble(clock.at(25), None)),
            &Observation::value(50.0, clock.at(10).wall)
        );
        r.apply_batch(fast_batch(
            clock.at(30),
            Origin::Kernel,
            vec![activity_err(ObservationState::ASLEEP)],
        ));
        r.apply_batch(fast_batch(
            clock.at(40),
            Origin::AmdSmi,
            vec![activity_err(ObservationState::SOURCE_ERROR)],
        ));
        assert_eq!(
            snapshot_activity(&r.assemble(clock.at(50), None)),
            &Observation::unavailable(ObservationState::ASLEEP)
        );
    }

    #[test]
    fn stale_last_good_survives_repeated_failures() {
        let clock = Clock::new();
        let mut r = reducer(&clock);
        r.apply_batch(fast_batch(
            clock.at(0),
            Origin::Kernel,
            vec![activity(70.0)],
        ));
        let _ = r.assemble(clock.at(1000), None);
        r.apply_batch(fast_batch(
            clock.at(1250),
            Origin::Kernel,
            vec![activity_err(ObservationState::SOURCE_ERROR)],
        ));
        assert_eq!(
            snapshot_activity(&r.assemble(clock.at(1300), None)),
            &Observation::stale(clock.at(0).wall)
        );
    }

    #[test]
    fn same_ids_with_changed_primary_or_pool_are_fatal() {
        let clock = Clock::new();
        let mut r = reducer(&clock);
        let mut changed = discovered(1);
        changed.partitions[0].is_primary = false;
        assert_eq!(
            r.apply_topology(vec![changed]),
            vec![StateEffect::PartitionConfigurationChanged(gpu_id())]
        );

        let mut r = reducer(&clock);
        let mut changed = discovered(1);
        changed.pool = MemoryPool::GTT;
        assert_eq!(
            r.apply_topology(vec![changed]),
            vec![StateEffect::PartitionConfigurationChanged(gpu_id())]
        );
    }

    #[test]
    fn invalid_memory_relationship_never_updates_observations_or_history() {
        let clock = Clock::new();
        let mut r = reducer(&clock);
        r.apply_batch(fast_batch(
            clock.at(0),
            Origin::Kernel,
            vec![
                MetricResult {
                    metric: MetricKey::Partition(PartitionMetric::MemUsedBytes),
                    outcome: Ok(Value::U64(200)),
                },
                MetricResult {
                    metric: MetricKey::Partition(PartitionMetric::MemTotalBytes),
                    outcome: Ok(Value::U64(100)),
                },
            ],
        ));
        r.end_tick(clock.at(0));
        let snapshot = r.assemble(clock.at(0), None);
        let memory = &snapshot.gpus[0].partitions[0].memory;
        assert_eq!(
            memory.used_bytes,
            Observation::unavailable(ObservationState::SOURCE_ERROR)
        );
        assert_eq!(
            memory.total_bytes,
            Observation::unavailable(ObservationState::SOURCE_ERROR)
        );
        assert_eq!(
            memory.occupancy_percent,
            Observation::unavailable(ObservationState::SOURCE_ERROR)
        );
        assert_eq!(
            r.render_model(&snapshot, None).gpus[0].memory_history,
            vec![None]
        );
    }

    #[test]
    fn timeout_preserves_existing_explicit_or_fresh_state() {
        let clock = Clock::new();
        let mut r = reducer(&clock);
        r.apply_batch(fast_batch(
            clock.at(0),
            Origin::Kernel,
            vec![activity_err(ObservationState::UNSUPPORTED_HARDWARE)],
        ));
        r.apply_kernel_timeout(&gpu_id(), Lane::Fast, clock.at(200));
        assert_eq!(
            snapshot_activity(&r.assemble(clock.at(200), None)),
            &Observation::unavailable(ObservationState::UNSUPPORTED_HARDWARE)
        );

        r.apply_batch(fast_batch(
            clock.at(250),
            Origin::AmdSmi,
            vec![activity(60.0)],
        ));
        r.apply_kernel_timeout(&gpu_id(), Lane::Fast, clock.at(450));
        assert_eq!(
            snapshot_activity(&r.assemble(clock.at(450), None)),
            &Observation::value(60.0, clock.at(250).wall)
        );
    }

    #[test]
    fn incomplete_invalid_memory_batch_does_not_partially_commit() {
        let clock = Clock::new();
        let mut r = reducer(&clock);
        r.apply_batch(fast_batch(
            clock.at(0),
            Origin::Kernel,
            vec![
                MetricResult {
                    metric: MetricKey::Partition(PartitionMetric::MemUsedBytes),
                    outcome: Ok(Value::U64(50)),
                },
                MetricResult {
                    metric: MetricKey::Partition(PartitionMetric::MemTotalBytes),
                    outcome: Ok(Value::U64(100)),
                },
            ],
        ));
        r.apply_batch(fast_batch(
            clock.at(250),
            Origin::Kernel,
            vec![
                MetricResult {
                    metric: MetricKey::Partition(PartitionMetric::MemUsedBytes),
                    outcome: Ok(Value::U64(150)),
                },
                MetricResult {
                    metric: MetricKey::Partition(PartitionMetric::MemTotalBytes),
                    outcome: Err(ObservationState::SOURCE_ERROR),
                },
            ],
        ));
        let snapshot = r.assemble(clock.at(300), None);
        let memory = &snapshot.gpus[0].partitions[0].memory;
        assert_eq!(memory.used_bytes.current(), Some(&50));
        assert_eq!(memory.total_bytes.current(), Some(&100));
        assert_eq!(memory.occupancy_percent.current(), Some(&50.0));
    }
    #[test]
    fn source_health_report_expires_on_its_cadence() {
        let clock = Clock::new();
        let mut r = reducer(&clock);
        r.apply_health_report(SourceHealthReport {
            gpu: gpu_id(),
            origin: Origin::Kernel,
            observed_mono: clock.at(0).mono,
            lane: Lane::Slow,
            candidates: vec![HealthCandidate {
                category: HealthCategory::THROTTLE,
                message: "thermal throttle".to_owned(),
                observed_at: clock.at(0).wall,
            }],
        });
        assert_eq!(
            r.assemble(clock.at(100), None).gpus[0].health.category,
            HealthCategory::THROTTLE
        );
        assert_ne!(
            r.assemble(clock.at(3000), None).gpus[0].health.category,
            HealthCategory::THROTTLE
        );
    }
}
