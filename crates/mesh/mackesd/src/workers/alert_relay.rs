//! MON-4 (v2.6) — alert relay worker.
//!
//! Watches `~/.local/share/mde/alerts/` for `*.json` event
//! files written by `mde-alert-emit` (MON-3) + the
//! upgrade-state transitions (OBS-7), and forwards each as an
//! FDO desktop notification. Primary delivery (OBS-8) is a
//! publish on the `fdo/*` Bus topic so the **cosmic-applet**
//! renders it through the FDO Notifications path; a direct
//! `notify-send` is the headless fallback. The notification
//! surfaces the alert's severity + summary + a deep-link to
//! the chart URL when present.
//!
//! Polling vs inotify: this worker polls every
//! `DEFAULT_TICK_INTERVAL` (2s) rather than using inotify
//! because (a) the existing `notification_relay` worker
//! already uses the same pattern with a documented rationale
//! (inotify-on-FUSE is flaky), (b) alerts are infrequent so
//! the 2s ceiling is operator-imperceptible, (c) tracking
//! seen-GFIDs via a `BTreeSet` mirrors the existing
//! `gluster_worker::healed_gfids` de-dupe shape.
//!
//! Once a file has been delivered, its ULID lands in the in-memory
//! `seen_alert_ids` set and a durable receipt beside the retained history. That
//! keeps repeat invocations and daemon restarts idempotent. Delivery failures
//! are not acknowledged, so the next sweep retries them.
//!
//! Best-effort: if `notify-send` isn't installed (operator
//! running headless), the worker logs at debug + continues
//! polling. The alert files stay on disk for future
//! consumers (MON-5 Workbench Mesh Health panel,
//! future audit tools).

#![cfg(feature = "async-services")]

use std::collections::BTreeSet;
use std::fs::OpenOptions;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

use super::{ShutdownToken, Worker};

/// Default sweep cadence — 2 seconds. Alerts are infrequent
/// but operators expect fairly prompt desktop toasts when an
/// outage fires.
pub const DEFAULT_TICK_INTERVAL: Duration = Duration::from_secs(2);

/// Bound the startup phase so alert freshness remains within the existing
/// first-poll cadence. The phase is derived from the node identity rather than
/// process randomness, so a restart does not create a new common-mode pattern.
const MAX_INITIAL_PHASE: Duration = Duration::from_secs(1);

/// Delivery receipts live beside the retained alert history. Keeping them in a
/// separate hidden directory lets a restarted worker distinguish history from
/// work without mutating or deleting the source event.
const DELIVERY_RECEIPTS_DIR: &str = ".relay-delivered-v1";

/// MON-3 currently emits ULIDs. Leave room for a future namespaced identifier,
/// but keep the receipt filename bounded and traversal-proof.
const MAX_ALERT_ID_BYTES: usize = 128;

/// Subset of the MON-3 `AlertEvent` schema the relay needs
/// to render an FDO notification. The full schema lives in
/// `crates/mde-alert-emit/src/main.rs::AlertEvent`; the
/// relay only deserializes the fields it consumes so a
/// future schema bump (additional optional fields) doesn't
/// break this worker.
#[derive(Debug, Clone, Deserialize)]
pub struct AlertEventPartial {
    /// Stable alert id (ULID).
    pub id: String,
    /// `crit` | `warn` | `info`.
    pub severity: String,
    /// Netdata alert name (e.g. `disk_usage.<filesystem>`).
    pub alert: String,
    /// Hostname the alert fired on.
    pub host: String,
    /// Operator-facing one-line summary.
    pub summary: String,
    /// Netdata chart URL (optional — empty when absent).
    #[serde(default)]
    pub chart_url: String,
}

