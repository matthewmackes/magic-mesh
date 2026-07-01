//! MV-5a — `scheduler`: the **placement slice** of the no-center scheduler.
//!
//! Where MV-3 ([`super::vm_lifecycle`]) and MV-4 ([`super::container`]) are the
//! per-node *actuators* (they turn a host-addressed `action/{vm,container}/
//! lifecycle` request into `virsh`/`podman` calls) and MV-2
//! ([`super::kvm_health`]) is the per-node *capacity signal*
//! (`event/kvm/services`), MV-5a is the *chooser*: it turns a host-agnostic
//! `action/schedule/place` request into a host-*targeted* create/run on the
//! matching lifecycle topic, picking the node from the live capacity map.
//!
//! ## Shape (mirrors `vm_lifecycle`)
//!
//! - The **pure core** is fully unit-tested with no bus: [`fold_capacity`]
//!   (latest-wins-by-host fold of `event/kvm/services`, exactly like the other
//!   capacity folders), [`choose_node`] (the placement decision), and
//!   [`plan_placement`] (what to publish) never touch the bus or a clock
//!   (`now_ms` is passed in).
//! - The sole outward seam is an injectable [`Publisher`] (production
//!   [`BusPublisher`] fires the `mde-bus` CLI through
//!   [`crate::proc_reap::fire_and_reap`], the same fire-and-reap path
//!   `vm_lifecycle` / `kvm_health` publish on; a `RecordingPublisher` drives the
//!   tests). The action-topic drain + capacity read are the same short sync
//!   `Persist` open-read-drop `vm_lifecycle` uses (never crosses an `.await`),
//!   and the cursor is primed to the newest message on start so a restart
//!   doesn't re-fire a queued placement.
//! - Rank-0-default like `vm_lifecycle` / `container` (runs on every node). An
//!   **interim** lowest-node-id single-actor election ([`is_leader`]) keeps N
//!   nodes each running this worker from emitting N duplicate placements — it's
//!   a pure function of the shared capacity view, so no consensus is needed.
//!
//! ## Scope — placement (MV-5a) + failover (MV-5b)
//!
//! MV-5a is *place-on-request*: choose a node and forward one create/run.
//!
//! **MV-5b** adds the *survives-node-loss* half on the SAME seams — no new
//! worker, no new consensus:
//! - **Desired-state persistence:** every decided placement is also persisted as
//!   a [`DesiredPlacement`] (`{kind, spec, chosen_host, request_id}`) to the
//!   [`DESIRED_TOPIC`] on the same bus [`Persist`] MV-5a already reads/writes, so
//!   the intent outlives a restart or a leader change. Read back latest-wins-by-key
//!   ([`fold_desired`]), exactly like the capacity fold.
//! - **Pure re-placement:** [`replace_decisions`] re-picks a live host (via
//!   [`choose_node`] over the surviving capacity) for any persisted placement whose
//!   node has left the mesh — never the dead node, skipped when nothing is live.
//! - **Failover tick + HA re-election:** the leader (now the lowest *live* node —
//!   [`is_failover_leader`], the re-election MV-5a's stale-capacity [`is_leader`]
//!   deferred) re-emits the host-targeted create/run for the new node and updates
//!   the persisted desired-state, in the existing worker loop.
//!
//! The live-node set is the etcd-lease-backed peer directory
//! ([`crate::substrate::peers::read_directory`], seam [`LiveDirectory`]): liveness
//! IS the keepalive lease — a departed node's row auto-deletes — so no staleness
//! guess is needed. Cockpit is already installed by `node-virt.yml`.

#![cfg(feature = "async-services")]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use mde_bus::persist::Persist;

use super::kvm_health::KvmHealth;
use super::{ShutdownToken, Worker};

/// Bus topic the worker drains for placement requests (host-agnostic — the
/// request's optional `host` is a placement *pin*, not a per-scheduler target).
pub const ACTION_TOPIC: &str = "action/schedule/place";

/// Bus topic the worker publishes each placement decision to.
pub const PLACEMENTS_TOPIC: &str = "event/schedule/placements";

/// MV-5b — bus topic the worker persists desired-state ([`DesiredPlacement`]) to.
/// An `event/` topic (persisted + mesh-replicated like `event/kvm/services`), so
/// the intent survives a restart / leader change and every node folds the same
/// desired-state view. Read back latest-wins-by-key ([`fold_desired`]).
pub const DESIRED_TOPIC: &str = "event/schedule/desired";

/// Action-drain cadence. The bus read is a cheap local log scan; placement is a
/// slow, operator-visible event, so a 2 s poll is responsive without spinning.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// A mesh node identity (the `host` a [`KvmHealth`] summary is stamped with, and
/// the key of the capacity map).
pub type NodeId = String;

// ───────────────────────────── data model ─────────────────────────────

/// The kind of workload to place — selects which lifecycle actuator (and thus
/// which action topic + birth verb) the placement is forwarded to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaceKind {
    /// A libvirt/KVM VM — forwarded to [`super::vm_lifecycle`] as `op:"create"`.
    Vm,
    /// A Podman container — forwarded to [`super::container`] as `op:"run"`.
    Container,
}

impl PlaceKind {
    /// The lifecycle action topic a chosen placement of this kind is published
    /// to — the real actuator consts, so the topics can't drift.
    #[must_use]
    pub fn action_topic(self) -> &'static str {
        match self {
            PlaceKind::Vm => super::vm_lifecycle::ACTION_TOPIC,
            PlaceKind::Container => super::container::ACTION_TOPIC,
        }
    }

    /// The lifecycle `op` verb that *creates* a workload of this kind — the tag
    /// the downstream actuator's action enum expects. A VM is `create`
    /// ([`super::vm_lifecycle`]'s `Create`), a container is `run`
    /// ([`super::container`]'s `Run` — there's no define/start split).
    #[must_use]
    pub fn birth_op(self) -> &'static str {
        match self {
            PlaceKind::Vm => "create",
            PlaceKind::Container => "run",
        }
    }
}

/// One placement request drained off [`ACTION_TOPIC`]. The `spec` is opaque —
/// the scheduler forwards it verbatim to the actuator (which owns its shape), so
/// this worker never has to know a `VmSpec` from a `ContainerSpec`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PlaceRequest {
    /// Whether to place a VM or a container.
    pub kind: PlaceKind,
    /// The actuator spec, forwarded downstream untouched.
    pub spec: serde_json::Value,
    /// An optional node *pin* — honored iff that node is a healthy candidate,
    /// otherwise the scheduler falls back to the healthiest node.
    #[serde(default)]
    pub host: Option<NodeId>,
    /// An optional caller correlation id, echoed into the placement decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

/// Parse a [`PlaceRequest`] body.
///
/// # Errors
/// A human-readable message on malformed JSON / unknown `kind`.
pub fn parse_request(body: &str) -> Result<PlaceRequest, String> {
    serde_json::from_str(body).map_err(|e| format!("malformed place request: {e}"))
}

/// The decision published to [`PLACEMENTS_TOPIC`] — an audit trail of *what went
/// where* (the actuator request itself carries the forwarded spec).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PlacementDecision {
    /// The caller's correlation id, if the request carried one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// The placed workload kind.
    pub kind: PlaceKind,
    /// The node the workload was placed on.
    pub chosen_host: NodeId,
    /// How many candidate nodes the decision considered.
    pub candidates: usize,
    /// Wall-clock decision time (ms since the Unix epoch).
    pub published_at_ms: u64,
}

