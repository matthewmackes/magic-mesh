//! WL-ARCH-001 Phase B + Workloads U2 — the mackesd `cloud` worker: the
//! **OpenTofu + Ansible cloud backend** over local libvirt/KVM.
//!
//! The worker is the mesh-side runner + status publisher for that stack. It:
//!
//! 1. **Drains `action/cloud/*` verbs off the Bus** ([`CLOUD_ACTION_PREFIX`]) and
//!    answers each with a neutral `CloudReply` on `reply/<ulid>`. Verbs:
//!    `list`/`list-instances` + `status` (READS), `provision` / `configure` /
//!    `destroy` and `instance-{start,stop,reboot,delete}` (MUTATIONS), plus the
//!    U1a Workloads verbs (`set-desired`/`plan`/`inventory`/…) as honest skeletons.
//! 2. **Shells OpenTofu + Ansible + virsh** through the injectable
//!    [`CloudRunner`](runner::CloudRunner) seam (production
//!    [`ShellCloudRunner`](runner::ShellCloudRunner); tests inject a fake).
//! 3. **Publishes `state/cloud/<node>`** — a [`CloudState`] carrying per-tool
//!    backend health + the resource roster, built entirely from the neutral
//!    `mackes_mesh_types::cloud` types.
//!
//! ## The two U2 gates (this module's drain wires them)
//!
//! - **Armed-token gate** ([`gate`]) — replaces the retired `MDE_CLOUD_APPLY=1`
//!   env wall. A live mutation is authorized by a mesh-identity-signed **armed
//!   token** (nonce + expiry, bound to the verb + placement node). `CloudState.
//!   apply_armed` is reinterpreted as *token-arming available on this node* (a
//!   capability, not a wall). `destroy` additionally requires a `typed_name`
//!   confirmation (== the destroy target).
//! - **Placement gate** ([`gate::placement_match`]) — replaces the leader gate.
//!   Every node drains `action/cloud/*`, but performs a MUTATION iff `body.node ==
//!   self.host`; a mutation for another node is that node's to perform, and a
//!   mutation for an *unreachable* node is honestly gated (never a silent swallow).
//!   Reads stay local on every node (each answers about its own roster).
//!
//! The split (`runner`/`gate`/`verbs`/`reconcile` + this run loop) is the worker
//! serialize point: U4–U10 each own a disjoint verb/reconcile handler after U2.

#![cfg(feature = "async-services")]

mod gate;
mod reconcile;
mod render;
mod runner;
mod verbs;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mackes_mesh_types::cloud::{
    cloud_state_topic, CloudProviderAdapter, CloudReply, CloudState, DriftSummary, NodeCapacity,
    ServiceHealth, WorkloadRow, CLOUD_ACTION_PREFIX,
};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;

use super::{ShutdownToken, Worker};

use gate::{placement_match, HmacTokenSigner, NullSigner, Placement, TokenSigner};
use runner::{
    default_iac_root, default_libvirt_uri, instances_table, CloudRunOutcome, CloudRunner,
    ShellCloudRunner, BACKEND_TOOLS,
};
use verbs::{CloudActionBody, CloudVerb};

// The armed-token capability the Workloads surface (a later unit) mints — exported
// so the module path stays stable across the split. `CloudRunner` + `TokenSigner`
// are reachable through the `with_runner` / `with_signer` builder signatures.
pub use gate::ArmedToken;

/// Action-drain cadence — a verb lands within ~3 s (as `router_action` / `container`).
pub const POLL: Duration = Duration::from_secs(3);

/// Unconditional `state/cloud/<node>` republish cadence (between change publishes).
pub const PUBLISH_HEARTBEAT: Duration = Duration::from_secs(60);

/// The throttled drift-plan cadence — a periodic `tofu plan` of THIS node's desired
/// slice, decoupled from [`PUBLISH_HEARTBEAT`] because a plan is far heavier than a
/// health probe (U5). A fresh drift snapshot forces an out-of-band mirror republish.
pub const DRIFT_POLL: Duration = Duration::from_secs(300);

/// A placement node is considered reachable while its `state/cloud/<node>` mirror
/// is fresher than this (3× the publish heartbeat — a wide margin so a node
/// mid-heartbeat is never falsely gated). A mutation for a node whose mirror is
/// staler than this (or absent) is honestly gated as "not reachable".
const PLACEMENT_STALE_AFTER_MS: i64 = 3 * 60 * 1000;

// ─────────────────────────── the worker ───────────────────────────

