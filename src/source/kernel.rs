//! Kernel telemetry adapter: DRM/sysfs discovery, hwmon, versioned
//! `gpu_metrics` parsing, and RAS collection.
//!
//! Discovery takes an explicit filesystem root (production `/`), never
//! debugfs. `gpu_metrics` is parsed with bounds-checked offset reads against
//! exact `(format_revision, content_revision)` layouts verified from
//! `kgd_pp_interface.h`; unknown layouts are explicit driver-version
//! evidence, never a guessed struct.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Instant;

use super::{
    KernelFastSample, KernelHealthSignal, KernelPartitionFast, KernelPartitionSlow,
    KernelSlowSample, Reading,
};
use crate::model::{MemoryPool, PartitionId, PciBdf, PhysicalGpuId, Timestamp};
use crate::state::{DiscoveredGpu, DiscoveredPartition};

/// Joules per `energy_accumulator` count: 15.259 µJ = 2⁻¹⁶ J.
const ENERGY_JOULES_PER_COUNT: f64 = 1.0 / 65536.0;

/// Reads a small text node into a trimmed string.
fn read_text(path: &Path) -> Reading<String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Reading::Value(text.trim().to_owned()),
        Err(error) => super::kernel_error_reading(&error),
    }
}

/// Reads a text node as an unsigned integer.
fn read_u64(path: &Path) -> Reading<u64> {
    match read_text(path) {
        Reading::Value(text) => match text.parse::<u64>() {
            Ok(value) => Reading::Value(value),
            Err(_) => Reading::Malformed,
        },
        other => other.map(|_| 0),
    }
}

/// Reads a text node as a signed integer.
fn read_i64(path: &Path) -> Reading<i64> {
    match read_text(path) {
        Reading::Value(text) => match text.parse::<i64>() {
            Ok(value) => Reading::Value(value),
            Err(_) => Reading::Malformed,
        },
        other => other.map(|_| 0),
    }
}

// ---------------------------------------------------------------------------
// gpu_metrics parsing
// ---------------------------------------------------------------------------

/// Decoded fields of one supported `gpu_metrics` layout, in documented units.
#[derive(Debug, Clone)]
pub(crate) struct GpuMetricsBlob {
    pub hotspot_millic: Reading<i64>,
    pub socket_power_microwatts: Reading<u64>,
    pub activity_centipercent: Reading<u64>,
    pub umc_centipercent: Reading<u64>,
    pub gfx_clock_hz: Reading<u64>,
    pub energy: Option<(u64, f64)>,
    pub throttle: ThrottleReading,
}

/// Throttle evidence carried by the layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ThrottleReading {
    /// The layout carries no interpretable throttle field.
    None,
    /// Instantaneous status bits (v1.3–v1.5, v2.x).
    Status { status: u32, indep: Option<u64> },
    /// Accumulated throttler residencies (v1.6+).
    Residency {
        counter: u32,
        prochot: u32,
        ppt: u32,
        socket_thm: u32,
        vr_thm: u32,
        hbm_thm: u32,
    },
}

/// Why a blob could not be decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BlobError {
    /// Header shorter than four bytes or content shorter than the
    /// recognized layout requires.
    Truncated,
    /// `(format_revision, content_revision)` this build does not interpret;
    /// v1.9's dynamic pointer layout is deliberately unsupported.
    UnsupportedVersion(u8, u8),
}

/// Bounds-checked little-endian field access over the raw blob.
struct Fields<'a>(&'a [u8]);

impl Fields<'_> {
    fn u16(&self, offset: usize) -> Option<u16> {
        let bytes = self.0.get(offset..offset + 2)?;
        Some(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&self, offset: usize) -> Option<u32> {
        let bytes = self.0.get(offset..offset + 4)?;
        Some(u32::from_le_bytes(bytes.try_into().ok()?))
    }

    fn u64(&self, offset: usize) -> Option<u64> {
        let bytes = self.0.get(offset..offset + 8)?;
        Some(u64::from_le_bytes(bytes.try_into().ok()?))
    }

    /// Sentinel-aware u16 read: the producer memsets unpopulated fields to
    /// all-ones of their declared width.
    fn r16(&self, offset: usize) -> Reading<u64> {
        match self.u16(offset) {
            None => Reading::Malformed,
            Some(0xFFFF) => Reading::Sentinel,
            Some(value) => Reading::Value(u64::from(value)),
        }
    }

    fn r32(&self, offset: usize) -> Reading<u64> {
        match self.u32(offset) {
            None => Reading::Malformed,
            Some(0xFFFF_FFFF) => Reading::Sentinel,
            Some(value) => Reading::Value(u64::from(value)),
        }
    }

    fn r64(&self, offset: usize) -> Option<u64> {
        match self.u64(offset) {
            Some(u64::MAX) | None => None,
            Some(value) => Some(value),
        }
    }
}

/// Throttle status bits with the memset sentinel treated as no data.
fn throttle_bits(raw: Option<u32>) -> u32 {
    match raw {
        Some(0xFFFF_FFFF) | None => 0,
        Some(bits) => bits,
    }
}

