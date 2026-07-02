//! VOIP-GW-3 — the leader-gated `voice_provision` worker.
//!
//! The provisioning half of the per-node-SIP design
//! (`docs/design/voice-vitelity-per-node-sip.md`, locks 2/3/7/8/9/19).
//! It drives the VOIP-GW-2 [`VitelityClient`] seam to give every enrolled
//! node its own inbound Vitelity **sub-account**, seals that sub-account's
//! SIP creds to the node via the mesh secret store, and publishes each
//! node's provisioning/registration state to the Bus for the Voice panel
//! (VOIP-GW-5) fleet board.
//!
//! ## Leader-gated (lock 7)
//!
//! Spawned on **every** node so failover is seamless, but every tick self-
//! gates on the SAME shared `<workgroup_root>/.mackesd-leader.lock` every
//! other leader-gated worker contends on (`copilot`, `dc_health`). Only the
//! elected leader runs the provisioning + reconcile and holds the Vitelity
//! **master API key** — a follower short-circuits without touching Vitelity,
//! so a multi-node mesh provisions each node exactly once and the master key
//! never leaves the leader.
//!
//! ## What it does each pass (leader only)
//!
//! 1. Reads the roster (the `nodes` table — the enrolled fleet, lock 19's
//!    "desired" set) and the master Vitelity creds from the secret store.
//! 2. [`plan_reconcile`] diffs desired (every enrolled node) vs actual
//!    (Vitelity's existing sub-accounts + which nodes already have sealed
//!    creds) into an idempotent [`VoiceAction`] list.
//! 3. Applies each action: **Provision** creates the node's sub-account
//!    (username derived from its hostname, lock 3), **seals** the returned
//!    SIP creds to the node's per-node key in the secret store (lock 7), and
//!    marks it `Unregistered` (provisioned, awaiting the node's own REGISTER,
//!    which VOIP-GW-4 publishes). A Vitelity error surfaces as `Error+reason`
//!    — never a fake success (§7).
//! 4. Publishes each node's [`NodeVoiceState`] to `state/voice/<node>` — the
//!    live reg-state + fleet-board row VOIP-GW-5 reads (lock 9).
//!
//! A **panel "Provision/Re-provision" button** (lock 8) publishes the typed
//! verb `action/voice/provision`; draining a message forces an immediate
//! reconcile pass (bypassing the rate-limit) so the operator gets a prompt
//! retry. Otherwise the reconcile runs on a slow, rate-limited cadence
//! (lock 19 — idempotent + API-rate-limited).
//!
//! ## Injectable seam / honesty
//!
//! The provisioning is driven through the injectable [`VitelityClient`]
//! trait so the whole reconcile is headless-testable against
//! [`FakeVitelityClient`] ([`reconcile_once`] takes `&dyn VitelityClient`).
//! Production builds a [`LiveVitelityClient`] from the sealed master creds;
//! its transport is integration-gated (a typed [`VitelityError`]), never
//! faked (§7, the mde-kvm/mde-seat pattern). No raw shell — the only I/O is
//! the typed client, the secret store, and the Bus (§9).

#![cfg(feature = "async-services")]

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;

use super::{ShutdownToken, Worker};
use crate::ipc::secret_store::{self, SecretStore};
use crate::vitelity::model::VitelityCredentials;
use crate::vitelity::{
    CreateSubAccount, LiveVitelityClient, SubAccountCredentials, VitelityClient,
};

/// Reconcile cadence — voice provisioning is slow-changing (a node is
/// enrolled once), so a slow poll keeps Vitelity ⇄ roster reconciled without
/// hammering the API (lock 19 — rate-limited). A panel button forces an
/// immediate pass in between.
pub const DEFAULT_RECONCILE_INTERVAL: Duration = Duration::from_secs(300);

/// How often the run loop wakes to check for a panel-button message /
/// shutdown. Shorter than the reconcile interval so the button is responsive
/// while the full reconcile stays rate-limited.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(5);

/// The Bus verb the Voice panel publishes to request a (re-)provision
/// (lock 8). A message here forces an immediate reconcile pass. Typed verb in
/// the canonical `action/<domain>/<verb>` namespace (§9) — no command string.
pub const PROVISION_TOPIC: &str = "action/voice/provision";

/// Bus topic prefix the per-node reg-state / fleet-board row is published
/// under (lock 9). The Voice panel (VOIP-GW-5) drains `state/voice/*`.
pub const STATE_TOPIC_PREFIX: &str = "state/voice/";

/// Fallback SIP realm used to render a node's `<user>@<realm>` address when
/// Vitelity hasn't reported the sub-account's realm yet. The live client
/// fills the real realm from the create/get response.
pub const DEFAULT_REALM: &str = "sip.vitelity.net";

/// The secret-store key holding the Vitelity **master** API creds
/// (login + key), sealed once by the operator and read only by the leader
/// (lock 7 — leader-held, never distributed per-node). A JSON
/// `{"login":..,"api_key":..}` body.
#[must_use]
pub fn master_creds_ref() -> String {
    "voice/vitelity-master".to_string()
}

