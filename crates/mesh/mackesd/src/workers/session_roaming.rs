//! E12-8 — `session_roaming`: the **roaming + persistence policy** over the
//! E12-5b [`super::session_broker`]'s VDI sessions.
//!
//! Where [`super::session_broker`] *tracks* which peer serves which VM to which
//! client and converges that roster into the shared plane, this worker adds the
//! **policy that makes a user's desktops follow them** to any Workstation and
//! **survive a disconnect**:
//!
//! - **Roaming ("desktops follow me"):** when a user arrives at a new Workstation,
//!   their persisted sessions are re-pointed at the arriving peer and reconnected —
//!   the serving VM keeps running where it was, only the driving client moves. The
//!   pure decision is [`reconcile_roaming`] (persisted → live-here diff), the same
//!   shape as [`super::scheduler::replace_decisions`].
//! - **Multi-monitor layout persistence:** a user's [`MonitorLayout`] (which VM
//!   surface lands on which monitor, with geometry) is persisted per-peer so the
//!   arrangement is restored on the arriving Workstation. A [`RoamingState`] bundles
//!   a user's open sessions + their layout under the per-peer key.
//! - **Disconnect policy:** [`on_disconnect`] honors a per-VM [`DisconnectPolicy`] —
//!   the default [`DisconnectPolicy::KeepRunning`] leaves the VM running and the
//!   session reconnectable (design lock 5), while `Suspend` / `Shutdown` are honored
//!   when set.
//! - **Reconnect on node loss:** [`on_node_loss`] holds a session whose serving node
//!   left the mesh in a reconnectable `Disconnected` state (never `Closed`) so a
//!   later reconnect returns it to `Active` once the scheduler re-places the VM.
//!
//! ## Shape (mirrors [`super::session_broker`] / [`super::scheduler`])
//!
//! - The **pure core** is fully unit-tested with no bus and no clock: the policy fns
//!   ([`reconcile_roaming`], [`on_disconnect`], [`on_node_loss`], [`roam_to`]) and
//!   the whole-tick composer [`plan_roaming`] take `now_ms` (the crate forbids
//!   ambient time on these paths, exactly as the broker's state machine does).
//! - **The session persistence reuses the broker's [`SessionStore`] seam verbatim**
//!   (production [`MeshSessionStore`], integration-gated) — this worker never
//!   invents a parallel session model. The [`MonitorLayout`] rides a tiny companion
//!   [`LayoutStore`] gated by the very same [`SessionStoreError::IntegrationGated`],
//!   so the live cross-peer persist stays honest (§7) until the etcd writer lands.
//! - **Leader-gated** on the shared `.mackesd-leader.lock` (the same
//!   [`crate::leader`] election [`super::session_broker`] / `dc_auditor` use): every
//!   node folds the roaming request log, but only the elected node writes the shared
//!   plane, so an N-node mesh doesn't multi-write.
//!
//! ## Reused types (no parallel model — §6 glue)
//!
//! The session itself is the broker's [`VdiSession`] (its [`SessionState`] machine,
//! its [`SessionId`] / [`VmId`] / [`NodeId`] identities, and its [`SessionStore`] /
//! [`MeshSessionStore`] persistence) — imported, never redefined.

#![cfg(feature = "async-services")]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use mde_bus::persist::Persist;

use super::scheduler::NodeId;
use super::session_broker::{
    close_session, mark_active, mark_disconnected, MeshSessionStore, SessionId, SessionState,
    SessionStore, SessionStoreError, VdiSession, VmId,
};
use super::{ShutdownToken, Worker};

/// Bus topic the worker drains for roaming-policy requests (arrive / set-policy /
/// save-layout).
///
/// Distinct from the broker's `action/vdi/session` lifecycle lane — this is the
/// *policy* plane over those sessions.
pub const ACTION_TOPIC: &str = "action/vdi/roaming";

/// Convergence cadence — the same human-paced 2 s poll the broker / scheduler drain
/// at (the bus read is a cheap local log scan).
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(2);

// ───────────────────────────── monitor layout ─────────────────────────────

/// A rectangle on the virtual desktop, in pixels — where a VM surface lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MonitorGeometry {
    /// Left edge (px) on the virtual desktop; may be negative for a left monitor.
    pub x: i32,
    /// Top edge (px) on the virtual desktop.
    pub y: i32,
    /// Surface width in px.
    pub width: u32,
    /// Surface height in px.
    pub height: u32,
}

impl MonitorGeometry {
    /// A geometry at `(x, y)` sized `width` × `height`.
    #[must_use]
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// The surface area in px² (`width` × `height`), saturating on overflow.
    #[must_use]
    pub const fn area(self) -> u64 {
        (self.width as u64).saturating_mul(self.height as u64)
    }
}

/// One monitor's placement of a session's VM surface: which [`SessionId`] /
/// [`VmId`] shows on monitor `monitor`, with what geometry.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MonitorAssignment {
    /// The 0-based monitor index this surface is shown on.
    pub monitor: u32,
    /// The session whose desktop is shown here (a broker [`SessionId`]).
    pub session_id: SessionId,
    /// The target VM surface (the broker [`VmId`] — the libvirt UUID).
    pub vm_id: VmId,
    /// Where the surface sits on the virtual desktop.
    pub geometry: MonitorGeometry,
    /// Whether this is the primary monitor.
    pub primary: bool,
}

/// A user's multi-monitor layout: which VM surface lands on which monitor, with
/// geometry.
///
/// The per-surface mapping a roaming user carries between Workstations. Plain
/// serde — persisted through the [`LayoutStore`] seam.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MonitorLayout {
    /// One assignment per shown VM surface. Ordering is not significant; lookups
    /// go through the helper methods.
    pub assignments: Vec<MonitorAssignment>,
}

impl MonitorLayout {
    /// The empty layout (no surfaces mapped yet).
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            assignments: Vec::new(),
        }
    }

    /// Build a layout from a set of assignments.
    #[must_use]
    pub const fn new(assignments: Vec<MonitorAssignment>) -> Self {
        Self { assignments }
    }

    /// How many surfaces the layout maps.
    #[must_use]
    pub fn surface_count(&self) -> usize {
        self.assignments.len()
    }

    /// The assignment shown on `monitor`, if any.
    #[must_use]
    pub fn on_monitor(&self, monitor: u32) -> Option<&MonitorAssignment> {
        self.assignments.iter().find(|a| a.monitor == monitor)
    }

    /// The primary-monitor assignment, if one is flagged.
    #[must_use]
    pub fn primary(&self) -> Option<&MonitorAssignment> {
        self.assignments.iter().find(|a| a.primary)
    }
}

// ───────────────────────────── roaming state ─────────────────────────────

/// A user's roaming desktop state, keyed per-peer: their open sessions plus the
/// [`MonitorLayout`] that arranges those sessions' surfaces.
///
/// Reuses the broker's [`VdiSession`] verbatim (no parallel session model) and is
/// plain serde so it persists through the reused session plane + the companion
/// [`LayoutStore`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RoamingState {
    /// The peer these desktops are driven from — the roaming key (a client
    /// Workstation [`NodeId`]). After a roam this is the arriving Workstation.
    pub client_peer: NodeId,
    /// The user's open sessions driven from `client_peer` (reused [`VdiSession`]s).
    pub sessions: Vec<VdiSession>,
    /// The multi-monitor layout arranging those sessions' surfaces.
    pub layout: MonitorLayout,
}

