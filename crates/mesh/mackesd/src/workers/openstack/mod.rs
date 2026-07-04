//! QC-2 (QUASAR-CLOUD) — the mackesd `openstack` worker: the supervision
//! root of the mesh-becomes-an-OpenStack-cloud epic
//! (`docs/design/quasar-cloud.md`, locked 2026-07-03).
//!
//! Every MCNF node is a universal `OpenStack` node (Q1/Q5/Q22): this worker
//! runs Red-Hat-convention **Kolla service containers** under Podman on
//! whatever the fleet's one-state doctrine says this node hosts — APIs on
//! every node, leader-hosted `MariaDB`, no controller box — and owns their
//! supervision (Q20), reporting over the Bus.
//!
//! ## Shape (mirrors the `container`/`storage`/`chat` worker architecture)
//!
//! - **Two injectable seams**, both headless-testable with fakes:
//!   [`fleet::FleetStateSource`] (WHICH services — the one-state doctrine
//!   read; QC-4 wired the live leg: the TOML companion on the Syncthing share
//!   plus the leader bit off the `/mesh/leader` lease) and
//!   [`podman::PodmanRunner`] (HOW they run — production shells
//!   `podman` through the bounded proc path; a podman-less host answers a
//!   typed [`podman::RunnerError::PodmanAbsent`]).
//! - **A pure reconcile core** ([`reconcile::plan_converge`] +
//!   [`reconcile::converge_cycle`]): desired vs running → start missing /
//!   restart killed / stop extra. A start's image is satisfied locally or
//!   loaded from the mesh share's operator-mirrored, checksum-verified
//!   archive ([`images`] — QC-3's Syncthing lane; design Q18, no registry
//!   on the airgapped fleet), and additionally gated on the rendered Kolla
//!   config (QC-4's renderer); a gated doctrine converges nothing.
//! - **The `state/openstack/<node>` mirror** ([`reconcile::OpenStackState`])
//!   — the same per-node Bus mirror idiom `state/storage/<node>` uses,
//!   published on change + heartbeat via the `mde-bus` CLI fire-and-reap
//!   path. `[!]`-grade converge failures ride the `mackesd::alert` lane,
//!   which the chat worker folds into the mesh chat (NOTIFY-CHAT lock 11).
//!
//! ## What later QC slices extend (the module map)
//!
//! - [`catalog`] — the service vocabulary: QC-6 binds the API entries to
//!   the Nebula interface, wave-2 services (Q25) land as new variants.
//! - [`fleet`] — QC-4 wired the live doctrine read (landed): the TOML
//!   companion on the Syncthing share + the leader bit off the `/mesh/leader`
//!   lease; QC-10's capacity-derived quotas read the same doctrine.
//! - [`config_render`] — QC-4's one-state → Kolla config renderer (landed):
//!   materializes each desired service's `/etc/kolla/<svc>/config.json` (+ the
//!   service config it points to) from the doctrine, atomically. QC-5 seals the
//!   real per-service credentials it substitutes.
//! - [`secrets`] — QC-5's sealed per-service secret set (the leader mints once
//!   from the OS CSPRNG, seals `0600` on the Syncthing share, every other node
//!   reads it; the renderer substitutes it for QC-4's placeholder).
//! - [`images`] — QC-3's airgap archive lane (landed): the decided
//!   `<share>/kolla/<release>/` layout + `SHA256SUMS` verification; QC-9's
//!   DIB pipeline reuses the same share conventions for tenant images.
//! - [`podman`] — QC-5 grows per-service health probes.
//! - [`reconcile`] — QC-5 extends the mirror with API health; QC-11's typed
//!   verbs consume the same state model.

#![cfg(feature = "async-services")]

pub mod catalog;
pub mod config_render;
pub mod fleet;
pub mod images;
pub mod podman;
pub mod reconcile;
pub mod secrets;
#[cfg(test)]
pub(crate) mod testkit;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::nebula_supervisor::DEFAULT_OVERLAY_IP_PATH;
use super::{ShutdownToken, Worker};

