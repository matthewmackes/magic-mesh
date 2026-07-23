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
//! faked (§7, same typed-seam pattern as the seat clients). No raw shell — the only I/O is
//! the typed client, the secret store, and the Bus (§9).

#![cfg(feature = "async-services")]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;

use super::{ShutdownToken, Worker};
use crate::ipc::action_auth::{ActionAuthorizer, MutationContext};
use crate::ipc::secret_store::{self, SecretStore};
use crate::vitelity::model::VitelityCredentials;
use crate::vitelity::{
    CreateSubAccount, Did, DidRouting, FailoverPolicy, LiveVitelityClient, SubAccountCredentials,
    VitelityClient,
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

/// The typed verb the Voice panel publishes to route an **existing** master
/// DID to a node's sub-account (lock 11 — route-only, never a new-DID provision).
///
/// Body: [`DidRouteRequest`]. A message forces an immediate reconcile so the
/// operator sees the route take promptly. Typed verb in the canonical
/// `action/<domain>/<verb>` namespace (§9).
pub const DID_ROUTE_TOPIC: &str = "action/voice/did-route";

/// The typed verb the Voice panel publishes to set a node's offline-inbound
/// failover policy (lock 10). Body: [`FailoverRequest`].
pub const FAILOVER_TOPIC: &str = "action/voice/failover";

/// The Bus topic the master account's existing DID inventory is published to
/// (lock 11).
///
/// The Voice panel reads it to offer the route control; it is a single
/// fleet-wide list, NOT under [`STATE_TOPIC_PREFIX`] (which is one row per
/// node). The body is a JSON array of [`Did`].
pub const DIDS_TOPIC: &str = "state/voice-dids";

/// VOIP-GW-7 — the "Apply to fleet" verb: apply the leader-held shared-outbound.
///
/// The typed verb the Voice panel publishes to apply the one leader-held
/// **shared-outbound** (fleet) config (lock 13). Body: [`SharedConfigRequest`].
/// A message forces an immediate reconcile so the operator sees the config take.
/// Typed verb in the canonical `action/<domain>/<verb>` namespace (§9).
pub const SHARED_CONFIG_TOPIC: &str = "action/voice/shared-config";

/// VOIP-GW-7 — the Bus topic the fleet **cutover status** is published to.
///
/// A single fleet-wide row (NOT under [`STATE_TOPIC_PREFIX`], which is one row
/// per node — the same choice [`DIDS_TOPIC`] makes), so the panel's per-node
/// board never mistakes it for a node (lock 18). Body: [`CutoverStatus`].
pub const CUTOVER_TOPIC: &str = "state/voice-cutover";

/// VOIP-GW-7 — the Bus topic the current leader-held shared-outbound is mirrored to.
///
/// So the panel can show the operator the value that is actually in force (e.g.
/// a lifted legacy caller-ID, lock 13). A single fleet-wide row alongside
/// [`CUTOVER_TOPIC`]. Body: [`SharedOutboundConfig`].
pub const SHARED_STATE_TOPIC: &str = "state/voice-shared";

/// Stable authorization scope for fleet-wide Voice mutations. Capabilities
/// bind to this scope rather than the currently elected leader so failover
/// does not require re-minting an operator request.
pub const VOICE_AUTH_NODE: &str = "voice";
/// Closed semantic verb for the immediate provision/reconcile action.
pub const VOICE_PROVISION_AUTH_VERB: &str = "voice-provision";
/// Closed semantic verb for existing-DID routing intents.
pub const VOICE_DID_ROUTE_AUTH_VERB: &str = "voice-did-route";
/// Closed semantic verb for offline-inbound failover intents.
pub const VOICE_FAILOVER_AUTH_VERB: &str = "voice-failover";
/// Closed semantic verb for fleet shared-outbound configuration.
pub const VOICE_SHARED_CONFIG_AUTH_VERB: &str = "voice-shared-config";

/// Version of the small, token-free accepted-intent journal. Armed tokens
/// never enter this file; the shared authorizer's replay ledger owns nonce
/// consumption and remains the only capability secret.
const AUTHORIZED_INTENTS_SCHEMA_VERSION: u64 = 1;
/// Bounded state file size / per-key intent limits. A malformed or oversized
/// journal fails closed and leaves the worker with no desired mutations.
const MAX_AUTHORIZED_INTENTS_BYTES: usize = 64 * 1024;
const MAX_AUTHORIZED_DID_ROUTES: usize = 512;
const MAX_AUTHORIZED_FAILOVER: usize = 512;
const AUTHORIZED_INTENTS_FILE: &str = ".mackesd-voice-authorized-intents.json";

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

/// VOIP-GW-7 — the secret-store key the leader-held **shared-outbound** config
/// is sealed under (lock 13).
///
/// ONE entry for the whole fleet, held by the leader/provisioner only (the same
/// store the master key uses). Its presence is what marks the fleet "lifted off
/// the single-account model".
#[must_use]
pub fn shared_outbound_ref() -> String {
    "voice/shared-outbound".to_string()
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
    /// The master-account DIDs currently routed to this node's sub-account
    /// (lock 11). Reflects the **actual** Vitelity routing after the pass — a
    /// route that failed to apply is absent, never fabricated (§7). Empty when
    /// no DID targets this node.
    #[serde(default)]
    pub routed_dids: Vec<String>,
    /// The node's offline-inbound failover policy that was successfully applied
    /// this pass (lock 10). `None` when the operator hasn't set one (or the
    /// apply failed) — never a fabricated policy.
    #[serde(default)]
    pub failover: Option<FailoverPolicy>,
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

/// A desired DID→node route the operator set from the panel (lock 11).
///
/// The reconcile drives `route_did` to make it so; `node_id == None` means
/// "route this DID back to the master account's main line" (unroute).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesiredDidRoute {
    /// The existing master-account DID to route (never a new-DID provision).
    pub did: String,
    /// The node whose sub-account the DID should ring, or `None` for the main
    /// account.
    pub node_id: Option<String>,
}

/// A desired per-node failover policy the operator set from the panel (lock 10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesiredFailover {
    /// The node the policy applies to.
    pub node_id: String,
    /// The offline-inbound policy (voicemail / forward / none).
    pub policy: FailoverPolicy,
}

/// One idempotent DID-routing action (lock 11 + 19).
///
/// A pure diff of desired (operator intent) vs actual (Vitelity's current
/// `routed_to`) so it is unit-testable without any I/O. Never creates a DID —
/// it only re-points an existing one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DidAction {
    /// Point an existing DID at a node's sub-account username.
    Route {
        /// The DID to re-point.
        did: String,
        /// The sub-account username it should ring.
        username: String,
    },
    /// Point an existing DID back at the master account's main line.
    Unroute {
        /// The DID to release back to the main account.
        did: String,
    },
}

/// Pure DID-routing diff (lock 11 + 19): desired routes vs Vitelity's actual
/// DID inventory → the idempotent action list.
///
/// - A DID the master account does **not** own is skipped (route-existing only,
///   lock 11 — we never invent a DID).
/// - A route to a node whose username can't be resolved (unprovisioned) is
///   skipped until the node is provisioned.
/// - A DID already pointed at the desired target is a no-op (idempotent).
#[must_use]
pub fn plan_did_routes(
    routes: &[DesiredDidRoute],
    node_username: &HashMap<String, String, impl std::hash::BuildHasher>,
    actual: &[Did],
) -> Vec<DidAction> {
    let mut actions = Vec::new();
    for route in routes {
        // Lock 11: only route a DID the master account already owns.
        let Some(current) = actual.iter().find(|d| d.number == route.did) else {
            continue;
        };
        match &route.node_id {
            Some(node_id) => {
                let Some(username) = node_username.get(node_id) else {
                    continue;
                };
                if current.routed_to.as_deref() != Some(username.as_str()) {
                    actions.push(DidAction::Route {
                        did: route.did.clone(),
                        username: username.clone(),
                    });
                }
            }
            None => {
                if current.routed_to.is_some() {
                    actions.push(DidAction::Unroute {
                        did: route.did.clone(),
                    });
                }
            }
        }
    }
    actions
}