/// The `cloud` worker (per-node, rank-0 universal). The action drain is
/// placement-routed (not leader-gated); the `state/cloud/<node>` mirror is
/// per-node universal.
pub struct CloudWorker {
    /// This node's id — the `state/cloud/<host>` namespace, the placement key, and
    /// the audit actor's node.
    host: String,
    /// The mesh node id (`peer:<host>`) — the audit actor identity.
    node_id: String,
    /// The injectable backend seam (production: [`ShellCloudRunner`]).
    runner: Arc<dyn CloudRunner>,
    /// The armed-token verification/signing seam (production: keyed
    /// [`HmacTokenSigner`]; a node with no arming key uses [`NullSigner`], staging
    /// every mutation).
    signer: Arc<dyn TokenSigner>,
    /// Whether token-arming is available on this node (a capability, published as
    /// `CloudState.apply_armed`). `false` ⇒ this node has no arming key and stages
    /// every mutation honestly.
    arm_capable: bool,
    /// The workgroup / desired-state root — the per-node desired store
    /// (`<state_root>/mcnf/cloud/desired/<node>/…`) U4's `set-desired` writes and
    /// U5's reconcile/drift tick reads.
    state_root: PathBuf,
    /// The hash-chain audit DB (destructive performed ops audit here).
    db_path: PathBuf,
    /// The Bus root the mirror publish targets + the action drain reads (`None` ⇒
    /// publish/drain is a no-op — a pre-RPM dev box with no bus).
    bus_root: Option<PathBuf>,
    /// Fold/publish cadence.
    poll: Duration,
    /// Mirror republish heartbeat.
    heartbeat: Duration,
    /// Test-only reachability override (the set of nodes considered reachable).
    /// `None` ⇒ the real bus-mirror freshness check.
    reachable_override: Option<HashSet<String>>,
    /// The throttled drift-plan cadence (decoupled from the mirror heartbeat).
    drift_interval: Duration,
    /// The most recent drift snapshot — per-workload [`WorkloadRow`]s + the node
    /// [`DriftSummary`] the throttled tick ([`Self::refresh_drift`]) computed, folded
    /// into every `state/cloud/<node>` mirror. Empty until the first tick.
    drift: std::sync::Mutex<(Vec<WorkloadRow>, DriftSummary)>,
}

impl CloudWorker {
    /// Construct with production defaults: the [`ShellCloudRunner`] over the
    /// deployed IaC tree + local libvirt, the armed-token signer from the mesh
    /// arming key seam ([`gate::ARM_KEY_ENV`]; absent ⇒ arming unavailable), the
    /// honest reconcile skeleton, the canonical audit DB, and the persisted Bus
    /// tree. `host` is this node's id; `node_id` is the audit actor;
    /// `workgroup_root` is the desired-state / tfvars root (reserved for the
    /// reconcile seam).
    #[must_use]
    pub fn new(host: String, node_id: String, workgroup_root: PathBuf) -> Self {
        let runner = Arc::new(ShellCloudRunner::new(
            &default_iac_root(),
            default_libvirt_uri(),
        ));
        // The arming key seam: a node holding the mesh arming key can verify tokens
        // (arm-capable); a node without one stages every mutation honestly.
        let (signer, arm_capable): (Arc<dyn TokenSigner>, bool) = match HmacTokenSigner::from_env()
        {
            Some(s) => (Arc::new(s), true),
            None => (Arc::new(NullSigner), false),
        };
        Self {
            host,
            node_id,
            runner,
            signer,
            arm_capable,
            state_root: workgroup_root,
            db_path: crate::default_db_path(),
            bus_root: default_bus_root(),
            poll: POLL,
            heartbeat: PUBLISH_HEARTBEAT,
            reachable_override: None,
            drift_interval: DRIFT_POLL,
            drift: std::sync::Mutex::new((Vec::new(), DriftSummary::default())),
        }
    }

    /// Inject a backend runner (tests supply a fake).
    #[must_use]
    pub fn with_runner(mut self, runner: Arc<dyn CloudRunner>) -> Self {
        self.runner = runner;
        self
    }

    /// Inject the armed-token signer (the arming seam). Setting a signer marks the
    /// node arm-capable — it can now verify tokens.
    #[must_use]
    pub fn with_signer(mut self, signer: Arc<dyn TokenSigner>) -> Self {
        self.signer = signer;
        self.arm_capable = true;
        self
    }

    /// Override the arm-capable capability flag independently of the signer (tests
    /// of the published `apply_armed` capability signal).
    #[must_use]
    pub const fn with_arm_capable(mut self, capable: bool) -> Self {
        self.arm_capable = capable;
        self
    }

    /// Override the audit DB path (tests point it at a tempdir).
    #[must_use]
    pub fn with_db_path(mut self, p: PathBuf) -> Self {
        self.db_path = p;
        self
    }

    /// Override the Bus root (tests point it at a tempdir; `None` disables it).
    #[must_use]
    pub fn with_bus_root(mut self, root: Option<PathBuf>) -> Self {
        self.bus_root = root;
        self
    }

