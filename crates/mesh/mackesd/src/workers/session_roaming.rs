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
//! - **Per-monitor different VMs (E12-10):** a layout may pin a *distinct* session
//!   (⇒ a distinct VM desktop) to each monitor via
//!   [`MonitorLayout::monitor_sessions`], keyed by a **replug-stable** [`MonitorId`].
//!   [`place_sessions`] / [`reconcile_roaming_placed`] resolve where every roamed
//!   session reopens on the arriving Workstation; a session whose pinned monitor is
//!   missing there falls back **deterministically to the primary monitor**, and a
//!   conflicted layout (one session on two monitors / two sessions on one monitor)
//!   is refused with a typed [`LayoutConflict`]. The pins are also validated
//!   against the **live session set** every placement: a monitor whose pinned
//!   session died is left honestly **unassigned**
//!   ([`ResolvedPlacement::unassigned_monitors`]) — never a fabricated session.
//!   Producers edit the mapping through [`MonitorLayout::assign`] (steal
//!   semantics keep it 1:1 by construction) / [`MonitorLayout::unassign`]. The
//!   default — no pins, and the shape every pre-E12-10 persisted layout
//!   deserializes to — stays single-session-fullscreen. The live
//!   two-monitors-two-VMs demo stays **hardware-gated** (it needs a real
//!   multi-head Workstation driving two VM desktops; nothing here fakes it).
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
//!   (production [`MeshSessionStore`]) — this worker never invents a parallel
//!   session model. The [`MonitorLayout`] rides a tiny companion [`LayoutStore`]
//!   on the same replicated workgroup root, preloaded on worker start so saved
//!   layouts survive a daemon restart before the next roam.
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

/// A **replug-stable** monitor identity: the EDID `vendor:model:serial` triple when
/// the panel exposes one, else the connector name (`DP-1`, `HDMI-A-2`, …).
///
/// [`MonitorAssignment::monitor`] keys geometry on the 0-based *enumeration index*,
/// which reshuffles when cables are replugged or the Workstation reboots. The
/// per-monitor session pins ([`MonitorLayout::monitor_sessions`]) key on this stable
/// id instead, so "the left DELL shows the build VM" survives a replug.
pub type MonitorId = String;

/// One per-monitor session pin (E12-10): the monitor with stable identity
/// `monitor_id` shows `session_id`'s VM desktop.
///
/// Distinct sessions on distinct monitors is how "two monitors show two different
/// VMs" is expressed; [`MonitorLayout::validate`] enforces the 1:1 shape.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MonitorSession {
    /// The replug-stable monitor identity (see [`MonitorId`]).
    pub monitor_id: MonitorId,
    /// The session whose desktop is pinned to that monitor (a broker
    /// [`SessionId`] — the VM rides the session, so distinct sessions ⇒ distinct
    /// VM surfaces).
    pub session_id: SessionId,
}

impl MonitorSession {
    /// Pin `session_id` to the monitor with stable id `monitor_id`.
    #[must_use]
    pub fn new(monitor_id: impl Into<MonitorId>, session_id: impl Into<SessionId>) -> Self {
        Self {
            monitor_id: monitor_id.into(),
            session_id: session_id.into(),
        }
    }
}

/// What an [`MonitorLayout::assign`] displaced to keep the pin set 1:1 — the
/// steal-semantics receipt.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Displaced {
    /// The monitor the assigned session was **stolen** from (it had been pinned
    /// elsewhere; that monitor is now unpinned).
    pub stolen_from: Option<MonitorId>,
    /// The session that was **replaced** on the target monitor (it is now
    /// unpinned — back to the primary-monitor default).
    pub replaced: Option<SessionId>,
}

impl Displaced {
    /// Whether the assign displaced nothing (a fresh pin, or a re-asserted one).
    #[must_use]
    pub const fn is_clean(&self) -> bool {
        self.stolen_from.is_none() && self.replaced.is_none()
    }
}

/// A typed conflict in a [`MonitorLayout`]'s per-monitor session pins — the layout
/// is refused (by the fold and by [`place_sessions`]) rather than silently
/// last-wins'd.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutConflict {
    /// The same session is pinned to two monitors — a session drives exactly one
    /// monitor surface per layout.
    SessionOnTwoMonitors {
        /// The doubly-pinned session.
        session_id: SessionId,
        /// The first monitor it was pinned to.
        first: MonitorId,
        /// The second (conflicting) monitor.
        second: MonitorId,
    },
    /// Two sessions are pinned to the same monitor — a monitor shows exactly one
    /// session's desktop.
    TwoSessionsOnOneMonitor {
        /// The doubly-assigned monitor.
        monitor_id: MonitorId,
        /// The first session pinned to it.
        first: SessionId,
        /// The second (conflicting) session.
        second: SessionId,
    },
}

impl std::fmt::Display for LayoutConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SessionOnTwoMonitors {
                session_id,
                first,
                second,
            } => write!(
                f,
                "session {session_id} is pinned to two monitors ({first} and {second}); \
                 a session drives exactly one monitor per layout"
            ),
            Self::TwoSessionsOnOneMonitor {
                monitor_id,
                first,
                second,
            } => write!(
                f,
                "monitor {monitor_id} is pinned to two sessions ({first} and {second}); \
                 a monitor shows exactly one session"
            ),
        }
    }
}

impl std::error::Error for LayoutConflict {}

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
    /// The per-monitor session pins (E12-10): which session's VM desktop shows on
    /// which **replug-stable** [`MonitorId`]. **Empty ⇒ single-session
    /// fullscreen** — the pre-E12-10 default (and the shape every previously
    /// persisted layout deserializes to): every session resolves to the primary
    /// monitor and the shell drives the focused one fullscreen. An empty pin set
    /// also serializes away, so a legacy layout round-trips byte-identical.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub monitor_sessions: Vec<MonitorSession>,
}

