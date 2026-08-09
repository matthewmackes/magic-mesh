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
//!   [`MeshSessionStore`] is the Syncthing-replicated cross-peer session
//!   directory; a `FakeStore` drives the tests). The live store writes one JSON
//!   record per session under the workgroup root, so the session-broker publishes
//!   real shared state today while the eventual etcd lease plane can supersede the
//!   same trait without changing the broker fold.
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
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mackes_mesh_types::cloud::{
    cloud_request_digest, CloudArmSigner, CloudArmedToken, CLOUD_ACTION_SCHEMA_VERSION,
};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;

use etcd_client::{GetOptions, PutOptions};

use super::scheduler::NodeId;
use super::{ShutdownToken, Worker};
use crate::ipc::action_auth::{ActionAuthorizer, MutationContext};
use crate::substrate::etcd::{connect, session_key, SESSIONS_PREFIX, SESSION_LEASE_TTL_S};
use crate::substrate::peers::block_on;

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

/// A persisted VDI session is a compact replicated control record: it contains
/// only peer, VM, and session identifiers, one lifecycle enum, and timestamps.
/// It has no payload/blob fields, so 256 KiB leaves ample room for legacy
/// identifiers while bounding hostile materialization before JSON parsing.
const MAX_SESSION_RECORD_BYTES: usize = 256 * 1024;

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
    /// Typed desktop intent retained with the roaming session. A Browser VM
    /// profile also fixes the selected transport: `BrowserVm` is Sunshine and
    /// `BrowserVmRdp` is the explicit one-session alternate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<mackes_mesh_types::vdi_session::DesktopSessionProfile>,
    /// The current lifecycle state.
    pub state: SessionState,
    /// When the session was first opened (ms since the Unix epoch, passed in).
    pub opened_at_ms: u64,
    /// When the state last changed (ms since the Unix epoch, passed in).
    pub updated_at_ms: u64,
    /// Guest-owned application identity, present only for App VM sessions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<mackes_mesh_types::vdi_session::AppVmLaunchRequest>,
    /// Guest/application readiness, present only for App VM sessions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_state: Option<mackes_mesh_types::vdi_session::AppVmLifecycleState>,
    /// Optional bounded context attached to a non-ready App VM state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_state_reason: Option<String>,
    /// Highest guest runtime generation accepted for this App VM session.
    ///
    /// A zero value preserves compatibility with pre-generation snapshots;
    /// the first explicitly numbered runtime report must be greater than it.
    #[serde(default)]
    pub app_state_generation: u64,
}

/// A session lifecycle request drained off [`ACTION_TOPIC`] — the wire verb the
/// shell / connect flow publishes. Internally tagged on `op`.
///
/// arch-2 (2026-07-11) — the type itself now lives in
/// [`mackes_mesh_types::vdi_session`] so the shell's `discovery` / `session_rail`
/// mirrors reuse it instead of maintaining byte-compatible copies; it's re-exported
/// here so existing `session_broker::SessionRequest` paths
/// `onboard::first_desktop`) keep resolving unchanged. The wire shape is
/// byte-identical: the broker's [`SessionId`] / [`NodeId`] / [`VmId`] are all
/// `= String` aliases, so the shared `String`-typed fields serialise the same.
pub use mackes_mesh_types::vdi_session::{
    AppVmLaunchRequest, AppVmLifecycleState, AppVmRuntimeEvidence, BrowserVmProfileError,
    BrowserVmTransport, DesktopSessionProfile, SessionRequest, APP_VM_RUNTIME_TOPIC,
    BROWSER_VM_WORKLOAD_ID,
};

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
    /// An App VM open carried invalid untrusted catalog/session data.
    InvalidAppVm(mackes_mesh_types::vdi_session::AppVmLaunchRequestError),
    /// An App VM readiness update attempted an invalid lifecycle jump.
    IllegalAppState {
        /// Current readiness state.
        from: AppVmLifecycleState,
        /// Requested readiness state.
        to: AppVmLifecycleState,
    },
    /// A guest readiness report was older than the last accepted generation.
    StaleAppState {
        /// The rejected guest generation.
        generation: u64,
    },
    /// A repeated App VM open tried to retarget an already admitted session.
    /// Session identity is the idempotency key; changing its guest or serving
    /// route would create split-brain ownership rather than a resume.
    ConflictingAppSession {
        /// The stable session identity being protected.
        id: SessionId,
        /// The bounded reason for refusing the replay.
        reason: &'static str,
    },
    /// A Browser VM workload/profile pairing failed validation.
    InvalidDesktopProfile {
        /// The VM identity carried by the rejected request.
        vm_id: VmId,
        /// The exact fail-closed validation reason.
        reason: BrowserVmProfileError,
    },
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IllegalTransition { from, to } => {
                write!(f, "illegal session transition {from:?} → {to:?}")
            }
            Self::UnknownSession(id) => write!(f, "unknown session {id}"),
            Self::InvalidAppVm(error) => write!(f, "invalid App VM session: {error}"),
            Self::IllegalAppState { from, to } => {
                write!(f, "illegal App VM readiness transition {from:?} → {to:?}")
            }
            Self::StaleAppState { generation } => {
                write!(f, "stale App VM readiness generation {generation}")
            }
            Self::ConflictingAppSession { id, reason } => {
                write!(f, "conflicting App VM session `{id}`: {reason}")
            }
            Self::InvalidDesktopProfile { vm_id, reason } => write!(
                f,
                "invalid Browser VM session request for `{vm_id}`: {reason}"
            ),
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
    open_session_with_profile(id, serving_peer, vm_id, client_peer, None, now_ms)
}

/// Build a fresh session while retaining its typed desktop profile.
const fn open_session_with_profile(
    id: SessionId,
    serving_peer: NodeId,
    vm_id: VmId,
    client_peer: NodeId,
    profile: Option<DesktopSessionProfile>,
    now_ms: u64,
) -> VdiSession {
    VdiSession {
        id,
        serving_peer,
        vm_id,
        client_peer,
        profile,
        state: SessionState::Requested,
        opened_at_ms: now_ms,
        updated_at_ms: now_ms,
        app: None,
        app_state: None,
        app_state_reason: None,
        app_state_generation: 0,
    }
}

/// Build a fresh validated App VM session in [`SessionState::Requested`].
pub fn open_app_session(
    id: SessionId,
    serving_peer: NodeId,
    vm_id: VmId,
    client_peer: NodeId,
    app_id: String,
    catalog_revision: String,
    guest_profile: String,
    requested_capabilities: Vec<String>,
    resume: bool,
    now_ms: u64,
) -> Result<VdiSession, mackes_mesh_types::vdi_session::AppVmLaunchRequestError> {
    let app = AppVmLaunchRequest::new(
        app_id,
        catalog_revision,
        guest_profile,
        requested_capabilities,
        id.clone(),
        resume,
    )?;
    app.validate_admitted()?;
    Ok(VdiSession {
        id,
        serving_peer,
        vm_id,
        client_peer,
        profile: None,
        state: SessionState::Requested,
        opened_at_ms: now_ms,
        updated_at_ms: now_ms,
        app: Some(app),
        app_state: Some(AppVmLifecycleState::WaitingForPlacement),
        app_state_reason: None,
        app_state_generation: 0,
    })
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
/// analogue of `scheduler`'s `fold_capacity`). Repeated `OpenApp` requests for
/// one admitted identity are idempotent and preserve readiness; a replay cannot
/// retarget the session to another VM or serving/client peer.
///
/// # Errors
/// [`SessionError::UnknownSession`] when a transition op names an absent id,
/// [`SessionError::IllegalTransition`] when the transition is forbidden, or
/// [`SessionError::ConflictingAppSession`] when an App VM replay would retarget
/// an admitted session. Plain `Open` retains its legacy replace behavior;
/// `OpenApp` is identity-bound and does not overwrite an existing session.
pub fn apply_request(
    roster: &mut BTreeMap<SessionId, VdiSession>,
    req: SessionRequest,
    now_ms: u64,
) -> Result<(), SessionError> {
    let browser_transport = req.browser_transport();
    match req {
        SessionRequest::Open {
            id,
            serving_peer,
            vm_id,
            client_peer,
            profile,
        } => {
            browser_transport.map_err(|reason| SessionError::InvalidDesktopProfile {
                vm_id: vm_id.clone(),
                reason,
            })?;
            let session = open_session_with_profile(
                id.clone(),
                serving_peer,
                vm_id,
                client_peer,
                profile,
                now_ms,
            );
            roster.insert(id, session);
            Ok(())
        }
        SessionRequest::OpenApp {
            id,
            serving_peer,
            vm_id,
            client_peer,
            app_id,
            catalog_revision,
            guest_profile,
            requested_capabilities,
            resume,
        } => {
            let session = open_app_session(
                id.clone(),
                serving_peer,
                vm_id,
                client_peer,
                app_id,
                catalog_revision,
                guest_profile,
                requested_capabilities,
                resume,
                now_ms,
            )
            .map_err(SessionError::InvalidAppVm)?;
            let Some(existing) = roster.get(&id).cloned() else {
                roster.insert(id, session);
                return Ok(());
            };
            let Some(existing_app) = existing.app.as_ref() else {
                return Err(SessionError::ConflictingAppSession {
                    id,
                    reason: "session id is already owned by a non-App VM session",
                });
            };
            let requested_app = session
                .app
                .as_ref()
                .expect("open_app_session always carries an App VM declaration");
            if existing.state.is_terminal() {
                return Err(SessionError::ConflictingAppSession {
                    id,
                    reason: "the existing App VM session is closed; use a new session id",
                });
            }
            if existing.vm_id != session.vm_id
                || existing.serving_peer != session.serving_peer
                || existing.client_peer != session.client_peer
            {
                return Err(SessionError::ConflictingAppSession {
                    id,
                    reason: "VM or serving/client peer changed",
                });
            }
            if existing_app.session_id != requested_app.session_id
                || existing_app.app_id != requested_app.app_id
                || existing_app.guest_profile != requested_app.guest_profile
            {
                return Err(SessionError::ConflictingAppSession {
                    id,
                    reason: "app identity, guest profile, or session identity changed",
                });
            }
            // A retry may refresh catalog/capability/resume intent, but it must
            // retain the already observed VDI and guest lifecycle states. This
            // is the key property that makes reconnect/retry converge on one
            // guest instead of briefly advertising a false cold launch.
            if existing_app != requested_app {
                let mut refreshed = existing;
                refreshed.app = session.app;
                refreshed.updated_at_ms = now_ms;
                roster.insert(id, refreshed);
            }
            Ok(())
        }
        SessionRequest::AppState {
            id,
            state,
            reason,
            generation,
        } => {
            transition(roster, &id, |session| {
                // The VDI session identity is its lifetime boundary. Once that
                // identity is closed, a delayed guest observation or replayed
                // signed AppState action belongs to the retired incarnation and
                // must not mutate its readiness, reason, or update timestamp.
                if session.state.is_terminal() {
                    return Err(SessionError::IllegalTransition {
                        from: session.state,
                        to: session.state,
                    });
                }
                if session.app.is_none() {
                    return Err(SessionError::IllegalTransition {
                        from: session.state,
                        to: session.state,
                    });
                }
                let generation_is_stale = if generation == 0 {
                    session.app_state_generation != 0
                } else {
                    generation <= session.app_state_generation
                };
                if generation_is_stale {
                    return Err(SessionError::StaleAppState { generation });
                }
                let current = session
                    .app_state
                    .unwrap_or(AppVmLifecycleState::WaitingForPlacement);
                if !current.can_transition_to(state) {
                    return Err(SessionError::IllegalAppState {
                        from: current,
                        to: state,
                    });
                }
                let mut next = session.clone();
                next.app_state = Some(state);
                next.app_state_reason = reason.map(|value| value.chars().take(255).collect());
                if generation != 0 {
                    next.app_state_generation = generation;
                }
                next.updated_at_ms = now_ms;
                Ok(next)
            })
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

// ─────────────────────────── store seam ───────────────────────────

/// A typed failure from the [`SessionStore`] seam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStoreError {
    /// A store implementation needs a real prerequisite that is not present in
    /// this build/environment. The production file-backed store normally returns
    /// [`Self::Failed`] for concrete I/O problems; this variant remains available
    /// for alternate live stores such as a future etcd lease plane.
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

/// Read one Syncthing-replicated session record through the descriptor that will
/// actually be consumed. Reject final symlinks, blocking special files,
/// oversized input, growth/shrinkage while reading, and invalid UTF-8 before
/// `serde_json` materializes a [`VdiSession`].
fn read_bounded_session_record(path: &Path) -> std::io::Result<String> {
    #[cfg(unix)]
    let file: std::fs::File = {
        use rustix::fs::{Mode, OFlags};

        rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )?
        .into()
    };
    #[cfg(not(unix))]
    let file = {
        let metadata = std::fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "session record must be a regular non-symlink file",
            ));
        }
        std::fs::File::open(path)?
    };

    let expected_len = file.metadata()?.len();
    read_bounded_session_file(file, path, expected_len)
}

