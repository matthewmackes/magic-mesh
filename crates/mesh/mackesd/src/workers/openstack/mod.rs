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
//!   lease.
//! - [`capacity`] — QC-10's node-capacity read + the two open-cloud guardrails
//!   it derives: capacity-scaled Nova flavors (Q39) and hard per-user quotas
//!   (Q89, the ENT-12 blast-radius boundary). The leader renders these into the
//!   [`config_render::render_cloud_bootstrap`] seed the deploy applies.
//! - [`config_render`] — QC-4's one-state → Kolla config renderer (landed):
//!   materializes each desired service's `/etc/kolla/<svc>/config.json` (+ the
//!   service config it points to) from the doctrine, atomically. QC-5 seals the
//!   real per-service credentials it substitutes; QC-9 renders Glance's
//!   local file store + image cache + cross-node `copy-image` replication.
//! - [`designate`] — QC-17's naming plane (Q46 — Designate replaces DNS/
//!   naming): the pure peer-directory → zone-record fold, the re-seed plan +
//!   feed script the QC-10 seed runs (the peer directory rebuilds the zones
//!   from scratch), the peer-fed pool topology (every node's bind9 — no
//!   fixed center), and the honest live-resolve gate.
//! - [`image_pipeline`] — QC-9's diskimage-builder → Glance pipeline (the
//!   pinned, testable definition that retires `build-mde-vm-golden.sh`, Q36/53):
//!   `disk-image-create` from a versioned element set → `glance image-create`
//!   into a node's file store → `glance-replicator livecopy` fan-out to every
//!   API node. QC-11/12 drive it from the typed `image` verb + Cloud plane.
//! - [`secrets`] — QC-5's sealed per-service secret set (the leader mints once
//!   from the OS CSPRNG, seals `0600` on the Syncthing share, every other node
//!   reads it; the renderer substitutes it for QC-4's placeholder).
//! - [`images`] — QC-3's airgap archive lane (landed): the decided
//!   `<share>/kolla/<release>/` layout + `SHA256SUMS` verification for the Kolla
//!   *service* images (the containers). Tenant VM images take the separate
//!   [`image_pipeline`] (DIB → Glance) lane — service images ride Podman, VM
//!   images ride Glance.
//! - [`podman`] — QC-5 grows per-service health probes.
//! - [`reconcile`] — QC-5 extends the mirror with API health; QC-11's typed
//!   verbs consume the same state model.
//! - [`verbs`] — QC-11's typed `action/cloud/*` Bus verb surface (Q40/Q70): the
//!   read verbs fold the [`reconcile::OpenStackState`] mirror; the
//!   `list-instances` + `instance-{start,stop,reboot,delete}` verbs gate on the
//!   real state and drive the Nova [`verbs::InstanceOps`] seam, so every mesh
//!   client (the Cloud plane, the phone, `meshctl`) drives the cloud through
//!   typed requests, never raw `openstack`.

#![cfg(feature = "async-services")]

pub mod capacity;
pub mod catalog;
/// IAC-1 — the OpenStack **client foundation** (clouds.yaml auth → Keystone
/// catalog → per-service API health → the standard resource/verb call seam). The
/// client half of the integration (this worker is the server half); the seam the
/// IAC surface (IAC-2..N) builds on.
pub mod client;
pub mod config_render;
pub mod designate;
pub mod fleet;
pub mod image_pipeline;
pub mod images;
pub mod podman;
pub mod reconcile;
pub mod secrets;
#[cfg(test)]
pub(crate) mod testkit;
pub mod verbs;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mde_bus::persist::Persist;

use super::nebula_supervisor::DEFAULT_OVERLAY_IP_PATH;
use super::{ShutdownToken, Worker};

use capacity::NodeCapacity;
use client::{CloudClient, LiveOpenStack};
use config_render::{render_cloud_bootstrap, render_fleet_heat_stack, OverlayBind};
use designate::{MeshPeerDirectory, PeerDirectorySource};
use fleet::{FleetStateSource, MeshFleetState};
use podman::{PodmanCli, PodmanRunner, DEFAULT_KOLLA_CONFIG_ROOT};
use reconcile::{converge_cycle, CycleOutcome, DoctrineStatus, OpenStackState};
use verbs::{drain_cloud_verbs, CloudNotifier, InstanceOps, OpenstackCli, CLOUD_VERBS};

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

