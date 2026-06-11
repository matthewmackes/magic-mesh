//! NF-21.1 — `sshd_overlay_bind` worker.
//!
//! Owns the `/etc/ssh/sshd_config.d/mackes-mesh.conf` drop-in that
//! binds the SSH daemon to this peer's Nebula overlay IP. Replaces
//! the Python helper `mesh_nebula.py::write_sshd_overlay_bind` so
//! the legacy module can fully retire (see DEAD-2.14 + the
//! retirement plan comment block at the top of that module).
//!
//! Lifecycle:
//!
//! 1. **Watch** `/var/lib/mackesd/nebula/overlay-ip` (the file
//!    `nebula_supervisor` publishes via `publish_overlay_ip` after
//!    every CA bundle change). Polls the mtime every
//!    [`DEFAULT_TICK_INTERVAL`] — overlay-IP changes are rare (only
//!    on re-enrollment under a new CA epoch), so a 5 s tick is
//!    plenty.
//! 2. **Write** the drop-in atomically when the mtime advances
//!    *and* the published IP differs from the one currently in the
//!    drop-in. Idempotent on no-op changes.
//! 3. **Reload sshd** via `systemctl reload sshd` so the daemon
//!    binds to the new overlay address.
//!
//! Per the open-mesh / flat-trust directive (`project_open_mesh_directive`),
//! the drop-in carries `ListenAddress <overlay>` and a comment;
//! it does NOT add per-service ACLs.
//!
//! On peers where the overlay-IP file doesn't exist (pre-enrollment
//! or fresh dev box) the worker is a quiet no-op — every tick
//! observes a missing file and skips. Once `nebula_supervisor`
//! publishes the first overlay IP, the next tick picks it up.

#![cfg(feature = "async-services")]

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::{ShutdownToken, Worker};

/// Default sweep cadence. Overlay-IP changes are infrequent +
/// driven by CA epoch rotation, so a 5 s tick keeps the worker
/// cheap.
pub const DEFAULT_TICK_INTERVAL: Duration = Duration::from_secs(5);

/// Default path of the overlay-IP publish file written by
/// `nebula_supervisor::publish_overlay_ip` (`GF-1.3.a`).
pub const DEFAULT_OVERLAY_IP_PATH: &str = "/var/lib/mackesd/nebula/overlay-ip";

/// Default drop-in path. The systemd-side `sshd.service` reads
/// `/etc/ssh/sshd_config.d/*.conf` on every reload; this file is
/// the only mackes-owned drop-in.
pub const DEFAULT_DROPIN_PATH: &str = "/etc/ssh/sshd_config.d/mackes-mesh.conf";

/// Default systemd unit reloaded on overlay-IP change.
pub const DEFAULT_SSHD_UNIT: &str = "sshd.service";

/// Worker handle. Tracks the last-observed mtime + last-written
/// overlay IP so reloads only fire on real changes.
pub struct SshdOverlayBindWorker {
    overlay_ip_path: PathBuf,
    dropin_path: PathBuf,
    tick_interval: Duration,
    sshd_unit: &'static str,
    last_overlay_mtime: Option<SystemTime>,
    last_written_ip: Option<String>,
}

impl Default for SshdOverlayBindWorker {
    fn default() -> Self {
        Self::new()
    }
}

impl SshdOverlayBindWorker {
    /// Construct a worker pinned to the default overlay-IP +
    /// drop-in paths + the `sshd.service` unit.
    #[must_use]
    pub fn new() -> Self {
        Self {
            overlay_ip_path: PathBuf::from(DEFAULT_OVERLAY_IP_PATH),
            dropin_path: PathBuf::from(DEFAULT_DROPIN_PATH),
            tick_interval: DEFAULT_TICK_INTERVAL,
            sshd_unit: DEFAULT_SSHD_UNIT,
            last_overlay_mtime: None,
            last_written_ip: None,
        }
    }

    /// Override the overlay-IP publish path — used by tests.
    #[must_use]
    pub fn with_overlay_ip_path(mut self, path: PathBuf) -> Self {
        self.overlay_ip_path = path;
        self
    }

    /// Override the drop-in path — used by tests that can't write
    /// under `/etc`.
    #[must_use]
    pub fn with_dropin_path(mut self, path: PathBuf) -> Self {
        self.dropin_path = path;
        self
    }

    /// Override the tick interval — used by tests that need a
    /// faster pulse.
    #[must_use]
    pub fn with_tick_interval(mut self, interval: Duration) -> Self {
        self.tick_interval = interval;
        self
    }

    /// Override the sshd unit — empty string disables the reload
    /// shell-out so tests don't touch the live systemctl.
    #[must_use]
    pub fn with_sshd_unit(mut self, unit: &'static str) -> Self {
        self.sshd_unit = unit;
        self
    }

