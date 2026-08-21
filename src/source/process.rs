//! On-demand process attribution from DRM fdinfo and KFD.
//!
//! Two independent accounting systems are reported without reconciliation:
//! DRM fdinfo (associated by `drm-pdev`, memory from the non-deprecated
//! `drm-resident-*` keys, KiB × 1024) and KFD (`/sys/class/kfd/kfd/proc`,
//! membership plus `vram_<gpuid>` bytes). No utilization or engine-time
//! claim is produced. Command lines are never read.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::Reading;
use crate::model::{PciBdf, Timestamp};

/// One attributed process row for a single (PID, GPU) association.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProcessRow {
    pub pid: u32,
    /// Permitted process name (`comm`), or the exact reason it is missing.
    pub name: Reading<String>,
    /// PCI address of the associated GPU, when attribution resolved one.
    pub bdf: Option<PciBdf>,
    /// DRM fdinfo resident VRAM, bytes (KiB × 1024).
    pub fdinfo_vram_bytes: Reading<u64>,
    /// DRM fdinfo resident GTT, bytes (KiB × 1024).
    pub fdinfo_gtt_bytes: Reading<u64>,
    /// KFD `vram_<gpuid>` accounting, bytes. Reported separately from
    /// fdinfo; the two systems differ and must not be summed or reconciled.
    pub kfd_vram_bytes: Reading<u64>,
    /// Container identity derived from the cgroup path, when present.
    pub container: Option<String>,
}

/// One full process scan.
#[derive(Debug, Clone)]
pub(crate) struct ProcessSample {
    pub read_wall: Timestamp,
    /// DRM fdinfo scan availability (may be permission-limited while rows survive).
    pub fdinfo_status: Reading<()>,
    /// KFD scan availability, independent of fdinfo.
    pub kfd_status: Reading<()>,
    pub rows: Vec<ProcessRow>,
}

/// Process source adapter over an explicit filesystem root.
#[derive(Debug)]
pub(crate) struct ProcessSource {
    root: PathBuf,
}

/// Accumulated per-(pid, bdf-or-gpuid) evidence during one scan.
#[derive(Default)]
struct Accumulator {
    fdinfo_vram_kib: Option<u64>,
    fdinfo_gtt_kib: Option<u64>,
    vram_malformed: bool,
    gtt_malformed: bool,
    kfd_vram_bytes: Option<Reading<u64>>,
}

/// Process/KFD access failures are feature-local permission states. Unlike
/// amdgpu sensor reads, EPERM here never means the physical GPU is asleep.
fn process_error_reading<T>(error: &std::io::Error) -> Reading<T> {
    match error.raw_os_error() {
        Some(1 | 13) => Reading::PermissionDenied,
        _ if error.kind() == std::io::ErrorKind::NotFound => Reading::Absent,
        _ => Reading::Error,
    }
}

fn merge_status(left: Reading<()>, right: Reading<()>) -> Reading<()> {
    use Reading::{Absent, Error, PermissionDenied, Value};
    match (left, right) {
        (PermissionDenied, _) | (_, PermissionDenied) => PermissionDenied,
        (Error, _) | (_, Error) => Error,
        (Absent, _) | (_, Absent) => Absent,
        (Value(()), Value(())) => Value(()),
        _ => Reading::Error,
    }
}

