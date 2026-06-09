//! v2.0.0 Phase B.6 — ansible-pull worker.
//!
//! Supervises the external `ansible-pull` binary on a 15-minute
//! cadence (matching the legacy `mackes-ansible-pull.timer`).
//! Replaces `mackes-ansible-pull.service` + `mackes-ansible-pull.timer`
//! + `mackes/fleet.py`'s subprocess scheduling. The actual playbook
//! / inventory URL surfaces are operator-provided via env vars + the
//! fleet config layer (Phase G); this worker just owns the cadence
//! + supervision.

#![cfg(feature = "async-services")]

use std::ffi::OsString;
use std::time::Duration;

use super::subprocess_tick::SubprocessTickWorker;

/// Cadence locked at 900 s (15 min) per the legacy
/// `mackes-ansible-pull.timer` `OnUnitActiveSec=15min` setting.
pub const TICK_INTERVAL_S: u64 = 900;

/// Default URL the legacy `mackes-ansible-pull` invocation pulled
/// from. Override via the `MDE_ANSIBLE_PULL_URL` env var (the
/// supervisor's argv passes it through to the binary).
pub const DEFAULT_PULL_URL_ENV: &str = "MDE_ANSIBLE_PULL_URL";

/// Construct the supervisor-ready worker. argv:
///   `ansible-pull -U <url> -i localhost,`
///
/// When `MDE_ANSIBLE_PULL_URL` is unset the worker still runs but
/// `ansible-pull` will fail-fast with an empty URL — the supervisor
/// catches the non-zero exit, backs off, and the operator sees the
/// error in `journalctl -t mded`.
#[must_use]
pub fn build() -> SubprocessTickWorker {
    let url = std::env::var(DEFAULT_PULL_URL_ENV).unwrap_or_default();
    SubprocessTickWorker::new(
        "ansible-pull",
        "ansible-pull",
        vec![
            OsString::from("-U"),
            OsString::from(url),
            OsString::from("-i"),
            OsString::from("localhost,"),
        ],
        Duration::from_secs(TICK_INTERVAL_S),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workers::Worker;

    #[test]
    fn ansible_pull_worker_name_matches_phase_b_lock() {
        let w = build();
        assert_eq!(w.name(), "ansible-pull");
    }

    #[test]
    fn tick_interval_matches_legacy_timer() {
        assert_eq!(TICK_INTERVAL_S, 900);
    }

    #[test]
    fn default_pull_url_env_var_uses_mde_prefix() {
        // Phase 0.6 — every MDE env var uses MDE_ prefix.
        assert!(DEFAULT_PULL_URL_ENV.starts_with("MDE_"));
    }
}
