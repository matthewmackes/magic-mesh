//! WL-ARCH-001 Phase B + Workloads U2 — the mackesd `cloud` worker: the
//! **OpenTofu + Ansible cloud backend** over local libvirt/KVM.
//!
//! The worker is the mesh-side runner + status publisher for that stack. It:
//!
//! 1. **Drains `action/cloud/*` verbs off the Bus** ([`CLOUD_ACTION_PREFIX`]) and
//!    answers each with a neutral `CloudReply` on `reply/<ulid>`. It serves
//!    inventory/status reads, dry plans, armed Ansible configuration, and typed
//!    desired-state/image declarations. Retained `provision` requests explicitly
//!    fail closed; direct VM lifecycle and console verbs are unclassified.
//! 2. **Shells read-only/dry-run OpenTofu and armed Ansible** through the injectable
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
//!   capability, not a wall). Legacy workspace-wide `destroy` is refused.
//! - **Placement gate** ([`gate::placement_match`]) — replaces the leader gate.
//!   Every node drains `action/cloud/*`, but performs a MUTATION iff `body.node ==
//!   self.host`; a mutation for another node is that node's to perform, and a
//!   mutation for an *unreachable* node is honestly gated (never a silent swallow).
//!   Reads stay local on every node (each answers about its own roster).
//!
//! The split (`runner`/`gate`/`verbs`/`reconcile` + this run loop) is the worker
//! serialize point: U4–U10 each own a disjoint verb/reconcile handler after U2.

#![cfg(feature = "async-services")]

mod android_provider;
mod gate;
mod path_key;
mod reconcile;
mod render;
mod runner;
mod verbs;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use mackes_mesh_types::android_apps::{
    android_catalog_state_topic, AndroidAppAvailability, AndroidAppInventory, AndroidAppReadiness,
    AndroidGuestBootState, AndroidGuestInventoryRequest, AndroidGuestRequest, AndroidGuestResponse,
    AndroidImagePackageManifest, AndroidLaunchReadiness, AndroidLauncherResolvability,
    AndroidSignedCatalog, AndroidUnavailableReason, MAX_ANDROID_OBSERVATION_AGE_MS,
};
use mackes_mesh_types::android_provider::{
    AndroidProviderAdmission, AndroidVdiSource, CuttlefishGuestBootState as ProviderGuestBootState,
    CuttlefishGuestReadiness as ProviderGuestReadiness, CuttlefishGuestReadinessEvidence,
    CuttlefishVmId, CuttlefishVmLifecycleState, CuttlefishVmObservation, CuttlefishVmTarget,
};
use mackes_mesh_types::cloud::{
    cloud_state_topic, CloudProviderAdapter, CloudReply, CloudState, DeliveryType, DeploymentRole,
    DriftSummary, HealthState, NodeCapacity, ServiceHealth, WorkloadRow, CLOUD_ACTION_PREFIX,
    MAX_ANDROID_INVENTORIES_PER_STATE,
};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;

use super::{ShutdownToken, Worker};

use android_provider::{
    configured_image_path, preflight, AndroidHostProbe, AndroidPreflightInput,
    ProductionAndroidHostProbe,
};
#[cfg(test)]
pub(crate) use gate::nonce_digest;
pub(crate) use gate::{
    claim_nonce, placement_match, verify_token, HmacTokenSigner, NullSigner, Placement,
    TokenSigner, TokenVerdict, DEFAULT_AUTH_ROOT,
};
use runner::{
    default_browser_vm_image_source, default_iac_root, default_libvirt_uri, instances_table,
    CloudRunOutcome, CloudRunner, ShellCloudRunner, BACKEND_TOOLS,
};
#[cfg(test)]
use verbs::AndroidInventoryLedgerError;
pub(crate) use verbs::{
    AndroidGuestProvider, AndroidGuestProviderRegistry, AndroidGuestProviderRegistryError,
    LibvirtCuttlefishProviderClient,
};
use verbs::{AndroidInventoryLedger, AndroidInventoryLedgerAdmission, CloudActionBody, CloudVerb};

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