impl ProcessSource {
    /// Creates the adapter over an explicit root (`/` in production).
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Maps KFD `gpu_id` values to PCI BDFs and retains partial-scan state.
    fn kfd_topology(&self) -> (HashMap<u64, PciBdf>, Reading<()>) {
        let mut map = HashMap::new();
        let nodes = self.root.join("sys/class/kfd/kfd/topology/nodes");
        let entries = match std::fs::read_dir(&nodes) {
            Ok(entries) => entries,
            Err(error) => return (map, process_error_reading(&error)),
        };
        let mut status = Reading::Value(());
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    status = merge_status(status, Reading::Error);
                    continue;
                }
            };
            let dir = entry.path();
            let gpu_id = match std::fs::read_to_string(dir.join("gpu_id")) {
                Ok(value) => match value.trim().parse::<u64>() {
                    Ok(value) => value,
                    Err(_) => {
                        status = merge_status(status, Reading::Error);
                        continue;
                    }
                },
                Err(error) => {
                    status = merge_status(status, process_error_reading(&error));
                    continue;
                }
            };
            if gpu_id == 0 {
                continue;
            }
            let properties = match std::fs::read_to_string(dir.join("properties")) {
                Ok(value) => value,
                Err(error) => {
                    status = merge_status(status, process_error_reading(&error));
                    continue;
                }
            };
            let mut domain = 0u64;
            let mut location = None;
            for line in properties.lines() {
                let mut parts = line.split_whitespace();
                match (parts.next(), parts.next()) {
                    (Some("domain"), Some(value)) => domain = value.parse().unwrap_or(0),
                    (Some("location_id"), Some(value)) => location = value.parse::<u64>().ok(),
                    _ => {}
                }
            }
            let Some(location) = location else {
                status = merge_status(status, Reading::Error);
                continue;
            };
            let bus = (location >> 8) & 0xFF;
            let device = (location >> 3) & 0x1F;
            let function = location & 0x7;
            match PciBdf::parse(&format!("{domain:04x}:{bus:02x}:{device:02x}.{function:x}")) {
                Ok(bdf) => {
                    map.insert(gpu_id, bdf);
                }
                Err(_) => status = merge_status(status, Reading::Error),
            }
        }
        (map, status)
    }

    /// Performs one bounded full scan. Enumerates candidate PIDs from both
    /// `/proc` fdinfo and the world-readable KFD proc tree.
    pub fn scan(&self) -> ProcessSample {
        let read_wall = Timestamp::now();
        let (topology, topology_status) = self.kfd_topology();
        let mut evidence: HashMap<(u32, Option<PciBdf>), Accumulator> = HashMap::new();

        let mut fdinfo_statuses = Vec::new();
        match std::fs::read_dir(self.root.join("proc")) {
            Ok(entries) => {
                for entry in entries {
                    let entry = match entry {
                        Ok(entry) => entry,
                        Err(_) => {
                            fdinfo_statuses.push(Reading::Error);
                            continue;
                        }
                    };
                    let name = entry.file_name();
                    let Some(pid) = name.to_str().and_then(|n| n.parse::<u32>().ok()) else {
                        continue;
                    };
                    fdinfo_statuses.push(self.scan_fdinfo(pid, &entry.path(), &mut evidence));
                }
            }
            Err(error) => fdinfo_statuses.push(process_error_reading(&error)),
        }

        let kfd_proc = self.root.join("sys/class/kfd/kfd/proc");
        let mut kfd_status = topology_status;
        match std::fs::read_dir(&kfd_proc) {
            Ok(entries) => {
                for entry in entries {
                    let entry = match entry {
                        Ok(entry) => entry,
                        Err(_) => {
                            kfd_status = merge_status(kfd_status, Reading::Error);
                            continue;
                        }
                    };
                    let name = entry.file_name();
                    let Some(pid) = name.to_str().and_then(|n| n.parse::<u32>().ok()) else {
                        continue;
                    };
                    kfd_status = merge_status(
                        kfd_status,
                        self.scan_kfd(pid, &entry.path(), &topology, &mut evidence),
                    );
                }
            }
            Err(error) => {
                kfd_status = merge_status(kfd_status, process_error_reading(&error));
            }
        }
        let mut rows: Vec<ProcessRow> = evidence
            .into_iter()
            .map(|((pid, bdf), acc)| {
                let proc_dir = self.root.join("proc").join(pid.to_string());
                let field = |kib: Option<u64>, malformed: bool| {
                    if malformed {
                        Reading::Malformed
                    } else {
                        fdinfo_reading(kib, &proc_dir)
                    }
                };
                ProcessRow {
                    pid,
                    name: read_comm(&proc_dir),
                    bdf,
                    fdinfo_vram_bytes: field(acc.fdinfo_vram_kib, acc.vram_malformed),
                    fdinfo_gtt_bytes: field(acc.fdinfo_gtt_kib, acc.gtt_malformed),
                    kfd_vram_bytes: acc.kfd_vram_bytes.unwrap_or(Reading::Absent),
                    container: read_container(&proc_dir),
                }
            })
            .collect();
        rows.sort_by(|a, b| {
            let key = |row: &ProcessRow| {
                let value = |reading| match reading {
                    Reading::Value(v) => v,
                    _ => 0,
                };
                value(row.fdinfo_vram_bytes)
                    .max(value(row.fdinfo_gtt_bytes))
                    .max(value(row.kfd_vram_bytes))
            };
            key(b).cmp(&key(a)).then_with(|| a.pid.cmp(&b.pid))
        });

        let fdinfo_status = if fdinfo_statuses
            .iter()
            .any(|status| matches!(status, Reading::PermissionDenied))
        {
            Reading::PermissionDenied
        } else if fdinfo_statuses
            .iter()
            .any(|status| matches!(status, Reading::Error))
        {
            Reading::Error
        } else if fdinfo_statuses
            .iter()
            .any(|status| matches!(status, Reading::Absent))
        {
            Reading::Absent
        } else if fdinfo_statuses
            .iter()
            .any(|status| matches!(status, Reading::Value(())))
        {
            Reading::Value(())
        } else {
            Reading::Absent
        };
        ProcessSample {
            read_wall,
            fdinfo_status,
            kfd_status,
            rows,
        }
    }

    /// Reads every fdinfo entry of one PID, deduplicating DRM clients.
    fn scan_fdinfo(
        &self,
        pid: u32,
        proc_dir: &Path,
        evidence: &mut HashMap<(u32, Option<PciBdf>), Accumulator>,
    ) -> Reading<()> {
        let entries = match std::fs::read_dir(proc_dir.join("fdinfo")) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Reading::Value(());
            }
            Err(error) => return process_error_reading(&error),
        };
        // One DRM client may be visible through several duplicated fds;
        // count each client id once.
        let mut seen_clients: Vec<(Option<PciBdf>, String)> = Vec::new();
        let mut status = Reading::Value(());
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    status = merge_status(status, Reading::Error);
                    continue;
                }
            };
            let content = match std::fs::read_to_string(entry.path()) {
                Ok(content) => content,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    status = merge_status(status, process_error_reading(&error));
                    continue;
                }
            };
            if !content.contains("drm-driver:") || !content.contains("amdgpu") {
                continue;
            }
            let mut pdev = None;
            let mut client_id = String::new();
            let mut vram_kib: Option<Result<u64, ()>> = None;
            let mut gtt_kib: Option<Result<u64, ()>> = None;
            for line in content.lines() {
                let Some((key, value)) = line.split_once(':') else {
                    continue;
                };
                let value = value.trim();
                match key.trim() {
                    "drm-pdev" => pdev = PciBdf::parse(value).ok(),
                    "drm-client-id" => client_id = value.to_owned(),
                    "drm-resident-vram" => vram_kib = Some(parse_kib(value)),
                    "drm-resident-gtt" => gtt_kib = Some(parse_kib(value)),
                    _ => {}
                }
            }
            let dedup_id = if client_id.is_empty() {
                format!("fd:{}", entry.file_name().to_string_lossy())
            } else {
                client_id
            };
            let client = (pdev.clone(), dedup_id);
            if seen_clients.contains(&client) {
                continue;
            }
            seen_clients.push(client);
            let acc = evidence.entry((pid, pdev)).or_default();
            match vram_kib {
                Some(Ok(kib)) => match acc.fdinfo_vram_kib.unwrap_or(0).checked_add(kib) {
                    Some(total) => acc.fdinfo_vram_kib = Some(total),
                    None => {
                        acc.fdinfo_vram_kib = None;
                        acc.vram_malformed = true;
                    }
                },
                Some(Err(())) => acc.vram_malformed = true,
                None => {}
            }
            match gtt_kib {
                Some(Ok(kib)) => match acc.fdinfo_gtt_kib.unwrap_or(0).checked_add(kib) {
                    Some(total) => acc.fdinfo_gtt_kib = Some(total),
                    None => {
                        acc.fdinfo_gtt_kib = None;
                        acc.gtt_malformed = true;
                    }
                },
                Some(Err(())) => acc.gtt_malformed = true,
                None => {}
            }
        }
        status
    }

    /// Reads one KFD proc entry: queue membership and `vram_<gpuid>`.
    fn scan_kfd(
        &self,
        pid: u32,
        kfd_dir: &Path,
        topology: &HashMap<u64, PciBdf>,
        evidence: &mut HashMap<(u32, Option<PciBdf>), Accumulator>,
    ) -> Reading<()> {
        let entries = match std::fs::read_dir(kfd_dir) {
            Ok(entries) => entries,
            Err(error) => return process_error_reading(&error),
        };
        let mut status = Reading::Value(());
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    status = merge_status(status, Reading::Error);
                    continue;
                }
            };
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let Some(gpu_id) = name.strip_prefix("vram_") else {
                continue;
            };
            let Ok(gpu_id) = gpu_id.parse::<u64>() else {
                status = merge_status(status, Reading::Error);
                continue;
            };
            let bdf = topology.get(&gpu_id).cloned();
            let reading = match std::fs::read_to_string(entry.path()) {
                Ok(text) => match text.trim().parse::<u64>() {
                    Ok(bytes) => Reading::Value(bytes),
                    Err(_) => Reading::Malformed,
                },
                Err(error) => process_error_reading(&error),
            };
            status = match reading {
                Reading::PermissionDenied => merge_status(status, Reading::PermissionDenied),
                Reading::Malformed | Reading::Error => merge_status(status, Reading::Error),
                Reading::Absent => merge_status(status, Reading::Absent),
                _ => status,
            };
            evidence.entry((pid, bdf)).or_default().kfd_vram_bytes = Some(reading);
        }
        status
    }
}