    /// Override the fold cadence (tests, to avoid multi-second waits).
    #[must_use]
    pub const fn with_poll(mut self, poll: Duration) -> Self {
        self.poll = poll;
        self
    }

    /// Override the drift-plan cadence (tests, to force a tick — or to push it far
    /// out so a fast-poll test never shells `tofu`).
    #[must_use]
    pub const fn with_drift_interval(mut self, interval: Duration) -> Self {
        self.drift_interval = interval;
        self
    }

    /// Override the placement reachability oracle with an explicit reachable-node
    /// set (tests) — bypasses the bus-mirror freshness check so placement routing
    /// is deterministic without live peers.
    #[must_use]
    pub fn with_reachable_nodes(mut self, nodes: Option<HashSet<String>>) -> Self {
        self.reachable_override = nodes;
        self
    }

    /// Write one hash-chain audit row for a performed destructive cloud op through
    /// the EXISTING events plane (best-effort — a store fault is logged, never
    /// fatal). Makes the reply's `audited: true` truthful.
    pub(crate) fn audit(&self, verb: &str, instance: Option<&str>, outcome: &CloudRunOutcome) {
        crate::events::append_and_alert(
            &self.db_path,
            &self.node_id,
            crate::events::EventKind::AdminAction,
            serde_json::json!({
                "action": "cloud",
                "verb": verb,
                "instance": instance,
                "ok": outcome.ok,
                "applied": outcome.applied,
                "summary": outcome.summary,
            }),
        );
    }

    /// Handle one `action/cloud/<verb>` request end to end → a typed [`CloudReply`].
    /// This is the per-request logic assuming THIS node performs it; the drain's
    /// placement gate decides which node calls it for a mutation.
    #[must_use]
    pub fn handle(&self, verb_name: &str, body: &str) -> CloudReply {
        verbs::dispatch(self, verb_name, body)
    }

    /// Whether placement node `node` is reachable — the test override when set,
    /// else a fresh `state/cloud/<node>` mirror on the bus (a node publishing its
    /// mirror is up and running its cloud worker).
    fn node_reachable(&self, persist: &Persist, node: &str) -> bool {
        if let Some(set) = &self.reachable_override {
            return set.contains(node);
        }
        match persist.read_latest(&cloud_state_topic(node)) {
            Ok(Some(msg)) => msg
                .body
                .as_deref()
                .and_then(|b| serde_json::from_str::<CloudState>(b).ok())
                .is_some_and(|st| {
                    now_ms().saturating_sub(st.published_at_ms) <= PLACEMENT_STALE_AFTER_MS
                }),
            _ => false,
        }
    }

    /// Write a typed reply to `reply/<request-ulid>` (best-effort).
    fn write_reply(&self, persist: &Persist, req_ulid: &str, reply: &CloudReply) {
        let body = serde_json::to_string(reply).unwrap_or_default();
        if let Err(e) = persist.write(&reply_topic(req_ulid), Priority::Default, None, Some(&body))
        {
            tracing::warn!(target: "mackesd::cloud", ulid = %req_ulid, error = %e, "cloud reply write failed");
        }
    }

    /// Drain every new `action/cloud/*` request, advance the per-topic cursors, and
    /// route each by PLACEMENT (not leadership):
    ///
    /// - a READ is served locally on every node (each answers its own roster);
    /// - a MUTATION is performed iff its `body.node` is this host (or is empty —
    ///   the legacy node-agnostic path);
    /// - a MUTATION for another node is skipped when that node is reachable (it
    ///   performs + replies), and honestly gated (`placement node <N> not
    ///   reachable`) when it is not — never a silent swallow.
    ///
    /// Returns `true` when any request was handled (so the caller force-republishes
    /// the fresh roster).
    fn drain_actions(&self, cursors: &mut HashMap<String, String>) -> bool {
        let Some(root) = self.bus_root.clone() else {
            return false;
        };
        let Ok(persist) = Persist::open(root) else {
            return false;
        };
        let Ok(topics) = persist.list_topics() else {
            return false;
        };
        let mut acted = false;
        for topic in topics {
            let Some(verb_name) = topic.strip_prefix(CLOUD_ACTION_PREFIX) else {
                continue;
            };
            let verb_name = verb_name.to_string();
            let is_mutation = CloudVerb::from_verb(&verb_name).is_some_and(CloudVerb::is_mutation);
            let cursor = cursors.get(&topic).cloned();
            let Ok(msgs) = persist.list_since(&topic, cursor.as_deref()) else {
                continue;
            };
            for msg in msgs {
                cursors.insert(topic.clone(), msg.ulid.clone());
                let body = msg.body.as_deref().unwrap_or("{}");
                // Placement routing: reads stay local; a mutation goes to its node.
                let route = if is_mutation {
                    let parsed = CloudActionBody::parse(body);
                    match placement_match(&parsed.node, &self.host) {
                        Placement::Local => Route::Handle,
                        Placement::Remote(n) => {
                            if self.node_reachable(&persist, &n) {
                                Route::Skip
                            } else {
                                Route::GateUnreachable(n)
                            }
                        }
                    }
                } else {
                    Route::Handle
                };

                match route {
                    Route::Handle => {
                        let reply = self.handle(&verb_name, body);
                        tracing::info!(
                            target: "mackesd::cloud",
                            ulid = %msg.ulid, verb = %verb_name, ok = reply.ok,
                            audited = reply.audited, "cloud action handled (placement-local)"
                        );
                        self.write_reply(&persist, &msg.ulid, &reply);
                        acted = true;
                    }
                    Route::Skip => {}
                    Route::GateUnreachable(n) => {
                        let reply = CloudReply {
                            ok: false,
                            verb: verb_name.clone(),
                            gated: Some(format!("placement node {n} not reachable")),
                            ..Default::default()
                        };
                        tracing::info!(
                            target: "mackesd::cloud",
                            ulid = %msg.ulid, verb = %verb_name, node = %n,
                            "cloud mutation honestly gated — placement target unreachable"
                        );
                        self.write_reply(&persist, &msg.ulid, &reply);
                        acted = true;
                    }
                }
            }
        }
        acted
    }

