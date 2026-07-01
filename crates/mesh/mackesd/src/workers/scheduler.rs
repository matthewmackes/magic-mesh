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
//! ## Scope — placement ONLY
//!
//! This slice is *place-on-request*: choose a node and forward one create/run.
//! **Deferred to MV-5b** (NOT stubbed here): etcd desired-state persistence,
//! failover / re-placement on node loss (incl. HA leader re-election beyond the
//! interim [`is_leader`] guard), and Cockpit (already installed by
//! `node-virt.yml`).

#![cfg(feature = "async-services")]

use std::collections::BTreeMap;
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
    // Forward the opaque spec under the actuator's birth verb, host-targeted at
    // the chosen node — the same envelope vm_lifecycle/container drain.
    let action_body = serde_json::json!({
        "op": req.kind.birth_op(),
        "host": chosen,
        "spec": req.spec,
    })
    .to_string();
    let decision = PlacementDecision {
        request_id: req.request_id.clone(),
        kind: req.kind,
        chosen_host: chosen.clone(),
        candidates: candidates.len(),
        published_at_ms: now_ms,
    };
    Some(Placement {
        chosen_host: chosen,
        action_topic: req.kind.action_topic(),
        action_body,
        decision,
    })
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

/// The MV-5a placement worker.
pub struct SchedulerWorker {
    /// This node's id — its identity in the [`is_leader`] election.
    host: NodeId,
    /// The injectable publish seam (production: [`BusPublisher`]).
    publisher: Box<dyn Publisher + Send + Sync>,
    /// Action-drain cadence.
    poll: Duration,
    /// Bus root override (tests). `None` ⇒ [`default_bus_root`].
    bus_root_override: Option<PathBuf>,
}

impl SchedulerWorker {
    /// Construct with production defaults: the live [`BusPublisher`], the default
    /// cadence, and the auto-resolved bus root. `host` is this node's id.
    #[must_use]
    pub fn new(host: NodeId) -> Self {
        Self {
            host,
            publisher: Box::new(BusPublisher),
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
    use std::sync::{Arc, Mutex};

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
