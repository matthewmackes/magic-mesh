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
//!   env wall. A live mutation is authorized by a root-shell-minted HMAC **armed
//!   token** (nonce + expiry, bound to verb + placement + target). `CloudState.
//!   apply_armed` is reinterpreted as *token-arming available on this node* (a
//!   capability, not a wall). Destructive per-workload lifecycle operations also
//!   require a `typed_name` matching the exact target; legacy workspace-wide
//!   `destroy` is refused.
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
mod path_key;
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

#[cfg(test)]
pub(crate) use gate::nonce_digest;
pub(crate) use gate::{
    claim_nonce, placement_match, verify_token, HmacTokenSigner, NullSigner, Placement,
    TokenSigner, TokenVerdict, DEFAULT_AUTH_ROOT,
};
use runner::{
    default_iac_root, default_libvirt_uri, instances_table, CloudRunOutcome, CloudRunner,
    ShellCloudRunner, BACKEND_TOOLS,
};
use verbs::{CloudActionBody, CloudVerb};

/// Maximum remaining lifetime accepted for any cloud mutation capability.
/// Consumers enforce this independently of the root shell's minting policy so
/// a signed token can never become a long-lived bearer credential.
pub(crate) const MAX_AUTH_TTL_MS: i64 = 30_000;

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
    /// [`HmacTokenSigner`]; a node with no arming key uses [`NullSigner`], refusing
    /// every mutation).
    signer: Arc<dyn TokenSigner>,
    /// Whether token-arming is available on this node (a capability, published as
    /// `CloudState.apply_armed`). `false` means this node has no arming key and
    /// fails every mutation closed.
    arm_capable: bool,
    /// Host-local spent-nonce ledger. This must never live in the
    /// Syncthing-replicated workgroup tree in production.
    auth_root: PathBuf,
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
    /// deployed IaC tree + local libvirt, a placement-node-local arming authority,
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
        // Unit tests and offline callers pass isolated roots and inject a signer.
        // Production has a host-local replay ledger and obtains verification
        // authority only from a root-only systemd credential.
        let production = workgroup_root == mackes_mesh_types::peers::default_workgroup_root();
        let auth_root = if production {
            PathBuf::from(gate::DEFAULT_AUTH_ROOT)
        } else {
            workgroup_root.join("mcnf/cloud/test-auth")
        };
        let (signer, arm_capable): (Arc<dyn TokenSigner>, bool) = if production {
            match HmacTokenSigner::from_systemd_credential() {
                Ok(signer) => (Arc::new(signer), true),
                Err(error) => {
                    tracing::error!(
                        target: "mackesd::cloud",
                        %error,
                        "cloud live authorization unavailable; mutations fail closed"
                    );
                    (Arc::new(NullSigner), false)
                }
            }
        } else {
            (Arc::new(NullSigner), false)
        };
        Self {
            host,
            node_id,
            runner,
            signer,
            arm_capable,
            auth_root,
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

    /// Override the durable replay root (tests avoid touching `/var/lib`).
    #[must_use]
    pub fn with_auth_root(mut self, root: PathBuf) -> Self {
        self.auth_root = root;
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

    /// Atomically consume an authenticated token nonce in a durable ledger below
    /// the daemon-owned cloud state root. `create_new` is the cross-thread and
    /// cross-process compare-and-set: exactly one request can claim a nonce, and
    /// a daemon restart cannot make a spent capability valid again.
    pub(crate) fn claim_armed_nonce(
        &self,
        nonce: &str,
        expires_at_ms: i64,
        now_ms: i64,
    ) -> Result<bool, String> {
        gate::claim_nonce(&self.auth_root, nonce, expires_at_ms, now_ms)
    }

    /// Verify and atomically consume one capability. Every live handler uses this
    /// seam so image/container paths cannot accidentally skip durable replay.
    pub(crate) fn consume_armed_token(
        &self,
        raw: Option<&str>,
        verb: &str,
        node: &str,
        target: &str,
        request_body: &str,
    ) -> TokenVerdict {
        let now = now_ms();
        let mut verdict = gate::verify_token(
            raw,
            verb,
            node,
            target,
            request_body,
            now,
            self.signer.as_ref(),
        );
        if verdict.is_valid() {
            let Some(token) = raw.and_then(ArmedToken::parse) else {
                return TokenVerdict::Malformed;
            };
            if token.expires_at_ms > now.saturating_add(MAX_AUTH_TTL_MS) {
                return TokenVerdict::LifetimeTooLong;
            }
            let claim = self.claim_armed_nonce(&token.nonce, token.expires_at_ms, now);
            verdict = match claim {
                Ok(true) => TokenVerdict::Valid,
                Ok(false) => TokenVerdict::Replayed,
                Err(_) => TokenVerdict::ReplayStoreUnavailable,
            };
        }
        verdict
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
    /// - list/status reads are served locally; node-local inventory/output/plan
    ///   reads and every mutation require explicit placement;
    /// - a placement-scoped action is handled iff `body.node` is this host;
    /// - a scoped action without placement is refused (never fanned out);
    /// - an action for another node is skipped when that node is reachable (it
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
            let placement_scoped =
                CloudVerb::from_verb(&verb_name).is_some_and(CloudVerb::requires_placement);
            let cursor = cursors.get(&topic).cloned();
            let Ok(msgs) = persist.list_since(&topic, cursor.as_deref()) else {
                continue;
            };
            for msg in msgs {
                cursors.insert(topic.clone(), msg.ulid.clone());
                let body = msg.body.as_deref().unwrap_or("{}");
                // Placement routing: reads stay local; a mutation goes to its node.
                let route = if placement_scoped {
                    let parsed = CloudActionBody::parse(body);
                    match placement_match(&parsed.node, &self.host) {
                        Placement::Local => Route::Handle,
                        Placement::Missing => Route::GateMissing,
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
                    Route::GateMissing => {
                        let reply = CloudReply {
                            ok: false,
                            verb: verb_name.clone(),
                            gated: Some(
                                "cloud action requires an explicit placement node".to_string(),
                            ),
                            ..Default::default()
                        };
                        self.write_reply(&persist, &msg.ulid, &reply);
                        acted = true;
                    }
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
    /// A mutation omitted placement; refuse it without touching any backend.
    GateMissing,
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
    mde_bus::default_data_dir()
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

    fn valid_expiry() -> i64 {
        now_ms().saturating_add(MAX_AUTH_TTL_MS)
    }

    /// A valid armed token for `(verb, node)` signed with the shared test key.
    fn valid_token(verb: &str, node: &str, target: &str, request_body: &str) -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT_NONCE: AtomicU64 = AtomicU64::new(1);
        let nonce = format!(
            "nonce-{}-{}",
            std::process::id(),
            NEXT_NONCE.fetch_add(1, Ordering::Relaxed)
        );
        ArmedToken::mint(
            &signer(),
            &nonce,
            valid_expiry(),
            verb,
            node,
            target,
            &mackes_mesh_types::cloud::cloud_request_digest(request_body).unwrap(),
        )
        .encode()
    }

    /// A worker whose signer holds the test key (arm-capable) — armed mutations apply.
    fn armed_worker(runner: Arc<dyn CloudRunner>) -> CloudWorker {
        CloudWorker::new("me".into(), "peer:me".into(), PathBuf::from("/tmp"))
            .with_runner(runner)
            .with_signer(Arc::new(signer()))
            .with_bus_root(None)
    }

    /// A worker with no arming key (NullSigner) — every mutation fails closed.
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
    fn placement_local_list_returns_only_the_handling_workers_roster() {
        let runner = Arc::new(FakeRunner {
            roster: vec![instance("web", "ACTIVE")],
            ..Default::default()
        });
        let reply = staged_worker(runner).handle("list-instances-local", r#"{"node":"me"}"#);
        assert!(reply.ok);
        assert_eq!(reply.instances.unwrap()[0].name, "web");
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

    // ── the armed-token gate: fail closed (no/invalid token) vs armed ──
    #[test]
    fn unsigned_mutations_are_refused_before_any_backend_call() {
        let runner = Arc::new(FakeRunner::default());
        let w = staged_worker(runner.clone());
        for (verb, body) in [
            ("provision", r#"{"node":"me"}"#),
            ("configure", r#"{"node":"me"}"#),
            ("instance-start", r#"{"node":"me","instance":"web"}"#),
        ] {
            let reply = w.handle(verb, body);
            assert!(!reply.ok, "{verb} must not fabricate success");
            let gated = reply.gated.unwrap();
            assert!(gated.contains("no armed token"), "{verb}: {gated}");
            assert!(
                gated.contains("nothing changed or disclosed"),
                "{verb}: {gated}"
            );
        }
        assert!(
            runner.calls.lock().unwrap().is_empty(),
            "unsigned mutations must be refused before the backend seam"
        );
    }

    #[test]
    fn an_armed_mutation_applies_and_is_not_audited_when_non_destructive() {
        let runner = Arc::new(FakeRunner::default());
        let w = armed_worker(runner.clone());
        let base = r#"{"node":"me"}"#;
        let body = format!(
            r#"{{"node":"me","armed_token":"{}"}}"#,
            valid_token(
                "provision",
                "me",
                mackes_mesh_types::cloud::CLOUD_ARM_NODE_SCOPE,
                base,
            )
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
    fn an_armed_token_is_single_use_even_across_worker_restart() {
        let tmp = tempfile::tempdir().unwrap();
        let base = r#"{"node":"me"}"#;
        let token = valid_token(
            "provision",
            "me",
            mackes_mesh_types::cloud::CLOUD_ARM_NODE_SCOPE,
            base,
        );
        let body = format!(r#"{{"node":"me","armed_token":"{token}"}}"#);

        let first_runner = Arc::new(FakeRunner::default());
        let first = CloudWorker::new("me".into(), "peer:me".into(), tmp.path().to_path_buf())
            .with_runner(first_runner.clone())
            .with_signer(Arc::new(signer()))
            .with_bus_root(None);
        assert!(first.handle("provision", &body).ok);
        assert_eq!(
            first_runner.calls.lock().unwrap().as_slice(),
            &[("provision".into(), true)]
        );
        drop(first);

        let restarted_runner = Arc::new(FakeRunner::default());
        let restarted = CloudWorker::new("me".into(), "peer:me".into(), tmp.path().to_path_buf())
            .with_runner(restarted_runner.clone())
            .with_signer(Arc::new(signer()))
            .with_bus_root(None);
        let replay = restarted.handle("provision", &body);
        assert!(!replay.ok);
        assert!(replay
            .gated
            .as_deref()
            .is_some_and(|reason| reason.contains("already used")));
        assert!(
            restarted_runner.calls.lock().unwrap().is_empty(),
            "a replay must be refused before the backend seam"
        );
    }

    #[test]
    fn an_expired_or_forged_token_never_reaches_the_backend() {
        let runner = Arc::new(FakeRunner::default());
        let w = armed_worker(runner.clone());
        // A token minted by a different key is forged from this worker's vantage.
        let forged = ArmedToken::mint(
            &HmacTokenSigner::new(b"other".to_vec()),
            "nonce-12345678",
            valid_expiry(),
            "provision",
            "me",
            mackes_mesh_types::cloud::CLOUD_ARM_NODE_SCOPE,
            &mackes_mesh_types::cloud::cloud_request_digest(r#"{"node":"me"}"#).unwrap(),
        )
        .encode();
        let body = format!(r#"{{"node":"me","armed_token":"{forged}"}}"#);
        let reply = w.handle("provision", &body);
        assert!(!reply.ok);
        assert!(reply.gated.unwrap().contains("signature did not verify"));
        assert!(
            runner.calls.lock().unwrap().is_empty(),
            "a forged token must be refused before the backend seam"
        );
    }

    #[test]
    fn an_overlong_token_never_reaches_the_backend() {
        let runner = Arc::new(FakeRunner::default());
        let w = armed_worker(runner.clone());
        let base = r#"{"node":"me"}"#;
        let token = ArmedToken::mint(
            &signer(),
            "nonce-overlong-cloud-capability",
            now_ms().saturating_add(MAX_AUTH_TTL_MS + 30_000),
            "provision",
            "me",
            mackes_mesh_types::cloud::CLOUD_ARM_NODE_SCOPE,
            &mackes_mesh_types::cloud::cloud_request_digest(base).unwrap(),
        )
        .encode();
        let body = format!(r#"{{"node":"me","armed_token":"{token}"}}"#);

        let reply = w.handle("provision", &body);
        assert!(!reply.ok);
        assert!(reply
            .gated
            .as_deref()
            .is_some_and(|reason| reason.contains("exceeds the 30-second lifetime")));
        assert!(
            runner.calls.lock().unwrap().is_empty(),
            "an overlong token must be refused before the backend seam"
        );
    }

    // ── destructive lifecycle: target-scoped + typed confirmation ──
    #[test]
    fn workspace_wide_destroy_is_retired_and_never_reaches_the_runner() {
        let runner = Arc::new(FakeRunner::default());
        let w = armed_worker(runner.clone());
        let reply = w.handle("destroy", r#"{"node":"me"}"#);
        assert!(!reply.ok);
        assert!(reply
            .error
            .as_deref()
            .is_some_and(|e| e.contains("workspace-wide destroy is retired")));
        assert!(runner.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn armed_target_delete_retracts_only_that_workload_and_is_audited() {
        use mackes_mesh_types::cloud::{DeliveryType, WorkloadSpec};

        let tmp = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::default());
        for name in ["web", "peer"] {
            reconcile::write_desired_doc(
                tmp.path(),
                &WorkloadSpec {
                    name: name.to_string(),
                    delivery_type: DeliveryType::ServiceVm,
                    node: "me".to_string(),
                    vcpu: 2,
                    memory_mb: 2048,
                    disk_gb: 20,
                    image: None,
                    network_isolation: false,
                    raw_hcl: None,
                },
            )
            .unwrap();
        }
        let w = CloudWorker::new("me".into(), "peer:me".into(), tmp.path().to_path_buf())
            .with_runner(runner.clone())
            .with_signer(Arc::new(signer()))
            .with_db_path(tmp.path().join("events.sqlite"))
            .with_bus_root(None);
        let body = format!(
            r#"{{"node":"me","instance":"web","typed_name":"web","armed_token":"{}"}}"#,
            valid_token(
                "instance-delete",
                "me",
                "web",
                r#"{"node":"me","instance":"web","typed_name":"web"}"#,
            )
        );
        let reply = w.handle("instance-delete", &body);
        assert!(reply.ok, "gated: {:?} err: {:?}", reply.gated, reply.error);
        assert!(reply.audited, "a performed target delete is audited");
        assert_eq!(
            runner.calls.lock().unwrap().as_slice(),
            &[("lifecycle-delete".into(), true)]
        );
        let remaining = reconcile::read_desired_slice(tmp.path(), "me");
        assert_eq!(remaining.len(), 1, "the peer desired doc remains");
        assert_eq!(remaining[0].name, "peer");
    }

    #[test]
    fn an_armed_target_delete_without_typed_confirmation_is_blocked() {
        let runner = Arc::new(FakeRunner::default());
        let w = armed_worker(runner.clone());
        let body = format!(
            r#"{{"node":"me","instance":"web","armed_token":"{}"}}"#,
            valid_token(
                "instance-delete",
                "me",
                "web",
                r#"{"node":"me","instance":"web"}"#,
            )
        );
        let reply = w.handle("instance-delete", &body);
        assert!(!reply.ok);
        assert!(reply.error.unwrap().contains("typed_name"));
        assert!(
            runner.calls.lock().unwrap().is_empty(),
            "a blocked delete never runs the backend"
        );
    }

    #[test]
    fn an_overlong_delete_target_is_rejected_before_backend_io() {
        let runner = Arc::new(FakeRunner::default());
        let w = staged_worker(runner.clone());
        let target = "x".repeat(251);
        let body = serde_json::json!({
            "node": "me",
            "instance": target,
            "typed_name": target,
        });

        let reply = w.handle("instance-delete", &body.to_string());
        assert!(!reply.ok);
        assert!(reply.error.unwrap().contains("too long"));
        assert!(
            runner.calls.lock().unwrap().is_empty(),
            "invalid state filename must fail before the backend"
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
            valid_token(
                "instance-start",
                "me",
                "web",
                r#"{"node":"me","instance":"web"}"#,
            )
        );
        let good = w.handle("instance-start", &body);
        assert!(good.ok, "gated: {:?}", good.gated);
        assert_eq!(
            runner.calls.lock().unwrap().as_slice(),
            &[("lifecycle-start".into(), true)]
        );
    }

    #[test]
    fn armed_bulk_lifecycle_selects_targets_from_the_workers_live_roster() {
        let runner = Arc::new(FakeRunner {
            roster: vec![
                instance("web", "ACTIVE"),
                instance("db", "SHUTOFF"),
                instance("cache", "ACTIVE"),
                instance("web", "ACTIVE"),
                instance("broken", "ERROR"),
            ],
            ..Default::default()
        });
        let w = armed_worker(runner.clone());
        let base = r#"{"schema_version":1,"node":"me"}"#;
        let body = format!(
            r#"{{"schema_version":1,"node":"me","armed_token":"{}"}}"#,
            valid_token(
                "instance-stop-all",
                "me",
                mackes_mesh_types::cloud::CLOUD_ARM_NODE_SCOPE,
                base,
            )
        );
        let reply = w.handle("instance-stop-all", &body);
        assert!(
            reply.ok,
            "gated: {:?}, error: {:?}",
            reply.gated, reply.error
        );
        assert_eq!(
            reply.raw_log.as_deref(),
            Some("2 succeeded, 0 failed (of 2)")
        );
        assert_eq!(
            runner.calls.lock().unwrap().as_slice(),
            &[
                ("lifecycle-stop".into(), true),
                ("lifecycle-stop".into(), true),
            ],
            "only the two unique ACTIVE domains are selected inside the worker"
        );
    }

    #[test]
    fn unarmed_bulk_lifecycle_never_reads_targets_or_calls_the_runner() {
        let runner = Arc::new(FakeRunner {
            roster: vec![instance("web", "ACTIVE")],
            ..Default::default()
        });
        let reply = armed_worker(runner.clone())
            .handle("instance-reboot-all", r#"{"schema_version":1,"node":"me"}"#);
        assert!(!reply.ok);
        assert!(reply
            .gated
            .as_deref()
            .is_some_and(|reason| reason.contains("not authorized")));
        assert!(runner.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn an_unknown_verb_is_an_honest_error() {
        let w = staged_worker(Arc::new(FakeRunner::default()));
        let reply = w.handle("frobnicate", "{}");
        assert!(!reply.ok);
        assert!(reply.error.unwrap().contains("unknown cloud verb"));
    }

    #[test]
    fn unauthenticated_desired_android_and_console_actions_change_or_disclose_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let worker = CloudWorker::new("me".into(), "peer:me".into(), tmp.path().to_path_buf())
            .with_signer(Arc::new(signer()))
            .with_bus_root(None);

        let desired = worker.handle(
            "set-desired",
            r#"{"node":"me","spec":{"name":"poison","delivery_type":"service_vm","node":"me","vcpu":64,"memory_mb":65536,"disk_gb":999}}"#,
        );
        assert!(!desired.ok);
        assert!(desired
            .gated
            .as_deref()
            .is_some_and(|reason| reason.contains("not authorized")));
        assert!(reconcile::read_desired_slice(tmp.path(), "me").is_empty());

        let android = worker.handle(
            "android-provision",
            r#"{"node":"me","name":"poison-android"}"#,
        );
        assert!(!android.ok);
        assert!(android
            .gated
            .as_deref()
            .is_some_and(|reason| reason.contains("not authorized")));
        assert!(reconcile::read_desired_slice(tmp.path(), "me").is_empty());

        let console = worker.handle("console-attach", r#"{"node":"me","instance":"secret-vm"}"#);
        assert!(!console.ok);
        assert!(console.console.is_none());
        assert!(console
            .gated
            .as_deref()
            .is_some_and(|reason| reason.contains("not authorized")));
    }

    #[test]
    fn configure_token_cannot_authorize_a_substituted_playbook() {
        let runner = Arc::new(FakeRunner::default());
        let worker = armed_worker(runner.clone());
        let authorized = r#"{"node":"me","playbook":"site.yml","group":"cloud_vm"}"#;
        let token = valid_token(
            "configure",
            "me",
            mackes_mesh_types::cloud::CLOUD_ARM_NODE_SCOPE,
            authorized,
        );
        let altered = format!(
            r#"{{"node":"me","playbook":"attacker.yml","group":"cloud_vm","armed_token":"{token}"}}"#
        );
        let reply = worker.handle("configure", &altered);
        assert!(!reply.ok);
        assert!(reply
            .gated
            .as_deref()
            .is_some_and(|reason| reason.contains("request body")));
        assert!(
            runner.calls.lock().unwrap().is_empty(),
            "a body-substituted token must be refused before the backend seam"
        );
    }

    #[test]
    fn an_unknown_future_request_schema_is_rejected_before_any_backend_call() {
        let runner = Arc::new(FakeRunner::default());
        let w = staged_worker(runner.clone());
        let reply = w.handle("provision", r#"{"schema_version":99,"node":"me"}"#);
        assert!(!reply.ok);
        assert!(reply
            .error
            .as_deref()
            .is_some_and(|error| error.contains("unsupported cloud request schema version 99")));
        assert!(runner.calls.lock().unwrap().is_empty());
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
        // A node without an arming key advertises no capability (fails closed).
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
    async fn an_unprivileged_bus_writer_cannot_request_mint_authority() {
        let tmp = tempfile::tempdir().unwrap();
        let bus = tmp.path().to_path_buf();
        let persist = Persist::open(bus.clone()).unwrap();
        // Any local uid can write this public spool. `authorize` must remain an
        // unknown verb and no response schema may carry a minted token.
        let req = persist
            .write(
                "action/cloud/authorize",
                Priority::Default,
                None,
                Some(r#"{"node":"me","verb":"provision","confirmation":"apply"}"#),
            )
            .unwrap();
        let worker = CloudWorker::new("me".into(), "peer:me".into(), tmp.path().to_path_buf())
            .with_signer(Arc::new(signer()))
            .with_bus_root(Some(bus));

        assert!(worker.drain_actions(&mut HashMap::new()));
        let replies = persist.list_since(&reply_topic(&req.ulid), None).unwrap();
        assert_eq!(replies.len(), 1);
        let raw = replies[0].body.as_deref().unwrap();
        let reply: CloudReply = serde_json::from_str(raw).unwrap();
        assert!(!reply.ok);
        assert!(reply
            .error
            .as_deref()
            .is_some_and(|error| error.contains("unknown cloud verb")));
        assert!(
            !raw.contains("armed_token"),
            "public replies expose no mint field"
        );
    }

    #[tokio::test]
    async fn drain_performs_a_mutation_only_on_its_placement_node() {
        let tmp = tempfile::tempdir().unwrap();
        let bus = tmp.path().to_path_buf();
        let persist = Persist::open(bus.clone()).unwrap();
        // A mutation placed on node "l", armed for "l".
        let body = format!(
            r#"{{"node":"l","armed_token":"{}"}}"#,
            valid_token(
                "provision",
                "l",
                mackes_mesh_types::cloud::CLOUD_ARM_NODE_SCOPE,
                r#"{"node":"l"}"#,
            )
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
    async fn drain_refuses_a_mutation_without_explicit_placement() {
        let tmp = tempfile::tempdir().unwrap();
        let bus = tmp.path().to_path_buf();
        let persist = Persist::open(bus.clone()).unwrap();
        let req = persist
            .write(
                "action/cloud/provision",
                Priority::Default,
                None,
                Some("{}"),
            )
            .unwrap();
        let runner = Arc::new(FakeRunner::default());
        let worker = CloudWorker::new("me".into(), "peer:me".into(), tmp.path().to_path_buf())
            .with_runner(runner.clone())
            .with_bus_root(Some(bus));

        assert!(worker.drain_actions(&mut HashMap::new()));
        let replies = persist.list_since(&reply_topic(&req.ulid), None).unwrap();
        assert_eq!(replies.len(), 1);
        let reply: CloudReply = serde_json::from_str(
            replies[0]
                .body
                .as_deref()
                .expect("placement refusal carries a body"),
        )
        .unwrap();
        assert!(reply
            .gated
            .as_deref()
            .is_some_and(|reason| reason.contains("explicit placement")));
        assert!(runner.calls.lock().unwrap().is_empty());
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