    /// Seed each existing `action/cloud/*` topic's cursor to its newest message so
    /// a (re)start doesn't replay a backlog of verbs.
    fn prime_cursors(&self, cursors: &mut HashMap<String, String>) {
        let Some(root) = self.bus_root.clone() else {
            return;
        };
        let Ok(persist) = Persist::open(root) else {
            return;
        };
        let Ok(topics) = persist.list_topics() else {
            return;
        };
        for topic in topics {
            if !topic.starts_with(CLOUD_ACTION_PREFIX) {
                continue;
            }
            if let Ok(Some(ulid)) = persist.latest_ulid(&topic) {
                cursors.insert(topic, ulid);
            }
        }
    }

    /// Run one throttled drift tick: render + `tofu plan` THIS node's desired slice,
    /// fold the live roster into per-workload rows + the node drift rollup, and cache
    /// it for the next `state/cloud/<node>` publish. Best-effort + honest (§7) — a
    /// plan the backend can't run leaves each row's drift `Unknown`, never a
    /// fabricated in-sync. A no-op when the node has nothing declared (empty slice).
    fn refresh_drift(&self) {
        let snapshot = reconcile::drift_snapshot(
            self.runner.as_ref(),
            &self.state_root,
            &self.host,
            &default_libvirt_uri(),
            now_ms(),
        );
        if let Ok(mut guard) = self.drift.lock() {
            *guard = snapshot;
        }
    }

    /// Build the current `state/cloud/<node>` mirror: probe each backend tool's
    /// health + fold the live roster into a resource table (all neutral types), plus
    /// the latest drift tick's per-workload rows + rollup (U5).
    #[must_use]
    pub fn build_state(&self) -> CloudState {
        let health: Vec<ServiceHealth> = BACKEND_TOOLS
            .iter()
            .map(|t| self.runner.probe_tool(t))
            .collect();
        let resources = match self.runner.list_instances() {
            Ok(instances) => vec![instances_table(&instances)],
            Err(_) => Vec::new(),
        };
        // Fold in the throttled drift tick's latest snapshot (empty until the first
        // tick / a node with nothing declared).
        let (workloads, drift_summary) = self.drift.lock().map(|g| g.clone()).unwrap_or_default();
        CloudState {
            host: self.host.clone(),
            adapter: CloudProviderAdapter::ConstructCloud,
            health,
            resources,
            // `apply_armed` is now the token-arming CAPABILITY of this node, not the
            // retired env wall — whether this node can honor an armed mutation.
            apply_armed: self.arm_capable,
            published_at_ms: now_ms(),
            workloads,
            drift_summary,
            node_capacity: NodeCapacity::default(),
        }
    }

    /// Publish the current mirror to `state/cloud/<host>` (best-effort).
    fn publish_state(&self) {
        let state = self.build_state();
        if let Some(mut persist) =
            crate::bus_publish::open_bus(self.bus_root.as_ref().map(PathBuf::clone))
        {
            crate::bus_publish::publish_json(&mut persist, &cloud_state_topic(&self.host), &state);
        }
    }
}

/// The placement decision for one drained mutation message.
enum Route {
    /// This node performs the request (a read, or a mutation placed here).
    Handle,
    /// Another (reachable) node performs it — do nothing.
    Skip,
    /// The placement target is unreachable — reply honest-gated (no silent swallow).
    GateUnreachable(String),
}

