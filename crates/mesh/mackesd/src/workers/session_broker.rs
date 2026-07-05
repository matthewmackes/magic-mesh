//! E12-5b — `session_broker`: the mackesd **VDI session-broker** worker.
//!
//! The mackesd side of the E12-5 remote-desktop milestone. Where the shell
//! (`mde-shell-egui`) *renders* a VM desktop and [`super::scheduler`] *places*
//! the workload, this worker *tracks* the live VDI sessions — which peer serves
//! which VM to which client, and each session's state — and converges that roster
//! into shared mesh state so **any** peer can see the active sessions (the
//! roaming-session plane, design lock 5 in `docs/design/quasar-vdi-desktop.md`).
//!
//! ## Shape (mirrors [`super::scheduler`])
//!
//! - The **pure core** is fully unit-tested with no bus and no clock: the session
//!   state machine ([`open_session`] / [`mark_active`] / [`mark_disconnected`] /
//!   [`close_session`], each taking `now_ms` — the crate forbids ambient time on
//!   these paths, exactly as `scheduler`'s `plan_placement` does), the incremental
//!   folder [`apply_request`] (a drained `action/vdi/session` op → the in-memory
//!   session map), and the leader convergence decision [`reconcile`] (desired vs.
//!   observed → a minimal [`SessionAction`] set — the same shape as `scheduler`'s
//!   `replace_decisions`).
//! - The sole outward seam is the injectable [`SessionStore`] (production
//!   [`MeshSessionStore`] is the etcd-leased / Syncthing-replicated cross-peer
//!   session directory; a `FakeStore` drives the tests). The live cross-peer
//!   publish is **integration-gated** — [`MeshSessionStore`]'s methods return a
//!   typed [`SessionStoreError::IntegrationGated`] naming exactly what the live
//!   call needs, never a fake success (§7-legal, like `adopt_xcp::LiveAdopter` and
//!   MV-5's persist).
//! - **Leader-gated** ([`crate::leader`], the shared `.mackesd-leader.lock`, the
//!   same election `dc_auditor` uses): every node folds the mesh-replicated
//!   `action/vdi/session` log into its own session view, but only the elected node
//!   converges the shared plane, so an N-node mesh doesn't multi-write.
//!
//! ## Reused types (no parallel VM/peer model — §6 glue)
//!
//! - The serving + client peers are [`NodeId`] (re-exported from
//!   [`super::scheduler`]) — the very namespace the scheduler places VMs onto, so a
//!   session's `serving_peer` is the node that ran the placement.
//! - The target VM is identified by its [`VmId`] — the libvirt UUID that
//!   [`super::compute_registry::ComputeEvent::vm_id`] already publishes, not a new
//!   VM type.

#![cfg(feature = "async-services")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use mackes_mesh_types::openstack::HOT_TEMPLATE_VERSION;
use mde_bus::persist::Persist;

use super::openstack::verbs::{cloud_action_topic, HeatRequest};
use super::scheduler::NodeId;
use super::{ShutdownToken, Worker};

/// Bus topic the worker drains for session lifecycle requests.
///
/// Host-agnostic — the shell (or a peer's connect flow) publishes a
/// [`SessionRequest`] here and the leader folds them into the roaming-session
/// roster.
pub const ACTION_TOPIC: &str = "action/vdi/session";

/// Convergence cadence. The bus read is a cheap local log scan and a session is a
/// slow, human-paced event, so a 2 s poll is responsive without spinning (the same
/// cadence `scheduler` drains at).
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// A VDI session identity — an opaque id minted by the requesting shell (a ULID in
/// production), the key of the roster and the mesh-state record.
pub type SessionId = String;

/// A target-VM identity: the libvirt UUID a guest is stamped with.
///
/// Reused verbatim from [`super::compute_registry::ComputeEvent::vm_id`] /
/// [`super::vm_lifecycle`] so the broker never invents a parallel VM type — a
/// session merely *points at* a VM the compute plane owns.
pub type VmId = String;

// ───────────────────────────── data model ─────────────────────────────

/// The lifecycle state of one VDI session.
///
/// The legal transitions (enforced by the pure decision fns):
/// `Requested → Active` (the connect succeeded), `Active ⇄ Disconnected` (the link
/// dropped / the client reconnected), and any non-terminal state `→ Closed` (the
/// session ended). `Closed` is terminal — no transition leaves it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// A session was opened but the remote-desktop connect hasn't completed.
    Requested,
    /// The desktop is connected and interactive.
    Active,
    /// The link dropped but the VM keeps running — a reconnect returns to
    /// [`SessionState::Active`] (design lock 5: a disconnected VM keeps running).
    Disconnected,
    /// The session ended (terminal). Converged *out* of the shared plane.
    Closed,
}

impl SessionState {
    /// `true` for the terminal [`SessionState::Closed`] state.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Closed)
    }

    /// `true` when a session in this state should be *visible* in the shared
    /// roster (everything but the terminal [`SessionState::Closed`], which is
    /// converged out).
    #[must_use]
    pub const fn is_publishable(self) -> bool {
        !matches!(self, Self::Closed)
    }
}

/// One tracked VDI session: which `serving_peer` serves which `vm_id` to which
/// `client_peer`, plus its [`SessionState`] and the caller-supplied timestamps.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VdiSession {
    /// The session identity (roster key + mesh-state record key).
    pub id: SessionId,
    /// The peer hosting/serving the VM desktop (a scheduler [`NodeId`]).
    pub serving_peer: NodeId,
    /// The target VM (libvirt UUID — see [`VmId`]).
    pub vm_id: VmId,
    /// The peer whose shell is driving the desktop (a scheduler [`NodeId`]).
    pub client_peer: NodeId,
    /// The current lifecycle state.
    pub state: SessionState,
    /// When the session was first opened (ms since the Unix epoch, passed in).
    pub opened_at_ms: u64,
    /// When the state last changed (ms since the Unix epoch, passed in).
    pub updated_at_ms: u64,
}

/// A session lifecycle request drained off [`ACTION_TOPIC`] — the wire verb the
/// shell / connect flow publishes. Internally tagged on `op`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SessionRequest {
    /// Open a new session (state [`SessionState::Requested`]).
    Open {
        /// The session id to mint.
        id: SessionId,
        /// The peer that will serve the VM.
        serving_peer: NodeId,
        /// The target VM (libvirt UUID).
        vm_id: VmId,
        /// The peer whose shell drives it.
        client_peer: NodeId,
    },
    /// The connect completed — mark the session [`SessionState::Active`].
    Active {
        /// The session id.
        id: SessionId,
    },
    /// The link dropped — mark the session [`SessionState::Disconnected`].
    Disconnect {
        /// The session id.
        id: SessionId,
    },
    /// The session ended — mark it [`SessionState::Closed`] (terminal).
    Close {
        /// The session id.
        id: SessionId,
    },
}

/// Parse a [`SessionRequest`] body.
///
/// # Errors
/// A human-readable message on malformed JSON / unknown `op`.
pub fn parse_request(body: &str) -> Result<SessionRequest, String> {
    serde_json::from_str(body).map_err(|e| format!("malformed session request: {e}"))
}

/// A typed failure from a session state-machine transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionError {
    /// A transition the state machine forbids (e.g. re-activating a `Closed`
    /// session, or disconnecting one that was never `Active`).
    IllegalTransition {
        /// The state the session was in.
        from: SessionState,
        /// The state the caller tried to move it to.
        to: SessionState,
    },
    /// A transition op named a session id the roster doesn't hold.
    UnknownSession(SessionId),
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IllegalTransition { from, to } => {
                write!(f, "illegal session transition {from:?} → {to:?}")
            }
            Self::UnknownSession(id) => write!(f, "unknown session {id}"),
        }
    }
}

impl std::error::Error for SessionError {}

// ─────────────────────────── pure: state machine ───────────────────────────

/// Build a fresh session in [`SessionState::Requested`]. `now_ms` seeds both the
/// open and the last-update time (passed in — no ambient clock).
#[must_use]
pub const fn open_session(
    id: SessionId,
    serving_peer: NodeId,
    vm_id: VmId,
    client_peer: NodeId,
    now_ms: u64,
) -> VdiSession {
    VdiSession {
        id,
        serving_peer,
        vm_id,
        client_peer,
        state: SessionState::Requested,
        opened_at_ms: now_ms,
        updated_at_ms: now_ms,
    }
}

