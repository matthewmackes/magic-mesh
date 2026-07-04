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
//!   read; the live etcd/Syncthing leg is typed
//!   [`fleet::FleetStateError::IntegrationGated`] until QC-4 authors the
//!   record) and [`podman::PodmanRunner`] (HOW they run — production shells
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
//! - [`fleet`] — QC-4 wires the live doctrine read (the `/mesh/cloud/` etcd
//!   record + TOML companion) and richer placement (`RabbitMQ` cluster
//!   topology); QC-10's capacity-derived quotas read the same doctrine.
//! - [`images`] — QC-3's airgap archive lane (landed): the decided
//!   `<share>/kolla/<release>/` layout + `SHA256SUMS` verification; QC-9's
//!   DIB pipeline reuses the same share conventions for tenant images.
//! - [`podman`] — QC-4 grows per-service health probes.
//! - [`reconcile`] — QC-4 extends the mirror with API health; QC-11's typed
//!   verbs consume the same state model.

#![cfg(feature = "async-services")]

pub mod catalog;
pub mod fleet;
pub mod images;
pub mod podman;
pub mod reconcile;
#[cfg(test)]
pub(crate) mod testkit;

use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::{ShutdownToken, Worker};

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

/// The QC-2 `openstack` worker.
pub struct OpenstackWorker {
    /// This node's id — the mirror topic namespace + `host` stamp.
    host: String,
    /// The doctrine seam (production: [`MeshFleetState`], integration-gated
    /// until QC-4).
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
    /// Converge cadence.
    poll: Duration,
    /// Mirror republish heartbeat.
    heartbeat: Duration,
}

impl OpenstackWorker {
    /// Construct with production defaults: the gated [`MeshFleetState`]
    /// doctrine seam over `workgroup_root`, the live [`PodmanCli`] runner,
    /// the `/etc/kolla` config root, `workgroup_root` doubling as the QC-3
    /// archive share (it IS the Syncthing-replicated `/mnt/mesh-storage`),
    /// and the default cadences. `host` is this node's id (the mirror
    /// `host` stamp).
    #[must_use]
    pub fn new(host: String, workgroup_root: PathBuf) -> Self {
        Self {
            host,
            fleet: Arc::new(MeshFleetState::new(workgroup_root.clone())),
            runner: Arc::new(PodmanCli::new()),
            config_root: PathBuf::from(DEFAULT_KOLLA_CONFIG_ROOT),
            share_root: workgroup_root,
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
        let outcome: CycleOutcome = match tokio::task::spawn_blocking(move || {
            converge_cycle(&*fleet, &*runner, &config_root, &share_root, &host)
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