/// Parses one supported `gpu_metrics` blob after validating its header.
pub(crate) fn parse_gpu_metrics(bytes: &[u8]) -> Result<GpuMetricsBlob, BlobError> {
    if bytes.len() < 4 {
        return Err(BlobError::Truncated);
    }
    let f = Fields(bytes);
    let structure_size = usize::from(f.u16(0).unwrap_or(0));
    let format = bytes[2];
    let content = bytes[3];

    /// Expected sizes verified against `kgd_pp_interface.h`.
    fn expected_size(format: u8, content: u8) -> Option<usize> {
        Some(match (format, content) {
            (1, 3) => 120,
            (1, 4) => 288,
            (1, 5) => 360,
            (1, 6) => 1664,
            (1, 7) => 2208,
            (1, 8) => 3360,
            (2, 1) => 120,
            (2, 2) => 128,
            (2, 3) => 152,
            (2, 4) => 168,
            (3, 0) => 264,
            _ => return None,
        })
    }

    let Some(expected) = expected_size(format, content) else {
        return Err(BlobError::UnsupportedVersion(format, content));
    };
    // A recognized layout must supply at least its full struct; the sysfs
    // read may be padded but never shorter.
    if bytes.len() < expected || structure_size != expected {
        return Err(BlobError::Truncated);
    }

    let blob = match (format, content) {
        (1, 3) => GpuMetricsBlob {
            hotspot_millic: f.r16(6).map(|c| c as i64 * 1000),
            socket_power_microwatts: f.r16(22).map(|w| w * 1_000_000),
            activity_centipercent: f.r16(16).map(|p| p * 100),
            umc_centipercent: f.r16(18).map(|p| p * 100),
            gfx_clock_hz: f.r16(54).map(|mhz| mhz * 1_000_000),
            energy: f.r64(24).map(|raw| (raw, ENERGY_JOULES_PER_COUNT)),
            throttle: ThrottleReading::Status {
                status: throttle_bits(f.u32(68)),
                indep: f.r64(112),
            },
        },
        (1, content @ 4..=8) => {
            let energy_offset = match content {
                4 => 24,
                5 => 88,
                6 => 16,
                _ => 24, // v1.7 and v1.8
            };
            let clock_offset = match content {
                4 => 240,
                5 => 312,
                6 => 264,
                _ => 296, // v1.7 and v1.8 share the prefix
            };
            let throttle = match content {
                4 => ThrottleReading::Status {
                    status: throttle_bits(f.u32(40)),
                    indep: None,
                },
                5 => ThrottleReading::Status {
                    status: throttle_bits(f.u32(104)),
                    indep: None,
                },
                6 => ThrottleReading::Residency {
                    counter: throttle_bits(f.u32(32)),
                    prochot: f.u32(36).unwrap_or(0),
                    ppt: f.u32(40).unwrap_or(0),
                    socket_thm: f.u32(44).unwrap_or(0),
                    vr_thm: f.u32(48).unwrap_or(0),
                    hbm_thm: f.u32(52).unwrap_or(0),
                },
                _ => ThrottleReading::Residency {
                    counter: throttle_bits(f.u32(40)),
                    prochot: f.u32(44).unwrap_or(0),
                    ppt: f.u32(48).unwrap_or(0),
                    socket_thm: f.u32(52).unwrap_or(0),
                    vr_thm: f.u32(56).unwrap_or(0),
                    hbm_thm: f.u32(60).unwrap_or(0),
                },
            };
            GpuMetricsBlob {
                hotspot_millic: f.r16(4).map(|c| c as i64 * 1000),
                socket_power_microwatts: f.r16(10).map(|w| w * 1_000_000),
                activity_centipercent: f.r16(12).map(|p| p * 100),
                umc_centipercent: f.r16(14).map(|p| p * 100),
                gfx_clock_hz: f.r16(clock_offset).map(|mhz| mhz * 1_000_000),
                energy: f
                    .r64(energy_offset)
                    .map(|raw| (raw, ENERGY_JOULES_PER_COUNT)),
                throttle,
            }
        }
        // v2.1–v2.3 leave numeric unit scales undocumented upstream; only
        // the unitless throttle bits are interpreted. Text sysfs nodes and
        // hwmon carry the numeric APU observations for these layouts.
        (2, content @ 1..=3) => GpuMetricsBlob {
            hotspot_millic: Reading::Absent,
            socket_power_microwatts: Reading::Absent,
            activity_centipercent: Reading::Absent,
            umc_centipercent: Reading::Absent,
            gfx_clock_hz: Reading::Absent,
            energy: None,
            throttle: ThrottleReading::Status {
                status: throttle_bits(f.u32(108)),
                indep: if content >= 2 { f.r64(120) } else { None },
            },
        },
        // v2.4 documents centi-Celsius, centi-percent, mW, and MHz.
        (2, 4) => GpuMetricsBlob {
            hotspot_millic: f.r16(4).map(|centi| centi as i64 * 10),
            socket_power_microwatts: f.r16(40).map(|mw| mw * 1_000),
            activity_centipercent: f.r16(28),
            umc_centipercent: Reading::Absent,
            gfx_clock_hz: f.r16(76).map(|mhz| mhz * 1_000_000),
            energy: None,
            throttle: ThrottleReading::Status {
                status: throttle_bits(f.u32(108)),
                indep: f.r64(120),
            },
        },
        // v3.0 documents [0-100] activity, mW power, MHz clocks; its
        // temperature scale is undocumented, so hwmon owns temperature.
        (3, 0) => GpuMetricsBlob {
            hotspot_millic: Reading::Absent,
            socket_power_microwatts: f.r32(112).map(|mw| mw * 1_000),
            activity_centipercent: f.r16(42).map(|p| p * 100),
            umc_centipercent: Reading::Absent,
            gfx_clock_hz: f.r16(174).map(|mhz| mhz * 1_000_000),
            energy: None,
            throttle: ThrottleReading::None,
        },
        _ => unreachable!("expected_size gated the version"),
    };
    Ok(blob)
}

/// `indep_throttle_status` reason groups (SMU_THROTTLER_* bit ranges):
/// bits 0–7 power, 16–23 current, 32–47 temperature.
fn indep_reasons(indep: u64) -> Vec<&'static str> {
    let mut reasons = Vec::new();
    if indep & 0x0000_0000_0000_00FF != 0 {
        reasons.push("power");
    }
    if indep & 0x0000_0000_00FF_0000 != 0 {
        reasons.push("current");
    }
    if indep & 0x0000_FFFF_0000_0000 != 0 {
        reasons.push("thermal");
    }
    reasons
}

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

/// KFD framebuffer heap types.
const KFD_HEAP_FB_PRIVATE: u64 = 1;
const KFD_HEAP_FB_PUBLIC: u64 = 2;