#[async_trait::async_trait]
impl Worker for CloudWorker {
    fn name(&self) -> &'static str {
        "cloud"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let mut cursors: HashMap<String, String> = HashMap::new();
        // Don't replay a backlog of verbs across a restart.
        self.prime_cursors(&mut cursors);
        // Publish an initial mirror so a surface doesn't wait a full tick.
        self.publish_state();
        let mut last_pub = Instant::now();
        // The drift plan runs on its own (heavier) cadence, decoupled from the
        // health heartbeat; a fresh snapshot forces an out-of-band republish.
        let mut last_drift = Instant::now();
        loop {
            let acted = self.drain_actions(&mut cursors);
            let drift_due = last_drift.elapsed() >= self.drift_interval;
            if drift_due {
                self.refresh_drift();
                last_drift = Instant::now();
            }
            if acted || drift_due || last_pub.elapsed() >= self.heartbeat {
                self.publish_state();
                last_pub = Instant::now();
            }
            tokio::select! {
                () = shutdown.wait() => return Ok(()),
                () = tokio::time::sleep(self.poll) => {}
            }
        }
    }
}

// ─────────────────────────── small helpers ───────────────────────────

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn default_bus_root() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("mde").join("bus"))
}

#[cfg(test)]
mod tests {
    use super::gate::{ArmedToken, HmacTokenSigner};
    use super::runner::fake::{instance, FakeRunner};
    use super::runner::{TOOL_LIBVIRT, TOOL_TOFU};
    use super::*;
    use mackes_mesh_types::cloud::{CloudProviderAdapter, HealthState};

    const KEY: &[u8] = b"test-mesh-arming-key";

    fn signer() -> HmacTokenSigner {
        HmacTokenSigner::new(KEY.to_vec())
    }

    fn far_future() -> i64 {
        now_ms() + 3_600_000
    }

    /// A valid armed token for `(verb, node)` signed with the shared test key.
    fn valid_token(verb: &str, node: &str) -> String {
        ArmedToken::mint(&signer(), "nonce-12345678", far_future(), verb, node).encode()
    }

    /// A worker whose signer holds the test key (arm-capable) — armed mutations apply.
    fn armed_worker(runner: Arc<dyn CloudRunner>) -> CloudWorker {
        CloudWorker::new("me".into(), "peer:me".into(), PathBuf::from("/tmp"))
            .with_runner(runner)
            .with_signer(Arc::new(signer()))
            .with_bus_root(None)
    }

    /// A worker with no arming key (NullSigner) — every mutation stages honestly.
    fn staged_worker(runner: Arc<dyn CloudRunner>) -> CloudWorker {
        CloudWorker::new("me".into(), "peer:me".into(), PathBuf::from("/tmp"))
            .with_runner(runner)
            .with_bus_root(None)
    }

    // ── list / status reads ──
    #[test]
    fn list_returns_the_roster_and_matches_the_kdc_contract() {
        let runner = Arc::new(FakeRunner {
            roster: vec![instance("web", "ACTIVE"), instance("db", "SHUTOFF")],
            ..Default::default()
        });
        let w = staged_worker(runner);
        for verb in ["list", "list-instances", "status"] {
            let reply = w.handle(verb, "{}");
            assert!(reply.ok, "{verb} ok");
            let instances = reply.instances.expect("roster");
            assert_eq!(instances.len(), 2);
            assert_eq!(instances[0].name, "web");
        }
    }

    #[test]
    fn a_read_against_an_unreachable_backend_is_gated_not_faked() {
        let runner = Arc::new(FakeRunner {
            roster_err: Some("libvirt unavailable".into()),
            ..Default::default()
        });
        let w = staged_worker(runner);
        let reply = w.handle("list", "{}");
        assert!(!reply.ok);
        assert!(reply.instances.is_none(), "no fabricated empty roster");
        assert!(reply.gated.unwrap().contains("not ready"));
    }