/// Worker handle. Cheap to construct.
pub struct AlertRelayWorker {
    /// Alert-events dir. Default `~/.local/share/mde/alerts/`.
    alerts_dir: PathBuf,
    /// Sweep cadence.
    tick: Duration,
    /// Stable local identity used to spread the first directory sweep.
    node_id: String,
    /// `notify-send` binary path. Default `notify-send` (looked
    /// up on PATH). Tests inject `/bin/true` to neutralize the
    /// shell-out without a session bus. Used only as the headless
    /// fallback now that the primary path is the Bus (OBS-8).
    notify_send: String,
    /// OBS-8 — `mde-bus` binary. The primary delivery: publish the
    /// alert on the `fdo/*` topic so the cosmic-applet renders it via
    /// the FDO Notifications path. Default `mde-bus`; tests inject
    /// `/bin/true`. Empty string disables the Bus path (force the
    /// notify-send fallback in a test).
    bus_binary: String,
    /// IDs surfaced by this process. Successful delivery also creates a durable
    /// receipt beside the retained history so restarts remain idempotent.
    seen_alert_ids: std::sync::Mutex<BTreeSet<String>>,
}

impl AlertRelayWorker {
    /// Construct with production defaults — alerts dir at
    /// `$XDG_DATA_HOME/mde/alerts/` or
    /// `$HOME/.local/share/mde/alerts/`; 2s tick; PATH
    /// `notify-send`.
    #[must_use]
    pub fn new() -> Self {
        let alerts_dir = default_alerts_dir().unwrap_or_else(|| PathBuf::from("/tmp/mde-alerts"));
        Self {
            alerts_dir,
            tick: DEFAULT_TICK_INTERVAL,
            node_id: local_node_identity(),
            notify_send: "notify-send".to_owned(),
            bus_binary: "mde-bus".to_owned(),
            seen_alert_ids: std::sync::Mutex::new(BTreeSet::new()),
        }
    }

    /// Override the alerts dir. Tests redirect to a tempdir.
    #[must_use]
    pub fn with_alerts_dir(mut self, path: PathBuf) -> Self {
        self.alerts_dir = path;
        self
    }

    /// Override the tick cadence. Tests use shorter values.
    #[must_use]
    pub fn with_tick(mut self, t: Duration) -> Self {
        self.tick = t;
        self
    }

    /// Override the scheduling identity for deterministic tests and fixtures.
    #[must_use]
    pub fn with_node_id(mut self, node_id: impl Into<String>) -> Self {
        self.node_id = node_id.into();
        self
    }

    /// Override the `notify-send` binary path. Tests pass
    /// `/bin/true` so the worker doesn't attempt a real
    /// FDO notification on a headless test host.
    #[must_use]
    pub fn with_notify_send_binary(mut self, name: impl Into<String>) -> Self {
        self.notify_send = name.into();
        self
    }

    /// OBS-8 — override the `mde-bus` binary (tests inject `/bin/true`;
    /// `""` disables the Bus path so the notify-send fallback is exercised).
    #[must_use]
    pub fn with_bus_binary(mut self, name: impl Into<String>) -> Self {
        self.bus_binary = name.into();
        self
    }

    /// One tick. Pulled out for direct testing without the
    /// tokio time pulse.
    pub fn tick_once(&self) -> usize {
        let entries = match std::fs::read_dir(&self.alerts_dir) {
            Ok(e) => e,
            Err(_) => return 0,
        };
        let mut fired = 0usize;
        for entry in entries.flatten() {
            let path = entry.path();
            // Only consume *.json files; skip the *.json.tmp
            // tempfiles MON-3's atomic-rename uses.
            let Some(ext) = path.extension() else {
                continue;
            };
            if ext != "json" {
                continue;
            }
            let Ok(body) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(event) = serde_json::from_str::<AlertEventPartial>(&body) else {
                tracing::warn!(
                    target: "mackesd::alert_relay",
                    path = %path.display(),
                    "skipping unparseable alert event",
                );
                continue;
            };
            if !valid_receipt_id(&event.id) {
                tracing::warn!(
                    target: "mackesd::alert_relay",
                    path = %path.display(),
                    "skipping alert event with unsafe or overlong id",
                );
                continue;
            }
            if self.was_delivered(&event.id) {
                continue;
            }
            if self.fire_notification(&event) {
                self.mark_delivered(&event.id);
                fired += 1;
            }
        }
        fired
    }

    fn was_delivered(&self, id: &str) -> bool {
        if self
            .seen_alert_ids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(id)
        {
            return true;
        }
        std::fs::symlink_metadata(self.delivery_receipt(id))
            .map(|metadata| metadata.file_type().is_file())
            .unwrap_or(false)
    }