/// Maps BDFs only when the complete KFD scan contains exactly one explicit
/// framebuffer heap item and no iterator/read ambiguity for that BDF.
fn kfd_heap_types(root: &Path) -> HashMap<PciBdf, u64> {
    let nodes = root.join("sys/class/kfd/kfd/topology/nodes");
    let entries = match std::fs::read_dir(&nodes) {
        Ok(entries) => entries,
        Err(_) => return HashMap::new(),
    };
    let mut evidence: HashMap<PciBdf, (Vec<u64>, bool)> = HashMap::new();
    let mut global_incomplete = false;
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                global_incomplete = true;
                continue;
            }
        };
        let dir = entry.path();
        let properties = match std::fs::read_to_string(dir.join("properties")) {
            Ok(properties) => properties,
            Err(_) => {
                global_incomplete = true;
                continue;
            }
        };
        let mut domain = 0u64;
        let mut location = None;
        let mut simd_count = None;
        for line in properties.lines() {
            let mut parts = line.split_whitespace();
            match (parts.next(), parts.next()) {
                (Some("domain"), Some(value)) => domain = value.parse().unwrap_or(0),
                (Some("location_id"), Some(value)) => location = value.parse::<u64>().ok(),
                (Some("simd_count"), Some(value)) => simd_count = value.parse::<u64>().ok(),
                _ => {}
            }
        }
        if simd_count == Some(0) {
            continue;
        }
        let Some(location) = location else {
            global_incomplete = true;
            continue;
        };
        let bus = (location >> 8) & 0xFF;
        let device = (location >> 3) & 0x1F;
        let function = location & 0x7;
        let Ok(bdf) = PciBdf::parse(&format!("{domain:04x}:{bus:02x}:{device:02x}.{function:x}"))
        else {
            global_incomplete = true;
            continue;
        };
        let record = evidence.entry(bdf).or_insert_with(|| (Vec::new(), false));
        let banks = match std::fs::read_dir(dir.join("mem_banks")) {
            Ok(banks) => banks,
            Err(_) => {
                record.1 = true;
                continue;
            }
        };
        for bank in banks {
            let bank = match bank {
                Ok(bank) => bank,
                Err(_) => {
                    record.1 = true;
                    continue;
                }
            };
            let bank_properties = match std::fs::read_to_string(bank.path().join("properties")) {
                Ok(properties) => properties,
                Err(_) => {
                    record.1 = true;
                    continue;
                }
            };
            for line in bank_properties.lines() {
                let mut parts = line.split_whitespace();
                if let (Some("heap_type"), Some(value)) = (parts.next(), parts.next())
                    && let Ok(heap @ (KFD_HEAP_FB_PRIVATE | KFD_HEAP_FB_PUBLIC)) =
                        value.parse::<u64>()
                {
                    record.0.push(heap);
                }
            }
        }
    }
    if global_incomplete {
        return HashMap::new();
    }
    evidence
        .into_iter()
        .filter_map(|(bdf, (heaps, incomplete))| {
            (!incomplete && heaps.len() == 1).then(|| (bdf, heaps[0]))
        })
        .collect()
}

/// Resolved read paths for one XCP partition device.
#[derive(Debug, Clone)]
struct PartitionPaths {
    id: PartitionId,
    is_primary: bool,
    bdf: PciBdf,
    device: PathBuf,
    used_node: &'static str,
    total_node: &'static str,
}

/// One discovered physical GPU with its resolved kernel paths.
#[derive(Debug, Clone)]
pub(crate) struct KernelDevice {
    pub disc: DiscoveredGpu,
    /// Primary partition device directory (owns `gpu_metrics`).
    primary: PathBuf,
    hwmon: Option<PathBuf>,

    /// hwmon channel index whose label is `junction`, when present.
    hotspot_channel: Option<u32>,
    partitions: Vec<PartitionPaths>,
}
fn parse_amd_pci_names(text: &str) -> HashMap<String, String> {
    let mut names = HashMap::new();
    let mut in_amd = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if !line.starts_with(char::is_whitespace) {
            in_amd = line
                .split_whitespace()
                .next()
                .is_some_and(|vendor| vendor.eq_ignore_ascii_case("1002"));
            continue;
        }
        if !in_amd || line.starts_with("\t\t") {
            continue;
        }
        let mut fields = line.split_whitespace();
        let Some(device) = fields.next() else {
            continue;
        };
        if device.len() != 4 || !device.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            continue;
        }
        let description = fields.collect::<Vec<_>>().join(" ");
        if description.is_empty() {
            continue;
        }
        let preferred = description
            .rfind('[')
            .filter(|_| description.ends_with(']'))
            .map(|start| &description[start + 1..description.len() - 1])
            .unwrap_or(&description);
        let name = if preferred.starts_with("AMD ") {
            preferred.to_owned()
        } else {
            format!("AMD {preferred}")
        };
        names.insert(device.to_ascii_lowercase(), name);
    }
    names
}

fn amd_pci_names(root: &Path) -> HashMap<String, String> {
    for path in [
        root.join("usr/share/hwdata/pci.ids"),
        root.join("usr/share/misc/pci.ids"),
        root.join("usr/share/pci.ids"),
    ] {
        if let Ok(text) = std::fs::read_to_string(path) {
            return parse_amd_pci_names(&text);
        }
    }
    HashMap::new()
}

/// Kernel source adapter. Owns per-GPU throttle-residency baselines and a
/// reusable blob buffer.
#[derive(Debug)]
pub(crate) struct KernelSource {
    root: PathBuf,
    residency: HashMap<PhysicalGpuId, ThrottleReading>,
    buffer: Vec<u8>,
}

/// One raw card entry before grouping.
struct CardEntry {
    device: PathBuf,
    bdf: PciBdf,
    has_gpu_metrics: bool,
}