/// Pure failover plan (lock 10 + 19): the set-failover ops for every desired
/// policy whose node resolves to a sub-account username.
///
/// The Vitelity seam has no read-back for a sub-account's failover, so the
/// reconcile re-asserts the desired policy each (rate-limited) pass —
/// idempotent by construction (the same set), and self-healing against any
/// silent Vitelity-side drift.
#[must_use]
pub fn plan_failover(
    desired: &[DesiredFailover],
    node_username: &HashMap<String, String, impl std::hash::BuildHasher>,
) -> Vec<(String, FailoverPolicy, String)> {
    desired
        .iter()
        .filter_map(|f| {
            node_username
                .get(&f.node_id)
                .map(|u| (u.clone(), f.policy.clone(), f.node_id.clone()))
        })
        .collect()
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
    /// The master account's existing DID inventory as of this pass (lock 11).
    /// The panel reads it (via [`DIDS_TOPIC`]) to offer the route control.
    /// Empty when Vitelity was unreachable / not yet wired.
    pub dids: Vec<Did>,
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
    did_routes: &[DesiredDidRoute],
    failover: &[DesiredFailover],
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
                    routed_dids: Vec::new(),
                    failover: None,
                    updated_at_s: now_epoch_s(),
                })
                .collect();
            return ReconcileOutcome {
                states,
                provisioned: 0,
                dids: Vec::new(),
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
                                    routed_dids: Vec::new(),
                                    failover: None,
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
            routed_dids: Vec::new(),
            failover: None,
            updated_at_s: now_epoch_s(),
        });
    }

    // ── DID routing + failover reconcile (lock 10 + 11 + 19) ──
    //
    // Both run only after the sub-account list succeeded (the honesty gate
    // above): the integration-gated live client fails `list_sub_accounts`
    // first, so the whole fleet is already an honest Error and this code is
    // never reached with a faked Vitelity. With a reachable Vitelity (the fake
    // in tests, or a wired live transport) these apply the operator's intent.
    let node_username: HashMap<String, String> = desired
        .iter()
        .filter_map(|n| {
            let u = sub_account_username(&n.hostname);
            (!u.is_empty()).then(|| (n.node_id.clone(), u))
        })
        .collect();

    let (dids, did_by_username) = reconcile_did_routes(client, did_routes, &node_username);
    let failover_by_node = reconcile_failover(client, failover, &node_username);

    // Attach the real post-apply DID mapping + applied failover to each row.
    for st in &mut states {
        if let Some(username) = node_username.get(&st.node_id) {
            if let Some(dids) = did_by_username.get(username) {
                st.routed_dids = dids.clone();
            }
        }
        st.failover = failover_by_node.get(&st.node_id).cloned();
    }

    ReconcileOutcome {
        states,
        provisioned,
        dids,
    }
}

/// Reconcile the desired DID routes against Vitelity's actual inventory
/// (lock 11): list the master account's DIDs, apply each drift action, then
/// re-list so the returned mapping reflects the **real** post-apply routing —
/// a route that failed to apply is simply absent, never fabricated (§7).
///
/// Returns the DID inventory (for the panel) plus a `username → routed DIDs`
/// map (to fill each fleet row). A list failure yields empty maps (honest
/// "unknown"), not a guess.
fn reconcile_did_routes(
    client: &dyn VitelityClient,
    routes: &[DesiredDidRoute],
    node_username: &HashMap<String, String>,
) -> (Vec<Did>, HashMap<String, Vec<String>>) {
    let Ok(actual) = client.list_dids() else {
        return (Vec::new(), HashMap::new());
    };
    for action in plan_did_routes(routes, node_username, &actual) {
        let _ = match action {
            DidAction::Route { did, username } => {
                client.route_did(&did, &DidRouting::SubAccount(username))
            }
            DidAction::Unroute { did } => client.route_did(&did, &DidRouting::MainAccount),
        };
    }
    // Re-list to reflect the true state after applying (a failed route won't
    // appear). Fall back to the pre-apply list if the re-list itself fails.
    let post = client.list_dids().unwrap_or(actual);
    let mut by_username: HashMap<String, Vec<String>> = HashMap::new();
    for did in &post {
        if let Some(username) = &did.routed_to {
            by_username
                .entry(username.clone())
                .or_default()
                .push(did.number.clone());
        }
    }
    (post, by_username)
}

/// Re-assert every desired failover policy (lock 10). The seam has no
/// read-back, so the reconcile idempotently re-applies the desired policy each
/// pass; only a policy that actually applied is returned (and thus published),
/// so a failed apply never shows a fabricated policy (§7).
fn reconcile_failover(
    client: &dyn VitelityClient,
    desired: &[DesiredFailover],
    node_username: &HashMap<String, String>,
) -> HashMap<String, FailoverPolicy> {
    let mut applied = HashMap::new();
    for (username, policy, node_id) in plan_failover(desired, node_username) {
        if client.configure_failover(&username, &policy).is_ok() {
            applied.insert(node_id, policy);
        }
    }
    applied
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
        routed_dids: Vec::new(),
        failover: None,
        updated_at_s: now_epoch_s(),
    }
}

// ── VOIP-GW-7: the hard-cutover migration (locks 13 + 18) ────────────────────

/// The one leader-held **shared-outbound** (fleet) config (lock 13).
///
/// Sealed once at the leader; carries the shared caller-ID + the outbound trunk
/// label. The panel's "Apply to fleet" verb sets it; the reconcile persists +
/// mirrors it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SharedOutboundConfig {
    /// The shared caller-ID number all outbound PSTN presents (lock 4/13).
    pub caller_id: String,
    /// The shared outbound trunk label / account.
    pub outbound_trunk: String,
}

/// Durable, token-free projection of Voice intents that already passed the
/// action gate. It lets a worker restart without re-authorizing spent nonces;
/// the raw armed request is intentionally never persisted.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct AuthorizedVoiceIntents {
    schema_version: u64,
    #[serde(default)]
    did_routes: BTreeMap<String, Option<String>>,
    #[serde(default)]
    failover: BTreeMap<String, FailoverPolicy>,
    #[serde(default)]
    shared_config: Option<SharedOutboundConfig>,
}

/// The `action/voice/shared-config` body the panel publishes (lock 13).
#[derive(Debug, serde::Deserialize)]
struct SharedConfigRequest {
    caller_id: String,
    outbound_trunk: String,
}

/// The fleet migration phase (lock 18 — hard cutover).
///
/// A single fleet-wide state machine the leader drives each pass, computed
/// purely from whether the shared-outbound has been lifted and how many nodes
/// are reprovisioned onto the split model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CutoverPhase {
    /// The fleet is still on the pre-split single (flat) account model — the
    /// shared-outbound config has not been lifted to the fleet yet.
    Legacy,
    /// The shared-outbound config is lifted (leader-held) and keeps outbound
    /// alive, but no node has yet been reprovisioned onto its own inbound
    /// sub-account.
    LiftedSharedOutbound,
    /// Some nodes have crossed onto the split model; others are still pending —
    /// the flag day is in progress.
    NodesReprovisioning,
    /// Every enrolled node is on the split model — the cutover is done.
    CutoverComplete,
}

impl CutoverPhase {
    /// A one-line operator headline for the panel banner.
    #[must_use]
    pub const fn headline(self) -> &'static str {
        match self {
            Self::Legacy => {
                "Legacy single-account model — apply the fleet shared-outbound to begin"
            }
            Self::LiftedSharedOutbound => {
                "Shared-outbound lifted — outbound alive; reprovisioning nodes onto the split model"
            }
            Self::NodesReprovisioning => {
                "Cutover in progress — some nodes still on the legacy model"
            }
            Self::CutoverComplete => "Cutover complete — every node on the split model",
        }
    }
}

/// Pure cutover fold (lock 18): derive the migration [`CutoverPhase`].
///
/// From whether the shared-outbound has been lifted (leader-held config present)
/// and how many of the `total` enrolled nodes are reprovisioned onto the split
/// model. The single decision point — unit-tested without any I/O.
#[must_use]
pub const fn cutover_phase(lifted: bool, total: usize, reprovisioned: usize) -> CutoverPhase {
    if !lifted {
        return CutoverPhase::Legacy;
    }
    if total == 0 || reprovisioned == 0 {
        return CutoverPhase::LiftedSharedOutbound;
    }
    if reprovisioned >= total {
        CutoverPhase::CutoverComplete
    } else {
        CutoverPhase::NodesReprovisioning
    }
}