/// The concrete outcome of a placement decision — everything the worker's I/O
/// shell publishes. Returned by the pure [`plan_placement`] so the request →
/// publish wiring is testable without a bus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    /// The chosen node.
    pub chosen_host: NodeId,
    /// The lifecycle action topic to publish the create/run on.
    pub action_topic: &'static str,
    /// The host-targeted create/run body (`{"op":…,"host":…,"spec":…}`).
    pub action_body: String,
    /// The decision to publish to [`PLACEMENTS_TOPIC`].
    pub decision: PlacementDecision,
}

/// MV-5b — the persisted *desired state* of one placed workload: the minimal
/// intent (`{kind, spec, chosen_host, request_id}`) needed to rebuild — or
/// **re-place** — a [`Placement`] after a restart or a node loss. Persisted to
/// [`DESIRED_TOPIC`] and folded latest-wins-by-key ([`fold_desired`]); it is a
/// lossless projection of a `Placement` (the audit-only `candidates` /
/// `published_at_ms` are recomputed, not stored).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DesiredPlacement {
    /// Whether a VM or a container was placed.
    pub kind: PlaceKind,
    /// The actuator spec, forwarded downstream untouched (opaque).
    pub spec: serde_json::Value,
    /// The node the workload is currently desired to run on.
    pub chosen_host: NodeId,
    /// The caller correlation id, if the request carried one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

impl DesiredPlacement {
    /// Build the full [`Placement`] envelope (action topic + host-targeted
    /// create/run body + audit decision) for this desired-state. `candidates` /
    /// `now_ms` seed the [`PlacementDecision`]. This is the **single source of
    /// truth** for the create/run wire shape — [`plan_placement`] and the failover
    /// re-placement both build through it, so the envelope can't drift.
    #[must_use]
    pub fn to_placement(&self, candidates: usize, now_ms: u64) -> Placement {
        let action_body = serde_json::json!({
            "op": self.kind.birth_op(),
            "host": self.chosen_host,
            "spec": self.spec,
        })
        .to_string();
        Placement {
            chosen_host: self.chosen_host.clone(),
            action_topic: self.kind.action_topic(),
            action_body,
            decision: PlacementDecision {
                request_id: self.request_id.clone(),
                kind: self.kind,
                chosen_host: self.chosen_host.clone(),
                candidates,
                published_at_ms: now_ms,
            },
        }
    }

    /// Recover the desired-state from a planned / re-placed [`Placement`] (the
    /// inverse of [`to_placement`]): `kind` + `request_id` come off the decision,
    /// the opaque `spec` is read back out of the host-targeted body. `None` iff the
    /// body is not the `{op,host,spec}` envelope this module writes.
    #[must_use]
    pub fn from_placement(p: &Placement) -> Option<Self> {
        let body: serde_json::Value = serde_json::from_str(&p.action_body).ok()?;
        Some(Self {
            kind: p.decision.kind,
            spec: body.get("spec").cloned().unwrap_or(serde_json::Value::Null),
            chosen_host: p.chosen_host.clone(),
            request_id: p.decision.request_id.clone(),
        })
    }
}

// ─────────────────────────── pure: decision ───────────────────────────

/// Fold a stream of `event/kvm/services` bodies into a latest-wins-by-host
/// capacity map (later messages for the same `host` overwrite earlier ones —
/// exactly like the other capacity folders). Unparseable bodies are skipped.
#[must_use]
pub fn fold_capacity<'a>(bodies: impl IntoIterator<Item = &'a str>) -> BTreeMap<NodeId, KvmHealth> {
    let mut map = BTreeMap::new();
    for body in bodies {
        if let Ok(h) = serde_json::from_str::<KvmHealth>(body) {
            map.insert(h.host.clone(), h);
        }
    }
    map
}

/// The pure placement decision: pick the target node for `req` from
/// `candidates` (a `(node_id, health)` slice — the folded capacity map).
///
/// - If `req.host` is `Some` **and** that node is a healthy candidate
///   ([`KvmHealth::all_healthy`]), honor the pin.
/// - Otherwise the node with the most active services
///   ([`KvmHealth::active`]), deterministic tie-break by `node_id` ascending.
/// - `None` iff there are no candidates.
///
/// No I/O — fully unit-testable.
#[must_use]
pub fn choose_node(candidates: &[(NodeId, KvmHealth)], req: &PlaceRequest) -> Option<NodeId> {
    if candidates.is_empty() {
        return None;
    }
    // 1. Honor an explicit, *healthy* pin. An absent-or-unhealthy pin falls
    //    through to the capacity-ranked pick.
    if let Some(pin) = req.host.as_deref() {
        if let Some((id, health)) = candidates.iter().find(|(id, _)| id == pin) {
            if health.all_healthy {
                return Some(id.clone());
            }
        }
    }
    // 2. Most active services wins; ties break to the smallest node_id. The
    //    node_id half of the key is unique, so the pick is order-independent
    //    (no reliance on `max_by`'s last-of-equals rule).
    candidates
        .iter()
        .max_by(|x, y| x.1.active.cmp(&y.1.active).then_with(|| y.0.cmp(&x.0)))
        .map(|(id, _)| id.clone())
}

/// Compose the full placement outcome for `req` over the folded `capacity`:
/// choose the node ([`choose_node`]) then build the host-targeted create/run
/// body + the decision record. `None` when there is no candidate to place onto.
/// Pure — driven directly by tests without a bus.
#[must_use]
pub fn plan_placement(
    capacity: &BTreeMap<NodeId, KvmHealth>,
    req: &PlaceRequest,
    now_ms: u64,
) -> Option<Placement> {
    let candidates: Vec<(NodeId, KvmHealth)> = capacity
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let chosen = choose_node(&candidates, req)?;
    // Forward the opaque spec host-targeted at the chosen node via the shared
    // envelope builder (the same shape vm_lifecycle/container drain), and carry
    // the intent as the persistable desired-state.
    let desired = DesiredPlacement {
        kind: req.kind,
        spec: req.spec.clone(),
        chosen_host: chosen,
        request_id: req.request_id.clone(),
    };
    Some(desired.to_placement(candidates.len(), now_ms))
}

/// Interim single-actor election: the node whose id sorts first among the nodes
/// currently reporting KVM health is this slice's active scheduler. Every node
/// folds the same `event/kvm/services` view, so they agree on one actor without
/// consensus — which keeps N rank-0 schedulers from emitting N duplicate
/// placements. `false` on an empty map (no capacity ⇒ nothing to place onto).
/// HA re-election on leader loss is MV-5b.
#[must_use]
pub fn is_leader(host: &str, capacity: &BTreeMap<NodeId, KvmHealth>) -> bool {
    capacity.keys().next().map(String::as_str) == Some(host)
}

// ─────────────────────── pure: failover (MV-5b) ───────────────────────

/// Strip the `peer:` node-id prefix to the bare hostname. The capacity map +
/// [`Placement::chosen_host`] are keyed by the full node-id (`peer:<host>`), while
/// the mesh peer directory stores the bare `hostname` (the `telemetry` heartbeat
/// strips `peer:` before writing its `PeerRecord`). Normalizing both sides here is
/// what lets [`live_node_ids`] compare the two namespaces.
fn bare_host(id: &str) -> &str {
    id.strip_prefix("peer:").unwrap_or(id)
}