impl KernelSource {
    /// Creates the adapter over an explicit filesystem root (`/` in
    /// production; a fixture tree in tests).
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            residency: HashMap::new(),
            buffer: Vec::new(),
        }
    }

    /// Discovers AMD PCI/DRM devices bound to `amdgpu`, groups XCP partition
    /// functions into physical GPUs, and resolves collection paths. A failed
    /// or partial scan is an error and must never confirm removals.
    pub fn discover(&self) -> Result<Vec<KernelDevice>, String> {
        let drm = self.root.join("sys/class/drm");
        let entries = std::fs::read_dir(&drm)
            .map_err(|error| format!("cannot read {}: {error}", drm.display()))?;
        let mut cards: Vec<CardEntry> = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| format!("incomplete DRM scan: {error}"))?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if !name
                .strip_prefix("card")
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            {
                continue;
            }
            let device = entry.path().join("device");
            if let Some(card) = probe_card(&device)? {
                cards.push(card);
            }
        }
        // Group partition functions of one package by domain:bus:device.
        cards.sort_by(|a, b| a.bdf.as_str().cmp(b.bdf.as_str()));
        let heap_types = kfd_heap_types(&self.root);
        let pci_names = amd_pci_names(&self.root);
        let mut devices: Vec<KernelDevice> = Vec::new();
        let mut index = 0;
        while index < cards.len() {
            let key = &cards[index].bdf.as_str()[..10];
            let mut group_end = index + 1;
            while group_end < cards.len() && &cards[group_end].bdf.as_str()[..10] == key {
                group_end += 1;
            }
            let group = &cards[index..group_end];
            devices.push(build_device(group, &heap_types, &pci_names)?);
            index = group_end;
        }
        Ok(devices)
    }

    /// Fast collection: one coherent `gpu_metrics` read merged with stable
    /// text nodes, plus per-partition activity and memory.
    pub fn collect_fast(&mut self, device: &KernelDevice) -> KernelFastSample {
        let read_wall = Timestamp::now();
        let read_mono = Instant::now();
        if !device.primary.exists() {
            return KernelFastSample {
                gpu: device.disc.id.clone(),
                read_wall,
                read_mono,
                device_missing: true,
                hotspot_millic: Reading::Error,
                socket_power_microwatts: Reading::Error,
                energy: None,
                partitions: device
                    .partitions
                    .iter()
                    .map(|part| KernelPartitionFast {
                        partition: part.id.clone(),
                        is_primary: part.is_primary,
                        activity_centipercent: Reading::Error,
                        mem_used_bytes: Reading::Error,
                        mem_total_bytes: Reading::Error,
                        mem_ctl_centipercent: Reading::Error,
                    })
                    .collect(),
            };
        }
        let blob = self.read_blob(device);

        let (hotspot, power, energy, blob_activity, blob_umc) = match &blob {
            BlobRead::Parsed(parsed) => (
                parsed.hotspot_millic,
                parsed.socket_power_microwatts,
                parsed.energy,
                parsed.activity_centipercent,
                parsed.umc_centipercent,
            ),
            BlobRead::Unavailable(state) => (
                state.map(|_| 0),
                state.map(|_| 0),
                None,
                state.map(|_| 0),
                state.map(|_| 0),
            ),
        };

        // Stable text/hwmon nodes provide independent kernel observations
        // wherever the blob lacks a usable field.
        let hotspot = hotspot.or_else(|| self.hwmon_hotspot(device));
        let power = power.or_else(|| self.hwmon_power(device));

        let partitions = device
            .partitions
            .iter()
            .map(|part| {
                let activity_text = || {
                    read_u64(&part.device.join("gpu_busy_percent"))
                        .checked_map(|p| p.checked_mul(100))
                };
                let activity = if part.is_primary {
                    blob_activity.or_else(activity_text)
                } else {
                    activity_text()
                };
                let mem_ctl_text = || {
                    read_u64(&part.device.join("mem_busy_percent"))
                        .checked_map(|p| p.checked_mul(100))
                };
                let mem_ctl = if part.is_primary {
                    blob_umc.or_else(mem_ctl_text)
                } else {
                    mem_ctl_text()
                };
                KernelPartitionFast {
                    partition: part.id.clone(),
                    is_primary: part.is_primary,
                    activity_centipercent: validate_centipercent(activity),
                    mem_used_bytes: read_u64(&part.device.join(part.used_node)),
                    mem_total_bytes: read_u64(&part.device.join(part.total_node)),
                    mem_ctl_centipercent: validate_centipercent(mem_ctl),
                }
            })
            .collect();

        KernelFastSample {
            gpu: device.disc.id.clone(),
            read_wall,
            read_mono,
            device_missing: false,
            hotspot_millic: validate_millic(hotspot),
            socket_power_microwatts: validate_microwatts(power),
            energy,
            partitions,
        }
    }

    /// Slow collection: limits, caps, clocks, throttle, and RAS health.
    pub fn collect_slow(&mut self, device: &KernelDevice) -> KernelSlowSample {
        let read_wall = Timestamp::now();
        let read_mono = Instant::now();
        if !device.primary.exists() {
            return KernelSlowSample {
                gpu: device.disc.id.clone(),
                read_wall,
                read_mono,
                device_missing: true,
                limit_millic: Reading::Error,
                cap_microwatts: Reading::Error,
                partitions: device
                    .partitions
                    .iter()
                    .map(|part| KernelPartitionSlow {
                        partition: part.id.clone(),
                        is_primary: part.is_primary,
                        gfx_clock_hz: Reading::Error,
                    })
                    .collect(),
                health: Vec::new(),
            };
        }

        let limit = self.hwmon_hotspot_limit(device);
        let cap = match &device.hwmon {
            Some(hwmon) => read_u64(&hwmon.join("power1_cap")),
            None => Reading::Absent,
        };

        let blob = self.read_blob(device);
        let blob_clock = match &blob {
            BlobRead::Parsed(parsed) => parsed.gfx_clock_hz,
            BlobRead::Unavailable(state) => state.map(|_| 0),
        };

        let partitions = device
            .partitions
            .iter()
            .map(|part| {
                let hwmon_clock = || match &device.hwmon {
                    Some(hwmon) if part.is_primary => read_u64(&hwmon.join("freq1_input")),
                    _ => Reading::Absent,
                };
                let clock = if part.is_primary {
                    blob_clock.or_else(hwmon_clock)
                } else {
                    Reading::Absent
                };
                KernelPartitionSlow {
                    partition: part.id.clone(),
                    is_primary: part.is_primary,
                    gfx_clock_hz: clock,
                }
            })
            .collect();

        let mut health = Vec::new();
        if let BlobRead::Parsed(parsed) = &blob {
            self.throttle_health(device, &parsed.throttle, &mut health);
        }
        self.ras_health(device, &mut health);

        KernelSlowSample {
            gpu: device.disc.id.clone(),
            read_wall,
            device_missing: false,
            read_mono,
            limit_millic: validate_millic(limit),
            cap_microwatts: validate_microwatts(cap),
            partitions,
            health,
        }
    }

    fn read_blob(&mut self, device: &KernelDevice) -> BlobRead {
        let path = device.primary.join("gpu_metrics");
        self.buffer.clear();
        let outcome =
            std::fs::File::open(&path).and_then(|mut file| file.read_to_end(&mut self.buffer));
        match outcome {
            Ok(_) => match parse_gpu_metrics(&self.buffer) {
                Ok(parsed) => BlobRead::Parsed(parsed),
                Err(BlobError::UnsupportedVersion(_, _)) => {
                    BlobRead::Unavailable(Reading::UnsupportedDriver)
                }
                Err(BlobError::Truncated) => BlobRead::Unavailable(Reading::Malformed),
            },
            Err(error) => BlobRead::Unavailable(super::kernel_error_reading(&error)),
        }
    }

    fn hwmon_hotspot(&self, device: &KernelDevice) -> Reading<i64> {
        match (&device.hwmon, device.hotspot_channel) {
            (Some(hwmon), Some(channel)) => read_i64(&hwmon.join(format!("temp{channel}_input"))),
            _ => Reading::Absent,
        }
    }

    fn hwmon_hotspot_limit(&self, device: &KernelDevice) -> Reading<i64> {
        let (Some(hwmon), Some(channel)) = (&device.hwmon, device.hotspot_channel) else {
            return Reading::Absent;
        };
        let crit = read_i64(&hwmon.join(format!("temp{channel}_crit")));
        crit.or_else(|| read_i64(&hwmon.join(format!("temp{channel}_emergency"))))
    }

    fn hwmon_power(&self, device: &KernelDevice) -> Reading<u64> {
        let Some(hwmon) = &device.hwmon else {
            return Reading::Absent;
        };
        let input = read_u64(&hwmon.join("power1_input"));
        input.or_else(|| read_u64(&hwmon.join("power1_average")))
    }

    /// Emits an active-throttle signal from status bits or advancing
    /// residency accumulators. The first residency observation only anchors
    /// the per-GPU baseline.
    fn throttle_health(
        &mut self,
        device: &KernelDevice,
        throttle: &ThrottleReading,
        health: &mut Vec<KernelHealthSignal>,
    ) {
        match throttle {
            ThrottleReading::None => {}
            ThrottleReading::Status { status, indep } => {
                if *status != 0 {
                    let reasons = match indep {
                        Some(indep) if *indep != 0 => indep_reasons(*indep).join(", "),
                        _ => String::new(),
                    };
                    health.push(KernelHealthSignal::ThrottleActive { reasons });
                }
            }
            ThrottleReading::Residency {
                counter,
                prochot,
                ppt,
                socket_thm,
                vr_thm,
                hbm_thm,
            } => {
                let previous = self
                    .residency
                    .insert(device.disc.id.clone(), throttle.clone());
                if let Some(ThrottleReading::Residency {
                    counter: p_counter,
                    prochot: p_prochot,
                    ppt: p_ppt,
                    socket_thm: p_socket,
                    vr_thm: p_vr,
                    hbm_thm: p_hbm,
                }) = previous
                    && *counter > p_counter
                {
                    let mut reasons = Vec::new();
                    if *prochot > p_prochot {
                        reasons.push("prochot");
                    }
                    if *ppt > p_ppt {
                        reasons.push("power");
                    }
                    if *socket_thm > p_socket || *vr_thm > p_vr || *hbm_thm > p_hbm {
                        reasons.push("thermal");
                    }
                    if !reasons.is_empty() {
                        health.push(KernelHealthSignal::ThrottleActive {
                            reasons: reasons.join(", "),
                        });
                    }
                }
            }
        }
    }

    /// Collects RAS uncorrectable counts and bad-page state.
    fn ras_health(&self, device: &KernelDevice, health: &mut Vec<KernelHealthSignal>) {
        let ras = device.primary.join("ras");
        if let Ok(entries) = std::fs::read_dir(&ras) {
            let mut uncorrectable = 0u64;
            for entry in entries.flatten() {
                let name = entry.file_name();
                let Some(name) = name.to_str() else { continue };
                if !name.ends_with("_err_count") {
                    continue;
                }
                if let Reading::Value(text) = read_text(&entry.path()) {
                    for line in text.lines() {
                        if let Some(count) = line.trim().strip_prefix("ue:") {
                            uncorrectable += count.trim().parse::<u64>().unwrap_or(0);
                        }
                    }
                }
            }
            if uncorrectable > 0 {
                health.push(KernelHealthSignal::EccErrors { uncorrectable });
            }
        }
        if let Reading::Value(text) = read_text(&ras.join("gpu_vram_bad_pages")) {
            let mut pending = 0u64;
            let mut unreservable = 0u64;
            for line in text.lines() {
                let flag = line.rsplit(':').next().map(str::trim);
                match flag {
                    Some("P") => pending += 1,
                    Some("F") => unreservable += 1,
                    _ => {}
                }
            }
            if pending > 0 || unreservable > 0 {
                health.push(KernelHealthSignal::BadPages {
                    pending,
                    unreservable,
                });
            }
        }
    }
}