impl MonitorLayout {
    /// The empty layout (no surfaces mapped, no per-monitor pins —
    /// single-session-fullscreen).
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            assignments: Vec::new(),
            monitor_sessions: Vec::new(),
        }
    }

    /// Build a layout from a set of assignments (no per-monitor pins — the
    /// single-session default; pin with [`MonitorLayout::with_monitor_sessions`]).
    #[must_use]
    pub const fn new(assignments: Vec<MonitorAssignment>) -> Self {
        Self {
            assignments,
            monitor_sessions: Vec::new(),
        }
    }

    /// Pin per-monitor sessions onto this layout (builder — E12-10). Check the
    /// 1:1 shape with [`MonitorLayout::validate`].
    #[must_use]
    pub fn with_monitor_sessions(mut self, monitor_sessions: Vec<MonitorSession>) -> Self {
        self.monitor_sessions = monitor_sessions;
        self
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

    /// Whether this layout pins sessions per-monitor (E12-10) rather than the
    /// single-session-fullscreen default.
    #[must_use]
    pub fn is_per_monitor(&self) -> bool {
        !self.monitor_sessions.is_empty()
    }

    /// The session pinned to the monitor with stable id `monitor_id`, if any.
    #[must_use]
    pub fn session_on(&self, monitor_id: &str) -> Option<&SessionId> {
        self.monitor_sessions
            .iter()
            .find(|m| m.monitor_id == monitor_id)
            .map(|m| &m.session_id)
    }

    /// The monitor `session_id` is pinned to, if any.
    #[must_use]
    pub fn monitor_of(&self, session_id: &str) -> Option<&MonitorId> {
        self.monitor_sessions
            .iter()
            .find(|m| m.session_id == session_id)
            .map(|m| &m.monitor_id)
    }

    /// Pin `session_id` to `monitor_id` with **steal semantics** — the 1:1 shape
    /// is kept by construction, so an [`MonitorLayout::assign`]-edited layout
    /// always [`MonitorLayout::validate`]s:
    ///
    /// - a session already pinned to another monitor is **stolen** from it (the
    ///   pin moves; the old monitor is left unpinned), and
    /// - a session already showing on the target monitor is **replaced** (it
    ///   becomes unpinned — back to the primary-monitor default).
    ///
    /// What was displaced comes back as [`Displaced`], so a producer (the shell's
    /// layout editor, ahead of a [`RoamingRequest::SaveLayout`]) can surface the
    /// side effects. Re-asserting an existing pin is a no-op.
    pub fn assign(
        &mut self,
        monitor_id: impl Into<MonitorId>,
        session_id: impl Into<SessionId>,
    ) -> Displaced {
        let monitor_id = monitor_id.into();
        let session_id = session_id.into();
        let mut displaced = Displaced::default();
        // One pass: drop every pin sharing the session (steal) or the monitor
        // (replace) — also self-healing should a conflicted layout sneak in.
        self.monitor_sessions.retain(|pin| {
            if pin.session_id == session_id {
                if pin.monitor_id != monitor_id {
                    displaced.stolen_from = Some(pin.monitor_id.clone());
                }
                return false;
            }
            if pin.monitor_id == monitor_id {
                displaced.replaced = Some(pin.session_id.clone());
                return false;
            }
            true
        });
        self.monitor_sessions.push(MonitorSession {
            monitor_id,
            session_id,
        });
        displaced
    }

    /// Clear the pin on `monitor_id`, returning the session that was unpinned
    /// (now back to the primary-monitor default). `None` when the monitor had no
    /// pin.
    pub fn unassign(&mut self, monitor_id: &str) -> Option<SessionId> {
        let pos = self
            .monitor_sessions
            .iter()
            .position(|pin| pin.monitor_id == monitor_id)?;
        Some(self.monitor_sessions.remove(pos).session_id)
    }

    /// Validate the per-monitor session pins: every pin is 1:1. The same session
    /// on two monitors, or two sessions on one monitor (including a duplicated
    /// pin), is a typed [`LayoutConflict`]. The single-session default (no pins)
    /// is trivially valid.
    ///
    /// # Errors
    /// The first [`LayoutConflict`], scanning pins in order (deterministic).
    pub fn validate(&self) -> Result<(), LayoutConflict> {
        let mut by_monitor: BTreeMap<&str, &SessionId> = BTreeMap::new();
        let mut by_session: BTreeMap<&str, &MonitorId> = BTreeMap::new();
        for pin in &self.monitor_sessions {
            if let Some(prev) = by_monitor.insert(pin.monitor_id.as_str(), &pin.session_id) {
                return Err(LayoutConflict::TwoSessionsOnOneMonitor {
                    monitor_id: pin.monitor_id.clone(),
                    first: prev.clone(),
                    second: pin.session_id.clone(),
                });
            }
            if let Some(prev) = by_session.insert(pin.session_id.as_str(), &pin.monitor_id) {
                return Err(LayoutConflict::SessionOnTwoMonitors {
                    session_id: pin.session_id.clone(),
                    first: prev.clone(),
                    second: pin.monitor_id.clone(),
                });
            }
        }
        Ok(())
    }
}

/// The monitor inventory of a (potentially arriving) Workstation: the primary
/// monitor plus any others, all by **replug-stable** [`MonitorId`].
///
/// Rides the [`RoamingRequest::Arrive`] wire verb (optional — the pre-E12-10 shape
/// without it still parses) and anchors the deterministic placement fallback: a
/// session pinned to a monitor **not** present here reopens on `primary`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkstationMonitors {
    /// The primary monitor — the deterministic fallback target for sessions whose
    /// pinned monitor is missing, and where single-session-fullscreen desktops
    /// land.
    pub primary: MonitorId,
    /// The other present monitors. `primary` need not be repeated here.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub others: BTreeSet<MonitorId>,
}

impl WorkstationMonitors {
    /// An inventory of `primary` plus `others`.
    #[must_use]
    pub fn new(
        primary: impl Into<MonitorId>,
        others: impl IntoIterator<Item = impl Into<MonitorId>>,
    ) -> Self {
        Self {
            primary: primary.into(),
            others: others.into_iter().map(Into::into).collect(),
        }
    }

    /// A single-monitor Workstation.
    #[must_use]
    pub fn single(primary: impl Into<MonitorId>) -> Self {
        Self {
            primary: primary.into(),
            others: BTreeSet::new(),
        }
    }

    /// Whether the monitor with stable id `monitor_id` is present.
    #[must_use]
    pub fn is_present(&self, monitor_id: &str) -> bool {
        self.primary == monitor_id || self.others.contains(monitor_id)
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

/// Where each roamed session reopens on an arriving Workstation — the resolved
/// per-monitor placement ([`place_sessions`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedPlacement {
    /// session → the present monitor its desktop reopens on.
    pub on_monitor: BTreeMap<SessionId, MonitorId>,
    /// The sessions that fell back to the primary monitor because their pinned
    /// monitor is absent from the arriving Workstation.
    pub fell_back: BTreeSet<SessionId>,
    /// Present monitors whose **pin went stale** — the pinned session is gone
    /// from the live session set (died / closed / never existed). Each is left
    /// honestly **unassigned**: it shows no desktop, and no session is ever
    /// fabricated for it. Unpinned monitors are not listed (under the
    /// single-session default they are simply idle, not a stale mapping).
    pub unassigned_monitors: BTreeSet<MonitorId>,
}

impl ResolvedPlacement {
    /// The sessions placed on `monitor_id`, in id order (deterministic).
    #[must_use]
    pub fn sessions_on(&self, monitor_id: &str) -> Vec<&SessionId> {
        self.on_monitor
            .iter()
            .filter(|(_, m)| m.as_str() == monitor_id)
            .map(|(s, _)| s)
            .collect()
    }
}

/// Resolve where every publishable session in `sessions` reopens on a Workstation
/// with `monitors` present, honoring `layout`'s per-monitor pins (E12-10).
///
/// - A session **pinned to a present monitor** reopens on exactly that monitor —
///   two monitors show two different VMs.
/// - A session **pinned to a missing monitor** (the user roamed to a Workstation
///   with fewer monitors) falls back to the **primary monitor** — the
///   deterministic, documented fallback — and is recorded in
///   [`ResolvedPlacement::fell_back`].
/// - An **unpinned** session — and every session under the single-session default
///   (no pins) — resolves to the primary monitor; the shell drives the focused
///   one fullscreen.
/// - A pin whose **session is dead** (absent from `sessions`, or terminal) is
///   validated against the live session set and its present monitor is left
///   honestly **unassigned** ([`ResolvedPlacement::unassigned_monitors`]) —
///   never a fake session.
///
/// Terminal / non-publishable sessions are not placed. Deterministic (id-sorted).
/// Placement never changes *which* sessions reopen (that is
/// [`reconcile_roaming`]'s diff) nor their per-session [`DisconnectPolicy`] — it
/// only answers *where*.
///
/// # Errors
/// A [`LayoutConflict`] when `layout`'s pins are not 1:1 (validated first —
/// nothing is placed off a conflicted layout).
pub fn place_sessions(
    layout: &MonitorLayout,
    monitors: &WorkstationMonitors,
    sessions: &[VdiSession],
) -> Result<ResolvedPlacement, LayoutConflict> {
    layout.validate()?;
    let by_id: BTreeMap<&SessionId, &VdiSession> = sessions.iter().map(|s| (&s.id, s)).collect();
    let mut placement = ResolvedPlacement::default();
    for (id, session) in by_id {
        if !session.state.is_publishable() {
            continue;
        }
        let monitor = match layout.monitor_of(id) {
            Some(pinned) if monitors.is_present(pinned) => pinned.clone(),
            Some(_missing) => {
                placement.fell_back.insert(id.clone());
                monitors.primary.clone()
            }
            None => monitors.primary.clone(),
        };
        placement.on_monitor.insert(id.clone(), monitor);
    }
    // Validate the pins against the LIVE session set: a present monitor whose
    // pinned session was not placed (the session is dead — absent or terminal)
    // is left honestly unassigned; nothing is fabricated onto it. (A live,
    // publishable, pinned session on a present monitor is always placed exactly
    // there, so "not placed" ⇔ "the session is gone".)
    for pin in &layout.monitor_sessions {
        if monitors.is_present(&pin.monitor_id)
            && !placement.on_monitor.contains_key(&pin.session_id)
        {
            placement.unassigned_monitors.insert(pin.monitor_id.clone());
        }
    }
    Ok(placement)
}

/// The E12-10 per-monitor roaming reconcile result.
///
/// Bundles [`reconcile_roaming`]'s minimal action diff **plus** the resolved
/// monitor placement for the (already-roamed) desired sessions on the arriving
/// Workstation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedRoaming {
    /// The plane writes ([`reconcile_roaming`] — unchanged semantics).
    pub actions: Vec<RoamingAction>,
    /// Where each desired session reopens ([`place_sessions`]).
    pub placement: ResolvedPlacement,
}

