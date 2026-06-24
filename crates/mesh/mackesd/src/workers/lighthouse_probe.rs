//! LIGHTHOUSE-8 — per-lighthouse deep-probe lane.
//!
//! The replicated peer directory ([`mackes_mesh_types::lighthouse`]) carries a
//! lighthouse's *binary* health (online / overlay-up / master-service ok). It
//! deliberately does NOT carry the live operational facts an operator wants when
//! a beacon goes red. This worker fills that gap: every ~15 s it probes EACH
//! lighthouse for
//!
//!   * **Nebula handshake** — is the local node's tunnel to the lighthouse
//!     established (live hostmap entry) and is the overlay address answering;
//!   * **public IP** — the lighthouse's dialable underlay `ip:port`
//!     (`external_addr` from the directory, or the chosen remote endpoint from
//!     the live hostmap);
//!   * **peer count** — how many overlay peers the mesh carries (the membership
//!     a lighthouse anchors), from the replicated directory;
//!   * **uptime** — how long the lighthouse's directory row has been
//!     continuously fresh (observed first-seen → now);
//!   * **CA cert expiry** — days until the mesh CA cert (every lighthouse cert
//!     is signed under it) reaches `notAfter`.
//!
//! and publishes one [`LighthouseProbe`] per lighthouse to
//! `compute/lighthouse-probe/<name>` on the mde-bus. The Workbench Lighthouses
//! tab subscribes + renders the five fields in each card.
//!
//! **GLUE, not a reimplementation.** Handshake/path come from
//! [`crate::nebula_admin`] (Nebula's debug-SSH hostmap, the same source
//! `mesh_latency` uses), overlay reachability from [`crate::transport_probe`],
//! cert expiry from [`crate::ca::expiry`] against the CA the mesh already
//! manages, and the lighthouse set + public addrs from the replicated directory
//! ([`mackes_mesh_types::lighthouse`]). Every unmeasurable field degrades to
//! `None` (the card renders `—`) — never a stub, never a guess (§7).

#![cfg(feature = "async-services")]

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mackes_mesh_types::lighthouse;
use mackes_mesh_types::lighthouse_probe::LighthouseProbe;
use mackes_mesh_types::peers::{self, PeerRecord};

use super::{ShutdownToken, Worker};

/// Default probe cadence — every 15 s (the unit spec).
pub const DEFAULT_PROBE_INTERVAL: Duration = Duration::from_secs(15);

/// Default `mde-bus` CLI name (resolved on `PATH`; overridable via `MDE_BUS_BIN`
/// for a dev tree, mirroring `bus_supervisor`).
const DEFAULT_BUS_BIN: &str = "mde-bus";

/// LIGHTHOUSE-8 worker — probes every lighthouse each tick + publishes a
/// [`LighthouseProbe`] per lighthouse to the bus.
pub struct LighthouseProbeWorker {
    /// QNM-Shared root (the fs fallback directory the peer records live under).
    workgroup_root: PathBuf,
    /// Path to the mesh CA cert, for the cert-expiry probe.
    ca_cert_path: PathBuf,
    /// Probe cadence.
    interval: Duration,
    /// Per-lighthouse first-seen wall-clock (ms) the worker has observed the row
    /// continuously fresh from — the uptime anchor. Reset when a lighthouse goes
    /// stale/offline so a flap restarts the count honestly.
    first_seen_ms: HashMap<String, u64>,
}

impl LighthouseProbeWorker {
    /// Construct with production defaults (15 s cadence, the managed CA cert).
    #[must_use]
    pub fn new(workgroup_root: PathBuf) -> Self {
        Self {
            workgroup_root,
            ca_cert_path: PathBuf::from(crate::ca::DEFAULT_CA_CERT_PATH),
            interval: DEFAULT_PROBE_INTERVAL,
            first_seen_ms: HashMap::new(),
        }
    }

    /// Override the probe cadence (tests / fast-cadence debugging).
    #[must_use]
    pub fn with_interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }

    /// Override the CA cert path (tests).
    #[must_use]
    pub fn with_ca_cert(mut self, path: PathBuf) -> Self {
        self.ca_cert_path = path;
        self
    }
}

#[async_trait::async_trait]
impl Worker for LighthouseProbeWorker {
    fn name(&self) -> &'static str {
        "lighthouse_probe"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let mut tick = tokio::time::interval(self.interval);
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    let root = self.workgroup_root.clone();
                    let ca = self.ca_cert_path.clone();
                    // The probe shells out (nebula debug-SSH, transport TCP,
                    // nebula-cert) — run it off the async executor so a slow
                    // edge can't stall the runtime. The hostmap + cert reads
                    // are shared across all lighthouses in one pass.
                    let (mut probes, seen) = tokio::task::spawn_blocking(move || {
                        probe_all(&root, &ca, now_ms())
                    })
                    .await
                    .unwrap_or_default();
                    self.update_uptime(&mut probes, &seen);
                    for probe in &probes {
                        publish_probe(DEFAULT_BUS_BIN, probe);
                    }
                }
                _ = shutdown.wait() => break,
            }
        }
        Ok(())
    }
}

