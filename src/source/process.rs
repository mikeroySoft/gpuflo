//! On-demand process attribution from DRM fdinfo and KFD.
//!
//! Two independent accounting systems are reported without reconciliation:
//! DRM fdinfo (associated by `drm-pdev`, memory from the non-deprecated
//! `drm-resident-*` keys, KiB × 1024) and KFD (`/sys/class/kfd/kfd/proc`,
//! membership plus `vram_<gpuid>` bytes). No utilization or engine-time
//! claim is produced. Command lines are never read.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

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
    pub read_mono: Instant,
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

impl ProcessSource {
    /// Creates the adapter over an explicit root (`/` in production).
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Maps KFD `gpu_id` values to PCI BDFs from the KFD topology tree.
    fn kfd_topology(&self) -> HashMap<u64, PciBdf> {
        let mut map = HashMap::new();
        let nodes = self.root.join("sys/class/kfd/kfd/topology/nodes");
        let Ok(entries) = std::fs::read_dir(&nodes) else {
            return map;
        };
        for entry in entries.flatten() {
            let dir = entry.path();
            let Ok(gpu_id) = std::fs::read_to_string(dir.join("gpu_id")) else {
                continue;
            };
            let Ok(gpu_id) = gpu_id.trim().parse::<u64>() else {
                continue;
            };
            if gpu_id == 0 {
                continue; // CPU node.
            }
            let Ok(properties) = std::fs::read_to_string(dir.join("properties")) else {
                continue;
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
            let Some(location) = location else { continue };
            let bus = (location >> 8) & 0xFF;
            let device = (location >> 3) & 0x1F;
            let function = location & 0x7;
            if let Ok(bdf) =
                PciBdf::parse(&format!("{domain:04x}:{bus:02x}:{device:02x}.{function:x}"))
            {
                map.insert(gpu_id, bdf);
            }
        }
        map
    }

    /// Performs one bounded full scan. Enumerates candidate PIDs from both
    /// `/proc` fdinfo and the world-readable KFD proc tree so a permission-
    /// limited fdinfo still yields an honest row.
    pub fn scan(&self) -> ProcessSample {
        let read_wall = Timestamp::now();
        let read_mono = Instant::now();
        let topology = self.kfd_topology();

        // (pid, Some(bdf)) or (pid, None) for unresolvable associations.
        let mut evidence: HashMap<(u32, Option<PciBdf>), Accumulator> = HashMap::new();

        // DRM fdinfo pass over numeric /proc entries.
        if let Ok(entries) = std::fs::read_dir(self.root.join("proc")) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let Some(pid) = name.to_str().and_then(|n| n.parse::<u32>().ok()) else {
                    continue;
                };
                self.scan_fdinfo(pid, &entry.path(), &mut evidence);
            }
        }