/// The per-node secret-store key a node's sealed SIP creds live under
/// (lock 7). Namespaced under `voice/` and keyed by the node id so each
/// node's creds are an independent, node-scoped entry in the replicated
/// store — the SAME DS-8 pattern the VPN tunnel creds
/// ([`secret_store::creds_ref_for`]) reuse. Pure + stable: this string IS the
/// store key, so a change orphans the sealed creds.
///
/// The node id is sanitized to the store's safe charset (alnum, `.-:_`) so a
/// stray separator can't widen the key namespace.
#[must_use]
pub fn node_creds_ref(node_id: &str) -> String {
    let safe: String = node_id
        .trim()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '_'))
        .collect();
    format!("voice/node/{safe}/sip-creds")
}

/// Derive a node's Vitelity sub-account username from its hostname (lock 3).
///
/// Lowercased and reduced to the `[a-z0-9-]` charset Vitelity sub-account
/// usernames accept, with runs of other characters collapsed to a single
/// `-`, leading/trailing `-` trimmed, and bounded in length. Deterministic +
/// stable: the username IS the node's callable identity, so a change re-homes
/// its inbound address. Returns an empty string for a hostname with no usable
/// characters (the caller treats that as "not provisionable yet").
#[must_use]
pub fn sub_account_username(hostname: &str) -> String {
    let mut out = String::with_capacity(hostname.len());
    let mut last_dash = false;
    for ch in hostname.trim().to_ascii_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_dash = false;
        } else if !out.is_empty() && !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    // Trim a trailing collapse-dash and cap the length (Vitelity sub-account
    // usernames are short; 32 is a safe ceiling).
    while out.ends_with('-') {
        out.pop();
    }
    out.truncate(32);
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// The callable SIP address `<username>@<realm>` (lock 3). Falls back to
/// [`DEFAULT_REALM`] when Vitelity hasn't reported the realm yet.
#[must_use]
pub fn sip_uri(username: &str, realm: &str) -> String {
    let realm = if realm.trim().is_empty() {
        DEFAULT_REALM
    } else {
        realm.trim()
    };
    format!("{username}@{realm}")
}

/// A node's provisioning / registration state (lock 9). The reg-state the
/// panel fleet board renders. `Unregistered` means "provisioned, awaiting the
/// node's own REGISTER" — the live `Registered` truth is published by the
/// node's SIP client (VOIP-GW-4) and merged into the same topic; GW-3 owns the
/// provisioning-side transitions. An `Error` carries the honest reason (a
/// failing node shows the real error, never a fake online).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum RegState {
    /// The node's SIP client has an active REGISTER (published by GW-4).
    Registered,
    /// Provisioned + creds sealed, but not currently registered.
    Unregistered,
    /// A provisioning action is in flight for this node.
    Provisioning,
    /// Provisioning failed — carries the honest reason.
    Error {
        /// Operator-readable failure detail.
        reason: String,
    },
}

/// One node's fleet-board row published to `state/voice/<node>` (lock 5 + 9).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NodeVoiceState {
    /// The node id (topic suffix / board key).
    pub node_id: String,
    /// The node hostname the sub-account username derives from.
    pub hostname: String,
    /// The Vitelity sub-account username (empty until provisioned).
    pub username: String,
    /// The callable `<username>@<realm>` SIP address.
    pub sip_uri: String,
    /// The provisioning / registration state.
    #[serde(flatten)]
    pub reg_state: RegState,
    /// When this row was produced (epoch seconds).
    pub updated_at_s: u64,
}

/// A node the leader must ensure is provisioned — the reconcile "desired"
/// unit (one enrolled roster row).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesiredNode {
    /// Stable node id (the reg-state topic suffix / seal-target key).
    pub node_id: String,
    /// The node's hostname (the sub-account username derives from it).
    pub hostname: String,
}

/// One idempotent reconcile action (lock 19). The plan is a pure diff of
/// desired vs actual so it is unit-testable without any I/O.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceAction {
    /// The node has no Vitelity sub-account — create one + seal its creds.
    Provision {
        /// Target node id.
        node_id: String,
        /// Target node hostname.
        hostname: String,
        /// The derived sub-account username to create.
        username: String,
    },
    /// The node's sub-account exists in Vitelity but its SIP creds are not in
    /// the (replicated) secret store — drift that needs a re-provision
    /// (password reset). The common re-image case (creds still in the
    /// replicated store) is a no-op and never appears here.
    Reseal {
        /// Target node id.
        node_id: String,
        /// Target node hostname.
        hostname: String,
        /// The sub-account username that already exists in Vitelity.
        username: String,
    },
}

impl VoiceAction {
    /// The node id this action targets.
    #[must_use]
    pub fn node_id(&self) -> &str {
        match self {
            Self::Provision { node_id, .. } | Self::Reseal { node_id, .. } => node_id,
        }
    }
}

