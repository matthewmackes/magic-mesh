//! PD-13 (L5) — presence-transition alerts.
//!
//! Sweeps the replicated PeerRecords every 30 s, computes each
//! peer's presence tier (the same Q11 thresholds the directory
//! serves), and on an Online↔Offline transition writes one alert
//! JSON into the `alert_relay` watch dir — riding the existing
//! alert→`notify-send` pipeline (OBS-7/8 plumbing) instead of
//! growing a second notifier. Idle flaps are deliberately quiet:
//! only the offline boundary notifies (a peer at lunch isn't news;
//! a peer that vanished is).
//!
//! The first sweep seeds silently (a daemon restart must not
//! re-announce the whole mesh).

#![cfg(feature = "async-services")]

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use super::{ShutdownToken, Worker};
use crate::ipc::directory::presence_tier;

/// Sweep cadence — matches the heartbeat/presence granularity.
pub const SWEEP_INTERVAL: Duration = Duration::from_secs(30);

/// Compute the notifications a sweep should emit given the previous
/// and current tier maps (pure — the testable core). Only crossings
/// of the offline boundary notify.
#[must_use]
pub fn transitions(
    prev: &HashMap<String, String>,
    current: &HashMap<String, String>,
) -> Vec<(String, &'static str)> {
    let mut out = Vec::new();
    for (host, tier) in current {
        let was = prev.get(host).map(String::as_str);
        let is_offline = tier == "offline";
        let was_offline = was == Some("offline");
        match (was, is_offline, was_offline) {
            // Known peer crossed INTO offline.
            (Some(_), true, false) => out.push((host.clone(), "offline")),
            // Known peer came BACK from offline.
            (Some(_), false, true) => out.push((host.clone(), "online")),
            _ => {}
        }
    }
    out
}

/// The presence-transition watcher.
pub struct PresenceWatchWorker {
    workgroup_root: PathBuf,
    alerts_dir: PathBuf,
    self_hostname: String,
}

impl PresenceWatchWorker {
    #[must_use]
    pub fn new(workgroup_root: PathBuf, alerts_dir: PathBuf, self_hostname: String) -> Self {
        Self {
            workgroup_root,
            alerts_dir,
            self_hostname,
        }
    }

    fn current_tiers(&self) -> HashMap<String, String> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        mackes_mesh_types::peers::read_peers(&mackes_mesh_types::peers::peers_dir(
            &self.workgroup_root,
        ))
        .into_iter()
        .filter(|r| r.hostname != self.self_hostname)
        .map(|r| {
            (
                r.hostname,
                presence_tier(now_ms, r.last_seen_ms).to_string(),
            )
        })
        .collect()
    }

    fn emit(&self, host: &str, direction: &str) {
        // Deterministic id per (host, direction, minute) so a re-sweep
        // inside the relay's dedupe window can't double-notify.
        let minute = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() / 60);
        let id = format!("presence-{host}-{direction}-{minute}");
        let (severity, summary) = if direction == "offline" {
            ("warn", format!("Peer {host} went offline"))
        } else {
            ("info", format!("Peer {host} is back online"))
        };
        let event = serde_json::json!({
            "id": id,
            "severity": severity,
            "alert": format!("mesh.presence.{direction}"),
            "host": host,
            "summary": summary,
        });
        if std::fs::create_dir_all(&self.alerts_dir).is_ok() {
            let path = self.alerts_dir.join(format!("{id}.json"));
            let _ = std::fs::write(path, event.to_string());
        }
    }
}

#[async_trait::async_trait]
impl Worker for PresenceWatchWorker {
    fn name(&self) -> &'static str {
        "presence_watch"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        // Seed silently — a restart must not re-announce the mesh.
        let mut prev = self.current_tiers();
        loop {
            tokio::select! {
                _ = shutdown.wait() => return Ok(()),
                () = tokio::time::sleep(SWEEP_INTERVAL) => {}
            }
            let current = self.current_tiers();
            for (host, direction) in transitions(&prev, &current) {
                tracing::info!(peer = %host, direction, "presence_watch: transition (PD-13)");
                self.emit(&host, direction);
            }
            prev = current;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiers(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(h, t)| ((*h).to_string(), (*t).to_string()))
            .collect()
    }

    #[test]
    fn only_offline_boundary_crossings_notify() {
        let prev = tiers(&[("pine", "online"), ("oak", "offline"), ("elm", "online")]);
        let cur = tiers(&[("pine", "offline"), ("oak", "online"), ("elm", "idle")]);
        let mut got = transitions(&prev, &cur);
        got.sort();
        assert_eq!(
            got,
            vec![
                ("oak".to_string(), "online"),
                ("pine".to_string(), "offline"),
            ],
            "online→offline + offline→online notify; online→idle is quiet"
        );
    }

    #[test]
    fn unknown_peers_seed_silently() {
        // A peer appearing for the first time (fresh seed / new enroll)
        // never notifies — only known-state transitions do.
        let prev = HashMap::new();
        let cur = tiers(&[("pine", "offline")]);
        assert!(transitions(&prev, &cur).is_empty());
    }

    #[test]
    fn idle_flaps_are_quiet() {
        let prev = tiers(&[("pine", "online")]);
        let cur = tiers(&[("pine", "idle")]);
        assert!(transitions(&prev, &cur).is_empty());
    }
}
