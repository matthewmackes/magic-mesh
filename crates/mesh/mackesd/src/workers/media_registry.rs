//! MEDIA-7 — register the navidrome/media service into the mesh registry.
//!
//! Runs ONLY on a `Lighthouse_Media` node — it is capability-gated on
//! MEDIA-1's [`Capability::Media`](mde_role::Capability::Media) (worker name
//! `navidrome` in `worker_role::WORKER_CAPABILITIES`), so the spawn site gates
//! it through `worker_role::runs_in("navidrome", deploy_class)` and it is
//! absent on every non-media node.
//!
//! Each tick it:
//!   1. probes the local navidrome instance's health
//!      ([`mesh_media::probe_navidrome`]),
//!   2. builds the registration ([`mesh_media::MediaRegistration`], carrying
//!      the per-instance `health` field), and
//!   3. publishes it into the SAME mesh service registry the other published
//!      services use — the per-peer Bus topic
//!      `mesh/services/media/<peer>` AND the replicated QNM-Shared plane at
//!      `<mount>/<host>/media-registry.json` (the registry plane every node
//!      already reads, alongside `compute-inventory.json` /
//!      `running-apps.json`).
//!
//! Publish cadence mirrors `compute_registry` (BUS-RUN-FULL-1 / ADR-0005):
//! on-change plus a slow heartbeat, so a registry whose body hasn't changed
//! isn't republished every tick, but a late subscriber / pruned topic still
//! finds a recent doc. The QNM-Shared mirror is written every tick (atomic
//! tmp+rename), since a reader of the replicated file always wants the latest.
//!
//! Out of scope (MEDIA-2): the live-stream / bucket acceptance and the running
//! navidrome itself. This is the registry-publish + per-instance-health half —
//! real, spawned, and gated (§7).

#![cfg(feature = "async-services")]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::{ShutdownToken, Worker};
use crate::mesh_media::{self, MediaRegistration};

/// 30 s registry tick — a published service-presence registration is slow-
/// changing (the instance is up or it isn't); 30 s keeps the `health` field
/// fresh without a tight loop. The on-change publish below means an up→down
/// flip still propagates on the very next tick.
pub const DEFAULT_TICK_INTERVAL: Duration = Duration::from_secs(30);

/// Slow heartbeat for the on-change Bus publish — republish an unchanged
/// registration at most this often so a freshly-pruned topic / late subscriber
/// still finds a recent doc. Matches `compute_registry::PUBLISH_HEARTBEAT`'s
/// rationale.
pub const PUBLISH_HEARTBEAT: Duration = Duration::from_secs(300);

/// Publish a media registration to its per-peer Bus topic via the `mde-bus`
/// CLI (typed argv, §9 — no shell). Pub so a future immediate-on-start publish
/// can reuse it. Best-effort: a missing/failing `mde-bus` is logged by the
/// reaper, never fatal.
pub fn publish_registration(reg: &MediaRegistration) {
    let topic = mesh_media::media_registry_topic(&reg.node_id);
    let Ok(body) = serde_json::to_string(reg) else {
        return;
    };
    let mut cmd = Command::new("mde-bus");
    cmd.args(["publish", &topic, "--body-flag", &body]);
    crate::proc_reap::fire_and_reap(cmd, crate::proc_reap::DEFAULT_REAP_TIMEOUT);
}

/// Mirror a node's media registration to the replicated QNM-Shared registry
/// plane at `<mount>/<hostname>/media-registry.json` — the SAME plane the
/// other published services replicate through. Atomic (tmp + rename) so a
/// reader never sees a half-written file. Best-effort: a missing mount / write
/// error is logged, never fatal.
pub fn write_shared_registration(mount: &Path, hostname: &str, reg: &MediaRegistration) {
    if hostname.is_empty() {
        return;
    }
    let dir = mount.join(hostname);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!("media_registry: mkdir {} failed: {e}", dir.display());
        return;
    }
    let Ok(body) = serde_json::to_string(reg) else {
        return;
    };
    let tmp = dir.join("media-registry.json.tmp");
    let final_path = dir.join(mesh_media::MEDIA_REGISTRY_FILE);
    if let Err(e) = std::fs::write(&tmp, body.as_bytes()) {
        tracing::warn!("media_registry: write {} failed: {e}", tmp.display());
        return;
    }
    if let Err(e) = std::fs::rename(&tmp, &final_path) {
        tracing::warn!("media_registry: rename registration failed: {e}");
    }
}