/// Consume an already-open session record and verify that the inode remains the
/// same-sized regular file that was observed before reading. The explicit
/// `expected_len` seam also lets hostile-input tests cover a growth race without
/// making the production path depend on timing.
fn read_bounded_session_file(
    mut file: std::fs::File,
    path: &Path,
    expected_len: u64,
) -> std::io::Result<String> {
    use std::io::Read as _;

    let metadata = file.metadata()?;
    if !metadata.file_type().is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("session record {} is not a regular file", path.display()),
        ));
    }
    if metadata.len() != expected_len {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("session record {} changed before reading", path.display()),
        ));
    }
    if metadata.len() > MAX_SESSION_RECORD_BYTES as u64 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "session record {} exceeds {MAX_SESSION_RECORD_BYTES}-byte limit",
                path.display()
            ),
        ));
    }

    let capacity = usize::try_from(metadata.len())
        .unwrap_or(MAX_SESSION_RECORD_BYTES)
        .min(MAX_SESSION_RECORD_BYTES)
        .saturating_add(1);
    let mut bytes = Vec::with_capacity(capacity);
    (&mut file)
        .take((MAX_SESSION_RECORD_BYTES as u64).saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_SESSION_RECORD_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "session record {} exceeds {MAX_SESSION_RECORD_BYTES}-byte limit",
                path.display()
            ),
        ));
    }

    let final_len = file.metadata()?.len();
    if final_len != expected_len || bytes.len() as u64 != final_len {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("session record {} changed while reading", path.display()),
        ));
    }

    String::from_utf8(bytes)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

/// Production [`SessionStore`]: the roaming-session plane.
///
/// The authoritative cross-peer session directory under the mesh workgroup root.
/// Each session is a single JSON record in a Syncthing-replicated directory:
/// leader convergence writes atomically with `rename`, peers list deterministically,
/// and closed/stale sessions are removed idempotently. This is the live fallback
/// half of design lock 5; an etcd lease-backed store can replace this trait
/// implementation later without changing the broker's fold.
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

    fn dir(&self) -> PathBuf {
        self.workgroup_root
            .join("sessions")
            .join("vdi")
            .join("records")
    }

    fn path_for(&self, id: &str) -> PathBuf {
        self.dir()
            .join(format!("{}.json", safe_session_file_stem(id)))
    }
}

impl SessionStore for MeshSessionStore {
    fn publish(&self, session: &VdiSession) -> Result<(), SessionStoreError> {
        let dir = self.dir();
        std::fs::create_dir_all(&dir).map_err(|e| SessionStoreError::Failed {
            op: "publish",
            reason: format!("create {}: {e}", dir.display()),
        })?;
        let final_path = self.path_for(&session.id);
        let tmp = dir.join(format!(
            ".{}.{}.tmp",
            safe_session_file_stem(&session.id),
            std::process::id()
        ));
        let body = serde_json::to_vec_pretty(session).map_err(|e| SessionStoreError::Failed {
            op: "publish",
            reason: format!("serialize session {}: {e}", session.id),
        })?;
        std::fs::write(&tmp, body).map_err(|e| SessionStoreError::Failed {
            op: "publish",
            reason: format!("write {}: {e}", tmp.display()),
        })?;
        std::fs::rename(&tmp, &final_path).map_err(|e| SessionStoreError::Failed {
            op: "publish",
            reason: format!("rename {} → {}: {e}", tmp.display(), final_path.display()),
        })?;
        Ok(())
    }

    fn list(&self) -> Result<Vec<VdiSession>, SessionStoreError> {
        let dir = self.dir();
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => {
                return Err(SessionStoreError::Failed {
                    op: "list",
                    reason: format!("read {}: {e}", dir.display()),
                });
            }
        };
        let mut rows = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| SessionStoreError::Failed {
                op: "list",
                reason: format!("read {} entry: {e}", dir.display()),
            })?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let raw =
                read_bounded_session_record(&path).map_err(|e| SessionStoreError::Failed {
                    op: "list",
                    reason: format!("read {}: {e}", path.display()),
                })?;
            let row: VdiSession =
                serde_json::from_str(&raw).map_err(|e| SessionStoreError::Failed {
                    op: "list",
                    reason: format!("parse {}: {e}", path.display()),
                })?;
            rows.push(row);
        }
        rows.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(rows)
    }

    fn remove(&self, id: &str) -> Result<(), SessionStoreError> {
        let path = self.path_for(id);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(SessionStoreError::Failed {
                op: "remove",
                reason: format!("remove {}: {e}", path.display()),
            }),
        }
    }
}

fn safe_session_file_stem(id: &str) -> String {
    let mut out = String::new();
    for byte in id.bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'.' | b'-' | b'_' | b':' => {
                out.push(char::from(byte));
            }
            _ => {
                use std::fmt::Write as _;
                let _ = write!(&mut out, "_{byte:02x}");
            }
        }
    }
    if out.is_empty() {
        "_".to_string()
    } else {
        out
    }
}

// ──────────────────── etcd lease-backed store (E12-5/8) ────────────────────

/// The etcd operations the [`EtcdSessionStore`] needs, factored behind a trait so
/// the store's **lease logic** — grant-on-first-publish, keep-alive-on-refresh,
/// revoke-on-remove, and expiry-frees-the-row — is unit-tested against an
/// in-memory fake with a controllable clock, exactly as the store seam itself is
/// faked for the broker. Production wires [`LiveSessionLeaseOps`] over the
/// SUBSTRATE-V2 etcd client ([`crate::substrate::etcd`]).
///
/// Byte-oriented (the honest etcd boundary): the store owns the [`VdiSession`]
/// serde; the ops move keys, JSON values, and lease ids.
trait SessionLeaseOps: Send + Sync {
    /// Grant a lease of `ttl_s` seconds and return its id (etcd auto-deletes every
    /// key bound to the lease once it expires without a keep-alive).
    fn grant_lease(&self, ttl_s: i64) -> Result<i64, SessionStoreError>;

    /// Refresh (keep-alive) `lease_id`, resetting its TTL.
    ///
    /// # Errors
    /// [`SessionStoreError::Failed`] when the lease is already gone (expired) or
    /// etcd is unreachable — the store treats a lost lease as a signal to re-grant.
    fn keep_alive(&self, lease_id: i64) -> Result<(), SessionStoreError>;

    /// Put `value_json` at the session key for `id`, bound to `lease_id`.
    fn put(&self, id: &str, value_json: &str, lease_id: i64) -> Result<(), SessionStoreError>;

    /// Range-read every live session record's raw JSON value under the prefix
    /// (rows whose lease has expired are already gone — that IS the auto-free).
    fn list(&self) -> Result<Vec<String>, SessionStoreError>;

    /// Remove the session `id`: revoke its `lease_id` (if known — which auto-deletes
    /// the row) and delete the key (idempotent, covers a row written under a lease
    /// this process no longer tracks).
    fn revoke_and_delete(&self, id: &str, lease_id: Option<i64>) -> Result<(), SessionStoreError>;
}

/// The lease-backed [`SessionStore`]: the roaming-session plane on **etcd**.
///
/// The lease half of design lock 5 (the [`MeshSessionStore`] file plane is the
/// fallback). Each published session is bound to an etcd lease; the store keeps
/// its leases alive on every convergence tick (through [`SessionStore::list`],
/// the once-per-tick call the leader makes), so a live session's row is
/// continuously refreshed. When the converging node **crashes**, the keep-alive
/// stops, the leases lapse, and etcd auto-deletes the rows — the crashed-seat
/// session frees itself, with no file-scan the [`MeshSessionStore`] needed and no
/// lingering `Active` row.
///
/// Lease ids live in a small in-memory registry keyed by session id. It is
/// process-local: after a restart the store no longer knows the old lease ids, so
/// those rows lapse on their own and the pure [`reconcile`] re-publishes the still
/// desired ones under fresh leases (a brief, self-healing flap — the same
/// stateless-across-restarts shape the etcd leader election uses).
pub struct EtcdSessionStore {
    /// The injected etcd-lease seam (production: [`LiveSessionLeaseOps`]).
    ops: Box<dyn SessionLeaseOps>,
    /// The TTL granted for a session's lease.
    ttl_s: i64,
    /// session id → the lease its row is bound to (this process's leases only).
    leases: Mutex<BTreeMap<SessionId, i64>>,
}

impl EtcdSessionStore {
    /// Construct the production store over the etcd `endpoints`
    /// (`/etc/mackesd/etcd-endpoints`), using the [`SESSION_LEASE_TTL_S`] TTL.
    #[must_use]
    pub fn new(endpoints: Vec<String>) -> Self {
        Self::with_ops(
            Box::new(LiveSessionLeaseOps::new(endpoints)),
            SESSION_LEASE_TTL_S,
        )
    }

    /// Construct over an injected lease seam + TTL (tests).
    fn with_ops(ops: Box<dyn SessionLeaseOps>, ttl_s: i64) -> Self {
        Self {
            ops,
            ttl_s,
            leases: Mutex::new(BTreeMap::new()),
        }
    }
}

impl SessionStore for EtcdSessionStore {
    fn publish(&self, session: &VdiSession) -> Result<(), SessionStoreError> {
        let value = serde_json::to_string(session).map_err(|e| SessionStoreError::Failed {
            op: "publish",
            reason: format!("serialize session {}: {e}", session.id),
        })?;
        let mut leases = self.leases.lock().expect("session lease registry mutex");
        match leases.get(&session.id).copied() {
            // A session we already track: keep its lease alive and re-put the
            // (possibly updated) value under the SAME lease so the row stays put and
            // the TTL resets. If the lease has already lapsed (keep-alive fails), fall
            // through to a fresh grant so the row is re-bound to a live lease.
            Some(lease_id) if self.ops.keep_alive(lease_id).is_ok() => {
                self.ops.put(&session.id, &value, lease_id)?;
            }
            _ => {
                let lease_id = self.ops.grant_lease(self.ttl_s)?;
                self.ops.put(&session.id, &value, lease_id)?;
                leases.insert(session.id.clone(), lease_id);
            }
        }
        Ok(())
    }