/// fdinfo memory: accumulated KiB × 1024, or the reason it is missing.
fn fdinfo_reading(kib: Option<u64>, proc_dir: &Path) -> Reading<u64> {
    match kib {
        Some(kib) => kib
            .checked_mul(1024)
            .map_or(Reading::Malformed, Reading::Value),
        // Distinguish an unreadable fdinfo directory (permission-limited
        // cross-user attribution) from a KFD-only process.
        None => match std::fs::read_dir(proc_dir.join("fdinfo")) {
            Ok(_) => Reading::Absent,
            Err(error) => process_error_reading(&error),
        },
    }
}

/// fdinfo memory normalized to KiB. Current kernels may select KiB/MiB/GiB
/// per value; unsuffixed zero remains valid.
fn parse_kib(value: &str) -> Result<u64, ()> {
    let value = value.trim();
    let (number, factor) = if let Some(number) = value.strip_suffix("KiB") {
        (number.trim(), 1)
    } else if let Some(number) = value.strip_suffix("MiB") {
        (number.trim(), 1024)
    } else if let Some(number) = value.strip_suffix("GiB") {
        (number.trim(), 1024 * 1024)
    } else {
        (value, 1)
    };
    number
        .parse::<u64>()
        .ok()
        .and_then(|number| number.checked_mul(factor))
        .ok_or(())
}

