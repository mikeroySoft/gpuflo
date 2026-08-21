//! The only source-to-domain seam: maps source readings to observation
//! states, converts units, assigns physical/XCP scope, and emits typed
//! canonical batches. Owns no freshness, history, health priority, or
//! rendering.

use crate::model::ObservationState;
use crate::source::{KernelFastSample, KernelHealthSignal, KernelSlowSample, Reading};
use crate::state::health::HealthCandidate;
use crate::state::{
    Lane, MetricBatch, MetricKey, MetricResult, Origin, PartitionMetric, SocketMetric,
    SourceHealthReport, Value,
};

/// Maps reading evidence to exactly one canonical observation state.
/// `structural_owner` substitutes structural absence on secondary partitions
/// whose metric is owned and reported by the primary XCP.
fn map_state<T>(reading: &Reading<T>, reported_by_primary: bool) -> ObservationState {
    match reading {
        Reading::Value(_) => unreachable!("mapped only for failures"),
        Reading::Absent | Reading::Sentinel if reported_by_primary => {
            ObservationState::REPORTED_BY_PRIMARY_PARTITION
        }
        Reading::Absent | Reading::Sentinel => ObservationState::UNSUPPORTED_HARDWARE,
        Reading::Asleep => ObservationState::ASLEEP,
        Reading::PermissionDenied => ObservationState::PERMISSION_DENIED,
        Reading::UnsupportedDriver => ObservationState::UNSUPPORTED_DRIVER_VERSION,
        Reading::Malformed | Reading::Error => ObservationState::SOURCE_ERROR,
    }
}

/// One converted metric outcome.
fn result<T>(
    metric: MetricKey,
    reading: Reading<T>,
    reported_by_primary: bool,
    convert: impl FnOnce(T) -> Value,
) -> MetricResult {
    let outcome = match reading {
        Reading::Value(value) => Ok(convert(value)),
        ref failed => Err(map_state(failed, reported_by_primary)),
    };
    MetricResult { metric, outcome }
}

fn centipercent(v: u64) -> Value {
    Value::F64(v as f64 / 100.0)
}

fn millic(v: i64) -> Value {
    Value::F64(v as f64 / 1000.0)
}

fn microwatts(v: u64) -> Value {
    Value::F64(v as f64 / 1_000_000.0)
}

fn hz(v: u64) -> Value {
    Value::F64(v as f64 / 1_000_000.0)
}

fn bytes(v: u64) -> Value {
    Value::U64(v)
}

/// Normalizes one kernel fast sample into socket and partition batches.
pub(crate) fn kernel_fast(sample: KernelFastSample) -> Vec<MetricBatch> {
    let mut batches = Vec::with_capacity(1 + sample.partitions.len());
    batches.push(MetricBatch {
        gpu: sample.gpu.clone(),
        partition: None,
        origin: Origin::Kernel,
        lane: Lane::Fast,
        observed_wall: sample.read_wall,
        observed_mono: sample.read_mono,
        results: vec![
            result(
                MetricKey::Socket(SocketMetric::HotspotCelsius),
                sample.hotspot_millic,
                false,
                millic,
            ),
            result(
                MetricKey::Socket(SocketMetric::SocketWatts),
                sample.socket_power_microwatts,
                false,
                microwatts,
            ),
        ],
        energy_accumulator: sample.energy,
    });
    for part in sample.partitions {
        let secondary = !part.is_primary;
        batches.push(MetricBatch {
            gpu: sample.gpu.clone(),
            partition: Some(part.partition),
            origin: Origin::Kernel,
            lane: Lane::Fast,
            observed_wall: sample.read_wall,
            observed_mono: sample.read_mono,
            results: vec![
                result(
                    MetricKey::Partition(PartitionMetric::ActivityPercent),
                    part.activity_centipercent,
                    secondary,
                    centipercent,
                ),
                result(
                    MetricKey::Partition(PartitionMetric::MemUsedBytes),
                    part.mem_used_bytes,
                    false,
                    bytes,
                ),
                result(
                    MetricKey::Partition(PartitionMetric::MemTotalBytes),
                    part.mem_total_bytes,
                    false,
                    bytes,
                ),
                result(
                    MetricKey::Partition(PartitionMetric::MemCtlActivityPercent),
                    part.mem_ctl_centipercent,
                    secondary,
                    centipercent,
                ),
            ],
            energy_accumulator: None,
        });
    }
    batches
}