/// Clone `session` with a new `state` + refreshed `updated_at_ms`.
fn with_state(session: &VdiSession, to: SessionState, now_ms: u64) -> VdiSession {
    VdiSession {
        state: to,
        updated_at_ms: now_ms,
        ..session.clone()
    }
}

/// Transition a session to [`SessionState::Active`] (the connect completed).
///
/// Idempotent from `Active`; a valid reconnect from `Requested` / `Disconnected`.
///
/// # Errors
/// [`SessionError::IllegalTransition`] from the terminal [`SessionState::Closed`].
pub fn mark_active(session: &VdiSession, now_ms: u64) -> Result<VdiSession, SessionError> {
    match session.state {
        SessionState::Requested | SessionState::Active | SessionState::Disconnected => {
            Ok(with_state(session, SessionState::Active, now_ms))
        }
        SessionState::Closed => Err(SessionError::IllegalTransition {
            from: SessionState::Closed,
            to: SessionState::Active,
        }),
    }
}

/// Transition a session to [`SessionState::Disconnected`] (the link dropped).
///
/// Idempotent from `Disconnected`.
///
/// # Errors
/// [`SessionError::IllegalTransition`] from `Requested` (never connected) or the
/// terminal `Closed`.
pub fn mark_disconnected(session: &VdiSession, now_ms: u64) -> Result<VdiSession, SessionError> {
    match session.state {
        SessionState::Active | SessionState::Disconnected => {
            Ok(with_state(session, SessionState::Disconnected, now_ms))
        }
        other => Err(SessionError::IllegalTransition {
            from: other,
            to: SessionState::Disconnected,
        }),
    }
}

/// Transition a session to the terminal [`SessionState::Closed`]. Always valid
/// (idempotent from `Closed`) — a session can end from any state.
#[must_use]
pub fn close_session(session: &VdiSession, now_ms: u64) -> VdiSession {
    with_state(session, SessionState::Closed, now_ms)
}

/// Apply one drained [`SessionRequest`] to the in-memory `roster` (latest-wins by
/// id — the incremental fold the worker runs per drained message, the session
/// analogue of `scheduler`'s `fold_capacity`).
///
/// # Errors
/// [`SessionError::UnknownSession`] when a transition op names an absent id, or
/// [`SessionError::IllegalTransition`] when the transition is forbidden. `Open`
/// never errors (it mints / overwrites the row).
pub fn apply_request(
    roster: &mut BTreeMap<SessionId, VdiSession>,
    req: SessionRequest,
    now_ms: u64,
) -> Result<(), SessionError> {
    match req {
        SessionRequest::Open {
            id,
            serving_peer,
            vm_id,
            client_peer,
        } => {
            let session = open_session(id.clone(), serving_peer, vm_id, client_peer, now_ms);
            roster.insert(id, session);
            Ok(())
        }
        SessionRequest::Active { id } => transition(roster, &id, |s| mark_active(s, now_ms)),
        SessionRequest::Disconnect { id } => {
            transition(roster, &id, |s| mark_disconnected(s, now_ms))
        }
        SessionRequest::Close { id } => {
            let Some(cur) = roster.get(&id) else {
                return Err(SessionError::UnknownSession(id));
            };
            let closed = close_session(cur, now_ms);
            roster.insert(id, closed);
            Ok(())
        }
    }
}

/// Look up `id` in `roster`, apply the fallible transition `f`, and store the
/// result. `UnknownSession` when the id is absent.
fn transition(
    roster: &mut BTreeMap<SessionId, VdiSession>,
    id: &str,
    f: impl FnOnce(&VdiSession) -> Result<VdiSession, SessionError>,
) -> Result<(), SessionError> {
    let Some(cur) = roster.get(id) else {
        return Err(SessionError::UnknownSession(id.to_string()));
    };
    let next = f(cur)?;
    roster.insert(next.id.clone(), next);
    Ok(())
}

// ─────────────────────────── pure: convergence ───────────────────────────

/// One convergence step the leader applies to the shared session plane through
/// the [`SessionStore`] seam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionAction {
    /// Publish (create or update) this session's record in mesh state.
    Publish(VdiSession),
    /// Remove this session id from mesh state (ended, or no longer tracked).
    Remove(SessionId),
}

/// The pure convergence decision: the minimal [`SessionAction`] set that makes the
/// `observed` shared plane match the leader's `desired` roster.
///
/// - A publishable desired session that is absent-or-different in `observed` is
///   `Publish`ed; one already byte-identical is left alone (no needless write).
/// - A terminal ([`SessionState::Closed`]) desired session that still lingers in
///   `observed` is `Remove`d (a closed session isn't an active session).
/// - An `observed` id the leader no longer tracks is `Remove`d (stale row).
///
/// Deterministic (both scans walk id-sorted maps) and clock-free — the same shape
/// as `scheduler`'s `replace_decisions`.
#[must_use]
pub fn reconcile(
    desired: &[VdiSession],
    observed: &BTreeMap<SessionId, VdiSession>,
) -> Vec<SessionAction> {
    // Id-keyed so the scan is deterministic (id-sorted) and lookups are cheap.
    let desired_by_id: BTreeMap<SessionId, &VdiSession> =
        desired.iter().map(|s| (s.id.clone(), s)).collect();
    let mut out = Vec::new();
    for (id, d) in &desired_by_id {
        if d.state.is_publishable() {
            if observed.get(id) != Some(*d) {
                out.push(SessionAction::Publish((*d).clone()));
            }
        } else if observed.contains_key(id) {
            out.push(SessionAction::Remove(id.clone()));
        }
    }
    // Rows the plane holds that the leader no longer tracks at all.
    for id in observed.keys() {
        if !desired_by_id.contains_key(id) {
            out.push(SessionAction::Remove(id.clone()));
        }
    }
    out
}

// ─────────────────────── QC-14: Nova placement ───────────────────────
// The session-broker PLACES a VDI desktop as a Nova instance (flavor + VDI
// metadata) while keeping ownership of the display path / roaming / seat binding
// (design Q33). A desktop is created as a **managed Heat stack** — the sanctioned
// creation path (QC-12: "creation = stacks-as-code via audited heat-create") —
// driven through the QC-11 typed cloud verbs' PUBLIC Bus interface. The broker
// never speaks raw OpenStack (§9): it builds the verb's own typed [`HeatRequest`]
// body and publishes on `action/cloud/<verb>`. Display stays the existing viewer
// path: a placed desktop folds into the SAME [`SessionRequest::Open`] wire verb
// the shell's chooser/discovery already render, so `mde-vdi-rdp`/`mde-vdi-vnc`
// render it unchanged — no new render code.

/// Bus topic the broker drains for desktop **placement** requests. A shell that
/// wants a fresh VDI desktop publishes a [`DesktopSpec`] here; the broker places
/// it as a Nova instance and emits the placed session onto [`ACTION_TOPIC`].
pub const PLACE_TOPIC: &str = "action/vdi/place";

/// The VDI desktop size class the requester picks — mapped onto the
/// capacity-derived Nova flavor ladder (`m1.*`, design Q39) by [`select_flavor`].
/// The `m1.tiny` rung is intentionally unreachable: a desktop needs at least the
/// `small` shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopClass {
    /// A light desktop → `m1.small`.
    Small,
    /// The default desktop → `m1.medium`.
    #[default]
    Standard,
    /// A heavy desktop → `m1.large`.
    Large,
}

/// Select the Nova flavor name for a desktop class. Q39 flavors are
/// capacity-derived + named `m1.*`; this maps a desktop tier onto that ladder
/// (Nova/Placement resolves the concrete shape from the live flavor set).
#[must_use]
pub const fn select_flavor(class: DesktopClass) -> &'static str {
    match class {
        DesktopClass::Small => "m1.small",
        DesktopClass::Standard => "m1.medium",
        DesktopClass::Large => "m1.large",
    }
}

