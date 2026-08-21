//! Atomic daily-summary persistence.
//!
//! Loads and writes only the small [`DailySummaryRecord`] at a path already
//! resolved by `config`; this module never reads environment variables.
//! Writes use a same-directory temporary file plus atomic rename, coalesced
//! through one latest-value slot with a capacity-one wakeup.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, bounded};

use crate::state::history::DailySummaryRecord;

/// Daily summaries are tiny; refuse planted or corrupted oversized files.
const MAX_RECORD_BYTES: u64 = 64 * 1024;
/// Process-local uniqueness for `create_new` temporary files.
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Loads a regular, bounded persisted record. Missing is normal; malformed,
/// unsafe, oversized, and unreadable paths remain distinct failures.
pub(crate) fn load(path: &Path) -> Result<Option<DailySummaryRecord>, String> {
    use std::os::unix::fs::OpenOptionsExt;

    let file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(0x20000 | 0x800) // Linux O_NOFOLLOW | O_NONBLOCK
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("cannot open {}: {error}", path.display())),
    };
    let metadata = file
        .metadata()
        .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    if metadata.len() > MAX_RECORD_BYTES {
        return Err(format!(
            "{} exceeds {MAX_RECORD_BYTES} bytes",
            path.display()
        ));
    }
    let mut text = String::with_capacity(metadata.len() as usize);
    file.take(MAX_RECORD_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if text.len() as u64 > MAX_RECORD_BYTES {
        return Err(format!(
            "{} exceeds {MAX_RECORD_BYTES} bytes",
            path.display()
        ));
    }
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|error| format!("invalid daily summary {}: {error}", path.display()))
}

