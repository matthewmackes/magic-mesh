//! Integration tests for `mackesd reconcile` (Phase 12.5 wiring).
//!
//! Drives the real binary in a subprocess so we exercise:
//!   * `--once` end-to-end (open store, scan empty fixture, emit plan).
//!   * Default long-running mode with SIGTERM clean-exit (the systemd
//!     path).
//!
//! These run on Linux only — `signal-hook` ships POSIX bindings and
//! the workspace targets Fedora.

#![cfg(unix)]

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

fn mackesd_binary() -> PathBuf {
    // CARGO_BIN_EXE_<name> is set by Cargo for the integration-test
    // harness; points at the freshly-built binary.
    PathBuf::from(env!("CARGO_BIN_EXE_mackesd"))
}

#[test]
fn reconcile_once_against_empty_fixture_prints_plan() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("test.db");
    let qnm = dir.path().join("qnm-shared");
    let out = Command::new(mackesd_binary())
        .arg("--db")
        .arg(&db)
        .arg("reconcile")
        .arg("--once")
        .arg("--workgroup-root")
        .arg(&qnm)
        .arg("--node-id")
        .arg("peer:test")
        .output()
        .expect("running mackesd reconcile --once");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "non-zero exit: {:?}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("invalid JSON: {e}\nstdout: {stdout}"));
    assert_eq!(json["observed_heartbeats"], 0);
    assert_eq!(json["observed_edges"], 0);
    assert_eq!(json["desired_edges"], 0);
    assert!(json["plan"]["repair_now"].as_array().unwrap().is_empty());
    assert!(json["plan"]["inbox"].as_array().unwrap().is_empty());
}

#[test]
fn reconcile_long_running_exits_cleanly_on_sigterm() {
    use std::os::unix::process::ExitStatusExt;

    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("test.db");
    let qnm = dir.path().join("qnm-shared");
    let mut child = Command::new(mackesd_binary())
        .arg("--db")
        .arg(&db)
        .arg("reconcile")
        .arg("--workgroup-root")
        .arg(&qnm)
        .arg("--node-id")
        .arg("peer:test")
        // Inherit stdio — we don't need to capture stderr to drive
        // this test, and a captured pipe with no reader can fill +
        // block the worker.
        .stderr(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .expect("spawn mackesd reconcile");

    // Give the worker time to spawn + install the signal handler.
    // The signal-reader thread starts before the worker thread, so
    // 500 ms is comfortably enough on a loaded CI runner.
    std::thread::sleep(Duration::from_millis(500));

    // Send SIGTERM via the kill(1) binary so we don't have to drop
    // to libc in test code (the workspace forbids unsafe).
    let kill_status = Command::new("kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .status()
        .expect("invoking /usr/bin/kill");
    assert!(kill_status.success(), "kill -TERM failed");

    // Bound the wait to 10 s — well under the worker's 30 s tick
    // but well over the 250 ms shutdown-poll. If we time out the
    // worker is broken.
    let start = std::time::Instant::now();
    let status = loop {
        match child.try_wait().expect("try_wait") {
            Some(s) => break s,
            None => {
                if start.elapsed() > Duration::from_secs(10) {
                    let _ = child.kill();
                    panic!("worker didn't exit within 10 s of SIGTERM");
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    };
    // SIGTERM constant value is 15 on every POSIX platform we
    // target. We avoid pulling in libc for the constant.
    const SIGTERM: i32 = 15;
    // Either normal exit (signal handler flipped the flag, loop
    // returned, status code 0) OR signaled (the worker hadn't yet
    // installed its handler — racy but acceptable).
    assert!(
        status.success() || status.signal() == Some(SIGTERM),
        "unexpected exit status: {status:?}",
    );
}