    // ── the armed-token gate: staged (no/invalid token) vs armed ──
    #[test]
    fn a_mutation_without_a_token_stages_a_dry_run_and_applies_nothing() {
        let runner = Arc::new(FakeRunner::default());
        let w = staged_worker(runner.clone());
        let reply = w.handle("provision", r#"{"node":"me"}"#);
        assert!(!reply.ok, "a staged mutation is not a fabricated success");
        let gated = reply.gated.unwrap();
        assert!(gated.contains("no armed token"), "{gated}");
        assert!(gated.contains("nothing applied"), "{gated}");
        // The runner ran a plan (apply=false), never an apply.
        assert_eq!(
            runner.calls.lock().unwrap().as_slice(),
            &[("provision".into(), false)]
        );
    }

    #[test]
    fn an_armed_mutation_applies_and_is_not_audited_when_non_destructive() {
        let runner = Arc::new(FakeRunner::default());
        let w = armed_worker(runner.clone());
        let body = format!(
            r#"{{"node":"me","armed_token":"{}"}}"#,
            valid_token("provision", "me")
        );
        let reply = w.handle("provision", &body);
        assert!(reply.ok, "gated: {:?}", reply.gated);
        assert!(!reply.audited, "provision is not destructive");
        assert_eq!(
            runner.calls.lock().unwrap().as_slice(),
            &[("provision".into(), true)]
        );
    }

    #[test]
    fn an_expired_or_forged_token_stages_and_never_applies() {
        let runner = Arc::new(FakeRunner::default());
        let w = armed_worker(runner.clone());
        // A token minted by a different key is forged from this worker's vantage.
        let forged = ArmedToken::mint(
            &HmacTokenSigner::new(b"other".to_vec()),
            "nonce-12345678",
            far_future(),
            "provision",
            "me",
        )
        .encode();
        let body = format!(r#"{{"node":"me","armed_token":"{forged}"}}"#);
        let reply = w.handle("provision", &body);
        assert!(!reply.ok);
        assert!(reply.gated.unwrap().contains("signature did not verify"));
        assert_eq!(
            runner.calls.lock().unwrap().as_slice(),
            &[("provision".into(), false)],
            "a forged token never applies"
        );
    }

    // ── destroy: armed + typed-arming confirmation ──
    #[test]
    fn an_armed_destroy_with_typed_name_applies_audits_and_reports_audited() {
        let tmp = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::default());
        let w = armed_worker(runner.clone()).with_db_path(tmp.path().join("events.sqlite"));
        let body = format!(
            r#"{{"node":"me","name":"me","typed_name":"me","armed_token":"{}"}}"#,
            valid_token("destroy", "me")
        );
        let reply = w.handle("destroy", &body);
        assert!(reply.ok, "gated: {:?} err: {:?}", reply.gated, reply.error);
        assert!(reply.audited, "a performed destroy is audited");
        assert_eq!(
            runner.calls.lock().unwrap().as_slice(),
            &[("destroy".into(), true)]
        );
    }

    #[test]
    fn an_armed_destroy_without_the_typed_name_confirmation_is_blocked() {
        let runner = Arc::new(FakeRunner::default());
        let w = armed_worker(runner.clone());
        // Armed, but the typed_name confirmation is missing → blocked, nothing applied.
        let body = format!(
            r#"{{"node":"me","name":"me","armed_token":"{}"}}"#,
            valid_token("destroy", "me")
        );
        let reply = w.handle("destroy", &body);
        assert!(!reply.ok);
        assert!(reply.error.unwrap().contains("typed_name"));
        assert!(
            runner.calls.lock().unwrap().is_empty(),
            "a blocked destroy never runs the backend"
        );
    }

    #[test]
    fn a_lifecycle_verb_requires_an_instance_and_routes_the_action() {
        let runner = Arc::new(FakeRunner::default());
        let w = armed_worker(runner.clone());
        // Missing instance → honest rejection, no runner call.
        let bad = w.handle("instance-start", r#"{"node":"me"}"#);
        assert!(!bad.ok && bad.error.unwrap().contains("instance"));
        assert!(runner.calls.lock().unwrap().is_empty());
        // With an instance + a valid token → the start action applies.
        let body = format!(
            r#"{{"node":"me","instance":"web","armed_token":"{}"}}"#,
            valid_token("instance-start", "me")
        );
        let good = w.handle("instance-start", &body);
        assert!(good.ok, "gated: {:?}", good.gated);
        assert_eq!(
            runner.calls.lock().unwrap().as_slice(),
            &[("lifecycle-start".into(), true)]
        );
    }

    #[test]
    fn an_unknown_verb_is_an_honest_error() {
        let w = staged_worker(Arc::new(FakeRunner::default()));
        let reply = w.handle("frobnicate", "{}");
        assert!(!reply.ok);
        assert!(reply.error.unwrap().contains("unknown cloud verb"));
    }

    #[test]
    fn every_workloads_verb_is_wired_no_skeleton_remains() {
        // All eight Workloads verbs are wired: set-desired/plan (U4), image-build (U6),
        // container-deploy (U7), inventory/output (U10), console-attach (U8),
        // android-provision (U9). None may still surface the U2 "not yet wired"
        // skeleton — a verb may honestly gate (armed-token / tool-absent), but the
        // skeleton message is a regression. Each verb's real behavior is covered by
        // its own module tests.
        let w = staged_worker(Arc::new(FakeRunner::default()));
        for verb in [
            "set-desired",
            "plan",
            "image-build",
            "container-deploy",
            "inventory",
            "output",
            "console-attach",
            "android-provision",
        ] {
            let reply = w.handle(verb, r#"{"node":"me"}"#);
            let gated = reply.gated.unwrap_or_default();
            let err = reply.error.unwrap_or_default();
            assert!(
                !gated.contains("not yet wired") && !err.contains("not yet wired"),
                "{verb} still returns the not-yet-wired skeleton: gated={gated} err={err}"
            );
        }
    }

    // ── the state mirror ──
    #[test]
    fn build_state_reports_the_arming_capability_and_the_roster_table() {
        let runner = Arc::new(FakeRunner {
            roster: vec![instance("web", "ACTIVE")],
            tofu_up: true,
            ..Default::default()
        });
        // Arm-capable node ⇒ apply_armed capability true.
        let w = armed_worker(runner);
        let state = w.build_state();
        assert_eq!(state.host, "me");
        assert_eq!(state.adapter, CloudProviderAdapter::ConstructCloud);
        assert!(
            state.apply_armed,
            "an arm-capable node advertises the capability"
        );
        assert_eq!(
            state.tool_health(TOOL_TOFU).map(|h| h.state),
            Some(HealthState::Up)
        );
        assert_eq!(
            state.tool_health(TOOL_LIBVIRT).map(|h| h.state),
            Some(HealthState::Absent)
        );
        assert_eq!(state.resources.len(), 1);
        assert_eq!(state.resources[0].rows.len(), 1);
        // A node without an arming key advertises no capability (stages).
        let w2 = staged_worker(Arc::new(FakeRunner::default()));
        assert!(!w2.build_state().apply_armed);
    }

    // ── the U5 drift tick folds workloads + rollup into the mirror ──
    #[test]
    fn a_drift_tick_folds_workload_rows_and_the_rollup_into_the_mirror() {
        use mackes_mesh_types::cloud::{DeliveryType, DriftFlag, WorkloadSpec};
        let tmp = tempfile::tempdir().unwrap();
        // Declare a workload on this node in its per-node desired slice.
        let spec = WorkloadSpec {
            name: "web".into(),
            delivery_type: DeliveryType::ServiceVm,
            node: "me".into(),
            vcpu: 2,
            memory_mb: 2048,
            disk_gb: 20,
            image: None,
            network_isolation: false,
            raw_hcl: None,
        };
        super::reconcile::write_desired_doc(tmp.path(), &spec).unwrap();
        // A plan that reports pending changes ⇒ the workload is drifted.
        let runner = Arc::new(FakeRunner {
            roster: vec![instance("web", "ACTIVE")],
            plan_ndjson: Some(
                r#"{"type":"change_summary","changes":{"add":1,"change":0,"remove":0}}"#.into(),
            ),
            ..Default::default()
        });
        let w = CloudWorker::new("me".into(), "peer:me".into(), tmp.path().to_path_buf())
            .with_runner(runner)
            .with_bus_root(None);
        // Before a tick the mirror carries no workloads (never fabricated).
        assert!(w.build_state().workloads.is_empty());
        w.refresh_drift();
        let state = w.build_state();
        assert_eq!(state.workloads.len(), 1);
        assert_eq!(state.workloads[0].name, "web");
        assert_eq!(state.workloads[0].drift, DriftFlag::Drift);
        assert_eq!(state.drift_summary.drift_count, 1);
        assert!(state.drift_summary.last_plan_ms > 0);
    }

    // ── placement-routed drain ──
    #[tokio::test]
    async fn drain_performs_a_mutation_only_on_its_placement_node() {
        let tmp = tempfile::tempdir().unwrap();
        let bus = tmp.path().to_path_buf();
        let persist = Persist::open(bus.clone()).unwrap();
        // A mutation placed on node "l", armed for "l".
        let body = format!(
            r#"{{"node":"l","armed_token":"{}"}}"#,
            valid_token("provision", "l")
        );
        let req = persist
            .write(
                "action/cloud/provision",
                Priority::Default,
                None,
                Some(&body),
            )
            .unwrap();

        // The NON-placement node "f" (with "l" reachable) drains: it advances its
        // cursor but writes NO reply — the target node performs it.
        let follower = CloudWorker::new("f".into(), "peer:f".into(), tmp.path().to_path_buf())
            .with_runner(Arc::new(FakeRunner::default()))
            .with_bus_root(Some(bus.clone()))
            .with_reachable_nodes(Some(HashSet::from(["l".to_string()])));
        let mut cursors = HashMap::new();
        follower.drain_actions(&mut cursors);
        assert!(
            persist
                .list_since(&reply_topic(&req.ulid), None)
                .unwrap()
                .is_empty(),
            "a non-placement node must not reply for a reachable target"
        );

        // The placement node "l" (arm-capable) drains: it performs + replies ok.
        let runner = Arc::new(FakeRunner::default());
        let leader = CloudWorker::new("l".into(), "peer:l".into(), tmp.path().to_path_buf())
            .with_runner(runner.clone())
            .with_signer(Arc::new(signer()))
            .with_bus_root(Some(bus.clone()));
        let mut cursors = HashMap::new();
        assert!(
            leader.drain_actions(&mut cursors),
            "the placement node acted"
        );
        let replies = persist.list_since(&reply_topic(&req.ulid), None).unwrap();
        assert_eq!(replies.len(), 1, "exactly one reply");
        let reply: CloudReply = serde_json::from_str(replies[0].body.as_deref().unwrap()).unwrap();
        assert!(reply.ok, "gated: {:?}", reply.gated);
        assert_eq!(
            runner.calls.lock().unwrap().as_slice(),
            &[("provision".into(), true)],
            "the placement node applied the armed mutation"
        );
    }

    #[tokio::test]
    async fn drain_honestly_gates_a_mutation_for_an_unreachable_target() {
        let tmp = tempfile::tempdir().unwrap();
        let bus = tmp.path().to_path_buf();
        let persist = Persist::open(bus.clone()).unwrap();
        let req = persist
            .write(
                "action/cloud/provision",
                Priority::Default,
                None,
                Some(r#"{"node":"ghost"}"#),
            )
            .unwrap();
        // Node "f" drains a mutation placed on offline "ghost" (not reachable) →
        // honest gated, never a silent swallow.
        let w = CloudWorker::new("f".into(), "peer:f".into(), tmp.path().to_path_buf())
            .with_runner(Arc::new(FakeRunner::default()))
            .with_bus_root(Some(bus.clone()))
            .with_reachable_nodes(Some(HashSet::new()));
        let mut cursors = HashMap::new();
        assert!(w.drain_actions(&mut cursors));
        let replies = persist.list_since(&reply_topic(&req.ulid), None).unwrap();
        assert_eq!(replies.len(), 1);
        let reply: CloudReply = serde_json::from_str(replies[0].body.as_deref().unwrap()).unwrap();
        assert!(!reply.ok);
        assert!(reply
            .gated
            .unwrap()
            .contains("placement node ghost not reachable"));
    }

    #[tokio::test]
    async fn reads_are_served_locally_on_every_node_regardless_of_placement() {
        let tmp = tempfile::tempdir().unwrap();
        let bus = tmp.path().to_path_buf();
        let persist = Persist::open(bus.clone()).unwrap();
        let req = persist
            .write(
                "action/cloud/list-instances",
                Priority::Default,
                None,
                Some("{}"),
            )
            .unwrap();
        // Any node serves the read from its own roster — no placement gate.
        let w = CloudWorker::new("f".into(), "peer:f".into(), tmp.path().to_path_buf())
            .with_runner(Arc::new(FakeRunner {
                roster: vec![instance("web", "ACTIVE")],
                ..Default::default()
            }))
            .with_bus_root(Some(bus.clone()))
            .with_reachable_nodes(Some(HashSet::new()));
        let mut cursors = HashMap::new();
        assert!(w.drain_actions(&mut cursors));
        let replies = persist.list_since(&reply_topic(&req.ulid), None).unwrap();
        assert_eq!(replies.len(), 1);
        let reply: CloudReply = serde_json::from_str(replies[0].body.as_deref().unwrap()).unwrap();
        assert!(reply.ok);
        assert_eq!(reply.instances.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn prime_cursors_skips_the_backlog_so_a_restart_does_not_replay() {
        let tmp = tempfile::tempdir().unwrap();
        let bus = tmp.path().to_path_buf();
        let persist = Persist::open(bus.clone()).unwrap();
        persist
            .write(
                "action/cloud/provision",
                Priority::Default,
                None,
                Some(r#"{"node":"l"}"#),
            )
            .unwrap();
        let runner = Arc::new(FakeRunner::default());
        let w = CloudWorker::new("l".into(), "peer:l".into(), tmp.path().to_path_buf())
            .with_runner(runner.clone())
            .with_signer(Arc::new(signer()))
            .with_bus_root(Some(bus.clone()));
        let mut cursors = HashMap::new();
        w.prime_cursors(&mut cursors);
        assert!(
            !w.drain_actions(&mut cursors),
            "the backlog is not replayed"
        );
        assert!(
            runner.calls.lock().unwrap().is_empty(),
            "no stale provision fired"
        );
    }

    #[tokio::test]
    async fn run_loop_exits_promptly_on_shutdown() {
        let mut w =
            staged_worker(Arc::new(FakeRunner::default())).with_poll(Duration::from_millis(10));
        let (tx, rx) = tokio::sync::watch::channel(false);
        let token = ShutdownToken::from_receiver(rx);
        let handle = tokio::spawn(async move { w.run(token).await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        tx.send(true).expect("signal shutdown");
        let joined = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(joined.is_ok(), "worker must exit promptly on shutdown");
        assert!(joined.unwrap().expect("join").is_ok());
    }
}