    /// One tick of the worker's loop. Public so tests can drive
    /// it deterministically without the tokio-time pulse.
    ///
    /// Returns the outcome. Errors writing the drop-in or
    /// reloading sshd are logged + swallowed — the supervisor's
    /// restart policy handles hard failures.
    pub fn tick_once(&mut self) -> TickOutcome {
        // Missing publish file = pre-enrollment; quiet no-op.
        if !self.overlay_ip_path.exists() {
            return TickOutcome::NoOverlayYet;
        }
        // Look for a forward mtime jump vs the last-seen one.
        match std::fs::metadata(&self.overlay_ip_path).and_then(|m| m.modified()) {
            Ok(now) => {
                let advanced = self.last_overlay_mtime.is_none_or(|last| now > last);
                self.last_overlay_mtime = Some(now);
                if !advanced {
                    return TickOutcome::Idle;
                }
            }
            Err(e) => {
                tracing::warn!(
                    target: "mackesd::sshd_overlay_bind",
                    error = %e,
                    path = %self.overlay_ip_path.display(),
                    "stat() of overlay-ip publish file failed"
                );
                return TickOutcome::Idle;
            }
        }
        // mtime advanced — read the IP + write the drop-in if
        // changed.
        let overlay_ip = match std::fs::read_to_string(&self.overlay_ip_path) {
            Ok(s) => s.trim().to_string(),
            Err(e) => {
                tracing::warn!(
                    target: "mackesd::sshd_overlay_bind",
                    error = %e,
                    path = %self.overlay_ip_path.display(),
                    "read() of overlay-ip publish file failed"
                );
                return TickOutcome::Idle;
            }
        };
        if overlay_ip.is_empty() {
            tracing::warn!(
                target: "mackesd::sshd_overlay_bind",
                "overlay-ip publish file empty; deferring"
            );
            return TickOutcome::Idle;
        }
        if self.last_written_ip.as_deref() == Some(overlay_ip.as_str()) {
            return TickOutcome::Idle;
        }
        let body = render_dropin_body(&overlay_ip);
        if let Err(e) = write_dropin_atomic(&self.dropin_path, &body) {
            tracing::warn!(
                target: "mackesd::sshd_overlay_bind",
                error = %e,
                path = %self.dropin_path.display(),
                "failed to write sshd drop-in"
            );
            return TickOutcome::Idle;
        }
        self.last_written_ip = Some(overlay_ip.clone());
        tracing::info!(
            target: "mackesd::sshd_overlay_bind",
            overlay_ip = %overlay_ip,
            path = %self.dropin_path.display(),
            "wrote sshd drop-in binding to nebula overlay"
        );
        if !self.sshd_unit.is_empty() {
            match reload_sshd(self.sshd_unit) {
                Ok(()) => tracing::info!(
                    target: "mackesd::sshd_overlay_bind",
                    unit = %self.sshd_unit,
                    "systemctl reload triggered"
                ),
                Err(e) => tracing::warn!(
                    target: "mackesd::sshd_overlay_bind",
                    unit = %self.sshd_unit,
                    error = %e,
                    "systemctl reload failed"
                ),
            }
        }
        TickOutcome::Wrote
    }
}

/// Per-tick result. Exposed for tests that want to assert the
/// write edge condition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TickOutcome {
    /// The overlay-ip publish file doesn't exist yet (pre-enrollment).
    NoOverlayYet,
    /// Publish file is unchanged or empty; nothing to do.
    Idle,
    /// Drop-in was (re)written + reload signalled.
    Wrote,
}

#[async_trait::async_trait]
impl Worker for SshdOverlayBindWorker {
    fn name(&self) -> &'static str {
        "sshd_overlay_bind"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        loop {
            let _ = self.tick_once();
            tokio::select! {
                _ = shutdown.wait() => break,
                _ = tokio::time::sleep(self.tick_interval) => {},
            }
        }
        Ok(())
    }
}

/// Pure helper — render the drop-in body for a given overlay IP.
/// Atomic-write-friendly: no leading whitespace, trailing newline.
#[must_use]
pub fn render_dropin_body(overlay_ip: &str) -> String {
    // **SSH ALWAYS keeps public access** (operator directive 2026-06-10).
    // ADDITIVE bind: listen on ALL interfaces (0.0.0.0 + ::) so sshd stays
    // reachable over BOTH the overlay ({overlay_ip}) AND the
    // underlay/public address — the public listener is never dropped.
    // Binding
    // `ListenAddress <overlay>` ONLY made sshd drop its all-interfaces
    // default and listen solely on the overlay — which locked admins
    // out of cloud nodes managed over the public IP (bed finding). A
    // specific overlay `ListenAddress` is intentionally NOT emitted: it
    // is already covered by 0.0.0.0, and listing both makes sshd fail to
    // bind the duplicate port. To re-add overlay-only hardening later,
    // gate it behind an explicit opt-in that keeps a break-glass listener.
    format!(
        "# Generated by mackesd::workers::sshd_overlay_bind (NF-21.1)\n\
         # Do not edit by hand — the worker rewrites this on every\n\
         # overlay-IP change. Replaces mesh_nebula.py::write_sshd_overlay_bind.\n\
         # Overlay IP (reachable via 0.0.0.0 below): {overlay_ip}\n\
         ListenAddress 0.0.0.0\n\
         ListenAddress ::\n\
         # Per the open-mesh directive: every enrolled peer reaches\n\
         # every other on the overlay; no per-service ACLs here.\n"
    )
}