impl LighthouseProbeWorker {
    /// Fold this tick's freshly-seen lighthouses into the first-seen tracker +
    /// stamp each probe's `uptime_s`. A lighthouse seen fresh keeps its anchor;
    /// a lighthouse that fell off `seen` (went stale/offline) drops its anchor so
    /// the next bring-up restarts the count.
    ///
    /// The anchor is the wall-clock instant this node FIRST observed the row
    /// fresh (`probed_at_ms`), so `uptime_s` is "uptime as observed here",
    /// starting at 0 on first sight — the node can't know how long the
    /// lighthouse ran before it started observing, and anchoring to the row's
    /// `last_seen_ms` would mis-report up to a stale-window of seconds as uptime.
    fn update_uptime(&mut self, probes: &mut [LighthouseProbe], seen: &HashMap<String, u64>) {
        // Drop anchors for lighthouses no longer fresh this tick.
        self.first_seen_ms.retain(|name, _| seen.contains_key(name));
        for probe in probes.iter_mut() {
            if seen.contains_key(&probe.name) {
                let anchor = *self
                    .first_seen_ms
                    .entry(probe.name.clone())
                    .or_insert(probe.probed_at_ms);
                probe.uptime_s = Some(probe.probed_at_ms.saturating_sub(anchor) / 1000);
            }
        }
    }
}

/// Read the lighthouse set (etcd-or-fs, same source the daemon's directory uses)
/// and probe each one, returning the probes plus a `name → last_seen_ms` map of
/// the lighthouses that are fresh *this tick* (the uptime tracker's input).
///
/// Pure-ish: all I/O is subprocess/file reads with honest degradation; no panic.
#[must_use]
fn probe_all(
    workgroup_root: &std::path::Path,
    ca_cert_path: &std::path::Path,
    now_ms: u64,
) -> (Vec<LighthouseProbe>, HashMap<String, u64>) {
    let peers = read_directory_peers(workgroup_root);
    let lighthouses = lighthouse::lighthouse_records(&peers);

    // One hostmap query for the whole tick (joined to each lighthouse by name).
    let hostmap = crate::nebula_admin::query_tunnels_default();
    // One CA-cert read for the whole tick (shared trust anchor).
    let now_unix = i64::try_from(now_ms / 1000).unwrap_or(i64::MAX);
    let cert_expiry_days = crate::ca::expiry::ca_cert_days_remaining(ca_cert_path, now_unix);
    // The overlay membership the lighthouse set anchors: the mesh's peer count.
    // Mesh-wide (the directory doesn't attribute peers to a single lighthouse).
    let peer_count = u32::try_from(peers.len()).ok();

    let mut probes = Vec::with_capacity(lighthouses.len());
    let mut seen = HashMap::new();
    for lh in &lighthouses {
        // Fresh this tick ⇒ feeds the uptime tracker; a stale (offline) row is
        // still probed (so the card shows its degraded state) but doesn't anchor
        // uptime.
        let fresh = now_ms.saturating_sub(lh.last_seen_ms) < lighthouse::DEFAULT_STALE_MS;
        if fresh {
            seen.insert(lh.hostname.clone(), lh.last_seen_ms);
        }
        probes.push(probe_one(
            lh,
            &hostmap,
            cert_expiry_days,
            peer_count,
            now_ms,
        ));
    }
    (probes, seen)
}

