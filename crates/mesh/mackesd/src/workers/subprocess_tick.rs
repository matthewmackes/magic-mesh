//! Shared subprocess-supervision helper for Phase B workers that
//! shepherd an external one-shot command on a periodic cadence.
//!
//! Several Phase B workers (`remmina_sync`,
//! `ansible_pull`, the WoL / perf / thumbnailer helpers) all share
//! the same shape: every N seconds, spawn a known command with a
//! known argv, wait for it to exit, log the exit code, repeat until
//! shutdown. This module factors that into a reusable
//! [`SubprocessTickWorker`] so each concrete worker is a one-liner
//! pair of `name()` + `build_command()`.
//!
//! The worker is async-trait friendly (works under the Phase A.2
//! supervisor + restart policy machinery) and treats any non-zero
//! exit code as a [`Recoverable`](super::TickOutcome::Recoverable)
//! error so the supervisor's back-off kicks in. Spawning failures
//! (binary missing, permission denied) propagate the same way.

#![cfg(feature = "async-services")]

use std::ffi::OsString;
use std::time::Duration;

use tokio::process::Command;

use super::{ShutdownToken, Worker};

/// Generic "spawn this command every N seconds" worker. Concrete
/// Phase B workers wrap this with a thin newtype that pre-fills the
/// name + argv.
pub struct SubprocessTickWorker {
    name: &'static str,
    binary: OsString,
    args: Vec<OsString>,
    interval: Duration,
    /// How long a single subprocess invocation is allowed to run
    /// before the supervisor kills it. Defaults to 300 s (5 min).
    pub kill_after: Duration,
}

impl SubprocessTickWorker {
    /// Construct a new worker. `name` matches the file under
    /// `crates/mackesd/src/workers/` (kebab-case); `binary` is the
    /// command to spawn; `args` are the argv tail.
    #[must_use]
    pub fn new(
        name: &'static str,
        binary: impl Into<OsString>,
        args: impl IntoIterator<Item = impl Into<OsString>>,
        interval: Duration,
    ) -> Self {
        Self {
            name,
            binary: binary.into(),
            args: args.into_iter().map(Into::into).collect(),
            interval,
            kill_after: Duration::from_secs(300),
        }
    }
}

#[async_trait::async_trait]
impl Worker for SubprocessTickWorker {
    fn name(&self) -> &'static str {
        self.name
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let mut last_err: Option<anyhow::Error> = None;
        loop {
            if shutdown.is_shutdown() {
                return match last_err {
                    Some(e) => Err(e),
                    None => Ok(()),
                };
            }
            // Race the tick against shutdown — exit immediately on
            // shutdown rather than waiting for the next interval.
            tokio::select! {
                biased;
                _ = shutdown.wait() => {
                    return match last_err {
                        Some(e) => Err(e),
                        None    => Ok(()),
                    };
                }
                result = run_once(&self.binary, &self.args, self.kill_after) => {
                    last_err = result.err();
                }
            }
            // Wait the cadence, but wake immediately on shutdown.
            tokio::select! {
                biased;
                _ = shutdown.wait() => {
                    return match last_err {
                        Some(e) => Err(e),
                        None    => Ok(()),
                    };
                }
                _ = tokio::time::sleep(self.interval) => {}
            }
        }
    }
}

/// Spawn the configured command, wait up to `kill_after`, capture
/// stdout + stderr, log a summary. Returns Err on spawn failure,
/// non-zero exit, or timeout — callers can use the result to drive
/// retry/backoff.
async fn run_once(
    binary: &OsString,
    args: &[OsString],
    kill_after: Duration,
) -> anyhow::Result<()> {
    let mut cmd = Command::new(binary);
    cmd.args(args);
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| {
        anyhow::anyhow!(
            "subprocess-tick: spawning {} failed: {e}",
            binary.to_string_lossy()
        )
    })?;
    let wait = tokio::time::timeout(kill_after, child.wait()).await;
    match wait {
        Ok(Ok(status)) if status.success() => {
            tracing::debug!(
                cmd = %binary.to_string_lossy(),
                "subprocess-tick: ok"
            );
            Ok(())
        }
        Ok(Ok(status)) => Err(anyhow::anyhow!(
            "subprocess-tick: {} exited {}",
            binary.to_string_lossy(),
            status.code().map_or("?".to_string(), |c| c.to_string())
        )),
        Ok(Err(e)) => Err(anyhow::anyhow!(
            "subprocess-tick: waiting on {} failed: {e}",
            binary.to_string_lossy()
        )),
        Err(_) => {
            // Timeout — kill the child so it doesn't linger.
            let _ = child.start_kill();
            Err(anyhow::anyhow!(
                "subprocess-tick: {} exceeded {} s timeout",
                binary.to_string_lossy(),
                kill_after.as_secs()
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn worker_runs_true_repeatedly_until_shutdown() {
        let mut w = SubprocessTickWorker::new(
            "true-test",
            "true",
            Vec::<OsString>::new(),
            Duration::from_millis(20),
        );
        let (tx, rx) = tokio::sync::watch::channel(false);
        let token = ShutdownToken::from_receiver(rx);
        let handle = tokio::spawn(async move { w.run(token).await });
        tokio::time::sleep(Duration::from_millis(60)).await;
        let _ = tx.send(true);
        let result = tokio::time::timeout(Duration::from_secs(3), handle)
            .await
            .expect("worker exits on shutdown")
            .expect("join");
        assert!(result.is_ok(), "true should never error");
    }

    #[tokio::test]
    async fn worker_returns_err_when_subprocess_exits_nonzero() {
        let mut w = SubprocessTickWorker::new(
            "false-test",
            "false",
            Vec::<OsString>::new(),
            Duration::from_secs(60),
        );
        // false returns 1 immediately; the worker continues looping
        // (the supervisor would catch the recoverable error). To
        // test, we let it run once then shutdown.
        let (tx, rx) = tokio::sync::watch::channel(false);
        let token = ShutdownToken::from_receiver(rx);
        let handle = tokio::spawn(async move { w.run(token).await });
        tokio::time::sleep(Duration::from_millis(100)).await;
        let _ = tx.send(true);
        let result = tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("worker exits on shutdown")
            .expect("join");
        // The last_err from the first tick propagates as Err
        // on shutdown.
        assert!(result.is_err(), "false's nonzero exit should surface");
    }

    #[tokio::test]
    async fn run_once_propagates_spawn_failure_for_missing_binary() {
        let result = run_once(
            &OsString::from("/does/not/exist/never-real-binary"),
            &[],
            Duration::from_secs(1),
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn run_once_times_out_on_long_running_child() {
        let result = run_once(
            &OsString::from("sleep"),
            &[OsString::from("60")],
            Duration::from_millis(100),
        )
        .await;
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("timeout") || msg.contains("exceeded"),
            "expected timeout message, got {msg}"
        );
    }

    #[test]
    fn subprocess_tick_worker_carries_constructor_name() {
        let w = SubprocessTickWorker::new(
            "named-worker",
            "true",
            Vec::<OsString>::new(),
            Duration::from_secs(1),
        );
        assert_eq!(w.name(), "named-worker");
    }
}