/// The fleet cutover status published to [`CUTOVER_TOPIC`] (lock 18).
///
/// The phase plus the reprovision progress + the node ids still on the legacy
/// model, so the panel prompts the operator clearly through the flag day.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CutoverStatus {
    /// The single fleet-wide migration phase.
    pub phase: CutoverPhase,
    /// Enrolled nodes total.
    pub total_nodes: usize,
    /// How many are reprovisioned onto the split model (their own inbound sub).
    pub reprovisioned: usize,
    /// The nodes still on the legacy model (hostname, or node id) — the panel
    /// shows exactly which remain.
    pub pending_nodes: Vec<String>,
    /// Whether the fleet shared-outbound config is lifted (leader-held).
    pub shared_outbound_lifted: bool,
    /// When this status was produced (epoch seconds).
    pub updated_at_s: u64,
}

/// A node counts as **reprovisioned** onto the split model once it has its own
/// inbound sub-account provisioned + sealed — i.e. it is Registered or (the
/// steady state) Unregistered. A node still Provisioning (awaiting the master
/// key) or in Error has not crossed over yet.
const fn is_reprovisioned(reg: &RegState) -> bool {
    matches!(reg, RegState::Registered | RegState::Unregistered)
}

/// Pure fold (lock 18): derive the [`CutoverStatus`].
///
/// From the reconcile's per-node states + the lift flag. A node not yet
/// reprovisioned lands in `pending_nodes` (named by hostname, falling back to
/// node id).
#[must_use]
pub fn derive_cutover_status(states: &[NodeVoiceState], lifted: bool) -> CutoverStatus {
    let total = states.len();
    let mut reprovisioned = 0usize;
    let mut pending_nodes = Vec::new();
    for st in states {
        if is_reprovisioned(&st.reg_state) {
            reprovisioned += 1;
        } else if st.hostname.trim().is_empty() {
            pending_nodes.push(st.node_id.clone());
        } else {
            pending_nodes.push(st.hostname.clone());
        }
    }
    CutoverStatus {
        phase: cutover_phase(lifted, total, reprovisioned),
        total_nodes: total,
        reprovisioned,
        pending_nodes,
        shared_outbound_lifted: lifted,
        updated_at_s: now_epoch_s(),
    }
}

/// The hard-cutover invariant (lock 18): no node left dual-model.
///
/// `CutoverComplete` is reached ONLY when every enrolled node is reprovisioned
/// onto the split model — never while one still straddles the legacy account. We
/// never declare the flag day done while any node remains pending. Returns
/// `true` when the status honors the invariant.
#[must_use]
pub fn no_node_left_dual_model(status: &CutoverStatus) -> bool {
    match status.phase {
        CutoverPhase::CutoverComplete => {
            status.pending_nodes.is_empty()
                && status.total_nodes > 0
                && status.reprovisioned == status.total_nodes
        }
        _ => true,
    }
}

/// Fold the retained `action/voice/shared-config` messages into the desired
/// fleet shared-outbound config (lock 13), latest-wins (ULID order is
/// oldest→newest). `None` when the operator hasn't applied one yet.
#[cfg(test)]
fn read_desired_shared_config(persist: &Persist) -> Option<SharedOutboundConfig> {
    let mut latest: Option<SharedOutboundConfig> = None;
    if let Ok(msgs) = persist.list_since(SHARED_CONFIG_TOPIC, None) {
        for msg in msgs {
            if let Some(body) = msg.body {
                if let Ok(req) = serde_json::from_str::<SharedConfigRequest>(&body) {
                    latest = Some(SharedOutboundConfig {
                        caller_id: req.caller_id,
                        outbound_trunk: req.outbound_trunk,
                    });
                }
            }
        }
    }
    latest
}

/// Whether the fleet shared-outbound config is lifted (present in the leader
/// store). A store fault is treated as "not lifted" (honest — the pass will
/// re-attempt the persist and surface the real error there).
fn shared_outbound_is_lifted(store: &SecretStore) -> bool {
    matches!(store.get(&shared_outbound_ref()), Ok(Some(_)))
}

/// VOIP-GW-7 — apply the operator's "Apply to fleet" shared-outbound config.
///
/// Folds the latest retained `action/voice/shared-config` verb (lock 13) and
/// persists it to the leader-held store — the real "Apply to fleet" effect GW-5
/// left as an observable-only verb. Returns whether the fleet is now lifted off
/// the single-account model (the shared-outbound is present).
#[cfg(test)]
fn apply_shared_config(store: &SecretStore, persist: &Persist) -> bool {
    let desired = read_desired_shared_config(persist);
    apply_shared_config_value(store, desired.as_ref())
}

/// Apply an already-authorized shared-outbound intent. Production uses the
/// worker's authorized in-memory intent; the retained-reader wrapper above is
/// kept for the pure persistence tests and legacy callers.
fn apply_shared_config_value(store: &SecretStore, desired: Option<&SharedOutboundConfig>) -> bool {
    if let Some(cfg) = desired {
        match serde_json::to_string(&cfg) {
            Ok(body) => {
                if let Err(e) = store.put(&shared_outbound_ref(), &body) {
                    tracing::warn!(
                        target: "mackesd::voice_provision",
                        error = %e,
                        "sealing the fleet shared-outbound config failed"
                    );
                }
            }
            Err(e) => tracing::warn!(
                target: "mackesd::voice_provision",
                error = %e,
                "serializing the fleet shared-outbound config failed"
            ),
        }
    }
    shared_outbound_is_lifted(store)
}

/// Publish the fleet cutover status to [`CUTOVER_TOPIC`] (lock 18).
/// `Priority::Min` — a silent data topic the panel reads. Best-effort.
pub fn publish_cutover(persist: &Persist, status: &CutoverStatus) {
    let Ok(body) = serde_json::to_string(status) else {
        return;
    };
    if let Err(e) = persist.write(CUTOVER_TOPIC, Priority::Min, None, Some(&body)) {
        tracing::debug!(
            target: "mackesd::voice_provision",
            error = %e,
            "publishing voice cutover status failed"
        );
    }
}