use config_render::OverlayBind;
use fleet::{FleetStateSource, MeshFleetState};
use podman::{PodmanCli, PodmanRunner, DEFAULT_KOLLA_CONFIG_ROOT};
use reconcile::{converge_cycle, CycleOutcome, OpenStackState};

/// Converge cadence.
///
/// One `podman ps` (+ any mutations) per tick — the same order of cost as
/// the sibling `container` worker's heartbeat, and fast enough that a killed
/// service container is back within seconds.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(15);

/// Unconditional mirror republish cadence.
///
/// Between heartbeats the mirror is published only when its content changed,
/// so a freshly-pruned topic / late subscriber still finds a recent row
/// without the Bus filling with identical bodies.
pub const PUBLISH_HEARTBEAT: Duration = Duration::from_secs(60);

/// The per-node mirror topic: `state/openstack/<node>`.
#[must_use]
pub fn state_topic(node: &str) -> String {
    format!("state/openstack/{node}")
}

/// Publish a JSON body to `topic` via the `mde-bus` CLI — the same
/// fire-and-reap path the `container`/`storage` workers use. Best-effort: a
/// missing `mde-bus` binary (pre-RPM dev box) is swallowed, and the detached
/// reaper prevents a zombie pile.
fn publish_json<T: serde::Serialize>(topic: &str, body: &T) {
    let Ok(json) = serde_json::to_string(body) else {
        return;
    };
    let mut cmd = Command::new("mde-bus");
    cmd.args(["publish", topic, "--body-flag", &json]);
    crate::proc_reap::fire_and_reap(cmd, crate::proc_reap::DEFAULT_REAP_TIMEOUT);
}

/// Resolve this node's Nebula overlay bind address (QC-6, Q22/23) from the
/// canonical publish file `nebula_supervisor` writes on every signed-bundle
/// refresh (`DEFAULT_OVERLAY_IP_PATH`) — the same source of truth
/// `sshd_overlay_bind`/`cups_sync`/`boot_readiness` bind their listeners to.
///
/// A missing or empty file means the node isn't on the mesh yet (pre-enrollment
/// / fresh dev box): an honest [`OverlayBind::Unresolved`] that gates every
/// service's render, never a `0.0.0.0`/localhost fallback that would put a
/// control-plane API on the public underlay (§7).
fn resolve_overlay(path: &Path) -> OverlayBind {
    match std::fs::read_to_string(path) {
        Ok(s) if !s.trim().is_empty() => OverlayBind::Resolved(s.trim().to_string()),
        _ => OverlayBind::Unresolved(format!(
            "overlay address unresolved — node not on the mesh (no overlay IP published at \
             {} yet; enroll the node so nebula_supervisor writes it)",
            path.display()
        )),
    }
}

/// The QC-2 `openstack` worker.
pub struct OpenstackWorker {
    /// This node's id — the mirror topic namespace + `host` stamp.
    host: String,
    /// The doctrine seam (production: [`MeshFleetState`] — QC-4's live read
    /// off the Syncthing share + `/mesh/leader` lease).
    fleet: Arc<dyn FleetStateSource + Send + Sync>,
    /// The podman seam (production: [`PodmanCli`]). `Arc` so each cycle runs
    /// on a `spawn_blocking` thread without borrowing `self`.
    runner: Arc<dyn PodmanRunner + Send + Sync>,
    /// The Kolla config root ([`DEFAULT_KOLLA_CONFIG_ROOT`]; tests point it
    /// at a tempdir).
    config_root: PathBuf,
    /// The mesh share root the QC-3 archive lane reads
    /// (production: the same replicated workgroup root the doctrine seam
    /// rides — `/mnt/mesh-storage`; tests point it at a tempdir).
    share_root: PathBuf,
    /// The canonical overlay-IP publish file QC-6 resolves the API bind address
    /// from (production: [`DEFAULT_OVERLAY_IP_PATH`]; tests point it at a temp
    /// file or leave it absent to exercise the honest unresolved gate).
    overlay_ip_path: PathBuf,
    /// Converge cadence.
    poll: Duration,
    /// Mirror republish heartbeat.
    heartbeat: Duration,
}

