//! Fire-and-forget subprocess reaping (zombie prevention).
//!
//! A long-running process that does `std::process::Command::spawn()` and drops
//! the `Child` without `wait()` leaks a **zombie per call** — `std` installs no
//! SIGCHLD reaper, and only `tokio::process` children are reaped by the tokio
//! runtime. mackesd fires `mde-bus publish …` fire-and-forget from several
//! workers on a tick (config-tags, voip-rtt, compute-registry, firewall-monitor,
//! ca-revoke), so those defunct children piled up — the `state=Z` `mde-bus`
//! pile behind the live-mesh wedge.
//!
//! [`fire_and_reap`] keeps the fire-and-forget ergonomics (non-blocking, errors
//! swallowed) but spawns a short detached thread that `wait()`s on the child so
//! the kernel reaps it, with a timeout kill-switch so a hung child can't leak
//! the reaper thread instead.
//!
//! Pure `std` + lives OUTSIDE the `async-services`-gated `workers` module so the
//! always-compiled callers (`ca::revoke`, `voip_rtt`) can use it in a
//! no-default-features library build too.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Default reap deadline — generous for a quick `mde-bus publish`, short enough
/// that a wedged child (and its reaper thread) is cleaned up promptly.
pub const DEFAULT_REAP_TIMEOUT: Duration = Duration::from_secs(15);

/// Reaper poll cadence.
const POLL: Duration = Duration::from_millis(100);

/// Spawn `cmd` fire-and-forget and reap it on a detached thread (no zombie).
///
/// Non-blocking: returns immediately after the spawn. The child's
/// stdin/stdout/stderr are redirected to `/dev/null`. A spawn failure is
/// swallowed — every caller graceful-degrades when the target binary isn't
/// invocable (e.g. a pre-RPM dev box). If the child outlives `timeout` it is
/// killed and reaped so the reaper thread always terminates.
pub fn fire_and_reap(mut cmd: Command, timeout: Duration) {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let Ok(mut child) = cmd.spawn() else { return };
    std::thread::spawn(move || {
        let deadline = Instant::now() + timeout;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => {
                    if Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        return;
                    }
                    std::thread::sleep(POLL);
                }
                Err(_) => return,
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonblocking_and_reaps_a_slow_child() {
        // Returns immediately even for a slow child (fire-and-forget)…
        let mut cmd = Command::new("sleep");
        cmd.arg("1");
        let start = Instant::now();
        fire_and_reap(cmd, Duration::from_secs(10));
        assert!(
            start.elapsed() < Duration::from_millis(500),
            "fire_and_reap must not block the caller"
        );
        // …and the detached reaper waits the child out (no zombie left behind).
        std::thread::sleep(Duration::from_millis(1300));
    }

    #[test]
    fn swallows_spawn_failure() {
        let cmd = Command::new("/does/not/exist/never-real-binary");
        // Must not panic when the binary is absent (pre-RPM dev box).
        fire_and_reap(cmd, Duration::from_secs(1));
    }
}