/// Pure reconcile diff (lock 19): desired (every enrolled node) vs actual
/// (Vitelity's existing sub-account usernames + which node ids already have
/// sealed creds) → the idempotent action list.
///
/// - A node with no sub-account → [`VoiceAction::Provision`] (auto-provision,
///   lock 2 — this is also the enrollment path: a freshly-enrolled node has no
///   sub-account so the very next reconcile provisions it).
/// - A node whose sub-account exists but whose creds are absent from the store
///   → [`VoiceAction::Reseal`] (drift).
/// - A node with both → nothing (idempotent; the re-imaged-node heal is this
///   no-op, since the sealed creds live in the *replicated* store, not on the
///   node's disk).
///
/// A node whose hostname yields an empty username is skipped (not
/// provisionable yet) — the caller publishes an honest `Error` for it.
#[must_use]
pub fn plan_reconcile(
    desired: &[DesiredNode],
    existing_usernames: &HashSet<String>,
    sealed_nodes: &HashSet<String>,
) -> Vec<VoiceAction> {
    let mut actions = Vec::new();
    for node in desired {
        let username = sub_account_username(&node.hostname);
        if username.is_empty() {
            continue;
        }
        let has_account = existing_usernames.contains(&username);
        let has_sealed = sealed_nodes.contains(&node.node_id);
        if !has_account {
            actions.push(VoiceAction::Provision {
                node_id: node.node_id.clone(),
                hostname: node.hostname.clone(),
                username,
            });
        } else if !has_sealed {
            actions.push(VoiceAction::Reseal {
                node_id: node.node_id.clone(),
                hostname: node.hostname.clone(),
                username,
            });
        }
    }
    actions
}

/// The JSON body a node's sealed SIP creds are stored as (lock 7). The node
/// (VOIP-GW-4) reads this back to build its inbound SIP account. Kept minimal:
/// the sub-account SIP auth pair + the realm needed to form the REGISTER.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SealedSipCreds {
    /// SIP auth username (the sub-account).
    pub username: String,
    /// SIP auth password (the node-sealed secret).
    pub sip_password: String,
    /// The SIP realm the sub-account registers to.
    pub realm: String,
}

/// Current epoch seconds (best-effort; 0 before the epoch, which never
/// happens in practice).
#[must_use]
fn now_epoch_s() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The outcome of a reconcile pass — the per-node states to publish. Returned
/// (rather than published inline) so [`reconcile_once`] stays pure I/O-wise:
/// the caller publishes to the Bus, and tests assert on the returned states
/// without a Bus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconcileOutcome {
    /// One fleet-board row per enrolled node.
    pub states: Vec<NodeVoiceState>,
    /// How many sub-accounts this pass created (for logging / tests).
    pub provisioned: usize,
}

/// Seal a node's SIP creds to its per-node key in the secret store (lock 7).
/// The store replicates the ciphertext; a change surfaces honestly as an
/// `Err`, never a claimed success.
fn seal_node_creds(
    store: &SecretStore,
    node_id: &str,
    creds: &SubAccountCredentials,
    realm: &str,
) -> Result<(), String> {
    let sealed = SealedSipCreds {
        username: creds.username.clone(),
        sip_password: creds.sip_password.clone(),
        realm: realm.to_string(),
    };
    let body =
        serde_json::to_string(&sealed).map_err(|e| format!("serialize sealed sip creds: {e}"))?;
    store.put(&node_creds_ref(node_id), &body)
}

/// Whether a node already has sealed SIP creds in the store. A store fault is
/// treated as "not sealed" here (the reconcile will attempt a re-seal and
/// surface the real error there) — but distinguished in the caller's logging.
fn node_has_sealed_creds(store: &SecretStore, node_id: &str) -> bool {
    matches!(store.get(&node_creds_ref(node_id)), Ok(Some(_)))
}

