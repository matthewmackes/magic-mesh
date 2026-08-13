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
use std::io;
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
    cloud_state_topic, CloudInstance, CloudProviderAdapter, CloudReply, CloudState, DeliveryType,
    DeploymentRole, DriftSummary, HealthState, NodeCapacity, ServiceHealth, WorkloadRow,
    CLOUD_ACTION_PREFIX, MAX_ANDROID_INVENTORIES_PER_STATE,
};
use mackes_mesh_types::workloads::{
    reject_duplicate_json_keys, workload_state_topic, WorkloadPowerState, WorkloadStateSnapshot,
    MAX_WORKLOAD_WIRE_BYTES,
};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;

use super::{ShutdownToken, Worker};

use android_provider::{
    configured_image_path, preflight, AndroidHostProbe, AndroidPreflightInput,
    ProductionAndroidHostProbe,
};
pub(crate) use gate::{
    claim_nonce, placement_match, verify_token, HmacTokenSigner, NullSigner, Placement,
    TokenSigner, TokenVerdict, DEFAULT_AUTH_ROOT,
};
use runner::{
    default_browser_vm_image_source, default_iac_root, default_libvirt_uri, instances_table,
    CloudRunner, ShellCloudRunner, BACKEND_TOOLS,
};
#[cfg(test)]
use verbs::AndroidInventoryLedgerError;
#[cfg(test)]
pub(crate) use verbs::{AndroidGuestProvider, AndroidGuestProviderRegistryError};
pub(crate) use verbs::{
    AndroidGuestProviderRegistry, CuttlefishOuterWorkloadObservation,
    WorkloadCuttlefishProviderClient,
};
use verbs::{AndroidInventoryLedger, AndroidInventoryLedgerAdmission, CloudActionBody, CloudVerb};

const WORKLOAD_PROJECTION_MAX_AGE_MS: u64 = 120_000;

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

