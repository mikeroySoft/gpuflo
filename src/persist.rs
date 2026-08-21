//! Atomic daily-summary persistence.
//!
//! Loads and writes only the small [`DailySummaryRecord`] at a path already
//! resolved by `config`; this module never reads environment variables.
//! Writes use a same-directory temporary file plus atomic rename, coalesced
//! through one latest-value slot with a capacity-one wakeup.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, bounded};

use crate::state::history::DailySummaryRecord;

/// Loads the persisted record. Missing, unreadable, or malformed files are
/// treated as absent; a bad file never blocks startup.
pub(crate) fn load(path: &Path) -> Option<DailySummaryRecord> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Writes the record atomically: temporary file in the target directory,
/// flush, fsync, then rename. A failure leaves any previous file intact.
pub(crate) fn store(path: &Path, record: &DailySummaryRecord) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(&format!(".tmp.{}", std::process::id()));
    let tmp = PathBuf::from(tmp);
    let result = (|| {
        let mut file = std::fs::File::create(&tmp)?;
        let body = serde_json::to_vec_pretty(record)?;
        file.write_all(&body)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)
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
            };
        };
        let slot = Arc::new(Mutex::new(None::<DailySummaryRecord>));
        let (wake_tx, wake_rx) = bounded::<()>(1);
        let (done_tx, done_rx) = bounded::<()>(1);
        let thread_slot = Arc::clone(&slot);
        let join = std::thread::Builder::new()
            .name("gruflo-persist".to_owned())
            .spawn(move || {
                // Wake sender dropping closes the loop; write any final state.
                while wake_rx.recv().is_ok() {
                    if let Some(record) = thread_slot.lock().ok().and_then(|mut s| s.take()) {
                        let _ = store(&path, &record);
                    }
                }
                if let Some(record) = thread_slot.lock().ok().and_then(|mut s| s.take()) {
                    let _ = store(&path, &record);
                }
                let _ = done_tx.send(());
            })
            .expect("spawn persistence thread");
        Self {
            slot,
            wake: Some(wake_tx),
            done: Some(done_rx),
            join: Some(join),
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
    /// Returns false when the lane had to be detached.
    pub fn shutdown(mut self, deadline: Duration) -> bool {
        let Some(wake) = self.wake.take() else {
            return true;
        };
        drop(wake); // Closes the loop; the writer flushes remaining state.
        let done = self.done.take().expect("done channel present when enabled");
        let finished = done.recv_timeout(deadline).is_ok();
        if finished && let Some(join) = self.join.take() {
            let _ = join.join();
        }
        finished
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::state::history::GpuDailyRecord;

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
        std::env::temp_dir().join(format!("gruflo-persist-{}-{name}", std::process::id()))
    }

    #[test]
    fn missing_and_malformed_files_load_as_absent() {
        assert_eq!(load(Path::new("/nonexistent/gruflo/daily.json")), None);
        let path = temp_path("malformed/daily.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(load(&path), None);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn store_round_trips_and_replaces_atomically() {
        let path = temp_path("roundtrip/state/daily.json");
        let first = record("2026-08-21", 80.0);
        store(&path, &first).unwrap();
        assert_eq!(load(&path), Some(first));
        let second = record("2026-08-21", 95.0);
        store(&path, &second).unwrap();
        assert_eq!(load(&path), Some(second));
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
        assert_eq!(load(&path), Some(first));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn lane_coalesces_to_newest_and_flushes_on_shutdown() {
        let path = temp_path("lane/daily.json");
        let lane = PersistLane::start(Some(path.clone()));
        for peak in [10.0, 20.0, 99.0] {
            lane.update(record("2026-08-21", peak));
        }
        assert!(lane.shutdown(Duration::from_secs(5)));
        let loaded = load(&path).expect("record persisted");
        assert_eq!(loaded.gpus["gpu-a"].activity_peak_percent, Some(99.0));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn disabled_lane_accepts_updates_and_shutdown() {
        let lane = PersistLane::start(None);
        lane.update(record("2026-08-21", 10.0));
        assert!(lane.shutdown(Duration::from_millis(100)));
    }
}
