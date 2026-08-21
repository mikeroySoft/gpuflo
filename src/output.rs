//! Non-interactive output surfaces: `--once`/`--tiny` human lines and
//! `--json`/`--json-stream` snapshots.
//!
//! Everything here consumes canonical [`crate::model`] types, never reads the
//! environment, and never emits ANSI escape bytes. Unavailable observations
//! render their exact canonical phrase — never zero, a dash, or `N/A`.

use std::io::{self, Write};

use crate::model::{
    Memory, Observation, ObservationState, PhysicalGpu, Power, Snapshot, Temperature, Timestamp,
    state_phrase,
};

/// Writes one pretty JSON snapshot followed by a single trailing newline.
///
/// `--json` snapshots carry no `sequence`; the caller passes it as `None`.
pub(crate) fn write_json(out: &mut impl Write, snapshot: &Snapshot) -> io::Result<()> {
    debug_assert!(
        snapshot.sequence.is_none(),
        "--json snapshots must not carry a sequence"
    );
    serde_json::to_writer_pretty(&mut *out, snapshot)?;
    out.write_all(b"\n")
}

/// Writes one compact NDJSON snapshot line and flushes it.
///
/// `--json-stream` records carry `sequence`; the caller sets it.
pub(crate) fn write_ndjson_line(out: &mut impl Write, snapshot: &Snapshot) -> io::Result<()> {
    serde_json::to_writer(&mut *out, snapshot)?;
    out.write_all(b"\n")?;
    out.flush()
}

/// Writes one `--once` line for a physical GPU, in the contract's semantic
/// order: identity, primary-XCP label (multi-partition only), activity,
/// memory, hotspot, power, clock, health sentence.
pub(crate) fn write_once_line(
    out: &mut impl Write,
    gpu: &PhysicalGpu,
    sampled_at: Timestamp,
) -> io::Result<()> {
    let primary = primary_partition(gpu);
    write_identity(out, gpu)?;
    if gpu.partitions.len() > 1 {
        write!(out, " | xcp {}", primary.index)?;
    }
    write_activity(out, &primary.activity_percent, sampled_at)?;
    write_memory(out, &primary.memory, sampled_at)?;
    write_hotspot(out, &gpu.temperature, sampled_at)?;
    write_power(out, &gpu.power, sampled_at)?;
    write_clock(out, &primary.gfx_clock_mhz, sampled_at)?;
    writeln!(out, " | {}", gpu.health.message)
}

/// Writes one `--tiny` status line: identity, activity, memory, hotspot,
/// health sentence. No power or clock.
pub(crate) fn write_tiny_line(
    out: &mut impl Write,
    gpu: &PhysicalGpu,
    sampled_at: Timestamp,
) -> io::Result<()> {
    let primary = primary_partition(gpu);
    write_identity(out, gpu)?;
    write_activity(out, &primary.activity_percent, sampled_at)?;
    write_memory(out, &primary.memory, sampled_at)?;
    write_hotspot(out, &gpu.temperature, sampled_at)?;
    writeln!(out, " | {}", gpu.health.message)
}

/// The partition whose activity, memory, and clock represent the GPU on one
/// human line. The model guarantees at least one partition.
fn primary_partition(gpu: &PhysicalGpu) -> &crate::model::Partition {
    gpu.partitions
        .iter()
        .find(|partition| partition.is_primary)
        .or_else(|| gpu.partitions.first())
        .expect("a physical GPU always owns at least one partition")
}

fn write_identity(out: &mut impl Write, gpu: &PhysicalGpu) -> io::Result<()> {
    write!(out, "gpu {} {} [{}]", gpu.index, gpu.name, gpu.bdf)
}

fn write_activity(
    out: &mut impl Write,
    activity: &Observation<f64>,
    sampled_at: Timestamp,
) -> io::Result<()> {
    write!(out, " | activity ")?;
    match activity {
        Observation::Value { value, .. } => write!(out, "{value:.0}%"),
        Observation::Unavailable { state, observed_at } => {
            write_unavailable(out, state, *observed_at, sampled_at)
        }
    }
}