impl OpenstackWorker {
    /// Construct with production defaults: the live [`MeshFleetState`]
    /// doctrine seam over `host` + `workgroup_root`, the live [`PodmanCli`]
    /// runner,
    /// the `/etc/kolla` config root, `workgroup_root` doubling as the QC-3
    /// archive share (it IS the Syncthing-replicated `/mnt/mesh-storage`),
    /// and the default cadences. `host` is this node's id (the mirror
    /// `host` stamp).
    #[must_use]
    pub fn new(host: String, workgroup_root: PathBuf) -> Self {
        Self {
            fleet: Arc::new(MeshFleetState::new(host.clone(), workgroup_root.clone())),
            host,
            runner: Arc::new(PodmanCli::new()),
            config_root: PathBuf::from(DEFAULT_KOLLA_CONFIG_ROOT),
            share_root: workgroup_root,
            overlay_ip_path: PathBuf::from(DEFAULT_OVERLAY_IP_PATH),
            poll: DEFAULT_POLL_INTERVAL,
            heartbeat: PUBLISH_HEARTBEAT,
        }
    }

    /// Inject a doctrine source (tests).
    #[must_use]
    pub fn with_fleet(mut self, fleet: Arc<dyn FleetStateSource + Send + Sync>) -> Self {
        self.fleet = fleet;
        self
    }

    /// Inject a podman runner (tests).
    #[must_use]
    pub fn with_runner(mut self, runner: Arc<dyn PodmanRunner + Send + Sync>) -> Self {
        self.runner = runner;
        self
    }

    /// Override the Kolla config root (tests).
    #[must_use]
    pub fn with_config_root(mut self, root: PathBuf) -> Self {
        self.config_root = root;
        self
    }

    /// Override the QC-3 archive share root (tests).
    #[must_use]
    pub fn with_share_root(mut self, root: PathBuf) -> Self {
        self.share_root = root;
        self
    }

    /// Override the overlay-IP publish file QC-6 resolves the bind address from
    /// (tests).
    #[must_use]
    pub fn with_overlay_ip_path(mut self, path: PathBuf) -> Self {
        self.overlay_ip_path = path;
        self
    }

    /// Override the converge cadence (tests, to avoid multi-second waits).
    #[must_use]
    pub const fn with_poll(mut self, poll: Duration) -> Self {
        self.poll = poll;
        self
    }

    /// Run one converge cycle on a blocking thread (podman + doctrine I/O
    /// never pins the async runtime), surface its alerts, and publish the
    /// mirror when the content changed or the heartbeat elapsed.
    async fn cycle_and_publish(
        &self,
        last: &mut Option<OpenStackState>,
        last_pub_at: &mut Option<Instant>,
    ) {
        let fleet = Arc::clone(&self.fleet);
        let runner = Arc::clone(&self.runner);
        let config_root = self.config_root.clone();
        let share_root = self.share_root.clone();
        let host = self.host.clone();
        // QC-6 — resolve this node's overlay bind each tick (it may come up
        // after the worker starts, once the node enrolls); an unresolved overlay
        // gates every start honestly in the converge.
        let overlay = resolve_overlay(&self.overlay_ip_path);
        let outcome: CycleOutcome = match tokio::task::spawn_blocking(move || {
            converge_cycle(
                &*fleet,
                &*runner,
                &config_root,
                &share_root,
                &host,
                &overlay,
            )
        })
        .await
        {
            Ok(outcome) => outcome,
            Err(e) => {
                tracing::warn!(error = %e, "openstack: converge task join failed");
                return;
            }
        };
        // `[!]`-grade failures ride the alert lane (→ chat, lock 11).
        for alert in &outcome.alerts {
            tracing::warn!(target: "mackesd::alert", "ALERT (warn): {alert}");
        }
        let now = Instant::now();
        let changed = last
            .as_ref()
            .is_none_or(|prev| !prev.same_ignoring_time(&outcome.state));
        let heartbeat_due = last_pub_at.is_none_or(|at| now.duration_since(at) >= self.heartbeat);
        if changed || heartbeat_due {
            publish_json(&state_topic(&self.host), &outcome.state);
            *last_pub_at = Some(now);
        }
        *last = Some(outcome.state);
    }
}