/// Reconcile the mesh peer `directory` (bare hostnames — liveness IS the etcd
/// keepalive lease, so a departed node is simply absent) against the node-id-keyed
/// `capacity` map: a capacity node is **live** iff its bare hostname is present in
/// the directory. The returned set is in the capacity / [`Placement::chosen_host`]
/// node-id namespace, so [`replace_decisions`] compares it directly. A node that
/// reports capacity but has left the directory (its lease lapsed) is *not* live —
/// which is exactly the node-loss [`replace_decisions`] re-places away from. Pure.
#[must_use]
pub fn live_node_ids(
    directory: &BTreeSet<NodeId>,
    capacity: &BTreeMap<NodeId, KvmHealth>,
) -> BTreeSet<NodeId> {
    let bare_dir: BTreeSet<&str> = directory.iter().map(|h| bare_host(h)).collect();
    capacity
        .keys()
        .filter(|id| bare_dir.contains(bare_host(id.as_str())))
        .cloned()
        .collect()
}

/// The failover actor: the lowest node-id among the **live** nodes. Unlike the
/// placement-path [`is_leader`] (lowest in the never-expiring capacity map, which
/// keeps electing a node that has since died), this re-elects over the live set —
/// so when the leader itself is the lost node, the next live node picks up
/// re-placement. This is the HA re-election MV-5a deferred. `false` on an empty
/// live set (nobody to lead / nothing live to place onto).
#[must_use]
pub fn is_failover_leader(host: &str, live: &BTreeSet<NodeId>) -> bool {
    live.iter().next().map(String::as_str) == Some(host)
}

/// The mesh-scoped key a [`DesiredPlacement`] is folded under: the caller's
/// `request_id` when present, else the workload identity `(kind, spec)`. Stable
/// across a re-placement (only `chosen_host` changes), so a re-placement's new
/// record shadows the prior one in [`fold_desired`] rather than duplicating it.
fn desired_key(d: &DesiredPlacement) -> String {
    d.request_id.clone().unwrap_or_else(|| {
        format!(
            "{}:{}",
            serde_json::to_string(&d.kind).unwrap_or_default(),
            serde_json::to_string(&d.spec).unwrap_or_default(),
        )
    })
}

/// Fold a stream of [`DESIRED_TOPIC`] bodies into a latest-wins-by-key desired-state
/// map (later records for the same [`desired_key`] overwrite earlier ones — exactly
/// like [`fold_capacity`]). Unparseable bodies are skipped. This is what makes the
/// failover tick idempotent: after a re-placement re-persists a workload onto its
/// new node, the next fold sees only that latest record, not the stale one.
#[must_use]
pub fn fold_desired<'a>(
    bodies: impl IntoIterator<Item = &'a str>,
) -> BTreeMap<String, DesiredPlacement> {
    let mut map = BTreeMap::new();
    for body in bodies {
        if let Ok(d) = serde_json::from_str::<DesiredPlacement>(body) {
            map.insert(desired_key(&d), d);
        }
    }
    map
}

/// The pure re-placement decision: for each persisted [`Placement`] whose
/// `chosen_host` is **not** in `live`, re-pick a target from the surviving live
/// capacity ([`choose_node`] over `capacity ∩ live`) and rebuild a host-targeted
/// [`Placement`] for the new node. A placement whose node is still live is left
/// alone; one with no live candidate to move to is skipped. The dead node is never
/// a candidate (it is absent from `live`, hence from the filtered capacity), so a
/// workload is never re-placed back onto the node it is failing away from.
/// Deterministic (input order preserved, [`choose_node`] tie-break) and clock-free
/// — the failover tick stamps the fresh audit time on the returned decisions.
#[must_use]
pub fn replace_decisions(
    persisted: &[Placement],
    live: &BTreeSet<NodeId>,
    capacity: &BTreeMap<NodeId, KvmHealth>,
) -> Vec<Placement> {
    // Candidate pool = capacity entries for still-live nodes only. The lost node
    // is excluded here, so `choose_node` can never re-pick it.
    let live_candidates: Vec<(NodeId, KvmHealth)> = capacity
        .iter()
        .filter(|(id, _)| live.contains(*id))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let mut out = Vec::new();
    for p in persisted {
        if live.contains(&p.chosen_host) {
            continue; // node still alive — the workload keeps running, nothing to do
        }
        // Recover the desired workload, then re-choose over the LIVE capacity. A
        // pin-less request: the original pin, if any, was the node we're leaving.
        let Some(desired) = DesiredPlacement::from_placement(p) else {
            continue;
        };
        let req = PlaceRequest {
            kind: desired.kind,
            spec: serde_json::Value::Null, // choose_node reads only the (absent) pin
            host: None,
            request_id: desired.request_id.clone(),
        };
        let Some(new_host) = choose_node(&live_candidates, &req) else {
            continue; // nothing live to place onto — leave the intent as-is
        };
        let replaced = DesiredPlacement {
            chosen_host: new_host,
            ..desired
        };
        out.push(replaced.to_placement(live_candidates.len(), p.decision.published_at_ms));
    }
    out
}

// ─────────────────────────── bus + worker ───────────────────────────

/// The outward publish seam. Production wires [`BusPublisher`]; tests wire a
/// recorder so the request → publish wiring runs without an `mde-bus` binary.
pub trait Publisher {
    /// Publish `body` to `topic`. Best-effort (a failed publish is swallowed,
    /// like every other tick-publisher).
    fn publish(&self, topic: &str, body: &str);
}

/// Production [`Publisher`]: the `mde-bus publish` CLI fired through the detached
/// [`crate::proc_reap::fire_and_reap`] reaper — the same path `vm_lifecycle` /
/// `kvm_health` publish on. A missing binary (pre-RPM dev box) is swallowed.
#[derive(Debug, Clone, Copy, Default)]
pub struct BusPublisher;

impl Publisher for BusPublisher {
    fn publish(&self, topic: &str, body: &str) {
        let mut cmd = Command::new("mde-bus");
        cmd.args(["publish", topic, "--body-flag", body]);
        crate::proc_reap::fire_and_reap(cmd, crate::proc_reap::DEFAULT_REAP_TIMEOUT);
    }
}

/// MV-5b — the "who is alive right now" seam the failover tick reads. Production
/// wires [`PeerDirectory`] (the etcd lease-backed mesh directory); tests wire a
/// fixed set so the pure re-placement wiring runs without etcd.
pub trait LiveDirectory {
    /// The hostnames currently present in the mesh peer directory (bare — the
    /// `peer:` prefix is normalized against capacity in [`live_node_ids`]).
    fn live_hostnames(&self) -> BTreeSet<NodeId>;
}

/// Production [`LiveDirectory`]: the canonical etcd-first peer directory
/// ([`crate::substrate::peers::read_directory`]), where **liveness is the etcd
/// keepalive lease** — a departed node's row auto-deletes, so a stale
/// `last_seen_ms` guess is never needed. Falls back to the replicated fs union
/// under `workgroup_root` when the coordination plane is un-provisioned (same
/// precedence every other directory reader uses).
pub struct PeerDirectory {
    /// Shared-storage root — the fs-union fallback when etcd is absent.
    workgroup_root: PathBuf,
}

impl LiveDirectory for PeerDirectory {
    fn live_hostnames(&self) -> BTreeSet<NodeId> {
        crate::substrate::peers::read_directory(&self.workgroup_root)
            .into_iter()
            .map(|r| r.hostname)
            .collect()
    }
}

