//! EFF-20 — timeout-bounded subprocess execution.
//!
//! Several daemon workers shell out to system tools on a tick
//! (`firewall-cmd`, `resolvectl`, `systemctl`, `ping`, `lpadmin`, …).
//! Most did so with a bare `Command::output()` / `Command::status()`
//! and **no timeout**, so a child that hangs (a wedged `firewall-cmd`,
//! a `ping` to a black-hole) pins the caller forever — and because the
//! sync worker `tick_once()` helpers run *directly* on the tokio
//! runtime thread, a hang pins a whole runtime worker indefinitely.
//!
//! These helpers run a command with a hard deadline and kill the child
//! when it's exceeded, so the worst case is bounded to `timeout`
//! instead of "forever". Two flavours:
//!
//! * [`output_with_timeout`] / [`status_with_timeout`] — **blocking**,
//!   dependency-free (poll `try_wait`), for the sync `tick_once`
//!   helpers. Suited to the small-output system commands the workers
//!   run; a child that fills its ~64 KiB stdout pipe without exiting
//!   blocks on write and is killed at the deadline (acceptable here —
//!   these commands emit little).
//! * [`status_with_timeout_async`] — for workers already on
//!   `tokio::process`; wraps `child.wait()` in `tokio::time::timeout`
//!   and kills on expiry.

#![cfg(feature = "async-services")]

use std::process::{Command, ExitStatus, Output, Stdio};
use std::time::{Duration, Instant};

/// Default per-invocation timeout for the mesh workers' system
/// commands. Generous enough for a slow `firewall-cmd` reload, short
/// enough that a wedged child frees the thread within the tick budget.
pub const DEFAULT_CMD_TIMEOUT: Duration = Duration::from_secs(15);

/// Poll cadence for the blocking helpers' `try_wait` loop.
const POLL: Duration = Duration::from_millis(25);

/// Run `cmd` to completion capturing stdout+stderr, killing it (and
/// returning [`std::io::ErrorKind::TimedOut`]) if it runs longer than
/// `timeout`. Blocking — call from a sync context (or `spawn_blocking`).
///
/// # Errors
/// Spawn failure, a wait error, or the timeout (after which the child
/// is killed and reaped).
pub fn output_with_timeout(mut cmd: Command, timeout: Duration) -> std::io::Result<Output> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    let deadline = Instant::now() + timeout;
    loop {
        if child.try_wait()?.is_some() {
            return child.wait_with_output();
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(timed_out(timeout));
        }
        std::thread::sleep(POLL);
    }
}

/// Like [`output_with_timeout`] but discards output and returns only
/// the exit status (stdout/stderr go to `/dev/null`). Blocking.
///
/// # Errors
/// Spawn failure, a wait error, or the timeout (after which the child
/// is killed and reaped).
pub fn status_with_timeout(mut cmd: Command, timeout: Duration) -> std::io::Result<ExitStatus> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = cmd.spawn()?;
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(timed_out(timeout));
        }
        std::thread::sleep(POLL);
    }
}

/// Run a `tokio::process::Command` to completion with a hard timeout,
/// killing the child on expiry. For workers already on the async
/// process API.
///
/// # Errors
/// Spawn failure, a wait error, or the timeout (after which the child
/// is sent a kill signal).
pub async fn status_with_timeout_async(
    mut cmd: tokio::process::Command,
    timeout: Duration,
) -> std::io::Result<ExitStatus> {
    cmd.kill_on_drop(true);
    let mut child = cmd.spawn()?;
    match tokio::time::timeout(timeout, child.wait()).await {
        Ok(r) => r,
        Err(_) => {
            let _ = child.start_kill();
            Err(timed_out(timeout))
        }
    }
}

fn timed_out(timeout: Duration) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        format!("subprocess exceeded {}s timeout", timeout.as_secs()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_with_timeout_captures_fast_command() {
        let mut cmd = Command::new("echo");
        cmd.arg("hello");
        let out = output_with_timeout(cmd, Duration::from_secs(5)).expect("echo runs");
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hello");
    }

    #[test]
    fn status_with_timeout_reports_nonzero() {
        let cmd = Command::new("false");
        let st = status_with_timeout(cmd, Duration::from_secs(5)).expect("false runs");
        assert!(!st.success());
    }

    #[test]
    fn output_with_timeout_kills_a_hung_child() {
        let mut cmd = Command::new("sleep");
        cmd.arg("60");
        let start = Instant::now();
        let r = output_with_timeout(cmd, Duration::from_millis(150));
        assert!(r.is_err());
        assert_eq!(r.unwrap_err().kind(), std::io::ErrorKind::TimedOut);
        // Returned promptly at the deadline, not after sleep's 60 s.
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "must not wait for the child"
        );
    }

    #[test]
    fn spawn_failure_surfaces() {
        let cmd = Command::new("/does/not/exist/never-real-binary");
        assert!(output_with_timeout(cmd, Duration::from_secs(1)).is_err());
    }

    #[tokio::test]
    async fn async_status_kills_a_hung_child() {
        let mut cmd = tokio::process::Command::new("sleep");
        cmd.arg("60");
        let start = Instant::now();
        let r = status_with_timeout_async(cmd, Duration::from_millis(150)).await;
        assert!(r.is_err());
        assert!(start.elapsed() < Duration::from_secs(5));
    }
}
