//! Deterministic product state: canonical observations, freshness, topology,
//! histories, peaks, daily summaries, and health.
//!
//! [`reducer`] applies typed normalized events to owned state; [`history`]
//! owns preallocated rings and daily accumulation; [`health`] owns priority
//! and wording inputs. None performs I/O.

pub(crate) mod health;
pub(crate) mod history;
pub(crate) mod reducer;

use std::time::Instant;

use crate::model::{
    MemoryPool, ObservationState, PartitionId, PciBdf, PhysicalGpuId, Snapshot, Timestamp,
};

/// Which source produced a batch; kernel observations are authoritative.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Origin {
    Kernel,
    AmdSmi,
}

/// Which collection lane produced a batch; fixes the staleness cadence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Lane {
    /// 250 ms kernel fast collection.
    Fast,
    /// 1 s slow health collection.
    Slow,
}

impl Lane {
    /// Approved freshness threshold: `max(1 s, 3 × cadence)`.
    pub fn stale_after(self) -> std::time::Duration {
        match self {
            Lane::Fast => std::time::Duration::from_secs(1),
            Lane::Slow => std::time::Duration::from_secs(3),
        }
    }
}

/// Socket-scoped metrics owned by the physical GPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SocketMetric {
    HotspotCelsius,
    LimitCelsius,
    SocketWatts,
    CapWatts,
}

/// Partition-scoped metrics owned by one XCP.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum PartitionMetric {
    ActivityPercent,
    MemUsedBytes,
    MemTotalBytes,
    GfxClockMhz,
    MemCtlActivityPercent,
}

/// One metric at its hardware scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum MetricKey {
    Socket(SocketMetric),
    Partition(PartitionMetric),
}

/// A normalized numeric value in its canonical unit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Value {
    F64(f64),
    U64(u64),
}

/// One normalized metric outcome: a value or the reason there is none.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MetricResult {
    pub metric: MetricKey,
    pub outcome: Result<Value, ObservationState>,
}

/// A coherent normalized batch from one source read of one scope.
#[derive(Debug, Clone)]
pub(crate) struct MetricBatch {
    pub gpu: PhysicalGpuId,
    /// `None` targets socket scope on the physical GPU.
    pub partition: Option<PartitionId>,
    pub origin: Origin,
    pub lane: Lane,
    /// Wall-clock time the source observed the values.
    pub observed_wall: Timestamp,
    /// Monotonic time of the same read, for freshness arithmetic.
    pub observed_mono: Instant,
    pub results: Vec<MetricResult>,
    /// Raw socket energy accumulator and joules-per-count resolution, when
    /// the source exposes one. Used only for the daily energy summary.
    pub energy_accumulator: Option<(u64, f64)>,
}

/// Replace-set of source-backed health candidates for one physical GPU.
#[derive(Debug, Clone)]
pub(crate) struct SourceHealthReport {
    pub gpu: PhysicalGpuId,
    pub origin: Origin,
    pub candidates: Vec<health::HealthCandidate>,
}

/// One discovered XCP partition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiscoveredPartition {
    pub id: PartitionId,
    pub is_primary: bool,
    /// PCI address of the partition's own DRM function.
    pub bdf: PciBdf,
}

/// One discovered physical GPU and its partition topology.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiscoveredGpu {
    pub id: PhysicalGpuId,
    pub bdf: PciBdf,
    pub name: String,
    pub uuid: Option<String>,
    pub serial: Option<String>,
    pub pool: MemoryPool,
    pub partitions: Vec<DiscoveredPartition>,
}

/// Effects the reducer asks the monitor to surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StateEffect {
    /// A physical GPU was confirmed present for the first time (or returned).
    GpuAdded(PhysicalGpuId),
    /// A physical GPU was confirmed removed by rescan.
    GpuRemoved(PhysicalGpuId),
    /// Partition topology of a known GPU changed: fatal restart condition.
    PartitionConfigurationChanged(PhysicalGpuId),
}

/// Per-GPU render extras aligned with `Snapshot::gpus` by index.
#[derive(Debug, Clone)]
pub(crate) struct RenderGpu {
    pub id: PhysicalGpuId,
    /// Primary-partition activity history: one slot per production tick,
    /// `None` where no fresh observation arrived (rendered as a gap).
    pub activity_history: Vec<Option<f64>>,
    /// Primary-partition memory occupancy history, same timeline.
    pub memory_history: Vec<Option<f64>>,
    /// Session peak of primary-partition activity, if any fresh value seen.
    pub session_peak_activity: Option<f64>,
}

/// Latest process-overlay scan, present only while the overlay is scoped.
#[derive(Debug, Clone)]
pub(crate) struct ProcessOverlay {
    pub scanned_at: Timestamp,
    pub rows: Vec<crate::source::process::ProcessRow>,
    /// Resolves row `drm-pdev` BDFs to stable physical GPU identities.
    pub gpu_by_bdf: std::collections::HashMap<PciBdf, PhysicalGpuId>,
}

/// Immutable presentation projection: canonical current observations plus
/// bounded history. Never contains layout, styles, or animation state.
#[derive(Debug, Clone)]
pub(crate) struct RenderModel {
    pub snapshot: Snapshot,
    pub gpus: Vec<RenderGpu>,
    pub processes: Option<ProcessOverlay>,
}