/// Read new [`ACTION_TOPIC`] messages since `cursor`, advancing it. A short sync
/// open-read-drop (never crosses an `.await`), mirroring `vm_lifecycle`.
fn read_new_requests(bus_root: &Path, cursor: &mut Option<String>) -> Vec<PlaceRequest> {
    let Ok(persist) = Persist::open(bus_root.to_path_buf()) else {
        return vec![];
    };
    let Ok(msgs) = persist.list_since(ACTION_TOPIC, cursor.as_deref()) else {
        return vec![];
    };
    let mut out = Vec::new();
    for msg in msgs {
        *cursor = Some(msg.ulid.clone());
        let body = msg.body.as_deref().unwrap_or("");
        match parse_request(body) {
            Ok(r) => out.push(r),
            Err(e) => tracing::warn!(ulid = %msg.ulid, error = %e, "scheduler: bad place request"),
        }
    }
    out
}

/// Fold the latest `event/kvm/services` per node into a capacity map. A short
/// sync open-read-drop like [`read_new_requests`].
fn read_capacity(bus_root: &Path) -> BTreeMap<NodeId, KvmHealth> {
    let Ok(persist) = Persist::open(bus_root.to_path_buf()) else {
        return BTreeMap::new();
    };
    let Ok(msgs) = persist.list_since(super::kvm_health::SERVICES_TOPIC, None) else {
        return BTreeMap::new();
    };
    let bodies: Vec<String> = msgs
        .into_iter()
        .map(|m| m.body.unwrap_or_default())
        .collect();
    fold_capacity(bodies.iter().map(String::as_str))
}

/// MV-5b — read the persisted desired-state, folded latest-wins-by-key
/// ([`fold_desired`]). A short sync open-read-drop like [`read_capacity`]; the
/// `event/schedule/desired` log survives a restart / leader change, so the leader
/// that takes over on failover sees the full intent even though it never handled
/// the original request.
fn read_desired(bus_root: &Path) -> Vec<DesiredPlacement> {
    let Ok(persist) = Persist::open(bus_root.to_path_buf()) else {
        return vec![];
    };
    let Ok(msgs) = persist.list_since(DESIRED_TOPIC, None) else {
        return vec![];
    };
    let bodies: Vec<String> = msgs
        .into_iter()
        .map(|m| m.body.unwrap_or_default())
        .collect();
    fold_desired(bodies.iter().map(String::as_str))
        .into_values()
        .collect()
}

/// Seed the cursor to the newest existing message so a (re)start doesn't
/// re-execute a queued placement. `None` when the topic is empty.
fn prime_cursor(bus_root: &Path) -> Option<String> {
    let persist = Persist::open(bus_root.to_path_buf()).ok()?;
    let msgs = persist.list_since(ACTION_TOPIC, None).ok()?;
    msgs.last().map(|m| m.ulid.clone())
}

fn default_bus_root() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("mde").join("bus"))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// The placement (MV-5a) + failover (MV-5b) worker.
pub struct SchedulerWorker {
    /// This node's id — its identity in the [`is_leader`] /
    /// [`is_failover_leader`] elections.
    host: NodeId,
    /// The injectable publish seam (production: [`BusPublisher`]).
    publisher: Box<dyn Publisher + Send + Sync>,
    /// The injectable live-node seam (production: [`PeerDirectory`]).
    live_dir: Box<dyn LiveDirectory + Send + Sync>,
    /// Action-drain cadence.
    poll: Duration,
    /// Bus root override (tests). `None` ⇒ [`default_bus_root`].
    bus_root_override: Option<PathBuf>,
}

impl SchedulerWorker {
    /// Construct with production defaults: the live [`BusPublisher`], the etcd
    /// [`PeerDirectory`], the default cadence, and the auto-resolved bus root.
    /// `host` is this node's id.
    #[must_use]
    pub fn new(host: NodeId) -> Self {
        Self {
            host,
            publisher: Box::new(BusPublisher),
            live_dir: Box::new(PeerDirectory {
                workgroup_root: crate::default_qnm_shared_root(),
            }),
            poll: DEFAULT_POLL_INTERVAL,
            bus_root_override: None,
        }
    }

    /// Inject a publisher (tests). Production uses the [`BusPublisher`] default.
    #[must_use]
    pub fn with_publisher(mut self, publisher: Box<dyn Publisher + Send + Sync>) -> Self {
        self.publisher = publisher;
        self
    }

    /// Inject a live-node directory (tests). Production uses the etcd-backed
    /// [`PeerDirectory`] default.
    #[must_use]
    pub fn with_live_directory(mut self, live_dir: Box<dyn LiveDirectory + Send + Sync>) -> Self {
        self.live_dir = live_dir;
        self
    }

    /// Override the action-drain cadence (tests, to avoid multi-second waits).
    #[must_use]
    pub fn with_poll(mut self, poll: Duration) -> Self {
        self.poll = poll;
        self
    }

    /// Override the Bus root (tests).
    #[must_use]
    pub fn with_bus_root(mut self, root: PathBuf) -> Self {
        self.bus_root_override = Some(root);
        self
    }

    fn bus_root(&self) -> Option<PathBuf> {
        self.bus_root_override.clone().or_else(default_bus_root)
    }

    /// Drain new placement requests (advancing the cursor) and, when this node
    /// is the elected scheduler, choose a node for each + publish the create/run
    /// and the decision.
    async fn drain_and_place(&self, bus_root: &Path, cursor: &mut Option<String>) {
        let requests = read_new_requests(bus_root, cursor);
        if requests.is_empty() {
            return;
        }
        let capacity = read_capacity(bus_root);
        // Only the elected node acts; non-leaders still advanced their cursor
        // above, so the leader handles new requests from its own cursor (a
        // deliberate no-catch-up-on-failover — that's MV-5b).
        if !is_leader(&self.host, &capacity) {
            return;
        }
        for req in requests {
            match plan_placement(&capacity, &req, now_ms()) {
                Some(p) => {
                    let Ok(decision_body) = serde_json::to_string(&p.decision) else {
                        continue;
                    };
                    self.publisher.publish(p.action_topic, &p.action_body);
                    self.publisher.publish(PLACEMENTS_TOPIC, &decision_body);
                    // MV-5b — persist the desired-state so the intent survives a
                    // restart / leader change and the failover tick can re-place
                    // this workload if its node is later lost.
                    if let Some(desired) = DesiredPlacement::from_placement(&p) {
                        if let Ok(desired_body) = serde_json::to_string(&desired) {
                            self.publisher.publish(DESIRED_TOPIC, &desired_body);
                        }
                    }
                    tracing::info!(
                        kind = ?req.kind, chosen = %p.chosen_host,
                        "scheduler: placed workload",
                    );
                }
                None => tracing::warn!(
                    target: "mackesd::alert",
                    "ALERT (warn): scheduler found no healthy candidate for a place request — dropping",
                ),
            }
        }
    }