/// Normalizes one kernel slow sample into batches plus the replace-set of
/// source-backed health candidates.
pub(crate) fn kernel_slow(sample: KernelSlowSample) -> (Vec<MetricBatch>, SourceHealthReport) {
    let mut batches = Vec::with_capacity(1 + sample.partitions.len());
    batches.push(MetricBatch {
        gpu: sample.gpu.clone(),
        partition: None,
        origin: Origin::Kernel,
        lane: Lane::Slow,
        observed_wall: sample.read_wall,
        observed_mono: sample.read_mono,
        results: vec![
            result(
                MetricKey::Socket(SocketMetric::LimitCelsius),
                sample.limit_millic,
                false,
                millic,
            ),
            result(
                MetricKey::Socket(SocketMetric::CapWatts),
                sample.cap_microwatts,
                false,
                microwatts,
            ),
        ],
        energy_accumulator: None,
    });
    for part in sample.partitions {
        let secondary = !part.is_primary;
        batches.push(MetricBatch {
            gpu: sample.gpu.clone(),
            partition: Some(part.partition),
            origin: Origin::Kernel,
            lane: Lane::Slow,
            observed_wall: sample.read_wall,
            observed_mono: sample.read_mono,
            results: vec![result(
                MetricKey::Partition(PartitionMetric::GfxClockMhz),
                part.gfx_clock_hz,
                secondary,
                hz,
            )],
            energy_accumulator: None,
        });
    }
    let candidates = sample
        .health
        .iter()
        .map(|signal| health_candidate(signal, sample.read_wall))
        .collect();
    (
        batches,
        SourceHealthReport {
            gpu: sample.gpu,
            origin: Origin::Kernel,
            candidates,
        },
    )
}

