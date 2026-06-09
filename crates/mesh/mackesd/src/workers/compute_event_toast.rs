//! `compute_event_toast` worker (VIRT-21) — subscribes to every
//! `compute/event/<peer>` topic on the Mackes Bus and raises an FDO
//! desktop toast ("VM <name> started/stopped/crashed on <hostname>")
//! via `notify-send`, so an operator learns about fleet VM lifecycle
//! changes without keeping `mde-virtual` open.
//!
//! - Crash events use `--urgency=critical`; start/stop use `normal`.
//! - Each `compute/event/<peer>` topic is consumed with its own cursor
//!   (mirrors `compute_migrate`). On first sight of a topic the cursor
//!   is seeded to the current head, so a worker (re)start doesn't replay
//!   the historical backlog as a toast storm — only events published
//!   after the worker comes up are surfaced.
//! - Best-effort: when `notify-send` is absent (headless peer) the
//!   launch failure is logged at debug and the worker keeps draining.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use mde_bus::persist::Persist;

use super::compute_registry::ComputeEvent;
use super::{ShutdownToken, Worker};

/// Poll cadence — responsive enough for lifecycle toasts without
/// hammering the Bus directory.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(3);

/// FDO urgency for a compute event. Crashes are `critical`; starts and
/// stops are `normal`.
#[must_use]
pub fn event_urgency(event: &str) -> &'static str {
    if event == "crashed" {
        "critical"
    } else {
        "normal"
    }
}

/// Build the `notify-send` argv for one compute event. Pure so tests
/// can assert the shape without shelling out. Falls back to the peer's
/// Nebula address when `hostname` is empty.
#[must_use]
pub fn toast_argv(binary: &str, ev: &ComputeEvent) -> Vec<String> {
    let location = if ev.hostname.is_empty() {
        ev.peer.as_str()
    } else {
        ev.hostname.as_str()
    };
    vec![
        binary.to_owned(),
        "--app-name=MDE Virtual".to_owned(),
        format!("--urgency={}", event_urgency(&ev.event)),
        format!("VM {} {}", ev.vm_name, ev.event),
        format!("on {location}"),
    ]
}

fn fire_toast(binary: &str, ev: &ComputeEvent) {
    let argv = toast_argv(binary, ev);
    match Command::new(&argv[0]).args(&argv[1..]).output() {
        Ok(o) if o.status.success() => {
            tracing::info!(
                target: "mackesd::compute_event_toast",
                vm = %ev.vm_name,
                event = %ev.event,
                host = %ev.hostname,
                "fired compute lifecycle toast",
            );
        }
        Ok(o) => {
            tracing::debug!(
                target: "mackesd::compute_event_toast",
                status = ?o.status,
                stderr = %String::from_utf8_lossy(&o.stderr),
                "notify-send exited non-zero",
            );
        }
        Err(e) => {
            tracing::debug!(
                target: "mackesd::compute_event_toast",
                error = %e,
                binary = %binary,
                "notify-send launch failed (peer may be headless)",
            );
        }
    }
}

/// One poll pass: enumerate every `compute/event/*` topic, seed unseen
/// topics to head, and toast each new message on already-seen topics.
fn poll_and_toast(
    persist: &Persist,
    notify_send: &str,
    cursors: &mut BTreeMap<String, Option<String>>,
) {
    let topics = match persist.list_topics() {
        Ok(t) => t,
        Err(e) => {
            tracing::debug!(error = %e, "compute_event_toast: list_topics failed");
            return;
        }
    };
    for t in topics.iter().filter(|t| t.starts_with("compute/event/")) {
        if !cursors.contains_key(t) {
            // First sight: seed the cursor to the current head so the
            // pre-existing backlog isn't replayed as toasts.
            let head = persist
                .list_since(t, None)
                .ok()
                .and_then(|msgs| msgs.last().map(|m| m.ulid.clone()));
            cursors.insert(t.clone(), head);
            continue;
        }
        let cursor = cursors.get(t).cloned().flatten();
        let msgs = match persist.list_since(t, cursor.as_deref()) {
            Ok(m) => m,
            Err(e) => {
                tracing::debug!(topic = %t, error = %e, "compute_event_toast: list_since failed");
                continue;
            }
        };
        for msg in msgs {
            cursors.insert(t.clone(), Some(msg.ulid.clone()));
            let Some(body) = msg.body.as_deref() else {
                continue;
            };
            match serde_json::from_str::<ComputeEvent>(body) {
                Ok(ev) => fire_toast(notify_send, &ev),
                Err(e) => {
                    tracing::warn!(ulid = %msg.ulid, error = %e, "compute_event_toast: bad event body")
                }
            }
        }
    }
}