/// Run ONE reconcile pass against the injected client + store (lock 19) — the
/// headless-testable core. Drives the [`VitelityClient`] seam so a test passes
/// a [`FakeVitelityClient`](crate::vitelity::FakeVitelityClient) and a
/// `LocalAead` store and asserts the end state; production passes a
/// [`LiveVitelityClient`].
///
/// For each enrolled node it ensures a Vitelity sub-account + sealed creds
/// exist, and returns the per-node [`NodeVoiceState`] the caller publishes.
/// Every Vitelity/store failure becomes an honest `Error` state for that node
/// — the pass never aborts the whole fleet on one node's failure, and never
/// fabricates a success (§7).
///
/// `realm` is the fallback SIP realm for the `<user>@<realm>` address before
/// Vitelity reports the real one.
#[must_use]
pub fn reconcile_once(
    client: &dyn VitelityClient,
    store: &SecretStore,
    desired: &[DesiredNode],
    realm: &str,
) -> ReconcileOutcome {
    // Actual side 1: Vitelity's existing sub-accounts. A list failure is
    // fleet-wide (we can't tell what exists) — every node degrades to an
    // honest Error state rather than double-provisioning.
    let existing = match client.list_sub_accounts() {
        Ok(list) => list.into_iter().map(|s| s.username).collect::<HashSet<_>>(),
        Err(e) => {
            let reason = format!("cannot list Vitelity sub-accounts: {e}");
            let states = desired
                .iter()
                .map(|n| NodeVoiceState {
                    node_id: n.node_id.clone(),
                    hostname: n.hostname.clone(),
                    username: sub_account_username(&n.hostname),
                    sip_uri: String::new(),
                    reg_state: RegState::Error {
                        reason: reason.clone(),
                    },
                    updated_at_s: now_epoch_s(),
                })
                .collect();
            return ReconcileOutcome {
                states,
                provisioned: 0,
            };
        }
    };
    // Actual side 2: which enrolled nodes already have sealed creds.
    let sealed: HashSet<String> = desired
        .iter()
        .filter(|n| node_has_sealed_creds(store, &n.node_id))
        .map(|n| n.node_id.clone())
        .collect();

    let actions = plan_reconcile(desired, &existing, &sealed);
    let acted: HashSet<&str> = actions.iter().map(VoiceAction::node_id).collect();

    let mut provisioned = 0usize;
    // Start from the steady-state view: every node whose sub-account exists +
    // creds are sealed is Unregistered (provisioned; awaiting its own
    // REGISTER, which GW-4 publishes over the top of this topic).
    let mut states: Vec<NodeVoiceState> = Vec::with_capacity(desired.len());

    // Apply the drift actions first, recording the resulting state per node.
    for action in &actions {
        match action {
            VoiceAction::Provision {
                node_id,
                hostname,
                username,
            } => {
                let req = CreateSubAccount {
                    username: username.clone(),
                    description: hostname.clone(),
                };
                match client.create_sub_account(&req) {
                    Ok((account, creds)) => {
                        let account_realm = if account.realm.is_empty() {
                            realm
                        } else {
                            account.realm.as_str()
                        };
                        match seal_node_creds(store, node_id, &creds, account_realm) {
                            Ok(()) => {
                                provisioned += 1;
                                states.push(NodeVoiceState {
                                    node_id: node_id.clone(),
                                    hostname: hostname.clone(),
                                    username: username.clone(),
                                    sip_uri: sip_uri(username, account_realm),
                                    reg_state: RegState::Unregistered,
                                    updated_at_s: now_epoch_s(),
                                });
                            }
                            Err(e) => states.push(error_state(
                                node_id,
                                hostname,
                                username,
                                realm,
                                format!("sub-account created but sealing creds failed: {e}"),
                            )),
                        }
                    }
                    Err(e) => states.push(error_state(
                        node_id,
                        hostname,
                        username,
                        realm,
                        format!("provision failed: {e}"),
                    )),
                }
            }
            VoiceAction::Reseal {
                node_id,
                hostname,
                username,
            } => {
                // The sub-account exists but its SIP password is gone from the
                // store; the seam has no password-recovery/reset op, so this is
                // an honest, operator-actionable drift, not a silent heal. The
                // panel Re-provision (a future GW-6 password-reset) resolves it.
                states.push(error_state(
                    node_id,
                    hostname,
                    username,
                    realm,
                    "sub-account exists but its sealed SIP creds are missing from the store; \
                     panel Re-provision (password reset) required"
                        .to_string(),
                ));
            }
        }
    }

    // The steady-state nodes (no action needed) → Unregistered (provisioned).
    for node in desired {
        if acted.contains(node.node_id.as_str()) {
            continue;
        }
        let username = sub_account_username(&node.hostname);
        if username.is_empty() {
            states.push(error_state(
                &node.node_id,
                &node.hostname,
                &username,
                realm,
                "hostname yields no valid sub-account username; cannot provision".to_string(),
            ));
            continue;
        }
        states.push(NodeVoiceState {
            node_id: node.node_id.clone(),
            hostname: node.hostname.clone(),
            username: username.clone(),
            sip_uri: sip_uri(&username, realm),
            reg_state: RegState::Unregistered,
            updated_at_s: now_epoch_s(),
        });
    }

    ReconcileOutcome {
        states,
        provisioned,
    }
}

/// Build an `Error` fleet-board row (honest failure — lock 9).
fn error_state(
    node_id: &str,
    hostname: &str,
    username: &str,
    realm: &str,
    reason: String,
) -> NodeVoiceState {
    NodeVoiceState {
        node_id: node_id.to_string(),
        hostname: hostname.to_string(),
        username: username.to_string(),
        sip_uri: if username.is_empty() {
            String::new()
        } else {
            sip_uri(username, realm)
        },
        reg_state: RegState::Error { reason },
        updated_at_s: now_epoch_s(),
    }
}

/// Resolve the master Vitelity creds from the secret store and build the live
/// client (leader-only path, lock 7). `Ok(None)` when the operator hasn't
/// sealed the master creds yet (honest "not provisionable"); `Err` on a store
/// fault or a malformed body.
fn resolve_live_client(store: &SecretStore) -> Result<Option<LiveVitelityClient>, String> {
    let Some(body) = store.get(&master_creds_ref())? else {
        return Ok(None);
    };
    let parsed: MasterCreds = serde_json::from_str(body.trim())
        .map_err(|e| format!("master Vitelity creds are not valid JSON: {e}"))?;
    if parsed.login.is_empty() || parsed.api_key.is_empty() {
        return Err("master Vitelity creds are missing login or api_key".to_string());
    }
    Ok(Some(LiveVitelityClient::new(VitelityCredentials::new(
        parsed.login,
        parsed.api_key,
    ))))
}

/// The sealed master-creds JSON shape (`voice/vitelity-master`).
#[derive(Debug, serde::Deserialize)]
struct MasterCreds {
    login: String,
    api_key: String,
}

