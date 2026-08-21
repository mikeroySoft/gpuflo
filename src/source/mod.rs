//! Private owned source vocabulary shared by the three source adapters.
//!
//! Adapters emit owned source-specific samples with unit-documented fields;
//! they never expose file handles, C pointers, or borrowed memory. Expected
//! failures are data ([`Reading`] states), not control-flow errors.

pub(crate) mod amdsmi;
pub(crate) mod kernel;
pub(crate) mod process;

use std::time::Instant;

use crate::model::{PartitionId, PhysicalGpuId, Timestamp};

/// One raw source reading: a value in the field's documented unit or the
/// evidence-based reason there is none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reading<T> {
    Value(T),
    /// The source does not provide this field (node or layout field absent).
    Absent,
    /// The source emitted its documented unavailable sentinel.
    Sentinel,
    /// Runtime-suspended amdgpu denied the read (`EPERM`).
    Asleep,
    /// The source exists but access was denied (`EACCES`).
    PermissionDenied,
    /// Explicit version/layout evidence prevents interpretation.
    UnsupportedDriver,
    /// Recognized source produced malformed or out-of-domain content.
    Malformed,
    /// Unexpected I/O failure.
    Error,
}

impl<T> Reading<T> {
    /// Prefers this reading's value; otherwise falls back, keeping the more
    /// specific failure evidence when both fail.
    pub fn or_else(self, fallback: impl FnOnce() -> Reading<T>) -> Reading<T> {
        match self {
            Reading::Value(_) => self,
            _ => {
                let other = fallback();
                match (&self, &other) {
                    (_, Reading::Value(_)) => other,
                    // Specific runtime evidence beats structural absence.
                    (Reading::Absent | Reading::Sentinel, _) => other,
                    _ => self,
                }
            }
        }
    }

    /// Maps the value, preserving failure evidence.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Reading<U> {
        match self {
            Reading::Value(v) => Reading::Value(f(v)),
            Reading::Absent => Reading::Absent,
            Reading::Sentinel => Reading::Sentinel,
            Reading::Asleep => Reading::Asleep,
            Reading::PermissionDenied => Reading::PermissionDenied,
            Reading::UnsupportedDriver => Reading::UnsupportedDriver,
            Reading::Malformed => Reading::Malformed,
            Reading::Error => Reading::Error,
        }
    }

    /// Fallibly maps a value; conversion failure is malformed source data.
    pub fn checked_map<U>(self, f: impl FnOnce(T) -> Option<U>) -> Reading<U> {
        match self {
            Reading::Value(v) => f(v).map_or(Reading::Malformed, Reading::Value),
            Reading::Absent => Reading::Absent,
            Reading::Sentinel => Reading::Sentinel,
            Reading::Asleep => Reading::Asleep,
            Reading::PermissionDenied => Reading::PermissionDenied,
            Reading::UnsupportedDriver => Reading::UnsupportedDriver,
            Reading::Malformed => Reading::Malformed,
            Reading::Error => Reading::Error,
        }
    }
}

/// Maps an I/O error to reading evidence. amdgpu returns `EPERM` from a
/// runtime-suspended device ("deny access" without waking it); `EACCES` is
/// genuine permission denial; a missing node is structural absence.
pub(crate) fn kernel_error_reading<T>(error: &std::io::Error) -> Reading<T> {
    /// `EPERM`.
    const EPERM: i32 = 1;
    /// `EACCES`.
    const EACCES: i32 = 13;
    match error.raw_os_error() {
        Some(EPERM) => Reading::Asleep,
        Some(EACCES) => Reading::PermissionDenied,
        _ if error.kind() == std::io::ErrorKind::NotFound => Reading::Absent,
        _ => Reading::Error,
    }
}

/// Fast (250 ms) kernel sample for one physical GPU.
#[derive(Debug, Clone)]
pub(crate) struct KernelFastSample {
    pub gpu: PhysicalGpuId,
    pub read_wall: Timestamp,
    pub read_mono: Instant,
    /// The physical device path disappeared during this collection; asks the
    /// coordinator for immediate topology verification.
    pub device_missing: bool,
    /// Socket hotspot temperature, milli-Celsius.
    pub hotspot_millic: Reading<i64>,
    /// Socket power, microwatts.
    pub socket_power_microwatts: Reading<u64>,
    /// Raw socket energy accumulator and joules-per-count, when exposed.
    pub energy: Option<(u64, f64)>,
    pub partitions: Vec<KernelPartitionFast>,
}

/// Fast partition-scoped kernel readings.
#[derive(Debug, Clone)]
pub(crate) struct KernelPartitionFast {
    pub partition: PartitionId,
    pub is_primary: bool,
    /// GFX activity, centi-percent.
    pub activity_centipercent: Reading<u64>,
    /// Used capacity of the applicable pool, bytes.
    pub mem_used_bytes: Reading<u64>,
    /// Total capacity of the applicable pool, bytes.
    pub mem_total_bytes: Reading<u64>,
    /// Memory-controller activity, centi-percent.
    pub mem_ctl_centipercent: Reading<u64>,
}

/// Slow (1 s) kernel sample for one physical GPU.
#[derive(Debug, Clone)]
pub(crate) struct KernelSlowSample {
    pub gpu: PhysicalGpuId,
    pub read_wall: Timestamp,
    pub read_mono: Instant,
    /// The physical device path disappeared during this collection.
    pub device_missing: bool,
    /// Hotspot slowdown/critical limit, milli-Celsius.
    pub limit_millic: Reading<i64>,
    /// Power cap, microwatts.
    pub cap_microwatts: Reading<u64>,
    pub partitions: Vec<KernelPartitionSlow>,
    /// Source-backed health signals observed this collection.
    pub health: Vec<KernelHealthSignal>,
}

/// Slow partition-scoped kernel readings.
#[derive(Debug, Clone)]
pub(crate) struct KernelPartitionSlow {
    pub partition: PartitionId,
    pub is_primary: bool,
    /// Current GFX clock, hertz.
    pub gfx_clock_hz: Reading<u64>,
}

/// Source-reported fault/throttle evidence from kernel interfaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum KernelHealthSignal {
    /// An active throttle with its source-named reason groups.
    ThrottleActive { reasons: String },
    /// Uncorrectable ECC errors accumulated across RAS blocks.
    EccErrors { uncorrectable: u64 },
    /// Pending or unreservable bad VRAM pages.
    BadPages { pending: u64, unreservable: u64 },
}