/// Mirror the current leader-held shared-outbound config to [`SHARED_STATE_TOPIC`]
/// (lock 13) so the panel can show the value in force (e.g. a lifted legacy
/// caller-ID). Best-effort; publishes nothing when none is set.
fn publish_shared_state(persist: &Persist, store: &SecretStore) {
    if let Ok(Some(body)) = store.get(&shared_outbound_ref()) {
        if let Err(e) = persist.write(SHARED_STATE_TOPIC, Priority::Min, None, Some(&body)) {
            tracing::debug!(
                target: "mackesd::voice_provision",
                error = %e,
                "mirroring shared-outbound config failed"
            );
        }
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
    /// Shared verifier for every privileged Voice action topic.
    authorizer: Arc<ActionAuthorizer>,
    /// Authorized desired intents. They are updated only after exact-body
    /// verification, persisted locally, then replayed idempotently on each
    /// reconcile pass.
    desired_did_routes: HashMap<String, Option<String>>,
    desired_failover: HashMap<String, FailoverPolicy>,
    desired_shared_config: Option<SharedOutboundConfig>,
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
            authorizer: Arc::new(ActionAuthorizer::production()),
            desired_did_routes: HashMap::new(),
            desired_failover: HashMap::new(),
            desired_shared_config: None,
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

    /// Test-only verifier injection; production always uses the systemd
    /// credential loaded by [`ActionAuthorizer::production`].
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_authorizer(mut self, authorizer: Arc<ActionAuthorizer>) -> Self {
        self.authorizer = authorizer;
        self
    }

    /// Path for the token-free accepted-intent projection. It sits beside the
    /// daemon's SQLite state (the production default is root-owned local
    /// state), never on the replicated workgroup volume.
    fn authorized_intents_path(&self) -> PathBuf {
        self.db_path.with_file_name(AUTHORIZED_INTENTS_FILE)
    }

    /// Load accepted intents without attempting to re-authorize their spent
    /// capabilities. Invalid, future, or oversized state fails closed.
    fn load_authorized_intents(&mut self) {
        let path = self.authorized_intents_path();
        let Ok(metadata) = std::fs::metadata(&path) else {
            return;
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.uid() != rustix::process::geteuid().as_raw() || metadata.mode() & 0o077 != 0
            {
                tracing::warn!(
                    target: "mackesd::voice_provision",
                    path = %path.display(),
                    "authorized Voice intent journal is not owner-only; ignoring it"
                );
                return;
            }
        }
        let Ok(bytes) = std::fs::read(&path) else {
            return;
        };
        if bytes.len() > MAX_AUTHORIZED_INTENTS_BYTES {
            tracing::warn!(
                target: "mackesd::voice_provision",
                path = %path.display(),
                cap = MAX_AUTHORIZED_INTENTS_BYTES,
                "authorized Voice intent journal exceeds its size cap; ignoring it"
            );
            return;
        }
        let Ok(state) = serde_json::from_slice::<AuthorizedVoiceIntents>(&bytes) else {
            tracing::warn!(
                target: "mackesd::voice_provision",
                path = %path.display(),
                "authorized Voice intent journal is invalid; ignoring it"
            );
            return;
        };
        if state.schema_version != AUTHORIZED_INTENTS_SCHEMA_VERSION
            || state.did_routes.len() > MAX_AUTHORIZED_DID_ROUTES
            || state.failover.len() > MAX_AUTHORIZED_FAILOVER
        {
            tracing::warn!(
                target: "mackesd::voice_provision",
                path = %path.display(),
                "authorized Voice intent journal has an unsupported schema or cardinality; ignoring it"
            );
            return;
        }
        self.desired_did_routes = state.did_routes.into_iter().collect();
        self.desired_failover = state.failover.into_iter().collect();
        self.desired_shared_config = state.shared_config;
    }

    /// Persist the accepted desired projection atomically after authorization.
    /// The file contains only typed intents, never an armed token or request
    /// body. A bounded journal keeps hostile target churn from exhausting disk.
    fn persist_authorized_intents(&self) -> Result<(), String> {
        if self.desired_did_routes.len() > MAX_AUTHORIZED_DID_ROUTES
            || self.desired_failover.len() > MAX_AUTHORIZED_FAILOVER
        {
            return Err("authorized Voice intent journal is full".to_string());
        }
        let state = AuthorizedVoiceIntents {
            schema_version: AUTHORIZED_INTENTS_SCHEMA_VERSION,
            did_routes: self
                .desired_did_routes
                .iter()
                .map(|(did, node_id)| (did.clone(), node_id.clone()))
                .collect(),
            failover: self
                .desired_failover
                .iter()
                .map(|(node_id, policy)| (node_id.clone(), policy.clone()))
                .collect(),
            shared_config: self.desired_shared_config.clone(),
        };
        let bytes = serde_json::to_vec(&state)
            .map_err(|_| "authorized Voice intent journal serialization failed".to_string())?;
        if bytes.len() > MAX_AUTHORIZED_INTENTS_BYTES {
            return Err("authorized Voice intent journal exceeds its size cap".to_string());
        }
        let path = self.authorized_intents_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|_| "authorized Voice intent state directory unavailable".to_string())?;
        }
        let tmp = path.with_extension("json.tmp");
        let mut options = OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&tmp)
            .map_err(|_| "authorized Voice intent journal is not writable".to_string())?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| "authorized Voice intent journal write failed".to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))
                .map_err(|_| "authorized Voice intent journal permissions failed".to_string())?;
        }
        drop(file);
        std::fs::rename(&tmp, &path)
            .map_err(|_| "authorized Voice intent journal commit failed".to_string())
    }

    /// Admit one Bus action into the desired state only after authorization.
    /// Returns `true` when the action is valid and should trigger a reconcile.
    fn accept_action(&mut self, topic: &str, body: Option<&str>) -> Result<bool, String> {
        match authorize_voice_action(&self.authorizer, topic, body)? {
            AuthorizedVoiceAction::Provision => Ok(true),
            AuthorizedVoiceAction::DidRoute(route) => {
                if !self.desired_did_routes.contains_key(&route.did)
                    && self.desired_did_routes.len() >= MAX_AUTHORIZED_DID_ROUTES
                {
                    return Err("authorized Voice intent journal is full".to_string());
                }
                let did = route.did;
                let previous = self.desired_did_routes.insert(did.clone(), route.node_id);
                if let Err(error) = self.persist_authorized_intents() {
                    match previous {
                        Some(value) => {
                            self.desired_did_routes.insert(did, value);
                        }
                        None => {
                            self.desired_did_routes.remove(&did);
                        }
                    }
                    return Err(error);
                }
                Ok(true)
            }
            AuthorizedVoiceAction::Failover(failover) => {
                if !self.desired_failover.contains_key(&failover.node_id)
                    && self.desired_failover.len() >= MAX_AUTHORIZED_FAILOVER
                {
                    return Err("authorized Voice intent journal is full".to_string());
                }
                let node_id = failover.node_id;
                let previous = self
                    .desired_failover
                    .insert(node_id.clone(), failover.policy);
                if let Err(error) = self.persist_authorized_intents() {
                    match previous {
                        Some(value) => {
                            self.desired_failover.insert(node_id, value);
                        }
                        None => {
                            self.desired_failover.remove(&node_id);
                        }
                    }
                    return Err(error);
                }
                Ok(true)
            }
            AuthorizedVoiceAction::SharedConfig(config) => {
                let previous = self.desired_shared_config.replace(config);
                if let Err(error) = self.persist_authorized_intents() {
                    self.desired_shared_config = previous;
                    return Err(error);
                }
                Ok(true)
            }
        }
    }

    /// Only the elected leader provisions + holds the master key (lock 7).
    /// Reuses the shared leader lock — a cheap synchronous check per pass.
    fn is_leader(&self) -> bool {
        crate::leader_gate::LeaderGate::from_lock_path(
            self.leader_lock.clone(),
            self.node_id.clone(),
        )
        .is_leader()
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
        // VOIP-GW-7 — apply + mirror the leader-held shared-outbound config first
        // (lock 13), so the cutover phase below reflects the current lift state
        // even when no node is enrolled yet.
        let lifted = apply_shared_config_value(&store, self.desired_shared_config.as_ref());
        publish_shared_state(persist, &store);

        let desired = self.read_desired();
        if desired.is_empty() {
            // No enrolled nodes, but still drive the cutover machine off the lift
            // flag (Legacy vs LiftedSharedOutbound) so the panel prompts honestly.
            publish_cutover(persist, &derive_cutover_status(&[], lifted));
            return None;
        }
        // Re-apply the operator's already-authorized DID-route + failover
        // intents (latest-wins per key) so the reconcile remains idempotent
        // every pass — the desired side of lock 10 + 11 + 19.
        let did_routes: Vec<DesiredDidRoute> = self
            .desired_did_routes
            .iter()
            .map(|(did, node_id)| DesiredDidRoute {
                did: did.clone(),
                node_id: node_id.clone(),
            })
            .collect();
        let failover: Vec<DesiredFailover> = self
            .desired_failover
            .iter()
            .map(|(node_id, policy)| DesiredFailover {
                node_id: node_id.clone(),
                policy: policy.clone(),
            })
            .collect();
        // Resolve the client: the injected test client, else the live client
        // built from the sealed master creds. No master creds → publish a
        // Provisioning state for every node (honest "awaiting the master key")
        // and skip the API.
        let outcome = if let Some(client) = self.client_override.as_deref() {
            reconcile_once(
                client,
                &store,
                &desired,
                &did_routes,
                &failover,
                &self.realm,
            )
        } else {
            match resolve_live_client(&store) {
                Ok(Some(client)) => reconcile_once(
                    &client,
                    &store,
                    &desired,
                    &did_routes,
                    &failover,
                    &self.realm,
                ),
                Ok(None) => awaiting_master_key(&desired, &self.realm),
                Err(e) => master_key_error(&desired, &self.realm, &e),
            }
        };
        for state in &outcome.states {
            publish_state(persist, state);
        }
        // Publish the master DID inventory so the panel can offer the route
        // control (lock 11). Best-effort; empty on an unreachable Vitelity.
        publish_dids(persist, &outcome.dids);
        // VOIP-GW-7 — drive + publish the fleet cutover status (lock 18) from the
        // per-node states + the lift flag: which nodes have crossed onto the
        // split model, and which still remain.
        publish_cutover(persist, &derive_cutover_status(&outcome.states, lifted));
        Some(outcome)
    }
}

/// The `action/voice/did-route` body the panel publishes: route an existing
/// DID to a node's sub-account, or (`node_id == None`) back to the main line.
#[derive(Debug, serde::Deserialize)]
struct DidRouteRequest {
    did: String,
    #[serde(default)]
    node_id: Option<String>,
}

/// The `action/voice/failover` body the panel publishes: a node's desired
/// offline-inbound policy.
#[derive(Debug, serde::Deserialize)]
struct FailoverRequest {
    node_id: String,
    policy: FailoverPolicy,
}

/// A Voice action that has passed the shared privileged-action boundary and
/// may therefore update the worker's in-memory desired state.
#[derive(Debug)]
enum AuthorizedVoiceAction {
    Provision,
    DidRoute(DesiredDidRoute),
    Failover(DesiredFailover),
    SharedConfig(SharedOutboundConfig),
}