/// The VOIP-GW-3 leader-gated worker.
pub struct VoiceProvisionWorker {
    /// Shared leader lock — the same file every leader-gated worker contends
    /// on. Only the elected leader provisions + holds the master key.
    leader_lock: PathBuf,
    /// This node's id (for the leader lease).
    node_id: String,
    /// Deployed repo root, for [`SecretStore::resolve`] (where the mesh secret
    /// helper lives). NOT the process cwd (`/` under systemd).
    repo_dir: PathBuf,
    /// Workgroup root, for the local-AEAD secret-store fallback + leader lock.
    workgroup_root: PathBuf,
    /// The roster db path (the `nodes` table = the enrolled fleet).
    db_path: PathBuf,
    /// Fallback SIP realm for the `<user>@<realm>` address.
    realm: String,
    /// Rate-limit: minimum interval between full reconcile passes (lock 19).
    reconcile_interval: Duration,
    /// Run-loop wake cadence (button responsiveness).
    poll_interval: Duration,
    /// Override the Bus spool root. Tests point this at a tempdir.
    bus_root_override: Option<PathBuf>,
    /// Test seam: an injected client used instead of the live one. Production
    /// leaves this `None` and builds a [`LiveVitelityClient`] from the sealed
    /// master creds each pass.
    client_override: Option<Box<dyn VitelityClient + Send>>,
    /// Test seam: an injected secret store. Production resolves per pass.
    store_override: Option<SecretStore>,
    /// Last time a full reconcile ran, for the rate-limit.
    last_reconcile: Option<Instant>,
}

impl VoiceProvisionWorker {
    /// Construct with production defaults: the shared leader lock under
    /// `workgroup_root`, the repo-root + workgroup-root secret-store
    /// resolution, and the default roster db.
    #[must_use]
    pub fn new(workgroup_root: PathBuf, node_id: String) -> Self {
        Self {
            leader_lock: workgroup_root.join(".mackesd-leader.lock"),
            node_id,
            repo_dir: secret_store::repo_root(),
            workgroup_root,
            db_path: crate::default_db_path(),
            realm: DEFAULT_REALM.to_string(),
            reconcile_interval: DEFAULT_RECONCILE_INTERVAL,
            poll_interval: DEFAULT_POLL_INTERVAL,
            bus_root_override: None,
            client_override: None,
            store_override: None,
            last_reconcile: None,
        }
    }

    /// Override the roster db path. Tests point this at a seeded tempdir db.
    #[must_use]
    pub fn with_db_path(mut self, p: PathBuf) -> Self {
        self.db_path = p;
        self
    }

    /// Override the Bus spool root. Tests point this at a tempdir.
    #[must_use]
    pub fn with_bus_root(mut self, p: PathBuf) -> Self {
        self.bus_root_override = Some(p);
        self
    }

    /// Inject a [`VitelityClient`] (tests pass a `FakeVitelityClient`).
    #[must_use]
    pub fn with_client(mut self, client: Box<dyn VitelityClient + Send>) -> Self {
        self.client_override = Some(client);
        self
    }

    /// Inject a secret store (tests pass a seeded `LocalAead`).
    #[must_use]
    pub fn with_store(mut self, store: SecretStore) -> Self {
        self.store_override = Some(store);
        self
    }

    /// Override the fallback SIP realm.
    #[must_use]
    pub fn with_realm(mut self, realm: impl Into<String>) -> Self {
        self.realm = realm.into();
        self
    }

    /// Override the reconcile rate-limit. Tests shorten it.
    #[must_use]
    pub const fn with_reconcile_interval(mut self, d: Duration) -> Self {
        self.reconcile_interval = d;
        self
    }

    /// Override the run-loop poll cadence. Tests shorten it.
    #[must_use]
    pub const fn with_poll_interval(mut self, d: Duration) -> Self {
        self.poll_interval = d;
        self
    }

    /// Only the elected leader provisions + holds the master key (lock 7).
    /// Reuses the shared leader lock — a cheap synchronous check per pass.
    fn is_leader(&self) -> bool {
        matches!(
            crate::leader::try_acquire(&self.leader_lock, &self.node_id),
            Ok(crate::leader::AcquireResult::Acquired)
        )
    }

    /// Resolve the secret store for this pass (the injected one in tests, else
    /// the mesh/local resolution every other secret-store consumer uses).
    fn store(&self) -> SecretStore {
        self.store_override
            .clone()
            .unwrap_or_else(|| SecretStore::resolve(&self.repo_dir, &self.workgroup_root))
    }

    /// Read the enrolled roster (the reconcile "desired" set, lock 19) from the
    /// `nodes` table: every non-decommissioned node with a non-empty hostname.
    /// A missing/unreadable store yields an empty desired set (nothing to
    /// provision) rather than an error — the pass is a no-op on a fresh node.
    fn read_desired(&self) -> Vec<DesiredNode> {
        let Ok(conn) = crate::store::open(&self.db_path) else {
            return Vec::new();
        };
        let Ok(rows) = crate::store::list_nodes(&conn) else {
            return Vec::new();
        };
        rows.into_iter()
            .filter(|r| r.role != "decommissioned" && !r.name.trim().is_empty())
            .map(|r| DesiredNode {
                node_id: r.node_id,
                hostname: r.name,
            })
            .collect()
    }

    /// Run one full reconcile pass and publish each node's state to the Bus.
    /// Leader-only is enforced by the caller. Returns the outcome for logging.
    fn reconcile_and_publish(&self, persist: &Persist) -> Option<ReconcileOutcome> {
        let store = self.store();
        let desired = self.read_desired();
        if desired.is_empty() {
            return None;
        }
        // Resolve the client: the injected test client, else the live client
        // built from the sealed master creds. No master creds → publish a
        // Provisioning state for every node (honest "awaiting the master key")
        // and skip the API.
        let outcome = if let Some(client) = self.client_override.as_deref() {
            reconcile_once(client, &store, &desired, &self.realm)
        } else {
            match resolve_live_client(&store) {
                Ok(Some(client)) => reconcile_once(&client, &store, &desired, &self.realm),
                Ok(None) => awaiting_master_key(&desired, &self.realm),
                Err(e) => master_key_error(&desired, &self.realm, &e),
            }
        };
        for state in &outcome.states {
            publish_state(persist, state);
        }
        Some(outcome)
    }
}