/// Build one lighthouse's probe from the directory row + the shared hostmap /
/// cert / peer-count facts. `uptime_s` is left `None` here — the worker stamps
/// it from its cross-tick first-seen tracker. Pure + unit-tested.
#[must_use]
fn probe_one(
    lh: &PeerRecord,
    hostmap: &HashMap<String, crate::nebula_admin::TunnelPath>,
    cert_expiry_days: Option<i64>,
    peer_count: Option<u32>,
    now_ms: u64,
) -> LighthouseProbe {
    let mut probe = LighthouseProbe::unmeasured(&lh.hostname, now_ms);
    probe.overlay_ip = lh.overlay_ip.clone();
    probe.cert_expiry_days = cert_expiry_days;
    probe.peer_count = peer_count;

    let tunnel = hostmap.get(&lh.hostname);

    // Public IP: the lighthouse's advertised underlay addr wins (the dialable
    // address peers enroll through); else the chosen remote endpoint from the
    // live hostmap (when we hold a direct tunnel).
    probe.public_ip = lh
        .external_addr
        .clone()
        .filter(|a| !a.is_empty())
        .or_else(|| tunnel.and_then(|t| t.endpoint.clone()));

    // Handshake: derivable only when the lighthouse has an overlay IP. A live
    // hostmap entry means a tunnel is established; otherwise fall back to a
    // transport reachability probe (the overlay answered ⇒ reachable but no
    // classified tunnel). No overlay IP ⇒ unknown (`None`).
    probe.handshake = match &lh.overlay_ip {
        Some(ip) if !ip.is_empty() => Some(if tunnel.is_some() {
            true
        } else {
            crate::transport_probe::probe_rtt(ip).reachable
        }),
        _ => None,
    };

    probe
}

/// Read peers from the etcd substrate when provisioned, else the replicated fs
/// directory (`<workgroup>/peers/*.json`) — the same etcd-or-fs precedence the
/// health reconciler uses, so the probe sees the canonical directory.
#[must_use]
fn read_directory_peers(workgroup_root: &std::path::Path) -> Vec<PeerRecord> {
    let eps = crate::substrate::etcd::default_endpoints();
    if !eps.is_empty() {
        if let Some(rows) = crate::substrate::peers::read_peers_blocking(&eps) {
            return rows;
        }
    }
    peers::read_peers(&peers::peers_dir(workgroup_root))
}

/// Publish one probe to `compute/lighthouse-probe/<name>` via the `mde-bus`
/// CLI (the same fire-and-reap path `compute_registry` / `voip_rtt` use). The
/// JSON body is the serialized [`LighthouseProbe`]; the workbench reads it back
/// off the bus spool. Best-effort: a missing/un-invocable `mde-bus` is a no-op.
fn publish_probe(bus_bin: &str, probe: &LighthouseProbe) {
    let topic = LighthouseProbe::topic(&probe.name);
    let Ok(body) = serde_json::to_string(probe) else {
        return;
    };
    let bin = std::env::var("MDE_BUS_BIN").unwrap_or_else(|_| bus_bin.to_string());
    let mut cmd = std::process::Command::new(bin);
    cmd.args(["publish", &topic, "--body-flag", &body]);
    crate::proc_reap::fire_and_reap(cmd, crate::proc_reap::DEFAULT_REAP_TIMEOUT);
}