/// Verify one raw Voice action body before it can influence a reconcile pass.
/// The exact body (including schema and armed token) is authenticated first;
/// typed decoding happens only after authorization succeeds. This keeps an
/// untrusted Bus message from reaching provider, routing, failover, or config
/// side effects.
fn authorize_voice_action(
    authorizer: &ActionAuthorizer,
    topic: &str,
    body: Option<&str>,
) -> Result<AuthorizedVoiceAction, String> {
    let raw = body.unwrap_or_default();
    let (verb, target) = match topic {
        PROVISION_TOPIC => (VOICE_PROVISION_AUTH_VERB, "fleet".to_string()),
        DID_ROUTE_TOPIC => {
            let target = serde_json::from_str::<serde_json::Value>(raw)
                .ok()
                .and_then(|value| {
                    value
                        .get("did")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned)
                })
                .unwrap_or_default();
            (VOICE_DID_ROUTE_AUTH_VERB, target)
        }
        FAILOVER_TOPIC => {
            let target = serde_json::from_str::<serde_json::Value>(raw)
                .ok()
                .and_then(|value| {
                    value
                        .get("node_id")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned)
                })
                .unwrap_or_default();
            (VOICE_FAILOVER_AUTH_VERB, target)
        }
        SHARED_CONFIG_TOPIC => (VOICE_SHARED_CONFIG_AUTH_VERB, "fleet".to_string()),
        other => return Err(format!("unknown Voice action topic: {other}")),
    };
    authorizer.authorize(
        raw,
        MutationContext {
            verb,
            node: VOICE_AUTH_NODE,
            target: &target,
        },
    )?;

    match topic {
        PROVISION_TOPIC => Ok(AuthorizedVoiceAction::Provision),
        DID_ROUTE_TOPIC => {
            let request: DidRouteRequest = serde_json::from_str(raw)
                .map_err(|_| "did-route: request body is invalid".to_string())?;
            Ok(AuthorizedVoiceAction::DidRoute(DesiredDidRoute {
                did: request.did,
                node_id: request.node_id,
            }))
        }
        FAILOVER_TOPIC => {
            let request: FailoverRequest = serde_json::from_str(raw)
                .map_err(|_| "failover: request body is invalid".to_string())?;
            Ok(AuthorizedVoiceAction::Failover(DesiredFailover {
                node_id: request.node_id,
                policy: request.policy,
            }))
        }
        SHARED_CONFIG_TOPIC => {
            let request: SharedConfigRequest = serde_json::from_str(raw)
                .map_err(|_| "shared-config: request body is invalid".to_string())?;
            Ok(AuthorizedVoiceAction::SharedConfig(SharedOutboundConfig {
                caller_id: request.caller_id,
                outbound_trunk: request.outbound_trunk,
            }))
        }
        _ => unreachable!("topic was matched above"),
    }
}

/// Fold the retained `action/voice/did-route` messages into the desired route
/// set (lock 11), latest-wins per DID (ULID order is oldest→newest).
#[cfg(test)]
fn read_desired_did_routes(persist: &Persist) -> Vec<DesiredDidRoute> {
    let mut latest: HashMap<String, Option<String>> = HashMap::new();
    if let Ok(msgs) = persist.list_since(DID_ROUTE_TOPIC, None) {
        for msg in msgs {
            if let Some(body) = msg.body {
                if let Ok(req) = serde_json::from_str::<DidRouteRequest>(&body) {
                    latest.insert(req.did, req.node_id);
                }
            }
        }
    }
    latest
        .into_iter()
        .map(|(did, node_id)| DesiredDidRoute { did, node_id })
        .collect()
}

/// Fold the retained `action/voice/failover` messages into the desired policy
/// set (lock 10), latest-wins per node.
#[cfg(test)]
fn read_desired_failover(persist: &Persist) -> Vec<DesiredFailover> {
    let mut latest: HashMap<String, FailoverPolicy> = HashMap::new();
    if let Ok(msgs) = persist.list_since(FAILOVER_TOPIC, None) {
        for msg in msgs {
            if let Some(body) = msg.body {
                if let Ok(req) = serde_json::from_str::<FailoverRequest>(&body) {
                    latest.insert(req.node_id, req.policy);
                }
            }
        }
    }
    latest
        .into_iter()
        .map(|(node_id, policy)| DesiredFailover { node_id, policy })
        .collect()
}