fn write_memory(out: &mut impl Write, memory: &Memory, sampled_at: Timestamp) -> io::Result<()> {
    write!(out, " | memory ")?;
    let label = pool_label(memory.pool.as_str());
    match (&memory.used_bytes, &memory.total_bytes) {
        (Observation::Value { value: used, .. }, Observation::Value { value: total, .. }) => {
            write!(out, "{}/{} GiB {label}", Gib(*used), Gib(*total))?;
            if let Observation::Value {
                value: occupancy, ..
            } = &memory.occupancy_percent
            {
                write!(out, " ({occupancy:.0}%)")?;
            }
            Ok(())
        }
        // Highest-information unavailable phrase: prefer used's state.
        (Observation::Unavailable { state, observed_at }, _)
        | (_, Observation::Unavailable { state, observed_at }) => {
            write!(out, "{label} ")?;
            write_unavailable(out, state, *observed_at, sampled_at)
        }
    }
}

fn write_hotspot(
    out: &mut impl Write,
    temperature: &Temperature,
    sampled_at: Timestamp,
) -> io::Result<()> {
    write!(out, " | hotspot ")?;
    match &temperature.hotspot_celsius {
        Observation::Value { value, .. } => match &temperature.limit_celsius {
            Observation::Value { value: limit, .. } => write!(out, "{value:.0}/{limit:.0}°C"),
            Observation::Unavailable { .. } => write!(out, "{value:.0}°C"),
        },
        Observation::Unavailable { state, observed_at } => {
            write_unavailable(out, state, *observed_at, sampled_at)
        }
    }
}

fn write_power(out: &mut impl Write, power: &Power, sampled_at: Timestamp) -> io::Result<()> {
    write!(out, " | power ")?;
    match &power.socket_watts {
        Observation::Value { value, .. } => match &power.cap_watts {
            Observation::Value { value: cap, .. } => write!(out, "{value:.0}/{cap:.0} W"),
            Observation::Unavailable { .. } => write!(out, "{value:.0} W"),
        },
        Observation::Unavailable { state, observed_at } => {
            write_unavailable(out, state, *observed_at, sampled_at)
        }
    }
}

fn write_clock(
    out: &mut impl Write,
    clock: &Observation<f64>,
    sampled_at: Timestamp,
) -> io::Result<()> {
    write!(out, " | clock ")?;
    match clock {
        Observation::Value { value, .. } => write!(out, "{value:.0} MHz"),
        Observation::Unavailable { state, observed_at } => {
            write_unavailable(out, state, *observed_at, sampled_at)
        }
    }
}

/// Writes the canonical phrase for an unavailable observation. A stale
/// observation renders its age against snapshot assembly time as
/// whole-plus-tenths seconds, e.g. `stale 4.2s`.
fn write_unavailable(
    out: &mut impl Write,
    state: &ObservationState,
    observed_at: Option<Timestamp>,
    sampled_at: Timestamp,
) -> io::Result<()> {
    if *state == ObservationState::STALE
        && let Some(last_good) = observed_at
    {
        let age = (sampled_at.as_odt() - last_good.as_odt())
            .as_seconds_f64()
            .max(0.0);
        return write!(out, "stale {age:.1}s");
    }
    out.write_all(state_phrase(state).as_bytes())
}

/// Human label for a memory pool string: known pools get their display case,
/// unknown pools pass through as-is.
fn pool_label(pool: &str) -> &str {
    match pool {
        "vram" => "VRAM",
        "shared" => "shared",
        "gtt" => "GTT",
        other => other,
    }
}

/// Byte count rendered as IEC gibibytes with one decimal, e.g. `182.4`.
struct Gib(u64);