/// How a desktop is placed. A first boot is [`PlacementMode::Create`]; **roaming =
/// rebuild** (design Q38 — no live migration; a roamed desktop is *rebuilt* from
/// its image/volume), which re-applies the SAME managed stack via `heat-update`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlacementMode {
    /// First placement — create the managed stack (`heat-create`).
    #[default]
    Create,
    /// Roam/refresh — rebuild the desktop by re-applying its stack (`heat-update`),
    /// per Q38 (rebuild, not migrate).
    Rebuild,
}

impl PlacementMode {
    /// The QC-11 cloud verb this mode drives.
    #[must_use]
    pub const fn verb(self) -> &'static str {
        match self {
            Self::Create => "heat-create",
            Self::Rebuild => "heat-update",
        }
    }
}

/// A desktop-placement request: what desktop to place, for whom. The wire verb the
/// shell publishes on [`PLACE_TOPIC`]; the broker turns it into a Nova instance
/// (flavor + VDI metadata) and then owns the resulting session's display path.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DesktopSpec {
    /// The session id to mint for the placed desktop (roster + plane key).
    pub session_id: SessionId,
    /// The peer whose shell drives the desktop (the display target / seat).
    pub client_peer: NodeId,
    /// The member the desktop belongs to — rides as VDI metadata and carries the
    /// per-user quota attribution (§8 hard quotas).
    pub owner: String,
    /// The desktop size class → the selected Nova flavor.
    #[serde(default)]
    pub class: DesktopClass,
    /// The Glance image to boot (the desktop OS).
    pub image: String,
    /// The Neutron network to attach; `None` leaves the choice to Nova/Heat (the
    /// flat provider net) — never guessed here (§7).
    #[serde(default)]
    pub network: Option<String>,
    /// Create vs. rebuild (roaming = rebuild, Q38).
    #[serde(default)]
    pub mode: PlacementMode,
}

/// Parse a [`DesktopSpec`] placement request body.
///
/// # Errors
/// A human-readable message on malformed JSON.
pub fn parse_placement_request(body: &str) -> Result<DesktopSpec, String> {
    serde_json::from_str(body).map_err(|e| format!("malformed desktop placement request: {e}"))
}

/// Quote a scalar for a HOT template value: quote when empty or when it holds a
/// YAML-significant char (`:` / `#`) or edge whitespace — a [`NodeId`] like
/// `peer:host` MUST be quoted. Mirrors the Cloud plane's launch composer.
fn yaml_scalar(value: &str) -> String {
    if value.is_empty() || value.contains(':') || value.contains('#') || value != value.trim() {
        format!("{value:?}")
    } else {
        value.to_string()
    }
}

/// The VDI metadata a placed desktop carries as Nova server metadata — the tags
/// the broker binds the session to. `mde:vdi` marks it a broker desktop; the
/// session/client/owner tags let any node's broker recognise + seat it, and the
/// owner tag ties it to the per-user quota (§8).
#[must_use]
pub fn vdi_metadata(spec: &DesktopSpec) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    m.insert("mde:vdi".to_string(), "true".to_string());
    m.insert("mde:session".to_string(), spec.session_id.clone());
    m.insert("mde:client".to_string(), spec.client_peer.clone());
    m.insert("mde:owner".to_string(), spec.owner.clone());
    m
}

/// The managed-stack name for a desktop session (`vdi-<session>`) — stable across
/// create + rebuild so a roam re-applies the SAME stack (Q38).
#[must_use]
pub fn placement_stack_name(session_id: &str) -> String {
    format!("vdi-{session_id}")
}

/// Compose the desktop's HOT template: one `OS::Nova::Server` from the selected
/// image + flavor + optional network, carrying the [`vdi_metadata`]. Mirrors the
/// Cloud plane's `launch_hot` shape (§6) but stamps the VDI metadata that makes
/// the instance a broker desktop.
#[must_use]
pub fn build_vdi_template(spec: &DesktopSpec, flavor: &str) -> String {
    use std::fmt::Write as _;
    let mut out = format!(
        "heat_template_version: {HOT_TEMPLATE_VERSION}\n\ndescription: >-\n  A VDI desktop \
         placed by the MCNF session-broker (QC-14).\n\nresources:\n"
    );
    out.push_str("  server:\n    type: OS::Nova::Server\n    properties:\n");
    let _ = writeln!(
        out,
        "      name: {}",
        yaml_scalar(&placement_stack_name(&spec.session_id))
    );
    let _ = writeln!(out, "      image: {}", yaml_scalar(&spec.image));
    let _ = writeln!(out, "      flavor: {}", yaml_scalar(flavor));
    if let Some(net) = spec.network.as_deref() {
        out.push_str("      networks:\n");
        let _ = writeln!(out, "        - network: {}", yaml_scalar(net));
    }
    out.push_str("      metadata:\n");
    for (k, v) in vdi_metadata(spec) {
        let _ = writeln!(out, "        {}: {}", yaml_scalar(&k), yaml_scalar(&v));
    }
    out
}

/// The typed placement request the broker drives through the QC-11 verb interface
/// — the cloud verb to publish plus its public [`HeatRequest`] body (§6: the verb's
/// OWN request type, no parallel model) plus the selected flavor (echoed).
///
/// (No `PartialEq` — the embedded [`HeatRequest`] doesn't derive it; tests assert on
/// the individual echoed fields.)
#[derive(Debug, Clone)]
pub struct NovaPlacementRequest {
    /// The QC-11 cloud verb (`heat-create` on create, `heat-update` on rebuild).
    pub verb: &'static str,
    /// The verb's typed body — the stack name + the HOT template.
    pub heat: HeatRequest,
    /// The Nova flavor the desktop was sized to.
    pub flavor: &'static str,
    /// The session this placement serves (echoed for the broker's fold).
    pub session_id: SessionId,
    /// The desktop's display target / seat.
    pub client_peer: NodeId,
}

impl NovaPlacementRequest {
    /// The `action/cloud/<verb>` Bus topic this placement publishes on.
    #[must_use]
    pub fn topic(&self) -> String {
        cloud_action_topic(self.verb)
    }
}

/// Build the Nova placement request for a desktop spec (pure — flavor selection +
/// VDI-metadata HOT template + the verb). No clock, no bus.
#[must_use]
pub fn build_placement(spec: &DesktopSpec) -> NovaPlacementRequest {
    let flavor = select_flavor(spec.class);
    let heat = HeatRequest {
        stack_name: placement_stack_name(&spec.session_id),
        template: build_vdi_template(spec, flavor),
        ..HeatRequest::default()
    };
    NovaPlacementRequest {
        verb: spec.mode.verb(),
        heat,
        flavor,
        session_id: spec.session_id.clone(),
        client_peer: spec.client_peer.clone(),
    }
}

/// The result of a successful Nova placement: the booted desktop's identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedDesktop {
    /// The Nova server id (libvirt UUID — a [`VmId`]) of the placed desktop.
    pub vm_id: VmId,
    /// The compute host Nova placed the desktop on — the session's serving peer.
    pub serving_host: NodeId,
}

/// The broker session-open a placed desktop folds into — the SAME `Open` wire verb
/// the shell publishes, now pointing at the broker-placed instance so the existing
/// viewer path (`mde-vdi-*`) renders it (§6 — no new session type / render code).
#[must_use]
pub fn placed_open(spec: &DesktopSpec, placed: &PlacedDesktop) -> SessionRequest {
    SessionRequest::Open {
        id: spec.session_id.clone(),
        serving_peer: placed.serving_host.clone(),
        vm_id: placed.vm_id.clone(),
        client_peer: spec.client_peer.clone(),
    }
}

/// A typed failure from the [`NovaPlacement`] seam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlacementError {
    /// The live cloud isn't reachable to actually boot the desktop yet — names the
    /// verb + what the live call needs. §7-legal (mirrors [`SessionStoreError`] +
    /// `onboard::first_desktop::FirstDesktopError`): a real typed error, never a
    /// fake vm_id.
    IntegrationGated {
        /// The cloud verb that would place the desktop.
        verb: &'static str,
        /// What the live call needs.
        reason: String,
    },
    /// A placement failed for a concrete runtime reason.
    Failed {
        /// The cloud verb that failed.
        verb: &'static str,
        /// The failure detail.
        reason: String,
    },
}