enum BlobRead {
    Parsed(GpuMetricsBlob),
    Unavailable(Reading<()>),
}

/// Probes one card device directory. `Ok(None)` is a complete scan of a card
/// not bound to amdgpu; I/O/identity errors make the enclosing scan incomplete.
fn probe_card(device: &Path) -> Result<Option<CardEntry>, String> {
    let uevent = std::fs::read_to_string(device.join("uevent"))
        .map_err(|error| format!("cannot read {}/uevent: {error}", device.display()))?;
    let mut driver = None;
    let mut slot = None;
    for line in uevent.lines() {
        if let Some(value) = line.strip_prefix("DRIVER=") {
            driver = Some(value.trim().to_owned());
        } else if let Some(value) = line.strip_prefix("PCI_SLOT_NAME=") {
            slot = Some(value.trim().to_owned());
        }
    }
    if driver.as_deref() != Some("amdgpu") {
        return Ok(None);
    }
    let vendor = std::fs::read_to_string(device.join("vendor"))
        .map_err(|error| format!("cannot read {}/vendor: {error}", device.display()))?;
    if vendor.trim() != "0x1002" {
        return Ok(None);
    }
    let slot =
        slot.ok_or_else(|| format!("missing PCI_SLOT_NAME in {}/uevent", device.display()))?;
    let bdf = PciBdf::parse(&slot)
        .map_err(|error| format!("invalid PCI identity in {}: {error}", device.display()))?;
    let has_gpu_metrics = device.join("gpu_metrics").exists();
    Ok(Some(CardEntry {
        device: device.to_owned(),
        bdf,
        has_gpu_metrics,
    }))
}