impl std::fmt::Display for Gib {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
        write!(f, "{:.1}", self.0 as f64 / GIB)
    }
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;
    use crate::model::{
        Health, HealthCategory, MemoryPool, Partition, PartitionId, PciBdf, PhysicalGpuId,
    };

    fn ts(odt: time::OffsetDateTime) -> Timestamp {
        Timestamp::from_odt(odt)
    }

    fn sampled_at() -> Timestamp {
        ts(datetime!(2026-08-20 23:45:12.250 UTC))
    }

    fn observed() -> Timestamp {
        ts(datetime!(2026-08-20 23:45:12.247381 UTC))
    }

    fn value(v: f64) -> Observation<f64> {
        Observation::value(v, observed())
    }

    fn partition(index: u32, is_primary: bool) -> Partition {
        Partition {
            id: PartitionId::new(format!("gpu-73fbc1-xcp-{index}")),
            index,
            is_primary,
            activity_percent: value(97.0),
            memory: Memory {
                pool: MemoryPool::VRAM,
                used_bytes: Observation::value(195_850_508_697, observed()),
                total_bytes: Observation::value(206_158_430_208, observed()),
                occupancy_percent: value(95.0),
            },
            gfx_clock_mhz: value(1700.0),
            memory_controller_activity_percent: value(64.0),
        }
    }

    fn gpu() -> PhysicalGpu {
        PhysicalGpu {
            id: PhysicalGpuId::new("gpu-73fbc1"),
            index: 0,
            bdf: PciBdf::parse("0000:41:00.0").unwrap(),
            name: "AMD Instinct MI300X".to_owned(),
            uuid: None,
            serial: None,
            health: Health {
                category: HealthCategory::NONE,
                message: "no active limits or faults".to_owned(),
                observed_at: observed(),
            },
            temperature: Temperature {
                hotspot_celsius: value(74.0),
                limit_celsius: value(95.0),
            },
            power: Power {
                socket_watts: value(318.0),
                cap_watts: value(320.0),
            },
            partitions: vec![partition(0, true)],
        }
    }

    fn snapshot(sequence: Option<u64>) -> Snapshot {
        Snapshot::new(sampled_at(), sequence, vec![gpu()])
    }

    fn once_line(gpu: &PhysicalGpu) -> String {
        let mut out = Vec::new();
        write_once_line(&mut out, gpu, sampled_at()).unwrap();
        String::from_utf8(out).unwrap()
    }

    fn tiny_line(gpu: &PhysicalGpu) -> String {
        let mut out = Vec::new();
        write_tiny_line(&mut out, gpu, sampled_at()).unwrap();
        String::from_utf8(out).unwrap()
    }

    /// Asserts each needle occurs, in order, each after the previous one.
    fn assert_ordered(haystack: &str, needles: &[&str]) {
        let mut from = 0;
        for needle in needles {
            match haystack[from..].find(needle) {
                Some(at) => from += at + needle.len(),
                None => panic!("{needle:?} missing (in order) from {haystack:?}"),
            }
        }
    }

    #[test]
    fn once_line_semantic_order_single_partition() {
        let line = once_line(&gpu());
        assert_ordered(
            &line,
            &[
                "gpu 0 AMD Instinct MI300X [0000:41:00.0]",
                "activity 97%",
                "memory 182.4/192.0 GiB",
                "VRAM",
                "(95%)",
                "hotspot 74/95°C",
                "power 318/320 W",
                "clock 1700 MHz",
                "no active limits or faults",
            ],
        );
        assert!(
            !line.contains("xcp"),
            "single-partition GPU must not label an XCP: {line:?}"
        );
        assert!(line.ends_with('\n'));
    }

    #[test]
    fn once_line_labels_primary_xcp_when_partitioned() {
        let mut gpu = gpu();
        gpu.partitions = vec![partition(2, false), partition(3, true)];
        let line = once_line(&gpu);
        assert_ordered(&line, &["[0000:41:00.0]", "xcp 3", "activity"]);
    }

    #[test]
    fn once_line_renders_every_canonical_unavailable_phrase() {
        let mut gpu = gpu();
        gpu.temperature.hotspot_celsius =
            Observation::unavailable(ObservationState::PERMISSION_DENIED);
        gpu.power.socket_watts = Observation::unavailable(ObservationState::ASLEEP);
        let p = &mut gpu.partitions[0];
        p.activity_percent = Observation::unavailable(ObservationState::UNSUPPORTED_HARDWARE);
        p.memory.used_bytes =
            Observation::unavailable(ObservationState::UNSUPPORTED_DRIVER_VERSION);
        p.gfx_clock_mhz = Observation::unavailable(ObservationState::SOURCE_ERROR);
        let line = once_line(&gpu);
        assert_ordered(
            &line,
            &[
                "activity unsupported hardware",
                "memory VRAM unsupported driver version",
                "hotspot permission denied",
                "power asleep",
                "clock source error",
            ],
        );

        let mut gpu = self::gpu();
        gpu.partitions = vec![
            partition(0, true),
            Partition {
                activity_percent: Observation::unavailable(
                    ObservationState::REPORTED_BY_PRIMARY_PARTITION,
                ),
                ..partition(1, false)
            },
        ];
        gpu.partitions.swap(0, 1); // Primary lookup must not depend on order.
        let line = once_line(&gpu);
        assert!(
            line.contains("activity 97%"),
            "primary partition drives the line: {line:?}"
        );

        // Render a secondary-only view through the shared phrase path.
        let mut out = Vec::new();
        write_activity(
            &mut out,
            &Observation::unavailable(ObservationState::REPORTED_BY_PRIMARY_PARTITION),
            sampled_at(),
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            " | activity reported by primary partition"
        );
    }

    #[test]
    fn stale_renders_age_not_value() {
        let mut gpu = gpu();
        let last_good = ts(datetime!(2026-08-20 23:45:08.050 UTC));
        gpu.partitions[0].activity_percent = Observation::stale(last_good);
        let line = once_line(&gpu);
        assert!(
            line.contains("activity stale 4.2s"),
            "stale age missing: {line:?}"
        );
        assert!(
            !line.contains("activity 97"),
            "stale must never leak the old value: {line:?}"
        );
    }

    #[test]
    fn stale_memory_uses_pool_label_and_age() {
        let mut gpu = gpu();
        let last_good = ts(datetime!(2026-08-20 23:44:10.250 UTC));
        gpu.partitions[0].memory.used_bytes = Observation::stale(last_good);
        let line = once_line(&gpu);
        assert!(
            line.contains("memory VRAM stale 62.0s"),
            "stale memory phrase missing: {line:?}"
        );
    }

    #[test]
    fn unavailable_limits_are_omitted_not_phrased() {
        let mut gpu = gpu();
        gpu.temperature.limit_celsius = Observation::unavailable(ObservationState::ASLEEP);
        gpu.power.cap_watts = Observation::unavailable(ObservationState::ASLEEP);
        let line = once_line(&gpu);
        assert!(line.contains("hotspot 74°C"), "{line:?}");
        assert!(line.contains("power 318 W"), "{line:?}");
        assert!(!line.contains("74/"), "{line:?}");
        assert!(!line.contains("318/"), "{line:?}");
    }

    #[test]
    fn unknown_state_and_pool_render_as_is() {
        let mut gpu = gpu();
        gpu.partitions[0].activity_percent =
            Observation::unavailable(ObservationState::new("thermally_gated"));
        gpu.partitions[0].memory.pool = MemoryPool::new("carveout");
        let line = once_line(&gpu);
        assert!(line.contains("activity thermally_gated"), "{line:?}");
        assert!(line.contains("GiB carveout"), "{line:?}");
    }

    #[test]
    fn lines_never_contain_ansi_escapes() {
        let mut gpu = gpu();
        gpu.partitions[0].activity_percent = Observation::stale(observed());
        for line in [once_line(&gpu), tiny_line(&gpu)] {
            assert!(!line.contains('\x1b'), "ANSI escape leaked: {line:?}");
        }
    }

    #[test]
    fn tiny_line_keeps_only_identity_activity_memory_hotspot_health() {
        let line = tiny_line(&gpu());
        assert_ordered(
            &line,
            &[
                "gpu 0 AMD Instinct MI300X [0000:41:00.0]",
                "activity 97%",
                "memory 182.4/192.0 GiB",
                "hotspot 74/95°C",
                "no active limits or faults",
            ],
        );
        assert!(!line.contains("power"), "{line:?}");
        assert!(!line.contains("clock"), "{line:?}");
        assert!(!line.contains("MHz"), "{line:?}");
        assert!(line.ends_with('\n'));
    }

    #[test]
    fn ndjson_is_one_parsing_line_with_trailing_newline() {
        let mut out = Vec::new();
        write_ndjson_line(&mut out, &snapshot(Some(7))).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.ends_with('\n'));
        let body = &text[..text.len() - 1];
        assert!(!body.contains('\n'), "NDJSON record must be one line");
        let parsed: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(parsed["sequence"], serde_json::json!(7));
    }

    #[test]
    fn pretty_json_ends_with_exactly_one_newline_and_parses() {
        let mut out = Vec::new();
        write_json(&mut out, &snapshot(None)).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.ends_with('\n'));
        assert!(!text.ends_with("\n\n"));
        assert!(text.contains('\n'), "pretty output spans lines");
        let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed["schema_version"], serde_json::json!(1));
        assert!(parsed.get("sequence").is_none());
    }
}