    /// Acknowledge only after one delivery route succeeds. The in-memory mark
    /// avoids repeats when a read-only or full alerts directory prevents the
    /// durable receipt; the missing receipt deliberately permits retry after a
    /// restart rather than losing an undelivered alert.
    fn mark_delivered(&self, id: &str) {
        let mut guard = self
            .seen_alert_ids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.insert(id.to_owned());
        drop(guard);

        let receipt = self.delivery_receipt(id);
        let Some(parent) = receipt.parent() else {
            return;
        };
        if let Err(error) = std::fs::create_dir_all(parent) {
            tracing::warn!(
                target: "mackesd::alert_relay",
                %error,
                path = %parent.display(),
                "could not create durable alert-delivery receipt directory",
            );
            return;
        }
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&receipt)
        {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => tracing::warn!(
                target: "mackesd::alert_relay",
                %error,
                path = %receipt.display(),
                "could not persist durable alert-delivery receipt",
            ),
        }
    }

    fn delivery_receipt(&self, id: &str) -> PathBuf {
        self.alerts_dir.join(DELIVERY_RECEIPTS_DIR).join(id)
    }

    fn fire_notification(&self, event: &AlertEventPartial) -> bool {
        // EFF-26 — the headless route: EVERY alert lands in the journal
        // at its severity level, unconditionally and first. On a
        // Lighthouse/Server with no desktop and no applet, the journal
        // (+ the operator's log pipeline) IS the alert surface; toast
        // delivery below is additive, not load-bearing.
        match event.severity.as_str() {
            "crit" => tracing::error!(
                target: "mackesd::alert",
                alert = %event.alert,
                host = %event.host,
                summary = %event.summary,
                "ALERT (crit)",
            ),
            "warn" => tracing::warn!(
                target: "mackesd::alert",
                alert = %event.alert,
                host = %event.host,
                summary = %event.summary,
                "ALERT (warn)",
            ),
            _ => tracing::info!(
                target: "mackesd::alert",
                alert = %event.alert,
                host = %event.host,
                summary = %event.summary,
                "ALERT (info)",
            ),
        }
        // OBS-8 — primary path: publish on the `fdo/*` Bus topic so the
        // cosmic-applet renders it through the FDO Notifications path
        // (the same FDO→mde-bus bridge path). Only when the Bus
        // path is unavailable (no `mde-bus`, e.g. a pre-RPM dev box) do
        // we fall back to a direct `notify-send`.
        if !self.bus_binary.is_empty() {
            let argv = bus_publish_argv(&self.bus_binary, event);
            match std::process::Command::new(&argv[0])
                .args(&argv[1..])
                .output()
            {
                Ok(o) if o.status.success() => {
                    tracing::info!(
                        target: "mackesd::alert_relay",
                        alert = %event.alert,
                        severity = %event.severity,
                        host = %event.host,
                        "published alert on the Bus FDO topic (OBS-8)",
                    );
                    return true;
                }
                Ok(o) => {
                    tracing::debug!(
                        target: "mackesd::alert_relay",
                        status = ?o.status,
                        "mde-bus publish exited non-zero; falling back to notify-send",
                    );
                }
                Err(e) => {
                    tracing::debug!(
                        target: "mackesd::alert_relay",
                        error = %e,
                        "mde-bus not invocable; falling back to notify-send",
                    );
                }
            }
        }
        let argv = notify_send_argv(&self.notify_send, event);
        match std::process::Command::new(&argv[0])
            .args(&argv[1..])
            .output()
        {
            Ok(o) if o.status.success() => {
                tracing::info!(
                    target: "mackesd::alert_relay",
                    alert = %event.alert,
                    severity = %event.severity,
                    host = %event.host,
                    "fired FDO notification (notify-send fallback)",
                );
                true
            }
            Ok(o) => {
                tracing::debug!(
                    target: "mackesd::alert_relay",
                    status = ?o.status,
                    stderr = %String::from_utf8_lossy(&o.stderr),
                    "notify-send exited non-zero",
                );
                false
            }
            Err(e) => {
                tracing::debug!(
                    target: "mackesd::alert_relay",
                    error = %e,
                    binary = %self.notify_send,
                    "notify-send launch failed (operator may be running headless)",
                );
                false
            }
        }
    }
}