    /// MV-5b failover tick: re-place workloads whose node has left the mesh. Only
    /// the leader acts, and leadership here re-elects over the **live** set
    /// ([`is_failover_leader`]) so a lost leader is taken over. Reads the persisted
    /// desired-state + the live-node directory, computes [`replace_decisions`], and
    /// for each re-placement re-emits the host-targeted create/run for the new node
    /// AND updates the persisted desired-state (so the next tick sees the workload
    /// as running on its new home — idempotent). Runs in the existing loop; no new
    /// worker, no new consensus. Best-effort like every tick-publisher.
    async fn failover_once(&self, bus_root: &Path) {
        // Cheap local-bus read first: no persisted intent ⇒ nothing to fail over
        // (and we skip the directory read entirely).
        let desired = read_desired(bus_root);
        if desired.is_empty() {
            return;
        }
        let capacity = read_capacity(bus_root);
        let live = live_node_ids(&self.live_dir.live_hostnames(), &capacity);
        // Single active actor: the lowest live node. A non-leader (or a node not
        // yet in its own live set) does nothing.
        if !is_failover_leader(&self.host, &live) {
            return;
        }
        let persisted: Vec<Placement> = desired.iter().map(|d| d.to_placement(0, 0)).collect();
        for mut p in replace_decisions(&persisted, &live, &capacity) {
            // Fresh audit time on the re-placement (replace_decisions is clock-free).
            p.decision.published_at_ms = now_ms();
            let Ok(decision_body) = serde_json::to_string(&p.decision) else {
                continue;
            };
            // 1. Re-emit the host-targeted create/run for the NEW node.
            self.publisher.publish(p.action_topic, &p.action_body);
            // 2. Audit the re-placement.
            self.publisher.publish(PLACEMENTS_TOPIC, &decision_body);
            // 3. Update the persisted desired-state to the new home (idempotent —
            //    next fold sees the workload as live there, so it isn't re-placed
            //    again).
            if let Some(updated) = DesiredPlacement::from_placement(&p) {
                if let Ok(body) = serde_json::to_string(&updated) {
                    self.publisher.publish(DESIRED_TOPIC, &body);
                }
            }
            tracing::warn!(
                target: "mackesd::alert",
                chosen = %p.chosen_host,
                "ALERT (warn): scheduler re-placed a workload after node loss (MV-5b failover)",
            );
        }
    }
}