    fn list(&self) -> Result<Vec<VdiSession>, SessionStoreError> {
        // Heartbeat: refresh every lease this node owns on each convergence tick, so
        // a live session never lapses while the leader keeps converging. Best-effort
        // — a keep-alive that fails means the lease already expired and the row is
        // gone; `reconcile` will re-publish it under a new lease this same tick.
        {
            let leases = self.leases.lock().expect("session lease registry mutex");
            for lease_id in leases.values().copied() {
                let _ = self.ops.keep_alive(lease_id);
            }
        }
        let mut rows = Vec::new();
        for raw in self.ops.list()? {
            match serde_json::from_str::<VdiSession>(&raw) {
                Ok(s) => rows.push(s),
                Err(e) => {
                    tracing::warn!(error = %e, "etcd_session_store: skipping unparseable record");
                }
            }
        }
        rows.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(rows)
    }

    fn remove(&self, id: &str) -> Result<(), SessionStoreError> {
        let lease_id = self
            .leases
            .lock()
            .expect("session lease registry mutex")
            .remove(id);
        self.ops.revoke_and_delete(id, lease_id)
    }
}

/// Production [`SessionLeaseOps`]: the SUBSTRATE-V2 etcd v3 client. Connects
/// per-op over the runtime-aware blocking bridge ([`block_on`]) exactly as
/// [`crate::substrate::peers`] does (etcd lease ids are cluster-global, so a
/// grant on one connection is kept-alive / put-under / revoked from the next).
struct LiveSessionLeaseOps {
    endpoints: Vec<String>,
}

impl LiveSessionLeaseOps {
    const fn new(endpoints: Vec<String>) -> Self {
        Self { endpoints }
    }
}

/// The "etcd runtime could not be built" error for a bridged op (mirrors the
/// `etcd runtime unavailable` string the substrate blocking façades return).
fn runtime_unavailable(op: &'static str) -> SessionStoreError {
    SessionStoreError::Failed {
        op,
        reason: "etcd runtime unavailable".to_string(),
    }
}

impl SessionLeaseOps for LiveSessionLeaseOps {
    fn grant_lease(&self, ttl_s: i64) -> Result<i64, SessionStoreError> {
        block_on(async {
            let mut c = connect(&self.endpoints)
                .await
                .map_err(|e| SessionStoreError::Failed {
                    op: "grant_lease",
                    reason: format!("connect: {e}"),
                })?;
            c.lease_grant(ttl_s, None)
                .await
                .map(|r| r.id())
                .map_err(|e| SessionStoreError::Failed {
                    op: "grant_lease",
                    reason: e.to_string(),
                })
        })
        .ok_or_else(|| runtime_unavailable("grant_lease"))?
    }

    fn keep_alive(&self, lease_id: i64) -> Result<(), SessionStoreError> {
        block_on(async {
            let mut c = connect(&self.endpoints)
                .await
                .map_err(|e| SessionStoreError::Failed {
                    op: "keep_alive",
                    reason: format!("connect: {e}"),
                })?;
            let (mut keeper, mut stream) =
                c.lease_keep_alive(lease_id)
                    .await
                    .map_err(|e| SessionStoreError::Failed {
                        op: "keep_alive",
                        reason: e.to_string(),
                    })?;
            keeper
                .keep_alive()
                .await
                .map_err(|e| SessionStoreError::Failed {
                    op: "keep_alive",
                    reason: e.to_string(),
                })?;
            // The ack carries the refreshed TTL; a zero/absent TTL means etcd has
            // already reaped the lease, so surface it as a failure → the store
            // re-grants rather than putting under a dead lease.
            match stream.message().await {
                Ok(Some(resp)) if resp.ttl() > 0 => Ok(()),
                Ok(_) => Err(SessionStoreError::Failed {
                    op: "keep_alive",
                    reason: format!("lease {lease_id} already expired"),
                }),
                Err(e) => Err(SessionStoreError::Failed {
                    op: "keep_alive",
                    reason: e.to_string(),
                }),
            }
        })
        .ok_or_else(|| runtime_unavailable("keep_alive"))?
    }

    fn put(&self, id: &str, value_json: &str, lease_id: i64) -> Result<(), SessionStoreError> {
        let key = session_key(id);
        let value = value_json.to_string();
        block_on(async {
            let mut c = connect(&self.endpoints)
                .await
                .map_err(|e| SessionStoreError::Failed {
                    op: "put",
                    reason: format!("connect: {e}"),
                })?;
            c.put(key, value, Some(PutOptions::new().with_lease(lease_id)))
                .await
                .map(|_| ())
                .map_err(|e| SessionStoreError::Failed {
                    op: "put",
                    reason: e.to_string(),
                })
        })
        .ok_or_else(|| runtime_unavailable("put"))?
    }

    fn list(&self) -> Result<Vec<String>, SessionStoreError> {
        block_on(async {
            let mut c = connect(&self.endpoints)
                .await
                .map_err(|e| SessionStoreError::Failed {
                    op: "list",
                    reason: format!("connect: {e}"),
                })?;
            let resp = c
                .get(SESSIONS_PREFIX, Some(GetOptions::new().with_prefix()))
                .await
                .map_err(|e| SessionStoreError::Failed {
                    op: "list",
                    reason: e.to_string(),
                })?;
            Ok(resp
                .kvs()
                .iter()
                .filter_map(|kv| kv.value_str().ok().map(String::from))
                .collect())
        })
        .ok_or_else(|| runtime_unavailable("list"))?
    }

    fn revoke_and_delete(&self, id: &str, lease_id: Option<i64>) -> Result<(), SessionStoreError> {
        let key = session_key(id);
        block_on(async {
            let mut c = connect(&self.endpoints)
                .await
                .map_err(|e| SessionStoreError::Failed {
                    op: "remove",
                    reason: format!("connect: {e}"),
                })?;
            // Revoke first (auto-deletes the bound row); an already-expired lease
            // revoke errors harmlessly, so it's best-effort.
            if let Some(l) = lease_id {
                let _ = c.lease_revoke(l).await;
            }
            c.delete(key, None)
                .await
                .map(|_| ())
                .map_err(|e| SessionStoreError::Failed {
                    op: "remove",
                    reason: e.to_string(),
                })
        })
        .ok_or_else(|| runtime_unavailable("remove"))?
    }
}

// ─────────────────────────── bus + worker ───────────────────────────

/// Read new [`ACTION_TOPIC`] messages since `cursor`, advancing it. A short sync
/// open-read-drop (never crosses an `.await`), mirroring `scheduler`. Every
/// request is authenticated against the exact wire body before it is returned
/// to the in-memory fold; malformed or unsigned bodies never reach
/// [`apply_request`].
fn read_new_actions(
    bus_root: &Path,
    cursor: &mut Option<String>,
    authorizer: &ActionAuthorizer,
) -> Result<Vec<SessionRequest>, String> {
    let persist = Persist::open(bus_root.to_path_buf())
        .map_err(|error| format!("open session Bus: {error}"))?;
    let msgs = persist
        .list_since(ACTION_TOPIC, cursor.as_deref())
        .map_err(|error| format!("read session actions: {error}"))?;
    let mut out = Vec::new();
    for msg in msgs {
        *cursor = Some(msg.ulid.clone());
        let body = msg.body.as_deref().unwrap_or("");
        if !crate::ipc::body_within_cap(Some(body)) {
            tracing::warn!(
                ulid = %msg.ulid,
                cap = crate::ipc::MAX_RPC_BODY_BYTES,
                "session_broker: oversized session request refused"
            );
            continue;
        }
        match parse_request(body) {
            Ok(r) => {
                if let Err(e) = authorize_session_request(authorizer, body, &r) {
                    tracing::warn!(
                        ulid = %msg.ulid,
                        error = %e,
                        "session_broker: unauthorized session request refused"
                    );
                    continue;
                }
                out.push(r);
            }
            Err(e) => {
                tracing::warn!(ulid = %msg.ulid, error = %e, "session_broker: bad session request");
            }
        }
    }
    Ok(out)
}

/// Return the closed capability verb and stable session target for one typed
/// request. The raw body is separately HMAC-bound, so this target prevents a
/// valid capability for one session/op from being retargeted semantically.
fn session_auth_target(request: &SessionRequest) -> (&'static str, String) {
    match request {
        SessionRequest::Open { id, .. } | SessionRequest::OpenApp { id, .. } => {
            ("vdi-session-open", format!("session:{id}"))
        }
        SessionRequest::Active { id } => ("vdi-session-active", format!("session:{id}")),
        SessionRequest::AppState { id, .. } => ("vdi-session-app-state", format!("session:{id}")),
        SessionRequest::Disconnect { id } => ("vdi-session-disconnect", format!("session:{id}")),
        SessionRequest::Close { id } => ("vdi-session-close", format!("session:{id}")),
    }
}

/// Verify a session action's exact body before any roster or shared-store
/// mutation. Parsing the typed request is pure and happens only to derive the
/// closed semantic target; the original body (including schema/token) is what
/// the verifier authenticates.
fn authorize_session_request(
    authorizer: &ActionAuthorizer,
    body: &str,
    request: &SessionRequest,
) -> Result<(), String> {
    let (verb, target) = session_auth_target(request);
    authorizer.authorize(
        body,
        MutationContext {
            verb,
            node: "vdi-session",
            target: &target,
        },
    )
}

/// Fold new `action/vdi/session` messages (advancing `cursor`) into `roster`.
/// Runs on every node (the log is mesh-replicated), so any node has a warm roster
/// ready to converge if it wins the election. A malformed op is dropped honestly.
fn drain(
    bus_root: &Path,
    cursor: &mut Option<String>,
    roster: &mut BTreeMap<SessionId, VdiSession>,
    authorizer: &ActionAuthorizer,
) -> Result<Vec<SessionRequest>, String> {
    let mut requests = Vec::new();
    for req in read_new_actions(bus_root, cursor, authorizer)? {
        if let Err(e) = apply_request(roster, req.clone(), now_ms()) {
            tracing::warn!(error = %e, "session_broker: dropping unresolvable session op");
        } else {
            requests.push(req);
        }
    }
    Ok(requests)
}

/// Read guest runtime observations from the replicated state topic. Malformed,
/// oversized, or invalid records advance the cursor but never reach the session
/// roster. Identity matching happens separately against the admitted session.
fn read_runtime_evidence(
    bus_root: &Path,
    cursor: &mut Option<String>,
) -> Vec<AppVmRuntimeEvidence> {
    let Ok(persist) = Persist::open(bus_root.to_path_buf()) else {
        return Vec::new();
    };
    let Ok(messages) = persist.list_since(APP_VM_RUNTIME_TOPIC, cursor.as_deref()) else {
        return Vec::new();
    };
    let mut evidence = Vec::new();
    for message in messages {
        *cursor = Some(message.ulid);
        let Some(body) = message.body.as_deref() else {
            continue;
        };
        if !crate::ipc::body_within_cap(Some(body)) {
            tracing::warn!("session_broker: oversized App VM runtime evidence refused");
            continue;
        }
        let Ok(record) = serde_json::from_str::<AppVmRuntimeEvidence>(body) else {
            tracing::warn!("session_broker: malformed App VM runtime evidence refused");
            continue;
        };
        if let Err(error) = record.validate() {
            tracing::warn!(%error, "session_broker: invalid App VM runtime evidence refused");
            continue;
        }
        evidence.push(record);
    }
    evidence
}