        // KFD pass: membership and separate memory accounting.
        let kfd_proc = self.root.join("sys/class/kfd/kfd/proc");
        if let Ok(entries) = std::fs::read_dir(&kfd_proc) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let Some(pid) = name.to_str().and_then(|n| n.parse::<u32>().ok()) else {
                    continue;
                };
                self.scan_kfd(pid, &entry.path(), &topology, &mut evidence);
            }
        }

        let mut rows: Vec<ProcessRow> = evidence
            .into_iter()
            .map(|((pid, bdf), acc)| {
                let proc_dir = self.root.join("proc").join(pid.to_string());
                let name = read_comm(&proc_dir);
                let container = read_container(&proc_dir);
                let field = |kib: Option<u64>, malformed: bool| {
                    if malformed && kib.is_none() {
                        Reading::Malformed
                    } else {
                        fdinfo_reading(kib, &proc_dir)
                    }
                };
                let vram = field(acc.fdinfo_vram_kib, acc.vram_malformed);
                let gtt = field(acc.fdinfo_gtt_kib, acc.gtt_malformed);
                ProcessRow {
                    pid,
                    name,
                    bdf,
                    fdinfo_vram_bytes: vram,
                    fdinfo_gtt_bytes: gtt,
                    kfd_vram_bytes: acc.kfd_vram_bytes.unwrap_or(Reading::Absent),
                    container,
                }
            })
            .collect();

        // Sort primarily by attributed GPU memory, descending; the two
        // accounting systems stay separate, so order on the larger honest
        // single figure without summing them.
        rows.sort_by(|a, b| {
            let key = |row: &ProcessRow| {
                let fdinfo = match row.fdinfo_vram_bytes {
                    Reading::Value(v) => v,
                    _ => 0,
                };
                let kfd = match row.kfd_vram_bytes {
                    Reading::Value(v) => v,
                    _ => 0,
                };
                fdinfo.max(kfd)
            };
            key(b).cmp(&key(a)).then_with(|| a.pid.cmp(&b.pid))
        });

        ProcessSample {
            read_wall,
            read_mono,
            rows,
        }
    }

    /// Reads every fdinfo entry of one PID, deduplicating DRM clients.
    fn scan_fdinfo(
        &self,
        pid: u32,
        proc_dir: &Path,
        evidence: &mut HashMap<(u32, Option<PciBdf>), Accumulator>,
    ) {
        let Ok(entries) = std::fs::read_dir(proc_dir.join("fdinfo")) else {
            return;
        };
        // One DRM client may be visible through several duplicated fds;
        // count each client id once.
        let mut seen_clients: Vec<(Option<PciBdf>, String)> = Vec::new();
        for entry in entries.flatten() {
            let Ok(content) = std::fs::read_to_string(entry.path()) else {
                continue;
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
            let client = (pdev.clone(), client_id);
            if seen_clients.contains(&client) {
                continue;
            }
            seen_clients.push(client);
            let acc = evidence.entry((pid, pdev)).or_default();
            match vram_kib {
                Some(Ok(kib)) => {
                    *acc.fdinfo_vram_kib.get_or_insert(0) += kib;
                }
                Some(Err(())) => acc.vram_malformed = true,
                None => {}
            }
            match gtt_kib {
                Some(Ok(kib)) => {
                    *acc.fdinfo_gtt_kib.get_or_insert(0) += kib;
                }
                Some(Err(())) => acc.gtt_malformed = true,
                None => {}
            }
        }
    }

    /// Reads one KFD proc entry: queue membership and `vram_<gpuid>`.
    fn scan_kfd(
        &self,
        pid: u32,
        kfd_dir: &Path,
        topology: &HashMap<u64, PciBdf>,
        evidence: &mut HashMap<(u32, Option<PciBdf>), Accumulator>,
    ) {
        let Ok(entries) = std::fs::read_dir(kfd_dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let Some(gpu_id) = name.strip_prefix("vram_") else {
                continue;
            };
            let Ok(gpu_id) = gpu_id.parse::<u64>() else {
                continue;
            };
            let bdf = topology.get(&gpu_id).cloned();
            let reading = match std::fs::read_to_string(entry.path()) {
                Ok(text) => match text.trim().parse::<u64>() {
                    Ok(bytes) => Reading::Value(bytes),
                    Err(_) => Reading::Malformed,
                },
                Err(error) => super::kernel_error_reading(&error),
            };
            evidence.entry((pid, bdf)).or_default().kfd_vram_bytes = Some(reading);
        }
    }
}

/// fdinfo memory: accumulated KiB × 1024, or the reason it is missing.
fn fdinfo_reading(kib: Option<u64>, proc_dir: &Path) -> Reading<u64> {
    match kib {
        Some(kib) => Reading::Value(kib * 1024),
        // Distinguish an unreadable fdinfo directory (permission-limited
        // cross-user attribution) from a KFD-only process.
        None => match std::fs::read_dir(proc_dir.join("fdinfo")) {
            Ok(_) => Reading::Absent,
            Err(error) => super::kernel_error_reading(&error),
        },
    }
}

/// `KiB`-suffixed fdinfo quantity.
fn parse_kib(value: &str) -> Result<u64, ()> {
    let number = value.strip_suffix("KiB").unwrap_or(value).trim();
    number.parse::<u64>().map_err(|_| ())
}

/// Permitted process name from `comm`; never the command line.
fn read_comm(proc_dir: &Path) -> Reading<String> {
    match std::fs::read_to_string(proc_dir.join("comm")) {
        Ok(text) => Reading::Value(text.trim().to_owned()),
        Err(error) => super::kernel_error_reading(&error),
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

    fn row<'a>(sample: &'a ProcessSample, pid: u32) -> &'a ProcessRow {
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
}