#[async_trait::async_trait]
impl Worker for SchedulerWorker {
    fn name(&self) -> &'static str {
        "scheduler"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let bus_root = self.bus_root();
        // Skip any backlog so a restart doesn't re-fire a queued placement.
        let mut cursor = bus_root.as_deref().and_then(prime_cursor);
        let mut tick = tokio::time::interval(self.poll);
        tick.tick().await; // consume the immediate first tick
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    if let Some(root) = &bus_root {
                        self.drain_and_place(root, &mut cursor).await;
                        // MV-5b — re-place workloads whose node has left (leader-only).
                        self.failover_once(root).await;
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
    use super::*;
    use std::sync::Mutex;

    /// A `KvmHealth` shaped just enough for the placement decision (only
    /// `host` / `active` / `all_healthy` are read by [`choose_node`]).
    fn health(host: &str, active: usize, all_healthy: bool) -> KvmHealth {
        KvmHealth {
            host: host.to_string(),
            services: vec![],
            active,
            total: active,
            all_healthy,
            published_at_ms: 0,
        }
    }

    fn candidates(pairs: &[(&str, usize, bool)]) -> Vec<(NodeId, KvmHealth)> {
        pairs
            .iter()
            .map(|(id, active, ok)| ((*id).to_string(), health(id, *active, *ok)))
            .collect()
    }

    fn req(kind: PlaceKind, pin: Option<&str>) -> PlaceRequest {
        PlaceRequest {
            kind,
            spec: serde_json::json!({"name": "w1"}),
            host: pin.map(str::to_string),
            request_id: None,
        }
    }

    // ── choose_node (the required pure tests) ──

    #[test]
    fn choose_node_honors_a_healthy_pin() {
        // node-b has more active services, but the healthy pin wins.
        let c = candidates(&[("node-a", 1, true), ("node-b", 9, true)]);
        let chosen = choose_node(&c, &req(PlaceKind::Vm, Some("node-a")));
        assert_eq!(chosen.as_deref(), Some("node-a"));
    }

    #[test]
    fn choose_node_picks_the_healthiest() {
        // No pin ⇒ the most active node.
        let c = candidates(&[
            ("node-a", 2, true),
            ("node-b", 5, true),
            ("node-c", 3, true),
        ]);
        let chosen = choose_node(&c, &req(PlaceKind::Container, None));
        assert_eq!(chosen.as_deref(), Some("node-b"));
    }

    #[test]
    fn choose_node_tie_breaks_by_node_id_ascending() {
        // Equal active ⇒ smallest node_id, regardless of input order.
        let fwd = candidates(&[("node-a", 4, true), ("node-b", 4, true)]);
        let rev = candidates(&[("node-b", 4, true), ("node-a", 4, true)]);
        assert_eq!(
            choose_node(&fwd, &req(PlaceKind::Vm, None)).as_deref(),
            Some("node-a")
        );
        assert_eq!(
            choose_node(&rev, &req(PlaceKind::Vm, None)).as_deref(),
            Some("node-a")
        );
    }

    #[test]
    fn choose_node_none_with_no_candidates() {
        assert_eq!(choose_node(&[], &req(PlaceKind::Vm, Some("node-a"))), None);
        assert_eq!(choose_node(&[], &req(PlaceKind::Container, None)), None);
    }

    #[test]
    fn choose_node_ignores_an_unhealthy_pin_and_falls_back_to_healthiest() {
        // Pinned node-a is unhealthy (and not the most active) ⇒ fall back to
        // the healthiest, node-b.
        let c = candidates(&[("node-a", 1, false), ("node-b", 3, true)]);
        let chosen = choose_node(&c, &req(PlaceKind::Vm, Some("node-a")));
        assert_eq!(chosen.as_deref(), Some("node-b"));
    }

    #[test]
    fn choose_node_ignores_an_absent_pin() {
        // A pin for a node not in the capacity map falls back to the healthiest.
        let c = candidates(&[("node-a", 2, true), ("node-b", 5, true)]);
        let chosen = choose_node(&c, &req(PlaceKind::Vm, Some("ghost")));
        assert_eq!(chosen.as_deref(), Some("node-b"));
    }

    // ── fold_capacity (latest-wins by host) ──

    #[test]
    fn fold_capacity_is_latest_wins_by_host() {
        let older = serde_json::to_string(&health("node-a", 1, false)).unwrap();
        let newer = serde_json::to_string(&health("node-a", 6, true)).unwrap();
        let other = serde_json::to_string(&health("node-b", 3, true)).unwrap();
        let map = fold_capacity([older.as_str(), other.as_str(), newer.as_str(), "garbage"]);
        assert_eq!(map.len(), 2);
        // node-a's later message wins.
        assert_eq!(map["node-a"].active, 6);
        assert!(map["node-a"].all_healthy);
        assert_eq!(map["node-b"].active, 3);
    }

    // ── plan_placement (the published envelopes) ──

    #[test]
    fn plan_placement_builds_a_host_targeted_vm_create() {
        let cap = fold_capacity([
            serde_json::to_string(&health("node-a", 2, true))
                .unwrap()
                .as_str(),
            serde_json::to_string(&health("node-b", 5, true))
                .unwrap()
                .as_str(),
        ]);
        let r = PlaceRequest {
            kind: PlaceKind::Vm,
            spec: serde_json::json!({"name": "web1", "vcpus": 2}),
            host: None,
            request_id: Some("req-42".into()),
        };
        let p = plan_placement(&cap, &r, 1234).expect("a placement");
        assert_eq!(p.chosen_host, "node-b"); // healthiest
        assert_eq!(p.action_topic, super::super::vm_lifecycle::ACTION_TOPIC);
        // The forwarded body is a host-targeted create the vm_lifecycle actuator
        // deserializes, carrying the opaque spec verbatim.
        let body: serde_json::Value = serde_json::from_str(&p.action_body).unwrap();
        assert_eq!(body["op"], "create");
        assert_eq!(body["host"], "node-b");
        assert_eq!(body["spec"]["name"], "web1");
        assert_eq!(body["spec"]["vcpus"], 2);
        assert_eq!(p.decision.request_id.as_deref(), Some("req-42"));
        assert_eq!(p.decision.candidates, 2);
        assert_eq!(p.decision.published_at_ms, 1234);
    }

    #[test]
    fn plan_placement_container_uses_run_op_and_topic() {
        let cap = fold_capacity([serde_json::to_string(&health("n1", 3, true))
            .unwrap()
            .as_str()]);
        let r = req(PlaceKind::Container, None);
        let p = plan_placement(&cap, &r, 0).expect("a placement");
        assert_eq!(p.action_topic, super::super::container::ACTION_TOPIC);
        let body: serde_json::Value = serde_json::from_str(&p.action_body).unwrap();
        // A container's birth verb is `run`, not `create`.
        assert_eq!(body["op"], "run");
        assert_eq!(body["host"], "n1");
    }

    #[test]
    fn plan_placement_none_without_capacity() {
        let cap: BTreeMap<NodeId, KvmHealth> = BTreeMap::new();
        assert!(plan_placement(&cap, &req(PlaceKind::Vm, None), 0).is_none());
    }

    // ── is_leader (interim single-actor election) ──

    #[test]
    fn is_leader_is_the_lowest_node_id() {
        let cap = fold_capacity([
            serde_json::to_string(&health("node-b", 1, true))
                .unwrap()
                .as_str(),
            serde_json::to_string(&health("node-a", 1, true))
                .unwrap()
                .as_str(),
        ]);
        assert!(is_leader("node-a", &cap));
        assert!(!is_leader("node-b", &cap));
        // No capacity ⇒ nobody is the leader.
        assert!(!is_leader("node-a", &BTreeMap::new()));
    }

    // ── request parsing + topics ──

    #[test]
    fn parse_request_round_trips_and_defaults_optional_fields() {
        let r = parse_request(
            r#"{"kind":"vm","spec":{"name":"d","vcpus":1},"host":"node-a","request_id":"r1"}"#,
        )
        .expect("parse");
        assert_eq!(r.kind, PlaceKind::Vm);
        assert_eq!(r.host.as_deref(), Some("node-a"));
        assert_eq!(r.request_id.as_deref(), Some("r1"));
        // host + request_id default to None; kind snake-cases to "container".
        let bare = parse_request(r#"{"kind":"container","spec":{}}"#).expect("parse");
        assert_eq!(bare.kind, PlaceKind::Container);
        assert!(bare.host.is_none());
        assert!(bare.request_id.is_none());
        assert!(parse_request("nope").is_err());
        assert!(parse_request(r#"{"kind":"teleport","spec":{}}"#).is_err());
    }

    #[test]
    fn topics_are_namespaced() {
        assert_eq!(ACTION_TOPIC, "action/schedule/place");
        assert!(ACTION_TOPIC.starts_with("action/"));
        assert_eq!(PLACEMENTS_TOPIC, "event/schedule/placements");
        assert!(PLACEMENTS_TOPIC.starts_with("event/"));
        // The forward topics are the real actuator consts (no drift).
        assert_eq!(
            PlaceKind::Vm.action_topic(),
            super::super::vm_lifecycle::ACTION_TOPIC
        );
        assert_eq!(
            PlaceKind::Container.action_topic(),
            super::super::container::ACTION_TOPIC
        );
    }

    #[test]
    fn worker_name_matches_module() {
        let w = SchedulerWorker::new("node".to_string());
        assert_eq!(w.name(), "scheduler");
    }

    // ─────────────────────── MV-5b: failover ───────────────────────

    /// A desired-state record (`{kind, spec, chosen_host, request_id}`).
    fn dp(kind: PlaceKind, host: &str, name: &str, rid: Option<&str>) -> DesiredPlacement {
        DesiredPlacement {
            kind,
            spec: serde_json::json!({ "name": name }),
            chosen_host: host.to_string(),
            request_id: rid.map(str::to_string),
        }
    }

    /// A node-id-keyed capacity map (what `read_capacity` folds to).
    fn cap(pairs: &[(&str, usize, bool)]) -> BTreeMap<NodeId, KvmHealth> {
        pairs
            .iter()
            .map(|(id, active, ok)| ((*id).to_string(), health(id, *active, *ok)))
            .collect()
    }

    /// A live-node set in the capacity/`chosen_host` namespace.
    fn live_set(ids: &[&str]) -> BTreeSet<NodeId> {
        ids.iter().map(|s| (*s).to_string()).collect()
    }

    // ── replace_decisions (the required pure tests) ──

    #[test]
    fn replace_decisions_reassigns_a_lost_node_to_healthiest_live() {
        // w1 was on peer:b; peer:b's lease lapsed (absent from live) though its
        // stale capacity lingers. Re-placed onto the healthiest LIVE node, peer:c.
        let persisted = vec![dp(PlaceKind::Vm, "peer:b", "w1", Some("r1")).to_placement(0, 100)];
        let capacity = cap(&[
            ("peer:a", 2, true),
            ("peer:b", 9, true),
            ("peer:c", 5, true),
        ]);
        let out = replace_decisions(&persisted, &live_set(&["peer:a", "peer:c"]), &capacity);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].chosen_host, "peer:c");
    }

    #[test]
    fn replace_decisions_leaves_a_live_node_untouched() {
        // peer:a is still live ⇒ the workload keeps running, no re-placement.
        let persisted = vec![dp(PlaceKind::Vm, "peer:a", "w1", None).to_placement(0, 100)];
        let capacity = cap(&[("peer:a", 2, true), ("peer:c", 5, true)]);
        let out = replace_decisions(&persisted, &live_set(&["peer:a", "peer:c"]), &capacity);
        assert!(out.is_empty());
    }

    #[test]
    fn replace_decisions_skips_when_no_live_candidate() {
        let persisted = vec![dp(PlaceKind::Vm, "peer:b", "w1", None).to_placement(0, 100)];
        let capacity = cap(&[("peer:b", 1, true)]);
        // (a) nothing live at all.
        assert!(replace_decisions(&persisted, &BTreeSet::new(), &capacity).is_empty());
        // (b) a node is live but has no capacity to place onto ⇒ still skipped.
        let out = replace_decisions(&persisted, &live_set(&["peer:x"]), &capacity);
        assert!(out.is_empty());
    }

    #[test]
    fn replace_decisions_never_targets_the_dead_node() {
        // peer:b is the MOST active in capacity but is dead — must not be re-picked;
        // the only live candidate, peer:a, wins.
        let persisted = vec![dp(PlaceKind::Vm, "peer:b", "w1", None).to_placement(0, 100)];
        let capacity = cap(&[("peer:a", 1, true), ("peer:b", 99, true)]);
        let out = replace_decisions(&persisted, &live_set(&["peer:a"]), &capacity);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].chosen_host, "peer:a");
    }

