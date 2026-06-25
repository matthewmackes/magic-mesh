//! SUBSTRATE-10 (SUBSTRATE-V2) — etcd **watch** worker: push, not poll.
//!
//! Every other coordination-plane consumer reads etcd by *polling*: the
//! [`crate::workers::health_reconciler`] re-derives peer health from a range-get
//! of `/mesh/peers/` on a 5 s tick, the healthz bridge blocking-reads the leader
//! key on demand. That's fine for the steady-state projection, but it means an
//! operator learns a peer dropped or leadership flipped only on the *next* tick —
//! up to a full reconcile interval late, and never as a discrete event.
//!
//! etcd's native primitive for "tell me the instant a key changes" is a **watch
//! stream** ([`crate::substrate::etcd::watch`]). This worker opens two:
//!   * `/mesh/peers/` (prefix) — a **Delete** event fires the moment a peer's
//!     keepalive lease expires (liveness IS the lease, SUBSTRATE-3), so a vanished
//!     peer is an INSTANT `mesh.etcd.peer_down` alert, not a 5 s-late poll diff;
//!   * `/mesh/leader` (single key) — a **Put** carrying a different leader
//!     `node_id` than we last saw is an INSTANT `mesh.etcd.leader_change` alert.
//!     The leader renews its lease every campaign tick (a fresh `renewed_at_s`
//!     each time), so we compare the decoded [`Lease::node_id`], NOT the raw
//!     value — a renew is silent; only a genuine handover notifies.
//!
//! Alerts ride the SAME lane `presence_watch` (PD-13) uses: a JSON file dropped
//! into the `alert_relay` watch dir, which the OBS-7/8 pipeline turns into an FDO
//! desktop notification (Bus-first, `notify-send` fallback). No second notifier.
//!
//! Degrade cleanly (§2): empty endpoints (a pre-cutover node not yet on etcd) →
//! the worker idles, re-checking on a slow cadence, never spinning. A connect or
//! stream error → log at warn, back off, reconnect — the worker never panics and
//! never crash-loops the supervisor on an etcd-down node (ENT-6 would trip it,
//! but the in-worker back-off keeps it from getting there).

#![cfg(feature = "async-services")]

use std::path::PathBuf;
use std::time::Duration;

use etcd_client::{EventType, WatchOptions};

use super::{ShutdownToken, Worker};
use crate::leader::Lease;
use crate::substrate::etcd::{connect, default_endpoints, watch, LEADER_KEY, PEERS_PREFIX};

/// Reconnect back-off after an etcd connect / stream error. Short enough that a
/// brief etcd blip recovers promptly; long enough that an etcd-down node doesn't
/// hot-loop connect attempts (which would also burn toward the ENT-6 breaker).
pub const RECONNECT_BACKOFF: Duration = Duration::from_secs(10);

/// Idle re-check cadence when this node has no etcd endpoints yet (pre-cutover).
/// `setup-etcd.sh` may provision endpoints after the daemon is already up, so we
/// re-read [`default_endpoints`] on this slow cadence rather than exiting.
pub const IDLE_RECHECK: Duration = Duration::from_secs(60);

/// The etcd-watch worker. Cheap to construct; all I/O happens inside [`run`].
pub struct EtcdWatchWorker {
    /// etcd client endpoints. Empty ⇒ this node isn't on the coordination plane
    /// yet (pre-cutover) ⇒ the worker idles. Defaults to [`default_endpoints`]
    /// via [`new`](Self::new); tests inject a fixed list.
    endpoints: Vec<String>,
    /// The `alert_relay` drop-dir — where each instant alert JSON lands.
    alerts_dir: PathBuf,
    /// This node's short hostname — excluded from peer-down alerts (a node can't
    /// watch its own lease expire: it's the one running this code).
    self_hostname: String,
    /// Re-read [`default_endpoints`] each idle tick instead of trusting the
    /// constructor snapshot. Production sets this so a post-boot `setup-etcd.sh`
    /// provision is picked up without a daemon restart; tests pin the injected
    /// list by leaving it `false`.
    refresh_endpoints: bool,
}

impl EtcdWatchWorker {
    /// Construct with production defaults — endpoints from
    /// [`default_endpoints`], re-read each idle tick.
    #[must_use]
    pub fn new(alerts_dir: PathBuf, self_hostname: String) -> Self {
        Self {
            endpoints: default_endpoints(),
            alerts_dir,
            self_hostname,
            refresh_endpoints: true,
        }
    }

    /// Pin a fixed endpoint list (tests — no `ENDPOINTS_FILE` re-read).
    #[must_use]
    pub fn with_endpoints(mut self, endpoints: Vec<String>) -> Self {
        self.endpoints = endpoints;
        self.refresh_endpoints = false;
        self
    }