impl std::fmt::Display for PlacementError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IntegrationGated { verb, reason } => {
                write!(f, "{verb}: integration-gated — {reason}")
            }
            Self::Failed { verb, reason } => write!(f, "{verb}: {reason}"),
        }
    }
}

impl std::error::Error for PlacementError {}

/// The injectable Nova-placement seam: turn a [`NovaPlacementRequest`] into a
/// booted desktop by driving the QC-11 cloud verb. Production wires
/// [`LiveNovaPlacement`]; tests drive a fake so the whole place → emit → fold →
/// converge pipeline runs without a live cloud.
pub trait NovaPlacement {
    /// Place (boot) the desktop the request describes.
    ///
    /// # Errors
    /// A [`PlacementError`] — `IntegrationGated` until a live Nova+Heat cloud is
    /// reachable via the verb, else `Failed`.
    fn place(&self, req: &NovaPlacementRequest) -> Result<PlacedDesktop, PlacementError>;
}

/// Production [`NovaPlacement`]: drives the desktop's `heat-create` / `heat-update`
/// verb over the Bus to a live Nova+Heat cloud.
///
/// This slice (QC-14) delivers the pure placement (flavor + VDI-metadata template)
/// + the seam; the live executor (publish the `action/cloud/<verb>` request + await
/// the `reply/<ulid>` carrying the booted server id) is wired by a later live-cloud
/// unit — exactly as [`MeshSessionStore`] gates on the live etcd writer. Until a
/// live cloud exists on this host, `place` returns a typed
/// [`PlacementError::IntegrationGated`] naming the verb + the missing live dep —
/// never a fake boot (§7, "the live boot honest-gates until a live cloud exists").
#[derive(Debug, Clone, Copy, Default)]
pub struct LiveNovaPlacement;

impl LiveNovaPlacement {
    /// Construct the production placement seam.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl NovaPlacement for LiveNovaPlacement {
    fn place(&self, req: &NovaPlacementRequest) -> Result<PlacedDesktop, PlacementError> {
        Err(PlacementError::IntegrationGated {
            verb: req.verb,
            reason: format!(
                "desktop stack `{}` (flavor {}) → needs a live Nova+Heat cloud reachable via the \
                 `{}` verb on `{}` to boot the instance and return its server id; no live cloud is \
                 present on this host yet",
                req.heat.stack_name,
                req.flavor,
                req.verb,
                req.topic()
            ),
        })
    }
}

// ─────────────────────────── store seam ───────────────────────────

/// A typed failure from the [`SessionStore`] seam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStoreError {
    /// The live cross-peer plane isn't wired in this build/environment yet — it
    /// needs a real prerequisite (the etcd session-lease writer over Nebula).
    /// Names the op + what is missing. §7-legal: a real method returning a real
    /// typed error, exactly as `adopt_xcp::LiveAdopter` does.
    IntegrationGated {
        /// Which store op (`publish` / `list` / `remove`).
        op: &'static str,
        /// What the live call needs before it can run.
        reason: String,
    },
    /// A store op failed for a concrete runtime reason.
    Failed {
        /// Which store op failed.
        op: &'static str,
        /// The failure detail.
        reason: String,
    },
}

impl std::fmt::Display for SessionStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IntegrationGated { op, reason } => {
                write!(f, "{op}: integration-gated — {reason}")
            }
            Self::Failed { op, reason } => write!(f, "{op}: {reason}"),
        }
    }
}

impl std::error::Error for SessionStoreError {}

/// The injectable shared-session-plane seam: publish / list / remove a
/// [`VdiSession`] in mesh state.
///
/// Production wires [`MeshSessionStore`]; the tests drive an in-memory fake so the
/// whole drain → fold → reconcile → apply pipeline runs without etcd.
pub trait SessionStore {
    /// Publish (create or update) `session` in the shared plane.
    ///
    /// # Errors
    /// A [`SessionStoreError`] — `IntegrationGated` until the live etcd writer
    /// lands, else `Failed`.
    fn publish(&self, session: &VdiSession) -> Result<(), SessionStoreError>;

    /// List every session record currently in the shared plane.
    ///
    /// # Errors
    /// A [`SessionStoreError`] — `IntegrationGated` until the live etcd reader
    /// lands, else `Failed`.
    fn list(&self) -> Result<Vec<VdiSession>, SessionStoreError>;

    /// Remove the session `id` from the shared plane.
    ///
    /// # Errors
    /// A [`SessionStoreError`] — `IntegrationGated` until the live etcd deleter
    /// lands, else `Failed`.
    fn remove(&self, id: &str) -> Result<(), SessionStoreError>;
}

/// Production [`SessionStore`]: the roaming-session plane.
///
/// The authoritative cross-peer session directory on the etcd keepalive-leased
/// coordination plane (Syncthing-replicated fallback), the substrate design lock 5
/// roams sessions on.
///
/// This slice (E12-5b) delivers the pure core + the seam; the live executor (the
/// etcd session-lease writer/reader/deleter reached over the Nebula overlay) is
/// wired by a later E12 unit. Until then each method returns a typed
/// [`SessionStoreError::IntegrationGated`] naming exactly what the live call needs
/// — never a fake success (§7).
#[derive(Debug, Clone)]
pub struct MeshSessionStore {
    /// Shared-storage root — the Syncthing-replicated fallback plane + where the
    /// leader lock lives.
    workgroup_root: PathBuf,
}

impl MeshSessionStore {
    /// Construct over the mesh `workgroup_root` (the replicated shared volume).
    #[must_use]
    pub const fn new(workgroup_root: PathBuf) -> Self {
        Self { workgroup_root }
    }
}

impl SessionStore for MeshSessionStore {
    fn publish(&self, session: &VdiSession) -> Result<(), SessionStoreError> {
        Err(SessionStoreError::IntegrationGated {
            op: "publish",
            reason: format!(
                "session {} → needs the live etcd session-lease writer over Nebula (the \
                 roaming-session plane under {}); the cross-peer connect isn't wired yet",
                session.id,
                self.workgroup_root.display()
            ),
        })
    }

    fn list(&self) -> Result<Vec<VdiSession>, SessionStoreError> {
        Err(SessionStoreError::IntegrationGated {
            op: "list",
            reason: format!(
                "needs the live etcd session-directory reader over Nebula (the roaming-session \
                 plane under {})",
                self.workgroup_root.display()
            ),
        })
    }

    fn remove(&self, id: &str) -> Result<(), SessionStoreError> {
        Err(SessionStoreError::IntegrationGated {
            op: "remove",
            reason: format!(
                "session {id} → needs the live etcd session-lease deleter over Nebula (the \
                 roaming-session plane under {})",
                self.workgroup_root.display()
            ),
        })
    }
}

// ─────────────────────────── bus + worker ───────────────────────────

/// Read new [`ACTION_TOPIC`] messages since `cursor`, advancing it. A short sync
/// open-read-drop (never crosses an `.await`), mirroring `scheduler`.
fn read_new_actions(bus_root: &Path, cursor: &mut Option<String>) -> Vec<SessionRequest> {
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
                tracing::warn!(ulid = %msg.ulid, error = %e, "session_broker: bad session request");
            }
        }
    }
    out
}

/// Read new [`PLACE_TOPIC`] messages since `cursor`, advancing it. Mirrors
/// [`read_new_actions`]; a malformed placement request is dropped honestly.
fn read_new_placements(bus_root: &Path, cursor: &mut Option<String>) -> Vec<DesktopSpec> {
    let Ok(persist) = Persist::open(bus_root.to_path_buf()) else {
        return vec![];
    };
    let Ok(msgs) = persist.list_since(PLACE_TOPIC, cursor.as_deref()) else {
        return vec![];
    };
    let mut out = Vec::new();
    for msg in msgs {
        *cursor = Some(msg.ulid.clone());
        let body = msg.body.as_deref().unwrap_or("");
        match parse_placement_request(body) {
            Ok(s) => out.push(s),
            Err(e) => {
                tracing::warn!(ulid = %msg.ulid, error = %e, "session_broker: bad placement request");
            }
        }
    }
    out
}