/// Worker handle.
pub struct MediaRegistryWorker {
    /// Registering node-id (the registry key / topic suffix).
    node_id: String,
    /// Hostname the QNM-Shared mirror is keyed under.
    hostname: String,
    /// Navidrome port to probe + register.
    port: u16,
    /// Registry tick cadence.
    tick: Duration,
    /// Replicated QNM-Shared registry root.
    mount: PathBuf,
    /// Slow heartbeat for the on-change Bus publish.
    publish_heartbeat: Duration,
    /// Last published body + when, so we publish on-change and only
    /// heartbeat-republish an unchanged registration.
    last_publish: Mutex<Option<(String, Instant)>>,
}

impl MediaRegistryWorker {
    /// Construct with production defaults. `node_id` is the registry key;
    /// `hostname` keys the QNM-Shared mirror.
    #[must_use]
    pub fn new(node_id: String, hostname: String) -> Self {
        Self {
            node_id,
            hostname,
            port: mesh_media::NAVIDROME_PORT,
            tick: DEFAULT_TICK_INTERVAL,
            mount: crate::default_qnm_shared_root(),
            publish_heartbeat: PUBLISH_HEARTBEAT,
            last_publish: Mutex::new(None),
        }
    }

    /// Override the replicated registry root. Used in tests + to honor a
    /// `--workgroup-root` override at the spawn site (so the worker writes
    /// where the registry readers look).
    #[must_use]
    pub fn with_mount(mut self, p: PathBuf) -> Self {
        self.mount = p;
        self
    }

    /// Override the probed/registered port. Used in tests.
    #[must_use]
    pub fn with_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// Build this tick's registration from a live health probe.
    fn build_registration(&self) -> MediaRegistration {
        let health = mesh_media::probe_navidrome(self.port);
        mesh_media::registration(&self.node_id, self.port, &health)
    }

    fn tick_once(&self) {
        let reg = self.build_registration();
        // Bus publish: on-change + slow heartbeat (BUS-RUN-FULL-1). Serialize
        // once and compare against the last published body.
        if let Ok(body) = serde_json::to_string(&reg) {
            let mut last = self
                .last_publish
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let now = Instant::now();
            let prev_body = last.as_ref().map(|(b, _)| b.as_str());
            let prev_at = last.as_ref().map(|(_, at)| *at);
            if crate::workers::compute_registry::should_publish(
                prev_body,
                &body,
                prev_at,
                now,
                self.publish_heartbeat,
            ) {
                publish_registration(&reg);
                *last = Some((body, now));
            }
        }
        // QNM-Shared mirror: the replicated registry plane every node reads.
        // Written every tick (the reader always wants the latest); the helper
        // is a no-op when the mount/host is absent.
        write_shared_registration(&self.mount, &self.hostname, &reg);
    }
}

#[async_trait::async_trait]
impl Worker for MediaRegistryWorker {
    fn name(&self) -> &'static str {
        "media_registry"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(self.tick) => {
                    self.tick_once();
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
    use crate::mesh_media::{HEALTH_DOWN, NAVIDROME_KIND};

    #[test]
    fn build_registration_probes_health_and_pins_kind() {
        // Port 1 is unbound → the probe degrades to `down`; the worker still
        // registers the navidrome kind + the configured port (registry is
        // service-presence: the instance is declared, health says it's down).
        let w = MediaRegistryWorker::new("peer:eagle".into(), "eagle".into()).with_port(1);
        let reg = w.build_registration();
        assert_eq!(reg.node_id, "peer:eagle");
        assert_eq!(reg.kind, NAVIDROME_KIND);
        assert_eq!(reg.port, 1);
        assert_eq!(reg.health, HEALTH_DOWN);
    }

    #[test]
    fn shared_mirror_writes_atomic_registry_file() {
        let tmp = std::env::temp_dir().join(format!("mde-mediareg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let reg = mesh_media::registration("peer:eagle", mesh_media::NAVIDROME_PORT, "up");
        write_shared_registration(&tmp, "eagle", &reg);
        let path = tmp.join("eagle").join(mesh_media::MEDIA_REGISTRY_FILE);
        let body = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_dir_all(&tmp);
        let back: MediaRegistration = serde_json::from_str(&body).unwrap();
        assert_eq!(back, reg);
        // No leftover tmp file (atomic rename consumed it).
        assert!(!tmp.join("eagle").join("media-registry.json.tmp").exists());
    }

    #[test]
    fn shared_mirror_skips_empty_hostname() {
        let tmp = std::env::temp_dir().join(format!("mde-mediareg-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let reg = mesh_media::registration("peer:x", mesh_media::NAVIDROME_PORT, "down");
        write_shared_registration(&tmp, "", &reg);
        // Nothing written for an empty hostname.
        assert!(!tmp.exists());
    }
}