    /// Run one connected watch session: open both streams, pump events until a
    /// stream ends/errors or shutdown fires. Returns `Ok(())` on shutdown,
    /// `Err(_)` on a stream error the caller turns into a back-off + reconnect.
    /// The `last_leader` is threaded across reconnects so a reconnect mid-term
    /// doesn't re-announce the sitting leader.
    async fn watch_session(
        &self,
        shutdown: &mut ShutdownToken,
        last_leader: &mut Option<String>,
    ) -> anyhow::Result<()> {
        let mut client = connect(&self.endpoints).await?;
        // Seed the leader baseline from the current key so the first PUT we see
        // after (re)connect isn't mistaken for a handover (it may be a renew of
        // the incumbent). A read failure leaves the prior baseline intact.
        if last_leader.is_none() {
            if let Ok(resp) = client.get(LEADER_KEY, None).await {
                *last_leader = resp
                    .kvs()
                    .first()
                    .and_then(|kv| kv.value_str().ok())
                    .and_then(Lease::decode)
                    .map(|l| l.node_id);
            }
        }
        let mut peers_stream = watch(
            &mut client,
            PEERS_PREFIX,
            Some(WatchOptions::new().with_prefix()),
        )
        .await?;
        let mut leader_stream = watch(&mut client, LEADER_KEY, None).await?;
        loop {
            tokio::select! {
                _ = shutdown.wait() => return Ok(()),
                msg = peers_stream.message() => {
                    let resp = msg?.ok_or_else(|| anyhow::anyhow!("peers watch stream closed"))?;
                    for ev in resp.events() {
                        if ev.event_type() == EventType::Delete {
                            if let Some(host) = ev
                                .kv()
                                .and_then(|kv| kv.key_str().ok())
                                .and_then(peer_host_from_key)
                            {
                                if host == self.self_hostname {
                                    continue;
                                }
                                tracing::info!(peer = %host, "etcd_watch: peer lease expired (SUBSTRATE-10)");
                                self.emit(&peer_down_alert(&host));
                            }
                        }
                    }
                }
                msg = leader_stream.message() => {
                    let resp = msg?.ok_or_else(|| anyhow::anyhow!("leader watch stream closed"))?;
                    for ev in resp.events() {
                        match ev.event_type() {
                            EventType::Put => {
                                let new_leader = ev
                                    .kv()
                                    .and_then(|kv| kv.value_str().ok())
                                    .and_then(Lease::decode)
                                    .map(|l| l.node_id);
                                if let Some(alert) = leader_change_alert(last_leader, &new_leader) {
                                    tracing::info!(leader = ?new_leader, "etcd_watch: leadership change (SUBSTRATE-10)");
                                    self.emit(&alert);
                                }
                                *last_leader = new_leader;
                            }
                            // A Delete is the leader lease expiring with no
                            // successor yet — the next campaigner's Put will
                            // carry the new id. Clear the baseline so that Put
                            // reads as a change.
                            EventType::Delete => {
                                *last_leader = None;
                            }
                        }
                    }
                }
            }
        }
    }

    /// Drop one alert JSON into the `alert_relay` watch dir (best-effort — a dir
    /// or write failure is logged via the relay's own absence, never fatal).
    fn emit(&self, alert: &serde_json::Value) {
        if std::fs::create_dir_all(&self.alerts_dir).is_err() {
            return;
        }
        let id = alert
            .get("id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("etcd-watch");
        let path = self.alerts_dir.join(format!("{id}.json"));
        let _ = std::fs::write(path, alert.to_string());
    }
}

#[async_trait::async_trait]
impl Worker for EtcdWatchWorker {
    fn name(&self) -> &'static str {
        "etcd_watch"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        // Threaded across reconnects: the leader id we last announced, so a
        // reconnect doesn't re-fire a handover alert for the sitting leader.
        let mut last_leader: Option<String> = None;
        loop {
            if self.refresh_endpoints {
                self.endpoints = default_endpoints();
            }
            // Pre-cutover (no etcd here): idle on a slow cadence, re-checking for
            // a late `setup-etcd.sh` provision. Never spin.
            if self.endpoints.is_empty() {
                tokio::select! {
                    _ = shutdown.wait() => return Ok(()),
                    () = tokio::time::sleep(IDLE_RECHECK) => continue,
                }
            }
            match self.watch_session(&mut shutdown, &mut last_leader).await {
                Ok(()) => return Ok(()), // shutdown
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "etcd_watch: watch session ended; backing off + reconnecting (§2 degrade)",
                    );
                    // Drop the leader baseline: after an etcd outage the key may
                    // have changed unobserved, so re-seed on reconnect.
                    last_leader = None;
                    tokio::select! {
                        _ = shutdown.wait() => return Ok(()),
                        () = tokio::time::sleep(RECONNECT_BACKOFF) => {}
                    }
                }
            }
        }
    }
}

/// Reduce a watched peer key (`/mesh/peers/<hostname>`) to the bare hostname.
/// `None` when the key doesn't carry the prefix (defensive — etcd only delivers
/// keys under the watched prefix, but a malformed key must never panic).
#[must_use]
pub fn peer_host_from_key(key: &str) -> Option<String> {
    key.strip_prefix(PEERS_PREFIX)
        .filter(|h| !h.is_empty())
        .map(str::to_owned)
}