impl RoamingState {
    /// Assemble the roaming state for `client_peer` from the full session plane +
    /// the persisted `layout`: the open (publishable) sessions driven from that peer.
    #[must_use]
    pub fn assemble(
        client_peer: NodeId,
        all_sessions: &[VdiSession],
        layout: MonitorLayout,
    ) -> Self {
        let sessions = all_sessions
            .iter()
            .filter(|s| s.client_peer == client_peer && s.state.is_publishable())
            .cloned()
            .collect();
        Self {
            client_peer,
            sessions,
            layout,
        }
    }

    /// The "desktops follow me" transform: re-key this state onto `workstation`,
    /// re-point + reconnect every session there ([`roam_to`]), and carry the layout
    /// so the arrangement is restored on the arriving peer. The serving VMs keep
    /// running where they were.
    #[must_use]
    pub fn roam_to(&self, workstation: &NodeId, now_ms: u64) -> Self {
        Self {
            client_peer: workstation.clone(),
            sessions: roam_to(&self.sessions, workstation, now_ms),
            layout: self.layout.clone(),
        }
    }
}

// ───────────────────────────── disconnect policy ─────────────────────────────

/// The per-VM policy applied when a session's client link drops.
///
/// The default [`DisconnectPolicy::KeepRunning`] realizes design lock 5 (a
/// disconnected VM keeps running and is reconnectable); `Suspend` / `Shutdown` are
/// honored when an operator/user sets them for a specific VM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisconnectPolicy {
    /// Default — the VM keeps running; the session goes `Disconnected` and a later
    /// reconnect returns it to `Active`.
    #[default]
    KeepRunning,
    /// Suspend the VM on disconnect (frees compute; fast resume on reconnect). The
    /// session stays reconnectable (`Disconnected`).
    Suspend,
    /// Shut the VM down on disconnect — the session ends (`Closed`).
    Shutdown,
}

/// The outcome of applying a [`DisconnectPolicy`] to a session — carries the
/// resulting [`VdiSession`] and encodes which policy fired.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisconnectOutcome {
    /// The VM keeps running; the session is now `Disconnected` (reconnectable).
    KeepRunning {
        /// The session after the transition.
        session: VdiSession,
    },
    /// The VM is suspended; the session is `Disconnected` (reconnectable).
    Suspend {
        /// The session after the transition.
        session: VdiSession,
    },
    /// The VM is shut down; the session is `Closed` (not reconnectable).
    Shutdown {
        /// The session after the transition.
        session: VdiSession,
    },
}

impl DisconnectOutcome {
    /// The resulting session, regardless of which policy fired.
    #[must_use]
    pub const fn session(&self) -> &VdiSession {
        match self {
            Self::KeepRunning { session }
            | Self::Suspend { session }
            | Self::Shutdown { session } => session,
        }
    }

    /// Whether the desktop can still be reconnected (`KeepRunning` / `Suspend` keep
    /// the session reconnectable; `Shutdown` ends it).
    #[must_use]
    pub const fn reconnectable(&self) -> bool {
        matches!(self, Self::KeepRunning { .. } | Self::Suspend { .. })
    }

    /// Whether the serving VM keeps running after the disconnect (only
    /// `KeepRunning` — `Suspend` freezes it, `Shutdown` stops it).
    #[must_use]
    pub const fn vm_kept_running(&self) -> bool {
        matches!(self, Self::KeepRunning { .. })
    }
}

/// The disconnect policy decision: apply `policy` to `session` when its client link
/// drops.
///
/// `KeepRunning` / `Suspend` move the session to `Disconnected` (via the broker's
/// [`mark_disconnected`]); `Shutdown` closes it (via [`close_session`]). A session
/// the state machine won't disconnect (never-connected `Requested`) is carried
/// unchanged under the keep/suspend outcomes — the VM directive still holds. Pure +
/// clock-passed, like the broker's state machine.
#[must_use]
pub fn on_disconnect(
    session: &VdiSession,
    policy: DisconnectPolicy,
    now_ms: u64,
) -> DisconnectOutcome {
    match policy {
        DisconnectPolicy::KeepRunning => DisconnectOutcome::KeepRunning {
            session: mark_disconnected(session, now_ms).unwrap_or_else(|_| session.clone()),
        },
        DisconnectPolicy::Suspend => DisconnectOutcome::Suspend {
            session: mark_disconnected(session, now_ms).unwrap_or_else(|_| session.clone()),
        },
        DisconnectPolicy::Shutdown => DisconnectOutcome::Shutdown {
            session: close_session(session, now_ms),
        },
    }
}

// ───────────────────────────── roaming actions ─────────────────────────────

/// One roaming-policy step the leader applies to the shared session plane through
/// the reused [`SessionStore`] seam — the roaming analogue of the broker's
/// `SessionAction`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoamingAction {
    /// Re-open (roam) a session onto the arriving Workstation: the VM keeps running
    /// on its `serving_peer`; the `client_peer` is re-pointed and the session
    /// reconnects to `Active`. Applied through the store as a `publish`.
    Reopen(VdiSession),
    /// Hold a session open across the loss of its serving node — marked
    /// `Disconnected` (reconnectable), never `Closed`, so a reconnect returns it to
    /// `Active` once the scheduler re-places the VM. Applied as a `publish`.
    HoldForReconnect(VdiSession),
    /// Release a session id from the plane (roamed away, or ended). Applied through
    /// the store as a `remove`.
    Release(SessionId),
}

impl RoamingAction {
    /// The session id this action targets — the de-dup key when composing a tick's
    /// actions (so one session gets at most one convergence write per tick).
    #[must_use]
    pub fn session_id(&self) -> SessionId {
        match self {
            Self::Reopen(s) | Self::HoldForReconnect(s) => s.id.clone(),
            Self::Release(id) => id.clone(),
        }
    }

    /// Apply this action through the reused session-plane `store`.
    ///
    /// # Errors
    /// A [`SessionStoreError`] — `IntegrationGated` until the live etcd writer lands,
    /// else `Failed`.
    pub fn apply(&self, store: &dyn SessionStore) -> Result<(), SessionStoreError> {
        match self {
            Self::Reopen(s) | Self::HoldForReconnect(s) => store.publish(s),
            Self::Release(id) => store.remove(id),
        }
    }
}

// ───────────────────────────── pure: policy ─────────────────────────────

/// Strip the `peer:` node-id prefix to the bare hostname — the same namespace
/// reconciliation [`super::scheduler`] does, so a session's `serving_peer`
/// (`peer:<host>`) compares against the mesh directory's bare hostnames.
fn bare_host(id: &str) -> &str {
    id.strip_prefix("peer:").unwrap_or(id)
}

/// Roam one session onto `arriving_at`: re-point its `client_peer` and reconnect it
/// to `Active` (the VM identity — `serving_peer` / `vm_id` — is untouched, so the VM
/// keeps running). `None` for a terminal (`Closed`) session — there's nothing to
/// roam.
fn roam_one(session: &VdiSession, arriving_at: &NodeId, now_ms: u64) -> Option<VdiSession> {
    if session.state.is_terminal() {
        return None;
    }
    let repointed = VdiSession {
        client_peer: arriving_at.clone(),
        ..session.clone()
    };
    // From any non-terminal state `mark_active` succeeds — the reconnect.
    mark_active(&repointed, now_ms).ok()
}