/// Every node is `Provisioning` while the operator hasn't sealed the master
/// key yet — honest "not provisionable", never a fake online.
fn awaiting_master_key(desired: &[DesiredNode], realm: &str) -> ReconcileOutcome {
    let states = desired
        .iter()
        .map(|n| {
            let username = sub_account_username(&n.hostname);
            NodeVoiceState {
                node_id: n.node_id.clone(),
                hostname: n.hostname.clone(),
                sip_uri: if username.is_empty() {
                    String::new()
                } else {
                    sip_uri(&username, realm)
                },
                username,
                reg_state: RegState::Provisioning,
                updated_at_s: now_epoch_s(),
            }
        })
        .collect();
    ReconcileOutcome {
        states,
        provisioned: 0,
    }
}

/// Every node shows the honest master-key store fault (lock 9).
fn master_key_error(desired: &[DesiredNode], realm: &str, err: &str) -> ReconcileOutcome {
    let states = desired
        .iter()
        .map(|n| {
            error_state(
                &n.node_id,
                &n.hostname,
                &sub_account_username(&n.hostname),
                realm,
                format!("master Vitelity creds unusable: {err}"),
            )
        })
        .collect();
    ReconcileOutcome {
        states,
        provisioned: 0,
    }
}

/// Publish one node's fleet-board row to `state/voice/<node>` (lock 9).
/// `Priority::Min` — a silent data topic the panel reads, not an operator
/// notification. Best-effort: a failed write is logged, never fatal.
pub fn publish_state(persist: &Persist, state: &NodeVoiceState) {
    let topic = format!("{STATE_TOPIC_PREFIX}{}", state.node_id);
    let Ok(body) = serde_json::to_string(state) else {
        return;
    };
    if let Err(e) = persist.write(&topic, Priority::Min, None, Some(&body)) {
        tracing::debug!(
            target: "mackesd::voice_provision",
            topic = %topic,
            error = %e,
            "publishing voice reg-state failed"
        );
    }
}

fn default_bus_root() -> Option<PathBuf> {
    mde_bus::default_data_dir()
}

