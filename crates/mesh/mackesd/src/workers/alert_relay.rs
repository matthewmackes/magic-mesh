//! MON-4 (v2.6) — alert relay worker.
//!
//! Watches `~/.local/share/mde/alerts/` for `*.json` event
//! files written by `mde-alert-emit` (MON-3) + forwards each
//! one as an FDO desktop notification via `notify-send`. The
//! notification surfaces the alert's severity + summary +
//! a deep-link to the chart URL when present.
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
//! Once a file's been surfaced, its ULID lands in the
//! `seen_alert_ids` set so repeat invocations of
//! `mde-alert-emit` against the same alert (idempotent
//! by design of MON-3's deterministic ULID) don't re-fire
//! the notification.
//!
//! Best-effort: if `notify-send` isn't installed (operator
//! running headless), the worker logs at debug + continues
//! polling. The alert files stay on disk for future
//! consumers (MON-5 Workbench Mesh Health panel,
//! future audit tools).

#![cfg(feature = "async-services")]

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

use super::{ShutdownToken, Worker};

/// Default sweep cadence — 2 seconds. Alerts are infrequent
/// but operators expect fairly prompt desktop toasts when an
/// outage fires.
pub const DEFAULT_TICK_INTERVAL: Duration = Duration::from_secs(2);

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
    /// `notify-send` binary path. Default `notify-send` (looked
    /// up on PATH). Tests inject `/bin/true` to neutralize the
    /// shell-out without a session bus.
    notify_send: String,
    /// IDs we've already surfaced. Persists for the worker's
    /// lifetime; on restart the relay re-surfaces every alert
    /// in the dir (operator can `rm ~/.local/share/mde/alerts/`
    /// to silence the chatter — those files outlive the
    /// notification toast by design so MON-5 + future audit
    /// tools can replay them).
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
            notify_send: "notify-send".to_owned(),
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

    /// Override the `notify-send` binary path. Tests pass
    /// `/bin/true` so the worker doesn't attempt a real
    /// FDO notification on a headless test host.
    #[must_use]
    pub fn with_notify_send_binary(mut self, name: impl Into<String>) -> Self {
        self.notify_send = name.into();
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
            if !self.mark_seen(&event.id) {
                continue;
            }
            self.fire_notification(&event);
            fired += 1;
        }
        fired
    }

    /// Record `id` as surfaced. Returns `true` if this is the
    /// first time we've seen it (caller should fire the
    /// notification); `false` if we've already surfaced it
    /// in this worker's lifetime.
    fn mark_seen(&self, id: &str) -> bool {
        let mut guard = self.seen_alert_ids.lock().expect("seen_alert_ids mutex");
        guard.insert(id.to_owned())
    }

    fn fire_notification(&self, event: &AlertEventPartial) {
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
                    "fired FDO notification",
                );
            }
            Ok(o) => {
                tracing::debug!(
                    target: "mackesd::alert_relay",
                    status = ?o.status,
                    stderr = %String::from_utf8_lossy(&o.stderr),
                    "notify-send exited non-zero",
                );
            }
            Err(e) => {
                tracing::debug!(
                    target: "mackesd::alert_relay",
                    error = %e,
                    binary = %self.notify_send,
                    "notify-send launch failed (operator may be running headless)",
                );
            }
        }
    }
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

#[async_trait::async_trait]
impl Worker for AlertRelayWorker {
    fn name(&self) -> &'static str {
        "alert_relay"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
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
            .with_notify_send_binary("/bin/true");
        assert_eq!(w.tick_once(), 0);
    }

    #[test]
    fn tick_once_fires_one_notification_per_new_alert() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_event(tmp.path(), "01H8XYZABC0000000000000001", "WARNING");
        write_event(tmp.path(), "01H8XYZABC0000000000000002", "CRITICAL");
        let w = AlertRelayWorker::new()
            .with_alerts_dir(tmp.path().to_path_buf())
            .with_notify_send_binary("/bin/true");
        assert_eq!(w.tick_once(), 2);
    }

    #[test]
    fn tick_once_dedupes_already_surfaced_alerts() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write_event(tmp.path(), "01H8XYZABC0000000000000001", "WARNING");
        let w = AlertRelayWorker::new()
            .with_alerts_dir(tmp.path().to_path_buf())
            .with_notify_send_binary("/bin/true");
        // First tick fires once.
        assert_eq!(w.tick_once(), 1);
        // Second tick is a no-op (event ID already in seen_alert_ids).
        assert_eq!(w.tick_once(), 0);
        // New event arrives → fires.
        write_event(tmp.path(), "01H8XYZABC0000000000000002", "CRITICAL");
        assert_eq!(w.tick_once(), 1);
    }

    #[test]
    fn tick_once_skips_unparseable_files() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("bad.json"), b"not valid json").unwrap();
        write_event(tmp.path(), "01H8XYZABC0000000000000001", "WARNING");
        let w = AlertRelayWorker::new()
            .with_alerts_dir(tmp.path().to_path_buf())
            .with_notify_send_binary("/bin/true");
        // Bad file is skipped (logged at warn); good file fires.
        assert_eq!(w.tick_once(), 1);
    }

    #[test]
    fn tick_once_ignores_tempfiles_from_mon3_atomic_rename() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("01H.json.tmp"), b"{}").unwrap();
        let w = AlertRelayWorker::new()
            .with_alerts_dir(tmp.path().to_path_buf())
            .with_notify_send_binary("/bin/true");
        assert_eq!(w.tick_once(), 0);
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