/// Reconcile a roaming arrival **with** per-monitor placement (E12-10).
///
/// The same minimal [`RoamingAction`] diff as [`reconcile_roaming`] (each assigned
/// session reopens; per-session [`DisconnectPolicy`] handling is untouched), plus
/// where every reopened session lands on the arriving Workstation's `monitors` per
/// `layout` — a pinned-but-missing monitor falls back to the primary
/// ([`place_sessions`]).
///
/// # Errors
/// A [`LayoutConflict`] when `layout` is invalid — nothing is reconciled off a
/// conflicted layout.
pub fn reconcile_roaming_placed(
    desired: &[VdiSession],
    observed: &BTreeMap<SessionId, VdiSession>,
    layout: &MonitorLayout,
    monitors: &WorkstationMonitors,
) -> Result<PlacedRoaming, LayoutConflict> {
    let placement = place_sessions(layout, monitors, desired)?;
    Ok(PlacedRoaming {
        actions: reconcile_roaming(desired, observed),
        placement,
    })
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

/// One convergence tick's full decision: the [`RoamingAction`] plane writes plus
/// the per-arrival monitor placements (E12-10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoamingPlan {
    /// The plane writes, de-duplicated by session id, deterministic (id-sorted).
    pub actions: Vec<RoamingAction>,
    /// Per arriving Workstation: where each roamed session reopens — only for
    /// arrivals whose monitor inventory is known ([`RoamingRequest::Arrive`]
    /// carried [`WorkstationMonitors`]).
    pub placements: BTreeMap<NodeId, ResolvedPlacement>,
}

