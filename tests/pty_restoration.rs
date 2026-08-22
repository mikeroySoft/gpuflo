//! Exactly three pseudoterminal restoration journeys against the compiled
//! interactive binary: normal quit, catchable signal, and injected fatal.
//!
//! Each journey verifies cursor restoration and alternate-screen exit
//! byte sequences on the PTY stream, plus diagnostic ordering for the
//! fatal path. SIGKILL/abort remain inherently outside this contract.

use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const ENTER_ALT: &str = "\x1b[?1049h";
const LEAVE_ALT: &str = "\x1b[?1049l";
const SHOW_CURSOR: &str = "\x1b[?25h";

struct Pty {
    master: OwnedFd,
    child: Child,
}

/// Copies the committed fixture into a mutable per-test host root.
fn fixture_root(tag: &str) -> PathBuf {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/kernel/discrete-spx");
    let root = std::env::temp_dir().join(format!("gpuflo-pty-{tag}-{}", std::process::id()));
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

/// Spawns the actual debug gpuflo binary on a fresh PTY.
fn spawn_gpuflo(tag: &str, extra_env: &[(&str, &str)]) -> Pty {
    let root = fixture_root(tag);

    // SAFETY: openpty fills two fresh descriptors on success.
    let (master, slave) = unsafe {
        let mut master: libc::c_int = 0;
        let mut slave: libc::c_int = 0;
        let winsize = libc::winsize {
            ws_row: 40,
            ws_col: 120,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let rc = libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &winsize,
        );
        assert_eq!(rc, 0, "openpty failed");
        (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave))
    };

    let slave_fd = slave.as_raw_fd();
    let mut command = Command::new(env!("CARGO_BIN_EXE_gpuflo"));
    command
        .env("GPUFLO_HOST_ROOT", &root)
        .env("TERM", "xterm-256color")
        .env_remove("NO_COLOR")
        .stdin(unsafe { Stdio::from_raw_fd(libc::dup(slave_fd)) })
        .stdout(unsafe { Stdio::from_raw_fd(libc::dup(slave_fd)) })
        .stderr(unsafe { Stdio::from_raw_fd(libc::dup(slave_fd)) });
    for (key, value) in extra_env {
        command.env(key, value);
    }
    // SAFETY: in the child, create a session and adopt the PTY slave as the
    // controlling terminal before exec.
    unsafe {
        command.pre_exec(move || {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::ioctl(slave_fd, libc::TIOCSCTTY, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = command.spawn().expect("spawn gpuflo on the pty");
    drop(slave);
    Pty { master, child }
}

impl Pty {
    /// Reads PTY output until `needle` appears or the deadline passes.
    fn read_until(&mut self, collected: &mut String, needle: &str, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        let mut file = std::fs::File::from(self.master.try_clone().unwrap());
        // SAFETY: O_NONBLOCK on the master keeps reads bounded.
        unsafe {
            let flags = libc::fcntl(self.master.as_raw_fd(), libc::F_GETFL);
            libc::fcntl(
                self.master.as_raw_fd(),
                libc::F_SETFL,
                flags | libc::O_NONBLOCK,
            );
        }
        let mut buffer = [0u8; 4096];
        while Instant::now() < deadline {
            match file.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => collected.push_str(&String::from_utf8_lossy(&buffer[..n])),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(_) => break, // EIO after child exit: PTY closed.
            }
            if collected.contains(needle) {
                return;
            }
        }
    }

    fn write(&mut self, bytes: &[u8]) {
        // SAFETY: writing to the owned master descriptor.
        unsafe {
            let rc = libc::write(self.master.as_raw_fd(), bytes.as_ptr().cast(), bytes.len());
            assert!(rc >= 0, "pty write failed");
        }
    }

    fn wait_exit(&mut self, timeout: Duration) -> std::process::ExitStatus {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.child.try_wait().expect("wait") {
                return status;
            }
            assert!(Instant::now() < deadline, "gpuflo did not exit in time");
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

/// Asserts cursor restoration precedes the alternate-screen exit in the
/// stream produced after `marker`.
fn assert_restored(output: &str) {
    let show = output.rfind(SHOW_CURSOR).expect("cursor restored");
    let leave = output.rfind(LEAVE_ALT).expect("alternate screen left");
    assert!(
        show < leave,
        "restoration order must be cursor, raw mode, screen"
    );
}

#[test]
fn normal_quit_restores_the_terminal() {
    let mut pty = spawn_gpuflo("quit", &[]);
    let mut output = String::new();
    pty.read_until(&mut output, ENTER_ALT, Duration::from_secs(5));
    assert!(output.contains(ENTER_ALT), "interactive mode entered");
    // Let the first frames render, then quit normally.
    pty.read_until(
        &mut output,
        "GPUFLO-NEVER-MATCHES",
        Duration::from_millis(700),
    );
    pty.write(b"q");
    let status = pty.wait_exit(Duration::from_secs(5));
    pty.read_until(&mut output, LEAVE_ALT, Duration::from_secs(2));
    assert_eq!(status.code(), Some(0));
    assert_restored(&output);
}

#[test]
fn sigint_restores_the_terminal_and_exits_130() {
    let mut pty = spawn_gpuflo("sigint", &[]);
    let mut output = String::new();
    pty.read_until(&mut output, ENTER_ALT, Duration::from_secs(5));
    pty.read_until(
        &mut output,
        "GPUFLO-NEVER-MATCHES",
        Duration::from_millis(700),
    );
    // SAFETY: signaling the child we spawned.
    unsafe {
        libc::kill(pty.child.id() as libc::pid_t, libc::SIGINT);
    }
    let status = pty.wait_exit(Duration::from_secs(5));
    pty.read_until(&mut output, LEAVE_ALT, Duration::from_secs(2));
    assert_eq!(status.code(), Some(130));
    assert_restored(&output);
}

#[test]
fn injected_fatal_restores_before_the_diagnostic_and_exits_1() {
    let mut pty = spawn_gpuflo("fatal", &[("GPUFLO_FATAL_AFTER_MS", "600")]);
    let mut output = String::new();
    pty.read_until(&mut output, ENTER_ALT, Duration::from_secs(5));
    let status = pty.wait_exit(Duration::from_secs(5));
    pty.read_until(&mut output, "injected fatal", Duration::from_secs(2));
    assert_eq!(status.code(), Some(1));
    assert_restored(&output);
    // The fatal diagnostic prints only after terminal restoration.
    let leave = output.rfind(LEAVE_ALT).expect("alternate screen left");
    let diagnostic = output.rfind("injected fatal").expect("diagnostic printed");
    assert!(
        leave < diagnostic,
        "restoration must precede the fatal diagnostic"
    );
}