/// Pure helper — write the drop-in via temp + rename so a sshd
/// reload mid-write doesn't see a half-formed config.
fn write_dropin_atomic(path: &Path, body: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("conf.tmp");
    std::fs::write(&tmp, body.as_bytes())?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Reload sshd via `systemctl reload <unit>`. `reload` (not
/// `try-reload-or-restart`) matches the Python helper's behavior +
/// keeps existing sessions alive during the rebind.
fn reload_sshd(unit: &str) -> Result<(), String> {
    let out = std::process::Command::new("systemctl")
        .args(["reload", unit])
        .output()
        .map_err(|e| format!("systemctl reload {unit}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_name_is_sshd_overlay_bind() {
        let w = SshdOverlayBindWorker::new();
        assert_eq!(w.name(), "sshd_overlay_bind");
    }

    #[test]
    fn render_body_always_keeps_public_access() {
        // SSH must ALWAYS keep public access (operator directive): the
        // drop-in listens on all interfaces (which includes the overlay)
        // and never emits an overlay-ONLY ListenAddress (that locked out
        // cloud nodes managed over the underlay).
        let body = render_dropin_body("10.42.0.5");
        assert!(body.contains("ListenAddress 0.0.0.0"));
        assert!(
            !body.contains("ListenAddress 10.42.0.5"),
            "must not bind overlay-ONLY (would drop the public listener):\n{body}"
        );
        // The overlay IP is still documented (in a comment).
        assert!(body.contains("10.42.0.5"));
        assert!(body.contains("NF-21.1"));
        assert!(body.ends_with('\n'));
    }

    #[test]
    fn missing_publish_file_yields_no_overlay_yet() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut w = SshdOverlayBindWorker::new()
            .with_overlay_ip_path(tmp.path().join("overlay-ip"))
            .with_dropin_path(tmp.path().join("mackes-mesh.conf"))
            .with_sshd_unit("");
        assert_eq!(w.tick_once(), TickOutcome::NoOverlayYet);
    }

    #[test]
    fn first_tick_with_overlay_writes_dropin() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let overlay = tmp.path().join("overlay-ip");
        let dropin = tmp.path().join("mackes-mesh.conf");
        std::fs::write(&overlay, "10.42.0.5\n").expect("seed overlay");
        let mut w = SshdOverlayBindWorker::new()
            .with_overlay_ip_path(overlay)
            .with_dropin_path(dropin.clone())
            .with_sshd_unit(""); // skip systemctl
        assert_eq!(w.tick_once(), TickOutcome::Wrote);
        let body = std::fs::read_to_string(&dropin).expect("dropin written");
        // Additive bind: public listener kept (all-interfaces), overlay
        // documented — never overlay-only.
        assert!(body.contains("ListenAddress 0.0.0.0"));
        assert!(!body.contains("ListenAddress 10.42.0.5"));
    }

    #[test]
    fn idempotent_when_overlay_unchanged() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let overlay = tmp.path().join("overlay-ip");
        let dropin = tmp.path().join("mackes-mesh.conf");
        std::fs::write(&overlay, "10.42.0.7\n").expect("seed overlay");
        let mut w = SshdOverlayBindWorker::new()
            .with_overlay_ip_path(overlay)
            .with_dropin_path(dropin)
            .with_sshd_unit("");
        assert_eq!(w.tick_once(), TickOutcome::Wrote);
        // Second tick: mtime unchanged → Idle. No reload.
        assert_eq!(w.tick_once(), TickOutcome::Idle);
    }

    #[test]
    fn second_tick_after_overlay_change_rewrites() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let overlay = tmp.path().join("overlay-ip");
        let dropin = tmp.path().join("mackes-mesh.conf");
        std::fs::write(&overlay, "10.42.0.5\n").expect("seed overlay");
        let mut w = SshdOverlayBindWorker::new()
            .with_overlay_ip_path(overlay.clone())
            .with_dropin_path(dropin.clone())
            .with_sshd_unit("");
        assert_eq!(w.tick_once(), TickOutcome::Wrote);
        // mtime needs to advance for the second write to fire;
        // sleep briefly then rewrite the publish file.
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(&overlay, "10.42.0.9\n").expect("rotate overlay");
        assert_eq!(w.tick_once(), TickOutcome::Wrote);
        let body = std::fs::read_to_string(&dropin).expect("dropin written");
        // The rewrite documents the NEW overlay IP (in the comment) and
        // keeps the all-interfaces public listener; the old IP is gone.
        assert!(body.contains("ListenAddress 0.0.0.0"));
        assert!(body.contains("10.42.0.9"));
        assert!(!body.contains("10.42.0.5"));
    }

    #[test]
    fn empty_overlay_publish_file_defers() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let overlay = tmp.path().join("overlay-ip");
        std::fs::write(&overlay, "").expect("seed empty overlay");
        let mut w = SshdOverlayBindWorker::new()
            .with_overlay_ip_path(overlay)
            .with_dropin_path(tmp.path().join("mackes-mesh.conf"))
            .with_sshd_unit("");
        assert_eq!(w.tick_once(), TickOutcome::Idle);
    }
}