/// Apply one validated guest observation only when it identifies the exact
/// admitted session hosted by this serving node. The signed action publication
/// keeps the shell's public lifecycle rail and the daemon roster on one wire.
fn apply_runtime_evidence(
    roster: &mut BTreeMap<SessionId, VdiSession>,
    evidence: AppVmRuntimeEvidence,
    node_id: &str,
    bus_root: &Path,
    signer: Option<&CloudArmSigner>,
) {
    let Some(session) = roster.get(&evidence.session_id) else {
        return;
    };
    let Some(app) = session.app.as_ref() else {
        return;
    };
    if session.serving_peer != node_id
        || app.session_id != evidence.session_id
        || app.app_id != evidence.app_id
        || session.vm_id != evidence.vm_id
    {
        tracing::warn!(
            session = %evidence.session_id,
            "session_broker: App VM runtime evidence identity mismatch refused"
        );
        return;
    }
    let state = evidence.state.lifecycle_state();
    let request = SessionRequest::AppState {
        id: evidence.session_id.clone(),
        generation: evidence.generation,
        state,
        reason: evidence.reason.clone(),
    };
    let Some(signer) = signer else {
        tracing::debug!(session = %evidence.session_id, "session_broker: runtime evidence waiting for action signer");
        return;
    };
    let mut candidate = roster.clone();
    if let Err(error) = apply_request(&mut candidate, request, now_ms()) {
        tracing::warn!(session = %evidence.session_id, %error, "session_broker: runtime readiness transition refused");
        return;
    }
    if let Err(error) = publish_app_state(
        bus_root,
        signer,
        &evidence.session_id,
        evidence.generation,
        state,
        evidence.reason.as_deref(),
    ) {
        tracing::debug!(session = %evidence.session_id, %error, "session_broker: runtime readiness publication deferred");
        return;
    }
    *roster = candidate;
}

/// Build the daemon-authenticated readiness event that follows an App VM open.
/// The exact body is HMAC-bound before the token is inserted, matching the
/// shell's `action/vdi/session` producer contract.
fn signed_app_state_body(
    signer: &CloudArmSigner,
    id: &str,
    generation: u64,
    state: AppVmLifecycleState,
    reason: Option<&str>,
) -> Result<String, String> {
    let request = SessionRequest::AppState {
        id: id.to_owned(),
        generation,
        state,
        reason: reason.map(str::to_owned),
    };
    let mut document: serde_json::Value = serde_json::from_str(&request.to_body())
        .map_err(|error| format!("serialize app readiness request: {error}"))?;
    document
        .as_object_mut()
        .ok_or_else(|| "app readiness request is not a JSON object".to_string())?
        .insert(
            "schema_version".to_string(),
            serde_json::Value::from(CLOUD_ACTION_SCHEMA_VERSION),
        );
    let unsigned = document.to_string();
    let digest = cloud_request_digest(&unsigned).map_err(str::to_string)?;
    let nonce = uuid::Uuid::new_v4().to_string();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| "system clock is before the Unix epoch".to_string())
        .and_then(|duration| {
            i64::try_from(duration.as_millis())
                .map_err(|_| "system clock is beyond the capability range".to_string())
        })?;
    let token = CloudArmedToken::mint(
        signer,
        &nonce,
        now.saturating_add(30_000),
        "vdi-session-app-state",
        "vdi-session",
        &format!("session:{id}"),
        &digest,
    )
    .encode();
    document
        .as_object_mut()
        .ok_or_else(|| "app readiness request is not a JSON object".to_string())?
        .insert("armed_token".to_string(), serde_json::Value::String(token));
    Ok(document.to_string())
}

/// Publish one daemon-owned app readiness event. A missing Bus or signer is an
/// honest no-op; the caller retries while the session remains at the initial
/// waiting state.
fn publish_app_state(
    bus_root: &Path,
    signer: &CloudArmSigner,
    id: &str,
    generation: u64,
    state: AppVmLifecycleState,
    reason: Option<&str>,
) -> Result<(), String> {
    let body = signed_app_state_body(signer, id, generation, state, reason)?;
    Persist::open(bus_root.to_path_buf())
        .map_err(|error| format!("open session Bus: {error}"))?
        .write(ACTION_TOPIC, Priority::Default, None, Some(&body))
        .map(|_| ())
        .map_err(|error| format!("publish app readiness: {error}"))
}

fn default_bus_root() -> Option<PathBuf> {
    mde_bus::default_data_dir()
}

