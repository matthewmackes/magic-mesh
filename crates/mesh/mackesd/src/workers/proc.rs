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
//! These helpers run each command in an invocation-owned process group, enforce
//! a hard deadline, and kill the whole group when the direct child exits or the
//! deadline expires. A hostile helper therefore cannot daemonize descendants or
//! retain inherited output pipes past the invocation. Two flavours:
//!
//! * [`output_with_timeout`] / [`status_with_timeout`] — **blocking**,
//!   dependency-free (poll `try_wait`), for the sync `tick_once`
//!   helpers. [`output_with_timeout`] drains stdout and stderr concurrently,
//!   retaining only a bounded prefix of each stream so a noisy child cannot
//!   deadlock the caller or grow memory without limit.
//! * [`status_with_timeout_async`] — for workers already on
//!   `tokio::process`; wraps `child.wait()` in `tokio::time::timeout`
//!   and kills on expiry.

#![cfg(feature = "async-services")]

use std::io::{self, Read};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Default per-invocation timeout for the mesh workers' system
/// commands. Generous enough for a slow `firewall-cmd` reload, short
/// enough that a wedged child frees the thread within the tick budget.
pub const DEFAULT_CMD_TIMEOUT: Duration = Duration::from_secs(15);

/// Poll cadence for the blocking helpers' `try_wait` loop.
const POLL: Duration = Duration::from_millis(25);

/// Maximum number of bytes retained from either captured stream.
///
/// The reader continues consuming after this limit and discards the excess,
/// keeping the child's pipe writable while bounding the returned [`Output`].
const MAX_CAPTURE_BYTES_PER_STREAM: usize = 64 * 1024;

const READ_BUFFER_BYTES: usize = 8 * 1024;

/// Run `cmd` to completion capturing stdout+stderr, killing it (and
/// returning [`std::io::ErrorKind::TimedOut`]) if it runs longer than
/// `timeout`. The command and its descendants are isolated in a fresh process
/// group which is retired before return. Blocking — call from a sync context
/// (or `spawn_blocking`).
///
/// At most [`MAX_CAPTURE_BYTES_PER_STREAM`] bytes are retained from each
/// stream. Excess output is drained and discarded, so a noisy child cannot
/// fill a pipe and deadlock the wait or cause unbounded memory growth.
///
/// # Errors
/// Spawn failure, a wait error, or the timeout (after which the invocation's
/// process group is killed and the direct child is reaped).
pub fn output_with_timeout(mut cmd: Command, timeout: Duration) -> std::io::Result<Output> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    isolate_process_group(&mut cmd);
    let mut child = cmd.spawn()?;

    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            reap_child(&mut child);
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "subprocess stdout pipe was not available",
            ));
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            reap_child(&mut child);
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "subprocess stderr pipe was not available",
            ));
        }
    };

    let stdout_reader = match spawn_pipe_reader(stdout, "mcnf-proc-stdout") {
        Ok(reader) => reader,
        Err(error) => {
            reap_child(&mut child);
            return Err(error);
        }
    };
    let stderr_reader = match spawn_pipe_reader(stderr, "mcnf-proc-stderr") {
        Ok(reader) => reader,
        Err(error) => {
            reap_child(&mut child);
            let _ = stdout_reader.join();
            return Err(error);
        }
    };

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                // A helper must not daemonize work past this invocation. Kill
                // any descendants that retained the invocation's process group
                // (and possibly its output pipes) before joining the readers.
                terminate_process_group(child.id());
                break Ok(status);
            }
            Ok(None) if Instant::now() >= deadline => {
                reap_child(&mut child);
                break Err(timed_out(timeout));
            }
            Ok(None) => thread::sleep(POLL),
            Err(error) => {
                reap_child(&mut child);
                break Err(error);
            }
        }
    };

    // Join both readers before returning, including on timeout/error, so no
    // background thread or pipe remains attached to a completed invocation.
    let stdout = join_pipe_reader(stdout_reader);
    let stderr = join_pipe_reader(stderr_reader);

    match status {
        Err(error) => Err(error),
        Ok(status) => Ok(Output {
            status,
            stdout: stdout?,
            stderr: stderr?,
        }),
    }
}

fn spawn_pipe_reader<R>(
    reader: R,
    name: &'static str,
) -> io::Result<JoinHandle<io::Result<Vec<u8>>>>
where
    R: Read + Send + 'static,
{
    thread::Builder::new()
        .name(name.to_string())
        .spawn(move || drain_pipe(reader))
}

fn drain_pipe<R>(mut reader: R) -> io::Result<Vec<u8>>
where
    R: Read,
{
    let mut captured = Vec::with_capacity(MAX_CAPTURE_BYTES_PER_STREAM.min(READ_BUFFER_BYTES));
    let mut buffer = [0u8; READ_BUFFER_BYTES];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            return Ok(captured);
        }

        let remaining = MAX_CAPTURE_BYTES_PER_STREAM.saturating_sub(captured.len());
        if remaining != 0 {
            captured.extend_from_slice(&buffer[..read.min(remaining)]);
        }
    }
}

