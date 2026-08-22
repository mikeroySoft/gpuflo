//! One public `Monitor` integration journey over the supported interface,
//! driven against a real (fixture) host root so discovery, kernel lanes,
//! normalization, the reducer, and both outward mailboxes are exercised
//! together.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gpuflo::{
    Monitor, MonitorCommand, MonitorEvent, MonitorOptions, PhysicalGpuId, ReceiveTimeoutError,
};

use std::sync::atomic::{AtomicBool, Ordering};

static DEBUG_ENV: AtomicBool = AtomicBool::new(false);

struct EnvGuard;

impl EnvGuard {
    fn acquire() -> Self {
        while DEBUG_ENV
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            std::thread::yield_now();
        }
        Self
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        DEBUG_ENV.store(false, Ordering::Release);
    }
}
/// Copies a committed fixture tree into a unique mutable temp root.
fn mutable_root(scenario: &str, tag: &str) -> PathBuf {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/kernel")
        .join(scenario);
    let root = std::env::temp_dir().join(format!("gpuflo-journey-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    copy_tree(&source, &root);
    root
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

fn start(root: &Path) -> Monitor {
    start_with_options(root, MonitorOptions::new())
}

fn start_with_options(root: &Path, options: MonitorOptions) -> Monitor {
    let _guard = EnvGuard::acquire();
    // SAFETY: serialized within this test process and consumed synchronously
    // by Monitor::start before the guard is released.
    unsafe { std::env::set_var("GPUFLO_HOST_ROOT", root) };
    let result = Monitor::start(options);
    unsafe { std::env::remove_var("GPUFLO_HOST_ROOT") };
    result.expect("monitor starts on the fixture host")
}

fn next_snapshot(monitor: &Monitor) -> gpuflo::Snapshot {
    loop {
        match monitor
            .receive_timeout(Duration::from_secs(3))
            .expect("event within bound")
        {
            MonitorEvent::Snapshot(snapshot) => return snapshot,
            MonitorEvent::Notice(_) => continue,
            MonitorEvent::Fatal(error) => panic!("unexpected fatal: {error}"),
            other => panic!("unexpected event: {other:?}"),
        }
    }
}

#[test]
fn full_monitor_journey() {
    let root = mutable_root("discrete-spx", "journey");
    let started = Instant::now();
    let monitor = start(&root);

    // 1. Priming: the first public snapshot follows the second fast sample,
    //    carries sequence 1, and arrives within the one-second budget.
    let first = next_snapshot(&monitor);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "first snapshot took {:?}",
        started.elapsed()
    );
    assert_eq!(first.sequence, Some(1));
    assert_eq!(first.gpus.len(), 1);
    let gpu = &first.gpus[0];
    assert_eq!(gpu.id, PhysicalGpuId::new("gpu-73fbc17bb4b8d1ce"));
    // Coherent blob value (97%), not the divergent text node (96%).
    assert_eq!(gpu.partitions[0].activity_percent.current(), Some(&97.0));
    assert_eq!(gpu.temperature.hotspot_celsius.current(), Some(&87.0));

    // 2. Production cadence: consecutive snapshots are one 250 ms tick apart.
    let second = next_snapshot(&monitor);
    let gap = second.sampled_at.as_odt() - first.sampled_at.as_odt();
    assert!(
        gap >= time::Duration::milliseconds(100) && gap <= time::Duration::milliseconds(600),
        "production gap was {gap}"
    );

    // 3. Slow-lane observations arrive with their own source times.
    let deadline = Instant::now() + Duration::from_secs(3);
    let with_slow = loop {
        let snapshot = next_snapshot(&monitor);
        if snapshot.gpus[0].power.cap_watts.current().is_some() {
            break snapshot;
        }
        assert!(Instant::now() < deadline, "slow collection never arrived");
    };
    assert_eq!(with_slow.gpus[0].power.cap_watts.current(), Some(&300.0));
    assert_eq!(
        with_slow.gpus[0].temperature.limit_celsius.current(),
        Some(&95.0)
    );
    assert_eq!(
        with_slow.gpus[0].partitions[0].gfx_clock_mhz.current(),
        Some(&1700.0)
    );

    // 4. A slow receiver observes a sequence gap, never queue growth.
    let before = next_snapshot(&monitor).sequence.unwrap();
    std::thread::sleep(Duration::from_millis(1200));
    let after = next_snapshot(&monitor).sequence.unwrap();
    assert!(
        after >= before + 3,
        "expected a visible sequence gap, got {before} -> {after}"
    );

    // 5. Commands are accepted while running.
    monitor
        .command(MonitorCommand::SetProcessScope(Some(PhysicalGpuId::new(
            "gpu-73fbc17bb4b8d1ce",
        ))))
        .unwrap();
    monitor.command(MonitorCommand::ResetSessionPeaks).unwrap();
    monitor
        .command(MonitorCommand::SetProcessScope(None))
        .unwrap();

    // 6. Confirmed disappearance: one factual notice, delivered before any
    //    pending snapshot, then empty snapshots while discovery continues.
    std::fs::remove_dir_all(root.join("sys/class/drm/card1")).unwrap();
    let deadline = Instant::now() + Duration::from_secs(6);
    let notice = loop {
        match monitor
            .receive_timeout(Duration::from_secs(6))
            .expect("event")
        {
            MonitorEvent::Notice(notice) => break notice,
            MonitorEvent::Snapshot(_) => {
                assert!(Instant::now() < deadline, "disconnect notice never arrived");
            }
            MonitorEvent::Fatal(error) => panic!("unexpected fatal: {error}"),
            other => panic!("unexpected event: {other:?}"),
        }
    };
    assert_eq!(notice.message, "GPU disconnected: gpu-73fbc17bb4b8d1ce");
    let empty = next_snapshot(&monitor);
    assert!(empty.gpus.is_empty(), "runtime empty state keeps streaming");

    // 7. Bounded shutdown.
    let begun = Instant::now();
    monitor.shutdown().expect("bounded shutdown");
    assert!(begun.elapsed() < Duration::from_secs(3));
}

#[test]
fn partition_configuration_change_is_fatal_and_closes_the_stream() {
    let root = mutable_root("discrete-spx", "fatal");
    let monitor = start(&root);
    let _ = next_snapshot(&monitor);

    // Grow the same package (domain:bus:device) by one partition function.
    let device = root.join("sys/class/drm/card9/device");
    std::fs::create_dir_all(&device).unwrap();
    std::fs::write(
        device.join("uevent"),
        "DRIVER=amdgpu\nPCI_SLOT_NAME=0000:41:00.1\n",
    )
    .unwrap();
    std::fs::write(device.join("vendor"), "0x1002\n").unwrap();
    std::fs::write(device.join("mem_info_vram_total"), "68719476736\n").unwrap();
    std::fs::write(device.join("mem_info_vram_used"), "0\n").unwrap();

    // Fatal must arrive, be delivered before pending snapshots, and close
    // the stream: no snapshot may follow it.
    let deadline = Instant::now() + Duration::from_secs(6);
    loop {
        match monitor
            .receive_timeout(Duration::from_secs(6))
            .expect("event")
        {
            MonitorEvent::Fatal(error) => {
                assert_eq!(
                    error.to_string(),
                    "GPU partition configuration changed; restart gpuflo"
                );
                break;
            }
            MonitorEvent::Snapshot(_) | MonitorEvent::Notice(_) => {
                assert!(Instant::now() < deadline, "fatal never arrived");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }
    match monitor.receive_timeout(Duration::from_secs(1)) {
        Err(ReceiveTimeoutError::Closed) => {}
        other => panic!("stream must close after fatal, got {other:?}"),
    }
    monitor.shutdown().expect("shutdown after fatal");
}

#[test]
fn shutdown_surfaces_persistence_failure() {
    let root = mutable_root("discrete-spx", "persist-failure");
    let blocked_parent =
        std::env::temp_dir().join(format!("gpuflo-state-blocked-{}", std::process::id()));
    std::fs::write(&blocked_parent, "not a directory").unwrap();
    let mut options = MonitorOptions::new();
    options.summary_path = Some(blocked_parent.join("daily.json"));
    let monitor = start_with_options(&root, options);
    let _ = next_snapshot(&monitor);
    let error = monitor
        .shutdown()
        .expect_err("persistence failure must surface");
    assert!(error.to_string().contains("persistence"));
    let _ = std::fs::remove_file(blocked_parent);
}