/// Build the instant peer-down alert for `host`. Shape matches the
/// `alert_relay::AlertEventPartial` the OBS-8 pipeline consumes (id / severity /
/// alert / host / summary). The id carries a minute bucket so a re-fire inside
/// the relay's dedupe window can't double-notify.
#[must_use]
pub fn peer_down_alert(host: &str) -> serde_json::Value {
    let minute = now_minute();
    serde_json::json!({
        "id": format!("etcd-peer-down-{host}-{minute}"),
        "severity": "warn",
        "alert": "mesh.etcd.peer_down",
        "host": host,
        "summary": format!("Peer {host} dropped off etcd (keepalive lease expired)"),
    })
}

/// Build the instant leader-change alert for `new_leader`.
#[must_use]
pub fn leader_change_alert_value(new_leader: &str) -> serde_json::Value {
    let minute = now_minute();
    serde_json::json!({
        "id": format!("etcd-leader-change-{new_leader}-{minute}"),
        "severity": "info",
        "alert": "mesh.etcd.leader_change",
        "host": new_leader,
        "summary": format!("Mesh leader is now {new_leader}"),
    })
}

/// Decide whether a leader watch event is a genuine handover worth alerting.
/// Pure + testable — the heart of "push only the real change, not every renew":
///   * a Put carrying the SAME `node_id` as `last` is a lease renew ⇒ `None`;
///   * a Put carrying a DIFFERENT (or first-seen) `node_id` ⇒ a leader-change alert;
///   * a Put with an undecodable value ⇒ `None` (never alert on garbage).
#[must_use]
pub fn leader_change_alert(
    last: &Option<String>,
    new_leader: &Option<String>,
) -> Option<serde_json::Value> {
    match new_leader {
        Some(new) if last.as_deref() != Some(new.as_str()) => Some(leader_change_alert_value(new)),
        _ => None,
    }
}

/// Current unix-minute bucket — the alert-id dedupe granularity (mirrors
/// `presence_watch::emit`).
fn now_minute() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() / 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_name_is_etcd_watch() {
        let w = EtcdWatchWorker::new(PathBuf::from("/tmp/a"), "self".into());
        assert_eq!(w.name(), "etcd_watch");
    }

    #[test]
    fn peer_host_strips_the_mesh_peers_prefix() {
        assert_eq!(
            peer_host_from_key("/mesh/peers/eagle").as_deref(),
            Some("eagle")
        );
        // A bare / non-prefixed key never yields a host (defensive).
        assert_eq!(peer_host_from_key("/mesh/leader"), None);
        assert_eq!(peer_host_from_key("/mesh/peers/"), None);
    }

    #[test]
    fn peer_down_alert_matches_the_relay_schema() {
        let a = peer_down_alert("anvil");
        assert_eq!(a["severity"], "warn");
        assert_eq!(a["alert"], "mesh.etcd.peer_down");
        assert_eq!(a["host"], "anvil");
        // The relay keys de-dupe off `id`; it MUST be present + name the host.
        let id = a["id"].as_str().unwrap();
        assert!(id.starts_with("etcd-peer-down-anvil-"), "id was {id}");
        assert!(a["summary"].as_str().unwrap().contains("anvil"));
    }

    #[test]
    fn leader_renew_is_silent_only_handover_alerts() {
        // First-seen leader → alert (no prior baseline).
        let first = leader_change_alert(&None, &Some("peer:eagle".into()));
        assert!(first.is_some(), "first-seen leader notifies");
        assert_eq!(first.unwrap()["alert"], "mesh.etcd.leader_change");

        // Same leader renewing its lease → SILENT (the core push-not-spam rule).
        let renew = leader_change_alert(&Some("peer:eagle".into()), &Some("peer:eagle".into()));
        assert!(renew.is_none(), "a lease renew must not re-alert");

        // Genuine handover → alert naming the NEW leader.
        let handover = leader_change_alert(&Some("peer:eagle".into()), &Some("peer:hawk".into()));
        let v = handover.expect("handover notifies");
        assert_eq!(v["host"], "peer:hawk");
        assert!(v["summary"].as_str().unwrap().contains("peer:hawk"));

        // An undecodable / vanished value → never alert on garbage.
        assert!(leader_change_alert(&Some("peer:eagle".into()), &None).is_none());
    }

    #[test]
    fn emit_writes_one_json_keyed_by_id() {
        let dir = tempfile::tempdir().expect("tmp");
        let w =
            EtcdWatchWorker::new(dir.path().to_path_buf(), "self".into()).with_endpoints(vec![]);
        let alert = peer_down_alert("oak");
        w.emit(&alert);
        let id = alert["id"].as_str().unwrap();
        let path = dir.path().join(format!("{id}.json"));
        let body = std::fs::read_to_string(&path).expect("alert file written");
        let back: serde_json::Value = serde_json::from_str(&body).expect("valid json");
        assert_eq!(back["host"], "oak");
        assert_eq!(back["alert"], "mesh.etcd.peer_down");
    }
}
