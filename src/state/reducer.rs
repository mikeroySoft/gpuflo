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
    Snapshot, Timestamp,
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
    /// No current value; `last_good` is retained only for `stale`.
    Unavailable {
        state: ObservationState,
        last_good: Option<Timestamp>,
    },
}

/// One metric cell: current canonical observation plus freshness bookkeeping.
#[derive(Debug, Clone)]
struct Cell {
    state: CellState,
    /// Most recent explicit read failure while a fresh value was retained.
    last_failure: Option<ObservationState>,
}

impl Cell {
    fn new() -> Self {
        Self {
            state: CellState::Empty,
            last_failure: None,
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
                // Kernel-first: optional enrichment never overwrites a fresh
                // kernel observation, regardless of which is newer.
                if origin == Origin::AmdSmi
                    && let CellState::Fresh {
                        origin: Origin::Kernel,
                        mono: k_mono,
                        lane: k_lane,
                        ..
                    } = &self.state
                    && mono.duration_since(*k_mono) <= k_lane.stale_after()
                {
                    return None;
                }
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
                match &self.state {
                    // Retain a fresh value; the freshness evaluation decides
                    // when the failure becomes visible.
                    CellState::Fresh { .. } => self.last_failure = Some(state.clone()),
                    _ => {
                        self.state = CellState::Unavailable {
                            state: state.clone(),
                            last_good: None,
                        };
                        self.last_failure = None;
                    }
                }
                None
            }
        }
    }

    /// Applies the freshness rule at `now`: a value past `max(1s, 3×cadence)`
    /// becomes its explicit failure state when one was observed, otherwise
    /// `stale` retaining the last good source time.
    fn evaluate(&mut self, now_mono: Instant) {
        if let CellState::Fresh {
            mono, lane, wall, ..
        } = &self.state
            && now_mono.duration_since(*mono) > lane.stale_after()
        {
            self.state = match self.last_failure.take() {
                Some(state) => CellState::Unavailable {
                    state,
                    last_good: None,
                },
                None => CellState::Unavailable {
                    state: ObservationState::STALE,
                    last_good: Some(*wall),
                },
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
            CellState::Unavailable { state, last_good } => Observation::Unavailable {
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
                CellState::Unavailable { state, last_good } => Observation::Unavailable {
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
            CellState::Unavailable { state, last_good } => match state.as_str() {
                // Structural capability is not telemetry trouble.
                "unsupported_hardware" | "reported_by_primary_partition" => None,
                _ => Some((state, *last_good)),
            },
            _ => None,
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
struct GpuState {
    disc: DiscoveredGpu,
    hotspot: Cell,
    temp_limit: Cell,
    power: Cell,
    power_cap: Cell,
    partitions: Vec<PartState>,
    kernel_health: Vec<HealthCandidate>,
    amdsmi_health: Vec<HealthCandidate>,
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
            kernel_health: Vec::new(),
            amdsmi_health: Vec::new(),
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
                    let old: Vec<_> = existing.disc.partitions.iter().map(|p| &p.id).collect();
                    let new: Vec<_> = gpu.partitions.iter().map(|p| &p.id).collect();
                    if old != new {
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
    pub fn apply_batch(&mut self, batch: MetricBatch) {
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
                let mut fresh_used: Option<u64> = None;
                let mut fresh_total: Option<u64> = None;
                for result in &batch.results {
                    let MetricKey::Partition(metric) = result.metric else {
                        continue;
                    };
                    let accepted = part.cell(metric).apply(
                        &result.outcome,
                        batch.origin,
                        batch.lane,
                        batch.observed_wall,
                        batch.observed_mono,
                    );
                    match (metric, accepted) {
                        (PartitionMetric::ActivityPercent, Some(Value::F64(v))) => {
                            part.staged_activity = Some(v);
                        }
                        (PartitionMetric::MemUsedBytes, Some(Value::U64(v))) => {
                            fresh_used = Some(v);
                        }
                        (PartitionMetric::MemTotalBytes, Some(Value::U64(v))) => {
                            fresh_total = Some(v);
                        }
                        _ => {}
                    }
                }
                if let (Some(used), Some(total)) = (fresh_used, fresh_total)
                    && total > 0
                {
                    part.staged_memory = Some(used as f64 / total as f64 * 100.0);
                }
            }
        }
    }

    /// Replaces the source-backed candidate set for one GPU and origin.
    pub fn apply_health_report(&mut self, report: SourceHealthReport) {
        let Some(gpu) = self.gpus.iter_mut().find(|g| g.disc.id == report.gpu) else {
            return;
        };
        match report.origin {
            Origin::Kernel => gpu.kernel_health = report.candidates,
            Origin::AmdSmi => gpu.amdsmi_health = report.candidates,
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
        (Some((used, at)), Some((total, _))) if total > 0 => {
            Observation::value(used as f64 / total as f64 * 100.0, at)
        }
        (None, _) => used.observation_f64(),
        (_, _) => total.observation_f64(),
    }
}

/// Collects source-backed and derived candidates, then selects one sentence.
fn assemble_health(gpu: &GpuState, now: Now) -> Health {
    let mut candidates: Vec<HealthCandidate> = Vec::new();
    candidates.extend(gpu.kernel_health.iter().cloned());
    candidates.extend(gpu.amdsmi_health.iter().cloned());

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

    // Telemetry trouble from contracted mode observations only.
    let mut contracted: Vec<&Cell> = vec![&gpu.hotspot, &gpu.power];
    for part in &gpu.partitions {
        contracted.push(&part.activity);
        contracted.push(&part.mem_used);
        contracted.push(&part.mem_total);
    }
    for cell in contracted {
        let Some((state, last_good)) = cell.trouble() else {
            continue;
        };
        let (message, observed_at) = match state.as_str() {
            "asleep" => ("GPU asleep".to_owned(), now.wall),
            "permission_denied" => ("telemetry permission denied".to_owned(), now.wall),
            "unsupported_driver_version" => (
                "telemetry unsupported by this driver version".to_owned(),
                now.wall,
            ),
            "stale" => {
                let age = last_good
                    .map(|t| (now.wall.as_odt() - t.as_odt()).as_seconds_f64().max(0.0))
                    .unwrap_or(0.0);
                (
                    format!("telemetry stale · last sample {age:.1}s ago"),
                    now.wall,
                )
            }
            _ => ("telemetry source error".to_owned(), now.wall),
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

    health::select(&candidates, now.wall)
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
        // No further results at all; past max(1s, 3×250ms) = 1s.
        let snapshot = r.assemble(clock.at(1500), None);
        assert_eq!(
            snapshot_activity(&snapshot),
            &Observation::stale(clock.at(0).wall)
        );
        // Health names the stale telemetry with its age.
        assert_eq!(snapshot.gpus[0].health.category, HealthCategory::TELEMETRY);
        assert_eq!(
            snapshot.gpus[0].health.message,
            "telemetry stale · last sample 1.5s ago"
        );
    }

    #[test]
    fn deadline_elapsed_with_asleep_evidence_shows_asleep() {
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
            &Observation::unavailable(ObservationState::ASLEEP)
        );
        assert_eq!(snapshot.gpus[0].health.message, "GPU asleep");
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
        // Structural scope ownership is not telemetry trouble.
        assert_ne!(snapshot.gpus[0].health.message, "telemetry source error");
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
}