/// Replaces source-owned terminal controls before identity/display use.
fn safe_source_text(text: &str) -> String {
    text.trim()
        .chars()
        .map(|c| if c.is_control() { '\u{fffd}' } else { c })
        .collect()
}

/// Builds one physical GPU from its sorted partition-function group.
fn build_device(
    group: &[CardEntry],
    heap_types: &HashMap<PciBdf, u64>,
    pci_names: &HashMap<String, String>,
) -> Result<KernelDevice, String> {
    let owners: Vec<_> = group
        .iter()
        .enumerate()
        .filter(|(_, card)| card.has_gpu_metrics)
        .collect();
    if owners.len() > 1 {
        return Err(format!(
            "multiple primary gpu_metrics owners for package {}",
            &group[0].bdf.as_str()[..10]
        ));
    }
    let primary_index = owners.first().map(|(index, _)| *index).unwrap_or(0);
    let primary = &group[primary_index];
    let package_bdf = PciBdf::parse(&format!("{}.0", &group[0].bdf.as_str()[..10]))
        .map_err(|error| error.to_string())?;
    let mut unique_ids: Vec<String> = group
        .iter()
        .filter_map(|card| {
            std::fs::read_to_string(card.device.join("unique_id"))
                .ok()
                .map(|value| safe_source_text(&value))
                .filter(|value| !value.is_empty())
        })
        .collect();
    unique_ids.sort();
    unique_ids.dedup();
    let unique_id = (unique_ids.len() == 1).then(|| unique_ids.remove(0));
    let serial = std::fs::read_to_string(primary.device.join("serial_number"))
        .ok()
        .map(|s| safe_source_text(&s))
        .filter(|s| !s.is_empty());
    let gpu_id = match &unique_id {
        Some(unique) => PhysicalGpuId::new(format!("gpu-{unique}")),
        None => PhysicalGpuId::new(format!("gpu-{package_bdf}")),
    };

    let name = std::fs::read_to_string(primary.device.join("product_name"))
        .ok()
        .map(|s| safe_source_text(&s))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            let device_id = std::fs::read_to_string(primary.device.join("device"))
                .map(|s| safe_source_text(&s))
                .unwrap_or_default();
            let key = device_id
                .strip_prefix("0x")
                .unwrap_or(&device_id)
                .to_ascii_lowercase();
            pci_names.get(&key).cloned().unwrap_or_else(|| {
                if device_id.is_empty() {
                    "AMD GPU".to_owned()
                } else {
                    format!("AMD GPU {device_id}")
                }
            })
        });

    // KFD heap topology is explicit physical-memory evidence. Without it,
    // preserve an unknown-safe pool label rather than guessing APU/discrete
    // from capacity; the stable VRAM nodes remain the conservative source.
    let mut package_heaps: Vec<u64> = group
        .iter()
        .filter_map(|card| heap_types.get(&card.bdf).copied())
        .collect();
    package_heaps.sort_unstable();
    package_heaps.dedup();
    let pool = match package_heaps.as_slice() {
        [KFD_HEAP_FB_PUBLIC] => MemoryPool::GTT,
        [KFD_HEAP_FB_PRIVATE] => MemoryPool::VRAM,
        _ => MemoryPool::new("unknown"),
    };
    let (used_node, total_node) = if pool == MemoryPool::GTT {
        ("mem_info_gtt_used", "mem_info_gtt_total")
    } else {
        ("mem_info_vram_used", "mem_info_vram_total")
    };

    let hwmon = std::fs::read_dir(primary.device.join("hwmon"))
        .ok()
        .and_then(|entries| {
            let mut dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
            dirs.sort();
            dirs.into_iter().next()
        });
    let hotspot_channel = hwmon.as_ref().and_then(|hwmon| {
        (1..=3u32).find(|channel| {
            matches!(
                read_text(&hwmon.join(format!("temp{channel}_label"))),
                Reading::Value(label) if label == "junction"
            )
        })
    });

    let partitions: Vec<PartitionPaths> = group
        .iter()
        .map(|card| PartitionPaths {
            id: PartitionId::new(format!("{gpu_id}-xcp-{}", card.bdf)),
            is_primary: std::ptr::eq(card, primary),
            bdf: card.bdf.clone(),
            device: card.device.clone(),
            used_node,
            total_node,
        })
        .collect();

    Ok(KernelDevice {
        disc: DiscoveredGpu {
            id: gpu_id,
            bdf: package_bdf,
            name,
            uuid: unique_id.map(|u| format!("amdgpu-{u}")),
            serial,
            pool,
            partitions: partitions
                .iter()
                .map(|p| DiscoveredPartition {
                    id: p.id.clone(),
                    is_primary: p.is_primary,
                    bdf: p.bdf.clone(),
                })
                .collect(),
        },
        primary: primary.device.clone(),
        hwmon,
        hotspot_channel,
        partitions,
    })
}

/// Domain validation without clamping: out-of-domain values are malformed.
fn validate_centipercent(reading: Reading<u64>) -> Reading<u64> {
    match reading {
        Reading::Value(v) if v > 10_000 => Reading::Malformed,
        other => other,
    }
}

fn validate_millic(reading: Reading<i64>) -> Reading<i64> {
    match reading {
        Reading::Value(v) if !(-273_150..=1_000_000).contains(&v) => Reading::Malformed,
        other => other,
    }
}