/// Wall-clock epoch milliseconds.
fn now_ms() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0),
    )
    .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lighthouse(host: &str, last_seen_ms: u64) -> PeerRecord {
        let mut p = PeerRecord::now(host, Some("11.0.5".into()), "healthy");
        p.last_seen_ms = last_seen_ms;
        p.role = Some(lighthouse::LIGHTHOUSE_ROLE.to_string());
        p.overlay_ip = Some("10.42.0.5".to_string());
        p.external_addr = Some("203.0.113.5:4242".to_string());
        p
    }

    #[test]
    fn worker_name_is_stable() {
        let w = LighthouseProbeWorker::new(PathBuf::from("/tmp/x"));
        assert_eq!(w.name(), "lighthouse_probe");
    }

    #[test]
    fn probe_one_prefers_external_addr_for_public_ip() {
        let lh = lighthouse("anvil", 1_000);
        let hostmap = HashMap::new();
        let p = probe_one(&lh, &hostmap, Some(180), Some(4), 2_000);
        assert_eq!(p.name, "anvil");
        assert_eq!(p.overlay_ip.as_deref(), Some("10.42.0.5"));
        assert_eq!(p.public_ip.as_deref(), Some("203.0.113.5:4242"));
        assert_eq!(p.cert_expiry_days, Some(180));
        assert_eq!(p.peer_count, Some(4));
        // Uptime is stamped by the worker tracker, not probe_one.
        assert!(p.uptime_s.is_none());
    }

    #[test]
    fn probe_one_uses_hostmap_endpoint_when_no_external_addr() {
        let mut lh = lighthouse("anvil", 1_000);
        lh.external_addr = None;
        let mut hostmap = HashMap::new();
        hostmap.insert(
            "anvil".to_string(),
            crate::nebula_admin::TunnelPath {
                name: "anvil".to_string(),
                endpoint: Some("198.51.100.9:4242".to_string()),
                relay_via: None,
            },
        );
        let p = probe_one(&lh, &hostmap, None, Some(2), 2_000);
        assert_eq!(p.public_ip.as_deref(), Some("198.51.100.9:4242"));
        // A live hostmap entry ⇒ handshake established.
        assert_eq!(p.handshake, Some(true));
        // No nebula-cert in the test env ⇒ cert expiry degrades to None.
        assert!(p.cert_expiry_days.is_none());
    }

    #[test]
    fn probe_one_handshake_is_none_without_overlay_ip() {
        let mut lh = lighthouse("anvil", 1_000);
        lh.overlay_ip = None;
        let hostmap = HashMap::new();
        let p = probe_one(&lh, &hostmap, None, None, 2_000);
        assert!(p.handshake.is_none());
        assert!(p.overlay_ip.is_none());
    }

    #[test]
    fn update_uptime_anchors_first_seen_and_resets_on_flap() {
        let mut w = LighthouseProbeWorker::new(PathBuf::from("/tmp/x"));

        // Tick 1: anvil first observed (probed at 12_000) → uptime anchors HERE,
        // so 0s up (we can't know how long it ran before this node saw it).
        let mut probes = vec![{
            let mut p = LighthouseProbe::unmeasured("anvil", 12_000);
            p.overlay_ip = Some("10.42.0.5".into());
            p
        }];
        let mut seen = HashMap::new();
        seen.insert("anvil".to_string(), 10_000u64);
        w.update_uptime(&mut probes, &seen);
        assert_eq!(probes[0].uptime_s, Some(0));

        // Tick 2: still fresh, probed later → uptime grows from the SAME anchor
        // (the 12_000 first-observation instant): (22_000 - 12_000)/1000 = 10s.
        let mut probes2 = vec![LighthouseProbe::unmeasured("anvil", 22_000)];
        w.update_uptime(&mut probes2, &seen);
        assert_eq!(probes2[0].uptime_s, Some(10));

        // Tick 3: anvil fell off `seen` (went stale) → anchor dropped, no uptime.
        let mut probes3 = vec![LighthouseProbe::unmeasured("anvil", 30_000)];
        let empty = HashMap::new();
        w.update_uptime(&mut probes3, &empty);
        assert!(probes3[0].uptime_s.is_none());
        assert!(!w.first_seen_ms.contains_key("anvil"));

        // Tick 4: anvil back → fresh anchor at this observation, count restarts.
        let mut probes4 = vec![LighthouseProbe::unmeasured("anvil", 41_000)];
        let mut seen4 = HashMap::new();
        seen4.insert("anvil".to_string(), 40_000u64);
        w.update_uptime(&mut probes4, &seen4);
        assert_eq!(probes4[0].uptime_s, Some(0));
    }

    #[test]
    fn probe_all_reads_fs_directory_and_skips_non_lighthouses() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let pdir = peers::peers_dir(root);
        let now = now_ms();

        // A lighthouse + a plain peer + a stale lighthouse.
        let lh = lighthouse("anvil", now);
        peers::write_peer_record(&pdir, &lh).expect("write lighthouse");
        let mut plain = PeerRecord::now("forge", Some("11.0.5".into()), "healthy");
        plain.last_seen_ms = now;
        peers::write_peer_record(&pdir, &plain).expect("write peer");
        let stale = lighthouse(
            "relic",
            now.saturating_sub(lighthouse::DEFAULT_STALE_MS + 5_000),
        );
        peers::write_peer_record(&pdir, &stale).expect("write stale lighthouse");

        let (probes, seen) = probe_all(root, std::path::Path::new("/nonexistent/ca.crt"), now);
        // Two lighthouses probed (anvil + relic); the plain peer is excluded.
        let names: Vec<&str> = probes.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"anvil"));
        assert!(names.contains(&"relic"));
        assert!(!names.contains(&"forge"));
        // peer_count counts the whole mesh membership (3 records).
        assert!(probes.iter().all(|p| p.peer_count == Some(3)));
        // Only the fresh lighthouse is in `seen` (the uptime input).
        assert!(seen.contains_key("anvil"));
        assert!(!seen.contains_key("relic"));
        // No nebula-cert in the test env ⇒ cert expiry degrades to None.
        assert!(probes.iter().all(|p| p.cert_expiry_days.is_none()));
    }

    #[tokio::test]
    async fn worker_exits_on_shutdown_token() {
        let mut w = LighthouseProbeWorker::new(PathBuf::from("/tmp/lh-probe-test"))
            .with_interval(Duration::from_millis(50))
            .with_ca_cert(PathBuf::from("/nonexistent/ca.crt"));
        let (tx, rx) = tokio::sync::watch::channel(false);
        let token = ShutdownToken::from_receiver(rx);
        let _ = tx.send(true);
        let result = tokio::time::timeout(Duration::from_secs(3), w.run(token))
            .await
            .expect("worker must exit on shutdown");
        assert!(result.is_ok());
    }
}