/// Composes one factual candidate from a kernel health signal.
fn health_candidate(
    signal: &KernelHealthSignal,
    observed_at: crate::model::Timestamp,
) -> HealthCandidate {
    use crate::model::HealthCategory;
    match signal {
        KernelHealthSignal::ThrottleActive { reasons } => HealthCandidate {
            category: HealthCategory::THROTTLE,
            message: if reasons.is_empty() {
                "throttle active".to_owned()
            } else {
                format!("{reasons} throttle active")
            },
            observed_at,
        },
        KernelHealthSignal::EccErrors { uncorrectable } => HealthCandidate {
            category: HealthCategory::FAULT,
            message: if *uncorrectable == 1 {
                "1 uncorrectable ECC error".to_owned()
            } else {
                format!("{uncorrectable} uncorrectable ECC errors")
            },
            observed_at,
        },
        KernelHealthSignal::BadPages {
            pending,
            unreservable,
        } => {
            let mut parts = Vec::new();
            if *pending > 0 {
                parts.push(format!("{pending} pending"));
            }
            if *unreservable > 0 {
                parts.push(format!("{unreservable} unreservable"));
            }
            HealthCandidate {
                category: HealthCategory::FAULT,
                message: format!("{} bad memory pages", parts.join(", ")),
                observed_at,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::model::{PartitionId, PhysicalGpuId, Timestamp};
    use crate::source::{KernelPartitionFast, KernelPartitionSlow};

    fn fast_sample() -> KernelFastSample {
        KernelFastSample {
            gpu: PhysicalGpuId::new("gpu-a"),
            read_wall: Timestamp::now(),
            read_mono: Instant::now(),
            hotspot_millic: Reading::Value(87_000),
            socket_power_microwatts: Reading::Value(318_000_000),
            energy: Some((42, 1.0 / 65536.0)),
            partitions: vec![
                KernelPartitionFast {
                    partition: PartitionId::new("gpu-a-xcp-0"),
                    is_primary: true,
                    activity_centipercent: Reading::Value(9_700),
                    mem_used_bytes: Reading::Value(1_000),
                    mem_total_bytes: Reading::Value(2_000),
                    mem_ctl_centipercent: Reading::Sentinel,
                },
                KernelPartitionFast {
                    partition: PartitionId::new("gpu-a-xcp-1"),
                    is_primary: false,
                    activity_centipercent: Reading::Absent,
                    mem_used_bytes: Reading::Asleep,
                    mem_total_bytes: Reading::PermissionDenied,
                    mem_ctl_centipercent: Reading::Absent,
                },
            ],
        }
    }

    fn outcome<'a>(
        batch: &'a MetricBatch,
        metric: MetricKey,
    ) -> &'a Result<Value, ObservationState> {
        &batch
            .results
            .iter()
            .find(|r| r.metric == metric)
            .unwrap()
            .outcome
    }

    #[test]
    fn units_convert_exactly_without_clamping() {
        let batches = kernel_fast(fast_sample());
        let socket = &batches[0];
        assert_eq!(socket.partition, None);
        assert_eq!(
            outcome(socket, MetricKey::Socket(SocketMetric::HotspotCelsius)),
            &Ok(Value::F64(87.0))
        );
        assert_eq!(
            outcome(socket, MetricKey::Socket(SocketMetric::SocketWatts)),
            &Ok(Value::F64(318.0))
        );
        assert_eq!(socket.energy_accumulator, Some((42, 1.0 / 65536.0)));
        let primary = &batches[1];
        assert_eq!(
            outcome(
                primary,
                MetricKey::Partition(PartitionMetric::ActivityPercent)
            ),
            &Ok(Value::F64(97.0))
        );
        assert_eq!(
            outcome(primary, MetricKey::Partition(PartitionMetric::MemUsedBytes)),
            &Ok(Value::U64(1_000))
        );
    }

    #[test]
    fn each_failure_maps_to_exactly_one_state() {
        let batches = kernel_fast(fast_sample());
        let primary = &batches[1];
        // Sentinel on the primary partition: structural hardware absence.
        assert_eq!(
            outcome(
                primary,
                MetricKey::Partition(PartitionMetric::MemCtlActivityPercent)
            ),
            &Err(ObservationState::UNSUPPORTED_HARDWARE)
        );
        let secondary = &batches[2];
        // Structural absence on a secondary XCP: owned by the primary.
        assert_eq!(
            outcome(
                secondary,
                MetricKey::Partition(PartitionMetric::ActivityPercent)
            ),
            &Err(ObservationState::REPORTED_BY_PRIMARY_PARTITION)
        );
        // Runtime evidence is never rewritten by partition scope.
        assert_eq!(
            outcome(
                secondary,
                MetricKey::Partition(PartitionMetric::MemUsedBytes)
            ),
            &Err(ObservationState::ASLEEP)
        );
        assert_eq!(
            outcome(
                secondary,
                MetricKey::Partition(PartitionMetric::MemTotalBytes)
            ),
            &Err(ObservationState::PERMISSION_DENIED)
        );
    }

    #[test]
    fn slow_sample_normalizes_limits_clock_and_health() {
        let sample = KernelSlowSample {
            gpu: PhysicalGpuId::new("gpu-a"),
            read_wall: Timestamp::now(),
            read_mono: Instant::now(),
            limit_millic: Reading::Value(95_000),
            cap_microwatts: Reading::UnsupportedDriver,
            partitions: vec![KernelPartitionSlow {
                partition: PartitionId::new("gpu-a-xcp-0"),
                is_primary: true,
                gfx_clock_hz: Reading::Value(1_700_000_000),
            }],
            health: vec![
                KernelHealthSignal::ThrottleActive {
                    reasons: "thermal".to_owned(),
                },
                KernelHealthSignal::EccErrors { uncorrectable: 2 },
                KernelHealthSignal::BadPages {
                    pending: 1,
                    unreservable: 0,
                },
            ],
        };
        let (batches, report) = kernel_slow(sample);
        assert_eq!(
            outcome(&batches[0], MetricKey::Socket(SocketMetric::LimitCelsius)),
            &Ok(Value::F64(95.0))
        );
        assert_eq!(
            outcome(&batches[0], MetricKey::Socket(SocketMetric::CapWatts)),
            &Err(ObservationState::UNSUPPORTED_DRIVER_VERSION)
        );
        assert_eq!(
            outcome(
                &batches[1],
                MetricKey::Partition(PartitionMetric::GfxClockMhz)
            ),
            &Ok(Value::F64(1_700.0))
        );
        assert_eq!(report.origin, Origin::Kernel);
        let messages: Vec<&str> = report
            .candidates
            .iter()
            .map(|c| c.message.as_str())
            .collect();
        assert_eq!(
            messages,
            vec![
                "thermal throttle active",
                "2 uncorrectable ECC errors",
                "1 pending bad memory pages",
            ]
        );
        assert_eq!(report.candidates[0].category.as_str(), "throttle");
        assert_eq!(report.candidates[1].category.as_str(), "fault");
    }
}