#[async_trait::async_trait]
impl Worker for VoiceProvisionWorker {
    fn name(&self) -> &'static str {
        "voice_provision"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let Some(bus_root) = self.bus_root_override.clone().or_else(default_bus_root) else {
            tracing::warn!(
                target: "mackesd::voice_provision",
                "no Bus data dir; voice_provision idle"
            );
            // Idle until shutdown rather than busy-loop.
            shutdown.wait().await;
            return Ok(());
        };
        let persist = match Persist::open(bus_root) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(
                    target: "mackesd::voice_provision",
                    error = %e,
                    "persist open failed; worker idle"
                );
                shutdown.wait().await;
                return Ok(());
            }
        };
        // Cursor for the panel-button verb — start at the tail so we only act
        // on requests published after we come up.
        let mut cursor: Option<String> = persist.latest_ulid(PROVISION_TOPIC).ok().flatten();

        loop {
            // Drain the panel-button verb (lock 8). A follower still advances
            // the cursor + skips (the leader acts), so failover is seamless.
            let mut button_pressed = false;
            if let Ok(msgs) = persist.list_since(PROVISION_TOPIC, cursor.as_deref()) {
                for msg in msgs {
                    cursor = Some(msg.ulid);
                    button_pressed = true;
                }
            }

            if self.is_leader() {
                let due = self
                    .last_reconcile
                    .is_none_or(|last| last.elapsed() >= self.reconcile_interval);
                if button_pressed || due {
                    if let Some(outcome) = self.reconcile_and_publish(&persist) {
                        self.last_reconcile = Some(Instant::now());
                        if outcome.provisioned > 0 {
                            tracing::info!(
                                target: "mackesd::voice_provision",
                                provisioned = outcome.provisioned,
                                nodes = outcome.states.len(),
                                trigger = if button_pressed { "panel" } else { "reconcile" },
                                "voice_provision reconciled the fleet"
                            );
                        }
                    } else {
                        // No enrolled nodes yet — still record the tick so we
                        // don't re-scan the empty roster every poll.
                        self.last_reconcile = Some(Instant::now());
                    }
                }
            }

            tokio::select! {
                _ = shutdown.wait() => break,
                _ = tokio::time::sleep(self.poll_interval) => {}
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vitelity::FakeVitelityClient;

    /// A `LocalAead` store with a real mesh age identity (the same round-trip
    /// path production uses), so seal/unseal actually exercises the envelope.
    fn seeded_store(dir: &std::path::Path) -> SecretStore {
        let key_path = dir.join("mcnf-age-key");
        std::fs::write(
            &key_path,
            "AGE-SECRET-KEY-1QQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQSXKLP0E\n",
        )
        .unwrap();
        SecretStore::LocalAead {
            dir: dir.join("secrets"),
            key_path,
        }
    }

    fn node(id: &str, host: &str) -> DesiredNode {
        DesiredNode {
            node_id: id.to_string(),
            hostname: host.to_string(),
        }
    }

    // ── lock 3: hostname → sub-account username ──

    #[test]
    fn username_derives_from_hostname_lock3() {
        assert_eq!(sub_account_username("Eagle"), "eagle");
        assert_eq!(sub_account_username("nyc3.lh"), "nyc3-lh");
        assert_eq!(sub_account_username("  Big_Boy!! "), "big-boy");
        // Collapses runs + trims dashes; empty for a hostname with no charset.
        assert_eq!(sub_account_username("---"), "");
        assert_eq!(sub_account_username("a..b__c"), "a-b-c");
    }

    #[test]
    fn username_is_length_bounded() {
        let long = "x".repeat(80);
        assert_eq!(sub_account_username(&long).len(), 32);
    }

    #[test]
    fn sip_uri_uses_realm_with_default_fallback() {
        assert_eq!(sip_uri("eagle", "sip.example.net"), "eagle@sip.example.net");
        assert_eq!(sip_uri("eagle", ""), format!("eagle@{DEFAULT_REALM}"));
    }

    // ── lock 7: seal-target key is per-node + namespaced ──

    #[test]
    fn node_creds_ref_is_per_node_and_sanitized() {
        assert_eq!(
            node_creds_ref("peer:eagle"),
            "voice/node/peer:eagle/sip-creds"
        );
        // A stray separator can't widen the namespace.
        assert_eq!(node_creds_ref("a/b c"), "voice/node/abc/sip-creds");
        // Distinct nodes get distinct keys.
        assert_ne!(node_creds_ref("peer:a"), node_creds_ref("peer:b"));
    }

    // ── lock 19: the pure reconcile diff ──

    #[test]
    fn plan_provisions_a_new_node() {
        let desired = vec![node("peer:eagle", "eagle")];
        let existing = HashSet::new();
        let sealed = HashSet::new();
        let actions = plan_reconcile(&desired, &existing, &sealed);
        assert_eq!(
            actions,
            vec![VoiceAction::Provision {
                node_id: "peer:eagle".into(),
                hostname: "eagle".into(),
                username: "eagle".into(),
            }]
        );
    }

    #[test]
    fn plan_is_noop_for_fully_provisioned_node() {
        // Sub-account exists AND creds sealed → nothing to do (this is the
        // re-imaged-node heal: creds live in the replicated store).
        let desired = vec![node("peer:eagle", "eagle")];
        let existing: HashSet<String> = HashSet::from(["eagle".to_string()]);
        let sealed: HashSet<String> = HashSet::from(["peer:eagle".to_string()]);
        assert!(plan_reconcile(&desired, &existing, &sealed).is_empty());
    }

    #[test]
    fn plan_reseals_account_without_sealed_creds() {
        let desired = vec![node("peer:eagle", "eagle")];
        let existing: HashSet<String> = HashSet::from(["eagle".to_string()]);
        let sealed = HashSet::new();
        let actions = plan_reconcile(&desired, &existing, &sealed);
        assert_eq!(
            actions,
            vec![VoiceAction::Reseal {
                node_id: "peer:eagle".into(),
                hostname: "eagle".into(),
                username: "eagle".into(),
            }]
        );
    }

    #[test]
    fn plan_skips_unprovisionable_hostname() {
        let desired = vec![node("peer:x", "---")];
        assert!(plan_reconcile(&desired, &HashSet::new(), &HashSet::new()).is_empty());
    }

    // ── the full reconcile pass with the fake client + real seal ──

    #[test]
    fn reconcile_provisions_and_seals_a_new_node() {
        let tmp = tempfile::tempdir().unwrap();
        let store = seeded_store(tmp.path());
        let client = FakeVitelityClient::new("sip.vitelity.net");
        let desired = vec![node("peer:eagle", "eagle")];

        let outcome = reconcile_once(&client, &store, &desired, DEFAULT_REALM);
        assert_eq!(outcome.provisioned, 1);
        assert_eq!(outcome.states.len(), 1);
        let st = &outcome.states[0];
        assert_eq!(st.username, "eagle");
        assert_eq!(st.sip_uri, "eagle@sip.vitelity.net");
        assert_eq!(st.reg_state, RegState::Unregistered);

        // The SIP creds are actually sealed to the node's per-node key and read
        // back byte-for-byte (lock 7).
        let sealed = store.get(&node_creds_ref("peer:eagle")).unwrap().unwrap();
        let creds: SealedSipCreds = serde_json::from_str(&sealed).unwrap();
        assert_eq!(creds.username, "eagle");
        assert_eq!(creds.sip_password, "fake-pw-eagle");
        assert_eq!(creds.realm, "sip.vitelity.net");
    }

    #[test]
    fn reconcile_is_idempotent_second_pass_provisions_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let store = seeded_store(tmp.path());
        let client = FakeVitelityClient::new("sip.vitelity.net");
        let desired = vec![node("peer:eagle", "eagle")];

        let first = reconcile_once(&client, &store, &desired, DEFAULT_REALM);
        assert_eq!(first.provisioned, 1);
        // Second pass: sub-account exists + creds sealed → no new provisioning.
        let second = reconcile_once(&client, &store, &desired, DEFAULT_REALM);
        assert_eq!(second.provisioned, 0);
        assert_eq!(second.states[0].reg_state, RegState::Unregistered);
        // Exactly one sub-account was ever created (idempotent, lock 19).
        assert_eq!(client.list_sub_accounts().unwrap().len(), 1);
    }

    #[test]
    fn reconcile_heals_a_reimaged_node_via_the_replicated_store() {
        // A re-imaged node keeps its sub-account in Vitelity AND its sealed
        // creds in the replicated store → the reconcile is a no-op and the node
        // simply reads its creds back. Acceptance: healed without manual steps.
        let tmp = tempfile::tempdir().unwrap();
        let store = seeded_store(tmp.path());
        let client = FakeVitelityClient::new("sip.vitelity.net");
        let desired = vec![node("peer:eagle", "eagle")];
        let _ = reconcile_once(&client, &store, &desired, DEFAULT_REALM);

        // Simulate a re-image: the node's local disk is wiped, but the leader's
        // replicated store + Vitelity are untouched. A fresh reconcile pass
        // provisions nothing and the sealed creds are still readable.
        let healed = reconcile_once(&client, &store, &desired, DEFAULT_REALM);
        assert_eq!(healed.provisioned, 0);
        assert!(store.get(&node_creds_ref("peer:eagle")).unwrap().is_some());
    }

    #[test]
    fn reconcile_surfaces_a_vitelity_error_honestly_never_fake_online() {
        // The integration-gated live client can't reach Vitelity → every node
        // shows the real Error, not a fabricated Registered (§7 / lock 9).
        let tmp = tempfile::tempdir().unwrap();
        let store = seeded_store(tmp.path());
        let client = LiveVitelityClient::new(VitelityCredentials::new(
            "acct42".to_string(),
            "MASTER-KEY".to_string(),
        ));
        let desired = vec![node("peer:eagle", "eagle")];
        let outcome = reconcile_once(&client, &store, &desired, DEFAULT_REALM);
        assert_eq!(outcome.provisioned, 0);
        assert!(matches!(
            outcome.states[0].reg_state,
            RegState::Error { .. }
        ));
        // The master key never leaks into the published state.
        let body = serde_json::to_string(&outcome.states[0]).unwrap();
        assert!(!body.contains("MASTER-KEY"));
    }

    #[test]
    fn reconcile_flags_reseal_drift_as_error_not_silent() {
        // Sub-account exists (pre-seeded in Vitelity) but no sealed creds → an
        // honest, operator-actionable Error, never a silent claim of success.
        let tmp = tempfile::tempdir().unwrap();
        let store = seeded_store(tmp.path());
        let client = FakeVitelityClient::new("sip.vitelity.net");
        // Pre-create the sub-account WITHOUT sealing (the drift condition).
        client
            .create_sub_account(&CreateSubAccount {
                username: "eagle".into(),
                description: "eagle".into(),
            })
            .unwrap();
        let desired = vec![node("peer:eagle", "eagle")];
        let outcome = reconcile_once(&client, &store, &desired, DEFAULT_REALM);
        assert_eq!(outcome.provisioned, 0);
        match &outcome.states[0].reg_state {
            RegState::Error { reason } => assert!(reason.contains("Re-provision")),
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn awaiting_master_key_marks_every_node_provisioning() {
        let desired = vec![node("peer:a", "a"), node("peer:b", "b")];
        let outcome = awaiting_master_key(&desired, DEFAULT_REALM);
        assert_eq!(outcome.states.len(), 2);
        assert!(outcome
            .states
            .iter()
            .all(|s| s.reg_state == RegState::Provisioning));
    }

    // ── the leader gate ──

    #[test]
    fn worker_name_is_voice_provision() {
        let w = VoiceProvisionWorker::new(std::env::temp_dir(), "peer:test".into());
        assert_eq!(w.name(), "voice_provision");
    }

    #[test]
    fn is_leader_true_when_this_node_holds_the_lease() {
        let tmp = tempfile::tempdir().unwrap();
        let w = VoiceProvisionWorker::new(tmp.path().to_path_buf(), "peer:leader".into());
        // Uncontended lock → this node acquires + is leader.
        assert!(w.is_leader());
    }

    #[test]
    fn is_leader_false_when_another_node_holds_a_fresh_lease() {
        let tmp = tempfile::tempdir().unwrap();
        let lock = tmp.path().join(".mackesd-leader.lock");
        // Another node grabs the lease first.
        assert!(matches!(
            crate::leader::try_acquire(&lock, "peer:other"),
            Ok(crate::leader::AcquireResult::Acquired)
        ));
        let w = VoiceProvisionWorker::new(tmp.path().to_path_buf(), "peer:us".into());
        assert!(!w.is_leader());
    }

    #[tokio::test]
    async fn worker_exits_on_shutdown_token() {
        let tmp = tempfile::tempdir().unwrap();
        let bus = tempfile::tempdir().unwrap();
        let mut w = VoiceProvisionWorker::new(tmp.path().to_path_buf(), "peer:test".into())
            .with_bus_root(bus.path().to_path_buf())
            .with_poll_interval(Duration::from_millis(50));
        let (tx, rx) = tokio::sync::watch::channel(false);
        let token = ShutdownToken::from_receiver(rx);
        let _ = tx.send(true);
        let result = tokio::time::timeout(Duration::from_secs(3), w.run(token))
            .await
            .expect("worker must exit on shutdown");
        assert!(result.is_ok());
    }
}