fn join_pipe_reader(reader: JoinHandle<io::Result<Vec<u8>>>) -> io::Result<Vec<u8>> {
    match reader.join() {
        Ok(result) => result,
        Err(_) => Err(io::Error::new(
            io::ErrorKind::Other,
            "subprocess output reader panicked",
        )),
    }
}

fn reap_child(child: &mut Child) {
    terminate_process_group(child.id());
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(unix)]
fn isolate_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt as _;

    command.process_group(0);
}

#[cfg(not(unix))]
fn isolate_process_group(_command: &mut Command) {}

#[cfg(unix)]
fn terminate_process_group(pid: u32) {
    if let Some(process_group) = rustix::process::Pid::from_raw(pid as i32) {
        let _ = rustix::process::kill_process_group(process_group, rustix::process::Signal::Kill);
    }
}

#[cfg(not(unix))]
fn terminate_process_group(_pid: u32) {}

/// Like [`output_with_timeout`] but discards output and returns only
/// the exit status (stdout/stderr go to `/dev/null`). Blocking.
///
/// # Errors
/// Spawn failure, a wait error, or the timeout (after which the invocation's
/// process group is killed and the direct child is reaped).
pub fn status_with_timeout(mut cmd: Command, timeout: Duration) -> std::io::Result<ExitStatus> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    isolate_process_group(&mut cmd);
    let mut child = cmd.spawn()?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                terminate_process_group(child.id());
                return Ok(status);
            }
            Ok(None) => {}
            Err(error) => {
                reap_child(&mut child);
                return Err(error);
            }
        }
        if Instant::now() >= deadline {
            reap_child(&mut child);
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
/// Spawn failure, a wait error, or the timeout (after which the invocation's
/// process group is sent a kill signal).
pub async fn status_with_timeout_async(
    mut cmd: tokio::process::Command,
    timeout: Duration,
) -> std::io::Result<ExitStatus> {
    cmd.kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = cmd.spawn()?;
    let pid = child.id();
    match tokio::time::timeout(timeout, child.wait()).await {
        Ok(result) => {
            if let Some(pid) = pid {
                terminate_process_group(pid);
            }
            result
        }
        Err(_) => {
            if let Some(pid) = pid {
                terminate_process_group(pid);
            }
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

    #[cfg(unix)]
    #[test]
    fn output_with_timeout_drains_stdout_and_stderr_concurrently() {
        let bytes = MAX_CAPTURE_BYTES_PER_STREAM * 4;
        let script = format!("head -c {bytes} /dev/zero & head -c {bytes} /dev/zero >&2 & wait");
        let mut cmd = Command::new("sh");
        cmd.args(["-c", &script]);

        let out = output_with_timeout(cmd, Duration::from_secs(5))
            .expect("both noisy streams should drain without deadlock");
        assert!(out.status.success());
        assert_eq!(out.stdout.len(), MAX_CAPTURE_BYTES_PER_STREAM);
        assert_eq!(out.stderr.len(), MAX_CAPTURE_BYTES_PER_STREAM);
    }

    #[cfg(unix)]
    #[test]
    fn output_with_timeout_truncates_capture_after_draining_excess() {
        let bytes = MAX_CAPTURE_BYTES_PER_STREAM * 4;
        let script = format!("head -c {bytes} /dev/zero; head -c {bytes} /dev/zero >&2");
        let mut cmd = Command::new("sh");
        cmd.args(["-c", &script]);

        let out = output_with_timeout(cmd, Duration::from_secs(5)).expect("bounded output runs");
        assert!(out.status.success());
        assert_eq!(out.stdout.len(), MAX_CAPTURE_BYTES_PER_STREAM);
        assert_eq!(out.stderr.len(), MAX_CAPTURE_BYTES_PER_STREAM);
        assert!(out.stdout.iter().all(|byte| *byte == 0));
        assert!(out.stderr.iter().all(|byte| *byte == 0));
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

    #[cfg(unix)]
    #[test]
    fn output_with_timeout_times_out_while_draining_a_noisy_child() {
        let cmd = Command::new("yes");
        let start = Instant::now();
        let r = output_with_timeout(cmd, Duration::from_millis(150));
        assert_eq!(
            r.expect_err("an infinite noisy child must time out").kind(),
            std::io::ErrorKind::TimedOut
        );
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "timeout must remain bounded while draining output"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn hostile_helper_descendant_cannot_outlive_the_bounded_process_invocation() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "sleep 60 & echo $!"]);
        let start = Instant::now();
        let output = output_with_timeout(cmd, Duration::from_secs(2))
            .expect("the direct helper exits successfully");
        assert!(output.status.success());
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "an inherited output pipe must not pin the worker"
        );

        let descendant_pid = String::from_utf8(output.stdout)
            .expect("helper pid is utf-8")
            .trim()
            .parse::<u32>()
            .expect("helper reports a descendant pid");
        let proc_entry = std::path::PathBuf::from(format!("/proc/{descendant_pid}"));
        let still_executing = std::fs::read_to_string(proc_entry.join("stat"))
            .ok()
            .and_then(|stat| stat.rsplit_once(") ").map(|(_, fields)| fields.to_owned()))
            .and_then(|fields| fields.chars().next())
            .is_some_and(|state| state != 'Z');
        assert!(
            !still_executing,
            "the descendant process must not remain executable"
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