/// Fold new `action/vdi/session` messages (advancing `cursor`) into `roster`.
/// Runs on every node (the log is mesh-replicated), so any node has a warm roster
/// ready to converge if it wins the election. A malformed op is dropped honestly.
fn drain(
    bus_root: &Path,
    cursor: &mut Option<String>,
    roster: &mut BTreeMap<SessionId, VdiSession>,
) {
    for req in read_new_actions(bus_root, cursor) {
        if let Err(e) = apply_request(roster, req, now_ms()) {
            tracing::warn!(error = %e, "session_broker: dropping unresolvable session op");
        }
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

/// The VDI session-broker worker. Leader-gated + best-effort.
pub struct SessionBrokerWorker {
    /// The injectable shared-plane seam (production: [`MeshSessionStore`]).
    store: Box<dyn SessionStore + Send + Sync>,
    /// The injectable Nova-placement seam (production: [`LiveNovaPlacement`]) —
    /// QC-14. Turns a drained [`DesktopSpec`] into a booted Nova instance.
    placement: Box<dyn NovaPlacement + Send + Sync>,
    /// This node's id — its identity in the leader election.
    node_id: NodeId,
    /// The shared leader lock (the same `.mackesd-leader.lock` `dc_auditor` uses).
    leader_lock: PathBuf,
    /// Convergence cadence.
    poll: Duration,
    /// Bus root override (tests). `None` ⇒ [`default_bus_root`].
    bus_root_override: Option<PathBuf>,
}

impl SessionBrokerWorker {
    /// Construct with production defaults: the etcd-first [`MeshSessionStore`] over
    /// `workgroup_root`, the shared leader lock under it, and the default cadence.
    /// `node_id` is this node's mesh identity.
    #[must_use]
    pub fn new(workgroup_root: PathBuf, node_id: NodeId) -> Self {
        // Derive the lock path first, then move `workgroup_root` into the store.
        let leader_lock = workgroup_root.join(".mackesd-leader.lock");
        Self {
            store: Box::new(MeshSessionStore::new(workgroup_root)),
            placement: Box::new(LiveNovaPlacement::new()),
            node_id,
            leader_lock,
            poll: DEFAULT_POLL_INTERVAL,
            bus_root_override: None,
        }
    }

    /// Inject a session store (tests). Production uses [`MeshSessionStore`].
    #[must_use]
    pub fn with_store(mut self, store: Box<dyn SessionStore + Send + Sync>) -> Self {
        self.store = store;
        self
    }

    /// Inject a Nova-placement seam (tests). Production uses [`LiveNovaPlacement`].
    #[must_use]
    pub fn with_placement(mut self, placement: Box<dyn NovaPlacement + Send + Sync>) -> Self {
        self.placement = placement;
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

    /// Leader-only: reconcile the local `roster` against the shared plane and apply
    /// the resulting [`SessionAction`]s through the store, then prune converged
    /// terminal sessions from the fold. Best-effort — a gated / failed store defers
    /// this tick (honest, never a fake success).
    fn converge(&self, roster: &mut BTreeMap<SessionId, VdiSession>) {
        if !self.is_leader() {
            return;
        }
        let observed: BTreeMap<SessionId, VdiSession> = match self.store.list() {
            Ok(rows) => rows.into_iter().map(|s| (s.id.clone(), s)).collect(),
            Err(e @ SessionStoreError::IntegrationGated { .. }) => {
                tracing::info!(error = %e, "session_broker: store integration-gated; deferring");
                return;
            }
            Err(e) => {
                tracing::warn!(error = %e, "session_broker: store list failed; deferring");
                return;
            }
        };
        let desired: Vec<VdiSession> = roster.values().cloned().collect();
        for action in reconcile(&desired, &observed) {
            let res = match &action {
                SessionAction::Publish(s) => self.store.publish(s),
                SessionAction::Remove(id) => self.store.remove(id),
            };
            if let Err(e) = res {
                tracing::warn!(error = %e, "session_broker: convergence action failed");
            }
        }
        // Drop converged terminal sessions so `Closed` rows don't accumulate in the
        // in-memory fold (they've been removed from the shared plane). The action
        // log still carries them, so a restart re-derives + re-removes idempotently.
        roster.retain(|_, s| s.state.is_publishable());
    }

    /// Place one desktop as a Nova instance through the QC-14 seam: build the
    /// request (flavor + VDI metadata) and drive it through the injected
    /// [`NovaPlacement`]. Pure request build + the seam — no bus write here (the
    /// caller emits the placed session).
    ///
    /// # Errors
    /// The seam's [`PlacementError`] — `IntegrationGated` until a live cloud exists.
    fn place_desktop(&self, spec: &DesktopSpec) -> Result<PlacedDesktop, PlacementError> {
        self.placement.place(&build_placement(spec))
    }

    /// Leader-only: drain [`PLACE_TOPIC`], place each desktop through the seam, and
    /// emit the placed session onto [`ACTION_TOPIC`] (the existing wire path the
    /// roster folds + the viewer renders). A gated / failed placement defers
    /// honestly — no fake session is emitted.
    ///
    /// The cursor advances on **every** node (each holds its own copy of the
    /// mesh-replicated log), but only the elected leader BOOTS — so an N-node mesh
    /// never multi-boots one desktop, and a placement isn't re-booted on a leader
    /// change (the leader consumed its own copy).
    fn drain_placements(&self, bus_root: &Path, cursor: &mut Option<String>) {
        let specs = read_new_placements(bus_root, cursor);
        if specs.is_empty() || !self.is_leader() {
            return;
        }
        let Ok(persist) = Persist::open(bus_root.to_path_buf()) else {
            tracing::warn!("session_broker: cannot open bus to emit placed sessions");
            return;
        };
        for spec in specs {
            match self.place_desktop(&spec) {
                Ok(placed) => {
                    let body = match serde_json::to_string(&placed_open(&spec, &placed)) {
                        Ok(b) => b,
                        Err(e) => {
                            tracing::warn!(error = %e, "session_broker: serialize placed session");
                            continue;
                        }
                    };
                    let priority = mde_bus::hooks::config::Priority::Default;
                    match persist.write(ACTION_TOPIC, priority, None, Some(&body)) {
                        Ok(_) => tracing::info!(
                            session = %spec.session_id,
                            vm = %placed.vm_id,
                            host = %placed.serving_host,
                            "session_broker: placed VDI desktop as a Nova instance"
                        ),
                        Err(e) => {
                            tracing::warn!(error = %e, "session_broker: emit placed session failed");
                        }
                    }
                }
                Err(e @ PlacementError::IntegrationGated { .. }) => tracing::info!(
                    error = %e,
                    session = %spec.session_id,
                    "session_broker: desktop placement integration-gated; deferring"
                ),
                Err(e) => tracing::warn!(
                    error = %e,
                    session = %spec.session_id,
                    "session_broker: desktop placement failed"
                ),
            }
        }
    }
}

#[async_trait::async_trait]
impl Worker for SessionBrokerWorker {
    fn name(&self) -> &'static str {
        "session_broker"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let Some(bus_root) = self.bus_root() else {
            tracing::debug!("session_broker: no bus root; worker idle");
            return Ok(());
        };
        // Read the FULL action log from the start (unlike `scheduler`, which primes
        // past the backlog): a session's state is a fold of the whole log, so a
        // (re)start must rebuild the complete roster before it converges.
        let mut cursor: Option<String> = None;
        let mut place_cursor: Option<String> = None;
        let mut roster: BTreeMap<SessionId, VdiSession> = BTreeMap::new();
        let mut tick = tokio::time::interval(self.poll);
        tick.tick().await; // consume the immediate first tick
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    // Place first (emits `Open`s onto ACTION_TOPIC), then fold the
                    // whole session log — incl. those just-emitted — so a placed
                    // desktop converges this same tick.
                    self.drain_placements(&bus_root, &mut place_cursor);
                    drain(&bus_root, &mut cursor, &mut roster);
                    self.converge(&mut roster);
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

    fn sess(id: &str, state: SessionState) -> VdiSession {
        VdiSession {
            id: id.to_string(),
            serving_peer: "peer:host".to_string(),
            vm_id: "uuid-1".to_string(),
            client_peer: "peer:client".to_string(),
            state,
            opened_at_ms: 100,
            updated_at_ms: 100,
        }
    }

    fn roster_of(sessions: &[VdiSession]) -> BTreeMap<SessionId, VdiSession> {
        sessions.iter().map(|s| (s.id.clone(), s.clone())).collect()
    }

    // ── state machine (open / active / disconnect / close) ──

    #[test]
    fn open_session_starts_requested_with_seeded_times() {
        let s = open_session(
            "s1".into(),
            "peer:a".into(),
            "uuid-9".into(),
            "peer:b".into(),
            4242,
        );
        assert_eq!(s.state, SessionState::Requested);
        assert_eq!(s.serving_peer, "peer:a");
        assert_eq!(s.vm_id, "uuid-9");
        assert_eq!(s.client_peer, "peer:b");
        assert_eq!(s.opened_at_ms, 4242);
        assert_eq!(s.updated_at_ms, 4242);
    }

    #[test]
    fn mark_active_from_requested_disconnected_and_idempotent() {
        for from in [
            SessionState::Requested,
            SessionState::Active,
            SessionState::Disconnected,
        ] {
            let s = mark_active(&sess("s", from), 200).expect("valid → active");
            assert_eq!(s.state, SessionState::Active);
            assert_eq!(s.updated_at_ms, 200, "the transition refreshes the clock");
        }
    }

    #[test]
    fn mark_active_rejects_a_closed_session() {
        let err = mark_active(&sess("s", SessionState::Closed), 200).unwrap_err();
        assert_eq!(
            err,
            SessionError::IllegalTransition {
                from: SessionState::Closed,
                to: SessionState::Active,
            }
        );
    }

    #[test]
    fn mark_disconnected_rules() {
        // Active / Disconnected → Disconnected.
        for from in [SessionState::Active, SessionState::Disconnected] {
            let s = mark_disconnected(&sess("s", from), 300).expect("valid → disconnected");
            assert_eq!(s.state, SessionState::Disconnected);
        }
        // Requested (never connected) + Closed (terminal) are rejected.
        for from in [SessionState::Requested, SessionState::Closed] {
            assert!(matches!(
                mark_disconnected(&sess("s", from), 300),
                Err(SessionError::IllegalTransition { .. })
            ));
        }
    }

    #[test]
    fn close_is_terminal_and_valid_from_any_state() {
        for from in [
            SessionState::Requested,
            SessionState::Active,
            SessionState::Disconnected,
            SessionState::Closed,
        ] {
            let s = close_session(&sess("s", from), 500);
            assert_eq!(s.state, SessionState::Closed);
            assert!(s.state.is_terminal());
            assert!(!s.state.is_publishable());
        }
    }

    // ── apply_request (the incremental fold) ──

    #[test]
    fn apply_request_folds_a_full_lifecycle() {
        let mut roster = BTreeMap::new();
        apply_request(
            &mut roster,
            SessionRequest::Open {
                id: "s1".into(),
                serving_peer: "peer:a".into(),
                vm_id: "uuid-1".into(),
                client_peer: "peer:b".into(),
            },
            1,
        )
        .expect("open");
        assert_eq!(roster["s1"].state, SessionState::Requested);
        apply_request(&mut roster, SessionRequest::Active { id: "s1".into() }, 2).expect("active");
        assert_eq!(roster["s1"].state, SessionState::Active);
        apply_request(
            &mut roster,
            SessionRequest::Disconnect { id: "s1".into() },
            3,
        )
        .expect("disconnect");
        assert_eq!(roster["s1"].state, SessionState::Disconnected);
        apply_request(&mut roster, SessionRequest::Close { id: "s1".into() }, 4).expect("close");
        assert_eq!(roster["s1"].state, SessionState::Closed);
        assert_eq!(roster["s1"].opened_at_ms, 1, "opened_at is preserved");
        assert_eq!(
            roster["s1"].updated_at_ms, 4,
            "updated_at tracks the last op"
        );
    }

    #[test]
    fn apply_request_unknown_and_illegal_ops_error() {
        let mut roster = BTreeMap::new();
        // A transition on an id the roster never opened.
        assert_eq!(
            apply_request(
                &mut roster,
                SessionRequest::Active { id: "ghost".into() },
                1
            ),
            Err(SessionError::UnknownSession("ghost".into()))
        );
        assert_eq!(
            apply_request(&mut roster, SessionRequest::Close { id: "ghost".into() }, 1),
            Err(SessionError::UnknownSession("ghost".into()))
        );
        // A forbidden transition on a real row.
        roster.insert("s".into(), sess("s", SessionState::Closed));
        assert!(matches!(
            apply_request(&mut roster, SessionRequest::Active { id: "s".into() }, 1),
            Err(SessionError::IllegalTransition { .. })
        ));
    }

    // ── reconcile (leader convergence) ──

    #[test]
    fn reconcile_publishes_a_new_active_session() {
        let desired = vec![sess("s1", SessionState::Active)];
        let out = reconcile(&desired, &BTreeMap::new());
        assert_eq!(
            out,
            vec![SessionAction::Publish(sess("s1", SessionState::Active))]
        );
    }

    #[test]
    fn reconcile_republishes_a_changed_session_only() {
        let desired = vec![sess("s1", SessionState::Active)];
        // Observed holds s1 in an older state ⇒ re-publish.
        let observed = roster_of(&[sess("s1", SessionState::Requested)]);
        assert_eq!(
            reconcile(&desired, &observed),
            vec![SessionAction::Publish(sess("s1", SessionState::Active))]
        );
        // Observed already byte-identical ⇒ no action (no needless write).
        let converged = roster_of(&[sess("s1", SessionState::Active)]);
        assert!(reconcile(&desired, &converged).is_empty());
    }

    #[test]
    fn reconcile_removes_closed_and_stale_rows() {
        // s1 is Closed in desired but still in the plane ⇒ Remove.
        // s2 (Active) is desired + absent ⇒ Publish.
        // s3 lingers in the plane but the leader no longer tracks it ⇒ Remove.
        let desired = vec![
            sess("s1", SessionState::Closed),
            sess("s2", SessionState::Active),
        ];
        let observed = roster_of(&[
            sess("s1", SessionState::Active),
            sess("s3", SessionState::Active),
        ]);
        let out = reconcile(&desired, &observed);
        assert_eq!(
            out,
            vec![
                SessionAction::Remove("s1".into()),
                SessionAction::Publish(sess("s2", SessionState::Active)),
                SessionAction::Remove("s3".into()),
            ]
        );
    }

    #[test]
    fn reconcile_is_deterministic() {
        let desired = vec![
            sess("s2", SessionState::Active),
            sess("s1", SessionState::Active),
        ];
        let observed = BTreeMap::new();
        // Repeat runs are byte-identical + id-sorted regardless of input order.
        let a = reconcile(&desired, &observed);
        let b = reconcile(&desired, &observed);
        assert_eq!(a, b);
        assert_eq!(
            a,
            vec![
                SessionAction::Publish(sess("s1", SessionState::Active)),
                SessionAction::Publish(sess("s2", SessionState::Active)),
            ]
        );
    }

    #[test]
    fn reconcile_closed_absent_is_a_noop() {
        // A Closed desired session the plane never held ⇒ nothing to remove.
        let desired = vec![sess("s1", SessionState::Closed)];
        assert!(reconcile(&desired, &BTreeMap::new()).is_empty());
    }

    // ── serde / parsing ──

    #[test]
    fn session_state_serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&SessionState::Disconnected).unwrap(),
            "\"disconnected\""
        );
    }

    #[test]
    fn parse_request_round_trips_ops() {
        let open = parse_request(
            r#"{"op":"open","id":"s1","serving_peer":"peer:a","vm_id":"u1","client_peer":"peer:b"}"#,
        )
        .expect("open parses");
        assert_eq!(
            open,
            SessionRequest::Open {
                id: "s1".into(),
                serving_peer: "peer:a".into(),
                vm_id: "u1".into(),
                client_peer: "peer:b".into(),
            }
        );
        assert_eq!(
            parse_request(r#"{"op":"close","id":"s1"}"#).expect("close parses"),
            SessionRequest::Close { id: "s1".into() }
        );
        assert!(parse_request("nonsense").is_err());
        assert!(parse_request(r#"{"op":"teleport","id":"s1"}"#).is_err());
    }

    #[test]
    fn topic_is_namespaced() {
        assert_eq!(ACTION_TOPIC, "action/vdi/session");
        assert!(ACTION_TOPIC.starts_with("action/"));
    }

    // ── the store seam ──

    #[test]
    fn mesh_session_store_is_integration_gated_not_faked() {
        let store = MeshSessionStore::new(PathBuf::from("/tmp/mesh-wg"));
        let s = sess("s1", SessionState::Active);
        // Every method returns a typed IntegrationGated (§7 — never a fake Ok).
        for (label, err) in [
            ("publish", store.publish(&s).unwrap_err()),
            ("list", store.list().map(|_| ()).unwrap_err()),
            ("remove", store.remove("s1").unwrap_err()),
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
                    panic!("expected an integration-gated error, got Failed {{{op}: {reason}}}")
                }
            }
        }
    }

    /// An in-memory [`SessionStore`] — the Fake seam. The map is an `Arc` so a test
    /// clones a handle before moving the store into the worker.
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

    #[test]
    fn fake_store_round_trips() {
        let store = FakeStore::default();
        store.publish(&sess("s1", SessionState::Active)).unwrap();
        assert_eq!(store.list().unwrap().len(), 1);
        store.remove("s1").unwrap();
        assert!(store.list().unwrap().is_empty());
    }

    #[test]
    fn worker_name_matches_module() {
        let w = SessionBrokerWorker::new(std::env::temp_dir(), "peer:a".to_string());
        assert_eq!(w.name(), "session_broker");
    }

    // ── worker wiring (seeded temp bus + injected fake store) ──

    /// Seed a temp bus with `action/vdi/session` bodies and return its root.
    fn seed_bus(reqs: &[SessionRequest]) -> PathBuf {
        use mde_bus::hooks::config::Priority;
        let dir = std::env::temp_dir().join(format!("mde-sb-{}-{}", now_ms(), reqs.len()));
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
    async fn worker_drains_folds_and_converges_into_the_store() {
        // A session that opened + went active, drained off the bus and converged
        // into the injected store by the leader (a fresh temp workgroup ⇒ this
        // node wins the lock).
        let bus = seed_bus(&[
            SessionRequest::Open {
                id: "s1".into(),
                serving_peer: "peer:a".into(),
                vm_id: "uuid-1".into(),
                client_peer: "peer:b".into(),
            },
            SessionRequest::Active { id: "s1".into() },
        ]);
        let wg = std::env::temp_dir().join(format!("mde-sb-wg-{}", now_ms()));
        std::fs::create_dir_all(&wg).expect("mk workgroup");
        let store = FakeStore::default();
        let rows = store.rows.clone();
        let w = SessionBrokerWorker::new(wg.clone(), "peer:a".to_string())
            .with_store(Box::new(store))
            .with_bus_root(bus.clone());

        let mut cursor = None;
        let mut roster = BTreeMap::new();
        drain(&bus, &mut cursor, &mut roster);
        assert_eq!(roster["s1"].state, SessionState::Active, "folded to Active");
        w.converge(&mut roster);

        let published = rows.lock().expect("rows mutex");
        assert_eq!(
            published.len(),
            1,
            "the leader published the active session"
        );
        assert_eq!(published["s1"].state, SessionState::Active);
        assert_eq!(published["s1"].serving_peer, "peer:a");
        drop(published);

        // A subsequent Close drains, converges to a Remove, and is pruned.
        let mut cursor2 = cursor;
        // Append a Close to the same bus + re-drain from the advanced cursor.
        {
            use mde_bus::hooks::config::Priority;
            let persist = Persist::open(bus.clone()).expect("reopen bus");
            persist
                .write(
                    ACTION_TOPIC,
                    Priority::Default,
                    None,
                    Some(
                        &serde_json::to_string(&SessionRequest::Close { id: "s1".into() }).unwrap(),
                    ),
                )
                .expect("write close");
        }
        drain(&bus, &mut cursor2, &mut roster);
        w.converge(&mut roster);
        assert!(
            rows.lock().expect("rows mutex").is_empty(),
            "the closed session was removed from the plane"
        );
        assert!(roster.is_empty(), "the converged terminal row was pruned");

        let _ = std::fs::remove_dir_all(&bus);
        let _ = std::fs::remove_dir_all(&wg);
    }

    #[tokio::test]
    async fn run_loop_exits_promptly_on_shutdown() {
        // An empty temp bus ⇒ nothing to fold; the gated MeshSessionStore default
        // means no etcd is needed (converge defers honestly).
        let bus = std::env::temp_dir().join(format!("mde-sb-run-{}", now_ms()));
        let wg = std::env::temp_dir().join(format!("mde-sb-runwg-{}", now_ms()));
        std::fs::create_dir_all(&wg).expect("mk workgroup");
        let (tx, rx) = tokio::sync::watch::channel(false);
        let mut w = SessionBrokerWorker::new(wg.clone(), "peer:a".to_string())
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

    // ── QC-14: Nova placement (flavor selection + VDI metadata + the seam) ──

    fn spec(session: &str) -> DesktopSpec {
        DesktopSpec {
            session_id: session.to_string(),
            client_peer: "peer:client".to_string(),
            owner: "alice".to_string(),
            class: DesktopClass::Standard,
            image: "ubuntu-24.04".to_string(),
            network: Some("mesh-flat".to_string()),
            mode: PlacementMode::Create,
        }
    }

    /// A recording [`NovaPlacement`] fake — returns a fixed placed desktop and
    /// records the requests it saw, so the place → emit → fold → converge pipeline
    /// runs without a live cloud.
    #[derive(Clone)]
    struct FakePlacement {
        placed: PlacedDesktop,
        seen: Arc<Mutex<Vec<NovaPlacementRequest>>>,
    }

    impl FakePlacement {
        fn new(vm_id: &str, host: &str) -> Self {
            Self {
                placed: PlacedDesktop {
                    vm_id: vm_id.to_string(),
                    serving_host: host.to_string(),
                },
                seen: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    impl NovaPlacement for FakePlacement {
        fn place(&self, req: &NovaPlacementRequest) -> Result<PlacedDesktop, PlacementError> {
            self.seen.lock().expect("seen mutex").push(req.clone());
            Ok(self.placed.clone())
        }
    }

    #[test]
    fn select_flavor_maps_class_to_the_derived_ladder() {
        assert_eq!(select_flavor(DesktopClass::Small), "m1.small");
        assert_eq!(select_flavor(DesktopClass::Standard), "m1.medium");
        assert_eq!(select_flavor(DesktopClass::Large), "m1.large");
        assert_eq!(DesktopClass::default(), DesktopClass::Standard);
    }

    #[test]
    fn placement_mode_picks_the_cloud_verb() {
        assert_eq!(PlacementMode::Create.verb(), "heat-create");
        assert_eq!(PlacementMode::Rebuild.verb(), "heat-update");
        assert_eq!(PlacementMode::default(), PlacementMode::Create);
    }

    #[test]
    fn build_placement_selects_flavor_and_names_the_stack() {
        let req = build_placement(&spec("s1"));
        assert_eq!(req.verb, "heat-create");
        assert_eq!(req.flavor, "m1.medium");
        assert_eq!(req.heat.stack_name, "vdi-s1");
        assert_eq!(req.session_id, "s1");
        assert_eq!(req.client_peer, "peer:client");
        // The placement rides the public QC-11 heat verb topic (no raw OpenStack).
        assert_eq!(req.topic(), "action/cloud/heat-create");
    }

    #[test]
    fn build_placement_rebuild_reapplies_the_same_stack_via_heat_update() {
        // Roaming = rebuild (Q38): a rebuild keeps the stack name + drives
        // heat-update — the desktop is re-provisioned, not live-migrated.
        let mut s = spec("s1");
        s.mode = PlacementMode::Rebuild;
        let req = build_placement(&s);
        assert_eq!(req.verb, "heat-update");
        assert_eq!(
            req.heat.stack_name, "vdi-s1",
            "same managed stack across a roam"
        );
        assert_eq!(req.topic(), "action/cloud/heat-update");
    }

    #[test]
    fn vdi_metadata_tags_the_desktop_for_the_broker() {
        let m = vdi_metadata(&spec("s1"));
        assert_eq!(m.get("mde:vdi").map(String::as_str), Some("true"));
        assert_eq!(m.get("mde:session").map(String::as_str), Some("s1"));
        assert_eq!(m.get("mde:client").map(String::as_str), Some("peer:client"));
        assert_eq!(m.get("mde:owner").map(String::as_str), Some("alice"));
    }

    #[test]
    fn vdi_template_is_a_nova_server_with_flavor_image_and_metadata() {
        let hot = build_vdi_template(&spec("s1"), "m1.medium");
        assert!(hot.starts_with(&format!("heat_template_version: {HOT_TEMPLATE_VERSION}")));
        assert!(hot.contains("type: OS::Nova::Server"));
        assert!(hot.contains("name: vdi-s1"));
        assert!(hot.contains("flavor: m1.medium"));
        assert!(hot.contains("image: ubuntu-24.04"));
        assert!(hot.contains("network: mesh-flat"));
        assert!(hot.contains("metadata:"));
        // The metadata keys hold a `:` so YAML-quoting is mandatory; the client
        // peer id (also `:`) is quoted as a value.
        assert!(hot.contains("\"mde:vdi\": true"), "hot: {hot}");
        assert!(hot.contains("\"mde:session\": s1"), "hot: {hot}");
        assert!(
            hot.contains("\"mde:client\": \"peer:client\""),
            "peer id must be quoted: {hot}"
        );
    }

    #[test]
    fn vdi_template_omits_the_network_block_when_unpinned() {
        let mut s = spec("s1");
        s.network = None;
        let hot = build_vdi_template(&s, "m1.small");
        assert!(!hot.contains("networks:"), "no network guessed (§7): {hot}");
        assert!(hot.contains("flavor: m1.small"));
    }

    #[test]
    fn parse_placement_request_round_trips_and_defaults() {
        let body = serde_json::to_string(&spec("s1")).unwrap();
        assert_eq!(parse_placement_request(&body).unwrap(), spec("s1"));
        assert!(parse_placement_request("nonsense").is_err());
        // class + mode + network default when omitted.
        let minimal = r#"{"session_id":"s2","client_peer":"peer:c","owner":"bob","image":"win11"}"#;
        let d = parse_placement_request(minimal).unwrap();
        assert_eq!(d.class, DesktopClass::Standard);
        assert_eq!(d.mode, PlacementMode::Create);
        assert_eq!(d.network, None);
    }

    #[test]
    fn placed_open_points_the_session_at_the_placed_instance() {
        let placed = PlacedDesktop {
            vm_id: "uuid-boot".to_string(),
            serving_host: "peer:compute-3".to_string(),
        };
        // The placed desktop folds into the SAME `Open` wire verb the shell renders:
        // serving_peer is Nova's chosen host, vm_id the booted server, client the seat.
        assert_eq!(
            placed_open(&spec("s1"), &placed),
            SessionRequest::Open {
                id: "s1".into(),
                serving_peer: "peer:compute-3".into(),
                vm_id: "uuid-boot".into(),
                client_peer: "peer:client".into(),
            }
        );
    }

    #[test]
    fn live_placement_is_integration_gated_not_faked() {
        // §7: the live boot honest-gates until a live cloud exists — a real typed
        // error naming the verb + the missing dep, never a fabricated vm_id.
        let err = LiveNovaPlacement::new()
            .place(&build_placement(&spec("s1")))
            .unwrap_err();
        match err {
            PlacementError::IntegrationGated { verb, reason } => {
                assert_eq!(verb, "heat-create");
                assert!(
                    reason.contains("live Nova+Heat cloud"),
                    "names the missing dep: {reason}"
                );
                assert!(reason.contains("vdi-s1"), "names the stack: {reason}");
                assert!(
                    reason.contains("action/cloud/heat-create"),
                    "names the verb topic: {reason}"
                );
            }
            PlacementError::Failed { verb, reason } => {
                panic!("expected integration-gated, got Failed {{{verb}: {reason}}}")
            }
        }
    }

    #[test]
    fn place_desktop_drives_the_injected_seam() {
        let fake = FakePlacement::new("uuid-9", "peer:host");
        let seen = fake.seen.clone();
        let w = SessionBrokerWorker::new(std::env::temp_dir(), "peer:a".to_string())
            .with_placement(Box::new(fake));
        let placed = w
            .place_desktop(&spec("s1"))
            .expect("fake places the desktop");
        assert_eq!(placed.vm_id, "uuid-9");
        assert_eq!(placed.serving_host, "peer:host");
        // The seam saw the built request (flavor + heat-create verb).
        let reqs = seen.lock().expect("seen mutex");
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].verb, "heat-create");
        assert_eq!(reqs[0].flavor, "m1.medium");
    }

    #[test]
    fn default_placement_is_gated_so_place_desktop_defers() {
        // The production default (LiveNovaPlacement) gates → place_desktop returns
        // IntegrationGated (no fake boot) with no seam injected.
        let w = SessionBrokerWorker::new(std::env::temp_dir(), "peer:a".to_string());
        assert!(matches!(
            w.place_desktop(&spec("s1")),
            Err(PlacementError::IntegrationGated { .. })
        ));
    }

    /// Seed a temp bus with `action/vdi/place` desktop specs and return its root.
    fn seed_place_bus(specs: &[DesktopSpec]) -> PathBuf {
        use mde_bus::hooks::config::Priority;
        let dir = std::env::temp_dir().join(format!("mde-sb-place-{}-{}", now_ms(), specs.len()));
        let persist = Persist::open(dir.clone()).expect("open bus");
        for s in specs {
            persist
                .write(
                    PLACE_TOPIC,
                    Priority::Default,
                    None,
                    Some(&serde_json::to_string(s).unwrap()),
                )
                .expect("write place request");
        }
        dir
    }

    #[test]
    fn drain_placements_boots_via_the_seam_and_emits_the_placed_session() {
        // The leader drains a place request, places it through the fake seam, and
        // emits a `SessionRequest::Open` on ACTION_TOPIC — the existing wire path the
        // roster folds + the viewer renders (display path stays the existing viewer).
        let bus = seed_place_bus(&[spec("s1")]);
        let wg = std::env::temp_dir().join(format!("mde-sb-place-wg-{}", now_ms()));
        std::fs::create_dir_all(&wg).expect("mk workgroup");
        let w = SessionBrokerWorker::new(wg.clone(), "peer:a".to_string())
            .with_placement(Box::new(FakePlacement::new("uuid-boot", "peer:compute-3")))
            .with_bus_root(bus.clone());

        let mut place_cursor = None;
        w.drain_placements(&bus, &mut place_cursor); // leader (fresh temp wg)

        // The placed session landed on ACTION_TOPIC and folds into a live roster.
        let mut cursor = None;
        let mut roster = BTreeMap::new();
        drain(&bus, &mut cursor, &mut roster);
        assert_eq!(roster.len(), 1, "the placed desktop became a session");
        let s = &roster["s1"];
        assert_eq!(s.state, SessionState::Requested);
        assert_eq!(
            s.vm_id, "uuid-boot",
            "session points at the placed instance"
        );
        assert_eq!(
            s.serving_peer, "peer:compute-3",
            "serving peer = Nova's host"
        );
        assert_eq!(s.client_peer, "peer:client", "the seat is preserved");

        let _ = std::fs::remove_dir_all(&bus);
        let _ = std::fs::remove_dir_all(&wg);
    }

    #[test]
    fn drain_placements_with_gated_seam_emits_no_session() {
        // The default gated placement (no live cloud) emits nothing — an honest defer.
        let bus = seed_place_bus(&[spec("s1")]);
        let wg = std::env::temp_dir().join(format!("mde-sb-gated-wg-{}", now_ms()));
        std::fs::create_dir_all(&wg).expect("mk workgroup");
        let w =
            SessionBrokerWorker::new(wg.clone(), "peer:a".to_string()).with_bus_root(bus.clone());

        let mut place_cursor = None;
        w.drain_placements(&bus, &mut place_cursor); // gated LiveNovaPlacement

        let mut cursor = None;
        let mut roster = BTreeMap::new();
        drain(&bus, &mut cursor, &mut roster);
        assert!(roster.is_empty(), "a gated placement emits no fake session");

        let _ = std::fs::remove_dir_all(&bus);
        let _ = std::fs::remove_dir_all(&wg);
    }
}