/// Compose one convergence tick's full [`RoamingPlan`] from the folded policy
/// inputs over the `observed` shared plane.
///
/// Roaming arrivals ([`reconcile_roaming`] over [`roam_to`]) + the per-VM disconnect
/// policy ([`on_disconnect`]) + node-loss holds ([`on_node_loss`]). De-duplicated by
/// session id (last decision wins: node-loss over disconnect-policy over roaming) so
/// each session gets at most one write per tick, and deterministic (id-sorted). Pure
/// — the worker's `converge` applies the result through the store.
///
/// **Per-monitor placement (E12-10):** `monitors` and `layouts` are keyed by the
/// peer they apply to (the fold re-keys a roaming user's layout onto the arriving
/// Workstation). For each arrival whose monitor inventory is known, the plan also
/// resolves *where* each roamed session reopens ([`reconcile_roaming_placed`]) —
/// the single-session default applies when no layout is persisted. The action diff
/// is identical either way: placement answers *where*, never *whether*.
#[must_use]
pub fn plan_roaming(
    arrivals: &BTreeMap<NodeId, NodeId>,
    monitors: &BTreeMap<NodeId, WorkstationMonitors>,
    layouts: &BTreeMap<NodeId, MonitorLayout>,
    policies: &BTreeMap<VmId, DisconnectPolicy>,
    observed: &BTreeMap<SessionId, VdiSession>,
    live: &BTreeSet<NodeId>,
    now_ms: u64,
) -> RoamingPlan {
    let mut by_id: BTreeMap<SessionId, RoamingAction> = BTreeMap::new();
    let mut placements: BTreeMap<NodeId, ResolvedPlacement> = BTreeMap::new();
    // 1. Roaming: for each arrival, roam the sessions driven from `from` onto
    //    `workstation` and reconcile them into the plane — with per-monitor
    //    placement when the arriving Workstation's monitors are known.
    for (from, workstation) in arrivals {
        let user_obs: BTreeMap<SessionId, VdiSession> = observed
            .iter()
            .filter(|(_, s)| &s.client_peer == from)
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let user_sessions: Vec<VdiSession> = user_obs.values().cloned().collect();
        let desired = roam_to(&user_sessions, workstation, now_ms);
        // The fold refuses conflicted layouts, so the placed reconcile only errs
        // on a caller-supplied invalid layout — the sessions still roam then,
        // just un-placed.
        let single_session = MonitorLayout::empty();
        let placed = monitors.get(workstation).and_then(|mons| {
            let layout = layouts.get(workstation).unwrap_or(&single_session);
            reconcile_roaming_placed(&desired, &user_obs, layout, mons).ok()
        });
        if let Some(p) = placed {
            placements.insert(workstation.clone(), p.placement);
            for action in p.actions {
                by_id.insert(action.session_id(), action);
            }
        } else {
            for action in reconcile_roaming(&desired, &user_obs) {
                by_id.insert(action.session_id(), action);
            }
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
    RoamingPlan {
        actions: by_id.into_values().collect(),
        placements,
    }
}

// ───────────────────────────── layout store seam ─────────────────────────────

/// The injectable per-peer [`MonitorLayout`] persistence seam.
///
/// Production wires [`MeshLayoutStore`]; the tests drive an in-memory fake so the
/// whole pipeline runs without the replicated workgroup root. Reuses the broker's
/// [`SessionStoreError`] as its error type, so the layout persist reports failures
/// identically to the session persist.
pub trait LayoutStore {
    /// Publish (create or update) `client_peer`'s layout in the shared plane.
    ///
    /// # Errors
    /// A [`SessionStoreError`] if the replicated store cannot be written.
    fn publish(
        &self,
        client_peer: &NodeId,
        layout: &MonitorLayout,
    ) -> Result<(), SessionStoreError>;

    /// List every persisted `(client_peer, layout)` pair.
    ///
    /// # Errors
    /// A [`SessionStoreError`] if the replicated store cannot be read.
    fn list(&self) -> Result<Vec<(NodeId, MonitorLayout)>, SessionStoreError>;

    /// Remove `client_peer`'s layout from the shared plane.
    ///
    /// # Errors
    /// A [`SessionStoreError`] if the replicated store cannot be updated.
    fn remove(&self, client_peer: &NodeId) -> Result<(), SessionStoreError>;
}

/// Production [`LayoutStore`]: the per-peer monitor-layout plane on the same
/// Syncthing-replicated coordination substrate the broker's [`MeshSessionStore`]
/// roams sessions on.
///
/// Each peer layout is one JSON row under the workgroup root. Writes are staged to
/// a temp file and atomically renamed into place, list output is deterministic, and
/// remove is idempotent. A future etcd lease-backed store can replace this trait
/// implementation without changing the roaming fold.
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

    fn dir(&self) -> PathBuf {
        self.workgroup_root
            .join("sessions")
            .join("vdi")
            .join("layouts")
    }

    fn path_for(&self, client_peer: &NodeId) -> PathBuf {
        self.dir()
            .join(format!("{}.json", safe_layout_file_stem(client_peer)))
    }
}

impl LayoutStore for MeshLayoutStore {
    fn publish(
        &self,
        client_peer: &NodeId,
        layout: &MonitorLayout,
    ) -> Result<(), SessionStoreError> {
        let dir = self.dir();
        std::fs::create_dir_all(&dir).map_err(|e| SessionStoreError::Failed {
            op: "publish",
            reason: format!("create {}: {e}", dir.display()),
        })?;
        let final_path = self.path_for(client_peer);
        let tmp = dir.join(format!(
            ".{}.{}.tmp",
            safe_layout_file_stem(client_peer),
            std::process::id()
        ));
        let row = PersistedLayout {
            client_peer: client_peer.clone(),
            layout: layout.clone(),
        };
        let body = serde_json::to_vec_pretty(&row).map_err(|e| SessionStoreError::Failed {
            op: "publish",
            reason: format!("serialize layout for {client_peer}: {e}"),
        })?;
        std::fs::write(&tmp, body).map_err(|e| SessionStoreError::Failed {
            op: "publish",
            reason: format!("write {}: {e}", tmp.display()),
        })?;
        std::fs::rename(&tmp, &final_path).map_err(|e| SessionStoreError::Failed {
            op: "publish",
            reason: format!("rename {} -> {}: {e}", tmp.display(), final_path.display()),
        })?;
        Ok(())
    }

    fn list(&self) -> Result<Vec<(NodeId, MonitorLayout)>, SessionStoreError> {
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
            let raw = std::fs::read_to_string(&path).map_err(|e| SessionStoreError::Failed {
                op: "list",
                reason: format!("read {}: {e}", path.display()),
            })?;
            let row: PersistedLayout =
                serde_json::from_str(&raw).map_err(|e| SessionStoreError::Failed {
                    op: "list",
                    reason: format!("parse {}: {e}", path.display()),
                })?;
            rows.push((row.client_peer, row.layout));
        }
        rows.sort_by(|(a, _), (b, _)| a.cmp(b));
        Ok(rows)
    }

    fn remove(&self, client_peer: &NodeId) -> Result<(), SessionStoreError> {
        let path = self.path_for(client_peer);
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct PersistedLayout {
    client_peer: NodeId,
    layout: MonitorLayout,
}

fn safe_layout_file_stem(id: &str) -> String {
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
        /// The arriving Workstation's monitor inventory (replug-stable ids), when
        /// the shell knows it — enables per-monitor placement (E12-10). Absent on
        /// the pre-E12-10 wire shape, which still parses (and roams un-placed).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        monitors: Option<WorkstationMonitors>,
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
    /// Latest known monitor inventory per Workstation (from
    /// [`RoamingRequest::Arrive`]).
    monitors: BTreeMap<NodeId, WorkstationMonitors>,
    /// Per-VM disconnect policy.
    policies: BTreeMap<VmId, DisconnectPolicy>,
    /// Per-peer persisted monitor layout (validated — a conflicted save is
    /// refused, never folded, never persisted).
    layouts: BTreeMap<NodeId, MonitorLayout>,
}

impl RoamingFold {
    /// Fold one drained request into the view (latest-wins by key).
    ///
    /// An [`RoamingRequest::Arrive`] also re-keys the roaming user's layout onto
    /// the arriving Workstation — desktops follow me, so the arrangement follows
    /// too (the origin peer keeps its copy for a roam back) — and records the
    /// arriving monitor inventory when the wire carried one. A
    /// [`RoamingRequest::SaveLayout`] with conflicted per-monitor pins is refused
    /// with the typed [`LayoutConflict`] (logged) rather than silently folded.
    fn apply(&mut self, req: RoamingRequest) {
        match req {
            RoamingRequest::Arrive {
                from,
                workstation,
                monitors,
            } => {
                if let Some(inventory) = monitors {
                    self.monitors.insert(workstation.clone(), inventory);
                }
                if let Some(layout) = self.layouts.get(&from).cloned() {
                    self.layouts.insert(workstation.clone(), layout);
                }
                self.arrivals.insert(from, workstation);
            }
            RoamingRequest::SetPolicy { vm_id, policy } => {
                self.policies.insert(vm_id, policy);
            }
            RoamingRequest::SaveLayout {
                client_peer,
                layout,
            } => match layout.validate() {
                Ok(()) => {
                    self.layouts.insert(client_peer, layout);
                }
                Err(conflict) => {
                    tracing::warn!(
                        peer = %client_peer,
                        error = %conflict,
                        "session_roaming: refused conflicted monitor layout"
                    );
                }
            },
        }
    }

    /// Seed the in-memory fold from persisted layout rows before replaying new bus
    /// requests. Invalid rows are refused just like conflicted [`SaveLayout`]
    /// requests: they are logged and never used for placement.
    fn load_layouts(
        &mut self,
        rows: impl IntoIterator<Item = (NodeId, MonitorLayout)>,
    ) -> Vec<(NodeId, LayoutConflict)> {
        let mut refused = Vec::new();
        for (client_peer, layout) in rows {
            match layout.validate() {
                Ok(()) => {
                    self.layouts.insert(client_peer, layout);
                }
                Err(conflict) => {
                    tracing::warn!(
                        peer = %client_peer,
                        error = %conflict,
                        "session_roaming: refused persisted conflicted monitor layout"
                    );
                    refused.push((client_peer, conflict));
                }
            }
        }
        refused
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

    /// Warm the roaming fold from the replicated layout plane. This is what makes a
    /// saved monitor layout survive a daemon restart before a later `Arrive`
    /// request re-keys that layout onto the new Workstation.
    fn preload_layouts(&self, fold: &mut RoamingFold) {
        match self.layouts.list() {
            Ok(rows) => {
                fold.load_layouts(rows);
            }
            Err(e @ SessionStoreError::IntegrationGated { .. }) => {
                tracing::info!(error = %e, "session_roaming: layout store integration-gated; starting without persisted layouts");
            }
            Err(e) => {
                tracing::warn!(error = %e, "session_roaming: layout store list failed; starting without persisted layouts");
            }
        }
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
        let plan = plan_roaming(
            &fold.arrivals,
            &fold.monitors,
            &fold.layouts,
            &fold.policies,
            &observed,
            &live,
            now_ms(),
        );
        for action in plan.actions {
            if let Err(e) = action.apply(self.store.as_ref()) {
                tracing::warn!(error = %e, "session_roaming: roaming action failed");
            }
        }
        // The leader's placement record (E12-10): which session reopens on which
        // monitor of the arriving Workstation — the shell-side renderer resolves
        // the same pure decision; this is its converge-time trace (incl. the
        // deterministic primary fallback for missing monitors and the honestly
        // unassigned monitors whose pinned session died).
        for (workstation, placement) in &plan.placements {
            tracing::info!(
                workstation = %workstation,
                on_monitor = ?placement.on_monitor,
                fell_back = ?placement.fell_back,
                unassigned = ?placement.unassigned_monitors,
                "session_roaming: per-monitor placement resolved"
            );
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
        self.preload_layouts(&mut fold);
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

    // ── E12-10: per-monitor session pins ──

    #[test]
    fn monitor_layout_old_json_shape_still_deserializes() {
        // The exact JSON shape the pre-E12-10 code persisted (no
        // `monitor_sessions` field) — a stored layout must keep deserializing.
        let old = r#"{"assignments":[{"monitor":0,"session_id":"s1","vm_id":"uuid-1","geometry":{"x":0,"y":0,"width":1920,"height":1080},"primary":true}]}"#;
        let layout: MonitorLayout = serde_json::from_str(old).expect("old shape deserializes");
        assert!(
            layout.monitor_sessions.is_empty(),
            "defaults to single-session-fullscreen"
        );
        assert!(!layout.is_per_monitor());
        assert_eq!(layout.surface_count(), 1);
        layout.validate().expect("the default is trivially valid");
        // And it re-serializes byte-identical (the empty pin set serializes
        // away), so legacy layouts stay stable through latest-wins folds.
        assert_eq!(serde_json::to_string(&layout).expect("serialize"), old);
    }

    #[test]
    fn per_monitor_layout_round_trips_and_looks_up() {
        let layout = MonitorLayout::empty().with_monitor_sessions(vec![
            MonitorSession::new("edid:DEL:U2720Q:1", "s1"),
            MonitorSession::new("edid:LEN:P27:2", "s2"),
        ]);
        assert!(layout.is_per_monitor());
        assert_eq!(layout.session_on("edid:LEN:P27:2"), Some(&"s2".to_string()));
        assert_eq!(
            layout.monitor_of("s1"),
            Some(&"edid:DEL:U2720Q:1".to_string())
        );
        assert!(layout.session_on("edid:GONE:9").is_none());
        assert!(layout.monitor_of("s9").is_none());
        layout.validate().expect("distinct 1:1 pins are valid");
        let json = serde_json::to_string(&layout).expect("serialize");
        let back: MonitorLayout = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, layout);
    }

    #[test]
    fn validate_rejects_one_session_on_two_monitors() {
        let layout = MonitorLayout::empty().with_monitor_sessions(vec![
            MonitorSession::new("edid:DEL:U2720Q:1", "s1"),
            MonitorSession::new("edid:LEN:P27:2", "s1"),
        ]);
        match layout.validate() {
            Err(LayoutConflict::SessionOnTwoMonitors {
                session_id,
                first,
                second,
            }) => {
                assert_eq!(session_id, "s1");
                assert_eq!(first, "edid:DEL:U2720Q:1");
                assert_eq!(second, "edid:LEN:P27:2");
            }
            other => panic!("expected SessionOnTwoMonitors, got {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_two_sessions_on_one_monitor() {
        let layout = MonitorLayout::empty().with_monitor_sessions(vec![
            MonitorSession::new("edid:DEL:U2720Q:1", "s1"),
            MonitorSession::new("edid:DEL:U2720Q:1", "s2"),
        ]);
        match layout.validate() {
            Err(LayoutConflict::TwoSessionsOnOneMonitor {
                monitor_id,
                first,
                second,
            }) => {
                assert_eq!(monitor_id, "edid:DEL:U2720Q:1");
                assert_eq!(first, "s1");
                assert_eq!(second, "s2");
            }
            other => panic!("expected TwoSessionsOnOneMonitor, got {other:?}"),
        }
        // The conflict is a real std error with a human-readable message.
        let err = layout.validate().unwrap_err();
        assert!(err.to_string().contains("edid:DEL:U2720Q:1"));
    }

    // ── E12-10: assign / steal (the mapping edits keep 1:1 by construction) ──

    #[test]
    fn assign_pins_a_session_and_reasserting_is_a_noop() {
        let mut layout = MonitorLayout::empty();
        let displaced = layout.assign("edid:DEL:U2720Q:1", "s1");
        assert!(displaced.is_clean(), "a fresh pin displaces nothing");
        assert_eq!(
            layout.session_on("edid:DEL:U2720Q:1"),
            Some(&"s1".to_string())
        );
        layout.validate().expect("assign keeps the layout valid");
        // Re-asserting the same pin changes nothing and displaces nothing.
        let again = layout.assign("edid:DEL:U2720Q:1", "s1");
        assert!(again.is_clean());
        assert_eq!(layout.monitor_sessions.len(), 1, "no duplicate pin");
        layout.validate().expect("still valid");
    }

    #[test]
    fn assign_steals_a_session_from_its_old_monitor() {
        // s1 shows on the DELL; assigning it to the LENOVO STEALS it — the pin
        // moves, the DELL is left unpinned, and the steal is reported.
        let mut layout = MonitorLayout::empty();
        layout.assign("edid:DEL:U2720Q:1", "s1");
        let displaced = layout.assign("edid:LEN:P27:2", "s1");
        assert_eq!(displaced.stolen_from, Some("edid:DEL:U2720Q:1".to_string()));
        assert_eq!(displaced.replaced, None);
        assert_eq!(layout.monitor_of("s1"), Some(&"edid:LEN:P27:2".to_string()));
        assert!(
            layout.session_on("edid:DEL:U2720Q:1").is_none(),
            "the old monitor is unpinned, not left with a stale copy"
        );
        layout.validate().expect("a steal keeps the layout 1:1");
    }

    #[test]
    fn assign_replaces_the_session_on_an_occupied_monitor() {
        // The DELL shows s1; assigning s2 there REPLACES s1 (which becomes
        // unpinned — back to the primary-monitor default), reported as such.
        let mut layout = MonitorLayout::empty();
        layout.assign("edid:DEL:U2720Q:1", "s1");
        let displaced = layout.assign("edid:DEL:U2720Q:1", "s2");
        assert_eq!(displaced.replaced, Some("s1".to_string()));
        assert_eq!(displaced.stolen_from, None);
        assert_eq!(
            layout.session_on("edid:DEL:U2720Q:1"),
            Some(&"s2".to_string())
        );
        assert!(layout.monitor_of("s1").is_none(), "s1 is unpinned");
        layout.validate().expect("a replace keeps the layout 1:1");
        // Steal + replace at once: s2 is on the DELL, s3 on the LENOVO; moving
        // s3 onto the DELL steals it from the LENOVO AND replaces s2.
        layout.assign("edid:LEN:P27:2", "s3");
        let both = layout.assign("edid:DEL:U2720Q:1", "s3");
        assert_eq!(both.stolen_from, Some("edid:LEN:P27:2".to_string()));
        assert_eq!(both.replaced, Some("s2".to_string()));
        assert_eq!(layout.monitor_sessions.len(), 1, "one pin remains");
        layout.validate().expect("still 1:1");
    }

    #[test]
    fn unassign_clears_a_monitor_pin() {
        let mut layout = MonitorLayout::empty();
        layout.assign("edid:DEL:U2720Q:1", "s1");
        assert_eq!(layout.unassign("edid:DEL:U2720Q:1"), Some("s1".to_string()));
        assert!(
            !layout.is_per_monitor(),
            "back to the single-session default"
        );
        assert_eq!(layout.unassign("edid:DEL:U2720Q:1"), None, "already clear");
    }

    // ── E12-10: placement (two monitors show two different VMs) ──

    #[test]
    fn place_sessions_puts_two_vms_on_two_monitors() {
        // The E12-10 acceptance: two monitors show two DIFFERENT VMs.
        let sessions = vec![
            sess("s1", "peer:ws", "peer:h1", SessionState::Active),
            sess("s2", "peer:ws", "peer:h2", SessionState::Active),
        ];
        assert_ne!(sessions[0].vm_id, sessions[1].vm_id, "distinct VMs");
        let layout = MonitorLayout::empty().with_monitor_sessions(vec![
            MonitorSession::new("edid:DEL:U2720Q:1", "s1"),
            MonitorSession::new("edid:LEN:P27:2", "s2"),
        ]);
        let monitors = WorkstationMonitors::new("edid:DEL:U2720Q:1", ["edid:LEN:P27:2"]);
        let placed = place_sessions(&layout, &monitors, &sessions).expect("valid layout places");
        assert_eq!(placed.on_monitor["s1"], "edid:DEL:U2720Q:1");
        assert_eq!(placed.on_monitor["s2"], "edid:LEN:P27:2");
        assert_eq!(placed.sessions_on("edid:DEL:U2720Q:1"), [&"s1".to_string()]);
        assert_eq!(placed.sessions_on("edid:LEN:P27:2"), [&"s2".to_string()]);
        assert!(placed.fell_back.is_empty(), "both monitors are present");
        assert!(placed.unassigned_monitors.is_empty(), "both pins are live");
    }

    #[test]
    fn place_sessions_falls_back_to_primary_on_missing_monitor() {
        // Roam to a Workstation with FEWER monitors: the session pinned to the
        // missing monitor deterministically reopens on the primary (documented
        // fallback), and the fallback is recorded.
        let sessions = vec![
            sess("s1", "peer:ws", "peer:h1", SessionState::Active),
            sess("s2", "peer:ws", "peer:h2", SessionState::Active),
        ];
        let layout = MonitorLayout::empty().with_monitor_sessions(vec![
            MonitorSession::new("edid:DEL:U2720Q:1", "s1"),
            MonitorSession::new("edid:LEN:P27:2", "s2"),
        ]);
        let single = WorkstationMonitors::single("edid:DEL:U2720Q:1");
        let placed = place_sessions(&layout, &single, &sessions).expect("places");
        assert_eq!(placed.on_monitor["s1"], "edid:DEL:U2720Q:1");
        assert_eq!(
            placed.on_monitor["s2"], "edid:DEL:U2720Q:1",
            "missing monitor → primary"
        );
        assert_eq!(placed.fell_back, ["s2".to_string()].into());
        // Deterministic: the same inputs place identically.
        assert_eq!(
            placed,
            place_sessions(&layout, &single, &sessions).expect("places")
        );
    }

    #[test]
    fn place_sessions_default_layout_is_single_session_on_primary() {
        // No pins (the default and every legacy layout): everything publishable
        // lands on the primary — single-session fullscreen. Terminal sessions
        // are not placed.
        let sessions = vec![
            sess("s1", "peer:ws", "peer:h1", SessionState::Active),
            sess("s2", "peer:ws", "peer:h2", SessionState::Closed), // terminal
        ];
        let monitors = WorkstationMonitors::new("edid:DEL:U2720Q:1", ["edid:LEN:P27:2"]);
        let placed = place_sessions(&MonitorLayout::empty(), &monitors, &sessions).expect("places");
        assert_eq!(
            placed.on_monitor.len(),
            1,
            "the terminal session is not placed"
        );
        assert_eq!(placed.on_monitor["s1"], "edid:DEL:U2720Q:1");
        assert!(placed.fell_back.is_empty(), "the default is not a fallback");
        // An UNPINNED session under a per-monitor layout also lands on primary.
        let layout = MonitorLayout::empty()
            .with_monitor_sessions(vec![MonitorSession::new("edid:LEN:P27:2", "s3")]);
        let more = vec![
            sess("s1", "peer:ws", "peer:h1", SessionState::Active),
            sess("s3", "peer:ws", "peer:h3", SessionState::Active),
        ];
        let placed = place_sessions(&layout, &monitors, &more).expect("places");
        assert_eq!(placed.on_monitor["s3"], "edid:LEN:P27:2");
        assert_eq!(
            placed.on_monitor["s1"], "edid:DEL:U2720Q:1",
            "unpinned → primary"
        );
        assert!(placed.fell_back.is_empty());
        assert!(
            placed.unassigned_monitors.is_empty(),
            "an unpinned monitor is idle, not a stale mapping"
        );
    }

    #[test]
    fn place_sessions_leaves_a_dead_sessions_monitor_unassigned() {
        // The pins are validated against the LIVE session set: s2's session died
        // (terminal) and s9's never existed — their present monitors are left
        // honestly UNASSIGNED, and no fake session is fabricated onto them.
        let layout = MonitorLayout::empty().with_monitor_sessions(vec![
            MonitorSession::new("edid:DEL:U2720Q:1", "s1"),
            MonitorSession::new("edid:LEN:P27:2", "s2"),
            MonitorSession::new("edid:AOC:24B:7", "s9"),
        ]);
        let monitors =
            WorkstationMonitors::new("edid:DEL:U2720Q:1", ["edid:LEN:P27:2", "edid:AOC:24B:7"]);
        let sessions = vec![
            sess("s1", "peer:ws", "peer:h1", SessionState::Active),
            sess("s2", "peer:ws", "peer:h2", SessionState::Closed), // died
                                                                    // s9: absent entirely
        ];
        let placed = place_sessions(&layout, &monitors, &sessions).expect("places");
        assert_eq!(placed.on_monitor.len(), 1, "only the live session places");
        assert_eq!(placed.on_monitor["s1"], "edid:DEL:U2720Q:1");
        assert!(
            !placed.on_monitor.contains_key("s2"),
            "the dead session is never resurrected"
        );
        assert_eq!(
            placed.unassigned_monitors,
            ["edid:LEN:P27:2".to_string(), "edid:AOC:24B:7".to_string()].into(),
            "monitors whose pinned session died are honestly unassigned"
        );
        assert!(placed.sessions_on("edid:LEN:P27:2").is_empty());
        assert!(
            placed.fell_back.is_empty(),
            "nothing fell back — it is gone"
        );
    }

    #[test]
    fn reconcile_roaming_placed_reopens_each_session_on_its_monitor() {
        // A user roams peer:old → peer:new (both monitors present): both sessions
        // Reopen — the exact same minimal diff as reconcile_roaming — and each
        // lands on its pinned monitor. Disconnect policy is untouched (it stays
        // the per-session concern of on_disconnect / plan_roaming step 2).
        let observed = roster_of(&[
            sess("s1", "peer:old", "peer:h1", SessionState::Disconnected),
            sess("s2", "peer:old", "peer:h2", SessionState::Active),
        ]);
        let user_sessions: Vec<VdiSession> = observed.values().cloned().collect();
        let desired = roam_to(&user_sessions, &"peer:new".to_string(), 700);
        let layout = MonitorLayout::empty().with_monitor_sessions(vec![
            MonitorSession::new("edid:DEL:U2720Q:1", "s1"),
            MonitorSession::new("edid:LEN:P27:2", "s2"),
        ]);
        let monitors = WorkstationMonitors::new("edid:DEL:U2720Q:1", ["edid:LEN:P27:2"]);
        let placed = reconcile_roaming_placed(&desired, &observed, &layout, &monitors)
            .expect("valid layout reconciles");
        assert_eq!(
            placed.actions,
            reconcile_roaming(&desired, &observed),
            "the action diff is unchanged — placement answers where, not whether"
        );
        assert_eq!(placed.actions.len(), 2, "both sessions reopen");
        assert_eq!(placed.placement.on_monitor["s1"], "edid:DEL:U2720Q:1");
        assert_eq!(placed.placement.on_monitor["s2"], "edid:LEN:P27:2");
        assert!(placed.placement.fell_back.is_empty());
        assert!(placed.placement.unassigned_monitors.is_empty());
    }

    #[test]
    fn reconcile_roaming_placed_dead_session_releases_and_unassigns() {
        // s2 died (Closed) before the user roamed: the reconcile path both
        // Releases it from the plane AND leaves its pinned monitor honestly
        // unassigned — the mapping is consulted, and no fake session appears.
        let observed = roster_of(&[
            sess("s1", "peer:old", "peer:h1", SessionState::Disconnected),
            sess("s2", "peer:old", "peer:h2", SessionState::Closed),
        ]);
        let user_sessions: Vec<VdiSession> = observed.values().cloned().collect();
        // roam_to drops the terminal session — only s1 is desired on peer:new.
        let desired = roam_to(&user_sessions, &"peer:new".to_string(), 700);
        let layout = MonitorLayout::empty().with_monitor_sessions(vec![
            MonitorSession::new("edid:DEL:U2720Q:1", "s1"),
            MonitorSession::new("edid:LEN:P27:2", "s2"),
        ]);
        let monitors = WorkstationMonitors::new("edid:DEL:U2720Q:1", ["edid:LEN:P27:2"]);
        let placed = reconcile_roaming_placed(&desired, &observed, &layout, &monitors)
            .expect("valid layout reconciles");
        assert!(
            placed
                .actions
                .contains(&RoamingAction::Release("s2".into())),
            "the dead session is released from the plane"
        );
        assert!(
            !placed.placement.on_monitor.contains_key("s2"),
            "never a fake session for the dead pin"
        );
        assert_eq!(
            placed.placement.unassigned_monitors,
            ["edid:LEN:P27:2".to_string()].into(),
            "its monitor falls back honestly — unassigned"
        );
        assert_eq!(placed.placement.on_monitor["s1"], "edid:DEL:U2720Q:1");
    }

    #[test]
    fn reconcile_roaming_placed_rejects_a_conflicted_layout() {
        let desired = vec![sess("s1", "peer:new", "peer:h", SessionState::Active)];
        let layout = MonitorLayout::empty().with_monitor_sessions(vec![
            MonitorSession::new("edid:DEL:U2720Q:1", "s1"),
            MonitorSession::new("edid:LEN:P27:2", "s1"),
        ]);
        let err = reconcile_roaming_placed(
            &desired,
            &BTreeMap::new(),
            &layout,
            &WorkstationMonitors::single("edid:DEL:U2720Q:1"),
        )
        .expect_err("a conflicted layout must not reconcile");
        assert!(matches!(err, LayoutConflict::SessionOnTwoMonitors { .. }));
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

        let plan = plan_roaming(
            &arrivals,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &policies,
            &observed,
            &live,
            42,
        );
        assert!(
            plan.placements.is_empty(),
            "no monitor inventory ⇒ no placement"
        );
        let out = plan.actions;
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
        let plan = plan_roaming(
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
            &observed,
            &live,
            42,
        );
        assert!(
            plan.actions.is_empty(),
            "KeepRunning holds without a plane write"
        );
        assert!(plan.placements.is_empty());
    }

    #[test]
    fn plan_roaming_resolves_per_monitor_placement_for_an_arrival() {
        // A user with two per-monitor sessions roams peer:old → peer:new, but
        // peer:new has only the primary monitor: both sessions still roam
        // (Reopen), s1 lands on its pinned monitor, s2 falls back to the primary.
        let observed = roster_of(&[
            sess("s1", "peer:old", "peer:live", SessionState::Disconnected),
            sess("s2", "peer:old", "peer:live", SessionState::Disconnected),
        ]);
        let arrivals: BTreeMap<NodeId, NodeId> =
            [("peer:old".to_string(), "peer:new".to_string())].into();
        let monitors: BTreeMap<NodeId, WorkstationMonitors> = [(
            "peer:new".to_string(),
            WorkstationMonitors::single("edid:DEL:U2720Q:1"),
        )]
        .into();
        // Keyed by the arriving peer — exactly how the fold re-keys a roaming
        // user's layout on Arrive.
        let layouts: BTreeMap<NodeId, MonitorLayout> = [(
            "peer:new".to_string(),
            MonitorLayout::empty().with_monitor_sessions(vec![
                MonitorSession::new("edid:DEL:U2720Q:1", "s1"),
                MonitorSession::new("edid:LEN:P27:2", "s2"),
            ]),
        )]
        .into();
        let live = live_set(&["live"]);

        let plan = plan_roaming(
            &arrivals,
            &monitors,
            &layouts,
            &BTreeMap::new(),
            &observed,
            &live,
            42,
        );
        assert_eq!(plan.actions.len(), 2, "both sessions roam regardless");
        assert!(plan
            .actions
            .iter()
            .all(|a| matches!(a, RoamingAction::Reopen(s) if s.client_peer == "peer:new")));
        let placement = &plan.placements["peer:new"];
        assert_eq!(placement.on_monitor["s1"], "edid:DEL:U2720Q:1");
        assert_eq!(
            placement.on_monitor["s2"], "edid:DEL:U2720Q:1",
            "pinned monitor missing on arrival ⇒ deterministic primary fallback"
        );
        assert_eq!(placement.fell_back, ["s2".to_string()].into());
    }

    // ── store seams ──

    #[test]
    fn mesh_layout_store_round_trips_sorted_records_and_removes_idempotently() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store = MeshLayoutStore::new(tmp.path().to_path_buf());
        assert_eq!(
            store.list().expect("missing layout dir lists empty"),
            Vec::<(NodeId, MonitorLayout)>::new()
        );

        let peer_b = "peer:b/with space".to_string();
        let peer_a = "peer:a".to_string();
        let layout_b = MonitorLayout::empty()
            .with_monitor_sessions(vec![MonitorSession::new("edid:DEL:U2720Q:B", "s-b")]);
        let layout_a = MonitorLayout::empty()
            .with_monitor_sessions(vec![MonitorSession::new("index:0", "s-a")]);

        store.publish(&peer_b, &layout_b).expect("publish b");
        store.publish(&peer_a, &layout_a).expect("publish a");

        assert_eq!(
            store.list().expect("list"),
            vec![
                (peer_a.clone(), layout_a.clone()),
                (peer_b.clone(), layout_b)
            ]
        );
        assert!(
            store.dir().join("peer:b_2fwith_20space.json").exists(),
            "peer ids are encoded into one safe path component"
        );

        let layout_a2 = MonitorLayout::empty()
            .with_monitor_sessions(vec![MonitorSession::new("index:1", "s-a2")]);
        store.publish(&peer_a, &layout_a2).expect("replace a");
        assert_eq!(
            store.list().expect("list after replace")[0],
            (peer_a.clone(), layout_a2)
        );

        store.remove(&peer_b).expect("remove b");
        store.remove(&peer_b).expect("remove b again");
        assert_eq!(store.list().expect("list after remove").len(), 1);
    }

    #[test]
    fn safe_layout_file_stem_encodes_path_separators_and_spaces() {
        assert_eq!(safe_layout_file_stem(""), "_");
        assert_eq!(safe_layout_file_stem("peer:ok-1"), "peer:ok-1");
        assert_eq!(safe_layout_file_stem("a/b c"), "a_2fb_20c");
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
    fn fake_layout_store_round_trips_a_per_monitor_layout() {
        // The E12-10 persistence round-trip: the per-monitor pins survive the
        // per-peer layout seam intact.
        let store = FakeLayoutStore::default();
        let peer = "peer:c".to_string();
        let mut layout = MonitorLayout::empty();
        layout.assign("edid:DEL:U2720Q:1", "s1");
        layout.assign("edid:LEN:P27:2", "s2");
        store.publish(&peer, &layout).unwrap();
        let listed = store.list().unwrap();
        assert_eq!(listed, vec![(peer.clone(), layout)]);
        store.remove(&peer).unwrap();
        assert!(store.list().unwrap().is_empty());
    }

    // ── parsing + topic + worker name ──

    #[test]
    fn parse_request_round_trips_ops() {
        // The pre-E12-10 wire shape (no `monitors`) MUST keep parsing.
        let arrive = parse_request(r#"{"op":"arrive","from":"peer:old","workstation":"peer:new"}"#)
            .expect("arrive parses");
        assert_eq!(
            arrive,
            RoamingRequest::Arrive {
                from: "peer:old".into(),
                workstation: "peer:new".into(),
                monitors: None,
            }
        );
        // And it re-serializes without the absent field — old wire shape stable.
        assert_eq!(
            serde_json::to_string(&arrive).expect("serialize"),
            r#"{"op":"arrive","from":"peer:old","workstation":"peer:new"}"#
        );
        // The E12-10 shape carries the arriving monitor inventory.
        let placed_arrive = parse_request(
            r#"{"op":"arrive","from":"peer:old","workstation":"peer:new","monitors":{"primary":"edid:DEL:U2720Q:1","others":["edid:LEN:P27:2"]}}"#,
        )
        .expect("arrive with monitors parses");
        assert_eq!(
            placed_arrive,
            RoamingRequest::Arrive {
                from: "peer:old".into(),
                workstation: "peer:new".into(),
                monitors: Some(WorkstationMonitors::new(
                    "edid:DEL:U2720Q:1",
                    ["edid:LEN:P27:2"]
                )),
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

    // ── fold (E12-10: layout follows the user; conflicted saves refused) ──

    #[test]
    fn fold_carries_the_layout_and_monitors_on_arrive() {
        let mut fold = RoamingFold::default();
        let layout = MonitorLayout::empty()
            .with_monitor_sessions(vec![MonitorSession::new("edid:DEL:U2720Q:1", "s1")]);
        fold.apply(RoamingRequest::SaveLayout {
            client_peer: "peer:old".into(),
            layout: layout.clone(),
        });
        fold.apply(RoamingRequest::Arrive {
            from: "peer:old".into(),
            workstation: "peer:new".into(),
            monitors: Some(WorkstationMonitors::single("edid:DEL:U2720Q:1")),
        });
        assert_eq!(
            fold.layouts.get("peer:new"),
            Some(&layout),
            "the layout follows the user to the arriving Workstation"
        );
        assert_eq!(
            fold.layouts.get("peer:old"),
            Some(&layout),
            "the origin keeps its arrangement for a roam back"
        );
        assert_eq!(
            fold.monitors.get("peer:new"),
            Some(&WorkstationMonitors::single("edid:DEL:U2720Q:1"))
        );
    }

    #[test]
    fn fold_refuses_a_conflicted_save_layout() {
        let mut fold = RoamingFold::default();
        fold.apply(RoamingRequest::SaveLayout {
            client_peer: "peer:c".into(),
            layout: MonitorLayout::empty().with_monitor_sessions(vec![
                MonitorSession::new("edid:DEL:U2720Q:1", "s1"),
                MonitorSession::new("edid:DEL:U2720Q:1", "s2"),
            ]),
        });
        assert!(
            fold.layouts.is_empty(),
            "a conflicted layout is never folded (and thus never persisted)"
        );
    }

    #[test]
    fn fold_preloads_persisted_layouts_and_refuses_conflicts() {
        let mut fold = RoamingFold::default();
        let valid = MonitorLayout::empty()
            .with_monitor_sessions(vec![MonitorSession::new("edid:DEL:U2720Q:1", "s1")]);
        let invalid = MonitorLayout::empty().with_monitor_sessions(vec![
            MonitorSession::new("edid:LEN:P27:2", "s2"),
            MonitorSession::new("edid:LEN:P27:2", "s3"),
        ]);

        let refused = fold.load_layouts([
            ("peer:old".to_string(), valid.clone()),
            ("peer:bad".to_string(), invalid),
        ]);

        assert_eq!(fold.layouts.get("peer:old"), Some(&valid));
        assert!(
            !fold.layouts.contains_key("peer:bad"),
            "a conflicted persisted row is never folded"
        );
        assert_eq!(refused.len(), 1);
        assert_eq!(refused[0].0, "peer:bad");
        assert!(matches!(
            refused[0].1,
            LayoutConflict::TwoSessionsOnOneMonitor { .. }
        ));
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
        // A process-wide sequence keeps parallel tests seeded in the same
        // millisecond from colliding on the temp dir.
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("mde-sr-{}-{seq}-{}", now_ms(), reqs.len()));
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
                monitors: None,
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
    async fn worker_roams_two_monitor_sessions_and_the_layout_follows() {
        // E12-10 end-to-end: two sessions (two DIFFERENT VMs) pinned to two
        // monitors on peer:old. The user arrives at single-monitor peer:new —
        // both sessions roam Active onto peer:new through the reused store
        // (placement falls back deterministically, never drops a desktop), and
        // the per-monitor layout follows the user through the layout seam.
        let layout = MonitorLayout::empty().with_monitor_sessions(vec![
            MonitorSession::new("edid:DEL:U2720Q:1", "s1"),
            MonitorSession::new("edid:LEN:P27:2", "s2"),
        ]);
        let bus = seed_bus(&[
            RoamingRequest::SaveLayout {
                client_peer: "peer:old".into(),
                layout: layout.clone(),
            },
            RoamingRequest::Arrive {
                from: "peer:old".into(),
                workstation: "peer:new".into(),
                monitors: Some(WorkstationMonitors::single("edid:DEL:U2720Q:1")),
            },
        ]);
        let wg = std::env::temp_dir().join(format!("mde-sr-wg2-{}", now_ms()));
        std::fs::create_dir_all(&wg).expect("mk workgroup");

        let store = FakeStore::default();
        store
            .publish(&sess(
                "s1",
                "peer:old",
                "peer:h1",
                SessionState::Disconnected,
            ))
            .unwrap();
        store
            .publish(&sess(
                "s2",
                "peer:old",
                "peer:h2",
                SessionState::Disconnected,
            ))
            .unwrap();
        let rows = store.rows.clone();
        let layouts = FakeLayoutStore::default();
        let layout_rows = layouts.rows.clone();

        let w = SessionRoamingWorker::new(wg.clone(), "peer:a".to_string())
            .with_store(Box::new(store))
            .with_layout_store(Box::new(layouts))
            .with_live_nodes(Box::new(FakeLiveNodes(live_set(&["h1", "h2"]))))
            .with_bus_root(bus.clone());

        let mut cursor = None;
        let mut fold = RoamingFold::default();
        drain(&bus, &mut cursor, &mut fold);
        w.converge(&fold);

        let published = rows.lock().expect("rows mutex");
        assert_eq!(published.len(), 2, "both per-monitor sessions roamed");
        for id in ["s1", "s2"] {
            assert_eq!(published[id].client_peer, "peer:new", "desktop followed");
            assert_eq!(published[id].state, SessionState::Active, "reconnected");
        }
        assert_ne!(
            published["s1"].vm_id, published["s2"].vm_id,
            "two monitors, two different VMs"
        );
        drop(published);
        let saved = layout_rows.lock().expect("layout mutex");
        assert_eq!(
            saved.get("peer:new"),
            Some(&layout),
            "the per-monitor layout followed the user to the arriving peer"
        );
        drop(saved);

        let _ = std::fs::remove_dir_all(&bus);
        let _ = std::fs::remove_dir_all(&wg);
    }

    #[tokio::test]
    async fn worker_preloads_persisted_layouts_before_roaming_arrivals() {
        // E12-8 restart proof: the saved layout already exists in the layout
        // plane, but this process only sees a later Arrive request. Preloading
        // layouts before the bus fold lets the layout still follow the user.
        let layout = MonitorLayout::empty().with_monitor_sessions(vec![
            MonitorSession::new("edid:DEL:U2720Q:1", "s1"),
            MonitorSession::new("edid:LEN:P27:2", "s2"),
        ]);
        let bus = seed_bus(&[RoamingRequest::Arrive {
            from: "peer:old".into(),
            workstation: "peer:new".into(),
            monitors: Some(WorkstationMonitors::new(
                "edid:DEL:U2720Q:1",
                ["edid:LEN:P27:2"],
            )),
        }]);
        let wg = std::env::temp_dir().join(format!("mde-sr-wg3-{}", now_ms()));
        std::fs::create_dir_all(&wg).expect("mk workgroup");

        let store = FakeStore::default();
        for (id, host) in [("s1", "peer:h1"), ("s2", "peer:h2")] {
            store
                .publish(&sess(id, "peer:old", host, SessionState::Disconnected))
                .unwrap();
        }
        let rows = store.rows.clone();
        let layouts = FakeLayoutStore::default();
        layouts
            .publish(&"peer:old".to_string(), &layout)
            .expect("seed persisted layout");
        let layout_rows = layouts.rows.clone();

        let w = SessionRoamingWorker::new(wg.clone(), "peer:a".to_string())
            .with_store(Box::new(store))
            .with_layout_store(Box::new(layouts))
            .with_live_nodes(Box::new(FakeLiveNodes(live_set(&["h1", "h2"]))))
            .with_bus_root(bus.clone());

        let mut cursor = None;
        let mut fold = RoamingFold::default();
        w.preload_layouts(&mut fold);
        drain(&bus, &mut cursor, &mut fold);
        w.converge(&fold);

        let published = rows.lock().expect("rows mutex");
        assert_eq!(published["s1"].client_peer, "peer:new");
        assert_eq!(published["s2"].client_peer, "peer:new");
        assert_eq!(published["s1"].state, SessionState::Active);
        assert_eq!(published["s2"].state, SessionState::Active);
        drop(published);

        let saved = layout_rows.lock().expect("layout mutex");
        assert_eq!(
            saved.get("peer:new"),
            Some(&layout),
            "the persisted old-peer layout followed the user after preload"
        );
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