fn valid_receipt_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ALERT_ID_BYTES
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
        && id != "."
        && id != ".."
}

impl Default for AlertRelayWorker {
    fn default() -> Self {
        Self::new()
    }
}

/// Build the `notify-send` argv for one event. Pure-fn so
/// tests can verify the argv shape without shelling.
#[must_use]
pub fn notify_send_argv(binary: &str, event: &AlertEventPartial) -> Vec<String> {
    let urgency = match event.severity.to_ascii_uppercase().as_str() {
        "CRITICAL" | "ERROR" => "critical",
        "WARNING" | "WARN" => "normal",
        _ => "low",
    };
    let mut argv = vec![
        binary.to_owned(),
        "--app-name=Mackes Alerts".to_owned(),
        format!("--urgency={urgency}"),
    ];
    if !event.chart_url.is_empty() {
        argv.push(format!("--hint=string:chart-url:{}", event.chart_url));
    }
    let title = format!("[{}] {}", event.host, event.alert);
    let body = if event.summary.is_empty() {
        format!("({} alert without summary)", event.severity)
    } else {
        event.summary.clone()
    };
    argv.push(title);
    argv.push(body);
    argv
}

/// OBS-8 — build the `mde-bus publish` argv that delivers an alert
/// through the cosmic-applet FDO Notifications path. Publishes on the
/// `fdo/MCNF Alerts` topic (rendered via the FDO Notifications bridge),
/// mapping severity → the Bus priority and `[host] alert` → the title.
/// `--no-broker` lets the publish persist + reach even pre-enrollment.
/// Pure so the argv shape is unit-tested without shelling.
#[must_use]
pub fn bus_publish_argv(binary: &str, event: &AlertEventPartial) -> Vec<String> {
    let priority = match event.severity.to_ascii_uppercase().as_str() {
        "CRITICAL" | "ERROR" => "urgent",
        "WARNING" | "WARN" => "default",
        _ => "min",
    };
    let title = format!("[{}] {}", event.host, event.alert);
    let body = if event.summary.is_empty() {
        format!("({} alert without summary)", event.severity)
    } else {
        event.summary.clone()
    };
    let mut argv = vec![
        binary.to_owned(),
        "publish".to_owned(),
        "fdo/MCNF Alerts".to_owned(),
        "--priority".to_owned(),
        priority.to_owned(),
        "--title".to_owned(),
        title,
        "--body-flag".to_owned(),
        body,
        "--no-broker".to_owned(),
    ];
    if !event.chart_url.is_empty() {
        argv.push("--hint".to_owned());
        argv.push(format!("string:chart-url:{}", event.chart_url));
    }
    argv
}

/// Resolve `~/.local/share/mde/alerts/` honoring
/// `$XDG_DATA_HOME` first.
pub fn default_alerts_dir() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        return Some(PathBuf::from(xdg).join("mde").join("alerts"));
    }
    let home = std::env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("mde")
            .join("alerts"),
    )
}

/// Resolve the identity used for startup scheduling without spawning a
/// process. An empty identity deliberately disables phasing: nodes that do
/// not expose an identity retain the existing timing rather than receiving a
/// misleading identical phase.
fn local_node_identity() -> String {
    for key in ["MACKESD_NODE_ID", "HOSTNAME"] {
        if let Ok(value) = std::env::var(key) {
            let value = value.trim();
            if !value.is_empty() {
                return value.to_owned();
            }
        }
    }
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_default()
}

/// Return a stable, bounded phase for the first alert-directory sweep.
///
/// The phase is at most half of a configured cadence, so the first sweep is
/// always due no later than the old cadence. FNV-1a is sufficient here because
/// this is scheduling spread, not a security primitive.
fn initial_phase_for(node_id: &str, tick: Duration) -> Duration {
    let window_ms = (tick.as_millis() / 2).min(MAX_INITIAL_PHASE.as_millis());
    if node_id.is_empty() || window_ms == 0 {
        return Duration::ZERO;
    }
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in node_id.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    let window_ms = window_ms as u64;
    Duration::from_millis(hash % (window_ms + 1))
}