/// The "desktops follow me" transform: re-point + reconnect every non-terminal
/// session in `sessions` onto `arriving_at`. Terminal sessions are dropped. Pure +
/// clock-passed.
#[must_use]
pub fn roam_to(sessions: &[VdiSession], arriving_at: &NodeId, now_ms: u64) -> Vec<VdiSession> {
    sessions
        .iter()
        .filter_map(|s| roam_one(s, arriving_at, now_ms))
        .collect()
}

/// The pure roaming reconcile — the "desktops follow me" decision when a user
/// arrives at a new Workstation.
///
/// The minimal [`RoamingAction`] set that makes the `observed` live-here plane
/// match the `desired` (persisted, already roamed) sessions. Mirrors
/// [`super::scheduler::replace_decisions`] / [`super::session_broker::reconcile`]:
///
/// - A publishable desired session absent-or-different in `observed` is `Reopen`ed;
///   an already-identical one is left alone (no needless write).
/// - A terminal desired session still lingering in `observed` is `Release`d.
/// - An `observed` id no longer desired here is `Release`d (roamed away / stale).
///
/// Deterministic (id-sorted scans) and clock-free.
#[must_use]
pub fn reconcile_roaming(
    desired: &[VdiSession],
    observed: &BTreeMap<SessionId, VdiSession>,
) -> Vec<RoamingAction> {
    let desired_by_id: BTreeMap<SessionId, &VdiSession> =
        desired.iter().map(|s| (s.id.clone(), s)).collect();
    let mut out = Vec::new();
    for (id, d) in &desired_by_id {
        if d.state.is_publishable() {
            if observed.get(id) != Some(*d) {
                out.push(RoamingAction::Reopen((*d).clone()));
            }
        } else if observed.contains_key(id) {
            out.push(RoamingAction::Release(id.clone()));
        }
    }
    for id in observed.keys() {
        if !desired_by_id.contains_key(id) {
            out.push(RoamingAction::Release(id.clone()));
        }
    }
    out
}

/// The reconnect-on-node-loss decision: hold a session reconnectable when its
/// `serving_peer` leaves the mesh.
///
/// For each session whose `serving_peer` is **not** in the `live` node set, hold it
/// reconnectable rather than dropping it — a connected session
/// ([`SessionState::Active`] / `Disconnected`) is marked `Disconnected`
/// ([`RoamingAction::HoldForReconnect`]) so a reconnect resumes it once the
/// scheduler re-places the VM, while a never-connected `Requested` session (nothing
/// to reconnect to) is `Release`d. Sessions on a live node — and terminal sessions —
/// are left alone. Mirrors [`super::scheduler::replace_decisions`]'s
/// live-filter; pure + clock-passed.
#[must_use]
pub fn on_node_loss(
    sessions: &[VdiSession],
    live: &BTreeSet<NodeId>,
    now_ms: u64,
) -> Vec<RoamingAction> {
    let live_bare: BTreeSet<&str> = live.iter().map(|n| bare_host(n)).collect();
    let mut out = Vec::new();
    for s in sessions {
        if s.state.is_terminal() {
            continue;
        }
        if live_bare.contains(bare_host(&s.serving_peer)) {
            continue; // serving node still alive — the session keeps running
        }
        match mark_disconnected(s, now_ms) {
            Ok(held) => out.push(RoamingAction::HoldForReconnect(held)),
            // A never-connected session on a lost node can't be reconnected.
            Err(_) => out.push(RoamingAction::Release(s.id.clone())),
        }
    }
    out
}

/// Compose one convergence tick's full [`RoamingAction`] set from the folded policy
/// inputs over the `observed` shared plane.
///
/// Roaming arrivals ([`reconcile_roaming`] over [`roam_to`]) + the per-VM disconnect
/// policy ([`on_disconnect`]) + node-loss holds ([`on_node_loss`]). De-duplicated by
/// session id (last decision wins: node-loss over disconnect-policy over roaming) so
/// each session gets at most one write per tick, and deterministic (id-sorted). Pure
/// — the worker's `converge` applies the result through the store.
#[must_use]
pub fn plan_roaming(
    arrivals: &BTreeMap<NodeId, NodeId>,
    policies: &BTreeMap<VmId, DisconnectPolicy>,
    observed: &BTreeMap<SessionId, VdiSession>,
    live: &BTreeSet<NodeId>,
    now_ms: u64,
) -> Vec<RoamingAction> {
    let mut by_id: BTreeMap<SessionId, RoamingAction> = BTreeMap::new();
    // 1. Roaming: for each arrival, roam the sessions driven from `from` onto
    //    `workstation` and reconcile them into the plane.
    for (from, workstation) in arrivals {
        let user_obs: BTreeMap<SessionId, VdiSession> = observed
            .iter()
            .filter(|(_, s)| &s.client_peer == from)
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let user_sessions: Vec<VdiSession> = user_obs.values().cloned().collect();
        let desired = roam_to(&user_sessions, workstation, now_ms);
        for action in reconcile_roaming(&desired, &user_obs) {
            by_id.insert(action.session_id(), action);
        }
    }
    // 2. Disconnect policy: a Shutdown-policy VM whose session is Disconnected ends
    //    it (Release). KeepRunning / Suspend keep it reconnectable (no plane write).
    for s in observed.values() {
        if s.state == SessionState::Disconnected {
            let policy = policies.get(&s.vm_id).copied().unwrap_or_default();
            if let DisconnectOutcome::Shutdown { session } = on_disconnect(s, policy, now_ms) {
                by_id.insert(session.id.clone(), RoamingAction::Release(session.id));
            }
        }
    }
    // 3. Node loss: hold sessions whose serving node left the mesh.
    let all: Vec<VdiSession> = observed.values().cloned().collect();
    for action in on_node_loss(&all, live, now_ms) {
        by_id.insert(action.session_id(), action);
    }
    by_id.into_values().collect()
}

// ───────────────────────────── layout store seam ─────────────────────────────

/// The injectable per-peer [`MonitorLayout`] persistence seam.
///
/// Production wires [`MeshLayoutStore`]; the tests drive an in-memory fake so the
/// whole pipeline runs without etcd. Reuses the broker's [`SessionStoreError`] as its
/// error type, so the layout persist is gated identically to the session persist.
pub trait LayoutStore {
    /// Publish (create or update) `client_peer`'s layout in the shared plane.
    ///
    /// # Errors
    /// A [`SessionStoreError`] — `IntegrationGated` until the live etcd writer lands.
    fn publish(
        &self,
        client_peer: &NodeId,
        layout: &MonitorLayout,
    ) -> Result<(), SessionStoreError>;

    /// List every persisted `(client_peer, layout)` pair.
    ///
    /// # Errors
    /// A [`SessionStoreError`] — `IntegrationGated` until the live etcd reader lands.
    fn list(&self) -> Result<Vec<(NodeId, MonitorLayout)>, SessionStoreError>;