fn validate_microwatts(reading: Reading<u64>) -> Reading<u64> {
    match reading {
        Reading::Value(v) if v > 20_000_000_000 => Reading::Malformed,
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(scenario: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/kernel")
            .join(scenario)
    }

    fn source(scenario: &str) -> (KernelSource, Vec<KernelDevice>) {
        let source = KernelSource::new(fixture(scenario));
        let devices = source.discover().unwrap();
        (source, devices)
    }

    #[test]
    fn discovery_requires_amd_vendor_and_amdgpu_binding() {
        let (_, devices) = source("discrete-spx");
        assert_eq!(
            devices.len(),
            1,
            "nvidia and radeon distractors must be ignored"
        );
        let disc = &devices[0].disc;
        assert_eq!(disc.id.as_str(), "gpu-73fbc17bb4b8d1ce");
        assert_eq!(disc.bdf.as_str(), "0000:41:00.0");
        assert_eq!(disc.name, "AMD Instinct MI210");
        assert_eq!(disc.serial.as_deref(), Some("GRUFLO-FIXTURE-0001"));
        assert_eq!(disc.pool, MemoryPool::VRAM);
        assert_eq!(disc.partitions.len(), 1);
        assert!(disc.partitions[0].is_primary);
    }

    #[test]
    fn discovery_on_empty_root_finds_nothing() {
        let source = KernelSource::new(PathBuf::from("/nonexistent-gruflo-root"));
        assert!(source.discover().is_err());
    }

    #[test]
    fn fast_collection_prefers_coherent_blob_with_text_fallback() {
        let (mut source, devices) = source("discrete-spx");
        let sample = source.collect_fast(&devices[0]);
        // v1.3 blob values, not the (different) text/hwmon values.
        assert_eq!(sample.hotspot_millic, Reading::Value(87_000));
        assert_eq!(sample.socket_power_microwatts, Reading::Value(250_000_000));
        assert_eq!(sample.energy, Some((1_234_567_890, 1.0 / 65536.0)));
        let part = &sample.partitions[0];
        assert_eq!(part.activity_centipercent, Reading::Value(9_700));
        assert_eq!(part.mem_ctl_centipercent, Reading::Value(4_200));
        assert_eq!(part.mem_used_bytes, Reading::Value(58_982_400_000));
        assert_eq!(part.mem_total_bytes, Reading::Value(68_719_476_736));
    }

    #[test]
    fn slow_collection_reads_limits_caps_and_clock() {
        let (mut source, devices) = source("discrete-spx");
        let sample = source.collect_slow(&devices[0]);
        assert_eq!(sample.limit_millic, Reading::Value(95_000));
        assert_eq!(sample.cap_microwatts, Reading::Value(300_000_000));
        assert_eq!(
            sample.partitions[0].gfx_clock_hz,
            Reading::Value(1_700_000_000)
        );
        assert!(
            sample.health.is_empty(),
            "no throttle, no uncorrectable errors"
        );
    }

    #[test]
    fn apu_uses_gtt_pool_and_centi_unit_blob() {
        let (mut source, devices) = source("apu-gtt");
        assert_eq!(devices[0].disc.pool, MemoryPool::GTT);
        assert_eq!(devices[0].disc.id.as_str(), "gpu-0000:c4:00.0");
        assert_eq!(devices[0].disc.name, "AMD GPU 0x15bf");
        let sample = source.collect_fast(&devices[0]);
        // v2.4 centi-Celsius 6750 -> 67500 milli-C; mW 15000 -> 15_000_000 uW.
        assert_eq!(sample.hotspot_millic, Reading::Value(67_500));
        assert_eq!(sample.socket_power_microwatts, Reading::Value(15_000_000));
        let part = &sample.partitions[0];
        assert_eq!(part.activity_centipercent, Reading::Value(4_300));
        assert_eq!(part.mem_used_bytes, Reading::Value(4_294_967_296));
        assert_eq!(part.mem_total_bytes, Reading::Value(17_179_869_184));
        // mem_busy_percent absent on this APU: structural absence.
        assert_eq!(part.mem_ctl_centipercent, Reading::Absent);
    }

    #[test]
    fn discrete_gpu_with_larger_gtt_pool_stays_vram() {
        // Live-host regression: 32 GiB VRAM beside a bigger system-RAM GTT
        // pool with KFD FB_PRIVATE evidence must not be labelled GTT, and
        // sentinel throttle fields must not fabricate a throttle.
        let (mut source, devices) = source("discrete-large-gtt");
        assert_eq!(devices[0].disc.pool, MemoryPool::VRAM);
        let sample = source.collect_fast(&devices[0]);
        assert_eq!(
            sample.partitions[0].mem_total_bytes,
            Reading::Value(34_208_743_424)
        );
        let slow = source.collect_slow(&devices[0]);
        assert!(
            slow.health.is_empty(),
            "sentinel throttle bits are not a throttle"
        );
    }

    #[test]
    fn v3_layout_reads_documented_fields_only() {
        let (mut source, devices) = source("apu-v3");
        let sample = source.collect_fast(&devices[0]);
        assert_eq!(
            sample.partitions[0].activity_centipercent,
            Reading::Value(3_800)
        );
        assert_eq!(sample.socket_power_microwatts, Reading::Value(21_000_000));
        // v3.0 temperature scale is undocumented and no hwmon junction exists.
        assert_eq!(sample.hotspot_millic, Reading::Absent);
        let slow = source.collect_slow(&devices[0]);
        assert_eq!(
            slow.partitions[0].gfx_clock_hz,
            Reading::Value(2_900_000_000)
        );
    }

    #[test]
    fn multi_xcp_groups_partitions_under_one_physical_gpu() {
        let (mut source, devices) = source("multi-xcp");
        assert_eq!(devices.len(), 1);
        let disc = &devices[0].disc;
        assert_eq!(disc.partitions.len(), 2);
        assert!(disc.partitions[0].is_primary);
        assert!(!disc.partitions[1].is_primary);
        let sample = source.collect_fast(&devices[0]);
        assert_eq!(sample.hotspot_millic, Reading::Value(74_000));
        assert_eq!(sample.socket_power_microwatts, Reading::Value(318_000_000));
        let primary = &sample.partitions[0];
        assert_eq!(primary.activity_centipercent, Reading::Value(9_700));
        let secondary = &sample.partitions[1];
        // The secondary partition owns its memory but no engine sensors.
        assert_eq!(secondary.activity_centipercent, Reading::Absent);
        assert_eq!(secondary.mem_used_bytes, Reading::Value(51_539_607_552));
    }

    #[test]
    fn unsupported_versions_are_driver_evidence_and_text_survives() {
        for scenario in ["boundary-v19", "boundary-unknown"] {
            let (mut source, devices) = source(scenario);
            let sample = source.collect_fast(&devices[0]);
            assert_eq!(
                sample.hotspot_millic,
                Reading::UnsupportedDriver,
                "{scenario} hotspot must carry version evidence"
            );
            // The stable text node keeps providing an independent value.
            assert!(matches!(
                sample.partitions[0].activity_centipercent,
                Reading::Value(_)
            ));
        }
    }

    #[test]
    fn truncated_recognized_layout_is_malformed() {
        let (mut source, devices) = source("boundary-truncated");
        let sample = source.collect_fast(&devices[0]);
        assert_eq!(sample.hotspot_millic, Reading::Malformed);
    }

    #[test]
    fn malformed_scalars_never_become_numbers() {
        let (mut source, devices) = source("boundary-malformed");
        let sample = source.collect_fast(&devices[0]);
        assert_eq!(
            sample.partitions[0].activity_centipercent,
            Reading::Malformed
        );
        assert_eq!(sample.partitions[0].mem_used_bytes, Reading::Malformed);
        assert_eq!(
            sample.partitions[0].mem_total_bytes,
            Reading::Value(25_753_026_560)
        );
    }

    #[test]
    fn throttle_ras_and_bad_pages_produce_health_signals() {
        let (mut source, devices) = source("boundary-throttle");
        let sample = source.collect_slow(&devices[0]);
        assert!(sample.health.contains(&KernelHealthSignal::ThrottleActive {
            reasons: "power, thermal".to_owned()
        }));
        assert!(
            sample
                .health
                .contains(&KernelHealthSignal::EccErrors { uncorrectable: 2 })
        );
        assert!(sample.health.contains(&KernelHealthSignal::BadPages {
            pending: 1,
            unreservable: 1
        }));
    }

    #[test]
    fn residency_throttle_needs_an_advancing_accumulator() {
        let (mut source, devices) = source("multi-xcp");
        // First read anchors the baseline; identical second read: no signal.
        let first = source.collect_slow(&devices[0]);
        assert!(first.health.is_empty());
        let second = source.collect_slow(&devices[0]);
        assert!(second.health.is_empty());
        // A synthetic advanced residency produces a thermal signal.
        let mut health = Vec::new();
        source.throttle_health(
            &devices[0],
            &ThrottleReading::Residency {
                counter: 110,
                prochot: 0,
                ppt: 0,
                socket_thm: 9,
                vr_thm: 0,
                hbm_thm: 0,
            },
            &mut health,
        );
        assert_eq!(
            health,
            vec![KernelHealthSignal::ThrottleActive {
                reasons: "thermal".to_owned()
            }]
        );
    }

    #[test]
    fn io_error_mapping_distinguishes_asleep_and_permission() {
        let eperm = std::io::Error::from_raw_os_error(1);
        assert_eq!(
            crate::source::kernel_error_reading::<u64>(&eperm),
            Reading::Asleep
        );
        let eacces = std::io::Error::from_raw_os_error(13);
        assert_eq!(
            crate::source::kernel_error_reading::<u64>(&eacces),
            Reading::PermissionDenied
        );
        let missing = std::io::Error::new(std::io::ErrorKind::NotFound, "gone");
        assert_eq!(
            crate::source::kernel_error_reading::<u64>(&missing),
            Reading::Absent
        );
    }

    #[test]
    fn unreadable_existing_node_is_permission_denied() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("gruflo-kernel-perm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let node = dir.join("gpu_busy_percent");
        std::fs::write(&node, "50\n").unwrap();
        std::fs::set_permissions(&node, std::fs::Permissions::from_mode(0o000)).unwrap();
        assert_eq!(read_u64(&node), Reading::PermissionDenied);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sentinels_are_recognized_before_numeric_validation() {
        // All-ones v1.3 payload: every metric field is the memset sentinel.
        let mut bytes = vec![0xFFu8; 120];
        bytes[0..2].copy_from_slice(&120u16.to_le_bytes());
        bytes[2] = 1;
        bytes[3] = 3;
        let blob = parse_gpu_metrics(&bytes).unwrap();
        assert_eq!(blob.hotspot_millic, Reading::Sentinel);
        assert_eq!(blob.socket_power_microwatts, Reading::Sentinel);
        assert_eq!(blob.activity_centipercent, Reading::Sentinel);
        assert_eq!(blob.energy, None);
    }

    #[test]
    fn domain_validation_rejects_without_clamping() {
        assert_eq!(
            validate_centipercent(Reading::Value(10_001)),
            Reading::Malformed
        );
        assert_eq!(
            validate_centipercent(Reading::Value(10_000)),
            Reading::Value(10_000)
        );
        assert_eq!(
            validate_millic(Reading::Value(-300_000)),
            Reading::Malformed
        );
        assert_eq!(
            validate_microwatts(Reading::Value(30_000_000_000)),
            Reading::Malformed
        );
    }

    #[test]
    fn local_pci_database_prefers_bracketed_marketing_name() {
        let names = parse_amd_pci_names(
            "1002  Advanced Micro Devices, Inc. [AMD/ATI]\n\
             # comment inside vendor section\n\
             \n\
             \t7551  Navi 48 [Radeon AI PRO R9700]\n\
             \t\t1da2 e490  subsystem\n\
             10de  NVIDIA Corporation\n\
             \t7551  unrelated\n",
        );
        assert_eq!(
            names.get("7551").map(String::as_str),
            Some("AMD Radeon AI PRO R9700")
        );
        assert_eq!(names.len(), 1);
    }
}