#[async_trait::async_trait]
impl Worker for AlertRelayWorker {
    fn name(&self) -> &'static str {
        "alert_relay"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        // Keep the first poll no later than the old cadence while spreading
        // identical daemon startups across the interval. Selecting shutdown
        // during this delay preserves prompt cancellation during boot.
        let first_delay = self
            .tick
            .saturating_sub(initial_phase_for(&self.node_id, self.tick));
        tokio::select! {
            _ = shutdown.wait() => return Ok(()),
            _ = tokio::time::sleep(first_delay) => {}
        }
        loop {
            tokio::select! {
                _ = shutdown.wait() => return Ok(()),
                _ = tokio::time::sleep(self.tick) => {
                    let _ = self.tick_once();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_event(dir: &std::path::Path, id: &str, severity: &str) {
        let event = serde_json::json!({
            "id": id,
            "ts": 1_716_000_000,
            "severity": severity,
            "category": "test.cat",
            "alert": "test_alert",
            "host": "peer:test",
            "summary": "test summary",
            "value": "42",
            "threshold": "10",
            "chart_url": format!("https://example/{id}"),
            "fired_by": "mde-alert-emit",
            "seen_by": [],
        });
        let path = dir.join(format!("{id}.json"));
        std::fs::write(&path, serde_json::to_string_pretty(&event).unwrap()).unwrap();
    }

    #[test]
    fn tick_once_no_ops_when_alerts_dir_missing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let missing = tmp.path().join("does-not-exist");
        let w = AlertRelayWorker::new()
            .with_alerts_dir(missing)
            .with_notify_send_binary("/bin/true")
            .with_bus_binary("/bin/true");
        assert_eq!(w.tick_once(), 0);
    }

    #[test]
    fn tick_once_fires_one_notification_per_new_alert() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_event(tmp.path(), "01H8XYZABC0000000000000001", "WARNING");
        write_event(tmp.path(), "01H8XYZABC0000000000000002", "CRITICAL");
        let w = AlertRelayWorker::new()
            .with_alerts_dir(tmp.path().to_path_buf())
            .with_notify_send_binary("/bin/true")
            .with_bus_binary("/bin/true");
        assert_eq!(w.tick_once(), 2);
    }

    #[test]
    fn tick_once_dedupes_already_surfaced_alerts() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_event(tmp.path(), "01H8XYZABC0000000000000001", "WARNING");
        let w = AlertRelayWorker::new()
            .with_alerts_dir(tmp.path().to_path_buf())
            .with_notify_send_binary("/bin/true")
            .with_bus_binary("/bin/true");
        // First tick fires once.
        assert_eq!(w.tick_once(), 1);
        // Second tick is a no-op (event ID already in seen_alert_ids).
        assert_eq!(w.tick_once(), 0);
        // New event arrives → fires.
        write_event(tmp.path(), "01H8XYZABC0000000000000002", "CRITICAL");
        assert_eq!(w.tick_once(), 1);
    }

    #[test]
    fn delivery_receipts_survive_restart_retry_failure_and_reject_hostile_ids() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let id = "01H8XYZABC0000000000000001";
        write_event(tmp.path(), id, "CRITICAL");

        let first = AlertRelayWorker::new()
            .with_alerts_dir(tmp.path().to_path_buf())
            .with_notify_send_binary("/bin/true")
            .with_bus_binary("/bin/true");
        assert_eq!(first.tick_once(), 1);
        assert!(tmp.path().join(DELIVERY_RECEIPTS_DIR).join(id).is_file());

        let restarted = AlertRelayWorker::new()
            .with_alerts_dir(tmp.path().to_path_buf())
            .with_notify_send_binary("/bin/true")
            .with_bus_binary("/bin/true");
        assert_eq!(restarted.tick_once(), 0);

        let failed_tmp = tempfile::tempdir().expect("failed tempdir");
        let failed_id = "01H8XYZABC0000000000000002";
        write_event(failed_tmp.path(), failed_id, "CRITICAL");
        let worker = AlertRelayWorker::new()
            .with_alerts_dir(failed_tmp.path().to_path_buf())
            .with_notify_send_binary("/bin/false")
            .with_bus_binary("/bin/false");

        assert_eq!(worker.tick_once(), 0);
        assert_eq!(worker.tick_once(), 0);
        assert!(!worker.was_delivered(failed_id));
        assert!(!failed_tmp
            .path()
            .join(DELIVERY_RECEIPTS_DIR)
            .join(failed_id)
            .exists());