/// The default Bus root (the persisted message tree), matching every other
/// mackesd worker's resolution (the QC-11 verb responder drains + replies here).
fn default_bus_root() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("mde").join("bus"))
}

/// QC-11 — seed each cloud verb topic's request cursor at its tail so a worker
/// restart doesn't replay stale requests (never re-performs a queued lifecycle
/// op). A missing Bus store leaves every cursor `None`; the first drain then
/// starts clean.
fn seed_verb_cursors(bus_root: Option<&Path>) -> BTreeMap<String, Option<String>> {
    let mut cursors = BTreeMap::new();
    let Some(root) = bus_root else {
        return cursors;
    };
    let Ok(persist) = Persist::open(root.to_path_buf()) else {
        return cursors;
    };
    for verb in CLOUD_VERBS {
        let topic = verbs::cloud_action_topic(verb);
        let latest = persist.latest_ulid(&topic).ok().flatten();
        cursors.insert(topic, latest);
    }
    cursors
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

/// QC-10 — on the leader, render the **cloud bootstrap seed** (capacity-derived
/// flavors + hard per-user quotas, design Q29/39/89) so the deploy can apply the
/// two open-cloud guardrails.
///
/// The seed is cloud-global bootstrap the leader owns (like the leader-hosted
/// `MariaDB`, Q15), so it's rendered only when this node's doctrine came back
/// `Enabled` with the leader lease held; every other node (and a Disabled/Gated
/// tick) no-ops. It reads the node's real capacity ([`NodeCapacity::probe`]) —
/// so the flavors/quotas track the fleet's actual shape — and a probe/render
/// failure rides the alert lane (→ chat, lock 11), never a fabricated capacity
/// or a silent swallow (§7). Idempotent: the seed's own guards make re-applying
/// on a later tick a no-op.
fn render_leader_bootstrap(
    config_root: &Path,
    peers: &dyn PeerDirectorySource,
    host: &str,
    instances: &[(String, String)],
    outcome: &mut CycleOutcome,
) {
    let DoctrineStatus::Enabled {
        leader: true,
        kolla_release,
    } = &outcome.state.doctrine
    else {
        return;
    };
    let release = kolla_release.clone();
    match NodeCapacity::probe() {
        Ok(cap) => {
            if let Err(e) = render_cloud_bootstrap(config_root, &release, &cap) {
                outcome.alerts.push(format!(
                    "openstack: cloud bootstrap seed render failed — {e}"
                ));
            }
        }
        Err(e) => outcome.alerts.push(format!(
            "openstack: node-capacity probe for the cloud bootstrap seed failed — {e}"
        )),
    }

    // QC-19 — the leader also renders the fleet Heat stack (Q61 — fleet renders
    // Heat, Heat executes) from the desired service set the doctrine converged
    // this tick: a real, fleet-derived inventory stack (no fabrication, §7), which
    // the bootstrap seed creates idempotently. Cloud-global like the seed, so
    // leader-only; a render failure rides the alert lane (→ chat), never a silent
    // swallow.
    let services: Vec<String> = outcome
        .state
        .services
        .iter()
        .map(|row| row.service.clone())
        .collect();
    if let Err(e) = render_fleet_heat_stack(config_root, &release, &services) {
        outcome
            .alerts
            .push(format!("openstack: fleet Heat stack render failed — {e}"));
    }

    // QC-17 — the leader renders the Designate naming plane's peer-fed inputs
    // (Q46: the peer directory feeds — and can re-seed — the mesh zone):
    // the pool topology (every node's bind9, no fixed center) and the zone
    // feed/re-seed script the QC-10 seed runs. Cloud-global like the seed, so
    // leader-only; this tick runs ON the leader, so `host` IS the leader the
    // leader-hosted names (mariadb/ovn-nb/ovn-sb.mesh) pin to. The live DNS
    // resolve check gates honestly into the feed's provenance header (§7 —
    // never a claimed-working naming plane). Render failures ride the alert
    // lane (→ chat), never a silent swallow.
    let peer_pairs = peers.pairs();
    if let Err(e) = designate::render_designate_pools(config_root, &release, &peer_pairs) {
        outcome
            .alerts
            .push(format!("openstack: designate pool render failed — {e}"));
    }
    let records = designate::derive_zone_records(&peer_pairs, Some(host), instances);
    let gate = designate::live_resolve(&format!("{host}.mesh"));
    let note = designate::resolve_note(&gate);
    if let Err(e) = designate::render_designate_feed(config_root, &release, &records, &note) {
        outcome.alerts.push(format!(
            "openstack: designate zone feed render failed — {e}"
        ));
    }
}

/// The QC-2 `openstack` worker.
pub struct OpenstackWorker {
    /// This node's id — the mirror topic namespace + `host` stamp.
    host: String,
    /// The doctrine seam (production: [`MeshFleetState`] — QC-4's live read
    /// off the Syncthing share + `/mesh/leader` lease).
    fleet: Arc<dyn FleetStateSource + Send + Sync>,
    /// QC-17 — the peer-directory seam the leader's Designate zone feed +
    /// pool topology derive from (production: [`MeshPeerDirectory`] — the
    /// etcd-first directory with the fs-union fallback).
    peers: Arc<dyn PeerDirectorySource + Send + Sync>,
    /// The podman seam (production: [`PodmanCli`]). `Arc` so each cycle runs
    /// on a `spawn_blocking` thread without borrowing `self`.
    runner: Arc<dyn PodmanRunner + Send + Sync>,
    /// QC-11 — the Nova instance seam the typed `action/cloud/instance-*`
    /// lifecycle + `list-instances` verbs drive (production: [`OpenstackCli`]).
    /// `Arc` so each verb drain runs on a `spawn_blocking` thread (an
    /// `openstack` shell-out never pins the async runtime) without borrowing
    /// `self`.
    instances: Arc<dyn InstanceOps + Send + Sync>,
    /// IAC-1 — the `OpenStack` client seam the typed `action/cloud/get-catalog`
    /// verb drives to produce the Keystone service directory + per-service API
    /// health (the §6 catalog+health contract the `IaC` surface consumes;
    /// production: [`LiveOpenStack`], which loads `clouds.yaml` per call and
    /// gates honestly on an unconfigured node). `Arc` so the verb drain runs on
    /// a `spawn_blocking` thread (the auth + probe HTTP never pins the async
    /// runtime) without borrowing `self`. The unified [`CloudClient`] seam also
    /// serves IAC-3's `list-resources` (one injected client, both verbs).
    catalog: Arc<dyn CloudClient>,
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
    /// The Bus root the QC-11 verb responder drains `action/cloud/*` requests
    /// from + writes `reply/<ulid>` to (production: [`default_bus_root`]; tests
    /// point it at a tempdir, or `None` to leave the responder idle).
    bus_root: Option<PathBuf>,
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
            peers: Arc::new(MeshPeerDirectory::new(workgroup_root.clone())),
            host,
            runner: Arc::new(PodmanCli::new()),
            instances: Arc::new(OpenstackCli::new()),
            catalog: LiveOpenStack::shared(),
            config_root: PathBuf::from(DEFAULT_KOLLA_CONFIG_ROOT),
            share_root: workgroup_root,
            overlay_ip_path: PathBuf::from(DEFAULT_OVERLAY_IP_PATH),
            bus_root: default_bus_root(),
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

    /// Inject the QC-17 peer-directory seam (tests — the Designate zone feed
    /// derives from a fixture directory instead of etcd).
    #[must_use]
    pub fn with_peers(mut self, peers: Arc<dyn PeerDirectorySource + Send + Sync>) -> Self {
        self.peers = peers;
        self
    }

    /// Inject the Nova instance seam (tests — the QC-11 verb responder).
    #[must_use]
    pub fn with_instances(mut self, instances: Arc<dyn InstanceOps + Send + Sync>) -> Self {
        self.instances = instances;
        self
    }

    /// Inject the IAC-1/IAC-3 `OpenStack` client seam (tests — the `get-catalog`
    /// + `list-resources` responder; production defaults to [`LiveOpenStack`]).
    #[must_use]
    pub fn with_catalog(mut self, catalog: Arc<dyn CloudClient>) -> Self {
        self.catalog = catalog;
        self
    }

    /// Override the Bus root the QC-11 verb responder uses (tests point it at a
    /// tempdir; `None` leaves the responder idle).
    #[must_use]
    pub fn with_bus_root(mut self, bus_root: Option<PathBuf>) -> Self {
        self.bus_root = bus_root;
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
        instance_snapshot: &[(String, String)],
    ) {
        let fleet = Arc::clone(&self.fleet);
        let runner = Arc::clone(&self.runner);
        let peers = Arc::clone(&self.peers);
        let config_root = self.config_root.clone();
        let share_root = self.share_root.clone();
        let host = self.host.clone();
        let instances = instance_snapshot.to_vec();
        // QC-6 — resolve this node's overlay bind each tick (it may come up
        // after the worker starts, once the node enrolls); an unresolved overlay
        // gates every start honestly in the converge.
        let overlay = resolve_overlay(&self.overlay_ip_path);
        let outcome: CycleOutcome = match tokio::task::spawn_blocking(move || {
            let mut outcome = converge_cycle(
                &*fleet,
                &*runner,
                &config_root,
                &share_root,
                &host,
                &overlay,
            );
            // QC-10 — the leader renders the cloud bootstrap seed (capacity-
            // derived flavors + hard per-user quotas) alongside the converge.
            // QC-17 — plus the Designate pool + zone feed (peer-directory-fed).
            render_leader_bootstrap(&config_root, &*peers, &host, &instances, &mut outcome);
            outcome
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

    /// QC-11 — drain net-new `action/cloud/*` verb requests and answer each on
    /// `reply/<ulid>` against the last-converged mirror `state` + the Nova seam.
    ///
    /// The whole drain (sqlite reads/writes + the possible `openstack`
    /// shell-out) runs on a `spawn_blocking` thread so a slow CLI never pins the
    /// async runtime — the same discipline the converge uses. `Persist` isn't
    /// `Sync`, so the closure opens its own handle each tick (cheap: one sqlite
    /// open + PRAGMA); the per-verb cursors ride in/out by value. Cursors are
    /// only advanced on a clean return, so a `spawn_blocking` panic never
    /// silently replays a queued lifecycle op (§7).
    async fn drain_verbs(
        &self,
        cursors: &mut BTreeMap<String, Option<String>>,
        notifier: &mut CloudNotifier,
        state: &OpenStackState,
    ) {
        let Some(bus_root) = self.bus_root.clone() else {
            return;
        };
        let instances = Arc::clone(&self.instances);
        let catalog = Arc::clone(&self.catalog);
        let state = state.clone();
        let snapshot = cursors.clone();
        // The notify producer's cross-tick state (host stamp + last-seen health)
        // rides in/out of the blocking task by value, like the verb cursors.
        let notif_in = notifier.clone();
        match tokio::task::spawn_blocking(move || {
            let mut cursors = snapshot;
            let mut notif = notif_in;
            match Persist::open(bus_root) {
                Ok(persist) => {
                    drain_cloud_verbs(
                        &persist,
                        &mut cursors,
                        &state,
                        &*instances,
                        &*catalog,
                        &mut notif,
                    );
                }
                Err(e) => {
                    tracing::debug!(target: "mackesd::openstack", error = %e, "cloud verbs: bus open failed");
                }
            }
            (cursors, notif)
        })
        .await
        {
            Ok((updated_cursors, updated_notif)) => {
                *cursors = updated_cursors;
                *notifier = updated_notif;
            }
            Err(e) => {
                tracing::warn!(target: "mackesd::openstack", error = %e, "cloud verb drain task join failed");
            }
        }
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
        // QC-11 — seed the verb-request cursors at the tail so a restart doesn't
        // replay stale `action/cloud/*` requests (no re-performed lifecycle op).
        let mut verb_cursors = seed_verb_cursors(self.bus_root.as_deref());
        // IAC-5 — the mesh-notify producer's cross-tick state (host stamp +
        // last-seen per-service health for the Up→Down edge).
        let mut notifier = CloudNotifier::new(self.host.clone());
        // QC-17 — the latest `(name, ip)` Nova roster snapshot the leader's
        // Designate zone feed derives instance records from. Empty until a
        // roster read lands (honest absence — instance records simply aren't
        // fed yet), refreshed by the QC-20 instance watch.
        let instance_snapshot: Vec<(String, String)> = Vec::new();
        // Converge + publish immediately on start so a panel doesn't wait a
        // full tick for the first mirror row.
        self.cycle_and_publish(&mut last, &mut last_pub_at, &instance_snapshot)
            .await;
        let mut tick = tokio::time::interval(self.poll);
        tick.tick().await; // consume the immediate first tick
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    self.cycle_and_publish(&mut last, &mut last_pub_at, &instance_snapshot).await;
                    // Answer any queued cloud verbs against the fresh mirror.
                    if let Some(state) = last.clone() {
                        self.drain_verbs(&mut verb_cursors, &mut notifier, &state).await;
                    }
                }
                () = shutdown.wait() => break,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::testkit::{FakeFleet, FakePeerDirectory, FakeRunner};
    use super::*;

    /// A two-node fixture directory for the leader-render tests (`node-a` is
    /// the host under test / the leader).
    fn fake_peers() -> FakePeerDirectory {
        FakePeerDirectory::new(&[("node-a", "10.42.0.9"), ("node-b", "10.42.0.4")])
    }

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

    // ── QC-10: the leader-gated cloud bootstrap seed ──

    fn outcome_with(doctrine: DoctrineStatus) -> CycleOutcome {
        CycleOutcome {
            state: OpenStackState {
                host: "node-a".into(),
                doctrine,
                runtime: reconcile::RuntimeStatus::Available,
                services: Vec::new(),
                extras: Vec::new(),
                published_at_ms: 0,
            },
            acted: false,
            alerts: Vec::new(),
        }
    }

    #[test]
    fn bootstrap_seed_is_not_rendered_off_the_leader() {
        // QC-10 — a non-leader (and a Disabled/Gated tick) renders no seed and
        // raises no alert: the cloud-global bootstrap is the leader's to own.
        let dir = tempfile::tempdir().unwrap();
        let seed = dir.path().join("bootstrap").join("cloud-bootstrap.sh");
        for doctrine in [
            DoctrineStatus::Enabled {
                leader: false,
                kolla_release: "2024.1".into(),
            },
            DoctrineStatus::Disabled,
            DoctrineStatus::Gated {
                reason: "no QC-4 record".into(),
            },
        ] {
            let mut outcome = outcome_with(doctrine);
            render_leader_bootstrap(dir.path(), &fake_peers(), "node-a", &[], &mut outcome);
            assert!(!seed.exists(), "no seed off the leader");
            // QC-17 — nor the Designate pool/feed (cloud-global = leader's).
            assert!(!dir
                .path()
                .join("bootstrap")
                .join("designate-feed.sh")
                .exists());
            assert!(outcome.alerts.is_empty(), "{:?}", outcome.alerts);
        }
    }

    #[test]
    fn the_leader_renders_the_capacity_derived_bootstrap_seed() {
        // QC-10 — on the leader the seed is rendered from this host's real probed
        // capacity (capacity-derived flavors + hard per-user quotas), so the
        // deploy can apply the two guardrails.
        let dir = tempfile::tempdir().unwrap();
        let mut outcome = outcome_with(DoctrineStatus::Enabled {
            leader: true,
            kolla_release: "2024.1".into(),
        });
        render_leader_bootstrap(dir.path(), &fake_peers(), "node-a", &[], &mut outcome);
        assert!(
            outcome.alerts.is_empty(),
            "the host probe/render must succeed: {:?}",
            outcome.alerts
        );
        let seed = std::fs::read_to_string(dir.path().join("bootstrap").join("cloud-bootstrap.sh"))
            .expect("the leader rendered the seed");
        assert!(seed.contains("QC-10"), "{seed}");
        assert!(seed.contains("ensure_flavor m1.large"), "{seed}");
        assert!(seed.contains("ensure_limit nova class:VCPU"), "{seed}");
    }

    #[test]
    fn the_leader_renders_the_peer_fed_designate_pool_and_zone_feed() {
        // QC-17/Q46 — alongside the QC-10 seed, the leader renders the
        // Designate naming plane's peer-directory-fed inputs: the pool
        // topology (every node's bind9) and the zone feed/re-seed script,
        // with the leader-hosted names pinned to THIS host (the tick runs on
        // the leader) and the instance snapshot folded in.
        let dir = tempfile::tempdir().unwrap();
        let mut outcome = outcome_with(DoctrineStatus::Enabled {
            leader: true,
            kolla_release: "2024.1".into(),
        });
        let instances = vec![("web-1".to_string(), "10.42.100.7".to_string())];
        render_leader_bootstrap(
            dir.path(),
            &fake_peers(),
            "node-a",
            &instances,
            &mut outcome,
        );
        assert!(
            outcome.alerts.is_empty(),
            "renders must succeed: {:?}",
            outcome.alerts
        );
        let feed = std::fs::read_to_string(dir.path().join("bootstrap").join("designate-feed.sh"))
            .expect("the leader rendered the zone feed");
        // Node + service + leader-pinned + instance records, all derived.
        assert!(
            feed.contains("ensure_rrset node-a.mesh. 10.42.0.9"),
            "{feed}"
        );
        assert!(
            feed.contains("ensure_rrset keystone.mesh. 10.42.0.4 10.42.0.9"),
            "{feed}"
        );
        assert!(
            feed.contains("ensure_rrset mariadb.mesh. 10.42.0.9"),
            "the leader-hosted name pins this host: {feed}"
        );
        assert!(
            feed.contains("ensure_rrset web-1.cloud.mesh. 10.42.100.7"),
            "{feed}"
        );
        // The honest live-resolve gate is stamped (§7): on a box where
        // node-a.mesh doesn't resolve it reads GATED, never claimed-working.
        assert!(feed.contains("# live-resolve gate: "), "{feed}");
        let pools =
            std::fs::read_to_string(dir.path().join("bootstrap").join("designate-pools.yaml"))
                .expect("the leader rendered the pool topology");
        assert!(pools.contains("bind9 on node-a"), "{pools}");
        assert!(pools.contains("bind9 on node-b"), "{pools}");
    }

    #[tokio::test]
    async fn tick_loop_exits_on_shutdown() {
        // Drives run() with the gated fake doctrine + an empty fake runner
        // (no live podman, no etcd) and exits promptly on shutdown. The verb
        // responder is idle (`with_bus_root(None)`) so the test stays hermetic.
        let (tx, rx) = tokio::sync::watch::channel(false);
        let mut w = OpenstackWorker::new("node".to_string(), PathBuf::from("/tmp"))
            .with_fleet(Arc::new(FakeFleet::gated("QC-4 record missing")))
            .with_runner(Arc::new(FakeRunner::new()))
            .with_bus_root(None)
            .with_poll(Duration::from_millis(10));
        let token = ShutdownToken::from_receiver(rx);
        let handle = tokio::spawn(async move { w.run(token).await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        tx.send(true).expect("signal shutdown");
        let joined = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(joined.is_ok(), "worker must exit promptly on shutdown");
        assert!(joined.unwrap().expect("join").is_ok());
    }

    #[tokio::test]
    async fn the_verb_responder_answers_a_cloud_request_from_the_run_loop() {
        // QC-11 — the wired responder: the running worker drains a queued
        // `action/cloud/get-status` request and lands a typed reply on
        // reply/<ulid> (a read answers even under a gated doctrine — it just
        // returns the honest mirror). Proves the mod.rs run-loop wiring +
        // bus_root injection, over a real Persist tempdir.
        use super::testkit::FakeInstanceOps;
        use mde_bus::hooks::config::Priority;
        use mde_bus::persist::Persist;
        use mde_bus::rpc::reply_topic;

        let dir = tempfile::tempdir().unwrap();
        let bus_root = dir.path().to_path_buf();

        let (tx, rx) = tokio::sync::watch::channel(false);
        let mut w = OpenstackWorker::new("node-a".to_string(), PathBuf::from("/tmp"))
            .with_fleet(Arc::new(FakeFleet::gated("QC-4 record missing")))
            .with_runner(Arc::new(FakeRunner::new()))
            .with_instances(Arc::new(FakeInstanceOps::new()))
            .with_bus_root(Some(bus_root.clone()))
            .with_poll(Duration::from_millis(10));
        let token = ShutdownToken::from_receiver(rx);
        let handle = tokio::spawn(async move { w.run(token).await });

        // Let the worker seed its cursors (empty topic ⇒ None) + tick once,
        // THEN publish the request so the next drain picks it up.
        tokio::time::sleep(Duration::from_millis(40)).await;
        let persist = Persist::open(bus_root.clone()).expect("persist");
        let req = persist
            .write(
                &verbs::cloud_action_topic("get-status"),
                Priority::Default,
                None,
                Some("{}"),
            )
            .expect("write request");
        drop(persist);

        // Poll for the reply across a few ticks.
        let reply_topic = reply_topic(&req.ulid);
        let mut answered = false;
        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let persist = Persist::open(bus_root.clone()).expect("persist");
            let replies = persist.list_since(&reply_topic, None).unwrap_or_default();
            if let Some(reply) = replies.first() {
                let decoded: verbs::CloudReply =
                    serde_json::from_str(&reply.body.clone().expect("body")).expect("decode");
                assert!(decoded.ok, "get-status answers even under a gated doctrine");
                assert_eq!(decoded.status.expect("status").host, "node-a");
                answered = true;
                break;
            }
        }
        tx.send(true).expect("signal shutdown");
        let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(
            answered,
            "the run-loop responder must answer the cloud request"
        );
    }
}