/// Permitted process name from `comm`; never the command line. Control
/// characters are replaced here so terminal safety is owned in-repo rather
/// than relying on a renderer's filtering implementation.
fn read_comm(proc_dir: &Path) -> Reading<String> {
    match std::fs::read_to_string(proc_dir.join("comm")) {
        Ok(text) => Reading::Value(
            text.trim()
                .chars()
                .map(|c| if c.is_control() { '\u{fffd}' } else { c })
                .collect(),
        ),
        Err(error) => process_error_reading(&error),
    }
}

/// Container identity from the cgroup path, for known container patterns.
fn read_container(proc_dir: &Path) -> Option<String> {
    let content = std::fs::read_to_string(proc_dir.join("cgroup")).ok()?;
    for line in content.lines() {
        let path = line.rsplit(':').next()?;
        for segment in path.split('/') {
            let scoped = segment.strip_suffix(".scope").unwrap_or(segment);
            for (prefix, label) in [
                ("docker-", "docker"),
                ("crio-", "crio"),
                ("cri-containerd-", "containerd"),
                ("libpod-", "podman"),
            ] {
                if let Some(id) = scoped.strip_prefix(prefix)
                    && id.len() >= 12
                    && id.bytes().all(|b| b.is_ascii_hexdigit())
                {
                    return Some(format!("{label}:{}", &id[..12]));
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(scenario: &str) -> ProcessSource {
        ProcessSource::new(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/process")
                .join(scenario),
        )
    }

    fn row(sample: &ProcessSample, pid: u32) -> &ProcessRow {
        sample.rows.iter().find(|r| r.pid == pid).unwrap()
    }

    #[test]
    fn hip_workload_reports_both_accountings_without_reconciling() {
        let sample = fixture("hip-workload").scan();
        assert_eq!(sample.rows.len(), 1);
        let row = &sample.rows[0];
        assert_eq!(row.pid, 4242);
        assert_eq!(row.name, Reading::Value("gruflo-hip".to_owned()));
        assert_eq!(row.bdf.as_ref().map(PciBdf::as_str), Some("0000:03:00.0"));
        // fdinfo: KiB × 1024 exactly, never × 1000.
        assert_eq!(row.fdinfo_vram_bytes, Reading::Value(340_104 * 1024));
        assert_eq!(row.fdinfo_gtt_bytes, Reading::Value(8_240 * 1024));
        // KFD accounting stays a separate figure (live evidence differs ~15%).
        assert_eq!(row.kfd_vram_bytes, Reading::Value(409_726_976));
        assert_eq!(row.container, None);
    }

    #[test]
    fn duplicate_fds_for_one_client_count_once() {
        let sample = fixture("boundaries").scan();
        let row = row(&sample, 100);
        assert_eq!(row.fdinfo_vram_bytes, Reading::Value(100_000 * 1024));
    }

    #[test]
    fn two_gpus_produce_two_rows_for_one_pid() {
        let sample = fixture("boundaries").scan();
        let rows: Vec<_> = sample.rows.iter().filter(|r| r.pid == 101).collect();
        assert_eq!(rows.len(), 2);
        let mut bdfs: Vec<_> = rows
            .iter()
            .map(|r| r.bdf.as_ref().unwrap().as_str())
            .collect();
        bdfs.sort();
        assert_eq!(bdfs, vec!["0000:03:00.0", "0000:41:00.0"]);
    }

    #[test]
    fn kfd_only_membership_is_reported_with_association() {
        let sample = fixture("boundaries").scan();
        let row = row(&sample, 102);
        assert_eq!(row.bdf.as_ref().map(PciBdf::as_str), Some("0000:03:00.0"));
        assert_eq!(row.kfd_vram_bytes, Reading::Value(1_048_576));
        // No fdinfo directory for this pid: the state is explicit.
        assert_eq!(row.fdinfo_vram_bytes, Reading::Absent);
    }

    #[test]
    fn malformed_units_never_become_numbers() {
        let sample = fixture("boundaries").scan();
        let row = row(&sample, 103);
        assert_eq!(row.fdinfo_vram_bytes, Reading::Malformed);
    }

    #[test]
    fn container_identity_is_derived_from_known_cgroup_patterns() {
        let sample = fixture("boundaries").scan();
        let row = row(&sample, 104);
        assert_eq!(row.container.as_deref(), Some("docker:abcdef123456"));
    }

    #[test]
    fn missing_name_keeps_the_row_with_its_state() {
        let sample = fixture("boundaries").scan();
        let row = row(&sample, 105);
        assert_eq!(row.name, Reading::Absent);
        assert_eq!(row.kfd_vram_bytes, Reading::Value(2_097_152));
    }

    #[test]
    fn rows_sort_by_attributed_memory_descending() {
        let sample = fixture("boundaries").scan();
        let keys: Vec<u64> = sample
            .rows
            .iter()
            .map(|r| {
                let f = match r.fdinfo_vram_bytes {
                    Reading::Value(v) => v,
                    _ => 0,
                };
                let k = match r.kfd_vram_bytes {
                    Reading::Value(v) => v,
                    _ => 0,
                };
                f.max(k)
            })
            .collect();
        let mut sorted = keys.clone();
        sorted.sort_by(|a, b| b.cmp(a));
        assert_eq!(keys, sorted);
    }

    #[test]
    fn no_row_ever_claims_utilization() {
        // Structural guarantee: ProcessRow has no utilization or engine-time
        // field. This test documents the contract at the type level.
        let row = ProcessRow {
            pid: 1,
            name: Reading::Value("x".into()),
            bdf: None,
            fdinfo_vram_bytes: Reading::Absent,
            fdinfo_gtt_bytes: Reading::Absent,
            kfd_vram_bytes: Reading::Absent,
            container: None,
        };
        let debug = format!("{row:?}");
        assert!(!debug.contains("percent"));
        assert!(!debug.contains("engine"));
    }

    #[test]
    fn extreme_fdinfo_quantity_is_malformed_instead_of_wrapping() {
        assert_eq!(
            fdinfo_reading(Some(u64::MAX), Path::new("/nonexistent")),
            Reading::Malformed
        );
    }

    #[test]
    fn fdinfo_memory_units_normalize_to_kib() {
        assert_eq!(parse_kib("26640 KiB"), Ok(26_640));
        assert_eq!(parse_kib("2 MiB"), Ok(2_048));
        assert_eq!(parse_kib("1 GiB"), Ok(1024 * 1024));
        assert_eq!(parse_kib("0"), Ok(0));
        assert!(parse_kib("2 MB").is_err());
    }
    #[test]
    fn process_names_are_sanitized_before_rendering() {
        let dir = std::env::temp_dir().join(format!("gruflo-process-name-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("comm"), "safe\u{1b}]0;owned\u{7}\n").unwrap();
        assert_eq!(
            read_comm(&dir),
            Reading::Value("safe\u{fffd}]0;owned\u{fffd}".to_owned())
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn process_eperm_is_permission_not_gpu_sleep() {
        assert_eq!(
            process_error_reading::<u64>(&std::io::Error::from_raw_os_error(1)),
            Reading::PermissionDenied
        );
        assert_eq!(
            process_error_reading::<u64>(&std::io::Error::from_raw_os_error(13)),
            Reading::PermissionDenied
        );
    }
}