        let hostile_tmp = tempfile::tempdir().expect("hostile tempdir");
        write_event(hostile_tmp.path(), "..", "CRITICAL");
        let worker = AlertRelayWorker::new()
            .with_alerts_dir(hostile_tmp.path().to_path_buf())
            .with_notify_send_binary("/bin/true")
            .with_bus_binary("/bin/true");

        assert_eq!(worker.tick_once(), 0);
        assert!(!hostile_tmp.path().join(DELIVERY_RECEIPTS_DIR).exists());

        let symlink_tmp = tempfile::tempdir().expect("symlink tempdir");
        let symlink_id = "01H8XYZABC0000000000000003";
        write_event(symlink_tmp.path(), symlink_id, "CRITICAL");
        let receipts = symlink_tmp.path().join(DELIVERY_RECEIPTS_DIR);
        std::fs::create_dir(&receipts).expect("receipt directory");
        let outside = symlink_tmp.path().join("outside");
        std::fs::write(&outside, b"not a receipt").expect("outside fixture");
        std::os::unix::fs::symlink(&outside, receipts.join(symlink_id))
            .expect("hostile receipt symlink");

        let first = AlertRelayWorker::new()
            .with_alerts_dir(symlink_tmp.path().to_path_buf())
            .with_notify_send_binary("/bin/true")
            .with_bus_binary("/bin/true");
        assert_eq!(first.tick_once(), 1);
        let restarted = AlertRelayWorker::new()
            .with_alerts_dir(symlink_tmp.path().to_path_buf())
            .with_notify_send_binary("/bin/true")
            .with_bus_binary("/bin/true");
        assert_eq!(restarted.tick_once(), 1);
        assert_eq!(
            std::fs::read(&outside).expect("outside intact"),
            b"not a receipt"
        );
    }

    #[test]
    fn tick_once_skips_unparseable_files() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("bad.json"), b"not valid json").unwrap();
        write_event(tmp.path(), "01H8XYZABC0000000000000001", "WARNING");
        let w = AlertRelayWorker::new()
            .with_alerts_dir(tmp.path().to_path_buf())
            .with_notify_send_binary("/bin/true")
            .with_bus_binary("/bin/true");
        // Bad file is skipped (logged at warn); good file fires.
        assert_eq!(w.tick_once(), 1);
    }

    #[test]
    fn tick_once_ignores_tempfiles_from_mon3_atomic_rename() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("01H.json.tmp"), b"{}").unwrap();
        let w = AlertRelayWorker::new()
            .with_alerts_dir(tmp.path().to_path_buf())
            .with_notify_send_binary("/bin/true")
            .with_bus_binary("/bin/true");
        assert_eq!(w.tick_once(), 0);
    }

    #[test]
    fn initial_phase_is_stable_bounded_and_preserves_first_poll_deadline() {
        let interval = DEFAULT_TICK_INTERVAL;
        let phase = initial_phase_for("peer:seat15", interval);
        assert_eq!(phase, initial_phase_for("peer:seat15", interval));
        assert!(phase <= MAX_INITIAL_PHASE);
        assert!(interval.saturating_sub(phase) <= interval);
        assert!(
            initial_phase_for("peer:seat15", interval)
                != initial_phase_for("peer:seat16", interval)
        );
        assert_eq!(initial_phase_for("", interval), Duration::ZERO);
        assert_eq!(
            initial_phase_for("peer:seat15", Duration::from_millis(1)),
            Duration::ZERO
        );
    }

    #[test]
    fn bus_publish_argv_targets_the_fdo_topic_with_priority() {
        let mk = |sev: &str, url: &str| AlertEventPartial {
            id: "x".into(),
            severity: sev.into(),
            alert: "disk_full".into(),
            host: "anvil".into(),
            summary: "/ at 95%".into(),
            chart_url: url.into(),
        };
        let crit = bus_publish_argv("mde-bus", &mk("CRITICAL", ""));
        // Publishes on the FDO topic the cosmic-applet renders (OBS-8).
        assert_eq!(crit[0], "mde-bus");
        assert_eq!(crit[1], "publish");
        assert_eq!(crit[2], "fdo/MCNF Alerts");
        assert!(crit
            .windows(2)
            .any(|w| w[0] == "--priority" && w[1] == "urgent"));
        assert!(crit.iter().any(|s| s == "[anvil] disk_full"));
        assert!(crit.iter().any(|s| s == "--no-broker"));
        // Severity → priority mapping + the chart-url hint.
        let warn = bus_publish_argv("mde-bus", &mk("WARNING", "http://c/1"));
        assert!(warn
            .windows(2)
            .any(|w| w[0] == "--priority" && w[1] == "default"));
        assert!(warn.iter().any(|s| s == "string:chart-url:http://c/1"));
        let info = bus_publish_argv("mde-bus", &mk("INFO", ""));
        assert!(info
            .windows(2)
            .any(|w| w[0] == "--priority" && w[1] == "min"));
        assert!(!info.iter().any(|s| s.starts_with("string:chart-url")));
    }

    #[test]
    fn notify_send_argv_maps_severity_to_urgency() {
        let mk = |sev: &str| AlertEventPartial {
            id: "x".into(),
            severity: sev.into(),
            alert: "a".into(),
            host: "h".into(),
            summary: "s".into(),
            chart_url: String::new(),
        };
        let crit = notify_send_argv("notify-send", &mk("CRITICAL"));
        assert!(crit.iter().any(|s| s == "--urgency=critical"));
        let warn = notify_send_argv("notify-send", &mk("WARNING"));
        assert!(warn.iter().any(|s| s == "--urgency=normal"));
        let clear = notify_send_argv("notify-send", &mk("CLEAR"));
        assert!(clear.iter().any(|s| s == "--urgency=low"));
    }

    #[test]
    fn notify_send_argv_includes_chart_url_hint_when_present() {
        let event = AlertEventPartial {
            id: "x".into(),
            severity: "WARNING".into(),
            alert: "a".into(),
            host: "h".into(),
            summary: "s".into(),
            chart_url: "https://peer:alice:19999/#menu_nebula".into(),
        };
        let argv = notify_send_argv("notify-send", &event);
        assert!(argv
            .iter()
            .any(|s| s == "--hint=string:chart-url:https://peer:alice:19999/#menu_nebula"));
    }

    #[test]
    fn notify_send_argv_omits_chart_url_hint_when_empty() {
        let event = AlertEventPartial {
            id: "x".into(),
            severity: "WARNING".into(),
            alert: "a".into(),
            host: "h".into(),
            summary: "s".into(),
            chart_url: String::new(),
        };
        let argv = notify_send_argv("notify-send", &event);
        assert!(!argv
            .iter()
            .any(|s| s.starts_with("--hint=string:chart-url:")));
    }

    #[test]
    fn notify_send_argv_substitutes_summary_for_empty() {
        let event = AlertEventPartial {
            id: "x".into(),
            severity: "WARNING".into(),
            alert: "a".into(),
            host: "h".into(),
            summary: String::new(),
            chart_url: String::new(),
        };
        let argv = notify_send_argv("notify-send", &event);
        assert!(argv.iter().any(|s| s.contains("alert without summary")));
    }

    #[test]
    fn notify_send_argv_title_includes_host_and_alert() {
        let event = AlertEventPartial {
            id: "x".into(),
            severity: "WARNING".into(),
            alert: "nebula_process_down".into(),
            host: "peer:alice".into(),
            summary: "s".into(),
            chart_url: String::new(),
        };
        let argv = notify_send_argv("notify-send", &event);
        assert!(argv.iter().any(|s| s == "[peer:alice] nebula_process_down"));
    }

    #[tokio::test]
    async fn worker_exits_on_shutdown_token() {
        let mut w = AlertRelayWorker::new()
            .with_notify_send_binary("/bin/true")
            .with_bus_binary("/bin/true")
            .with_node_id("peer:shutdown-test")
            .with_tick(Duration::from_millis(50));
        let (tx, rx) = tokio::sync::watch::channel(false);
        let token = ShutdownToken::from_receiver(rx);
        let _ = tx.send(true);
        let result = tokio::time::timeout(Duration::from_secs(3), w.run(token))
            .await
            .expect("worker must exit on shutdown");
        assert!(result.is_ok());
    }
}