    /// Remove `client_peer`'s layout from the shared plane.
    ///
    /// # Errors
    /// A [`SessionStoreError`] — `IntegrationGated` until the live etcd deleter lands.
    fn remove(&self, client_peer: &NodeId) -> Result<(), SessionStoreError>;
}

/// Production [`LayoutStore`]: the per-peer monitor-layout plane on the same
/// etcd-leased / Syncthing-replicated coordination substrate the broker's
/// [`MeshSessionStore`] roams sessions on.
///
/// Like [`MeshSessionStore`], this slice (E12-8) delivers the pure policy + the seam;
/// the live etcd writer/reader/deleter is wired by a later E12 unit. Until then each
/// method returns a typed [`SessionStoreError::IntegrationGated`] naming exactly what
/// the live call needs — never a fake success (§7).
#[derive(Debug, Clone)]
pub struct MeshLayoutStore {
    /// Shared-storage root — the Syncthing-replicated fallback plane.
    workgroup_root: PathBuf,
}

impl MeshLayoutStore {
    /// Construct over the mesh `workgroup_root` (the replicated shared volume).
    #[must_use]
    pub const fn new(workgroup_root: PathBuf) -> Self {
        Self { workgroup_root }
    }
}

impl LayoutStore for MeshLayoutStore {
    fn publish(
        &self,
        client_peer: &NodeId,
        _layout: &MonitorLayout,
    ) -> Result<(), SessionStoreError> {
        Err(SessionStoreError::IntegrationGated {
            op: "publish",
            reason: format!(
                "layout for {client_peer} → needs the live etcd layout-lease writer over Nebula \
                 (the roaming-session plane under {}); the cross-peer persist isn't wired yet",
                self.workgroup_root.display()
            ),
        })
    }

    fn list(&self) -> Result<Vec<(NodeId, MonitorLayout)>, SessionStoreError> {
        Err(SessionStoreError::IntegrationGated {
            op: "list",
            reason: format!(
                "needs the live etcd layout-directory reader over Nebula (the roaming-session \
                 plane under {})",
                self.workgroup_root.display()
            ),
        })
    }

    fn remove(&self, client_peer: &NodeId) -> Result<(), SessionStoreError> {
        Err(SessionStoreError::IntegrationGated {
            op: "remove",
            reason: format!(
                "layout for {client_peer} → needs the live etcd layout-lease deleter over Nebula \
                 (the roaming-session plane under {})",
                self.workgroup_root.display()
            ),
        })
    }
}

// ───────────────────────────── live-node seam ─────────────────────────────

/// The "who is alive right now" seam the node-loss policy reads.
///
/// Production wires [`PeerLiveNodes`] (the etcd-lease-backed mesh peer directory);
/// tests wire a fixed set. Mirrors [`super::scheduler`]'s `LiveDirectory`.
pub trait LiveNodes {
    /// The node ids currently present in the mesh peer directory. Liveness IS the
    /// etcd keepalive lease — a departed node is simply absent.
    fn live(&self) -> BTreeSet<NodeId>;
}

/// Production [`LiveNodes`]: the canonical etcd-first peer directory
/// ([`crate::substrate::peers::read_directory`]), where liveness is the etcd
/// keepalive lease (fs-union fallback under `workgroup_root`).
pub struct PeerLiveNodes {
    /// Shared-storage root — the fs-union fallback when etcd is absent.
    workgroup_root: PathBuf,
}

impl LiveNodes for PeerLiveNodes {
    fn live(&self) -> BTreeSet<NodeId> {
        crate::substrate::peers::read_directory(&self.workgroup_root)
            .into_iter()
            .map(|r| r.hostname)
            .collect()
    }
}

// ───────────────────────────── bus + worker ─────────────────────────────

/// Parse a [`RoamingRequest`] body.
///
/// # Errors
/// A human-readable message on malformed JSON / unknown `op`.
pub fn parse_request(body: &str) -> Result<RoamingRequest, String> {
    serde_json::from_str(body).map_err(|e| format!("malformed roaming request: {e}"))
}

/// A roaming-policy request drained off [`ACTION_TOPIC`]. Internally tagged on `op`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum RoamingRequest {
    /// A user arrived at a Workstation — roam every session driven from `from` onto
    /// `workstation` (the "desktops follow me" trigger).
    Arrive {
        /// The peer the user's sessions are currently driven from.
        from: NodeId,
        /// The Workstation the user has arrived at.
        workstation: NodeId,
    },
    /// Set a VM's disconnect policy (default [`DisconnectPolicy::KeepRunning`]).
    SetPolicy {
        /// The target VM (libvirt UUID).
        vm_id: VmId,
        /// The policy to honor on that VM's next disconnect.
        policy: DisconnectPolicy,
    },
    /// Persist a user's multi-monitor layout, keyed by their driving peer.
    SaveLayout {
        /// The peer this layout belongs to.
        client_peer: NodeId,
        /// The layout to persist.
        layout: MonitorLayout,
    },
}

/// The mesh-replicated fold of the roaming request log every node maintains, so any
/// node has a warm policy view ready to converge if it wins the election. Each map
/// is latest-wins by key, exactly like the broker's roster fold.
#[derive(Debug, Default)]
struct RoamingFold {
    /// Latest arrival target per driving peer (`from` → `workstation`).
    arrivals: BTreeMap<NodeId, NodeId>,
    /// Per-VM disconnect policy.
    policies: BTreeMap<VmId, DisconnectPolicy>,
    /// Per-peer persisted monitor layout.
    layouts: BTreeMap<NodeId, MonitorLayout>,
}

impl RoamingFold {
    /// Fold one drained request into the view (latest-wins by key).
    fn apply(&mut self, req: RoamingRequest) {
        match req {
            RoamingRequest::Arrive { from, workstation } => {
                self.arrivals.insert(from, workstation);
            }
            RoamingRequest::SetPolicy { vm_id, policy } => {
                self.policies.insert(vm_id, policy);
            }
            RoamingRequest::SaveLayout {
                client_peer,
                layout,
            } => {
                self.layouts.insert(client_peer, layout);
            }
        }
    }
}

/// Read new [`ACTION_TOPIC`] messages since `cursor`, advancing it. A short sync
/// open-read-drop (never crosses an `.await`), mirroring the broker / scheduler.
fn read_new_actions(bus_root: &Path, cursor: &mut Option<String>) -> Vec<RoamingRequest> {
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
            Err(e) => {
                tracing::warn!(ulid = %msg.ulid, error = %e, "session_roaming: bad roaming request");
            }
        }
    }
    out
}

