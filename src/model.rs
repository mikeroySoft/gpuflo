//! Canonical, semver-supported gpuflo vocabulary.
//!
//! Every metric is an [`Observation`]: either a numeric value with its source
//! observation time or exactly one explicit [`ObservationState`]. Physical
//! GPUs own socket-scoped power and temperature; XCP partitions own
//! partition-scoped activity, memory, and clocks. Nothing here synthesizes
//! aggregates or collapses unknown future strings into known defaults.

use std::borrow::Cow;
use std::fmt;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::BorrowedFormatItem;
use time::macros::format_description;

/// The machine-output schema major version produced by this build.
pub const SCHEMA_VERSION: u32 = 1;

/// RFC 3339 UTC with fixed microsecond precision.
///
/// The clock feeding observations is at least microsecond-precise, so six
/// subsecond digits satisfy the "milliseconds at minimum" contract without
/// inventing precision.
const TIMESTAMP_FORMAT: &[BorrowedFormatItem<'_>] =
    format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:6]Z");

/// A UTC wall-clock instant attached to an observation or snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Timestamp(OffsetDateTime);

impl Timestamp {
    /// Captures the current wall-clock time.
    pub fn now() -> Self {
        Self(OffsetDateTime::now_utc())
    }

    /// Wraps an explicit instant (test seams and source-reported times).
    pub fn from_odt(instant: OffsetDateTime) -> Self {
        Self(instant.to_offset(time::UtcOffset::UTC))
    }

    /// The wrapped instant.
    pub fn as_odt(&self) -> OffsetDateTime {
        self.0
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0.format(TIMESTAMP_FORMAT) {
            Ok(text) => f.write_str(&text),
            Err(_) => Err(fmt::Error),
        }
    }
}

impl Serialize for Timestamp {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        let parsed = OffsetDateTime::parse(&text, &time::format_description::well_known::Rfc3339)
            .map_err(serde::de::Error::custom)?;
        Ok(Self::from_odt(parsed))
    }
}