/// Publish the master DID inventory to [`DIDS_TOPIC`] (lock 11).
///
/// The single fleet-wide list the panel reads to offer the route control.
/// `Priority::Min` (a silent data topic); a failed write is logged, never fatal.
pub fn publish_dids(persist: &Persist, dids: &[Did]) {
    let Ok(body) = serde_json::to_string(dids) else {
        return;
    };
    if let Err(e) = persist.write(DIDS_TOPIC, Priority::Min, None, Some(&body)) {
        tracing::debug!(
            target: "mackesd::voice_provision",
            error = %e,
            "publishing voice DID inventory failed"
        );
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
                routed_dids: Vec::new(),
                failover: None,
                updated_at_s: now_epoch_s(),
            }
        })
        .collect();
    ReconcileOutcome {
        states,
        provisioned: 0,
        dids: Vec::new(),
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
        dids: Vec::new(),
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
        // Restore only the typed, already-authorized projection. The armed
        // request bodies and spent nonces remain in the shared authorizer's
        // durable replay ledger; restart never re-authorizes a Bus backlog.
        self.load_authorized_intents();
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
        // Cursors for the panel verbs — start at each tail so we only act on
        // requests published after we come up. Provision (lock 8), DID-route
        // (lock 11) and failover (lock 10) all force an immediate reconcile.
        let mut cursor: Option<String> = persist.latest_ulid(PROVISION_TOPIC).ok().flatten();
        let mut did_cursor: Option<String> = persist.latest_ulid(DID_ROUTE_TOPIC).ok().flatten();
        let mut failover_cursor: Option<String> =
            persist.latest_ulid(FAILOVER_TOPIC).ok().flatten();
        // VOIP-GW-7 — the "Apply to fleet" shared-outbound verb (lock 13) also
        // forces an immediate reconcile so the lift + cutover take promptly.
        let mut shared_cursor: Option<String> =
            persist.latest_ulid(SHARED_CONFIG_TOPIC).ok().flatten();

        loop {
            // Drain the panel verbs (lock 8/10/11). A follower still advances
            // the cursors + skips (the leader acts), so failover is seamless.
            let mut button_pressed = false;
            if let Ok(msgs) = persist.list_since(PROVISION_TOPIC, cursor.as_deref()) {
                for msg in msgs {
                    cursor = Some(msg.ulid);
                    match self.accept_action(PROVISION_TOPIC, msg.body.as_deref()) {
                        Ok(true) => button_pressed = true,
                        Ok(false) => {}
                        Err(error) => tracing::warn!(
                            target: "mackesd::voice_provision",
                            %error,
                            "refused unauthorized Voice provision action"
                        ),
                    }
                }
            }
            if let Ok(msgs) = persist.list_since(DID_ROUTE_TOPIC, did_cursor.as_deref()) {
                for msg in msgs {
                    did_cursor = Some(msg.ulid);
                    match self.accept_action(DID_ROUTE_TOPIC, msg.body.as_deref()) {
                        Ok(true) => button_pressed = true,
                        Ok(false) => {}
                        Err(error) => tracing::warn!(
                            target: "mackesd::voice_provision",
                            %error,
                            "refused unauthorized Voice DID-route action"
                        ),
                    }
                }
            }
            if let Ok(msgs) = persist.list_since(SHARED_CONFIG_TOPIC, shared_cursor.as_deref()) {
                for msg in msgs {
                    shared_cursor = Some(msg.ulid);
                    match self.accept_action(SHARED_CONFIG_TOPIC, msg.body.as_deref()) {
                        Ok(true) => button_pressed = true,
                        Ok(false) => {}
                        Err(error) => tracing::warn!(
                            target: "mackesd::voice_provision",
                            %error,
                            "refused unauthorized Voice shared-config action"
                        ),
                    }
                }
            }
            if let Ok(msgs) = persist.list_since(FAILOVER_TOPIC, failover_cursor.as_deref()) {
                for msg in msgs {
                    failover_cursor = Some(msg.ulid);
                    match self.accept_action(FAILOVER_TOPIC, msg.body.as_deref()) {
                        Ok(true) => button_pressed = true,
                        Ok(false) => {}
                        Err(error) => tracing::warn!(
                            target: "mackesd::voice_provision",
                            %error,
                            "refused unauthorized Voice failover action"
                        ),
                    }
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
    use crate::ipc::action_auth::{ActionAuthorizer, authorize_test_body};
    use crate::vitelity::FakeVitelityClient;

    const ACTION_KEY: &[u8] = b"voice-action-auth-test-key";
    const ACTION_NOW: i64 = 1_700_000_000_000;

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

        let outcome = reconcile_once(&client, &store, &desired, &[], &[], DEFAULT_REALM);
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

        let first = reconcile_once(&client, &store, &desired, &[], &[], DEFAULT_REALM);
        assert_eq!(first.provisioned, 1);
        // Second pass: sub-account exists + creds sealed → no new provisioning.
        let second = reconcile_once(&client, &store, &desired, &[], &[], DEFAULT_REALM);
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
        let _ = reconcile_once(&client, &store, &desired, &[], &[], DEFAULT_REALM);

        // Simulate a re-image: the node's local disk is wiped, but the leader's
        // replicated store + Vitelity are untouched. A fresh reconcile pass
        // provisions nothing and the sealed creds are still readable.
        let healed = reconcile_once(&client, &store, &desired, &[], &[], DEFAULT_REALM);
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
        let outcome = reconcile_once(&client, &store, &desired, &[], &[], DEFAULT_REALM);
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
        let outcome = reconcile_once(&client, &store, &desired, &[], &[], DEFAULT_REALM);
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
    fn voice_mutations_require_exact_single_use_authority_before_routing() {
        let tmp = tempfile::tempdir().unwrap();
        let auth_root = tmp.path().join("action-auth");
        let authorizer = Arc::new(ActionAuthorizer::for_test(
            ACTION_KEY, auth_root, ACTION_NOW,
        ));
        let unsigned = serde_json::json!({
            "schema_version": 1,
            "did": "15551234567",
            "node_id": "peer:eagle"
        })
        .to_string();

        let mut worker = VoiceProvisionWorker::new(tmp.path().to_path_buf(), "peer:leader".into())
            .with_db_path(tmp.path().join("voice.sqlite"))
            .with_authorizer(Arc::clone(&authorizer));
        // A hostile unsigned message is refused before it can enter the
        // desired set that drives provider/routing effects.
        assert!(worker
            .accept_action(DID_ROUTE_TOPIC, Some(&unsigned))
            .is_err());
        assert!(worker.desired_did_routes.is_empty());

        let context = MutationContext {
            verb: VOICE_DID_ROUTE_AUTH_VERB,
            node: VOICE_AUTH_NODE,
            target: "15551234567",
        };
        let armed = authorize_test_body(
            ACTION_KEY,
            &unsigned,
            context,
            "voice-route-once",
            ACTION_NOW + 30_000,
        );
        // Body tampering is rejected without claiming the valid nonce.
        let tampered = armed.replace("peer:eagle", "peer:pine");
        assert!(worker
            .accept_action(DID_ROUTE_TOPIC, Some(&tampered))
            .is_err());
        assert!(worker.accept_action(DID_ROUTE_TOPIC, Some(&armed)).unwrap());
        // A capability is durable single-use; replay cannot re-assert the
        // intent or trigger another provider call.
        assert!(worker
            .accept_action(DID_ROUTE_TOPIC, Some(&armed))
            .unwrap_err()
            .contains("already used"));
        let journal = std::fs::read_to_string(tmp.path().join(AUTHORIZED_INTENTS_FILE)).unwrap();
        assert!(journal.contains("15551234567"));
        assert!(!journal.contains("armed_token"));

        // Restart restores the typed intent without attempting to spend the
        // already-consumed capability again.
        let mut restarted =
            VoiceProvisionWorker::new(tmp.path().to_path_buf(), "peer:leader".into())
                .with_db_path(tmp.path().join("voice.sqlite"))
                .with_authorizer(authorizer);
        restarted.load_authorized_intents();
        assert_eq!(
            restarted.desired_did_routes.get("15551234567"),
            Some(&Some("peer:eagle".to_string()))
        );

        let store = seeded_store(tmp.path());
        let db = tmp.path().join("nodes.sqlite");
        let conn = crate::store::open(&db).unwrap();
        crate::store::upsert_node(&conn, "peer:eagle", "eagle", "pk", None).unwrap();
        drop(conn);
        let bus = tempfile::tempdir().unwrap();
        let client = FakeVitelityClient::new(DEFAULT_REALM).with_did("15551234567", None);
        worker = restarted
            .with_db_path(db)
            .with_bus_root(bus.path().to_path_buf())
            .with_store(store)
            .with_client(Box::new(client));
        let persist = Persist::open(bus.path().to_path_buf()).unwrap();
        // The authorized route is the only intent admitted to reconcile, so
        // the fake provider records the route only after that boundary.
        let _ = worker.reconcile_and_publish(&persist);
        let routed = worker
            .client_override
            .as_deref()
            .unwrap()
            .list_dids()
            .unwrap();
        assert_eq!(routed[0].routed_to, Some("eagle".to_string()));
    }

    #[test]
    fn voice_config_and_failover_actions_are_authorized_before_state_update() {
        let tmp = tempfile::tempdir().unwrap();
        let authorizer = Arc::new(ActionAuthorizer::for_test(
            ACTION_KEY,
            tmp.path().join("action-auth"),
            ACTION_NOW,
        ));
        let mut worker = VoiceProvisionWorker::new(tmp.path().to_path_buf(), "peer:leader".into())
            .with_db_path(tmp.path().join("voice.sqlite"))
            .with_authorizer(Arc::clone(&authorizer));

        let config_unsigned = serde_json::json!({
            "schema_version": 1,
            "caller_id": "15551110000",
            "outbound_trunk": "shared"
        })
        .to_string();
        assert!(worker
            .accept_action(SHARED_CONFIG_TOPIC, Some(&config_unsigned))
            .is_err());
        assert!(worker.desired_shared_config.is_none());
        let config_armed = authorize_test_body(
            ACTION_KEY,
            &config_unsigned,
            MutationContext {
                verb: VOICE_SHARED_CONFIG_AUTH_VERB,
                node: VOICE_AUTH_NODE,
                target: "fleet",
            },
            "voice-config-once",
            ACTION_NOW + 30_000,
        );
        assert!(worker
            .accept_action(SHARED_CONFIG_TOPIC, Some(&config_armed))
            .unwrap());
        assert_eq!(
            worker.desired_shared_config.as_ref().unwrap().caller_id,
            "15551110000"
        );

        let failover_unsigned = serde_json::json!({
            "schema_version": 1,
            "node_id": "peer:eagle",
            "policy": "Voicemail"
        })
        .to_string();
        assert!(worker
            .accept_action(FAILOVER_TOPIC, Some(&failover_unsigned))
            .is_err());
        assert!(worker.desired_failover.is_empty());
        let failover_armed = authorize_test_body(
            ACTION_KEY,
            &failover_unsigned,
            MutationContext {
                verb: VOICE_FAILOVER_AUTH_VERB,
                node: VOICE_AUTH_NODE,
                target: "peer:eagle",
            },
            "voice-failover-once",
            ACTION_NOW + 30_000,
        );
        assert!(worker
            .accept_action(FAILOVER_TOPIC, Some(&failover_armed))
            .unwrap());
        assert_eq!(
            worker.desired_failover.get("peer:eagle"),
            Some(&FailoverPolicy::Voicemail)
        );
        let mut restarted =
            VoiceProvisionWorker::new(tmp.path().to_path_buf(), "peer:leader".into())
                .with_db_path(tmp.path().join("voice.sqlite"))
                .with_authorizer(authorizer);
        restarted.load_authorized_intents();
        assert_eq!(
            restarted
                .desired_shared_config
                .as_ref()
                .unwrap()
                .outbound_trunk,
            "shared"
        );
        assert_eq!(
            restarted.desired_failover.get("peer:eagle"),
            Some(&FailoverPolicy::Voicemail)
        );
    }

    #[cfg(unix)]
    #[test]
    fn journal_ignores_group_readable_state() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("voice.sqlite");
        let path = db.with_file_name(AUTHORIZED_INTENTS_FILE);
        let state = AuthorizedVoiceIntents {
            schema_version: AUTHORIZED_INTENTS_SCHEMA_VERSION,
            did_routes: BTreeMap::from([("15551234567".to_string(), Some("peer:eagle".into()))]),
            ..AuthorizedVoiceIntents::default()
        };
        std::fs::write(&path, serde_json::to_vec(&state).unwrap()).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        let mut worker = VoiceProvisionWorker::new(tmp.path().to_path_buf(), "peer:leader".into())
            .with_db_path(db);
        worker.load_authorized_intents();
        assert!(worker.desired_did_routes.is_empty());
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

    // ── lock 11: the pure DID-route diff ──

    fn username_map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(id, u)| ((*id).to_string(), (*u).to_string()))
            .collect()
    }

    #[test]
    fn plan_did_routes_routes_an_unrouted_did_to_the_node() {
        let routes = vec![DesiredDidRoute {
            did: "15551234567".into(),
            node_id: Some("peer:eagle".into()),
        }];
        let map = username_map(&[("peer:eagle", "eagle")]);
        let actual = vec![Did {
            number: "15551234567".into(),
            routed_to: None,
        }];
        assert_eq!(
            plan_did_routes(&routes, &map, &actual),
            vec![DidAction::Route {
                did: "15551234567".into(),
                username: "eagle".into(),
            }]
        );
    }

    #[test]
    fn plan_did_routes_is_noop_when_already_routed() {
        // Idempotent (lock 19): a DID already at the desired target → no action.
        let routes = vec![DesiredDidRoute {
            did: "15551234567".into(),
            node_id: Some("peer:eagle".into()),
        }];
        let map = username_map(&[("peer:eagle", "eagle")]);
        let actual = vec![Did {
            number: "15551234567".into(),
            routed_to: Some("eagle".into()),
        }];
        assert!(plan_did_routes(&routes, &map, &actual).is_empty());
    }

    #[test]
    fn plan_did_routes_unroutes_back_to_main() {
        let routes = vec![DesiredDidRoute {
            did: "15551234567".into(),
            node_id: None,
        }];
        let actual = vec![Did {
            number: "15551234567".into(),
            routed_to: Some("eagle".into()),
        }];
        assert_eq!(
            plan_did_routes(&routes, &HashMap::new(), &actual),
            vec![DidAction::Unroute {
                did: "15551234567".into(),
            }]
        );
    }

    #[test]
    fn plan_did_routes_never_touches_a_did_the_account_does_not_own() {
        // Lock 11: routing a DID absent from the master inventory is skipped —
        // we never invent / provision a new DID.
        let routes = vec![DesiredDidRoute {
            did: "19999999999".into(),
            node_id: Some("peer:eagle".into()),
        }];
        let map = username_map(&[("peer:eagle", "eagle")]);
        assert!(plan_did_routes(&routes, &map, &[]).is_empty());
    }

    #[test]
    fn plan_did_routes_skips_an_unprovisioned_target() {
        // The node has no resolvable username yet → wait, don't route.
        let routes = vec![DesiredDidRoute {
            did: "15551234567".into(),
            node_id: Some("peer:new".into()),
        }];
        let actual = vec![Did {
            number: "15551234567".into(),
            routed_to: None,
        }];
        assert!(plan_did_routes(&routes, &HashMap::new(), &actual).is_empty());
    }

    // ── lock 10: the failover plan ──

    #[test]
    fn plan_failover_resolves_usernames_and_skips_unknown_nodes() {
        let desired = vec![
            DesiredFailover {
                node_id: "peer:eagle".into(),
                policy: FailoverPolicy::Voicemail,
            },
            DesiredFailover {
                node_id: "peer:ghost".into(),
                policy: FailoverPolicy::None,
            },
        ];
        let map = username_map(&[("peer:eagle", "eagle")]);
        let ops = plan_failover(&desired, &map);
        assert_eq!(
            ops,
            vec![(
                "eagle".to_string(),
                FailoverPolicy::Voicemail,
                "peer:eagle".to_string()
            )]
        );
    }

    // ── the full reconcile: DID routing + failover applied + published ──

    #[test]
    fn reconcile_routes_a_did_and_publishes_the_mapping() {
        let tmp = tempfile::tempdir().unwrap();
        let store = seeded_store(tmp.path());
        // A master DID exists (lock 11 — pre-owned, not provisioned).
        let client = FakeVitelityClient::new("sip.vitelity.net").with_did("15551234567", None);
        let desired = vec![node("peer:eagle", "eagle")];
        let routes = vec![DesiredDidRoute {
            did: "15551234567".into(),
            node_id: Some("peer:eagle".into()),
        }];

        let outcome = reconcile_once(&client, &store, &desired, &routes, &[], DEFAULT_REALM);
        // The node's row shows the real routed DID.
        assert_eq!(
            outcome.states[0].routed_dids,
            vec!["15551234567".to_string()]
        );
        // The published inventory reflects the applied route.
        assert_eq!(outcome.dids.len(), 1);
        assert_eq!(outcome.dids[0].routed_to, Some("eagle".to_string()));
        // Vitelity actually recorded the route (no new DID created).
        assert_eq!(client.list_dids().unwrap().len(), 1);
    }

    #[test]
    fn reconcile_reapplies_drifted_did_routing() {
        // Acceptance: the reconcile re-applies a DID whose routing drifted away.
        let tmp = tempfile::tempdir().unwrap();
        let store = seeded_store(tmp.path());
        // The DID drifted to the wrong sub-account.
        let client = FakeVitelityClient::new("sip.vitelity.net")
            .with_did("15551234567", Some("someone-else".into()));
        let desired = vec![node("peer:eagle", "eagle")];
        let routes = vec![DesiredDidRoute {
            did: "15551234567".into(),
            node_id: Some("peer:eagle".into()),
        }];
        let outcome = reconcile_once(&client, &store, &desired, &routes, &[], DEFAULT_REALM);
        assert_eq!(outcome.dids[0].routed_to, Some("eagle".to_string()));
        assert_eq!(
            outcome.states[0].routed_dids,
            vec!["15551234567".to_string()]
        );
    }

    #[test]
    fn reconcile_sets_and_publishes_failover_policy() {
        let tmp = tempfile::tempdir().unwrap();
        let store = seeded_store(tmp.path());
        let client = FakeVitelityClient::new("sip.vitelity.net");
        let desired = vec![node("peer:eagle", "eagle")];
        let failover = vec![DesiredFailover {
            node_id: "peer:eagle".into(),
            policy: FailoverPolicy::Forward {
                number: "15550001111".into(),
            },
        }];
        let outcome = reconcile_once(&client, &store, &desired, &[], &failover, DEFAULT_REALM);
        assert_eq!(
            outcome.states[0].failover,
            Some(FailoverPolicy::Forward {
                number: "15550001111".into()
            })
        );
        // Vitelity recorded it against the sub-account username.
        assert_eq!(
            client.failover_of("eagle"),
            Some(FailoverPolicy::Forward {
                number: "15550001111".into()
            })
        );
    }

    #[test]
    fn reconcile_does_not_provision_a_new_did() {
        // Acceptance: no new DID is ever created — with an empty inventory a
        // route intent is a no-op (route-existing only, lock 11).
        let tmp = tempfile::tempdir().unwrap();
        let store = seeded_store(tmp.path());
        let client = FakeVitelityClient::new("sip.vitelity.net");
        let desired = vec![node("peer:eagle", "eagle")];
        let routes = vec![DesiredDidRoute {
            did: "15551234567".into(),
            node_id: Some("peer:eagle".into()),
        }];
        let outcome = reconcile_once(&client, &store, &desired, &routes, &[], DEFAULT_REALM);
        assert!(outcome.dids.is_empty(), "no DID must be invented");
        assert!(outcome.states[0].routed_dids.is_empty());
    }

    #[test]
    fn did_route_state_serializes_for_the_panel() {
        // The published body carries the routed DID + failover so the panel's
        // live columns render (lock 9/10/11).
        let st = NodeVoiceState {
            node_id: "peer:eagle".into(),
            hostname: "eagle".into(),
            username: "eagle".into(),
            sip_uri: "eagle@sip.vitelity.net".into(),
            reg_state: RegState::Registered,
            routed_dids: vec!["15551234567".into()],
            failover: Some(FailoverPolicy::Voicemail),
            updated_at_s: 1,
        };
        let body = serde_json::to_string(&st).unwrap();
        assert!(body.contains("15551234567"));
        assert!(body.contains("routed_dids"));
        assert!(body.contains("Voicemail"));
    }

    // ── the Bus intent readers (desired side of lock 10 + 11) ──

    #[test]
    fn read_desired_did_routes_folds_latest_wins() {
        let tmp = tempfile::tempdir().unwrap();
        let persist = Persist::open(tmp.path().to_path_buf()).unwrap();
        persist
            .write(
                DID_ROUTE_TOPIC,
                Priority::Default,
                None,
                Some(r#"{"did":"15551234567","node_id":"peer:eagle"}"#),
            )
            .unwrap();
        // A later message re-points the same DID → it wins.
        persist
            .write(
                DID_ROUTE_TOPIC,
                Priority::Default,
                None,
                Some(r#"{"did":"15551234567","node_id":"peer:pine"}"#),
            )
            .unwrap();
        let routes = read_desired_did_routes(&persist);
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].did, "15551234567");
        assert_eq!(routes[0].node_id, Some("peer:pine".to_string()));
    }

    #[test]
    fn read_desired_failover_folds_latest_wins() {
        let tmp = tempfile::tempdir().unwrap();
        let persist = Persist::open(tmp.path().to_path_buf()).unwrap();
        persist
            .write(
                FAILOVER_TOPIC,
                Priority::Default,
                None,
                Some(r#"{"node_id":"peer:eagle","policy":"Voicemail"}"#),
            )
            .unwrap();
        persist
            .write(
                FAILOVER_TOPIC,
                Priority::Default,
                None,
                Some(r#"{"node_id":"peer:eagle","policy":{"Forward":{"number":"15550001111"}}}"#),
            )
            .unwrap();
        let failover = read_desired_failover(&persist);
        assert_eq!(failover.len(), 1);
        assert_eq!(
            failover[0].policy,
            FailoverPolicy::Forward {
                number: "15550001111".into()
            }
        );
    }

    // ── VOIP-GW-7: the hard-cutover migration (locks 13 + 18) ──

    fn voice_state(node_id: &str, host: &str, reg: RegState) -> NodeVoiceState {
        NodeVoiceState {
            node_id: node_id.to_string(),
            hostname: host.to_string(),
            username: sub_account_username(host),
            sip_uri: String::new(),
            reg_state: reg,
            routed_dids: Vec::new(),
            failover: None,
            updated_at_s: 0,
        }
    }

    #[test]
    fn cutover_phase_walks_legacy_to_complete() {
        // Not lifted → Legacy, regardless of node counts.
        assert_eq!(cutover_phase(false, 3, 3), CutoverPhase::Legacy);
        // Lifted but nothing reprovisioned → LiftedSharedOutbound (outbound alive).
        assert_eq!(
            cutover_phase(true, 3, 0),
            CutoverPhase::LiftedSharedOutbound
        );
        // Lifted, no nodes yet → still LiftedSharedOutbound (never a fake done).
        assert_eq!(
            cutover_phase(true, 0, 0),
            CutoverPhase::LiftedSharedOutbound
        );
        // Partway → NodesReprovisioning.
        assert_eq!(cutover_phase(true, 3, 1), CutoverPhase::NodesReprovisioning);
        // All crossed over → CutoverComplete.
        assert_eq!(cutover_phase(true, 3, 3), CutoverPhase::CutoverComplete);
    }

    #[test]
    fn derive_cutover_status_lists_the_pending_nodes() {
        // One node reprovisioned (Unregistered = provisioned), one still awaiting
        // (Provisioning), one failing (Error) → two remain, named by hostname.
        let states = vec![
            voice_state("peer:eagle", "eagle", RegState::Unregistered),
            voice_state("peer:pine", "pine", RegState::Provisioning),
            voice_state(
                "peer:oak",
                "oak",
                RegState::Error {
                    reason: "boom".into(),
                },
            ),
        ];
        let derived = derive_cutover_status(&states, true);
        assert_eq!(derived.phase, CutoverPhase::NodesReprovisioning);
        assert_eq!(derived.total_nodes, 3);
        assert_eq!(derived.reprovisioned, 1);
        assert_eq!(derived.pending_nodes, vec!["pine", "oak"]);
        assert!(derived.shared_outbound_lifted);
    }

    #[test]
    fn no_node_left_dual_model_never_completes_with_a_pending_node() {
        // The invariant (lock 18): a mixed fleet is NEVER reported CutoverComplete
        // while any node is still legacy — no node is left straddling both models.
        let mixed = derive_cutover_status(
            &[
                voice_state("peer:eagle", "eagle", RegState::Registered),
                voice_state("peer:pine", "pine", RegState::Provisioning),
            ],
            true,
        );
        assert_ne!(mixed.phase, CutoverPhase::CutoverComplete);
        assert!(no_node_left_dual_model(&mixed));

        // Every node reprovisioned → complete, and the invariant holds.
        let done = derive_cutover_status(
            &[
                voice_state("peer:eagle", "eagle", RegState::Registered),
                voice_state("peer:pine", "pine", RegState::Unregistered),
            ],
            true,
        );
        assert_eq!(done.phase, CutoverPhase::CutoverComplete);
        assert!(no_node_left_dual_model(&done));
        assert!(done.pending_nodes.is_empty());

        // A hand-built inconsistent status (Complete but a node pending) is caught
        // by the invariant — the guard the acceptance turns on.
        let bogus = CutoverStatus {
            phase: CutoverPhase::CutoverComplete,
            total_nodes: 2,
            reprovisioned: 1,
            pending_nodes: vec!["pine".into()],
            shared_outbound_lifted: true,
            updated_at_s: 0,
        };
        assert!(!no_node_left_dual_model(&bogus));
    }

    #[test]
    fn read_desired_shared_config_folds_latest_wins() {
        let tmp = tempfile::tempdir().unwrap();
        let persist = Persist::open(tmp.path().to_path_buf()).unwrap();
        persist
            .write(
                SHARED_CONFIG_TOPIC,
                Priority::Default,
                None,
                Some(r#"{"caller_id":"15551110000","outbound_trunk":"old"}"#),
            )
            .unwrap();
        // A later apply supersedes the earlier one.
        persist
            .write(
                SHARED_CONFIG_TOPIC,
                Priority::Default,
                None,
                Some(r#"{"caller_id":"15559990000","outbound_trunk":"shared-vitelity"}"#),
            )
            .unwrap();
        let cfg = read_desired_shared_config(&persist).expect("a config was applied");
        assert_eq!(cfg.caller_id, "15559990000");
        assert_eq!(cfg.outbound_trunk, "shared-vitelity");
    }

    #[test]
    fn apply_shared_config_persists_the_lifted_config_and_marks_lifted() {
        // The panel's "Apply to fleet" round-trip: the verb is consumed and the
        // shared-outbound is sealed to the leader store — the fleet is now lifted.
        let tmp = tempfile::tempdir().unwrap();
        let store = seeded_store(tmp.path());
        let bus = tempfile::tempdir().unwrap();
        let persist = Persist::open(bus.path().to_path_buf()).unwrap();
        // Before any apply: not lifted → the cutover is Legacy.
        assert!(!shared_outbound_is_lifted(&store));

        persist
            .write(
                SHARED_CONFIG_TOPIC,
                Priority::Default,
                None,
                Some(r#"{"caller_id":"15551234567","outbound_trunk":"shared"}"#),
            )
            .unwrap();
        let lifted = apply_shared_config(&store, &persist);
        assert!(lifted, "applying the shared config lifts the fleet");

        // The sealed config reads back byte-consistent (lock 13).
        let body = store.get(&shared_outbound_ref()).unwrap().unwrap();
        let cfg: SharedOutboundConfig = serde_json::from_str(&body).unwrap();
        assert_eq!(cfg.caller_id, "15551234567");
        assert_eq!(cfg.outbound_trunk, "shared");
    }

    #[test]
    fn cutover_status_serializes_for_the_panel() {
        // The published body carries the phase (kebab-case) + the pending nodes so
        // the panel banner renders the flag-day prompt.
        let status = CutoverStatus {
            phase: CutoverPhase::NodesReprovisioning,
            total_nodes: 2,
            reprovisioned: 1,
            pending_nodes: vec!["pine".into()],
            shared_outbound_lifted: true,
            updated_at_s: 7,
        };
        let body = serde_json::to_string(&status).unwrap();
        assert!(body.contains("\"phase\":\"nodes-reprovisioning\""));
        assert!(body.contains("pine"));
        // And it round-trips back (the panel mirror deserialises the same shape).
        let back: CutoverStatus = serde_json::from_str(&body).unwrap();
        assert_eq!(back, status);
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
