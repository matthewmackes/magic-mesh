//! Lockfile single-instance guard (AUD-8 / §2).
//!
//! The workbench used to claim a private D-Bus well-known name
//! (`dev.mackes.MDE.Workbench`) for single-instance detection — a §2
//! violation (only FDO `org.freedesktop.*` interop is allowed on D-Bus; new
//! MDE-private bus names are not). This replaces it with a dep-free pidfile:
//! `$XDG_RUNTIME_DIR/mde-workbench.lock` holds the live primary's PID. A second
//! launch reads it, confirms the holder is a live `mde-workbench` via
//! `/proc/<pid>/comm`, and hands its `--focus` slug off over the **Bus** (the
//! focus path was already Bus-native — only the name claim was D-Bus).
//!
//! The decision logic is split from the I/O so it can be unit-tested.

use std::io::Write;
use std::path::PathBuf;

/// Result of the single-instance check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrimaryStatus {
    /// This process is the primary. Open the workbench window normally.
    Primary,
    /// A sibling is already running. Hand the `--focus` slug off over the
    /// Bus and exit.
    Existing,
}

/// The pidfile path: `$XDG_RUNTIME_DIR/mde-workbench.lock`, falling back to
/// `/tmp` when the runtime dir is unset (early-boot / recovery shells).
#[must_use]
pub fn lock_path() -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("mde-workbench.lock")
}

/// Pure decision: given the PID recorded in the lockfile (if any) and a
/// liveness probe, decide whether we are primary. A live sibling means
/// `Existing`; a missing/stale holder means we become `Primary`.
#[must_use]
pub fn decide(holder_pid: Option<u32>, is_live_sibling: impl Fn(u32) -> bool) -> PrimaryStatus {
    match holder_pid {
        Some(pid) if is_live_sibling(pid) => PrimaryStatus::Existing,
        _ => PrimaryStatus::Primary,
    }
}

/// `true` if `pid` is a live process whose comm is `mde-workbench` (guards
/// against PID reuse — a recycled PID owned by some other program must not
/// read as "the workbench is already running"). Linux `/proc`, dep-free.
#[must_use]
pub fn live_workbench(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .map(|comm| comm.trim() == "mde-workbench")
        .unwrap_or(false)
}

/// Acquire the single-instance pidfile. Returns the status plus, when
/// `Primary`, the open lockfile handle — the caller keeps it alive for the
/// process lifetime so the recorded PID stays current. An I/O failure
/// degrades to `Primary` with no handle (run without protection rather than
/// refuse to start).
#[must_use]
pub fn acquire() -> (PrimaryStatus, Option<std::fs::File>) {
    let path = lock_path();
    let holder = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok());
    if decide(holder, live_workbench) == PrimaryStatus::Existing {
        return (PrimaryStatus::Existing, None);
    }
    // Become primary: (re)write our PID into the lockfile.
    match std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)
    {
        Ok(mut f) => {
            let _ = write!(f, "{}", std::process::id());
            let _ = f.flush();
            (PrimaryStatus::Primary, Some(f))
        }
        Err(_) => (PrimaryStatus::Primary, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_sibling_means_existing() {
        assert_eq!(decide(Some(1234), |_| true), PrimaryStatus::Existing);
    }

    #[test]
    fn stale_holder_means_primary() {
        // PID recorded but not a live workbench (crashed / reused) → us.
        assert_eq!(decide(Some(1234), |_| false), PrimaryStatus::Primary);
    }

    #[test]
    fn no_holder_means_primary() {
        assert_eq!(decide(None, |_| true), PrimaryStatus::Primary);
    }

    #[test]
    fn lock_path_lives_under_runtime_dir() {
        // Whatever the base, the file name is stable.
        assert_eq!(
            lock_path().file_name().unwrap().to_str().unwrap(),
            "mde-workbench.lock"
        );
    }

    #[test]
    fn live_workbench_false_for_impossible_pid() {
        // PID 0 has no /proc/0/comm → not a live workbench.
        assert!(!live_workbench(0));
    }
}