    #[test]
    fn replace_decisions_retargets_the_action_to_the_new_node() {
        // A container re-places as a container (run op + container topic), the
        // opaque spec + request_id are preserved, and the action is host-targeted
        // at the NEW node.
        let persisted =
            vec![dp(PlaceKind::Container, "peer:b", "svc1", Some("r7")).to_placement(0, 100)];
        let capacity = cap(&[("peer:a", 3, true)]);
        let out = replace_decisions(&persisted, &live_set(&["peer:a"]), &capacity);
        assert_eq!(out.len(), 1);
        let p = &out[0];
        assert_eq!(p.action_topic, super::super::container::ACTION_TOPIC);
        let body: serde_json::Value = serde_json::from_str(&p.action_body).unwrap();
        assert_eq!(body["op"], "run");
        assert_eq!(body["host"], "peer:a");
        assert_eq!(body["spec"]["name"], "svc1");
        assert_eq!(p.chosen_host, "peer:a");
        assert_eq!(p.decision.chosen_host, "peer:a");
        assert_eq!(p.decision.kind, PlaceKind::Container);
        assert_eq!(p.decision.request_id.as_deref(), Some("r7"));
    }

    #[test]
    fn replace_decisions_is_deterministic_and_tie_breaks_by_node_id() {
        // Equal-active live candidates ⇒ smallest node_id, and repeat runs are
        // byte-identical.
        let persisted = vec![dp(PlaceKind::Vm, "peer:dead", "w1", None).to_placement(0, 100)];
        let capacity = cap(&[("peer:a", 4, true), ("peer:b", 4, true)]);
        let live = live_set(&["peer:a", "peer:b"]);
        let out1 = replace_decisions(&persisted, &live, &capacity);
        let out2 = replace_decisions(&persisted, &live, &capacity);
        assert_eq!(out1, out2);
        assert_eq!(out1[0].chosen_host, "peer:a");
    }

    #[test]
    fn replace_decisions_handles_a_mix_of_live_and_lost() {
        // One workload on a live node (kept), one on a lost node (re-placed).
        let persisted = vec![
            dp(PlaceKind::Vm, "peer:a", "keep", None).to_placement(0, 100),
            dp(PlaceKind::Vm, "peer:dead", "move", Some("r2")).to_placement(0, 100),
        ];
        let capacity = cap(&[("peer:a", 2, true), ("peer:c", 5, true)]);
        let out = replace_decisions(&persisted, &live_set(&["peer:a", "peer:c"]), &capacity);
        assert_eq!(out.len(), 1, "only the lost workload is re-placed");
        assert_eq!(out[0].decision.request_id.as_deref(), Some("r2"));
        assert_eq!(out[0].chosen_host, "peer:c");
    }

    // ── live_node_ids (the peer: prefix reconciliation) ──

    #[test]
    fn live_node_ids_reconciles_the_peer_prefix() {
        // The directory stores BARE hostnames (`a`); capacity is node-id-keyed
        // (`peer:a`). peer:a is live; peer:b (absent from the directory) is gone.
        let capacity = cap(&[("peer:a", 1, true), ("peer:b", 1, true)]);
        let directory: BTreeSet<NodeId> = live_set(&["a"]);
        assert_eq!(live_node_ids(&directory, &capacity), live_set(&["peer:a"]));
    }

    #[test]
    fn live_node_ids_tolerates_a_prefixed_directory_and_empty() {
        let capacity = cap(&[("peer:a", 1, true)]);
        // Already-prefixed directory rows still reconcile.
        assert_eq!(
            live_node_ids(&live_set(&["peer:a"]), &capacity),
            live_set(&["peer:a"])
        );
        // Empty directory ⇒ nothing live.
        assert!(live_node_ids(&BTreeSet::new(), &capacity).is_empty());
    }

    #[test]
    fn is_failover_leader_is_the_lowest_live_node() {
        let live = live_set(&["peer:b", "peer:a", "peer:c"]);
        assert!(is_failover_leader("peer:a", &live));
        assert!(!is_failover_leader("peer:b", &live));
        // A lost leader (not in the live set) is not the leader — the next live
        // node takes over.
        let after_loss = live_set(&["peer:b", "peer:c"]);
        assert!(is_failover_leader("peer:b", &after_loss));
        // Empty live set ⇒ nobody leads.
        assert!(!is_failover_leader("peer:a", &BTreeSet::new()));
    }

    // ── desired-state persistence (fold + round-trip) ──

    #[test]
    fn fold_desired_is_latest_wins_by_request_id() {
        let d1 = serde_json::to_string(&dp(PlaceKind::Vm, "peer:a", "w1", Some("r1"))).unwrap();
        let moved = serde_json::to_string(&dp(PlaceKind::Vm, "peer:c", "w1", Some("r1"))).unwrap();
        let other =
            serde_json::to_string(&dp(PlaceKind::Container, "peer:a", "w2", Some("r2"))).unwrap();
        let map = fold_desired([d1.as_str(), other.as_str(), moved.as_str(), "garbage"]);
        assert_eq!(map.len(), 2);
        assert_eq!(map["r1"].chosen_host, "peer:c", "the later r1 record wins");
        assert_eq!(map["r2"].chosen_host, "peer:a");
    }

    #[test]
    fn fold_desired_keys_requestless_records_by_workload_identity() {
        // No request_id ⇒ keyed by (kind, spec), so a re-placement of the SAME
        // workload (new host) overwrites rather than duplicating.
        let a = serde_json::to_string(&dp(PlaceKind::Vm, "peer:a", "w9", None)).unwrap();
        let moved = serde_json::to_string(&dp(PlaceKind::Vm, "peer:c", "w9", None)).unwrap();
        let distinct = serde_json::to_string(&dp(PlaceKind::Vm, "peer:a", "other", None)).unwrap();
        let map = fold_desired([a.as_str(), moved.as_str(), distinct.as_str()]);
        assert_eq!(map.len(), 2);
        let w9 = map.values().find(|d| d.spec["name"] == "w9").expect("w9");
        assert_eq!(w9.chosen_host, "peer:c");
    }

    #[test]
    fn desired_placement_round_trips_through_a_placement() {
        let d = dp(PlaceKind::Container, "peer:a", "svc", Some("rid"));
        let p = d.to_placement(3, 42);
        // The audit envelope carried the seeds…
        assert_eq!(p.decision.candidates, 3);
        assert_eq!(p.decision.published_at_ms, 42);
        // …and the intent recovers losslessly.
        assert_eq!(DesiredPlacement::from_placement(&p).expect("recover"), d);
    }

    #[test]
    fn plan_placement_matches_the_desired_envelope() {
        // The MV-5a refactor is behavior-preserving: plan_placement's Placement is
        // exactly the desired-state for the chosen node built through to_placement.
        let capacity = cap(&[("peer:a", 2, true), ("peer:b", 5, true)]);
        let r = PlaceRequest {
            kind: PlaceKind::Vm,
            spec: serde_json::json!({ "name": "x" }),
            host: None,
            request_id: Some("r".into()),
        };
        let got = plan_placement(&capacity, &r, 7).expect("a placement");
        let expected = dp(PlaceKind::Vm, "peer:b", "x", Some("r")).to_placement(2, 7);
        assert_eq!(got, expected);
    }

    // ── failover tick wiring (seeded temp bus + injected directory) ──

    /// A [`LiveDirectory`] returning a fixed hostname set — the Fake seam.
    struct FakeDirectory(BTreeSet<NodeId>);
    impl LiveDirectory for FakeDirectory {
        fn live_hostnames(&self) -> BTreeSet<NodeId> {
            self.0.clone()
        }
    }