macro_rules! string_backed {
    ($(#[$doc:meta])* $name:ident { $($(#[$vdoc:meta])* $konst:ident => $text:literal),+ $(,)? }) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Cow<'static, str>);

        impl $name {
            $(
                $(#[$vdoc])*
                pub const $konst: Self = Self(Cow::Borrowed($text));
            )+

            /// Wraps an arbitrary string, preserving unknown future values.
            pub fn new(value: impl Into<String>) -> Self {
                let value = value.into();
                Self(Cow::Owned(value))
            }

            /// The canonical string form.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

string_backed! {
    /// Why an observation has no current numeric value.
    ObservationState {
        /// The source reports unsupported, emits a documented sentinel, or the
        /// metric is structurally inapplicable to this hardware/topology.
        UNSUPPORTED_HARDWARE => "unsupported_hardware",
        /// An explicit driver, ABI, or `gpu_metrics` layout mismatch prevents
        /// this build from interpreting the metric.
        UNSUPPORTED_DRIVER_VERSION => "unsupported_driver_version",
        /// The active source exists but access is denied.
        PERMISSION_DENIED => "permission_denied",
        /// Runtime-suspended amdgpu denied a read; polling must not wake it.
        ASLEEP => "asleep",
        /// The observation is owned and reported at the primary XCP scope.
        REPORTED_BY_PRIMARY_PARTITION => "reported_by_primary_partition",
        /// A previously good observation exceeded its freshness threshold.
        STALE => "stale",
        /// A recognized or required source is unavailable, timed out, returned
        /// malformed data, or failed unexpectedly with no fresh value retained.
        SOURCE_ERROR => "source_error",
    }
}

string_backed! {
    /// Priority class of the highest-priority source-backed health condition.
    HealthCategory {
        /// Uncorrectable fault, reset-required state, or severe RAS condition.
        FAULT => "fault",
        /// Active thermal, power, current, or other source-reported throttle.
        THROTTLE => "throttle",
        /// A source-reported limit being reached.
        LIMIT => "limit",
        /// Telemetry unavailable, stale, permission-limited, or asleep.
        TELEMETRY => "telemetry",
        /// Source-reported memory allocation pressure or failure.
        MEMORY_PRESSURE => "memory_pressure",
        /// No active limits or faults.
        NONE => "none",
    }
}

string_backed! {
    /// The applicable GPU memory pool.
    MemoryPool {
        /// Dedicated VRAM on a discrete GPU.
        VRAM => "vram",
        /// Explicitly shared memory on an APU.
        SHARED => "shared",
        /// GTT memory on an APU.
        GTT => "gtt",
    }
}

/// A metric at one hardware scope: a value with its source time or a state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Observation<T> {
    /// A current numeric value and when its source observed it.
    Value {
        /// The observed value in the canonical unit named by the field.
        value: T,
        /// When the source observed the value.
        observed_at: Timestamp,
    },
    /// No current numeric value; the state says why. A stale observation
    /// keeps the last good observation time but never its numeric value.
    Unavailable {
        /// Why the observation is unavailable.
        state: ObservationState,
        /// Last good observation time; present only for [`ObservationState::STALE`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        observed_at: Option<Timestamp>,
    },
}

impl<T> Observation<T> {
    /// A current value observed at `observed_at`.
    pub fn value(value: T, observed_at: Timestamp) -> Self {
        Self::Value { value, observed_at }
    }

    /// An unavailable observation without a retained time.
    pub fn unavailable(state: ObservationState) -> Self {
        Self::Unavailable {
            state,
            observed_at: None,
        }
    }

    /// A stale observation retaining the last good observation time.
    pub fn stale(last_good: Timestamp) -> Self {
        Self::Unavailable {
            state: ObservationState::STALE,
            observed_at: Some(last_good),
        }
    }

    /// The current value, if one exists.
    pub fn current(&self) -> Option<&T> {
        match self {
            Self::Value { value, .. } => Some(value),
            Self::Unavailable { .. } => None,
        }
    }

    /// The unavailable state, if the observation has no current value.
    pub fn state(&self) -> Option<&ObservationState> {
        match self {
            Self::Value { .. } => None,
            Self::Unavailable { state, .. } => Some(state),
        }
    }

    /// The source observation time, when known (fresh or stale).
    pub fn observed_at(&self) -> Option<Timestamp> {
        match self {
            Self::Value { observed_at, .. } => Some(*observed_at),
            Self::Unavailable { observed_at, .. } => *observed_at,
        }
    }
}

/// A validated PCI bus/device/function address, e.g. `0000:41:00.0`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct PciBdf(String);

impl PciBdf {
    /// Parses a `dddd:bb:dd.f` address, lowercasing hex digits.
    pub fn parse(text: &str) -> Result<Self, InvalidPciBdf> {
        let bytes = text.as_bytes();
        let valid = bytes.len() == 12
            && bytes[4] == b':'
            && bytes[7] == b':'
            && bytes[10] == b'.'
            && [0usize, 1, 2, 3, 5, 6, 8, 9, 11]
                .iter()
                .all(|&i| bytes[i].is_ascii_hexdigit());
        if !valid {
            return Err(InvalidPciBdf(text.to_owned()));
        }
        Ok(Self(text.to_ascii_lowercase()))
    }

    /// The canonical lowercase string form.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PciBdf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for PciBdf {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// A PCI address that failed [`PciBdf::parse`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid PCI BDF address: {0:?}")]
pub struct InvalidPciBdf(String);

/// Opaque stable identity of one physical GPU, usable for joins/persistence.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PhysicalGpuId(String);

impl PhysicalGpuId {
    /// Wraps a stable identity string.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The identity string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PhysicalGpuId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Opaque stable identity of one XCP partition within a physical GPU.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PartitionId(String);

impl PartitionId {
    /// Wraps a stable identity string.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The identity string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PartitionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The highest-priority active source-backed health condition of one GPU.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Health {
    /// Priority class for automation.
    pub category: HealthCategory,
    /// One factual source-backed sentence; never a score.
    pub message: String,
    /// When the condition (or its absence) was established.
    pub observed_at: Timestamp,
}

/// Socket-scoped temperature owned by the physical GPU.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Temperature {
    /// Hotspot (junction) temperature.
    pub hotspot_celsius: Observation<f64>,
    /// Source-reported slowdown or critical limit.
    pub limit_celsius: Observation<f64>,
}

/// Socket-scoped power owned by the physical GPU.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Power {
    /// Current socket power draw.
    pub socket_watts: Observation<f64>,
    /// Source-reported power cap.
    pub cap_watts: Observation<f64>,
}

/// Partition-scoped memory occupancy for the applicable pool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Memory {
    /// The applicable pool; unknown future pools round-trip unchanged.
    pub pool: MemoryPool,
    /// Used capacity in bytes.
    pub used_bytes: Observation<u64>,
    /// Total capacity in bytes.
    pub total_bytes: Observation<u64>,
    /// Used capacity relative to the pool, percent.
    pub occupancy_percent: Observation<f64>,
}

/// One XCP partition. Activity, memory, and clocks are partition-scoped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Partition {
    /// Opaque stable partition identity.
    pub id: PartitionId,
    /// Ephemeral display index; presentation only, never identity.
    pub index: u32,
    /// Whether this XCP reports socket-scoped telemetry for its physical GPU.
    pub is_primary: bool,
    /// Device-level GFX activity, percent.
    pub activity_percent: Observation<f64>,
    /// Memory occupancy of the applicable pool.
    pub memory: Memory,
    /// Current GFX clock.
    pub gfx_clock_mhz: Observation<f64>,
    /// Optional memory-controller activity, percent.
    pub memory_controller_activity_percent: Observation<f64>,
}

/// One physical GPU package/socket, owning socket-scoped observations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PhysicalGpu {
    /// Opaque stable identity.
    pub id: PhysicalGpuId,
    /// Ephemeral display index; presentation only, never identity.
    pub index: u32,
    /// PCI address.
    pub bdf: PciBdf,
    /// Marketing/model name.
    pub name: String,
    /// Optional source-reported UUID identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uuid: Option<String>,
    /// Optional source-reported serial identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
    /// Highest-priority active source-backed condition.
    pub health: Health,
    /// Socket-scoped temperature.
    pub temperature: Temperature,
    /// Socket-scoped power.
    pub power: Power,
    /// Nested XCP partitions; always at least one, even in SPX operation.
    pub partitions: Vec<Partition>,
}

/// One exportable view of every discovered physical GPU.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    /// Integer major version of the machine-output schema.
    pub schema_version: u32,
    /// Version of the producing gpuflo binary; not a payload compatibility key.
    pub gpuflo_version: String,
    /// When gpuflo assembled this exportable snapshot.
    pub sampled_at: Timestamp,
    /// Run-local exportable snapshot counter; present on streamed records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence: Option<u64>,
    /// Every discovered physical GPU in display order.
    pub gpus: Vec<PhysicalGpu>,
}