fn session_bus_root(override_root: Option<PathBuf>, default_root: Option<PathBuf>) -> PathBuf {
    override_root
        .or(default_root)
        .unwrap_or_else(|| PathBuf::from(mde_bus::SYSTEM_BUS_ROOT))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionBusIdentity {
    root: PathBuf,
    device: u64,
    inode: u64,
}

fn session_bus_identity(root: &Path) -> Result<SessionBusIdentity, String> {
    use std::os::unix::fs::MetadataExt as _;

    let index = root.join("index.sqlite");
    let metadata = std::fs::metadata(&index)
        .map_err(|error| format!("session Bus index {} unavailable: {error}", index.display()))?;
    if !metadata.is_file() {
        return Err(format!(
            "session Bus index {} is not a regular file",
            index.display()
        ));
    }
    Ok(SessionBusIdentity {
        root: root.to_path_buf(),
        device: metadata.dev(),
        inode: metadata.ino(),
    })
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
    /// This node's id — its identity in the leader election.
    node_id: NodeId,
    /// The shared leader lock (the same `.mackesd-leader.lock` `dc_auditor` uses).
    leader_lock: PathBuf,
    /// Convergence cadence.
    poll: Duration,
    /// Exact-body capability verifier for the privileged session-action lane.
    /// Missing production credentials install a fail-closed verifier.
    authorizer: Arc<ActionAuthorizer>,
    /// Root-only signer used for daemon-authored readiness transitions.
    app_state_signer: Option<CloudArmSigner>,
    /// Bus root override (tests). `None` ⇒ [`default_bus_root`].
    bus_root_override: Option<PathBuf>,
}

impl SessionBrokerWorker {
    /// Construct with production defaults: the **etcd-first** session store
    /// (lease-backed [`EtcdSessionStore`] when the coordination plane is
    /// provisioned, else the replicated-file [`MeshSessionStore`] fallback), the
    /// shared leader lock under `workgroup_root`, and the default cadence.
    /// `node_id` is this node's mesh identity.
    #[must_use]
    pub fn new(workgroup_root: PathBuf, node_id: NodeId) -> Self {
        let leader_lock = workgroup_root.join(".mackesd-leader.lock");
        Self {
            store: Self::select_store(&workgroup_root),
            node_id,
            leader_lock,
            poll: DEFAULT_POLL_INTERVAL,
            authorizer: Arc::new(ActionAuthorizer::production()),
            app_state_signer: crate::ipc::action_auth::production_action_signer().ok(),
            bus_root_override: None,
        }
    }

    /// Pick the session store the SAME etcd-first-with-fs-fallback way the peer
    /// directory does ([`crate::substrate::peers::read_directory`]): when
    /// `/etc/mackesd/etcd-endpoints` is non-empty the sessions ride the
    /// lease-backed [`EtcdSessionStore`] (a crashed node's rows auto-expire), else
    /// the replicated-file [`MeshSessionStore`] carries them. Both satisfy the same
    /// [`SessionStore`] seam, so the broker fold is identical either way.
    fn select_store(workgroup_root: &Path) -> Box<dyn SessionStore + Send + Sync> {
        let endpoints = crate::substrate::etcd::default_endpoints();
        if endpoints.is_empty() {
            Box::new(MeshSessionStore::new(workgroup_root.to_path_buf()))
        } else {
            Box::new(EtcdSessionStore::new(endpoints))
        }
    }

    /// Inject a session store (tests). Production uses [`MeshSessionStore`].
    #[must_use]
    pub fn with_store(mut self, store: Box<dyn SessionStore + Send + Sync>) -> Self {
        self.store = store;
        self
    }

    /// Inject an isolated verifier and replay ledger for hostile action tests.
    /// Production always uses the systemd-credential-backed authorizer.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_authorizer(mut self, authorizer: Arc<ActionAuthorizer>) -> Self {
        self.authorizer = authorizer;
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

    fn bus_root(&self) -> PathBuf {
        session_bus_root(self.bus_root_override.clone(), default_bus_root())
    }

    fn open_bus(&self) -> Result<(PathBuf, Persist, SessionBusIdentity), String> {
        let root = self.bus_root();
        let persist = Persist::open(root.clone())
            .map_err(|error| format!("open session Bus {}: {error}", root.display()))?;
        let identity = session_bus_identity(&root)?;
        Ok((root, persist, identity))
    }

    /// A replaced index is a new transient ingress boundary. Capture both tails
    /// before installing its identity so retained lifecycle and runtime rows can
    /// never replay against the already-converged roster.
    fn activate_replacement(
        &self,
        persist: &Persist,
        identity: &SessionBusIdentity,
    ) -> Result<(Option<String>, Option<String>), String> {
        let action_tail = persist
            .latest_ulid(ACTION_TOPIC)
            .map_err(|error| format!("prime session action tail: {error}"))?;
        let runtime_tail = persist
            .latest_ulid(APP_VM_RUNTIME_TOPIC)
            .map_err(|error| format!("prime App VM runtime tail: {error}"))?;
        if session_bus_identity(&identity.root)? != *identity {
            return Err("session Bus changed during replacement activation".to_string());
        }
        Ok((action_tail, runtime_tail))
    }

    /// Only the elected node converges the shared plane (no-fixed-center: any
    /// eligible node can be it, the elected one writes). Reuses the shared lock.
    fn is_leader(&self) -> bool {
        crate::leader_gate::LeaderGate::from_lock_path(
            self.leader_lock.clone(),
            self.node_id.clone(),
        )
        .is_leader()
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
}

#[async_trait::async_trait]
impl Worker for SessionBrokerWorker {
    fn name(&self) -> &'static str {
        "session_broker"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        // Read the FULL action log from the start (unlike `scheduler`, which primes
        // past the backlog): a session's state is a fold of the whole log, so a
        // (re)start must rebuild the complete roster before it converges.
        let mut active_bus: Option<SessionBusIdentity> = None;
        let mut cursor: Option<String> = None;
        let mut runtime_cursor: Option<String> = None;
        let mut roster: BTreeMap<SessionId, VdiSession> = BTreeMap::new();
        let mut tick = tokio::time::interval(self.poll);
        tick.tick().await; // consume the immediate first tick
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    let (bus_root, persist, identity) = match self.open_bus() {
                        Ok(opened) => opened,
                        Err(error) => {
                            tracing::debug!(%error, "session_broker: Bus unavailable; convergence deferred");
                            continue;
                        }
                    };
                    if active_bus.as_ref().is_some_and(|active| active != &identity) {
                        let (action_tail, evidence_tail) = match self.activate_replacement(&persist, &identity) {
                            Ok(tails) => tails,
                            Err(error) => {
                                tracing::debug!(%error, "session_broker: replacement Bus activation deferred");
                                continue;
                            }
                        };
                        cursor = action_tail;
                        runtime_cursor = evidence_tail;
                    }
                    active_bus = Some(identity);
                    // Fold the whole session log into the roster, then converge.
                    if let Err(error) = drain(
                        &bus_root,
                        &mut cursor,
                        &mut roster,
                        self.authorizer.as_ref(),
                    ) {
                        tracing::debug!(%error, "session_broker: Bus unavailable; convergence deferred");
                        continue;
                    }
                    if let Some(signer) = self.app_state_signer.as_ref() {
                        for session in roster.values().filter(|session| {
                            session.app.is_some()
                                && session.serving_peer == self.node_id
                                && session.app_state == Some(AppVmLifecycleState::WaitingForPlacement)
                        }) {
                            if let Err(error) = publish_app_state(
                                &bus_root,
                                signer,
                                &session.id,
                                session.app_state_generation.saturating_add(1),
                                AppVmLifecycleState::StartingGuest,
                                Some("App VM declaration admitted; waiting for guest boot evidence"),
                            ) {
                                tracing::debug!(session = %session.id, %error, "session_broker: app readiness publication deferred");
                            }
                        }
                    }
                    for evidence in read_runtime_evidence(&bus_root, &mut runtime_cursor) {
                        apply_runtime_evidence(
                            &mut roster,
                            evidence,
                            &self.node_id,
                            &bus_root,
                            self.app_state_signer.as_ref(),
                        );
                    }
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
    use crate::ipc::action_auth::authorize_test_body;
    use std::sync::{Arc, Mutex};

    const AUTH_KEY: &[u8] = b"vdi-session-action-auth-test-key";
    const AUTH_NOW: i64 = 1_700_000_000_000;

    fn sess(id: &str, state: SessionState) -> VdiSession {
        VdiSession {
            id: id.to_string(),
            serving_peer: "peer:host".to_string(),
            vm_id: "uuid-1".to_string(),
            client_peer: "peer:client".to_string(),
            profile: None,
            state,
            opened_at_ms: 100,
            updated_at_ms: 100,
            app: None,
            app_state: None,
            app_state_reason: None,
            app_state_generation: 0,
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
                profile: None,
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
    fn browser_transport_profile_is_retained_and_never_defaults_to_rdp() {
        let mut roster = BTreeMap::new();
        for (id, transport) in [
            ("browser-sunshine", BrowserVmTransport::Sunshine),
            ("browser-rdp", BrowserVmTransport::Rdp),
        ] {
            apply_request(
                &mut roster,
                SessionRequest::Open {
                    id: id.into(),
                    serving_peer: "peer:dell".into(),
                    vm_id: BROWSER_VM_WORKLOAD_ID.into(),
                    client_peer: "peer:surface".into(),
                    profile: Some(transport.session_profile()),
                },
                10,
            )
            .expect("typed Browser VM open");
            assert_eq!(
                roster[id]
                    .profile
                    .map(DesktopSessionProfile::browser_transport),
                Some(transport)
            );
        }
        assert_eq!(
            roster["browser-sunshine"]
                .profile
                .map(DesktopSessionProfile::browser_transport),
            Some(BrowserVmTransport::Sunshine)
        );
    }

    #[test]
    fn browser_profile_cannot_retarget_an_arbitrary_vm() {
        let mut roster = BTreeMap::new();
        let result = apply_request(
            &mut roster,
            SessionRequest::Open {
                id: "wrong-vm".into(),
                serving_peer: "peer:dell".into(),
                vm_id: "unrelated-vm".into(),
                client_peer: "peer:surface".into(),
                profile: Some(DesktopSessionProfile::BrowserVm),
            },
            10,
        );
        assert_eq!(
            result,
            Err(SessionError::InvalidDesktopProfile {
                vm_id: "unrelated-vm".into(),
                reason: BrowserVmProfileError::WrongWorkload,
            })
        );
        assert!(roster.is_empty());
    }

    #[test]
    fn browser_vm_without_a_profile_is_rejected_before_roster_admission() {
        let mut roster = BTreeMap::new();
        let result = apply_request(
            &mut roster,
            SessionRequest::Open {
                id: "untyped-browser".into(),
                serving_peer: "peer:dell".into(),
                vm_id: BROWSER_VM_WORKLOAD_ID.into(),
                client_peer: "peer:surface".into(),
                profile: None,
            },
            10,
        );
        assert_eq!(
            result,
            Err(SessionError::InvalidDesktopProfile {
                vm_id: BROWSER_VM_WORKLOAD_ID.into(),
                reason: BrowserVmProfileError::MissingProfile,
            })
        );
        assert!(roster.is_empty());
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

    #[test]
    fn apply_request_opens_validated_app_vm_session() {
        let mut roster = BTreeMap::new();
        apply_request(
            &mut roster,
            SessionRequest::OpenApp {
                id: "app-s".into(),
                serving_peer: "peer:host".into(),
                vm_id: "app-vm".into(),
                client_peer: "peer:seat".into(),
                app_id: "org.example.Editor".into(),
                catalog_revision: "catalog-1".into(),
                guest_profile: "wayland-standard".into(),
                requested_capabilities: vec!["audio".into()],
                resume: true,
            },
            42,
        )
        .expect("valid App VM open");
        let session = &roster["app-s"];
        assert_eq!(session.vm_id, "app-vm");
        assert_eq!(
            session.app.as_ref().map(|app| app.app_id.as_str()),
            Some("org.example.Editor")
        );
        assert!(session.app.as_ref().is_some_and(|app| app.resume));
    }

    #[test]
    fn repeated_app_open_preserves_guest_readiness_and_refreshes_intent() {
        let mut roster = BTreeMap::new();
        let open = |catalog_revision: &str, resume: bool| SessionRequest::OpenApp {
            id: "app-s".into(),
            serving_peer: "peer:host".into(),
            vm_id: "app-vm".into(),
            client_peer: "peer:seat".into(),
            app_id: "org.example.Editor".into(),
            catalog_revision: catalog_revision.into(),
            guest_profile: "wayland-standard".into(),
            requested_capabilities: vec!["audio".into()],
            resume,
        };
        apply_request(&mut roster, open("catalog-1", false), 42).expect("initial open");
        apply_request(
            &mut roster,
            SessionRequest::AppState {
                id: "app-s".into(),
                generation: 0,
                state: AppVmLifecycleState::Installing,
                reason: Some("image admitted".into()),
            },
            43,
        )
        .expect("installing");
        apply_request(
            &mut roster,
            SessionRequest::AppState {
                id: "app-s".into(),
                generation: 0,
                state: AppVmLifecycleState::StartingGuest,
                reason: None,
            },
            43,
        )
        .expect("guest boot");
        apply_request(
            &mut roster,
            SessionRequest::AppState {
                id: "app-s".into(),
                generation: 0,
                state: AppVmLifecycleState::StartingApp,
                reason: Some("compositor ready".into()),
            },
            43,
        )
        .expect("app start");

        apply_request(&mut roster, open("catalog-2", true), 44).expect("idempotent retry");
        let session = &roster["app-s"];
        assert_eq!(session.state, SessionState::Requested);
        assert_eq!(session.app_state, Some(AppVmLifecycleState::StartingApp));
        assert_eq!(
            session.app_state_reason.as_deref(),
            Some("compositor ready")
        );
        assert_eq!(session.updated_at_ms, 44);
        let app = session.app.as_ref().expect("app declaration");
        assert_eq!(app.catalog_revision, "catalog-2");
        assert!(app.resume, "retry carries the new resume intent");
    }

    #[test]
    fn app_open_retarget_is_rejected_without_mutating_the_admitted_session() {
        let mut roster = BTreeMap::new();
        let initial = SessionRequest::OpenApp {
            id: "app-s".into(),
            serving_peer: "peer:host".into(),
            vm_id: "app-vm".into(),
            client_peer: "peer:seat".into(),
            app_id: "org.example.Editor".into(),
            catalog_revision: "catalog-1".into(),
            guest_profile: "wayland-standard".into(),
            requested_capabilities: Vec::new(),
            resume: true,
        };
        apply_request(&mut roster, initial, 42).expect("initial open");
        apply_request(
            &mut roster,
            SessionRequest::AppState {
                id: "app-s".into(),
                generation: 0,
                state: AppVmLifecycleState::Installing,
                reason: None,
            },
            43,
        )
        .expect("installing");
        let before = roster.clone();

        let result = apply_request(
            &mut roster,
            SessionRequest::OpenApp {
                id: "app-s".into(),
                serving_peer: "peer:other-host".into(),
                vm_id: "other-vm".into(),
                client_peer: "peer:seat".into(),
                app_id: "org.example.Editor".into(),
                catalog_revision: "catalog-1".into(),
                guest_profile: "wayland-standard".into(),
                requested_capabilities: Vec::new(),
                resume: true,
            },
            44,
        );
        assert_eq!(
            result,
            Err(SessionError::ConflictingAppSession {
                id: "app-s".into(),
                reason: "VM or serving/client peer changed",
            })
        );
        assert_eq!(roster, before, "replay rejection is atomic");
    }

    #[test]
    fn closed_app_session_cannot_be_resurrected_by_a_replay() {
        let mut roster = BTreeMap::new();
        let request = SessionRequest::OpenApp {
            id: "app-s".into(),
            serving_peer: "peer:host".into(),
            vm_id: "app-vm".into(),
            client_peer: "peer:seat".into(),
            app_id: "org.example.Editor".into(),
            catalog_revision: "catalog-1".into(),
            guest_profile: "wayland-standard".into(),
            requested_capabilities: Vec::new(),
            resume: true,
        };
        apply_request(&mut roster, request.clone(), 42).expect("initial open");
        apply_request(
            &mut roster,
            SessionRequest::Close { id: "app-s".into() },
            43,
        )
        .expect("close");
        let result = apply_request(&mut roster, request, 44);
        assert!(matches!(
            result,
            Err(SessionError::ConflictingAppSession { reason, .. })
                if reason.contains("closed")
        ));
        assert_eq!(roster["app-s"].state, SessionState::Closed);
    }

    #[test]
    fn apply_request_rejects_invalid_app_vm_before_roster_mutation() {
        let mut roster = BTreeMap::new();
        let result = apply_request(
            &mut roster,
            SessionRequest::OpenApp {
                id: "app-s".into(),
                serving_peer: "peer:host".into(),
                vm_id: "app-vm".into(),
                client_peer: "peer:seat".into(),
                app_id: "org.example.Editor".into(),
                catalog_revision: "catalog-1".into(),
                guest_profile: "/tmp/image".into(),
                requested_capabilities: Vec::new(),
                resume: false,
            },
            42,
        );
        assert!(matches!(result, Err(SessionError::InvalidAppVm(_))));
        assert!(roster.is_empty());
    }

    #[test]
    fn apply_request_rejects_unadmitted_app_identity_and_capability() {
        for (app_id, requested_capabilities, expected) in [
            (
                "host-command",
                Vec::new(),
                mackes_mesh_types::vdi_session::AppVmLaunchRequestError::InvalidField("app_id"),
            ),
            (
                "org.example.Editor",
                vec!["host_socket".to_owned()],
                mackes_mesh_types::vdi_session::AppVmLaunchRequestError::InvalidCapability,
            ),
        ] {
            let mut roster = BTreeMap::new();
            let result = apply_request(
                &mut roster,
                SessionRequest::OpenApp {
                    id: "app-s".into(),
                    serving_peer: "peer:host".into(),
                    vm_id: "app-vm".into(),
                    client_peer: "peer:seat".into(),
                    app_id: app_id.into(),
                    catalog_revision: "catalog-1".into(),
                    guest_profile: "wayland-standard".into(),
                    requested_capabilities,
                    resume: false,
                },
                42,
            );
            assert_eq!(result, Err(SessionError::InvalidAppVm(expected)));
            assert!(roster.is_empty(), "rejected handoff must not enter roster");
        }
    }

    #[test]
    fn app_state_rejects_false_jump_and_accepts_idempotent_retry() {
        let mut roster = BTreeMap::new();
        apply_request(
            &mut roster,
            SessionRequest::OpenApp {
                id: "app-s".into(),
                serving_peer: "peer:host".into(),
                vm_id: "app-vm".into(),
                client_peer: "peer:seat".into(),
                app_id: "org.example.Editor".into(),
                catalog_revision: "catalog-1".into(),
                guest_profile: "wayland-standard".into(),
                requested_capabilities: Vec::new(),
                resume: false,
            },
            42,
        )
        .expect("open app");

        assert!(matches!(
            apply_request(
                &mut roster,
                SessionRequest::AppState {
                    id: "app-s".into(),
                    generation: 0,
                    state: AppVmLifecycleState::Connected,
                    reason: None,
                },
                43,
            ),
            Err(SessionError::IllegalAppState {
                from: AppVmLifecycleState::WaitingForPlacement,
                to: AppVmLifecycleState::Connected,
            })
        ));
        apply_request(
            &mut roster,
            SessionRequest::AppState {
                id: "app-s".into(),
                generation: 0,
                state: AppVmLifecycleState::Installing,
                reason: Some("image admission pending".into()),
            },
            44,
        )
        .expect("installing is a legal first transition");
        apply_request(
            &mut roster,
            SessionRequest::AppState {
                id: "app-s".into(),
                generation: 0,
                state: AppVmLifecycleState::Installing,
                reason: Some("retrying image admission".into()),
            },
            45,
        )
        .expect("same state is an idempotent retry");
        assert_eq!(
            roster["app-s"].app_state,
            Some(AppVmLifecycleState::Installing)
        );
    }

    #[test]
    fn closed_app_session_rejects_stale_app_state_without_mutation() {
        let mut roster = BTreeMap::new();
        apply_request(
            &mut roster,
            SessionRequest::OpenApp {
                id: "app-s".into(),
                serving_peer: "peer:host".into(),
                vm_id: "app-vm".into(),
                client_peer: "peer:seat".into(),
                app_id: "org.example.Editor".into(),
                catalog_revision: "catalog-1".into(),
                guest_profile: "wayland-standard".into(),
                requested_capabilities: Vec::new(),
                resume: false,
            },
            42,
        )
        .expect("open app");
        apply_request(
            &mut roster,
            SessionRequest::AppState {
                id: "app-s".into(),
                generation: 0,
                state: AppVmLifecycleState::Installing,
                reason: Some("admitted before close".into()),
            },
            43,
        )
        .expect("installing");
        apply_request(
            &mut roster,
            SessionRequest::Close { id: "app-s".into() },
            44,
        )
        .expect("close");
        let before = roster.clone();

        let result = apply_request(
            &mut roster,
            SessionRequest::AppState {
                id: "app-s".into(),
                generation: 0,
                state: AppVmLifecycleState::Installing,
                reason: Some("stale retry after close".into()),
            },
            45,
        );

        assert_eq!(
            result,
            Err(SessionError::IllegalTransition {
                from: SessionState::Closed,
                to: SessionState::Closed,
            })
        );
        assert_eq!(roster, before, "stale AppState rejection is atomic");
    }

    #[test]
    fn app_state_rejects_replayed_generation_without_mutation() {
        let mut roster = BTreeMap::new();
        apply_request(
            &mut roster,
            SessionRequest::OpenApp {
                id: "app-s".into(),
                serving_peer: "peer:host".into(),
                vm_id: "app-vm".into(),
                client_peer: "peer:seat".into(),
                app_id: "org.example.Editor".into(),
                catalog_revision: "catalog-1".into(),
                guest_profile: "wayland-standard".into(),
                requested_capabilities: Vec::new(),
                resume: false,
            },
            42,
        )
        .expect("open app");
        apply_request(
            &mut roster,
            SessionRequest::AppState {
                id: "app-s".into(),
                generation: 1,
                state: AppVmLifecycleState::Installing,
                reason: Some("image admitted".into()),
            },
            43,
        )
        .expect("generation one");
        apply_request(
            &mut roster,
            SessionRequest::AppState {
                id: "app-s".into(),
                generation: 2,
                state: AppVmLifecycleState::StartingGuest,
                reason: Some("guest boot".into()),
            },
            44,
        )
        .expect("generation two");
        let before = roster.clone();

        let result = apply_request(
            &mut roster,
            SessionRequest::AppState {
                id: "app-s".into(),
                generation: 1,
                state: AppVmLifecycleState::StartingGuest,
                reason: Some("replayed guest boot".into()),
            },
            45,
        );

        assert_eq!(result, Err(SessionError::StaleAppState { generation: 1 }));
        assert_eq!(roster, before, "generation rejection is atomic");
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
                profile: None,
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
    fn daemon_app_readiness_body_is_signed_for_the_exact_session() {
        let signer = CloudArmSigner::new(b"session-broker-test-key".to_vec()).unwrap();
        let body = signed_app_state_body(
            &signer,
            "app-s1",
            1,
            AppVmLifecycleState::StartingGuest,
            Some("guest boot pending"),
        )
        .expect("signed readiness body");
        let value: serde_json::Value = serde_json::from_str(&body).expect("json");
        assert_eq!(value["op"], "app_state");
        assert_eq!(value["id"], "app-s1");
        assert_eq!(value["state"], "starting_guest");
        assert_eq!(value["schema_version"], CLOUD_ACTION_SCHEMA_VERSION);
        let token = value["armed_token"].as_str().expect("armed token");
        let parsed = CloudArmedToken::parse(token).expect("token shape");
        assert_eq!(parsed.verb, "vdi-session-app-state");
        assert_eq!(parsed.node, "vdi-session");
        assert_eq!(parsed.target, "session:app-s1");
        let unsigned = serde_json::json!({
            "op": "app_state",
            "id": "app-s1",
            "generation": 1,
            "state": "starting_guest",
            "reason": "guest boot pending",
            "schema_version": CLOUD_ACTION_SCHEMA_VERSION,
        })
        .to_string();
        assert_eq!(
            parsed.request_sha256,
            cloud_request_digest(&unsigned).expect("digest")
        );
        assert!(signer.verify_payload(&parsed.signing_payload(), &parsed.signature));
    }

    #[test]
    fn guest_runtime_evidence_advances_only_the_matching_session() {
        let bus = tempfile::tempdir().expect("runtime bus");
        let signer = CloudArmSigner::new(b"runtime-evidence-test-key".to_vec()).unwrap();
        let mut roster = BTreeMap::new();
        apply_request(
            &mut roster,
            SessionRequest::OpenApp {
                id: "app-s1".into(),
                serving_peer: "node-a".into(),
                vm_id: "app-vm-1".into(),
                client_peer: "seat-a".into(),
                app_id: "org.example.Editor".into(),
                catalog_revision: "catalog-1".into(),
                guest_profile: "wayland-standard".into(),
                requested_capabilities: Vec::new(),
                resume: false,
            },
            1,
        )
        .expect("open app");
        apply_request(
            &mut roster,
            SessionRequest::AppState {
                id: "app-s1".into(),
                generation: 0,
                state: AppVmLifecycleState::StartingGuest,
                reason: None,
            },
            2,
        )
        .expect("guest starts");

        apply_runtime_evidence(
            &mut roster,
            AppVmRuntimeEvidence {
                session_id: "app-s1".into(),
                vm_id: "app-vm-1".into(),
                app_id: "org.example.Editor".into(),
                generation: 1,
                state: mackes_mesh_types::vdi_session::AppVmRuntimeState::StartingApp,
                reason: Some("portal ready".into()),
            },
            "node-a",
            bus.path(),
            Some(&signer),
        );
        assert_eq!(
            roster["app-s1"].app_state,
            Some(AppVmLifecycleState::StartingApp)
        );
        let actions = Persist::open(bus.path().to_path_buf())
            .expect("open action bus")
            .list_since(ACTION_TOPIC, None)
            .expect("read actions");
        assert_eq!(actions.len(), 1);
        assert!(actions[0]
            .body
            .as_deref()
            .is_some_and(|body| body.contains("starting_app") && body.contains("armed_token")));

        apply_runtime_evidence(
            &mut roster,
            AppVmRuntimeEvidence {
                session_id: "app-s1".into(),
                vm_id: "other-vm".into(),
                app_id: "org.example.Editor".into(),
                generation: 2,
                state: mackes_mesh_types::vdi_session::AppVmRuntimeState::Connected,
                reason: None,
            },
            "node-a",
            bus.path(),
            Some(&signer),
        );
        assert_eq!(
            roster["app-s1"].app_state,
            Some(AppVmLifecycleState::StartingApp),
            "mismatched guest identity cannot advance the session"
        );
    }

    #[test]
    fn guest_crash_is_recorded_but_illegal_runtime_recovery_is_atomic() {
        let bus = tempfile::tempdir().expect("runtime bus");
        let signer = CloudArmSigner::new(b"runtime-crash-test-key".to_vec()).unwrap();
        let mut roster = BTreeMap::new();
        apply_request(
            &mut roster,
            SessionRequest::OpenApp {
                id: "app-crash".into(),
                serving_peer: "node-a".into(),
                vm_id: "app-vm-crash".into(),
                client_peer: "seat-a".into(),
                app_id: "org.example.Editor".into(),
                catalog_revision: "catalog-1".into(),
                guest_profile: "wayland-standard".into(),
                requested_capabilities: Vec::new(),
                resume: false,
            },
            1,
        )
        .expect("open app");
        for state in [
            AppVmLifecycleState::StartingGuest,
            AppVmLifecycleState::StartingApp,
            AppVmLifecycleState::Connected,
        ] {
            apply_request(
                &mut roster,
                SessionRequest::AppState {
                    id: "app-crash".into(),
                    generation: 0,
                    state,
                    reason: None,
                },
                2,
            )
            .expect("valid readiness transition");
        }

        apply_runtime_evidence(
            &mut roster,
            AppVmRuntimeEvidence {
                session_id: "app-crash".into(),
                vm_id: "app-vm-crash".into(),
                app_id: "org.example.Editor".into(),
                generation: 1,
                state: mackes_mesh_types::vdi_session::AppVmRuntimeState::Failed,
                reason: Some("guest app process exited".into()),
            },
            "node-a",
            bus.path(),
            Some(&signer),
        );
        assert_eq!(
            roster["app-crash"].app_state,
            Some(AppVmLifecycleState::Failed),
            "a connected guest process crash must enter the explicit failed state"
        );
        assert_eq!(
            roster["app-crash"].app_state_reason.as_deref(),
            Some("guest app process exited")
        );
        let actions_before_recovery = Persist::open(bus.path().to_path_buf())
            .expect("open action bus")
            .list_since(ACTION_TOPIC, None)
            .expect("read crash action");
        assert_eq!(actions_before_recovery.len(), 1);
        let roster_before_recovery = roster.clone();

        // A stale guest report cannot jump directly from Failed to Connected.
        // The candidate fold and signed publication must both be absent, so a
        // malformed recovery report cannot mutate either local or shared state.
        apply_runtime_evidence(
            &mut roster,
            AppVmRuntimeEvidence {
                session_id: "app-crash".into(),
                vm_id: "app-vm-crash".into(),
                app_id: "org.example.Editor".into(),
                generation: 1,
                state: mackes_mesh_types::vdi_session::AppVmRuntimeState::Connected,
                reason: Some("stale connected report".into()),
            },
            "node-a",
            bus.path(),
            Some(&signer),
        );
        assert_eq!(roster, roster_before_recovery);
        let actions_after_recovery = Persist::open(bus.path().to_path_buf())
            .expect("reopen action bus")
            .list_since(ACTION_TOPIC, None)
            .expect("read action bus after rejected recovery");
        assert_eq!(actions_after_recovery, actions_before_recovery);
    }

    #[test]
    fn topic_is_namespaced() {
        assert_eq!(ACTION_TOPIC, "action/vdi/session");
        assert!(ACTION_TOPIC.starts_with("action/"));
    }

    #[test]
    fn unsigned_session_action_is_refused_before_roster_mutation() {
        use mde_bus::hooks::config::Priority;

        let bus = tempfile::tempdir().expect("temp bus");
        let persist = Persist::open(bus.path().to_path_buf()).expect("open bus");
        let unsigned = serde_json::json!({
            "schema_version": 1,
            "op": "open",
            "id": "unsigned",
            "serving_peer": "peer:a",
            "vm_id": "vm-unsigned",
            "client_peer": "peer:b"
        })
        .to_string();
        persist
            .write(ACTION_TOPIC, Priority::Default, None, Some(&unsigned))
            .expect("write unsigned action");
        let authorizer = ActionAuthorizer::for_test(AUTH_KEY, bus.path().join("auth"), AUTH_NOW);
        let mut cursor = None;
        let mut roster = BTreeMap::new();
        drain(bus.path(), &mut cursor, &mut roster, &authorizer)
            .expect("drain authorized session actions");
        assert!(roster.is_empty(), "unsigned action mutated the roster");
    }

    #[test]
    fn authorized_session_action_is_exact_body_bound_and_single_use() {
        use mde_bus::hooks::config::Priority;

        let bus = tempfile::tempdir().expect("temp bus");
        let persist = Persist::open(bus.path().to_path_buf()).expect("open bus");
        let req = SessionRequest::Open {
            id: "authorized".into(),
            serving_peer: "peer:a".into(),
            vm_id: "vm-1".into(),
            client_peer: "peer:b".into(),
            profile: None,
        };
        let armed = signed_body(&req, "session-replay");
        let tampered = armed.replace("vm-1", "vm-2");
        // A body tamper must not consume the original capability. The original
        // then succeeds once; publishing it again exercises the replay ledger.
        for body in [&tampered, &armed, &armed] {
            persist
                .write(ACTION_TOPIC, Priority::Default, None, Some(body))
                .expect("write session action");
        }
        let authorizer = ActionAuthorizer::for_test(AUTH_KEY, bus.path().join("auth"), AUTH_NOW);
        let mut cursor = None;
        let mut roster = BTreeMap::new();
        drain(bus.path(), &mut cursor, &mut roster, &authorizer).expect("drain replay fixture");
        assert_eq!(roster.len(), 1);
        assert_eq!(roster["authorized"].vm_id, "vm-1");
    }

    #[test]
    fn default_bus_root_uses_the_shared_mde_bus_resolver() {
        assert_eq!(default_bus_root(), mde_bus::default_data_dir());
        assert_eq!(
            session_bus_root(None, None),
            PathBuf::from(mde_bus::SYSTEM_BUS_ROOT)
        );
        assert_eq!(
            session_bus_root(Some(PathBuf::from("/tmp/session-bus")), None),
            PathBuf::from("/tmp/session-bus")
        );
    }

    // ── the store seam ──

    #[test]
    fn mesh_session_store_round_trips_sorted_records_and_removes_idempotently() {
        let tmp = tempfile::tempdir().expect("temp workgroup");
        let store = MeshSessionStore::new(tmp.path().to_path_buf());
        assert!(
            store
                .list()
                .expect("missing dir is an empty roster")
                .is_empty(),
            "a fresh workgroup has no sessions"
        );

        let s2 = sess("s2/slash", SessionState::Requested);
        let s1 = sess("s1", SessionState::Active);
        store.publish(&s2).expect("publish s2");
        store.publish(&s1).expect("publish s1");
        let rows = store.list().expect("list sessions");
        assert_eq!(
            rows.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            vec!["s1", "s2/slash"],
            "list order is deterministic by session id"
        );
        assert!(
            store.path_for("s2/slash").exists(),
            "unsafe ids are encoded into safe filenames"
        );

        store.remove("s1").expect("remove existing");
        store.remove("s1").expect("remove is idempotent");
        let rows = store.list().expect("list after remove");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "s2/slash");
    }

    #[test]
    fn session_store_reader_rejects_hostile_persisted_material() {
        use std::io::Write as _;

        let tmp = tempfile::tempdir().expect("temp session records");
        let valid = tmp.path().join("valid.json");
        let valid_body = serde_json::to_vec(&sess("valid", SessionState::Active))
            .expect("serialize valid session");
        std::fs::write(&valid, &valid_body).expect("write valid session");
        assert!(read_bounded_session_record(&valid).is_ok());

        let invalid_utf8 = tmp.path().join("invalid-utf8.json");
        std::fs::write(&invalid_utf8, [b'{', 0xff, b'}']).expect("write invalid UTF-8");
        assert_eq!(
            read_bounded_session_record(&invalid_utf8)
                .expect_err("invalid UTF-8 must be rejected")
                .kind(),
            std::io::ErrorKind::InvalidData
        );

        let oversized = tmp.path().join("oversized.json");
        std::fs::write(&oversized, vec![b'x'; MAX_SESSION_RECORD_BYTES + 1])
            .expect("write oversized session");
        assert_eq!(
            read_bounded_session_record(&oversized)
                .expect_err("oversized records must be rejected")
                .kind(),
            std::io::ErrorKind::InvalidData
        );

        let directory = tmp.path().join("directory.json");
        std::fs::create_dir(&directory).expect("create special-file substitute");
        assert!(read_bounded_session_record(&directory).is_err());

        let growth = tmp.path().join("growth.json");
        std::fs::write(&growth, &valid_body).expect("write growth fixture");
        let file = std::fs::File::open(&growth).expect("open growth fixture");
        let expected_len = file.metadata().expect("stat growth fixture").len();
        std::fs::OpenOptions::new()
            .append(true)
            .open(&growth)
            .expect("reopen growth fixture")
            .write_all(b"x")
            .expect("grow fixture");
        assert_eq!(
            read_bounded_session_file(file, &growth, expected_len)
                .expect_err("growth between stat and read must be rejected")
                .kind(),
            std::io::ErrorKind::InvalidData
        );

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&valid, tmp.path().join("linked.json"))
                .expect("create final symlink");
            assert!(read_bounded_session_record(&tmp.path().join("linked.json")).is_err());
        }
    }

    #[test]
    fn session_store_file_stems_escape_path_separators() {
        assert_eq!(safe_session_file_stem(""), "_");
        assert_eq!(safe_session_file_stem("vdi:ok-1"), "vdi:ok-1");
        assert_eq!(safe_session_file_stem("a/b c"), "a_2fb_20c");
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

    // ── the etcd lease-backed store (E12-5/8) ──

    /// A fake [`SessionLeaseOps`] with a controllable clock — the injected etcd
    /// seam that drives the [`EtcdSessionStore`]'s lease logic (grant / keep-alive /
    /// revoke / expiry-frees) without a live etcd node. `advance` moves the
    /// simulated wall clock; `reap` models etcd's background lease expiry (every
    /// key bound to an expired lease auto-deletes).
    #[derive(Clone, Default)]
    struct FakeLeaseOps {
        inner: Arc<Mutex<FakeEtcd>>,
    }

    #[derive(Default)]
    struct FakeEtcd {
        now_s: i64,
        next_lease: i64,
        /// lease id → (ttl, absolute deadline in `now_s`).
        leases: BTreeMap<i64, (i64, i64)>,
        /// session id → (json value, bound lease id).
        rows: BTreeMap<String, (String, i64)>,
        granted_ttls: Vec<i64>,
        keep_alives: Vec<i64>,
    }

    impl FakeEtcd {
        /// Delete every row whose lease deadline has passed — etcd's auto-free.
        fn reap(&mut self) {
            let now = self.now_s;
            let dead: Vec<i64> = self
                .leases
                .iter()
                .filter(|(_, (_, deadline))| *deadline <= now)
                .map(|(id, _)| *id)
                .collect();
            for id in &dead {
                self.leases.remove(id);
            }
            self.rows.retain(|_, (_, lease)| !dead.contains(lease));
        }
    }

    impl FakeLeaseOps {
        fn advance(&self, secs: i64) {
            self.inner.lock().expect("fake etcd").now_s += secs;
        }
        fn granted_ttls(&self) -> Vec<i64> {
            self.inner.lock().expect("fake etcd").granted_ttls.clone()
        }
        fn keep_alive_count(&self) -> usize {
            self.inner.lock().expect("fake etcd").keep_alives.len()
        }
        fn live_row_count(&self) -> usize {
            let mut g = self.inner.lock().expect("fake etcd");
            g.reap();
            g.rows.len()
        }
    }

    impl SessionLeaseOps for FakeLeaseOps {
        fn grant_lease(&self, ttl_s: i64) -> Result<i64, SessionStoreError> {
            let mut g = self.inner.lock().expect("fake etcd");
            g.next_lease += 1;
            let id = g.next_lease;
            let deadline = g.now_s + ttl_s;
            g.leases.insert(id, (ttl_s, deadline));
            g.granted_ttls.push(ttl_s);
            Ok(id)
        }
        fn keep_alive(&self, lease_id: i64) -> Result<(), SessionStoreError> {
            let mut g = self.inner.lock().expect("fake etcd");
            g.reap();
            match g.leases.get(&lease_id).copied() {
                Some((ttl, _)) => {
                    let deadline = g.now_s + ttl;
                    g.leases.insert(lease_id, (ttl, deadline));
                    g.keep_alives.push(lease_id);
                    Ok(())
                }
                None => Err(SessionStoreError::Failed {
                    op: "keep_alive",
                    reason: format!("lease {lease_id} expired"),
                }),
            }
        }
        fn put(&self, id: &str, value_json: &str, lease_id: i64) -> Result<(), SessionStoreError> {
            self.inner
                .lock()
                .expect("fake etcd")
                .rows
                .insert(id.to_string(), (value_json.to_string(), lease_id));
            Ok(())
        }
        fn list(&self) -> Result<Vec<String>, SessionStoreError> {
            let mut g = self.inner.lock().expect("fake etcd");
            g.reap();
            Ok(g.rows.values().map(|(v, _)| v.clone()).collect())
        }
        fn revoke_and_delete(
            &self,
            id: &str,
            lease_id: Option<i64>,
        ) -> Result<(), SessionStoreError> {
            let mut g = self.inner.lock().expect("fake etcd");
            g.rows.remove(id);
            if let Some(l) = lease_id {
                g.leases.remove(&l);
            }
            Ok(())
        }
    }

    #[test]
    fn etcd_store_publish_binds_the_row_to_a_ttl_lease() {
        let fake = FakeLeaseOps::default();
        let store = EtcdSessionStore::with_ops(Box::new(fake.clone()), 30);
        store
            .publish(&sess("s1", SessionState::Active))
            .expect("publish");
        // The write set a TTL lease (not a bare put) — the crash-expiry mechanism.
        assert_eq!(
            fake.granted_ttls(),
            vec![30],
            "the row is bound to a 30 s TTL lease"
        );
        let rows = store.list().expect("list");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "s1");
        assert_eq!(rows[0].state, SessionState::Active, "value round-trips");
    }

    #[test]
    fn etcd_store_keep_alive_renews_a_live_session_past_its_ttl() {
        let fake = FakeLeaseOps::default();
        let store = EtcdSessionStore::with_ops(Box::new(fake.clone()), 30);
        store
            .publish(&sess("s1", SessionState::Active))
            .expect("publish");
        // Three convergence ticks 20 s apart — 60 s total, twice a single 30 s TTL.
        // Each tick's list() keep-alive resets the lease, so the live session never
        // lapses (without renewal it would be gone by t=40 s).
        for _ in 0..3 {
            fake.advance(20);
            let rows = store.list().expect("list keeps the lease alive");
            assert_eq!(rows.len(), 1, "the live session survives via lease renewal");
        }
        assert!(
            fake.keep_alive_count() >= 3,
            "each convergence tick renewed the lease"
        );
    }

    #[test]
    fn etcd_store_expired_lease_frees_a_crashed_nodes_session() {
        let fake = FakeLeaseOps::default();
        // The converging node publishes a session...
        let store = EtcdSessionStore::with_ops(Box::new(fake.clone()), 30);
        store
            .publish(&sess("s1", SessionState::Active))
            .expect("publish");
        assert_eq!(fake.live_row_count(), 1, "the session is in the plane");

        // ...then CRASHES: no more list()/keep-alive. The wall clock passes the TTL.
        fake.advance(31);

        // A surviving peer (a fresh store over the SAME plane, with an empty lease
        // registry — it never owned this lease) sees the row already auto-deleted.
        // This is the whole point of E12-5/8: a crashed seat's session frees itself
        // via lease expiry, where the file store would leave a lingering row.
        let survivor = EtcdSessionStore::with_ops(Box::new(fake.clone()), 30);
        assert!(
            survivor.list().expect("list").is_empty(),
            "the crashed node's session auto-expired"
        );
        assert_eq!(fake.live_row_count(), 0);
    }

    #[test]
    fn etcd_store_republishes_under_a_fresh_lease_after_a_lost_lease() {
        let fake = FakeLeaseOps::default();
        let store = EtcdSessionStore::with_ops(Box::new(fake.clone()), 30);
        store
            .publish(&sess("s1", SessionState::Active))
            .expect("first publish");
        // The lease lapses (this node paused past the TTL); the registry still holds
        // the now-dead lease id.
        fake.advance(31);
        // reconcile re-publishes the still-desired session: keep-alive on the dead
        // lease fails, so the store re-grants rather than putting under a dead lease.
        store
            .publish(&sess("s1", SessionState::Active))
            .expect("re-publish after loss");
        assert_eq!(
            fake.granted_ttls(),
            vec![30, 30],
            "a fresh lease was granted on the lost-lease path"
        );
        assert_eq!(
            store.list().expect("list").len(),
            1,
            "the session is back in the plane under a live lease"
        );
    }

    #[test]
    fn etcd_store_remove_revokes_the_lease_and_deletes_the_row() {
        let fake = FakeLeaseOps::default();
        let store = EtcdSessionStore::with_ops(Box::new(fake.clone()), 30);
        store
            .publish(&sess("s1", SessionState::Active))
            .expect("publish");
        assert_eq!(fake.live_row_count(), 1);
        store.remove("s1").expect("remove");
        assert_eq!(fake.live_row_count(), 0, "the row is gone");
        // Idempotent — removing an absent id (no tracked lease) still succeeds.
        store.remove("s1").expect("remove is idempotent");
    }

    #[test]
    fn worker_name_matches_module() {
        let w = SessionBrokerWorker::new(std::env::temp_dir(), "peer:a".to_string());
        assert_eq!(w.name(), "session_broker");
    }

    // ── worker wiring (seeded temp bus + injected fake store) ──

    /// Sign one session request body for the isolated test authorizer.
    fn signed_body(req: &SessionRequest, nonce: &str) -> String {
        let mut unsigned = serde_json::to_value(req).expect("request value");
        unsigned
            .as_object_mut()
            .expect("session request object")
            .insert("schema_version".into(), serde_json::json!(1));
        let unsigned = unsigned.to_string();
        let (verb, target) = session_auth_target(req);
        authorize_test_body(
            AUTH_KEY,
            &unsigned,
            MutationContext {
                verb,
                node: "vdi-session",
                target: &target,
            },
            nonce,
            AUTH_NOW + 30_000,
        )
    }

    /// Seed a temp bus with authorized `action/vdi/session` bodies and return
    /// its root plus the matching isolated verifier.
    fn seed_bus(reqs: &[SessionRequest]) -> (PathBuf, Arc<ActionAuthorizer>) {
        use mde_bus::hooks::config::Priority;
        let dir = std::env::temp_dir().join(format!("mde-sb-{}-{}", now_ms(), reqs.len()));
        let persist = Persist::open(dir.clone()).expect("open bus");
        let authorizer = Arc::new(ActionAuthorizer::for_test(
            AUTH_KEY,
            dir.join("auth"),
            AUTH_NOW,
        ));
        for (index, r) in reqs.iter().enumerate() {
            persist
                .write(
                    ACTION_TOPIC,
                    Priority::Default,
                    None,
                    Some(&signed_body(r, &format!("seed-{index}"))),
                )
                .expect("write action");
        }
        (dir, authorizer)
    }

    #[tokio::test]
    async fn worker_drains_folds_and_converges_into_the_store() {
        // A session that opened + went active, drained off the bus and converged
        // into the injected store by the leader (a fresh temp workgroup ⇒ this
        // node wins the lock).
        let (bus, authorizer) = seed_bus(&[
            SessionRequest::Open {
                id: "s1".into(),
                serving_peer: "peer:a".into(),
                vm_id: "uuid-1".into(),
                client_peer: "peer:b".into(),
                profile: None,
            },
            SessionRequest::Active { id: "s1".into() },
        ]);
        let wg = std::env::temp_dir().join(format!("mde-sb-wg-{}", now_ms()));
        std::fs::create_dir_all(&wg).expect("mk workgroup");
        let store = FakeStore::default();
        let rows = store.rows.clone();
        let w = SessionBrokerWorker::new(wg.clone(), "peer:a".to_string())
            .with_store(Box::new(store))
            .with_bus_root(bus.clone())
            .with_authorizer(Arc::clone(&authorizer));

        let mut cursor = None;
        let mut roster = BTreeMap::new();
        drain(&bus, &mut cursor, &mut roster, authorizer.as_ref())
            .expect("drain initial session actions");
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
                    Some(&signed_body(
                        &SessionRequest::Close { id: "s1".into() },
                        "close-1",
                    )),
                )
                .expect("write close");
        }
        drain(&bus, &mut cursor2, &mut roster, authorizer.as_ref()).expect("drain close action");
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
        // An empty temp bus ⇒ nothing to fold; the production MeshSessionStore can
        // list an empty workgroup root without etcd or a pre-created session dir.
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

    #[tokio::test]
    async fn unavailable_bus_defers_convergence_without_removing_live_sessions() {
        let root = tempfile::tempdir().expect("session Bus failure fixture");
        let blocker = root.path().join("not-a-bus-directory");
        std::fs::write(&blocker, b"block Persist::open").expect("write Bus blocker");
        let workgroup = tempfile::tempdir().expect("session workgroup");
        let store = FakeStore::default();
        store
            .publish(&sess("live", SessionState::Active))
            .expect("seed live session");
        let rows = Arc::clone(&store.rows);
        let mut worker =
            SessionBrokerWorker::new(workgroup.path().to_path_buf(), "peer:recovery".to_string())
                .with_store(Box::new(store))
                .with_bus_root(blocker)
                .with_poll(Duration::from_millis(10));
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let task =
            tokio::spawn(
                async move { worker.run(ShutdownToken::from_receiver(shutdown_rx)).await },
            );

        tokio::time::sleep(Duration::from_millis(35)).await;
        assert!(
            !task.is_finished(),
            "Bus loss must not terminate the worker"
        );
        assert!(
            rows.lock().expect("rows mutex").contains_key("live"),
            "an unavailable action log must not look like an empty desired roster"
        );

        shutdown_tx.send(true).expect("request shutdown");
        tokio::time::timeout(Duration::from_millis(250), task)
            .await
            .expect("shutdown must interrupt the worker")
            .expect("worker task")
            .expect("worker result");
    }

    #[tokio::test]
    async fn late_and_replaced_bus_preserves_roster_skips_retained_and_applies_forward() {
        use mde_bus::hooks::config::Priority;

        let fixture = tempfile::tempdir().expect("session replacement fixture");
        let bus_root = fixture.path().join("bus");
        std::fs::write(&bus_root, b"block initial Bus open").expect("block Bus");
        let workgroup = tempfile::tempdir().expect("session workgroup");
        let store = FakeStore::default();
        let rows = Arc::clone(&store.rows);
        let authorizer = Arc::new(ActionAuthorizer::for_test(
            AUTH_KEY,
            fixture.path().join("auth"),
            AUTH_NOW,
        ));
        let mut worker = SessionBrokerWorker::new(
            workgroup.path().to_path_buf(),
            "peer:session-recovery".to_string(),
        )
        .with_store(Box::new(store))
        .with_bus_root(bus_root.clone())
        .with_authorizer(authorizer)
        .with_poll(Duration::from_millis(10));
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let task =
            tokio::spawn(
                async move { worker.run(ShutdownToken::from_receiver(shutdown_rx)).await },
            );

        let staged = fixture.path().join("late-bus");
        let late = Persist::open(staged.clone()).expect("stage late Bus");
        late.write(
            ACTION_TOPIC,
            Priority::Default,
            None,
            Some(&signed_body(
                &SessionRequest::Open {
                    id: "replacement-session".into(),
                    serving_peer: "peer:session-recovery".into(),
                    vm_id: "vm-replacement".into(),
                    client_peer: "peer:client".into(),
                    profile: None,
                },
                "late-open",
            )),
        )
        .expect("write late open");
        drop(late);
        std::fs::remove_file(&bus_root).expect("remove blocker");
        std::fs::rename(&staged, &bus_root).expect("install late Bus");

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if rows
                    .lock()
                    .expect("rows")
                    .contains_key("replacement-session")
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("late Bus action converges");

        let replacement_root = fixture.path().join("replacement-bus");
        let replacement = Persist::open(replacement_root.clone()).expect("replacement Bus");
        replacement
            .write(
                ACTION_TOPIC,
                Priority::Default,
                None,
                Some(&signed_body(
                    &SessionRequest::Close {
                        id: "replacement-session".into(),
                    },
                    "retained-close",
                )),
            )
            .expect("retained close");
        drop(replacement);
        std::fs::rename(&bus_root, fixture.path().join("retired-bus")).expect("retire prior Bus");
        std::fs::rename(&replacement_root, &bus_root).expect("install replacement Bus");
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert!(
            rows.lock()
                .expect("rows")
                .contains_key("replacement-session"),
            "retained replacement action must not close a live session"
        );

        Persist::open(bus_root.clone())
            .expect("open replacement")
            .write(
                ACTION_TOPIC,
                Priority::Default,
                None,
                Some(&signed_body(
                    &SessionRequest::Active {
                        id: "replacement-session".into(),
                    },
                    "forward-active",
                )),
            )
            .expect("forward active");
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if rows
                    .lock()
                    .expect("rows")
                    .get("replacement-session")
                    .is_some_and(|session| session.state == SessionState::Active)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("forward action converges");

        shutdown_tx.send(true).expect("shutdown");
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .expect("prompt shutdown")
            .expect("join")
            .expect("worker");
    }
}