/// Fold new `action/vdi/roaming` messages (advancing `cursor`) into `fold`. Runs on
/// every node (the log is mesh-replicated), so any node has a warm policy view ready
/// to converge if it wins the election.
fn drain(bus_root: &Path, cursor: &mut Option<String>, fold: &mut RoamingFold) {
    for req in read_new_actions(bus_root, cursor) {
        fold.apply(req);
    }
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

/// The VDI session-roaming worker. Leader-gated + best-effort — the policy layer
/// over [`super::session_broker`]'s sessions.
pub struct SessionRoamingWorker {
    /// The reused session-plane seam (production: [`MeshSessionStore`]).
    store: Box<dyn SessionStore + Send + Sync>,
    /// The per-peer layout seam (production: [`MeshLayoutStore`]).
    layouts: Box<dyn LayoutStore + Send + Sync>,
    /// The live-node seam (production: [`PeerLiveNodes`]).
    live_nodes: Box<dyn LiveNodes + Send + Sync>,
    /// This node's id — its identity in the leader election.
    node_id: NodeId,
    /// The shared leader lock (the same `.mackesd-leader.lock` the broker uses).
    leader_lock: PathBuf,
    /// Convergence cadence.
    poll: Duration,
    /// Bus root override (tests). `None` ⇒ [`default_bus_root`].
    bus_root_override: Option<PathBuf>,
}

impl SessionRoamingWorker {
    /// Construct with production defaults: the reused etcd-first [`MeshSessionStore`],
    /// the [`MeshLayoutStore`], the etcd [`PeerLiveNodes`] directory, the shared
    /// leader lock under `workgroup_root`, and the default cadence. `node_id` is this
    /// node's mesh identity.
    #[must_use]
    pub fn new(workgroup_root: PathBuf, node_id: NodeId) -> Self {
        let leader_lock = workgroup_root.join(".mackesd-leader.lock");
        Self {
            store: Box::new(MeshSessionStore::new(workgroup_root.clone())),
            layouts: Box::new(MeshLayoutStore::new(workgroup_root.clone())),
            live_nodes: Box::new(PeerLiveNodes { workgroup_root }),
            node_id,
            leader_lock,
            poll: DEFAULT_POLL_INTERVAL,
            bus_root_override: None,
        }
    }

    /// Inject a session store (tests). Production reuses [`MeshSessionStore`].
    #[must_use]
    pub fn with_store(mut self, store: Box<dyn SessionStore + Send + Sync>) -> Self {
        self.store = store;
        self
    }

    /// Inject a layout store (tests). Production uses [`MeshLayoutStore`].
    #[must_use]
    pub fn with_layout_store(mut self, layouts: Box<dyn LayoutStore + Send + Sync>) -> Self {
        self.layouts = layouts;
        self
    }

    /// Inject a live-node directory (tests). Production uses [`PeerLiveNodes`].
    #[must_use]
    pub fn with_live_nodes(mut self, live_nodes: Box<dyn LiveNodes + Send + Sync>) -> Self {
        self.live_nodes = live_nodes;
        self
    }

    /// Override the convergence cadence (tests, to avoid multi-second waits).
    #[must_use]
    pub const fn with_poll(mut self, poll: Duration) -> Self {
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

    /// Only the elected node converges the shared plane (no-fixed-center: any
    /// eligible node can be it, the elected one writes). Reuses the shared lock.
    fn is_leader(&self) -> bool {
        matches!(
            crate::leader::try_acquire(&self.leader_lock, &self.node_id),
            Ok(crate::leader::AcquireResult::Acquired)
        )
    }

    /// Leader-only: read the shared session plane + the live-node set, plan this
    /// tick's roaming actions ([`plan_roaming`]) from the folded policy view, apply
    /// them through the reused store, and persist any pending layouts. Best-effort —
    /// a gated / failed store defers this tick (honest, never a fake success).
    fn converge(&self, fold: &RoamingFold) {
        if !self.is_leader() {
            return;
        }
        let observed: BTreeMap<SessionId, VdiSession> = match self.store.list() {
            Ok(rows) => rows.into_iter().map(|s| (s.id.clone(), s)).collect(),
            Err(e @ SessionStoreError::IntegrationGated { .. }) => {
                tracing::info!(error = %e, "session_roaming: session store integration-gated; deferring");
                return;
            }
            Err(e) => {
                tracing::warn!(error = %e, "session_roaming: session store list failed; deferring");
                return;
            }
        };
        let live = self.live_nodes.live();
        for action in plan_roaming(&fold.arrivals, &fold.policies, &observed, &live, now_ms()) {
            if let Err(e) = action.apply(self.store.as_ref()) {
                tracing::warn!(error = %e, "session_roaming: roaming action failed");
            }
        }
        // Persist any folded layouts through the companion (gated) seam.
        for (peer, layout) in &fold.layouts {
            if let Err(e) = self.layouts.publish(peer, layout) {
                match e {
                    SessionStoreError::IntegrationGated { .. } => {
                        tracing::info!(error = %e, "session_roaming: layout store integration-gated; deferring");
                    }
                    SessionStoreError::Failed { .. } => {
                        tracing::warn!(error = %e, "session_roaming: layout persist failed");
                    }
                }
            }
        }
    }
}

#[async_trait::async_trait]
impl Worker for SessionRoamingWorker {
    fn name(&self) -> &'static str {
        "session_roaming"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let Some(bus_root) = self.bus_root() else {
            tracing::debug!("session_roaming: no bus root; worker idle");
            return Ok(());
        };
        // Read the FULL request log from the start (like the broker): the policy view
        // is a fold of the whole log, so a (re)start rebuilds it before converging.
        let mut cursor: Option<String> = None;
        let mut fold = RoamingFold::default();
        let mut tick = tokio::time::interval(self.poll);
        tick.tick().await; // consume the immediate first tick
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    drain(&bus_root, &mut cursor, &mut fold);
                    self.converge(&fold);
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

    fn sess(id: &str, client_peer: &str, serving_peer: &str, state: SessionState) -> VdiSession {
        VdiSession {
            id: id.to_string(),
            serving_peer: serving_peer.to_string(),
            vm_id: format!("uuid-{id}"),
            client_peer: client_peer.to_string(),
            state,
            opened_at_ms: 100,
            updated_at_ms: 100,
        }
    }

    fn roster_of(sessions: &[VdiSession]) -> BTreeMap<SessionId, VdiSession> {
        sessions.iter().map(|s| (s.id.clone(), s.clone())).collect()
    }

    /// A live-node set of (bare) hostnames — the node-loss `live` argument.
    fn live_set(ids: &[&str]) -> BTreeSet<NodeId> {
        ids.iter().map(|s| (*s).to_string()).collect()
    }

    // ── monitor layout ──

    #[test]
    fn monitor_layout_helpers_and_serde() {
        let layout = MonitorLayout::new(vec![
            MonitorAssignment {
                monitor: 0,
                session_id: "s1".into(),
                vm_id: "uuid-1".into(),
                geometry: MonitorGeometry::new(0, 0, 1920, 1080),
                primary: true,
            },
            MonitorAssignment {
                monitor: 1,
                session_id: "s2".into(),
                vm_id: "uuid-2".into(),
                geometry: MonitorGeometry::new(1920, 0, 2560, 1440),
                primary: false,
            },
        ]);
        assert_eq!(layout.surface_count(), 2);
        assert_eq!(
            layout.on_monitor(1).map(|a| a.session_id.as_str()),
            Some("s2")
        );
        assert_eq!(layout.primary().map(|a| a.monitor), Some(0));
        assert_eq!(MonitorGeometry::new(0, 0, 1920, 1080).area(), 1920 * 1080);
        assert!(MonitorLayout::empty().primary().is_none());
        // Round-trips through serde (it rides the request + layout plane).
        let json = serde_json::to_string(&layout).expect("serialize");
        let back: MonitorLayout = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, layout);
    }

    // ── disconnect policy ──

    #[test]
    fn disconnect_policy_default_is_keep_running() {
        assert_eq!(DisconnectPolicy::default(), DisconnectPolicy::KeepRunning);
        // The default serializes snake_case for the wire verb.
        assert_eq!(
            serde_json::to_string(&DisconnectPolicy::KeepRunning).unwrap(),
            "\"keep_running\""
        );
    }

    #[test]
    fn on_disconnect_keep_running_holds_the_vm_reconnectable() {
        let s = sess("s1", "peer:c", "peer:h", SessionState::Active);
        let out = on_disconnect(&s, DisconnectPolicy::KeepRunning, 200);
        assert!(matches!(out, DisconnectOutcome::KeepRunning { .. }));
        assert_eq!(out.session().state, SessionState::Disconnected);
        assert!(out.reconnectable());
        assert!(out.vm_kept_running());
    }

    #[test]
    fn on_disconnect_suspend_keeps_reconnectable_but_not_running() {
        let s = sess("s1", "peer:c", "peer:h", SessionState::Active);
        let out = on_disconnect(&s, DisconnectPolicy::Suspend, 200);
        assert!(matches!(out, DisconnectOutcome::Suspend { .. }));
        assert_eq!(out.session().state, SessionState::Disconnected);
        assert!(out.reconnectable());
        assert!(!out.vm_kept_running());
    }

    #[test]
    fn on_disconnect_shutdown_ends_the_session() {
        let s = sess("s1", "peer:c", "peer:h", SessionState::Active);
        let out = on_disconnect(&s, DisconnectPolicy::Shutdown, 200);
        assert!(matches!(out, DisconnectOutcome::Shutdown { .. }));
        assert_eq!(out.session().state, SessionState::Closed);
        assert!(!out.reconnectable());
        assert!(!out.vm_kept_running());
    }

    #[test]
    fn on_disconnect_tolerates_a_never_connected_session() {
        // Requested can't transition to Disconnected — keep/suspend carry it
        // unchanged (the VM directive still holds), shutdown still closes it.
        let s = sess("s1", "peer:c", "peer:h", SessionState::Requested);
        let keep = on_disconnect(&s, DisconnectPolicy::KeepRunning, 200);
        assert_eq!(keep.session().state, SessionState::Requested);
        let shut = on_disconnect(&s, DisconnectPolicy::Shutdown, 200);
        assert_eq!(shut.session().state, SessionState::Closed);
    }

    // ── roam_to (desktops follow me transform) ──

    #[test]
    fn roam_to_repoints_reconnects_and_keeps_the_vm() {
        let sessions = vec![
            sess("s1", "peer:old", "peer:vmhost", SessionState::Disconnected),
            sess("s2", "peer:old", "peer:vmhost2", SessionState::Active),
            sess("s3", "peer:old", "peer:vmhost", SessionState::Closed), // terminal → dropped
        ];
        let roamed = roam_to(&sessions, &"peer:new".to_string(), 900);
        assert_eq!(roamed.len(), 2, "the terminal session is not roamed");
        for r in &roamed {
            assert_eq!(
                r.client_peer, "peer:new",
                "client re-pointed to the arrival"
            );
            assert_eq!(r.state, SessionState::Active, "reconnected");
            assert_eq!(r.updated_at_ms, 900);
        }
        // The serving VM identity is untouched — the VM keeps running where it was.
        assert_eq!(roamed[0].serving_peer, "peer:vmhost");
        assert_eq!(roamed[0].vm_id, "uuid-s1");
    }

    #[test]
    fn roaming_state_assemble_and_roam() {
        let all = vec![
            sess("s1", "peer:old", "peer:h", SessionState::Disconnected),
            sess("s2", "peer:old", "peer:h", SessionState::Closed), // not open
            sess("s9", "peer:other", "peer:h", SessionState::Active), // different user
        ];
        let layout = MonitorLayout::new(vec![MonitorAssignment {
            monitor: 0,
            session_id: "s1".into(),
            vm_id: "uuid-s1".into(),
            geometry: MonitorGeometry::new(0, 0, 1920, 1080),
            primary: true,
        }]);
        let state = RoamingState::assemble("peer:old".into(), &all, layout.clone());
        assert_eq!(
            state.sessions.len(),
            1,
            "only the open session for this peer"
        );
        assert_eq!(state.sessions[0].id, "s1");
        // Roaming carries the layout + reconnects onto the new workstation.
        let roamed = state.roam_to(&"peer:new".to_string(), 500);
        assert_eq!(roamed.client_peer, "peer:new");
        assert_eq!(roamed.sessions[0].state, SessionState::Active);
        assert_eq!(roamed.layout, layout, "the monitor layout follows the user");
    }

    // ── reconcile_roaming (the required pure decision) ──

    #[test]
    fn reconcile_roaming_reopens_a_missing_session() {
        let desired = vec![sess("s1", "peer:new", "peer:h", SessionState::Active)];
        let out = reconcile_roaming(&desired, &BTreeMap::new());
        assert_eq!(
            out,
            vec![RoamingAction::Reopen(sess(
                "s1",
                "peer:new",
                "peer:h",
                SessionState::Active
            ))]
        );
    }

    #[test]
    fn reconcile_roaming_reopens_only_a_changed_session() {
        let desired = vec![sess("s1", "peer:new", "peer:h", SessionState::Active)];
        // Observed holds s1 still pointed at the old peer ⇒ re-open (roam).
        let observed = roster_of(&[sess("s1", "peer:old", "peer:h", SessionState::Disconnected)]);
        assert_eq!(
            reconcile_roaming(&desired, &observed),
            vec![RoamingAction::Reopen(sess(
                "s1",
                "peer:new",
                "peer:h",
                SessionState::Active
            ))]
        );
        // Already byte-identical ⇒ no needless write.
        let converged = roster_of(&[sess("s1", "peer:new", "peer:h", SessionState::Active)]);
        assert!(reconcile_roaming(&desired, &converged).is_empty());
    }

    #[test]
    fn reconcile_roaming_releases_stale_and_terminal() {
        // s1 Active desired + absent ⇒ Reopen. s2 (terminal) still in plane ⇒ Release.
        // s3 in plane but no longer desired ⇒ Release.
        let desired = vec![
            sess("s1", "peer:new", "peer:h", SessionState::Active),
            sess("s2", "peer:new", "peer:h", SessionState::Closed),
        ];
        let observed = roster_of(&[
            sess("s2", "peer:new", "peer:h", SessionState::Active),
            sess("s3", "peer:new", "peer:h", SessionState::Active),
        ]);
        let out = reconcile_roaming(&desired, &observed);
        assert_eq!(
            out,
            vec![
                RoamingAction::Reopen(sess("s1", "peer:new", "peer:h", SessionState::Active)),
                RoamingAction::Release("s2".into()),
                RoamingAction::Release("s3".into()),
            ]
        );
    }

    #[test]
    fn reconcile_roaming_is_deterministic() {
        let desired = vec![
            sess("s2", "peer:new", "peer:h", SessionState::Active),
            sess("s1", "peer:new", "peer:h", SessionState::Active),
        ];
        let a = reconcile_roaming(&desired, &BTreeMap::new());
        let b = reconcile_roaming(&desired, &BTreeMap::new());
        assert_eq!(a, b);
        assert_eq!(
            a[0].session_id(),
            "s1",
            "id-sorted regardless of input order"
        );
    }

    // ── on_node_loss (reconnect-on-node-loss) ──

    #[test]
    fn on_node_loss_holds_a_session_on_a_lost_node() {
        // s1's serving node peer:dead left the mesh ⇒ held Disconnected (never
        // Closed) so it can reconnect once the VM is re-placed. s2 is live ⇒ untouched.
        let sessions = vec![
            sess("s1", "peer:c", "peer:dead", SessionState::Active),
            sess("s2", "peer:c", "peer:live", SessionState::Active),
        ];
        let live = live_set(&["live"]); // bare hostname
        let out = on_node_loss(&sessions, &live, 700);
        assert_eq!(out.len(), 1);
        match &out[0] {
            RoamingAction::HoldForReconnect(s) => {
                assert_eq!(s.id, "s1");
                assert_eq!(s.state, SessionState::Disconnected);
                assert_eq!(s.updated_at_ms, 700);
            }
            other => panic!("expected HoldForReconnect, got {other:?}"),
        }
    }

    #[test]
    fn on_node_loss_releases_a_never_connected_session() {
        // A Requested session (never connected) on a lost node can't reconnect ⇒
        // Release. A terminal session is skipped entirely.
        let sessions = vec![
            sess("s1", "peer:c", "peer:dead", SessionState::Requested),
            sess("s2", "peer:c", "peer:dead", SessionState::Closed),
        ];
        let out = on_node_loss(&sessions, &BTreeSet::new(), 700);
        assert_eq!(out, vec![RoamingAction::Release("s1".into())]);
    }

    // ── plan_roaming (the whole-tick composer) ──

    #[test]
    fn plan_roaming_composes_arrival_shutdown_and_node_loss() {
        // s1: user roams peer:old → peer:new (serving node live) ⇒ Reopen@new.
        // s2: Disconnected with a Shutdown policy ⇒ Release.
        // s3: Active on a lost serving node ⇒ HoldForReconnect.
        let observed = roster_of(&[
            sess("s1", "peer:old", "peer:live", SessionState::Disconnected),
            sess("s2", "peer:x", "peer:live", SessionState::Disconnected),
            sess("s3", "peer:x", "peer:dead", SessionState::Active),
        ]);
        let arrivals: BTreeMap<NodeId, NodeId> =
            [("peer:old".to_string(), "peer:new".to_string())].into();
        let policies: BTreeMap<VmId, DisconnectPolicy> =
            [("uuid-s2".to_string(), DisconnectPolicy::Shutdown)].into();
        let live = live_set(&["live"]);

        let out = plan_roaming(&arrivals, &policies, &observed, &live, 42);
        // Deterministic (id-sorted): s1 Reopen, s2 Release, s3 Hold.
        assert_eq!(out.len(), 3);
        match &out[0] {
            RoamingAction::Reopen(s) => {
                assert_eq!(s.id, "s1");
                assert_eq!(s.client_peer, "peer:new");
                assert_eq!(s.state, SessionState::Active);
            }
            other => panic!("expected Reopen s1, got {other:?}"),
        }
        assert_eq!(out[1], RoamingAction::Release("s2".into()));
        match &out[2] {
            RoamingAction::HoldForReconnect(s) => {
                assert_eq!(s.id, "s3");
                assert_eq!(s.state, SessionState::Disconnected);
            }
            other => panic!("expected HoldForReconnect s3, got {other:?}"),
        }
    }

    #[test]
    fn plan_roaming_keep_running_is_a_noop() {
        // A Disconnected session with the default KeepRunning policy + a live serving
        // node produces no write — the desktop just waits, reconnectable.
        let observed = roster_of(&[sess(
            "s1",
            "peer:c",
            "peer:live",
            SessionState::Disconnected,
        )]);
        let live = live_set(&["live"]);
        let out = plan_roaming(&BTreeMap::new(), &BTreeMap::new(), &observed, &live, 42);
        assert!(out.is_empty(), "KeepRunning holds without a plane write");
    }

    // ── store seams ──

    #[test]
    fn mesh_layout_store_is_integration_gated_not_faked() {
        let store = MeshLayoutStore::new(PathBuf::from("/tmp/mesh-wg"));
        let peer = "peer:c".to_string();
        for (label, err) in [
            (
                "publish",
                store.publish(&peer, &MonitorLayout::empty()).unwrap_err(),
            ),
            ("list", store.list().map(|_| ()).unwrap_err()),
            ("remove", store.remove(&peer).unwrap_err()),
        ] {
            match err {
                SessionStoreError::IntegrationGated { op, reason } => {
                    assert_eq!(op, label);
                    assert!(
                        reason.contains("etcd"),
                        "names the missing live dep: {reason}"
                    );
                }
                SessionStoreError::Failed { op, reason } => {
                    panic!("expected integration-gated, got Failed {{{op}: {reason}}}")
                }
            }
        }
    }

    /// An in-memory [`SessionStore`] — the Fake seam (the broker's `FakeStore` is
    /// test-private, so the pipeline test carries its own).
    #[derive(Clone, Default)]
    struct FakeStore {
        rows: Arc<Mutex<BTreeMap<SessionId, VdiSession>>>,
    }

    impl SessionStore for FakeStore {
        fn publish(&self, session: &VdiSession) -> Result<(), SessionStoreError> {
            self.rows
                .lock()
                .expect("rows mutex")
                .insert(session.id.clone(), session.clone());
            Ok(())
        }
        fn list(&self) -> Result<Vec<VdiSession>, SessionStoreError> {
            Ok(self
                .rows
                .lock()
                .expect("rows mutex")
                .values()
                .cloned()
                .collect())
        }
        fn remove(&self, id: &str) -> Result<(), SessionStoreError> {
            self.rows.lock().expect("rows mutex").remove(id);
            Ok(())
        }
    }

    /// An in-memory [`LayoutStore`] — the Fake seam.
    #[derive(Clone, Default)]
    struct FakeLayoutStore {
        rows: Arc<Mutex<BTreeMap<NodeId, MonitorLayout>>>,
    }

    impl LayoutStore for FakeLayoutStore {
        fn publish(
            &self,
            client_peer: &NodeId,
            layout: &MonitorLayout,
        ) -> Result<(), SessionStoreError> {
            self.rows
                .lock()
                .expect("rows mutex")
                .insert(client_peer.clone(), layout.clone());
            Ok(())
        }
        fn list(&self) -> Result<Vec<(NodeId, MonitorLayout)>, SessionStoreError> {
            Ok(self
                .rows
                .lock()
                .expect("rows mutex")
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect())
        }
        fn remove(&self, client_peer: &NodeId) -> Result<(), SessionStoreError> {
            self.rows.lock().expect("rows mutex").remove(client_peer);
            Ok(())
        }
    }

    /// A [`LiveNodes`] returning a fixed set — the Fake seam.
    struct FakeLiveNodes(BTreeSet<NodeId>);
    impl LiveNodes for FakeLiveNodes {
        fn live(&self) -> BTreeSet<NodeId> {
            self.0.clone()
        }
    }

    #[test]
    fn fake_layout_store_round_trips() {
        let store = FakeLayoutStore::default();
        let peer = "peer:c".to_string();
        store.publish(&peer, &MonitorLayout::empty()).unwrap();
        assert_eq!(store.list().unwrap().len(), 1);
        store.remove(&peer).unwrap();
        assert!(store.list().unwrap().is_empty());
    }

    // ── parsing + topic + worker name ──

    #[test]
    fn parse_request_round_trips_ops() {
        let arrive = parse_request(r#"{"op":"arrive","from":"peer:old","workstation":"peer:new"}"#)
            .expect("arrive parses");
        assert_eq!(
            arrive,
            RoamingRequest::Arrive {
                from: "peer:old".into(),
                workstation: "peer:new".into(),
            }
        );
        let policy = parse_request(r#"{"op":"set_policy","vm_id":"u1","policy":"shutdown"}"#)
            .expect("set_policy parses");
        assert_eq!(
            policy,
            RoamingRequest::SetPolicy {
                vm_id: "u1".into(),
                policy: DisconnectPolicy::Shutdown,
            }
        );
        assert!(parse_request("nonsense").is_err());
        assert!(parse_request(r#"{"op":"teleport"}"#).is_err());
    }

    #[test]
    fn topic_is_namespaced() {
        assert_eq!(ACTION_TOPIC, "action/vdi/roaming");
        assert!(ACTION_TOPIC.starts_with("action/"));
    }

    #[test]
    fn worker_name_matches_module() {
        let w = SessionRoamingWorker::new(std::env::temp_dir(), "peer:a".to_string());
        assert_eq!(w.name(), "session_roaming");
    }

    // ── worker wiring (seeded temp bus + injected fakes) ──

    /// Seed a temp bus with `action/vdi/roaming` bodies and return its root.
    fn seed_bus(reqs: &[RoamingRequest]) -> PathBuf {
        use mde_bus::hooks::config::Priority;
        let dir = std::env::temp_dir().join(format!("mde-sr-{}-{}", now_ms(), reqs.len()));
        let persist = Persist::open(dir.clone()).expect("open bus");
        for r in reqs {
            persist
                .write(
                    ACTION_TOPIC,
                    Priority::Default,
                    None,
                    Some(&serde_json::to_string(r).unwrap()),
                )
                .expect("write action");
        }
        dir
    }

    #[tokio::test]
    async fn worker_drains_folds_and_roams_through_the_store() {
        // A user's session persisted on peer:old (Disconnected). An Arrive drains off
        // the bus; the leader roams it onto peer:new (Active) through the reused
        // store, and persists the saved layout through the (fake) layout store.
        let layout = MonitorLayout::new(vec![MonitorAssignment {
            monitor: 0,
            session_id: "s1".into(),
            vm_id: "uuid-s1".into(),
            geometry: MonitorGeometry::new(0, 0, 1920, 1080),
            primary: true,
        }]);
        let bus = seed_bus(&[
            RoamingRequest::Arrive {
                from: "peer:old".into(),
                workstation: "peer:new".into(),
            },
            RoamingRequest::SaveLayout {
                client_peer: "peer:new".into(),
                layout: layout.clone(),
            },
        ]);
        let wg = std::env::temp_dir().join(format!("mde-sr-wg-{}", now_ms()));
        std::fs::create_dir_all(&wg).expect("mk workgroup");

        let store = FakeStore::default();
        store
            .publish(&sess(
                "s1",
                "peer:old",
                "peer:vmhost",
                SessionState::Disconnected,
            ))
            .unwrap();
        let rows = store.rows.clone();
        let layouts = FakeLayoutStore::default();
        let layout_rows = layouts.rows.clone();

        let w = SessionRoamingWorker::new(wg.clone(), "peer:a".to_string())
            .with_store(Box::new(store))
            .with_layout_store(Box::new(layouts))
            // serving node peer:vmhost is live ⇒ no node-loss interference.
            .with_live_nodes(Box::new(FakeLiveNodes(live_set(&["vmhost"]))))
            .with_bus_root(bus.clone());

        let mut cursor = None;
        let mut fold = RoamingFold::default();
        drain(&bus, &mut cursor, &mut fold);
        w.converge(&fold);

        let published = rows.lock().expect("rows mutex");
        assert_eq!(published.len(), 1, "the session roamed, not duplicated");
        assert_eq!(
            published["s1"].client_peer, "peer:new",
            "desktop followed the user"
        );
        assert_eq!(published["s1"].state, SessionState::Active, "reconnected");
        assert_eq!(
            published["s1"].serving_peer, "peer:vmhost",
            "the VM kept running"
        );
        drop(published);
        // The monitor layout was persisted through the companion seam.
        let saved = layout_rows.lock().expect("layout mutex");
        assert_eq!(saved.get("peer:new"), Some(&layout));
        drop(saved);

        let _ = std::fs::remove_dir_all(&bus);
        let _ = std::fs::remove_dir_all(&wg);
    }

    #[tokio::test]
    async fn worker_shutdown_policy_ends_a_disconnected_session() {
        // A Disconnected session whose VM has a Shutdown policy is removed from the
        // plane on converge.
        let bus = seed_bus(&[RoamingRequest::SetPolicy {
            vm_id: "uuid-s1".into(),
            policy: DisconnectPolicy::Shutdown,
        }]);
        let wg = std::env::temp_dir().join(format!("mde-sr-wg-{}", now_ms()));
        std::fs::create_dir_all(&wg).expect("mk workgroup");
        let store = FakeStore::default();
        store
            .publish(&sess(
                "s1",
                "peer:c",
                "peer:live",
                SessionState::Disconnected,
            ))
            .unwrap();
        let rows = store.rows.clone();
        let w = SessionRoamingWorker::new(wg.clone(), "peer:a".to_string())
            .with_store(Box::new(store))
            .with_live_nodes(Box::new(FakeLiveNodes(live_set(&["live"]))))
            .with_bus_root(bus.clone());
        let mut cursor = None;
        let mut fold = RoamingFold::default();
        drain(&bus, &mut cursor, &mut fold);
        w.converge(&fold);
        assert!(
            rows.lock().expect("rows mutex").is_empty(),
            "the shutdown-policy session was removed from the plane"
        );
        let _ = std::fs::remove_dir_all(&bus);
        let _ = std::fs::remove_dir_all(&wg);
    }

    #[tokio::test]
    async fn run_loop_exits_promptly_on_shutdown() {
        // An empty temp bus ⇒ nothing to fold; the gated MeshSessionStore default
        // means no etcd is needed (converge defers honestly).
        let bus = std::env::temp_dir().join(format!("mde-sr-run-{}", now_ms()));
        let wg = std::env::temp_dir().join(format!("mde-sr-runwg-{}", now_ms()));
        std::fs::create_dir_all(&wg).expect("mk workgroup");
        let (tx, rx) = tokio::sync::watch::channel(false);
        let mut w = SessionRoamingWorker::new(wg.clone(), "peer:a".to_string())
            .with_bus_root(bus.clone())
            .with_poll(Duration::from_millis(10));
        let token = ShutdownToken::from_receiver(rx);
        let handle = tokio::spawn(async move { w.run(token).await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        tx.send(true).expect("signal shutdown");
        let joined = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(joined.is_ok(), "worker must exit promptly on shutdown");
        assert!(joined.unwrap().expect("join").is_ok());
        let _ = std::fs::remove_dir_all(&wg);
    }
}