const CLOUD_ACTION_TXN_PREFIX: &str = "state/cloud/action-transaction/";
const CLOUD_ACTION_TXN_SCHEMA: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BusIdentity {
    device: u64,
    inode: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum CloudActionTxnPhase {
    Claimed,
    Completed,
    Delivered,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct CloudActionTxn {
    schema_version: u16,
    host: String,
    request_ulid: String,
    action_topic: String,
    verb: String,
    phase: CloudActionTxnPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reply: Option<CloudReply>,
}

struct CloudBusActivation {
    identity: BusIdentity,
    cursors: HashMap<String, String>,
    pending_transactions: Vec<CloudActionTxn>,
}

enum StagedActionKind {
    Handle {
        body: String,
        mutation: bool,
        transaction: Option<CloudActionTxn>,
    },
    Reply(CloudReply),
    Skip,
}

struct StagedAction {
    topic: String,
    verb: String,
    ulid: String,
    kind: StagedActionKind,
}

#[cfg(test)]
#[derive(Default)]
struct CloudBusFaults {
    fail_action_read_topic: std::sync::Mutex<Option<String>>,
    fail_reply_writes: std::sync::atomic::AtomicUsize,
    fail_state_writes: std::sync::atomic::AtomicUsize,
}

// ─────────────────────────── the worker ───────────────────────────

/// The `cloud` worker (per-node, rank-0 universal). The action drain is
/// placement-routed (not leader-gated); the `state/cloud/<node>` mirror is
/// per-node universal.
pub struct CloudWorker {
    /// This node's id — the `state/cloud/<host>` namespace and placement key.
    host: String,
    /// Exact daemon node identity consumed by the node-local Workload worker.
    /// Unlike `host`, this retains the `peer:` namespace when one is present.
    workload_node_id: String,
    /// The pinned deployment role published in the cloud mirror.
    deployment_role: DeploymentRole,
    /// Whether this node may receive workload mutations. Lighthouses remain
    /// coordination-only even if a forged Bus request names them as placement.
    workloads_allowed: bool,
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
    /// Explicit Bus-root override. Production resolves the current user root on
    /// every transaction and falls back to the canonical system spool.
    bus_root_override: Option<PathBuf>,
    /// Compatibility mirror consumed by the in-module App VM declaration
    /// handler. It contains only an explicit override; production's `None`
    /// cannot freeze the Cloud run loop's dynamic Bus resolution.
    bus_root: Option<PathBuf>,
    /// Compatibility seam for tests/offline callers that explicitly select
    /// `with_bus_root(None)` to disable Bus work.
    bus_disabled: bool,
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
    #[cfg(test)]
    bus_faults: Arc<CloudBusFaults>,
}

impl CloudWorker {
    /// Construct with production defaults: the [`ShellCloudRunner`] over the
    /// deployed IaC tree + local libvirt, a placement-node-local arming authority,
    /// honest reconcile skeleton and the persisted Bus tree. `host` is this
    /// placement key; `node_id` is the exact identity used by the Workload API;
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
            .unwrap_or_default();

        Self {
            host,
            workload_node_id: node_id,
            deployment_role,
            workloads_allowed,
            runner,
            signer,
            arm_capable,
            auth_root,
            state_root: workgroup_root,
            android_inventory_ledger: std::sync::Mutex::new(android_inventory_ledger),
            android_guest_providers: AndroidGuestProviderRegistry::new(),
            android_provider_admissions: std::sync::Mutex::new(Vec::new()),
            android_host_probe: Arc::new(ProductionAndroidHostProbe::default()),
            android_lifecycle_lock: std::sync::Mutex::new(()),
            android_vdi_sources: std::sync::Mutex::new(Vec::new()),
            android_inventory_path,
            bus_root_override: None,
            bus_root: None,
            bus_disabled: false,
            poll: POLL,
            heartbeat: PUBLISH_HEARTBEAT,
            reachable_override: None,
            drift_interval: DRIFT_POLL,
            android_inventory_interval: ANDROID_INVENTORY_POLL,
            drift: std::sync::Mutex::new((Vec::new(), DriftSummary::default())),
            #[cfg(test)]
            bus_faults: Arc::new(CloudBusFaults::default()),
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

    /// Register one typed Android guest provider in focused worker tests.
    /// Production discovers and admits Cuttlefish adapters during inventory
    /// refresh rather than relying on startup-only injection.
    #[cfg(test)]
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

    /// Override or disable the durable Android observation journal in focused
    /// worker tests. Disabling it also clears constructor-loaded observations so
    /// a shared test root cannot leak evidence between cases. Production keeps
    /// the host-scoped path selected by `new`.
    #[cfg(test)]
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
        self.bus_disabled = root.is_none();
        self.bus_root = root.clone();
        self.bus_root_override = root;
        self
    }

    #[cfg(test)]
    fn with_bus_faults(mut self, faults: Arc<CloudBusFaults>) -> Self {
        self.bus_faults = faults;
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

    /// Handle one `action/cloud/<verb>` request end to end → a typed [`CloudReply`].
    ///
    /// The Bus drain routes placement-scoped requests before calling this method,
    /// but this is also a public crate seam used by adapters and tests. Re-checking
    /// placement here prevents a direct caller from presenting a valid capability
    /// for another node and making this worker run that node's action.
    #[must_use]
    pub fn handle(&self, verb_name: &str, body: &str) -> CloudReply {
        if let Some(reply) = self.reject_lighthouse_workload(verb_name) {
            return reply;
        }
        if let Some(reply) = self.reject_nonlocal_placement(verb_name, body) {
            return reply;
        }
        if matches!(
            CloudVerb::from_verb(verb_name),
            Some(CloudVerb::AndroidLifecycle)
        ) {
            let Ok(_lifecycle) = self.android_lifecycle_lock.lock() else {
                return CloudReply {
                    ok: false,
                    verb: verb_name.to_owned(),
                    error: Some(
                        "Android lifecycle serialization is unavailable; nothing changed"
                            .to_owned(),
                    ),
                    ..Default::default()
                };
            };
            return verbs::dispatch(self, verb_name, body);
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

    fn bus_root(&self) -> Option<PathBuf> {
        if self.bus_disabled {
            return None;
        }
        Some(cloud_bus_root_or_system(
            self.bus_root_override.clone().or_else(default_bus_root),
        ))
    }

    fn open_bus(&self) -> io::Result<Option<(Persist, BusIdentity)>> {
        let Some(root) = self.bus_root() else {
            return Ok(None);
        };
        let persist = Persist::open(root.clone()).map_err(io_other)?;
        let identity = bus_identity(&root)?;
        Ok(Some((persist, identity)))
    }

    fn transaction_topic(&self, request_ulid: &str) -> String {
        format!("{CLOUD_ACTION_TXN_PREFIX}{}/{request_ulid}", self.host)
    }

    fn read_transaction(
        &self,
        persist: &Persist,
        action_topic: &str,
        verb: &str,
        request_ulid: &str,
    ) -> io::Result<Option<CloudActionTxn>> {
        let Some(message) = persist
            .read_latest(&self.transaction_topic(request_ulid))
            .map_err(io_other)?
        else {
            return Ok(None);
        };
        let body = message
            .body
            .ok_or_else(|| io::Error::other("cloud action transaction has no body"))?;
        let transaction: CloudActionTxn = serde_json::from_str(&body).map_err(io_other)?;
        if transaction.schema_version != CLOUD_ACTION_TXN_SCHEMA
            || transaction.host != self.host
            || transaction.request_ulid != request_ulid
            || transaction.action_topic != action_topic
            || transaction.verb != verb
        {
            return Err(io::Error::other(
                "cloud action transaction identity does not match request",
            ));
        }
        Ok(Some(transaction))
    }

    fn write_transaction(&self, persist: &Persist, transaction: &CloudActionTxn) -> io::Result<()> {
        let body = serde_json::to_string(transaction).map_err(io_other)?;
        persist
            .write(
                &self.transaction_topic(&transaction.request_ulid),
                Priority::Default,
                None,
                Some(&body),
            )
            .map_err(io_other)?;
        Ok(())
    }

    /// Whether placement node `node` is reachable — the test override when set,
    /// else a fresh `state/cloud/<node>` mirror on the bus. Read/open/decode
    /// failures defer the complete action pass rather than becoming a false
    /// unreachable decision.
    fn node_reachable(&self, persist: &Persist, node: &str) -> io::Result<bool> {
        if let Some(set) = &self.reachable_override {
            return Ok(set.contains(node));
        }
        let Some(message) = persist
            .read_latest(&cloud_state_topic(node))
            .map_err(io_other)?
        else {
            return Ok(false);
        };
        let body = message
            .body
            .ok_or_else(|| io::Error::other("cloud reachability mirror has no body"))?;
        let state: CloudState = serde_json::from_str(&body).map_err(io_other)?;
        Ok(now_ms().saturating_sub(state.published_at_ms) <= PLACEMENT_STALE_AFTER_MS)
    }

    fn write_reply(
        &self,
        persist: &Persist,
        request_ulid: &str,
        reply: &CloudReply,
    ) -> io::Result<()> {
        #[cfg(test)]
        if take_fault(&self.bus_faults.fail_reply_writes) {
            return Err(io::Error::other("injected cloud reply write failure"));
        }
        let body = serde_json::to_string(reply).map_err(io_other)?;
        persist
            .write(
                &reply_topic(request_ulid),
                Priority::Default,
                None,
                Some(&body),
            )
            .map_err(io_other)?;
        Ok(())
    }

    fn reply_already_published(
        &self,
        persist: &Persist,
        request_ulid: &str,
        reply: &CloudReply,
    ) -> io::Result<bool> {
        let Some(message) = persist
            .read_latest(&reply_topic(request_ulid))
            .map_err(io_other)?
        else {
            return Ok(false);
        };
        let body = message
            .body
            .ok_or_else(|| io::Error::other("cloud reply row has no body"))?;
        Ok(body == serde_json::to_string(reply).map_err(io_other)?)
    }

    fn deliver_transaction(
        &self,
        persist: &Persist,
        transaction: &mut CloudActionTxn,
    ) -> io::Result<()> {
        if transaction.phase == CloudActionTxnPhase::Claimed {
            transaction.reply = Some(CloudReply {
                ok: false,
                verb: transaction.verb.clone(),
                gated: Some(
                    "cloud mutation outcome is unavailable after recovery; the claimed action was not repeated"
                        .to_string(),
                ),
                ..Default::default()
            });
            transaction.phase = CloudActionTxnPhase::Completed;
            self.write_transaction(persist, transaction)?;
        }
        if transaction.phase == CloudActionTxnPhase::Completed {
            let reply = transaction
                .reply
                .as_ref()
                .ok_or_else(|| io::Error::other("completed cloud transaction has no reply"))?;
            if !self.reply_already_published(persist, &transaction.request_ulid, reply)? {
                self.write_reply(persist, &transaction.request_ulid, reply)?;
            }
            transaction.phase = CloudActionTxnPhase::Delivered;
            self.write_transaction(persist, transaction)?;
        }
        Ok(())
    }

    fn activate_bus(
        &self,
        persist: &Persist,
        identity: BusIdentity,
    ) -> io::Result<CloudBusActivation> {
        let mut topics = persist.list_topics().map_err(io_other)?;
        topics.sort_unstable();
        let mut cursors = HashMap::new();
        let mut pending_transactions = Vec::new();
        let transaction_prefix = format!("{CLOUD_ACTION_TXN_PREFIX}{}/", self.host);
        for topic in topics {
            if topic.starts_with(CLOUD_ACTION_PREFIX) {
                #[cfg(test)]
                if self
                    .bus_faults
                    .fail_action_read_topic
                    .lock()
                    .is_ok_and(|fault| fault.as_deref() == Some(topic.as_str()))
                {
                    return Err(io::Error::other("injected cloud activation tail failure"));
                }
                if let Some(ulid) = persist.latest_ulid(&topic).map_err(io_other)? {
                    cursors.insert(topic, ulid);
                }
            } else if topic.starts_with(&transaction_prefix) {
                let message = persist
                    .read_latest(&topic)
                    .map_err(io_other)?
                    .ok_or_else(|| io::Error::other("listed cloud transaction disappeared"))?;
                let body = message
                    .body
                    .ok_or_else(|| io::Error::other("cloud transaction row has no body"))?;
                let transaction: CloudActionTxn = serde_json::from_str(&body).map_err(io_other)?;
                if transaction.schema_version != CLOUD_ACTION_TXN_SCHEMA
                    || transaction.host != self.host
                    || self.transaction_topic(&transaction.request_ulid) != topic
                {
                    return Err(io::Error::other(
                        "invalid cloud transaction during activation",
                    ));
                }
                if transaction.phase != CloudActionTxnPhase::Delivered {
                    if transaction.phase == CloudActionTxnPhase::Completed {
                        let reply = transaction.reply.as_ref().ok_or_else(|| {
                            io::Error::other("completed cloud transaction has no reply")
                        })?;
                        let _ = persist
                            .read_latest(&reply_topic(&transaction.request_ulid))
                            .map_err(io_other)?;
                        let _ = serde_json::to_string(reply).map_err(io_other)?;
                    }
                    pending_transactions.push(transaction);
                }
            }
        }
        Ok(CloudBusActivation {
            identity,
            cursors,
            pending_transactions,
        })
    }

    fn recover_pending_transactions(
        &self,
        persist: &Persist,
        pending: &mut Vec<CloudActionTxn>,
    ) -> io::Result<()> {
        let mut retained = Vec::new();
        let mut first_error = None;
        for mut transaction in std::mem::take(pending) {
            if let Err(error) = self.deliver_transaction(persist, &mut transaction) {
                retained.push(transaction);
                first_error.get_or_insert(error);
            }
        }
        *pending = retained;
        first_error.map_or(Ok(()), Err)
    }

    /// Stage every topic, message, transaction record, and required reachability
    /// mirror before dispatching any backend work.
    fn stage_actions(
        &self,
        persist: &Persist,
        cursors: &HashMap<String, String>,
    ) -> io::Result<Vec<StagedAction>> {
        let mut topics = persist.list_topics().map_err(io_other)?;
        topics.sort_unstable();
        let mut staged = Vec::new();
        for topic in topics {
            let Some(verb_name) = topic.strip_prefix(CLOUD_ACTION_PREFIX) else {
                continue;
            };
            let verb_name = verb_name.to_string();
            let classified = CloudVerb::from_verb(&verb_name);
            let placement_scoped = classified.is_some_and(CloudVerb::requires_placement);
            let cursor = cursors.get(&topic).cloned();
            #[cfg(test)]
            if self
                .bus_faults
                .fail_action_read_topic
                .lock()
                .is_ok_and(|fault| fault.as_deref() == Some(topic.as_str()))
            {
                return Err(io::Error::other("injected cloud action read failure"));
            }
            let messages = persist
                .list_since(&topic, cursor.as_deref())
                .map_err(io_other)?;
            for message in messages {
                let ulid = message.ulid;
                let Some(body) = message.body else {
                    staged.push(StagedAction {
                        topic: topic.clone(),
                        verb: verb_name.clone(),
                        ulid,
                        kind: StagedActionKind::Reply(CloudReply {
                            ok: false,
                            verb: verb_name.clone(),
                            error: Some("cloud action body is missing".to_string()),
                            ..Default::default()
                        }),
                    });
                    continue;
                };
                let parsed = CloudActionBody::parse(&body);
                if let Some(verb) = classified {
                    if let Some(error) = parsed.schema_error_for(verb) {
                        staged.push(StagedAction {
                            topic: topic.clone(),
                            verb: verb_name.clone(),
                            ulid,
                            kind: StagedActionKind::Reply(CloudReply {
                                ok: false,
                                verb: verb_name.clone(),
                                error: Some(error),
                                ..Default::default()
                            }),
                        });
                        continue;
                    }
                }
                let route = if placement_scoped {
                    match placement_match(&parsed.node, &self.host) {
                        Placement::Local => Route::Handle,
                        Placement::Missing => Route::GateMissing,
                        Placement::Remote(n) => {
                            if self.node_reachable(persist, &n)? {
                                Route::Skip
                            } else {
                                Route::GateUnreachable(n)
                            }
                        }
                    }
                } else {
                    Route::Handle
                };
                let kind = match route {
                    Route::Handle => {
                        let mutation = classified.is_some_and(CloudVerb::is_mutation);
                        let transaction = if mutation {
                            self.read_transaction(persist, &topic, &verb_name, &ulid)?
                        } else {
                            None
                        };
                        StagedActionKind::Handle {
                            body,
                            mutation,
                            transaction,
                        }
                    }
                    Route::Skip => StagedActionKind::Skip,
                    Route::GateMissing => StagedActionKind::Reply(CloudReply {
                        ok: false,
                        verb: verb_name.clone(),
                        gated: Some("cloud action requires an explicit placement node".to_string()),
                        ..Default::default()
                    }),
                    Route::GateUnreachable(node) => StagedActionKind::Reply(CloudReply {
                        ok: false,
                        verb: verb_name.clone(),
                        gated: Some(format!("placement node {node} not reachable")),
                        ..Default::default()
                    }),
                };
                staged.push(StagedAction {
                    topic: topic.clone(),
                    verb: verb_name.clone(),
                    ulid,
                    kind,
                });
            }
        }
        Ok(staged)
    }

    fn apply_staged_actions(
        &self,
        persist: &Persist,
        cursors: &mut HashMap<String, String>,
        staged: Vec<StagedAction>,
    ) -> io::Result<bool> {
        let mut acted = false;
        for action in staged {
            match action.kind {
                StagedActionKind::Skip => {
                    cursors.insert(action.topic, action.ulid);
                }
                StagedActionKind::Reply(reply) => {
                    self.write_reply(persist, &action.ulid, &reply)?;
                    cursors.insert(action.topic, action.ulid);
                    acted = true;
                }
                StagedActionKind::Handle {
                    body,
                    mutation,
                    transaction,
                } => {
                    if mutation {
                        let mut transaction = if let Some(transaction) = transaction {
                            transaction
                        } else {
                            let transaction = CloudActionTxn {
                                schema_version: CLOUD_ACTION_TXN_SCHEMA,
                                host: self.host.clone(),
                                request_ulid: action.ulid.clone(),
                                action_topic: action.topic.clone(),
                                verb: action.verb.clone(),
                                phase: CloudActionTxnPhase::Claimed,
                                reply: None,
                            };
                            self.write_transaction(persist, &transaction)?;
                            let reply = self.handle(&action.verb, &body);
                            let completed = CloudActionTxn {
                                phase: CloudActionTxnPhase::Completed,
                                reply: Some(reply),
                                ..transaction
                            };
                            self.write_transaction(persist, &completed)?;
                            completed
                        };
                        self.deliver_transaction(persist, &mut transaction)?;
                    } else {
                        let reply = self.handle(&action.verb, &body);
                        self.write_reply(persist, &action.ulid, &reply)?;
                    }
                    tracing::info!(
                        target: "mackesd::cloud",
                        ulid = %action.ulid,
                        verb = %action.verb,
                        "cloud action transaction committed"
                    );
                    cursors.insert(action.topic, action.ulid);
                    acted = true;
                }
            }
        }
        Ok(acted)
    }

    fn drain_actions_on(
        &self,
        persist: &Persist,
        cursors: &mut HashMap<String, String>,
    ) -> io::Result<bool> {
        let staged = self.stage_actions(persist, cursors)?;
        self.apply_staged_actions(persist, cursors, staged)
    }

    /// Compatibility seam retained for focused tests.
    #[cfg(test)]
    fn drain_actions(&self, cursors: &mut HashMap<String, String>) -> bool {
        let result = self.open_bus().and_then(|opened| match opened {
            Some((persist, _)) => self.drain_actions_on(&persist, cursors),
            None => Ok(false),
        });
        match result {
            Ok(acted) => acted,
            Err(error) => {
                tracing::warn!(target: "mackesd::cloud", %error, "cloud action transaction deferred");
                false
            }
        }
    }

    /// Atomically seed all existing Cloud action lanes. A failure leaves the
    /// caller's prior cursor set untouched.
    #[cfg(test)]
    fn prime_cursors(&self, cursors: &mut HashMap<String, String>) {
        let Ok(Some((persist, identity))) = self.open_bus() else {
            return;
        };
        if let Ok(activation) = self.activate_bus(&persist, identity) {
            *cursors = activation.cursors;
        }
    }

    /// Run one throttled drift tick: render + `tofu plan` THIS node's desired slice,
    /// fold the live roster into per-workload rows + the node drift rollup, and cache
    /// it for the next `state/cloud/<node>` publish. Best-effort + honest (§7) — a
    /// plan the backend can't run leaves each row's drift `Unknown`, never a
    /// fabricated in-sync. A no-op when the node has nothing declared (empty slice).
    fn refresh_drift(&self) {
        let roster = match self.workload_instances() {
            Ok(instances) => Some(instances),
            Err(error) => {
                tracing::warn!(
                    target: "mackesd::cloud",
                    %error,
                    "drift tick has no authoritative Workload roster"
                );
                None
            }
        };
        let snapshot = reconcile::drift_snapshot(
            self.runner.as_ref(),
            roster.as_deref(),
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

    /// Read and validate the sole authoritative local runtime projection.
    fn workload_snapshot(&self) -> Result<WorkloadStateSnapshot, String> {
        let Some((persist, _)) = self
            .open_bus()
            .map_err(|error| format!("Workload projection Bus unavailable: {error}"))?
        else {
            return Err("Workload projection Bus is disabled".to_string());
        };
        let topic = workload_state_topic(&self.host);
        let message = persist
            .read_latest(&topic)
            .map_err(|error| format!("Workload projection read failed: {error}"))?
            .ok_or_else(|| format!("authoritative Workload projection is absent: {topic}"))?;
        let body = message
            .body
            .as_deref()
            .ok_or_else(|| "authoritative Workload projection has no body".to_string())?;
        if body.len() > MAX_WORKLOAD_WIRE_BYTES {
            return Err("authoritative Workload projection exceeds its wire bound".to_string());
        }
        reject_duplicate_json_keys(body)
            .map_err(|_| "authoritative Workload projection contains duplicate keys".to_string())?;
        let snapshot: WorkloadStateSnapshot = serde_json::from_str(body)
            .map_err(|error| format!("authoritative Workload projection is malformed: {error}"))?;
        let observed_now = u64::try_from(now_ms()).unwrap_or(0);
        if snapshot.node != self.host {
            return Err("authoritative Workload projection belongs to another node".to_string());
        }
        snapshot
            .validate(observed_now)
            .map_err(|error| format!("authoritative Workload projection is invalid: {error}"))?;
        if snapshot.observed_at_ms > observed_now
            || observed_now.saturating_sub(snapshot.observed_at_ms) > WORKLOAD_PROJECTION_MAX_AGE_MS
        {
            return Err("authoritative Workload projection is stale or future-dated".to_string());
        }
        Ok(snapshot)
    }

    /// Project the sole authoritative Workloads snapshot into Cloud's legacy
    /// read-only row shape. Cloud must not rediscover VM power with `virsh`.
    fn workload_instances(&self) -> Result<Vec<CloudInstance>, String> {
        Ok(self
            .workload_snapshot()?
            .workloads
            .into_iter()
            .filter(|status| status.backend.is_vm())
            .map(|status| {
                let state = match status.power {
                    WorkloadPowerState::Running => "ACTIVE",
                    WorkloadPowerState::Stopped | WorkloadPowerState::Defined => "SHUTOFF",
                    WorkloadPowerState::Paused => "PAUSED",
                    WorkloadPowerState::Starting => "STARTING",
                    WorkloadPowerState::Stopping => "STOPPING",
                    WorkloadPowerState::Failed => "FAILED",
                };
                let id = status.workload_id.into_string();
                CloudInstance {
                    id: id.clone(),
                    name: id,
                    status: state.to_string(),
                    flavor: None,
                    image: status.image_ref,
                    networks: None,
                }
            })
            .collect())
    }

    /// Register production Cuttlefish adapters for Android desired rows whose
    /// verified package manifest is present. A missing manifest leaves the
    /// workload on the existing pending projection; it never gets a guessed
    /// image provenance or a fake provider.
    fn ensure_configured_cuttlefish_providers(&mut self) -> HashMap<String, u64> {
        let catalog = self.load_admitted_android_catalog();
        let artifact = configured_image_path();
        // One validated typed snapshot feeds every Cuttlefish adapter in this
        // refresh. `Err` is authority loss and must not be converted to an
        // absent outer VM; an available snapshot with no matching row is the
        // only honest absence signal.
        let workload_snapshot = self.workload_snapshot();
        let provider_healthy =
            self.runner.probe_tool(runner::TOOL_LIBVIRT).state == HealthState::Up;
        let observed_at = u64::try_from(now_ms()).unwrap_or(1).max(1);
        let mut admissions = Vec::new();
        let mut provider_generations = HashMap::new();
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
            let (outer_workload, generation) = match workload_snapshot.as_ref() {
                Err(_) => (None, 0),
                Ok(snapshot) => match snapshot
                    .workloads
                    .iter()
                    .find(|status| status.workload_id.as_str() == spec.name)
                {
                    None => (
                        Some(CuttlefishOuterWorkloadObservation::absent(&spec.name)),
                        0,
                    ),
                    Some(status) => match CuttlefishOuterWorkloadObservation::from_status(status) {
                        Ok(observation) => (Some(observation), status.generation),
                        Err(error) => {
                            tracing::warn!(target: "mackesd::cloud", workload = %spec.name, ?error, "non-VM Workload row refused by Cuttlefish provider");
                            continue;
                        }
                    },
                },
            };
            let Ok(client) = WorkloadCuttlefishProviderClient::with_guest_contract(
                outer_workload,
                manifest.clone(),
                catalog_digest,
            ) else {
                continue;
            };
            let workload_id = spec.name.clone();
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
            } else if generation > 0 {
                provider_generations.insert(workload_id, generation);
            }
        }
        admissions.sort_by(|left, right| left.workload_id.cmp(&right.workload_id));
        if let Ok(mut retained) = self.android_provider_admissions.lock() {
            *retained = admissions;
        }
        provider_generations
    }

    fn load_admitted_android_catalog(&self) -> Option<AndroidSignedCatalog> {
        const MAX_CATALOG_BYTES: usize = 2 * 1024 * 1024;
        let root = self.bus_root()?;
        let persist = Persist::open(root).ok()?;
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
        let provider_generations = self.ensure_configured_cuttlefish_providers();
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
        let mut vdi_sources = Vec::new();

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
            let inventory_admitted = match self.admit_android_inventory_response(&request, response)
            {
                Ok(AndroidInventoryLedgerAdmission::Inserted
                    | AndroidInventoryLedgerAdmission::Replaced) => {
                        changed = true;
                        true
                    }
                Ok(AndroidInventoryLedgerAdmission::Unchanged) => true,
                Err(error) => {
                    tracing::warn!(
                        target: "mackesd::cloud",
                        workload = %workload_id,
                        ?error,
                        "Android inventory observation was not retained"
                    );
                    false
                }
            };
            if !inventory_admitted {
                continue;
            }
            if let Some(source) = provider_generations
                .get(&workload_id)
                .and_then(|generation| {
                    self.android_guest_providers
                        .vdi_source(&workload_id, *generation)
                })
            {
                vdi_sources.push(source);
            }
        }
        vdi_sources.sort_by(|left, right| left.workload_id.cmp(&right.workload_id));
        if let Ok(mut retained) = self.android_vdi_sources.lock() {
            if *retained != vdi_sources {
                *retained = vdi_sources;
                changed = true;
            }
        }
        changed
    }

    /// Build the current `state/cloud/<node>` mirror: probe each backend tool's
    /// health + fold the typed Workload roster into a resource table, plus
    /// the latest drift tick's per-workload rows + rollup (U5).
    #[must_use]
    pub fn build_state(&self) -> CloudState {
        let health: Vec<ServiceHealth> = BACKEND_TOOLS
            .iter()
            .map(|t| self.runner.probe_tool(t))
            .collect();
        let resources = match self.workload_instances() {
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

    /// Publish the current mirror. Callers retain dirty state and retry until
    /// this complete write succeeds.
    fn publish_state(&self, persist: &Persist) -> io::Result<()> {
        #[cfg(test)]
        if take_fault(&self.bus_faults.fail_state_writes) {
            return Err(io::Error::other("injected cloud state write failure"));
        }
        let state = self.build_state();
        let body = serde_json::to_string(&state).map_err(io_other)?;
        persist
            .write(
                &cloud_state_topic(&self.host),
                Priority::Default,
                None,
                Some(&body),
            )
            .map_err(io_other)?;
        Ok(())
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
        let mut activation: Option<CloudBusActivation> = None;
        let mut state_dirty = true;
        let mut last_pub = Instant::now();
        let mut last_drift = Instant::now();
        let mut last_android_inventory = Instant::now();
        loop {
            match self.open_bus() {
                Err(error) => {
                    tracing::warn!(target: "mackesd::cloud", %error, "cloud Bus unavailable; worker will retry");
                    activation = None;
                }
                Ok(None) => {}
                Ok(Some((persist, identity))) => {
                    let needs_activation = activation
                        .as_ref()
                        .is_none_or(|active| active.identity != identity);
                    if needs_activation {
                        match self.activate_bus(&persist, identity) {
                            Ok(active) => {
                                activation = Some(active);
                                state_dirty = true;
                            }
                            Err(error) => {
                                tracing::warn!(target: "mackesd::cloud", %error, "cloud Bus activation deferred");
                                activation = None;
                            }
                        }
                    }

                    if let Some(active) = activation.as_mut() {
                        let recovery_ready = match self.recover_pending_transactions(
                            &persist,
                            &mut active.pending_transactions,
                        ) {
                            Ok(()) => true,
                            Err(error) => {
                                tracing::warn!(target: "mackesd::cloud", %error, "cloud action outbox recovery deferred");
                                false
                            }
                        };
                        if recovery_ready {
                            let cursors_before = active.cursors.clone();
                            match self.drain_actions_on(&persist, &mut active.cursors) {
                                Ok(acted) => state_dirty |= acted,
                                Err(error) => {
                                    state_dirty |= active.cursors != cursors_before;
                                    tracing::warn!(target: "mackesd::cloud", %error, "cloud action sweep deferred");
                                }
                            }
                        }

                        let drift_due = last_drift.elapsed() >= self.drift_interval;
                        if drift_due {
                            self.refresh_drift();
                            last_drift = Instant::now();
                            state_dirty = true;
                        }
                        // Provider discovery is the first step of the refresh, so
                        // an empty registry cannot be used as its own scheduling
                        // prerequisite. Production starts empty by design.
                        let inventory_due = drift_due
                            || last_android_inventory.elapsed()
                                >= self.android_inventory_interval;
                        if inventory_due {
                            last_android_inventory = Instant::now();
                            state_dirty |= self.refresh_android_inventories();
                        }
                        if state_dirty || last_pub.elapsed() >= self.heartbeat {
                            match self.publish_state(&persist) {
                                Ok(()) => {
                                    state_dirty = false;
                                    last_pub = Instant::now();
                                }
                                Err(error) => {
                                    state_dirty = true;
                                    tracing::warn!(target: "mackesd::cloud", %error, "cloud state publication failed; corrected-forward retry retained");
                                }
                            }
                        }
                    }
                }
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

fn cloud_bus_root_or_system(resolved: Option<PathBuf>) -> PathBuf {
    resolved.unwrap_or_else(|| PathBuf::from(mde_bus::SYSTEM_BUS_ROOT))
}

fn io_other(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

fn bus_identity(root: &Path) -> io::Result<BusIdentity> {
    let metadata = fs::metadata(root.join("index.sqlite"))?;
    if !metadata.is_file() {
        return Err(io::Error::other("cloud Bus index is not a regular file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(BusIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        Ok(BusIdentity {
            device: 0,
            inode: 0,
        })
    }
}

#[cfg(test)]
fn take_fault(counter: &std::sync::atomic::AtomicUsize) -> bool {
    counter
        .fetch_update(
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
            |remaining| remaining.checked_sub(1),
        )
        .is_ok()
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
    use ed25519_dalek::SigningKey;
    use mackes_mesh_types::android_apps::{
        AndroidAppAvailability, AndroidAppCapability, AndroidAppInventoryEntry,
        AndroidAppPermission, AndroidAppReadiness, AndroidCatalogAppPolicy,
        AndroidCatalogGuestReadiness, AndroidCatalogPayload, AndroidGuestBootState,
        AndroidGuestInventoryRequest, AndroidGuestInventoryResponse, AndroidGuestLaunchOutcome,
        AndroidGuestLaunchRequest, AndroidImageManifest, AndroidImagePackage,
        AndroidImagePackageManifest, AndroidImageProvenance, AndroidLaunchReadiness,
        AndroidLauncherResolvability, AndroidPackageVersion, AndroidResourceClass,
        AndroidResourceProfile, AndroidUnavailableReason, AospStarterApp, AospStarterCatalog,
        ANDROID_SIGNED_CATALOG_SCHEMA_VERSION,
    };
    use mackes_mesh_types::cloud::{CloudProviderAdapter, HealthState};
    use tempfile::tempdir;

    const KEY: &[u8] = b"test-mesh-arming-key";
    static ANDROID_RELEASE_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[derive(Default)]
    struct ScopedEnvironment(Vec<(&'static str, Option<std::ffi::OsString>)>);

    impl ScopedEnvironment {
        fn set(&mut self, name: &'static str, value: impl AsRef<std::ffi::OsStr>) {
            self.0.push((name, std::env::var_os(name)));
            std::env::set_var(name, value);
        }
    }

    impl Drop for ScopedEnvironment {
        fn drop(&mut self) {
            for (name, previous) in self.0.drain(..).rev() {
                match previous {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }

    struct ReadyAndroidHostProbe {
        digest: String,
    }

    impl AndroidHostProbe for ReadyAndroidHostProbe {
        fn facts(&self, _artifact: Option<&Path>) -> android_provider::AndroidHostFacts {
            android_provider::AndroidHostFacts {
                kvm_available: true,
                nested_virtualization: true,
                available_vcpus: 16,
                available_memory_mib: 32 * 1_024,
                available_disk_mib: 256 * 1_024,
            }
        }

        fn image_digest(&self, _artifact: &Path) -> io::Result<String> {
            Ok(self.digest.clone())
        }
    }

    #[derive(Default)]
    struct AndroidProvisionRunner {
        calls: std::sync::Mutex<Vec<String>>,
    }

    impl CloudRunner for AndroidProvisionRunner {
        fn probe_tool(&self, tool: &str) -> ServiceHealth {
            ServiceHealth {
                service_type: tool.to_owned(),
                interface: mackes_mesh_types::cloud::EndpointInterface::Internal,
                url: "(test)".to_owned(),
                state: if tool == TOOL_LIBVIRT {
                    HealthState::Up
                } else {
                    HealthState::Absent
                },
                latency_ms: Some(1),
                microversion: None,
                version_id: None,
                detail: Some("Android provision fixture".to_owned()),
            }
        }

        fn configure(&self) -> runner::CloudRunOutcome {
            self.calls.lock().unwrap().push("configure".to_owned());
            runner::CloudRunOutcome::failed("unexpected configure call")
        }

        fn plan_json(&self, _tfvars_json: &str) -> Result<String, String> {
            self.calls.lock().unwrap().push("plan".to_owned());
            Err("unexpected plan call".to_owned())
        }
    }

    fn install_android_release_fixture(
        root: &Path,
        now: u64,
    ) -> (ScopedEnvironment, Arc<dyn AndroidHostProbe>) {
        use sha2::{Digest, Sha256};

        let image_bytes = b"android-provision-test-image";
        let digest = format!("sha256:{:x}", Sha256::digest(image_bytes));
        let image_manifest = AndroidImageManifest::new(
            "android-test-image",
            digest.clone(),
            "aosp-source-test",
            "starter-catalog-v1",
            now.saturating_sub(2_000),
            now.saturating_sub(1_000),
            AospStarterCatalog::v1(),
        )
        .unwrap();
        let package_manifest = AndroidImagePackageManifest::new(
            AndroidImageProvenance::from_manifest(&image_manifest).unwrap(),
            AospStarterApp::ALL
                .into_iter()
                .map(|app| {
                    AndroidImagePackage::for_app(
                        app,
                        AndroidPackageVersion::new("2026.08.12", 1).unwrap(),
                    )
                })
                .collect(),
        )
        .unwrap();
        let app_policies = AospStarterApp::ALL
            .into_iter()
            .map(|app| AndroidCatalogAppPolicy {
                app,
                permissions: vec![AndroidAppPermission::Network],
                capabilities: vec![AndroidAppCapability::VdiDisplay],
                resources: AndroidResourceProfile {
                    class: AndroidResourceClass::Standard,
                    vcpus: 4,
                    memory_mib: 8_192,
                    disk_mib: 80 * 1_024,
                },
                guest_readiness: AndroidCatalogGuestReadiness::BootedInventoryAndLauncherReady,
            })
            .collect();
        let key = SigningKey::from_bytes(&[41; 32]);
        let catalog = AndroidSignedCatalog::sign(
            "android-release-v1",
            AndroidCatalogPayload {
                schema_version: ANDROID_SIGNED_CATALOG_SCHEMA_VERSION,
                catalog_id: "android-provision-test".to_owned(),
                revision: 1,
                issued_at_unix_ms: now.saturating_sub(500),
                expires_at_unix_ms: now.saturating_add(60_000),
                image_manifest,
                package_manifest,
                app_policies,
            },
            &key,
        )
        .unwrap();

        let trust_key = root.join("android-catalog-trust.hex");
        let state_file = root.join("android-catalog-state.json");
        let artifact = root.join("android-test-image.raw");
        let trust_key_hex = key
            .verifying_key()
            .to_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        fs::write(&trust_key, trust_key_hex).unwrap();
        fs::write(
            &state_file,
            serde_json::json!({"schema_version": 1, "catalog": catalog}).to_string(),
        )
        .unwrap();
        fs::write(&artifact, image_bytes).unwrap();

        let mut environment = ScopedEnvironment::default();
        environment.set("MDE_ANDROID_CATALOG_SIGNER_ID", "android-release-v1");
        environment.set("MDE_ANDROID_CATALOG_TRUST_KEY_FILE", &trust_key);
        environment.set("MDE_ANDROID_CATALOG_STATE_FILE", &state_file);
        environment.set("MDE_ANDROID_IMAGE_FILE", &artifact);
        (
            environment,
            Arc::new(ReadyAndroidHostProbe { digest }),
        )
    }

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

    fn workload_status(
        id: &str,
        backend: mackes_mesh_types::workloads::WorkloadBackend,
        power: WorkloadPowerState,
    ) -> mackes_mesh_types::workloads::WorkloadOperationStatus {
        use mackes_mesh_types::workloads::{
            WorkloadOperationPhase, WorkloadReadiness, WorkloadResources, WorkloadRuntimeSignals,
            WORKLOAD_CONTRACT_SCHEMA_VERSION,
        };
        mackes_mesh_types::workloads::WorkloadOperationStatus {
            schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
            request_id: format!("request-{id}"),
            workload_id: mackes_mesh_types::workloads::WorkloadId::new(id).unwrap(),
            backend,
            resources: WorkloadResources {
                vcpu: 2,
                memory_mb: 2_048,
                disk_gb: 20,
            },
            image_ref: Some(format!("image-{id}")),
            generation: 1,
            phase: WorkloadOperationPhase::Completed,
            power,
            readiness: WorkloadReadiness::Ready,
            signals: WorkloadRuntimeSignals::default(),
            retryable: false,
            attempt: 1,
            next_retry_at_ms: 0,
            reason: None,
            remediation: None,
            attachment: None,
        }
    }

    fn projection_worker(
        runner: Arc<dyn CloudRunner>,
        workloads: Vec<mackes_mesh_types::workloads::WorkloadOperationStatus>,
        observed_at_ms: u64,
        arm_capable: bool,
    ) -> (tempfile::TempDir, CloudWorker) {
        use mackes_mesh_types::workloads::WORKLOAD_CONTRACT_SCHEMA_VERSION;
        let temp = tempfile::tempdir().unwrap();
        let bus = temp.path().join("bus");
        let snapshot = WorkloadStateSnapshot {
            schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
            node: "me".to_string(),
            observed_at_ms,
            workloads,
        };
        Persist::open(bus.clone())
            .unwrap()
            .write(
                &workload_state_topic("me"),
                Priority::Default,
                None,
                Some(&serde_json::to_string(&snapshot).unwrap()),
            )
            .unwrap();
        let mut worker = CloudWorker::new("me".into(), "peer:me".into(), temp.path().join("state"))
            .with_runner(runner)
            .with_android_inventory_path(None)
            .with_bus_root(Some(bus));
        if arm_capable {
            worker = worker
                .with_signer(Arc::new(signer()))
                .with_arm_capable(true);
        }
        (temp, worker)
    }

    // ── list / status reads ──
    #[test]
    fn list_returns_the_typed_workload_roster_and_ignores_backend_inventory() {
        use mackes_mesh_types::workloads::WorkloadBackend;
        let runner = Arc::new(FakeRunner {
            roster: vec![instance("backend-bypass", "ACTIVE")],
            ..Default::default()
        });
        let (_temp, w) = projection_worker(
            runner,
            vec![
                workload_status(
                    "web",
                    WorkloadBackend::LibvirtVirtqemud,
                    WorkloadPowerState::Running,
                ),
                workload_status(
                    "db",
                    WorkloadBackend::LibvirtVirtqemud,
                    WorkloadPowerState::Stopped,
                ),
                workload_status(
                    "container",
                    WorkloadBackend::QuadletSystemd,
                    WorkloadPowerState::Running,
                ),
            ],
            u64::try_from(now_ms()).unwrap(),
            false,
        );
        for verb in ["list", "list-instances", "status"] {
            let reply = w.handle(verb, "{}");
            assert!(reply.ok, "{verb} ok");
            let instances = reply.instances.expect("roster");
            assert_eq!(instances.len(), 2);
            assert_eq!(instances[0].name, "web");
            assert_eq!(instances[0].status, "ACTIVE");
            assert_eq!(instances[1].name, "db");
            assert_eq!(instances[1].status, "SHUTOFF");
            assert!(instances.iter().all(|row| row.name != "backend-bypass"));
        }
    }

    #[test]
    fn cuttlefish_outer_observation_refuses_a_same_id_quadlet_row() {
        use mackes_mesh_types::workloads::WorkloadBackend;
        let status = workload_status(
            "android-t480",
            WorkloadBackend::QuadletSystemd,
            WorkloadPowerState::Running,
        );

        assert_eq!(
            CuttlefishOuterWorkloadObservation::from_status(&status),
            Err(verbs::CuttlefishProviderError::ProviderRejected),
            "a container row must not become Android outer-VM readiness"
        );
    }

    #[test]
    fn placement_local_list_returns_only_the_handling_workers_roster() {
        use mackes_mesh_types::workloads::WorkloadBackend;
        let runner = Arc::new(FakeRunner {
            roster: vec![instance("backend-bypass", "ACTIVE")],
            ..Default::default()
        });
        let (_temp, worker) = projection_worker(
            runner,
            vec![workload_status(
                "web",
                WorkloadBackend::LibvirtVirtqemud,
                WorkloadPowerState::Running,
            )],
            u64::try_from(now_ms()).unwrap(),
            false,
        );
        let reply = worker.handle("list-instances-local", r#"{"node":"me"}"#);
        assert!(reply.ok);
        assert_eq!(reply.instances.unwrap()[0].name, "web");
    }

    #[test]
    fn a_read_without_a_workload_projection_is_gated_not_faked() {
        let w = staged_worker(Arc::new(FakeRunner {
            roster: vec![instance("backend-bypass", "ACTIVE")],
            ..Default::default()
        }));
        let reply = w.handle("list", "{}");
        assert!(!reply.ok);
        assert!(reply.instances.is_none(), "no fabricated empty roster");
        assert!(reply.gated.unwrap().contains("runtime authority not ready"));
    }

    #[test]
    fn a_stale_workload_projection_cannot_fall_back_to_backend_inventory() {
        use mackes_mesh_types::workloads::WorkloadBackend;
        let runner = Arc::new(FakeRunner {
            roster: vec![instance("backend-bypass", "ACTIVE")],
            ..Default::default()
        });
        let now = u64::try_from(now_ms()).unwrap();
        let (_temp, worker) = projection_worker(
            runner,
            vec![workload_status(
                "stale-vm",
                WorkloadBackend::LibvirtVirtqemud,
                WorkloadPowerState::Running,
            )],
            now.saturating_sub(WORKLOAD_PROJECTION_MAX_AGE_MS + 1),
            false,
        );

        let reply = worker.handle("list", "{}");

        assert!(!reply.ok);
        assert!(reply.instances.is_none());
        assert!(reply
            .gated
            .as_deref()
            .is_some_and(|reason| reason.contains("stale or future-dated")));
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
        let _environment_lock = ANDROID_RELEASE_ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let now = u64::try_from(now_ms()).unwrap();
        let (_environment, host_probe) = install_android_release_fixture(tmp.path(), now);
        let runner = Arc::new(AndroidProvisionRunner::default());
        let w = CloudWorker::new("me".into(), "peer:me".into(), tmp.path().to_path_buf())
            .with_runner(runner.clone())
            .with_android_host_probe(host_probe)
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

    #[test]
    fn retired_instance_lifecycle_verbs_fail_closed_before_parsing_auth_or_backend() {
        let runner = Arc::new(FakeRunner::default());
        let w = staged_worker(runner.clone());
        for verb in [
            "destroy",
            "instance-start",
            "instance-stop",
            "instance-reboot",
            "instance-delete",
            "instance-start-all",
            "instance-stop-all",
            "instance-reboot-all",
        ] {
            assert_eq!(CloudVerb::from_verb(verb), None);
            let reply = w.handle(
                verb,
                r#"{"schema_version":99,"node":"elsewhere","instance":"web","armed_token":"executable-looking""#,
            );
            assert!(!reply.ok, "{verb} must fail closed");
            let error = reply.error.as_deref().expect("explicit retirement refusal");
            assert!(error.contains(verb), "{error}");
            assert!(error.contains("action/workload/operation"), "{error}");
            assert!(!error.contains("target-scoped"), "{error}");
            assert!(reply.gated.is_none());
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
        // Every recognized Workloads verb has a concrete handler or an explicit
        // retired-path refusal. None may surface the U2 "not yet wired" skeleton.
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
    fn build_state_reports_the_arming_capability_and_the_workload_roster_table() {
        use mackes_mesh_types::workloads::WorkloadBackend;
        let runner = Arc::new(FakeRunner {
            roster: vec![instance("backend-bypass", "ACTIVE")],
            tofu_up: true,
            ..Default::default()
        });
        // Arm-capable node ⇒ apply_armed capability true.
        let (_temp, w) = projection_worker(
            runner,
            vec![workload_status(
                "web",
                WorkloadBackend::LibvirtVirtqemud,
                WorkloadPowerState::Running,
            )],
            u64::try_from(now_ms()).unwrap(),
            true,
        );
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
        assert_eq!(state.resources[0].rows[0].id, "web");
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
        assert_eq!(
            cloud_bus_root_or_system(None),
            PathBuf::from(mde_bus::SYSTEM_BUS_ROOT)
        );
    }

    // ── the U5 drift tick folds workloads + rollup into the mirror ──
    #[test]
    fn a_drift_tick_folds_workload_rows_and_the_rollup_into_the_mirror() {
        use mackes_mesh_types::cloud::{DeliveryType, DriftFlag, WorkloadSpec};
        use mackes_mesh_types::workloads::WorkloadBackend;

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
        // A plan that reports pending changes ⇒ the workload is drifted.
        let runner = Arc::new(FakeRunner {
            roster: vec![instance("web", "ACTIVE")],
            plan_ndjson: Some(
                r#"{"type":"change_summary","changes":{"add":1,"change":0,"remove":0}}"#.into(),
            ),
            ..Default::default()
        });
        let (_temp, w) = projection_worker(
            runner,
            vec![workload_status(
                "web",
                WorkloadBackend::LibvirtVirtqemud,
                WorkloadPowerState::Running,
            )],
            u64::try_from(now_ms()).unwrap(),
            false,
        );
        super::reconcile::write_desired_doc(&w.state_root, &spec).unwrap();
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
        use mackes_mesh_types::workloads::{
            WorkloadBackend, WorkloadStateSnapshot, WORKLOAD_CONTRACT_SCHEMA_VERSION,
        };
        let tmp = tempfile::tempdir().unwrap();
        let bus = tmp.path().to_path_buf();
        let persist = Persist::open(bus.clone()).unwrap();
        let snapshot = WorkloadStateSnapshot {
            schema_version: WORKLOAD_CONTRACT_SCHEMA_VERSION,
            node: "f".to_string(),
            observed_at_ms: u64::try_from(now_ms()).unwrap(),
            workloads: vec![workload_status(
                "web",
                WorkloadBackend::LibvirtVirtqemud,
                WorkloadPowerState::Running,
            )],
        };
        persist
            .write(
                &workload_state_topic("f"),
                Priority::Default,
                None,
                Some(&serde_json::to_string(&snapshot).unwrap()),
            )
            .unwrap();
        let req = persist
            .write(
                "action/cloud/list-instances",
                Priority::Default,
                None,
                Some("{}"),
            )
            .unwrap();
        // Any node serves the read from its own typed projection — no placement gate.
        let w = CloudWorker::new("f".into(), "peer:f".into(), tmp.path().to_path_buf())
            .with_runner(Arc::new(FakeRunner {
                roster: vec![instance("backend-bypass", "ACTIVE")],
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

    fn armed_configure_body(node: &str) -> String {
        let base = format!(r#"{{"schema_version":1,"node":"{node}"}}"#);
        let token = valid_token(
            "configure",
            node,
            mackes_mesh_types::cloud::CLOUD_ARM_NODE_SCOPE,
            &base,
        );
        format!(r#"{{"schema_version":1,"node":"{node}","armed_token":"{token}"}}"#)
    }

    async fn wait_for_bus_message(root: &Path, topic: &str) -> mde_bus::persist::StoredMessage {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Ok(persist) = Persist::open(root.to_path_buf()) {
                    if let Ok(Some(message)) = persist.read_latest(topic) {
                        break message;
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("timed out waiting for Bus message")
    }

    #[tokio::test]
    async fn run_recovers_late_and_replaced_bus_without_replaying_retained_actions() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().join("bus");
        fs::write(&root, "blocking file").expect("block Bus root");
        let faults = Arc::new(CloudBusFaults::default());
        faults
            .fail_state_writes
            .store(1, std::sync::atomic::Ordering::SeqCst);
        let mut worker = CloudWorker::new("me".into(), "peer:me".into(), temp.path().join("state"))
            .with_runner(Arc::new(FakeRunner {
                roster: vec![instance("web", "ACTIVE")],
                ..Default::default()
            }))
            .with_bus_root(Some(root.clone()))
            .with_bus_faults(faults)
            .with_poll(Duration::from_millis(10))
            .with_drift_interval(Duration::from_secs(3_600));
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let task =
            tokio::spawn(
                async move { worker.run(ShutdownToken::from_receiver(shutdown_rx)).await },
            );

        let staged = temp.path().join("staged-bus");
        let staged_bus = Persist::open(staged.clone()).expect("staged Bus");
        let retained = staged_bus
            .write("action/cloud/status", Priority::Default, None, Some("{}"))
            .expect("retained action");
        drop(staged_bus);
        fs::remove_file(&root).expect("unblock Bus root");
        fs::rename(&staged, &root).expect("install late Bus");
        wait_for_bus_message(&root, &cloud_state_topic("me")).await;
        let bus = Persist::open(root.clone()).expect("late Bus");
        assert!(bus
            .read_latest(&reply_topic(&retained.ulid))
            .expect("retained reply read")
            .is_none());
        let forward = bus
            .write(
                "action/cloud/list-instances",
                Priority::Default,
                None,
                Some("{}"),
            )
            .expect("forward action");
        wait_for_bus_message(&root, &reply_topic(&forward.ulid)).await;
        drop(bus);

        let replacement = temp.path().join("replacement-bus");
        let replacement_bus = Persist::open(replacement.clone()).expect("replacement staging");
        let replacement_retained = replacement_bus
            .write("action/cloud/status", Priority::Default, None, Some("{}"))
            .expect("replacement retained action");
        drop(replacement_bus);
        fs::rename(&root, temp.path().join("retired-bus")).expect("retire Bus");
        fs::rename(&replacement, &root).expect("install replacement Bus");
        wait_for_bus_message(&root, &cloud_state_topic("me")).await;
        let bus = Persist::open(root.clone()).expect("replacement Bus");
        assert!(bus
            .read_latest(&reply_topic(&replacement_retained.ulid))
            .expect("replacement retained reply read")
            .is_none());
        let replacement_forward = bus
            .write(
                "action/cloud/list-instances",
                Priority::Default,
                None,
                Some("{}"),
            )
            .expect("replacement forward action");
        wait_for_bus_message(&root, &reply_topic(&replacement_forward.ulid)).await;

        shutdown_tx.send(true).expect("shutdown");
        task.await.expect("join").expect("worker");
    }

    #[test]
    fn activation_tail_prime_is_atomic_and_dynamic_first_action_executes_once() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().join("bus");
        let persist = Persist::open(root.clone()).expect("Bus");
        persist
            .write(
                "action/cloud/configure",
                Priority::Default,
                None,
                Some(&armed_configure_body("me")),
            )
            .expect("retained configure");
        persist
            .write("action/cloud/status", Priority::Default, None, Some("{}"))
            .expect("retained status");
        let faults = Arc::new(CloudBusFaults::default());
        *faults.fail_action_read_topic.lock().expect("fault lock") =
            Some("action/cloud/status".to_string());
        let worker = CloudWorker::new("me".into(), "peer:me".into(), temp.path().join("state"))
            .with_runner(Arc::new(FakeRunner::default()))
            .with_signer(Arc::new(signer()))
            .with_bus_root(Some(root.clone()))
            .with_bus_faults(faults.clone());
        let mut cursors = HashMap::from([("sentinel".to_string(), "cursor".to_string())]);
        worker.prime_cursors(&mut cursors);
        assert_eq!(
            cursors,
            HashMap::from([("sentinel".to_string(), "cursor".to_string())]),
            "partial activation cannot replace the prior cursor set"
        );
        *faults.fail_action_read_topic.lock().expect("fault lock") = None;
        worker.prime_cursors(&mut cursors);
        assert!(!worker.drain_actions(&mut cursors));

        let first = persist
            .write(
                "action/cloud/list-instances",
                Priority::Default,
                None,
                Some("{}"),
            )
            .expect("dynamic first action");
        assert!(worker.drain_actions(&mut cursors));
        assert!(!worker.drain_actions(&mut cursors));
        assert_eq!(
            persist
                .list_since(&reply_topic(&first.ulid), None)
                .expect("first replies")
                .len(),
            1
        );
        let second = persist
            .write(
                "action/cloud/list-instances",
                Priority::Default,
                None,
                Some("{}"),
            )
            .expect("second action");
        assert!(worker.drain_actions(&mut cursors));
        assert_eq!(
            persist
                .list_since(&reply_topic(&second.ulid), None)
                .expect("second replies")
                .len(),
            1
        );
    }

    #[test]
    fn final_lane_and_reachability_read_failures_defer_all_backend_effects() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().join("bus");
        let persist = Persist::open(root.clone()).expect("Bus");
        persist
            .write(
                "action/cloud/configure",
                Priority::Default,
                None,
                Some(&armed_configure_body("me")),
            )
            .expect("local mutation");
        persist
            .write("action/cloud/status", Priority::Default, None, Some("{}"))
            .expect("final lane");
        let faults = Arc::new(CloudBusFaults::default());
        *faults.fail_action_read_topic.lock().expect("fault lock") =
            Some("action/cloud/status".to_string());
        let runner = Arc::new(FakeRunner::default());
        let worker = CloudWorker::new("me".into(), "peer:me".into(), temp.path().join("state"))
            .with_runner(runner.clone())
            .with_signer(Arc::new(signer()))
            .with_bus_root(Some(root.clone()))
            .with_bus_faults(faults.clone());
        let mut cursors = HashMap::new();
        assert!(!worker.drain_actions(&mut cursors));
        assert!(cursors.is_empty());
        assert!(runner.calls.lock().expect("calls").is_empty());

        *faults.fail_action_read_topic.lock().expect("fault lock") = None;
        persist
            .write(
                &cloud_state_topic("ghost"),
                Priority::Default,
                None,
                Some("not-json"),
            )
            .expect("malformed reachability mirror");
        persist
            .write(
                "action/cloud/set-desired",
                Priority::Default,
                None,
                Some(r#"{"schema_version":1,"node":"ghost"}"#),
            )
            .expect("remote mutation");
        assert!(!worker.drain_actions(&mut cursors));
        assert!(cursors.is_empty());
        assert!(runner.calls.lock().expect("calls").is_empty());
    }

    #[test]
    fn reply_failure_recovers_durable_mutation_without_repeating_effect() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().join("bus");
        let persist = Persist::open(root.clone()).expect("Bus");
        let request = persist
            .write(
                "action/cloud/configure",
                Priority::Default,
                None,
                Some(&armed_configure_body("me")),
            )
            .expect("mutation");
        let faults = Arc::new(CloudBusFaults::default());
        faults
            .fail_reply_writes
            .store(1, std::sync::atomic::Ordering::SeqCst);
        let runner = Arc::new(FakeRunner::default());
        let worker = CloudWorker::new("me".into(), "peer:me".into(), temp.path().join("state"))
            .with_runner(runner.clone())
            .with_signer(Arc::new(signer()))
            .with_bus_root(Some(root.clone()))
            .with_bus_faults(faults);
        let mut cursors = HashMap::new();
        assert!(!worker.drain_actions(&mut cursors));
        assert_eq!(runner.calls.lock().expect("calls").len(), 1);
        assert!(!cursors.contains_key("action/cloud/configure"));
        assert!(persist
            .read_latest(&reply_topic(&request.ulid))
            .expect("reply read")
            .is_none());

        let restarted = CloudWorker::new("me".into(), "peer:me".into(), temp.path().join("state"))
            .with_runner(runner.clone())
            .with_signer(Arc::new(signer()))
            .with_bus_root(Some(root.clone()));
        let (reopened, identity) = restarted.open_bus().expect("open").expect("enabled");
        let mut activation = restarted
            .activate_bus(&reopened, identity)
            .expect("restart activation");
        assert_eq!(activation.pending_transactions.len(), 1);
        restarted
            .recover_pending_transactions(&reopened, &mut activation.pending_transactions)
            .expect("outbox recovery");
        assert!(activation.pending_transactions.is_empty());
        assert_eq!(runner.calls.lock().expect("calls").len(), 1);
        assert_eq!(
            persist
                .list_since(&reply_topic(&request.ulid), None)
                .expect("recovered replies")
                .len(),
            1
        );

        let malformed = persist
            .write("action/cloud/list-instances", Priority::Default, None, None)
            .expect("malformed request");
        let retry_faults = Arc::new(CloudBusFaults::default());
        retry_faults
            .fail_reply_writes
            .store(1, std::sync::atomic::Ordering::SeqCst);
        let retrying = CloudWorker::new(
            "me".into(),
            "peer:me".into(),
            temp.path().join("retry-state"),
        )
        .with_runner(runner.clone())
        .with_bus_root(Some(root.clone()))
        .with_bus_faults(retry_faults);
        assert!(!retrying
            .drain_actions_on(&reopened, &mut activation.cursors)
            .unwrap_or(false));
        assert!(!activation
            .cursors
            .contains_key("action/cloud/list-instances"));
        assert!(retrying
            .drain_actions_on(&reopened, &mut activation.cursors)
            .expect("malformed reply retry"));
        assert_eq!(
            persist
                .list_since(&reply_topic(&malformed.ulid), None)
                .expect("malformed replies")
                .len(),
            1
        );
        assert_eq!(runner.calls.lock().expect("calls").len(), 1);
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