#[async_trait::async_trait]
impl Worker for OpenstackWorker {
    fn name(&self) -> &'static str {
        "openstack"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let mut last: Option<OpenStackState> = None;
        let mut last_pub_at: Option<Instant> = None;
        // Converge + publish immediately on start so a panel doesn't wait a
        // full tick for the first mirror row.
        self.cycle_and_publish(&mut last, &mut last_pub_at).await;
        let mut tick = tokio::time::interval(self.poll);
        tick.tick().await; // consume the immediate first tick
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    self.cycle_and_publish(&mut last, &mut last_pub_at).await;
                }
                () = shutdown.wait() => break,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::testkit::{FakeFleet, FakeRunner};
    use super::*;

    #[test]
    fn mirror_topic_is_namespaced_per_node() {
        assert_eq!(state_topic("node-a"), "state/openstack/node-a");
        assert!(state_topic("x").starts_with("state/"));
    }

    #[test]
    fn worker_name_matches_module_and_census() {
        let w = OpenstackWorker::new("node".to_string(), PathBuf::from("/tmp"));
        assert_eq!(w.name(), "openstack");
    }

    #[test]
    fn resolve_overlay_reads_the_publish_file_and_trims() {
        // QC-6 — a published overlay IP resolves to a bind address.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("overlay-ip");
        std::fs::write(&path, "10.42.0.9\n").unwrap();
        assert_eq!(
            resolve_overlay(&path),
            OverlayBind::Resolved("10.42.0.9".to_string())
        );
    }

    #[test]
    fn resolve_overlay_gates_when_absent_or_empty() {
        // §7 — a node not on the mesh (no file, or a blank one mid-provision)
        // resolves to the honest unresolved gate, never a fabricated bind.
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope");
        let OverlayBind::Unresolved(reason) = resolve_overlay(&missing) else {
            unreachable!("absent file must be unresolved");
        };
        assert!(reason.contains("overlay address unresolved"), "{reason}");
        assert!(reason.contains("not on the mesh"), "{reason}");

        let blank = dir.path().join("overlay-ip");
        std::fs::write(&blank, "   \n").unwrap();
        assert!(matches!(
            resolve_overlay(&blank),
            OverlayBind::Unresolved(_)
        ));
    }

    #[test]
    fn overlay_ip_path_defaults_to_the_canonical_publish_file() {
        let w = OpenstackWorker::new("node".to_string(), PathBuf::from("/tmp"));
        assert_eq!(w.overlay_ip_path, PathBuf::from(DEFAULT_OVERLAY_IP_PATH));
    }

    #[tokio::test]
    async fn tick_loop_exits_on_shutdown() {
        // Drives run() with the gated fake doctrine + an empty fake runner
        // (no live podman, no etcd) and exits promptly on shutdown.
        let (tx, rx) = tokio::sync::watch::channel(false);
        let mut w = OpenstackWorker::new("node".to_string(), PathBuf::from("/tmp"))
            .with_fleet(Arc::new(FakeFleet::gated("QC-4 record missing")))
            .with_runner(Arc::new(FakeRunner::new()))
            .with_poll(Duration::from_millis(10));
        let token = ShutdownToken::from_receiver(rx);
        let handle = tokio::spawn(async move { w.run(token).await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        tx.send(true).expect("signal shutdown");
        let joined = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(joined.is_ok(), "worker must exit promptly on shutdown");
        assert!(joined.unwrap().expect("join").is_ok());
    }
}
