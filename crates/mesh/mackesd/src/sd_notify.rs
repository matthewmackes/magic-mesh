//! Minimal `sd_notify` — systemd readiness + watchdog, with no `libsystemd`
//! dependency (BULLETPROOF-2). mackesd runs as a `Type=notify` system service:
//! it sends `READY=1` once the supervisor + responders are up, then pings
//! `WATCHDOG=1` from the main watch loop. If the runtime ever wedges (deadlock,
//! a stuck main loop), the pings stop and systemd restarts the daemon — a hung
//! daemon (process alive but not working) is otherwise invisible to systemd.
//!
//! Protocol: write a newline/`=`-delimited state string to the `AF_UNIX`
//! datagram socket named by `$NOTIFY_SOCKET`. System services get a
//! filesystem-path socket (`/run/systemd/notify`); abstract-namespace sockets
//! (leading `@`) aren't sendable from the stable std datagram API, so we
//! degrade gracefully there (the watchdog simply doesn't arm) rather than pull
//! a dep. Every call is best-effort and never panics.

use std::os::unix::net::UnixDatagram;
use std::time::Duration;

/// Send a service-state line to `$NOTIFY_SOCKET`. `Ok(false)` when not running
/// under systemd notify (socket unset/empty/abstract); `Ok(true)` when sent.
///
/// # Errors
/// Propagates the socket open/send error (caller logs best-effort).
pub fn notify(state: &str) -> std::io::Result<bool> {
    let Ok(path) = std::env::var("NOTIFY_SOCKET") else {
        return Ok(false);
    };
    // Only the filesystem-path form is sendable from stable std; system
    // services use /run/systemd/notify, so this covers mackesd. Abstract
    // ('@'-prefixed) degrades to a no-op.
    if path.is_empty() || !path.starts_with('/') {
        return Ok(false);
    }
    let sock = UnixDatagram::unbound()?;
    sock.send_to(state.as_bytes(), &path)?;
    Ok(true)
}

/// `READY=1` — the daemon has finished startup (Type=notify gate).
///
/// # Errors
/// Per [`notify`].
pub fn notify_ready() -> std::io::Result<bool> {
    notify("READY=1")
}

/// `WATCHDOG=1` — keep-alive ping for `WatchdogSec`.
///
/// # Errors
/// Per [`notify`].
pub fn notify_watchdog() -> std::io::Result<bool> {
    notify("WATCHDOG=1")
}

/// The interval at which to ping the watchdog — half of `WATCHDOG_USEC`
/// (systemd's recommendation), but only when the watchdog is armed for *this*
/// process. `None` means no watchdog is configured, so the caller skips pinging.
#[must_use]
pub fn watchdog_interval() -> Option<Duration> {
    let pid_ok = match std::env::var("WATCHDOG_PID") {
        Ok(p) => p.parse::<u32>().ok() == Some(std::process::id()),
        // Unset => the watchdog (if any) applies to us.
        Err(_) => true,
    };
    let usec = std::env::var("WATCHDOG_USEC").ok()?.parse::<u64>().ok()?;
    watchdog_interval_from(usec, pid_ok)
}

/// Pure core of [`watchdog_interval`] — testable without env mutation.
#[must_use]
fn watchdog_interval_from(watchdog_usec: u64, pid_matches: bool) -> Option<Duration> {
    if !pid_matches || watchdog_usec == 0 {
        return None;
    }
    Some(Duration::from_micros(watchdog_usec / 2))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn no_socket_is_a_clean_noop() {
        // With NOTIFY_SOCKET unset, notify() reports "not under systemd".
        // (Other tests may set it; guard by removing first.)
        std::env::remove_var("NOTIFY_SOCKET");
        assert_eq!(notify("READY=1").unwrap(), false);
    }

    #[test]
    fn abstract_socket_degrades_to_noop() {
        std::env::set_var("NOTIFY_SOCKET", "@abstract-thing");
        assert_eq!(notify("READY=1").unwrap(), false);
        std::env::remove_var("NOTIFY_SOCKET");
    }

    #[test]
    fn sends_to_a_real_path_socket() {
        // Bind a datagram listener, point NOTIFY_SOCKET at it, send, recv.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notify.sock");
        let listener = UnixDatagram::bind(&path).unwrap();
        std::env::set_var("NOTIFY_SOCKET", &path);
        assert_eq!(notify("READY=1").unwrap(), true);
        let mut buf = [0u8; 64];
        listener
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let n = listener.recv(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"READY=1");
        std::env::remove_var("NOTIFY_SOCKET");
    }

    #[test]
    fn watchdog_interval_is_half_of_usec_when_armed() {
        assert_eq!(
            watchdog_interval_from(60_000_000, true),
            Some(Duration::from_micros(30_000_000))
        );
    }

    #[test]
    fn watchdog_disarmed_when_pid_mismatch_or_zero() {
        assert_eq!(watchdog_interval_from(60_000_000, false), None);
        assert_eq!(watchdog_interval_from(0, true), None);
    }
}