/// Bounded cadence for registered Android guest inventory providers.
pub const ANDROID_INVENTORY_POLL: Duration = Duration::from_secs(30);

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
    /// The pinned deployment role published in the cloud mirror.
    deployment_role: DeploymentRole,
    /// Whether this node may receive workload mutations. Lighthouses remain
    /// coordination-only even if a forged Bus request names them as placement.
    workloads_allowed: bool,
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
    /// Bounded, typed Android guest observations admitted by a future provider.
    /// The ledger is retained in a bounded host-scoped snapshot below the
    /// workgroup root, then folded into the Workloads mirror.
    android_inventory_ledger: std::sync::Mutex<AndroidInventoryLedger>,
    /// Providers keyed by the stable Android VM workload identity. An absent
    /// registration leaves the derived Workloads row pending.
    android_guest_providers: AndroidGuestProviderRegistry,
    /// Latest fail-closed image/host/provider admission folded into state/cloud.
    android_provider_admissions: std::sync::Mutex<Vec<AndroidProviderAdmission>>,
    /// Injectable host/image seam; production reads real local kernel/filesystem facts.
    android_host_probe: Arc<dyn AndroidHostProbe>,
    /// Serializes durable Android lifecycle compare-and-mutate operations.
    android_lifecycle_lock: std::sync::Mutex<()>,
    /// Current guest-owned display sources, keyed by lifecycle identity.
    android_vdi_sources: std::sync::Mutex<Vec<AndroidVdiSource>>,
    /// Durable host-scoped Android observation snapshot, if the host identity
    /// can be represented as one safe filename component.
    android_inventory_path: Option<PathBuf>,
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
    /// Poll cadence for registered Android guest providers.
    android_inventory_interval: Duration,
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
        let deployment_role = if production {
            match mde_role::load() {
                Ok(mde_role::Role::Lighthouse) => DeploymentRole::Lighthouse,
                Ok(mde_role::Role::Workstation) => DeploymentRole::Workstation,
                Err(error) => {
                    tracing::error!(
                        target: "mackesd::cloud",
                        %error,
                        "deployment role unavailable; cloud workloads fail closed"
                    );
                    DeploymentRole::Unknown
                }
            }
        } else {
            // Isolated test/offline roots are deliberately treated as a
            // workstation so existing backend seam tests exercise their verb
            // behavior without depending on the host's role file.
            DeploymentRole::Workstation
        };
        let workloads_allowed = matches!(deployment_role, DeploymentRole::Workstation);
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
        let android_inventory_path = android_inventory_ledger_path(&workgroup_root, &host);
        let android_inventory_ledger = android_inventory_path
            .as_deref()
            .map(AndroidInventoryLedger::load_from)
            .transpose()
            .unwrap_or_else(|error| {
                tracing::warn!(
                    target: "mackesd::cloud",
                    ?error,
                    "discarding invalid Android inventory ledger; Workloads remains pending"
                );
                None
            })
            .unwrap_or_else(AndroidInventoryLedger::new);

        Self {
            host,
            deployment_role,
            workloads_allowed,
            node_id,
            runner,
            signer,
            arm_capable,
            auth_root,
            state_root: workgroup_root,
            db_path: crate::default_db_path(),
            android_inventory_ledger: std::sync::Mutex::new(android_inventory_ledger),
            android_guest_providers: AndroidGuestProviderRegistry::new(),
            android_provider_admissions: std::sync::Mutex::new(Vec::new()),
            android_host_probe: Arc::new(ProductionAndroidHostProbe::default()),
            android_lifecycle_lock: std::sync::Mutex::new(()),
            android_vdi_sources: std::sync::Mutex::new(Vec::new()),
            android_inventory_path,
            bus_root: default_bus_root(),
            poll: POLL,
            heartbeat: PUBLISH_HEARTBEAT,
            reachable_override: None,
            drift_interval: DRIFT_POLL,
            android_inventory_interval: ANDROID_INVENTORY_POLL,
            drift: std::sync::Mutex::new((Vec::new(), DriftSummary::default())),
        }
    }

    /// Inject a backend runner (tests supply a fake).
    #[must_use]
    pub fn with_runner(mut self, runner: Arc<dyn CloudRunner>) -> Self {
        self.runner = runner;
        self
    }

    #[cfg(test)]
    fn with_android_host_probe(mut self, probe: Arc<dyn AndroidHostProbe>) -> Self {
        self.android_host_probe = probe;
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

    /// Register one typed Android guest provider for a stable workload identity.
    /// A missing registration is intentionally left as the pending Workloads
    /// projection; this builder is the explicit seam for a real Cuttlefish
    /// adapter, not a discovery or shell-execution shortcut.
    pub(crate) fn with_android_guest_provider(
        mut self,
        workload_id: impl Into<String>,
        provider: Arc<dyn AndroidGuestProvider>,
    ) -> Result<Self, AndroidGuestProviderRegistryError> {
        self.android_guest_providers
            .register(workload_id, provider)?;
        Ok(self)
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

    /// Override or disable the durable Android observation journal in focused
    /// worker tests. Disabling it also clears constructor-loaded observations so
    /// a shared test root cannot leak evidence between cases. Production keeps
    /// the host-scoped path selected by `new`.
    pub(crate) fn with_android_inventory_path(mut self, path: Option<PathBuf>) -> Self {
        self.android_inventory_path = path;
        if self.android_inventory_path.is_none() {
            if let Ok(mut ledger) = self.android_inventory_ledger.lock() {
                *ledger = AndroidInventoryLedger::new();
            }
        }
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

    /// Override the Android provider cadence in focused worker tests.
    #[must_use]
    pub const fn with_android_inventory_interval(mut self, interval: Duration) -> Self {
        self.android_inventory_interval = interval;
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
    ///
    /// The Bus drain routes placement-scoped requests before calling this method,
    /// but this is also a public crate seam used by adapters and tests. Re-checking
    /// placement here prevents a direct caller from presenting a valid capability
    /// for another node and making this worker run that node's lifecycle action.
    #[must_use]
    pub fn handle(&self, verb_name: &str, body: &str) -> CloudReply {
        if let Some(reply) = self.reject_lighthouse_workload(verb_name) {
            return reply;
        }
        if let Some(reply) = self.reject_nonlocal_placement(verb_name, body) {
            return reply;
        }
        verbs::dispatch(self, verb_name, body)
    }

    /// Lighthouses are the Nebula/etcd control plane, never an Android, VM, or
    /// container placement target. Keep this guard in the worker as well as the
    /// GUI filter so a forged or stale Bus request cannot bypass the role policy.
    fn reject_lighthouse_workload(&self, verb_name: &str) -> Option<CloudReply> {
        let verb = CloudVerb::from_verb(verb_name)?;
        if self.workloads_allowed || !verb.is_mutation() {
            return None;
        }
        Some(CloudReply {
            ok: false,
            verb: verb_name.to_string(),
            gated: Some(format!(
                "{} nodes are control-plane-only; workload actions must target a Workstation",
                match self.deployment_role {
                    DeploymentRole::Lighthouse => "lighthouse",
                    DeploymentRole::Unknown => "unidentified",
                    DeploymentRole::Workstation => "workstation",
                }
            )),
            ..Default::default()
        })
    }

    /// Defense-in-depth placement boundary for direct callers of [`Self::handle`].
    /// Schema failures remain owned by [`verbs::dispatch`] so malformed and future
    /// envelopes retain their precise error, while a valid remote placement is
    /// refused before any authorization replay claim or backend call.
    fn reject_nonlocal_placement(&self, verb_name: &str, body: &str) -> Option<CloudReply> {
        let verb = CloudVerb::from_verb(verb_name)?;
        if !verb.requires_placement() {
            return None;
        }
        let parsed = CloudActionBody::parse(body);
        if parsed.schema_error_for(verb).is_some() {
            return None;
        }
        if let Err(error) = path_key::segment("placement node", &parsed.node) {
            return Some(CloudReply {
                ok: false,
                verb: verb_name.to_string(),
                error: Some(error),
                ..Default::default()
            });
        }
        match placement_match(&parsed.node, &self.host) {
            Placement::Local => None,
            Placement::Missing => Some(CloudReply {
                ok: false,
                verb: verb_name.to_string(),
                gated: Some("cloud action requires an explicit placement node".to_string()),
                ..Default::default()
            }),
            Placement::Remote(node) => Some(CloudReply {
                ok: false,
                verb: verb_name.to_string(),
                gated: Some(format!(
                    "cloud action is placed on node `{node}`; worker `{}` will not execute it",
                    self.host
                )),
                ..Default::default()
            }),
        }
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
            let classified = CloudVerb::from_verb(&verb_name);
            let placement_scoped = classified.is_some_and(CloudVerb::requires_placement);
            let cursor = cursors.get(&topic).cloned();
            let Ok(msgs) = persist.list_since(&topic, cursor.as_deref()) else {
                continue;
            };
            for msg in msgs {
                cursors.insert(topic.clone(), msg.ulid.clone());
                let Some(body) = msg.body.as_deref() else {
                    let reply = CloudReply {
                        ok: false,
                        verb: verb_name.clone(),
                        error: Some("cloud action body is missing".to_string()),
                        ..Default::default()
                    };
                    self.write_reply(&persist, &msg.ulid, &reply);
                    acted = true;
                    continue;
                };
                let parsed = CloudActionBody::parse(body);
                if let Some(verb) = classified {
                    if let Some(error) = parsed.schema_error_for(verb) {
                        let reply = CloudReply {
                            ok: false,
                            verb: verb_name.clone(),
                            error: Some(error),
                            ..Default::default()
                        };
                        self.write_reply(&persist, &msg.ulid, &reply);
                        acted = true;
                        continue;
                    }
                }
                // Placement routing: reads stay local; a mutation goes to its node.
                let route = if placement_scoped {
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
            &default_browser_vm_image_source(),
            now_ms(),
        );
        if let Ok(mut guard) = self.drift.lock() {
            *guard = snapshot;
        }
    }

    /// Register production Cuttlefish adapters for Android desired rows whose
    /// verified package manifest is present. A missing manifest leaves the
    /// workload on the existing pending projection; it never gets a guessed
    /// image provenance or a fake provider.
    fn ensure_configured_cuttlefish_providers(&mut self) {
        let catalog = self.load_admitted_android_catalog();
        let artifact = configured_image_path();
        let provider_healthy =
            self.runner.probe_tool(runner::TOOL_LIBVIRT).state == HealthState::Up;
        let observed_at = u64::try_from(now_ms()).unwrap_or(1).max(1);
        let mut admissions = Vec::new();
        for spec in reconcile::read_desired_slice(&self.state_root, &self.host)
            .into_iter()
            .filter(|spec| spec.delivery_type == DeliveryType::AndroidVm)
            .take(MAX_ANDROID_INVENTORIES_PER_STATE)
        {
            let Ok(vm_id) = CuttlefishVmId::new(spec.name.clone()) else {
                tracing::warn!(target: "mackesd::cloud", workload = %spec.name, "invalid Android workload identity omitted from provider preflight");
                continue;
            };
            self.android_guest_providers.unregister(&spec.name);
            let manifest = load_android_package_manifest(&self.state_root, &spec.name);
            let admission = preflight(
                AndroidPreflightInput {
                    workload: &spec,
                    catalog: catalog.as_ref(),
                    package_manifest: manifest.as_ref(),
                    artifact: artifact.as_deref(),
                    provider_healthy,
                    now_unix_ms: observed_at,
                },
                self.android_host_probe.as_ref(),
            );
            let ready = admission.is_ready();
            admissions.push(admission.clone());
            if !ready {
                tracing::warn!(target: "mackesd::cloud", workload = %spec.name, refusal = ?admission.refusal, "Android provider placement refused");
                continue;
            }
            let Some(manifest) = manifest else {
                continue;
            };
            let Some(image_provenance) = admission.image_provenance.clone() else {
                continue;
            };
            let Ok(target) = CuttlefishVmTarget::new(vm_id, image_provenance) else {
                continue;
            };
            let Ok(evidence) = CuttlefishGuestReadinessEvidence::new(
                ProviderGuestBootState::Pending,
                ProviderGuestReadiness::Unknown,
                None,
            ) else {
                continue;
            };
            let Ok(observation) = CuttlefishVmObservation::new(
                target.clone(),
                CuttlefishVmLifecycleState::Absent,
                evidence,
                0,
                now_ms().try_into().unwrap_or(1),
            ) else {
                continue;
            };
            let Some(catalog_digest) = catalog
                .as_ref()
                .and_then(|catalog| catalog.payload.content_digest().ok())
            else {
                continue;
            };
            let Ok(client) = LibvirtCuttlefishProviderClient::with_guest_contract(
                self.runner.clone(),
                manifest.clone(),
                catalog_digest,
            ) else {
                continue;
            };
            if let Err(error) = self.android_guest_providers.register_cuttlefish_provider(
                spec.name,
                target,
                manifest,
                observation,
                client,
            ) {
                tracing::warn!(
                    target: "mackesd::cloud",
                    ?error,
                    "configured Cuttlefish provider was not admitted"
                );
            }
        }
        admissions.sort_by(|left, right| left.workload_id.cmp(&right.workload_id));
        if let Ok(mut retained) = self.android_provider_admissions.lock() {
            *retained = admissions;
        }
    }

    fn load_admitted_android_catalog(&self) -> Option<AndroidSignedCatalog> {
        const MAX_CATALOG_BYTES: usize = 2 * 1024 * 1024;
        let root = self.bus_root.as_ref()?;
        let persist = Persist::open(root.clone()).ok()?;
        let topic = android_catalog_state_topic(&self.host).ok()?;
        let body = persist.read_latest(&topic).ok()??.body?;
        if body.is_empty() || body.len() > MAX_CATALOG_BYTES {
            return None;
        }
        serde_json::from_str(&body).ok()
    }

    /// Poll registered Android guest providers and admit only their typed,
    /// request-correlated inventory response. Provider failures retain the
    /// last valid observation; an absent provider is not converted into false
    /// readiness and remains represented by the pending Workloads projection.
    fn refresh_android_inventories(&mut self) -> bool {
        self.ensure_configured_cuttlefish_providers();
        let workload_ids = self
            .drift
            .lock()
            .map(|guard| {
                let mut ids = guard
                    .0
                    .iter()
                    .filter(|workload| workload.delivery_type == DeliveryType::AndroidVm)
                    .map(|workload| workload.name.clone())
                    .collect::<Vec<_>>();
                ids.sort_unstable();
                ids.dedup();
                ids.into_iter()
                    .take(MAX_ANDROID_INVENTORIES_PER_STATE)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let mut changed = false;

        for workload_id in workload_ids {
            if self.android_guest_providers.provider(&workload_id).is_err() {
                continue;
            }
            let Ok(request) = AndroidGuestInventoryRequest::new("cloud-inventory", &workload_id)
            else {
                tracing::warn!(
                    target: "mackesd::cloud",
                    workload = %workload_id,
                    "skipping Android provider poll for an invalid workload identity"
                );
                continue;
            };
            let response = match self
                .android_guest_providers
                .dispatch(AndroidGuestRequest::Inventory(request.clone()))
            {
                Ok(AndroidGuestResponse::Inventory(response)) => response,
                Ok(AndroidGuestResponse::Launch(_)) => {
                    tracing::warn!(
                        target: "mackesd::cloud",
                        workload = %workload_id,
                        "Android inventory provider returned a launch response"
                    );
                    continue;
                }
                Err(error) => {
                    tracing::warn!(
                        target: "mackesd::cloud",
                        workload = %workload_id,
                        ?error,
                        "Android inventory provider response was rejected"
                    );
                    continue;
                }
            };
            match self.admit_android_inventory_response(&request, response) {
                Ok(AndroidInventoryLedgerAdmission::Inserted)
                | Ok(AndroidInventoryLedgerAdmission::Replaced) => changed = true,
                Ok(AndroidInventoryLedgerAdmission::Unchanged) => {}
                Err(error) => {
                    tracing::warn!(
                        target: "mackesd::cloud",
                        workload = %workload_id,
                        ?error,
                        "Android inventory observation was not retained"
                    );
                }
            }
        }
        changed
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
        let retained_android_inventories = self
            .android_inventory_ledger
            .lock()
            .map(|ledger| ledger.snapshot())
            .unwrap_or_default();
        let android_inventories =
            merged_android_inventories(&workloads, &retained_android_inventories, now_ms());
        let android_provider_admissions = self
            .android_provider_admissions
            .lock()
            .map(|rows| rows.clone())
            .unwrap_or_default();
        let android_vdi_sources = self
            .android_vdi_sources
            .lock()
            .map(|rows| rows.clone())
            .unwrap_or_default();
        CloudState {
            host: self.host.clone(),
            role: self.deployment_role,
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
            android_inventories,
            android_provider_admissions,
            android_vdi_sources,
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

/// Until a real Android guest provider is wired, publish an explicit pending
/// inventory for each admitted Android VM workload and nothing for other rows.
/// Workload names are the existing stable Workloads identity; invalid names are
/// rejected by the Android contract rather than copied into the wire mirror.
fn pending_android_inventories(workloads: &[WorkloadRow]) -> Vec<AndroidAppInventory> {
    let mut workload_ids = workloads
        .iter()
        .filter(|workload| workload.delivery_type == DeliveryType::AndroidVm)
        .map(|workload| workload.name.clone())
        .collect::<Vec<_>>();
    workload_ids.sort_unstable();
    workload_ids.dedup();

    workload_ids
        .into_iter()
        .take(MAX_ANDROID_INVENTORIES_PER_STATE)
        .filter_map(|workload_id| {
            let inventory = AndroidAppInventory::pending(workload_id);
            inventory.validate().ok().map(|()| inventory)
        })
        .collect()
}

/// Merge admitted guest evidence over the deterministic pending workload list.
/// Only currently published Android workloads are emitted; a response admitted
/// before its workload row appears remains retained and is picked up later. The
/// pending list already applies the CloudState 32-record bound, so replacement
/// cannot grow or reorder the published mirror.
fn merged_android_inventories(
    workloads: &[WorkloadRow],
    retained: &[AndroidAppInventory],
    now_unix_ms: i64,
) -> Vec<AndroidAppInventory> {
    let retained_by_workload = retained
        .iter()
        .map(|inventory| (inventory.workload_id.as_str(), inventory))
        .collect::<std::collections::BTreeMap<_, _>>();

    pending_android_inventories(workloads)
        .into_iter()
        .map(|pending| {
            retained_by_workload
                .get(pending.workload_id.as_str())
                .map_or(pending.clone(), |inventory| {
                    project_android_inventory_age((*inventory).clone(), now_unix_ms)
                })
        })
        .collect()
}

/// Project retained guest evidence at the consumer's clock.
///
/// Producers report the age at which an inventory was observed. The cloud
/// mirror must advance that age while retaining the immutable observation
/// timestamp, and must stop presenting ready packages as launchable once the
/// bounded retention window is crossed. The stale projection is deliberately
/// typed: it never rewrites a missing package/image reason, invents a fresh
/// observation, or contacts the guest/provider.
fn project_android_inventory_age(
    mut inventory: AndroidAppInventory,
    now_unix_ms: i64,
) -> AndroidAppInventory {
    let (Some(observed_at_unix_ms), Some(reported_age_ms)) =
        (inventory.observed_at_unix_ms, inventory.observation_age_ms)
    else {
        return inventory;
    };
    let now_unix_ms = u64::try_from(now_unix_ms).unwrap_or(0);
    let elapsed_ms = now_unix_ms.saturating_sub(observed_at_unix_ms);
    let age_ms = reported_age_ms.saturating_add(elapsed_ms);
    if age_ms <= MAX_ANDROID_OBSERVATION_AGE_MS {
        inventory.observation_age_ms = Some(age_ms);
        return inventory;
    }

    // The wire contract bounds the age field, so cap the displayed age at the
    // admitted boundary and carry the fact that it crossed the boundary in a
    // closed unavailable reason.
    inventory.observation_age_ms = Some(MAX_ANDROID_OBSERVATION_AGE_MS);
    inventory.guest_boot_state = AndroidGuestBootState::Unavailable;
    inventory.unavailable_reason = Some(AndroidUnavailableReason::ObservationStale);
    for entry in &mut inventory.entries {
        if entry.availability == AndroidAppAvailability::Installed {
            entry.readiness = AndroidAppReadiness::Unavailable;
            entry.launcher_resolvability = AndroidLauncherResolvability::Unavailable;
            entry.launch_readiness = AndroidLaunchReadiness::Unavailable;
            entry.unavailable_reason = Some(AndroidUnavailableReason::ObservationStale);
        }
    }
    debug_assert!(inventory.validate().is_ok());
    inventory
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
        let mut last_android_inventory = Instant::now();
        loop {
            let acted = self.drain_actions(&mut cursors);
            let drift_due = last_drift.elapsed() >= self.drift_interval;
            if drift_due {
                self.refresh_drift();
                last_drift = Instant::now();
            }
            let inventory_due = !self.android_guest_providers.is_empty()
                && (drift_due
                    || last_android_inventory.elapsed() >= self.android_inventory_interval);
            let inventory_changed = if inventory_due {
                last_android_inventory = Instant::now();
                self.refresh_android_inventories()
            } else {
                false
            };
            if acted || drift_due || inventory_changed || last_pub.elapsed() >= self.heartbeat {
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

/// Host-local Android package manifests are staged by the image/placement
/// pipeline under the daemon-owned cloud state root. They are never accepted
/// from a Bus request, and their bounded JSON contract still validates image
/// provenance plus the exact nine-package starter set on decode.
const ANDROID_PACKAGE_MANIFEST_MAX_BYTES: usize = 64 * 1024;

fn android_package_manifest_path(state_root: &Path, workload_id: &str) -> Option<PathBuf> {
    let key = path_key::file_stem("workload", workload_id, ".json").ok()?;
    Some(
        state_root
            .join("mcnf/cloud/android-manifests")
            .join(format!("{key}.json")),
    )
}

fn load_android_package_manifest(
    state_root: &Path,
    workload_id: &str,
) -> Option<AndroidImagePackageManifest> {
    let path = android_package_manifest_path(state_root, workload_id)?;
    let metadata = fs::symlink_metadata(&path).ok()?;
    if !metadata.file_type().is_file() || metadata.len() > ANDROID_PACKAGE_MANIFEST_MAX_BYTES as u64
    {
        return None;
    }
    let bytes = fs::read(&path).ok()?;
    if bytes.is_empty() || bytes.len() > ANDROID_PACKAGE_MANIFEST_MAX_BYTES {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

/// Resolve the bounded host-scoped Android observation journal path. Invalid
/// host identities disable persistence for that worker rather than allowing a
/// Bus-controlled value to become a path traversal component.
fn android_inventory_ledger_path(state_root: &Path, host: &str) -> Option<PathBuf> {
    let host = path_key::file_stem("host", host, ".json").ok()?;
    Some(
        state_root
            .join("mcnf/cloud/android-inventory")
            .join(format!("{host}.json")),
    )
}

#[cfg(test)]
mod tests {
    use super::gate::{ArmedToken, HmacTokenSigner};
    use super::runner::fake::{instance, FakeRunner};
    use super::runner::{TOOL_LIBVIRT, TOOL_TOFU};
    use super::*;
    use mackes_mesh_types::android_apps::{
        AndroidAppAvailability, AndroidAppInventoryEntry, AndroidAppReadiness,
        AndroidGuestBootState, AndroidGuestInventoryRequest, AndroidGuestInventoryResponse,
        AndroidGuestLaunchOutcome, AndroidGuestLaunchRequest, AndroidImagePackage,
        AndroidImagePackageManifest, AndroidImageProvenance, AndroidLaunchReadiness,
        AndroidLauncherResolvability, AndroidPackageVersion, AndroidUnavailableReason,
        AospStarterApp,
    };
    use mackes_mesh_types::cloud::{CloudProviderAdapter, HealthState};
    use tempfile::tempdir;

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
            .with_android_inventory_path(None)
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
        let reply = w.handle("configure", r#"{"schema_version":1,"node":"me"}"#);
        assert!(!reply.ok, "configure must not fabricate success");
        let gated = reply.gated.unwrap();
        assert!(gated.contains("no armed token"), "{gated}");
        assert!(gated.contains("nothing changed or disclosed"), "{gated}");

        let retired = w.handle("provision", r#"{"schema_version":1,"node":"me"}"#);
        assert!(!retired.ok);
        assert!(retired
            .error
            .as_deref()
            .is_some_and(|error| error.contains("cloud provision is retired")));
        assert!(
            runner.calls.lock().unwrap().is_empty(),
            "unsigned mutations must be refused before the backend seam"
        );
    }

    #[test]
    fn lighthouse_rejects_workload_mutations_before_auth_or_backend() {
        let runner = Arc::new(FakeRunner::default());
        let mut w = staged_worker(runner.clone());
        w.deployment_role = DeploymentRole::Lighthouse;
        w.workloads_allowed = false;
        let reply = w.handle(
            "android-provision",
            r#"{"schema_version":1,"node":"me","name":"android-me"}"#,
        );
        assert!(!reply.ok);
        assert!(reply
            .gated
            .as_deref()
            .is_some_and(|message| message.contains("control-plane-only")));
        assert!(runner.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn retired_provision_refuses_even_an_armed_request_without_backend_contact() {
        let runner = Arc::new(FakeRunner::default());
        let w = armed_worker(runner.clone());
        let base = r#"{"schema_version":1,"node":"me"}"#;
        let body = format!(
            r#"{{"schema_version":1,"node":"me","armed_token":"{}"}}"#,
            valid_token(
                "provision",
                "me",
                mackes_mesh_types::cloud::CLOUD_ARM_NODE_SCOPE,
                base,
            )
        );
        let reply = w.handle("provision", &body);
        assert!(!reply.ok);
        assert!(reply.error.as_deref().is_some_and(|error| {
            error.contains("cloud provision is retired")
                && error.contains("action/workload/operation")
        }));
        assert!(runner.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn android_provision_retains_desired_state_without_live_apply() {
        let tmp = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::default());
        let w = CloudWorker::new("me".into(), "peer:me".into(), tmp.path().to_path_buf())
            .with_runner(runner.clone())
            .with_signer(Arc::new(signer()))
            .with_bus_root(None);

        let android_base = r#"{"schema_version":1,"node":"me","name":"phone"}"#;
        let android_token = valid_token("android-provision", "me", "phone", android_base);
        let android_body = format!(
            r#"{{"schema_version":1,"node":"me","name":"phone","armed_token":"{android_token}"}}"#
        );
        let prepared = w.handle("android-provision", &android_body);
        assert!(
            prepared.ok,
            "gated: {:?} error: {:?}",
            prepared.gated, prepared.error
        );
        assert_eq!(
            reconcile::read_desired_slice(tmp.path(), "me")[0].delivery_type,
            mackes_mesh_types::cloud::DeliveryType::AndroidVm
        );

        assert!(runner.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn retired_provision_does_not_consume_tokens_across_worker_restart() {
        let tmp = tempfile::tempdir().unwrap();
        let base = r#"{"schema_version":1,"node":"me"}"#;
        let token = valid_token(
            "configure",
            "me",
            mackes_mesh_types::cloud::CLOUD_ARM_NODE_SCOPE,
            base,
        );
        let body = format!(r#"{{"schema_version":1,"node":"me","armed_token":"{token}"}}"#);

        let first_runner = Arc::new(FakeRunner::default());
        let first = CloudWorker::new("me".into(), "peer:me".into(), tmp.path().to_path_buf())
            .with_runner(first_runner.clone())
            .with_signer(Arc::new(signer()))
            .with_bus_root(None);
        assert!(!first.handle("provision", &body).ok);
        assert!(first_runner.calls.lock().unwrap().is_empty());
        drop(first);

        let restarted_runner = Arc::new(FakeRunner::default());
        let restarted = CloudWorker::new("me".into(), "peer:me".into(), tmp.path().to_path_buf())
            .with_runner(restarted_runner.clone())
            .with_signer(Arc::new(signer()))
            .with_bus_root(None);
        let replay = restarted.handle("provision", &body);
        assert!(!replay.ok);
        assert!(replay
            .error
            .as_deref()
            .is_some_and(|reason| reason.contains("cloud provision is retired")));
        assert!(
            restarted_runner.calls.lock().unwrap().is_empty(),
            "a replay must be refused before the backend seam"
        );
    }

    #[test]
    fn a_forged_token_never_reaches_the_backend() {
        let runner = Arc::new(FakeRunner::default());
        let w = armed_worker(runner.clone());
        // A token minted by a different key is forged from this worker's vantage.
        let forged = ArmedToken::mint(
            &HmacTokenSigner::new(b"other".to_vec()),
            "nonce-12345678",
            valid_expiry(),
            "configure",
            "me",
            mackes_mesh_types::cloud::CLOUD_ARM_NODE_SCOPE,
            &mackes_mesh_types::cloud::cloud_request_digest(r#"{"schema_version":1,"node":"me"}"#)
                .unwrap(),
        )
        .encode();
        let body = format!(r#"{{"schema_version":1,"node":"me","armed_token":"{forged}"}}"#);
        let reply = w.handle("configure", &body);
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
        let base = r#"{"schema_version":1,"node":"me"}"#;
        let token = ArmedToken::mint(
            &signer(),
            "nonce-overlong-cloud-capability",
            now_ms().saturating_add(MAX_AUTH_TTL_MS + 30_000),
            "configure",
            "me",
            mackes_mesh_types::cloud::CLOUD_ARM_NODE_SCOPE,
            &mackes_mesh_types::cloud::cloud_request_digest(base).unwrap(),
        )
        .encode();
        let body = format!(r#"{{"schema_version":1,"node":"me","armed_token":"{token}"}}"#);

        let reply = w.handle("configure", &body);
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
        let reply = w.handle("destroy", r#"{"schema_version":1,"node":"me"}"#);
        assert!(!reply.ok);
        assert!(reply
            .error
            .as_deref()
            .is_some_and(|e| e.contains("workspace-wide destroy is retired")));
        assert!(runner.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn retired_lifecycle_and_console_verbs_are_refused_before_auth_or_backend() {
        let runner = Arc::new(FakeRunner::default());
        let w = staged_worker(runner.clone());
        for verb in [
            "instance-start",
            "instance-stop",
            "instance-reboot",
            "instance-delete",
            "instance-start-all",
            "instance-stop-all",
            "instance-reboot-all",
            "container-restart",
            "container-logs",
            "container-destroy",
            "console-attach",
        ] {
            let reply = w.handle(verb, r#"{"schema_version":1,"node":"me","instance":"web"}"#);
            assert!(!reply.ok);
            assert!(reply
                .error
                .as_deref()
                .is_some_and(|error| { error.contains("unknown cloud verb") }));
        }
        assert!(runner.calls.lock().unwrap().is_empty());
        assert!(runner.tool_calls.lock().unwrap().is_empty());
    }

    #[test]
    fn an_unknown_verb_is_an_honest_error() {
        let w = staged_worker(Arc::new(FakeRunner::default()));
        let reply = w.handle("frobnicate", "{}");
        assert!(!reply.ok);
        assert!(reply.error.unwrap().contains("unknown cloud verb"));
    }

    #[test]
    fn unauthenticated_desired_and_android_actions_change_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let worker = CloudWorker::new("me".into(), "peer:me".into(), tmp.path().to_path_buf())
            .with_signer(Arc::new(signer()))
            .with_bus_root(None);

        let desired = worker.handle(
            "set-desired",
            r#"{"schema_version":1,"node":"me","spec":{"name":"poison","delivery_type":"service_vm","node":"me","vcpu":64,"memory_mb":65536,"disk_gb":999}}"#,
        );
        assert!(!desired.ok);
        assert!(desired
            .gated
            .as_deref()
            .is_some_and(|reason| reason.contains("not authorized")));
        assert!(reconcile::read_desired_slice(tmp.path(), "me").is_empty());
        let android = worker.handle(
            "android-provision",
            r#"{"schema_version":1,"node":"me","name":"poison-android"}"#,
        );
        assert!(!android.ok);
        assert!(android
            .gated
            .as_deref()
            .is_some_and(|reason| reason.contains("not authorized")));
        assert!(reconcile::read_desired_slice(tmp.path(), "me").is_empty());
    }

    #[test]
    fn configure_token_cannot_authorize_a_substituted_playbook() {
        let runner = Arc::new(FakeRunner::default());
        let worker = armed_worker(runner.clone());
        let authorized =
            r#"{"schema_version":1,"node":"me","playbook":"site.yml","group":"cloud_vm"}"#;
        let token = valid_token(
            "configure",
            "me",
            mackes_mesh_types::cloud::CLOUD_ARM_NODE_SCOPE,
            authorized,
        );
        let altered = format!(
            r#"{{"schema_version":1,"node":"me","playbook":"attacker.yml","group":"cloud_vm","armed_token":"{token}"}}"#
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
    fn a_missing_mutation_schema_is_rejected_before_any_backend_call() {
        let runner = Arc::new(FakeRunner::default());
        let w = staged_worker(runner.clone());
        let reply = w.handle("configure", r#"{"node":"me","playbook":"site.yml"}"#);
        assert!(!reply.ok);
        assert!(reply
            .error
            .as_deref()
            .is_some_and(|error| error.contains("requires schema_version 1")));
        assert!(runner.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn every_workloads_verb_is_wired_no_skeleton_remains() {
        // All eight Workloads verbs are wired: set-desired/plan (U4), image-build (U6),
        // container-deploy (U7), inventory/output (U10), and android-provision
        // (U9). None may still surface the U2 "not yet wired"
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
            "android-provision",
        ] {
            let reply = w.handle(verb, r#"{"schema_version":1,"node":"me"}"#);
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

    fn mirror_workload(name: &str, delivery_type: DeliveryType) -> WorkloadRow {
        WorkloadRow {
            name: name.to_owned(),
            delivery_type,
            node: "me".to_owned(),
            status: "absent".to_owned(),
            cpu_pct: 0,
            mem_mb: 0,
            disk_gb: 80,
            reachable: false,
            drift: mackes_mesh_types::cloud::DriftFlag::Unknown,
            app: None,
        }
    }

    fn ready_android_inventory(workload_id: &str, observed_at_unix_ms: u64) -> AndroidAppInventory {
        let provenance = AndroidImageProvenance::new(
            "android-golden",
            "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "aosp-source-2026-08",
            "starter-catalog-v1",
        )
        .expect("valid image provenance");
        let version = AndroidPackageVersion::new("1.0.0", 1).expect("valid package version");
        let entries = AospStarterApp::ALL
            .into_iter()
            .map(|app| AndroidAppInventoryEntry {
                descriptor: app.descriptor(),
                availability: AndroidAppAvailability::Installed,
                package_version: Some(version.clone()),
                readiness: AndroidAppReadiness::Ready,
                launcher_resolvability: AndroidLauncherResolvability::Resolved,
                launch_readiness: AndroidLaunchReadiness::Ready,
                unavailable_reason: None,
            })
            .collect();
        AndroidAppInventory::observed(
            workload_id,
            provenance,
            AndroidGuestBootState::Ready,
            observed_at_unix_ms,
            0,
            entries,
        )
        .expect("valid ready inventory")
    }

    struct ReadyAndroidProvider;

    impl AndroidGuestProvider for ReadyAndroidProvider {
        fn inventory(&self, request: &AndroidGuestInventoryRequest) -> AndroidAppInventory {
            ready_android_inventory(
                &request.workload_id,
                u64::try_from(now_ms()).unwrap_or(1).max(1),
            )
        }

        fn launch(&self, _request: &AndroidGuestLaunchRequest) -> AndroidGuestLaunchOutcome {
            AndroidGuestLaunchOutcome::Unavailable
        }
    }

    #[test]
    fn android_vm_rows_get_sorted_deduplicated_pending_inventory_by_workload_id() {
        let worker = staged_worker(Arc::new(FakeRunner::default()));
        *worker.drift.lock().unwrap() = (
            vec![
                mirror_workload("phone-b", DeliveryType::AndroidVm),
                mirror_workload("service", DeliveryType::ServiceVm),
                mirror_workload("phone-a", DeliveryType::AndroidVm),
                mirror_workload("phone-a", DeliveryType::AndroidVm),
            ],
            DriftSummary::default(),
        );

        let state = worker.build_state();
        assert_eq!(
            state
                .android_inventories
                .iter()
                .map(|inventory| inventory.workload_id.as_str())
                .collect::<Vec<_>>(),
            ["phone-a", "phone-b"]
        );
        assert!(state.android_inventories.iter().all(|inventory| {
            inventory.guest_boot_state
                == mackes_mesh_types::android_apps::AndroidGuestBootState::Pending
                && inventory.observed_at_unix_ms.is_none()
                && inventory.validate().is_ok()
        }));
    }

    #[test]
    fn admitted_android_inventory_replaces_pending_and_replay_cannot_rollback() {
        let worker = staged_worker(Arc::new(FakeRunner::default()));
        *worker.drift.lock().unwrap() = (
            vec![mirror_workload("phone-a", DeliveryType::AndroidVm)],
            DriftSummary::default(),
        );
        assert_eq!(
            worker.build_state().android_inventories[0].guest_boot_state,
            AndroidGuestBootState::Pending
        );

        let request = AndroidGuestInventoryRequest::new("request-1", "phone-a")
            .expect("valid inventory request");
        let mut inventory = AndroidAppInventory::pending("phone-a");
        inventory.guest_boot_state = AndroidGuestBootState::Booting;
        inventory.observed_at_unix_ms = Some(1_786_000_000_000);
        inventory.observation_age_ms = Some(0);
        let response = AndroidGuestInventoryResponse::new(&request, inventory.clone())
            .expect("valid booting inventory response");
        assert_eq!(
            worker.admit_android_inventory_response(&request, response),
            Ok(AndroidInventoryLedgerAdmission::Inserted)
        );

        let state = worker.build_state();
        assert_eq!(
            state.android_inventories[0].guest_boot_state,
            AndroidGuestBootState::Booting
        );
        assert_eq!(
            state.android_inventories[0].observed_at_unix_ms,
            Some(1_786_000_000_000)
        );

        let mut older = inventory;
        older.observed_at_unix_ms = Some(1_785_999_999_999);
        let older_response = AndroidGuestInventoryResponse::new(&request, older)
            .expect("valid older inventory response");
        assert!(matches!(
            worker.admit_android_inventory_response(&request, older_response),
            Err(AndroidInventoryLedgerError::Replay { .. })
        ));
        assert_eq!(
            worker.build_state().android_inventories[0].observed_at_unix_ms,
            Some(1_786_000_000_000)
        );
    }

    #[test]
    fn registered_android_provider_refreshes_and_restarts_from_durable_ledger() {
        let tmp = tempdir().expect("temporary state root");
        let runner = Arc::new(FakeRunner::default());
        let mut worker = CloudWorker::new("me".into(), "peer:me".into(), tmp.path().into())
            .with_runner(runner.clone())
            .with_bus_root(None)
            .with_android_guest_provider("phone-a", Arc::new(ReadyAndroidProvider))
            .expect("register Android provider");
        *worker.drift.lock().expect("drift lock") = (
            vec![mirror_workload("phone-a", DeliveryType::AndroidVm)],
            DriftSummary::default(),
        );

        assert!(worker.refresh_android_inventories());
        let state = worker.build_state();
        assert_eq!(state.android_inventories.len(), 1);
        assert_eq!(
            state.android_inventories[0].guest_boot_state,
            AndroidGuestBootState::Ready
        );
        assert!(state.android_inventories[0].entries.iter().all(|entry| {
            entry.availability == AndroidAppAvailability::Installed && entry.is_launchable()
        }));

        let ledger_path = tmp.path().join("mcnf/cloud/android-inventory/me.json");
        assert!(ledger_path.is_file(), "provider evidence must be durable");

        let restarted = CloudWorker::new("me".into(), "peer:me".into(), tmp.path().into())
            .with_runner(runner)
            .with_bus_root(None);
        *restarted.drift.lock().expect("drift lock") = (
            vec![mirror_workload("phone-a", DeliveryType::AndroidVm)],
            DriftSummary::default(),
        );
        let restored = restarted.build_state();
        assert_eq!(
            restored.android_inventories[0].guest_boot_state,
            AndroidGuestBootState::Ready
        );
        assert_eq!(restored.android_inventories[0].workload_id, "phone-a");
    }

    #[test]
    fn unsigned_manifest_cannot_bypass_catalog_and_provider_preflight() {
        let tmp = tempdir().expect("temporary state root");
        let digest = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let spec = mackes_mesh_types::cloud::WorkloadSpec {
            name: "phone-a".to_owned(),
            delivery_type: DeliveryType::AndroidVm,
            node: "me".to_owned(),
            vcpu: 4,
            memory_mb: 8192,
            disk_gb: 80,
            storage_pool: mackes_mesh_types::cloud::StoragePool::default(),
            image: Some("android-golden".to_owned()),
            image_digest: Some(digest.to_owned()),
            network_isolation: false,
            raw_hcl: None,
            app: None,
        };
        reconcile::write_desired_doc(tmp.path(), &spec).expect("desired Android spec");

        let provenance = AndroidImageProvenance::new(
            "android-golden",
            digest,
            "aosp-source-2026-08",
            "starter-catalog-v1",
        )
        .expect("manifest provenance");
        let version = AndroidPackageVersion::new("1.0.0", 1).expect("package version");
        let manifest = AndroidImagePackageManifest::new(
            provenance,
            AospStarterApp::ALL
                .into_iter()
                .map(|app| AndroidImagePackage::for_app(app, version.clone()))
                .collect(),
        )
        .expect("package manifest");
        let manifest_path =
            android_package_manifest_path(tmp.path(), "phone-a").expect("safe manifest path");
        fs::create_dir_all(manifest_path.parent().expect("manifest parent"))
            .expect("manifest directory");
        fs::write(
            &manifest_path,
            serde_json::to_vec(&manifest).expect("manifest JSON"),
        )
        .expect("manifest file");

        let runner = Arc::new(FakeRunner {
            roster: vec![instance("phone-a", "ACTIVE")],
            ..Default::default()
        });
        let mut worker = CloudWorker::new("me".into(), "peer:me".into(), tmp.path().into())
            .with_runner(runner)
            .with_bus_root(None);
        *worker.drift.lock().expect("drift lock") = (
            vec![mirror_workload("phone-a", DeliveryType::AndroidVm)],
            DriftSummary::default(),
        );

        assert!(!worker.refresh_android_inventories());
        assert!(worker.android_guest_providers.provider("phone-a").is_err());
        let state = worker.build_state();
        assert_eq!(state.android_provider_admissions.len(), 1);
        assert_eq!(
            state.android_provider_admissions[0].refusal,
            Some(mackes_mesh_types::android_provider::AndroidProviderRefusal::CatalogUnavailable)
        );
    }

    #[test]
    fn retained_android_inventory_age_advances_without_mutating_observation_time() {
        let observed_at = 1_786_000_000_000;
        let inventory = ready_android_inventory("phone-a", observed_at);
        let projected = project_android_inventory_age(inventory, observed_at as i64 + 2_500);

        assert_eq!(projected.observed_at_unix_ms, Some(observed_at));
        assert_eq!(projected.observation_age_ms, Some(2_500));
        assert_eq!(projected.guest_boot_state, AndroidGuestBootState::Ready);
        assert!(projected.entries.iter().all(|entry| entry.is_launchable()));
        assert!(projected.validate().is_ok());
    }

    #[test]
    fn retained_android_inventory_becomes_typed_stale_and_non_launchable() {
        let observed_at = 1_786_000_000_000;
        let inventory = ready_android_inventory("phone-a", observed_at);
        let projected = project_android_inventory_age(
            inventory,
            observed_at as i64 + MAX_ANDROID_OBSERVATION_AGE_MS as i64 + 1,
        );

        assert_eq!(projected.observed_at_unix_ms, Some(observed_at));
        assert_eq!(
            projected.observation_age_ms,
            Some(MAX_ANDROID_OBSERVATION_AGE_MS)
        );
        assert_eq!(
            projected.guest_boot_state,
            AndroidGuestBootState::Unavailable
        );
        assert_eq!(
            projected.unavailable_reason,
            Some(AndroidUnavailableReason::ObservationStale)
        );
        assert!(projected.entries.iter().all(|entry| {
            !entry.is_launchable()
                && entry.readiness == AndroidAppReadiness::Unavailable
                && entry.launcher_resolvability == AndroidLauncherResolvability::Unavailable
                && entry.launch_readiness == AndroidLaunchReadiness::Unavailable
                && entry.unavailable_reason == Some(AndroidUnavailableReason::ObservationStale)
        }));
        assert!(projected.validate().is_ok());
    }

    #[test]
    fn non_android_rows_do_not_get_android_inventory() {
        let worker = staged_worker(Arc::new(FakeRunner::default()));
        *worker.drift.lock().unwrap() = (
            vec![
                mirror_workload("service", DeliveryType::ServiceVm),
                mirror_workload("desktop", DeliveryType::DesktopVm),
                mirror_workload("flatpak", DeliveryType::AppVm),
            ],
            DriftSummary::default(),
        );

        assert!(worker.build_state().android_inventories.is_empty());
    }

    #[test]
    fn default_bus_root_uses_the_shared_mde_bus_resolver() {
        assert_eq!(default_bus_root(), mde_bus::default_data_dir());
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
            storage_pool: mackes_mesh_types::cloud::StoragePool::default(),
            image: None,
            image_digest: None,
            network_isolation: false,
            raw_hcl: None,
            app: None,
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
            r#"{{"schema_version":1,"node":"l","armed_token":"{}"}}"#,
            valid_token(
                "provision",
                "l",
                mackes_mesh_types::cloud::CLOUD_ARM_NODE_SCOPE,
                r#"{"schema_version":1,"node":"l"}"#,
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

        // The placement node "l" drains and explicitly refuses the retired verb.
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
        assert!(!reply.ok);
        assert!(reply
            .error
            .as_deref()
            .is_some_and(|error| error.contains("cloud provision is retired")));
        assert!(runner.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn drain_rejects_missing_or_future_schema_before_placement_routing() {
        let tmp = tempfile::tempdir().unwrap();
        let bus = tmp.path().to_path_buf();
        let persist = Persist::open(bus.clone()).unwrap();
        let missing = persist
            .write(
                "action/cloud/provision",
                Priority::Default,
                None,
                Some(r#"{"node":"l"}"#),
            )
            .unwrap();
        let future = persist
            .write(
                "action/cloud/configure",
                Priority::Default,
                None,
                Some(r#"{"schema_version":99,"node":"l"}"#),
            )
            .unwrap();

        // The target is reachable, so a version-blind drain would skip both
        // requests. The envelope gate must answer locally before that route.
        let runner = Arc::new(FakeRunner::default());
        let follower = CloudWorker::new("f".into(), "peer:f".into(), tmp.path().to_path_buf())
            .with_runner(runner.clone())
            .with_bus_root(Some(bus))
            .with_reachable_nodes(Some(HashSet::from(["l".to_string()])));
        assert!(follower.drain_actions(&mut HashMap::new()));

        for (request, expected) in [
            (missing, "requires schema_version 1"),
            (future, "unsupported cloud request schema version 99"),
        ] {
            let replies = persist
                .list_since(&reply_topic(&request.ulid), None)
                .unwrap();
            assert_eq!(replies.len(), 1, "schema refusal must be answered");
            let reply: CloudReply =
                serde_json::from_str(replies[0].body.as_deref().unwrap()).unwrap();
            assert!(!reply.ok);
            assert!(reply
                .error
                .as_deref()
                .is_some_and(|error| error.contains(expected)));
        }
        assert!(runner.calls.lock().unwrap().is_empty());
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
                Some(r#"{"schema_version":1}"#),
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
                Some(r#"{"schema_version":1,"node":"ghost"}"#),
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
    async fn drain_refuses_a_missing_body_before_legacy_read_compatibility() {
        let tmp = tempfile::tempdir().unwrap();
        let bus = tmp.path().to_path_buf();
        let persist = Persist::open(bus.clone()).unwrap();
        let req = persist
            .write("action/cloud/list-instances", Priority::Default, None, None)
            .unwrap();
        let worker = CloudWorker::new("f".into(), "peer:f".into(), tmp.path().to_path_buf())
            .with_runner(Arc::new(FakeRunner {
                roster: vec![instance("secret-vm", "ACTIVE")],
                ..Default::default()
            }))
            .with_bus_root(Some(bus));

        assert!(worker.drain_actions(&mut HashMap::new()));
        let replies = persist.list_since(&reply_topic(&req.ulid), None).unwrap();
        assert_eq!(replies.len(), 1);
        let reply: CloudReply = serde_json::from_str(replies[0].body.as_deref().unwrap()).unwrap();
        assert!(!reply.ok);
        assert!(reply
            .error
            .as_deref()
            .is_some_and(|error| error == "cloud action body is missing"));
        assert!(
            reply.instances.is_none(),
            "missing bodies must not disclose reads"
        );
        assert!(reply.gated.is_none());
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
                Some(r#"{"schema_version":1,"node":"l"}"#),
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