    #[tokio::test]
    async fn failover_tick_re_emits_and_repersists_on_node_loss() {
        use mde_bus::hooks::config::Priority;
        // Seed a temp bus: capacity for two live nodes + one desired placement on a
        // node whose lease has since lapsed (peer:b).
        let dir = std::env::temp_dir().join(format!("mde-sched-failover-{}", now_ms()));
        {
            let persist = Persist::open(dir.clone()).expect("open bus");
            for h in [health("peer:a", 2, true), health("peer:c", 5, true)] {
                persist
                    .write(
                        super::super::kvm_health::SERVICES_TOPIC,
                        Priority::Default,
                        None,
                        Some(&serde_json::to_string(&h).unwrap()),
                    )
                    .expect("write capacity");
            }
            persist
                .write(
                    DESIRED_TOPIC,
                    Priority::Default,
                    None,
                    Some(
                        &serde_json::to_string(&dp(PlaceKind::Vm, "peer:b", "w1", Some("r1")))
                            .unwrap(),
                    ),
                )
                .expect("write desired");
        }

        let rec = RecordingPublisher::default();
        let log = rec.sent.clone();
        // Directory reports only a + c (bare hostnames — peer:b is gone). host is
        // peer:a = lowest live ⇒ this node is the failover leader.
        let w = SchedulerWorker::new("peer:a".to_string())
            .with_publisher(Box::new(rec))
            .with_live_directory(Box::new(FakeDirectory(live_set(&["a", "c"]))));
        w.failover_once(&dir).await;

        let sent = log.lock().expect("recorder mutex");
        // 1. Re-emitted a host-targeted VM create for the healthiest live node.
        let action = sent
            .iter()
            .find(|(t, _)| t == super::super::vm_lifecycle::ACTION_TOPIC)
            .expect("re-emitted create");
        let body: serde_json::Value = serde_json::from_str(&action.1).unwrap();
        assert_eq!(body["op"], "create");
        assert_eq!(body["host"], "peer:c");
        assert_eq!(body["spec"]["name"], "w1");
        // 2. Persisted desired-state updated to the new home.
        let updated = sent
            .iter()
            .find(|(t, _)| t == DESIRED_TOPIC)
            .expect("re-persisted desired-state");
        let ud: DesiredPlacement = serde_json::from_str(&updated.1).unwrap();
        assert_eq!(ud.chosen_host, "peer:c");
        assert_eq!(ud.request_id.as_deref(), Some("r1"));
        // 3. Audit trail emitted.
        assert!(sent.iter().any(|(t, _)| t == PLACEMENTS_TOPIC));
        drop(sent);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn failover_tick_is_a_noop_for_a_non_leader() {
        use mde_bus::hooks::config::Priority;
        let dir = std::env::temp_dir().join(format!("mde-sched-nolead-{}", now_ms()));
        {
            let persist = Persist::open(dir.clone()).expect("open bus");
            for h in [health("peer:a", 2, true), health("peer:c", 5, true)] {
                persist
                    .write(
                        super::super::kvm_health::SERVICES_TOPIC,
                        Priority::Default,
                        None,
                        Some(&serde_json::to_string(&h).unwrap()),
                    )
                    .expect("write capacity");
            }
            persist
                .write(
                    DESIRED_TOPIC,
                    Priority::Default,
                    None,
                    Some(
                        &serde_json::to_string(&dp(PlaceKind::Vm, "peer:b", "w1", Some("r1")))
                            .unwrap(),
                    ),
                )
                .expect("write desired");
        }
        let rec = RecordingPublisher::default();
        let log = rec.sent.clone();
        // host peer:c is NOT the lowest live node (peer:a is) ⇒ it must not act.
        let w = SchedulerWorker::new("peer:c".to_string())
            .with_publisher(Box::new(rec))
            .with_live_directory(Box::new(FakeDirectory(live_set(&["a", "c"]))));
        w.failover_once(&dir).await;
        assert!(
            log.lock().expect("recorder mutex").is_empty(),
            "a non-leader re-places nothing"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn placement_persists_desired_state() {
        use mde_bus::hooks::config::Priority;
        // A place request + a single healthy node (⇒ leader + only candidate).
        let dir = std::env::temp_dir().join(format!("mde-sched-persist-{}", now_ms()));
        {
            let persist = Persist::open(dir.clone()).expect("open bus");
            persist
                .write(
                    super::super::kvm_health::SERVICES_TOPIC,
                    Priority::Default,
                    None,
                    Some(&serde_json::to_string(&health("peer:a", 3, true)).unwrap()),
                )
                .expect("write capacity");
            let request = PlaceRequest {
                kind: PlaceKind::Vm,
                spec: serde_json::json!({ "name": "web1" }),
                host: None,
                request_id: Some("r1".into()),
            };
            persist
                .write(
                    ACTION_TOPIC,
                    Priority::Default,
                    None,
                    Some(&serde_json::to_string(&request).unwrap()),
                )
                .expect("write request");
        }
        let rec = RecordingPublisher::default();
        let log = rec.sent.clone();
        let w = SchedulerWorker::new("peer:a".to_string()).with_publisher(Box::new(rec));
        let mut cursor = None;
        w.drain_and_place(&dir, &mut cursor).await;

        let sent = log.lock().expect("recorder mutex");
        // The desired-state is persisted alongside the create + audit, carrying the
        // full intent so a later failover can re-place it.
        let desired = sent
            .iter()
            .find(|(t, _)| t == DESIRED_TOPIC)
            .expect("persisted desired-state");
        let d: DesiredPlacement = serde_json::from_str(&desired.1).unwrap();
        assert_eq!(d.kind, PlaceKind::Vm);
        assert_eq!(d.chosen_host, "peer:a");
        assert_eq!(d.spec["name"], "web1");
        assert_eq!(d.request_id.as_deref(), Some("r1"));
        // …and the host-targeted create was emitted too.
        assert!(sent
            .iter()
            .any(|(t, _)| t == super::super::vm_lifecycle::ACTION_TOPIC));
        drop(sent);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── run loop (injected recorder, no bus binary) ──

    /// A [`Publisher`] that records every publish for assertions — the Fake
    /// seam. The log is an `Arc` so a test can clone a handle to it before
    /// moving the worker into its task.
    #[derive(Clone, Default)]
    struct RecordingPublisher {
        sent: std::sync::Arc<Mutex<Vec<(String, String)>>>,
    }

    impl Publisher for RecordingPublisher {
        fn publish(&self, topic: &str, body: &str) {
            self.sent
                .lock()
                .expect("recorder mutex")
                .push((topic.to_string(), body.to_string()));
        }
    }

    #[tokio::test]
    async fn run_loop_exits_promptly_on_shutdown() {
        // An empty temp bus root ⇒ the drain reads nothing and publishes
        // nothing; the injected recorder means no `mde-bus` binary is needed.
        let dir = std::env::temp_dir().join(format!("mde-sched-test-{}", now_ms()));
        let rec = RecordingPublisher::default();
        let log = rec.sent.clone();
        let (tx, rx) = tokio::sync::watch::channel(false);
        let mut w = SchedulerWorker::new("node".to_string())
            .with_publisher(Box::new(rec))
            .with_bus_root(dir)
            .with_poll(Duration::from_millis(10));
        let token = ShutdownToken::from_receiver(rx);
        let handle = tokio::spawn(async move { w.run(token).await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        tx.send(true).expect("signal shutdown");
        let joined = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(joined.is_ok(), "worker must exit promptly on shutdown");
        // Nothing to place from an empty bus ⇒ nothing published.
        assert!(log.lock().expect("recorder mutex").is_empty());
        assert!(joined.unwrap().expect("join").is_ok());
    }
}