/// Writes the record atomically: an exclusive, same-directory temporary
/// regular file, flush, fsync, then rename. `create_new` refuses a planted
/// symlink; a failure leaves any previous complete file intact.
pub(crate) fn store(path: &Path, record: &DailySummaryRecord) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let body = serde_json::to_vec_pretty(record)?;
    let (tmp, mut file) = loop {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let mut name = path.as_os_str().to_owned();
        name.push(format!(".tmp.{}.{sequence}", std::process::id()));
        let tmp = PathBuf::from(name);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
        {
            Ok(file) => break (tmp, file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    };
    let result = (|| {
        file.write_all(&body)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)?;
        std::fs::File::open(parent)?.sync_all()
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Coalescing persistence lane: one latest-value slot, one wakeup channel,
/// one writer thread. `None` path disables persistence entirely.
pub(crate) struct PersistLane {
    slot: Arc<Mutex<Option<DailySummaryRecord>>>,
    wake: Option<Sender<()>>,
    done: Option<Receiver<()>>,
    join: Option<std::thread::JoinHandle<()>>,
    error: Arc<Mutex<Option<String>>>,
}

impl PersistLane {
    /// Starts the writer thread, or a disabled lane when `path` is `None`.
    pub fn start(path: Option<PathBuf>) -> Self {
        let Some(path) = path else {
            return Self {
                slot: Arc::new(Mutex::new(None)),
                wake: None,
                done: None,
                join: None,
                error: Arc::new(Mutex::new(None)),
            };
        };
        let slot = Arc::new(Mutex::new(None::<DailySummaryRecord>));
        let error = Arc::new(Mutex::new(None::<String>));
        let (wake_tx, wake_rx) = bounded::<()>(1);
        let (done_tx, done_rx) = bounded::<()>(1);
        let thread_slot = Arc::clone(&slot);
        let thread_error = Arc::clone(&error);
        let join = std::thread::Builder::new()
            .name("gruflo-persist".to_owned())
            .spawn(move || {
                // Wake sender dropping closes the loop; write any final state.
                while wake_rx.recv().is_ok() {
                    if let Some(record) = thread_slot.lock().ok().and_then(|mut s| s.take())
                        && let Err(failure) = store(&path, &record)
                        && let Ok(mut error) = thread_error.lock()
                    {
                        *error = Some(failure.to_string());
                    }
                }
                if let Some(record) = thread_slot.lock().ok().and_then(|mut s| s.take())
                    && let Err(failure) = store(&path, &record)
                    && let Ok(mut error) = thread_error.lock()
                {
                    *error = Some(failure.to_string());
                }
                let _ = done_tx.send(());
            })
            .expect("spawn persistence thread");
        Self {
            slot,
            wake: Some(wake_tx),
            done: Some(done_rx),
            join: Some(join),
            error,
        }
    }

    /// Replaces the pending summary with the newest state and wakes the
    /// writer. Obsolete pending summaries are coalesced, never queued.
    pub fn update(&self, record: DailySummaryRecord) {
        if self.wake.is_none() {
            return;
        }
        if let Ok(mut slot) = self.slot.lock() {
            *slot = Some(record);
        }
        if let Some(wake) = &self.wake {
            let _ = wake.try_send(());
        }
    }

    /// Requests a final flush and joins the writer to a bounded deadline.
    pub fn shutdown(mut self, deadline: Duration) -> Result<(), String> {
        let Some(wake) = self.wake.take() else {
            return Ok(());
        };
        drop(wake); // Closes the loop; the writer flushes remaining state.
        let done = self.done.take().expect("done channel present when enabled");
        if done.recv_timeout(deadline).is_err() {
            return Err("persistence lane did not stop within the shutdown bound".to_owned());
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
        match self.error.lock().ok().and_then(|mut error| error.take()) {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::os::unix::fs::DirBuilderExt;

    use super::*;
    use crate::state::history::GpuDailyRecord;

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn record(date: &str, peak: f64) -> DailySummaryRecord {
        let mut gpus = BTreeMap::new();
        gpus.insert(
            "gpu-a".to_owned(),
            GpuDailyRecord {
                activity_peak_percent: Some(peak),
                memory_peak_percent: Some(50.0),
                energy_joules: None,
            },
        );
        DailySummaryRecord {
            date: date.to_owned(),
            gpus,
        }
    }

    fn temp_path(name: &str) -> PathBuf {
        loop {
            let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir()
                .join(format!("gruflo-persist-{}-{sequence}", std::process::id()));
            match std::fs::DirBuilder::new().mode(0o700).create(&root) {
                Ok(()) => return root.join(name),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("cannot create test directory: {error}"),
            }
        }
    }

    #[test]
    fn missing_is_absent_and_malformed_is_an_error() {
        assert_eq!(
            load(Path::new("/nonexistent/gruflo/daily.json")).unwrap(),
            None
        );
        let path = temp_path("malformed/daily.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ not json").unwrap();
        assert!(load(&path).is_err());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn store_round_trips_and_replaces_atomically() {
        let path = temp_path("roundtrip/state/daily.json");
        let first = record("2026-08-21", 80.0);
        store(&path, &first).unwrap();
        assert_eq!(load(&path).unwrap(), Some(first));
        let second = record("2026-08-21", 95.0);
        store(&path, &second).unwrap();
        assert_eq!(load(&path).unwrap(), Some(second));
        // No temporary file remains.
        let entries: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(entries, vec![std::ffi::OsString::from("daily.json")]);
        let _ = std::fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn failed_write_preserves_previous_complete_file() {
        let path = temp_path("failwrite/daily.json");
        let first = record("2026-08-21", 80.0);
        store(&path, &first).unwrap();
        // Make the directory read-only so the temp file cannot be created.
        let dir = path.parent().unwrap();
        let mut perms = std::fs::metadata(dir).unwrap().permissions();
        let original = perms.clone();
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(0o555);
        std::fs::set_permissions(dir, perms).unwrap();
        let result = store(&path, &record("2026-08-21", 99.0));
        std::fs::set_permissions(dir, original).unwrap();
        assert!(result.is_err());
        assert_eq!(load(&path).unwrap(), Some(first));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn lane_coalesces_to_newest_and_flushes_on_shutdown() {
        let path = temp_path("lane/daily.json");
        let lane = PersistLane::start(Some(path.clone()));
        for peak in [10.0, 20.0, 99.0] {
            lane.update(record("2026-08-21", peak));
        }
        assert!(lane.shutdown(Duration::from_secs(5)).is_ok());
        let loaded = load(&path)
            .expect("load succeeds")
            .expect("record persisted");
        assert_eq!(loaded.gpus["gpu-a"].activity_peak_percent, Some(99.0));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn disabled_lane_accepts_updates_and_shutdown() {
        let lane = PersistLane::start(None);
        lane.update(record("2026-08-21", 10.0));
        assert!(lane.shutdown(Duration::from_millis(100)).is_ok());
    }

    #[test]
    fn load_rejects_symlinks_and_oversized_records() {
        use std::os::unix::fs::symlink;

        let dir = temp_path("safe-load");
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("target.json");
        std::fs::write(
            &target,
            serde_json::to_vec(&record("2026-08-21", 1.0)).unwrap(),
        )
        .unwrap();
        let linked = dir.join("linked.json");
        symlink(&target, &linked).unwrap();
        assert!(load(&linked).is_err());

        let oversized = dir.join("oversized.json");
        std::fs::write(&oversized, vec![b' '; MAX_RECORD_BYTES as usize + 1]).unwrap();
        assert!(load(&oversized).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn planted_temp_symlinks_are_never_followed() {
        use std::os::unix::fs::symlink;

        let path = temp_path("safe-store/daily.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let victim = path.parent().unwrap().join("victim");
        std::fs::write(&victim, "keep me").unwrap();
        // Cover every process-local sequence this test suite can plausibly
        // have consumed; store must skip them via create_new.
        for sequence in 0..1024 {
            let mut tmp = path.as_os_str().to_owned();
            tmp.push(format!(".tmp.{}.{sequence}", std::process::id()));
            symlink(&victim, PathBuf::from(tmp)).unwrap();
        }
        store(&path, &record("2026-08-21", 2.0)).unwrap();
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep me");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn lane_reports_final_write_failure() {
        let dir = temp_path("lane-failure");
        std::fs::write(&dir, "not a directory").unwrap();
        let lane = PersistLane::start(Some(dir.join("daily.json")));
        lane.update(record("2026-08-21", 10.0));
        assert!(lane.shutdown(Duration::from_secs(5)).is_err());
        let _ = std::fs::remove_file(dir);
    }
}