impl Snapshot {
    /// Assembles a snapshot for the current build.
    pub fn new(sampled_at: Timestamp, sequence: Option<u64>, gpus: Vec<PhysicalGpu>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            gpuflo_version: env!("CARGO_PKG_VERSION").to_owned(),
            sampled_at,
            sequence,
            gpus,
        }
    }
}

/// Canonical human-readable phrase for an unavailable observation.
///
/// `stale` requires an age computed against snapshot assembly time, so it is
/// rendered by callers that know both times; this covers the stateless states.
pub(crate) fn state_phrase(state: &ObservationState) -> &str {
    match state.as_str() {
        "unsupported_hardware" => "unsupported hardware",
        "unsupported_driver_version" => "unsupported driver version",
        "permission_denied" => "permission denied",
        "asleep" => "asleep",
        "reported_by_primary_partition" => "reported by primary partition",
        "source_error" => "source error",
        "stale" => "stale",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn ts() -> Timestamp {
        Timestamp::from_odt(datetime!(2026-08-20 23:45:12.247381 UTC))
    }

    #[test]
    fn value_observation_serializes_value_and_time() {
        let json = serde_json::to_value(Observation::value(97.0, ts())).unwrap();
        assert_eq!(json["value"], 97.0);
        assert_eq!(json["observed_at"], "2026-08-20T23:45:12.247381Z");
    }

    #[test]
    fn stale_observation_serializes_without_a_value() {
        let json = serde_json::to_value(Observation::<f64>::stale(ts())).unwrap();
        assert_eq!(json["state"], "stale");
        assert_eq!(json["observed_at"], "2026-08-20T23:45:12.247381Z");
        assert!(json.get("value").is_none());
        // Round-trip keeps the stale form, never a numeric value.
        let back: Observation<f64> = serde_json::from_value(json).unwrap();
        assert!(back.current().is_none());
        assert_eq!(back.state(), Some(&ObservationState::STALE));
        assert_eq!(back.observed_at(), Some(ts()));
    }

    #[test]
    fn plain_unavailable_omits_observed_at() {
        let json = serde_json::to_value(Observation::<u64>::unavailable(ObservationState::ASLEEP))
            .unwrap();
        assert_eq!(json, serde_json::json!({ "state": "asleep" }));
    }

    #[test]
    fn unknown_observation_state_round_trips() {
        let json = serde_json::json!({ "state": "future_state" });
        let obs: Observation<f64> = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(obs.state().unwrap().as_str(), "future_state");
        assert_ne!(obs.state(), Some(&ObservationState::NONEXISTENT()));
        assert_eq!(serde_json::to_value(&obs).unwrap(), json);
    }

    impl ObservationState {
        #[allow(non_snake_case)]
        fn NONEXISTENT() -> Self {
            ObservationState::new("none_of_the_known")
        }
    }

    #[test]
    fn pci_bdf_validates_and_lowercases() {
        assert_eq!(
            PciBdf::parse("0000:41:00.0").unwrap().as_str(),
            "0000:41:00.0"
        );
        assert_eq!(
            PciBdf::parse("0000:C1:00.0").unwrap().as_str(),
            "0000:c1:00.0"
        );
        assert!(PciBdf::parse("41:00.0").is_err());
        assert!(PciBdf::parse("0000:zz:00.0").is_err());
        assert!(PciBdf::parse("0000:41:00:0").is_err());
    }

    #[test]
    fn snapshot_keeps_socket_and_partition_scope_separate() {
        let gpu = PhysicalGpu {
            id: PhysicalGpuId::new("gpu-0000:41:00.0"),
            index: 0,
            bdf: PciBdf::parse("0000:41:00.0").unwrap(),
            name: "AMD Instinct MI300X".to_owned(),
            uuid: None,
            serial: None,
            health: Health {
                category: HealthCategory::NONE,
                message: "no active limits or faults".to_owned(),
                observed_at: ts(),
            },
            temperature: Temperature {
                hotspot_celsius: Observation::value(74.0, ts()),
                limit_celsius: Observation::value(95.0, ts()),
            },
            power: Power {
                socket_watts: Observation::value(318.0, ts()),
                cap_watts: Observation::value(320.0, ts()),
            },
            partitions: vec![Partition {
                id: PartitionId::new("gpu-0000:41:00.0-xcp-0"),
                index: 0,
                is_primary: true,
                activity_percent: Observation::value(97.0, ts()),
                memory: Memory {
                    pool: MemoryPool::VRAM,
                    used_bytes: Observation::value(195_850_508_697, ts()),
                    total_bytes: Observation::value(206_158_430_208, ts()),
                    occupancy_percent: Observation::value(95.0, ts()),
                },
                gfx_clock_mhz: Observation::value(1700.0, ts()),
                memory_controller_activity_percent: Observation::unavailable(
                    ObservationState::UNSUPPORTED_HARDWARE,
                ),
            }],
        };
        let snapshot = Snapshot::new(ts(), None, vec![gpu]);
        let json = serde_json::to_value(&snapshot).unwrap();

        assert_eq!(json["schema_version"], 1);
        assert!(json.get("sequence").is_none());
        let gpu = &json["gpus"][0];
        // Socket scope stays on the physical GPU.
        assert!(gpu.get("temperature").is_some());
        assert!(gpu.get("power").is_some());
        // Partition scope stays on the partition; no physical aggregate.
        assert!(gpu.get("activity_percent").is_none());
        assert!(gpu.get("memory").is_none());
        let part = &gpu["partitions"][0];
        assert!(part.get("activity_percent").is_some());
        assert!(part.get("memory").is_some());
        assert!(part.get("temperature").is_none());
        assert!(part.get("power").is_none());
        // No nulls anywhere in the payload.
        assert!(!format!("{json}").contains("null"));
        // Round-trips.
        let back: Snapshot = serde_json::from_value(json).unwrap();
        assert_eq!(back, snapshot);
    }

    #[test]
    fn canonical_phrases_match_contract() {
        assert_eq!(
            state_phrase(&ObservationState::UNSUPPORTED_HARDWARE),
            "unsupported hardware"
        );
        assert_eq!(
            state_phrase(&ObservationState::REPORTED_BY_PRIMARY_PARTITION),
            "reported by primary partition"
        );
        assert_eq!(state_phrase(&ObservationState::new("mystery")), "mystery");
    }
}