/// Worker handle.
pub struct ComputeEventToastWorker {
    notify_send: String,
    poll_interval: Duration,
    bus_root_override: Option<PathBuf>,
}

impl Default for ComputeEventToastWorker {
    fn default() -> Self {
        Self::new()
    }
}

impl ComputeEventToastWorker {
    /// Construct with production defaults (`notify-send`, 3 s poll).
    #[must_use]
    pub fn new() -> Self {
        Self {
            notify_send: "notify-send".to_owned(),
            poll_interval: DEFAULT_POLL_INTERVAL,
            bus_root_override: None,
        }
    }

    /// Override the Bus root directory. Used in tests.
    #[must_use]
    pub fn with_bus_root(mut self, p: PathBuf) -> Self {
        self.bus_root_override = Some(p);
        self
    }

    /// Override the `notify-send` binary path. Tests pass a stub.
    #[must_use]
    pub fn with_notify_send_binary(mut self, name: impl Into<String>) -> Self {
        self.notify_send = name.into();
        self
    }

    /// Override the poll cadence. Used in tests.
    #[must_use]
    pub fn with_poll_interval(mut self, d: Duration) -> Self {
        self.poll_interval = d;
        self
    }
}

fn default_bus_root() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("mde").join("bus"))
}

#[async_trait::async_trait]
impl Worker for ComputeEventToastWorker {
    fn name(&self) -> &'static str {
        "compute_event_toast"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let bus_root = match self.bus_root_override.clone().or_else(default_bus_root) {
            Some(r) => r,
            None => {
                tracing::debug!("compute_event_toast: no bus root; worker idle");
                return Ok(());
            }
        };
        let persist = match Persist::open(bus_root) {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!(error = %e, "compute_event_toast: persist open failed; worker idle");
                return Ok(());
            }
        };
        let mut cursors: BTreeMap<String, Option<String>> = BTreeMap::new();
        let mut tick = tokio::time::interval(self.poll_interval);
        tick.tick().await;
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    poll_and_toast(&persist, &self.notify_send, &mut cursors);
                }
                _ = shutdown.wait() => break,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(event: &str) -> ComputeEvent {
        ComputeEvent {
            vm_id: "uuid-1".into(),
            vm_name: "web1".into(),
            event: event.into(),
            peer: "10.42.0.5".into(),
            hostname: "host-b".into(),
        }
    }

    #[test]
    fn urgency_maps_crashed_to_critical() {
        assert_eq!(event_urgency("crashed"), "critical");
        assert_eq!(event_urgency("started"), "normal");
        assert_eq!(event_urgency("stopped"), "normal");
    }

    #[test]
    fn toast_argv_shape_and_urgency() {
        let argv = toast_argv("notify-send", &ev("crashed"));
        assert_eq!(argv[0], "notify-send");
        assert!(argv.contains(&"--app-name=MDE Virtual".to_string()));
        assert!(argv.contains(&"--urgency=critical".to_string()));
        // Summary + body carry the name, verb, and host.
        assert!(argv.iter().any(|a| a == "VM web1 crashed"));
        assert!(argv.iter().any(|a| a == "on host-b"));
    }

    #[test]
    fn toast_argv_falls_back_to_peer_without_hostname() {
        let mut e = ev("started");
        e.hostname = String::new();
        let argv = toast_argv("notify-send", &e);
        assert!(argv.iter().any(|a| a == "on 10.42.0.5"));
        assert!(argv.contains(&"--urgency=normal".to_string()));
    }
}
