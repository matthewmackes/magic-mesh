//! `host_state` worker (E12-19, Quasar host controls; design
//! `docs/design/quasar-host-controls.md`, locks 1/9/10 + the safety interlocks).
//!
//! Under Quasar the **shell owns the seat hardware in-process** (lock 1) — audio,
//! Bluetooth, displays, power. This worker is the mesh side of that split: it
//!
//! * **mirrors** this node's seat snapshot (which the shell publishes locally) out
//!   to the replicated `state/host/<node>/seat` topic, so every peer's Workbench —
//!   and the remote mixer / display / power views — sees this node's hardware; and
//! * **executes remote typed verbs** arriving on `action/host/<node>/*` (volume /
//!   mute / Bluetooth / display / power) behind the **allowlist + the safety
//!   interlocks** (lock 10): the verb set IS the allowlist (§9 — no generic
//!   "run this on the peer"); a display verb that would black a peer's last console
//!   is refused typed; reboot/poweroff on the etcd leader demands the confirm flag;
//!   and destructive verbs are two-phase (propose → confirm within a TTL).
//!
//! An **approved** verb is forwarded to the shell's local apply lane
//! ([`APPLY_TOPIC`]) — the shell owns the hardware (lock 1), so the worker never
//! touches a device itself; it is the mesh authorization gate in front of the
//! shell's seat. A **refused** verb writes a typed reason to the requester's result
//! lane. The whole authorization is a pure fold ([`authorize`]) unit-tested
//! headless; the run loop is thin Bus I/O over that fold.
//!
//! **§6 boundary:** this mesh worker mirrors a **JSON snapshot** (local serde
//! structs) rather than depending on the desktop `mde-seat` crate — the same
//! JSON-boundary the toast / clipboard / chat lanes use, so mesh-substrate grows no
//! desktop dependency. The interlock *semantics* mirror `mde_seat`'s
//! `DisplayLayout::guard_disable` / `PowerVerb::needs_confirm`; the code is a small
//! pure reimplementation over the mirrored JSON.
//!
//! **Live seam (integration-gated):** the shell publishing its snapshot on
//! [`LOCAL_SNAPSHOT_TOPIC`] and consuming [`APPLY_TOPIC`] is the shell↔worker wire
//! the DRM-seat integration completes; the worker faithfully mirrors + authorizes
//! whatever appears on those lanes, so the mesh half is complete and testable now.

#![cfg(feature = "async-services")]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use serde::{Deserialize, Serialize};

use super::{ShutdownToken, Worker};
use crate::leader::read_current_lease;

/// The local topic the shell publishes its seat snapshot on (this node only).
///
/// The worker reads it and republishes to the replicated per-node mirror. The live
/// publish (`--no-broker`) is the integration-gated shell↔worker seam.
pub const LOCAL_SNAPSHOT_TOPIC: &str = "state/host/local/seat";

/// The local lane the worker forwards an **approved** remote verb onto; the shell
/// consumes it and applies it through its in-process seat (lock 1).
pub const APPLY_TOPIC: &str = "action/host/local/apply";

/// The lease lockfile the fs leader election renews (mirrors `leader_election`).
const LEADER_LOCK: &str = ".mackesd-leader.lock";

/// Two-phase confirm TTL: a `propose` for a destructive verb must be followed by a
/// matching `confirm` within this window, else it lapses (interlock 3).
pub const CONFIRM_TTL: Duration = Duration::from_secs(30);

/// Poll cadence — responsive to a remote control action without hammering the Bus.
pub const POLL: Duration = Duration::from_secs(2);

/// The replicated per-node seat-mirror topic other peers read.
#[must_use]
pub fn mirror_topic(node: &str) -> String {
    format!("state/host/{node}/seat")
}

/// The per-node remote-verb request topic peers publish onto.
#[must_use]
pub fn action_topic(node: &str) -> String {
    format!("action/host/{node}/verb")
}

/// The per-node result lane where a verb's typed outcome (applied / refused) lands.
#[must_use]
pub fn result_topic(node: &str) -> String {
    format!("state/host/{node}/verb-result")
}

// ─────────────────────────── the mirrored snapshot (JSON boundary) ───────────

/// One display in the mirror — just enough for the last-console interlock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MirrorDisplay {
    /// The monitor identity (the remote display verb targets this).
    pub id: String,
    /// Whether it is currently lit.
    pub enabled: bool,
}

/// The JSON seat snapshot the shell mirrors — the subset the mesh views + the
/// interlocks need. A local serde struct (not an `mde-seat` dep — §6 boundary).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeatMirror {
    /// Master output volume 0–100.
    #[serde(default)]
    pub volume: u8,
    /// Master output muted.
    #[serde(default)]
    pub muted: bool,
    /// Bluetooth adapter powered.
    #[serde(default)]
    pub bluetooth_powered: bool,
    /// Displays (id + lit), for the never-black-the-last-console guard.
    #[serde(default)]
    pub displays: Vec<MirrorDisplay>,
    /// Battery percentages (multi + peripherals), for the remote Power view.
    #[serde(default)]
    pub batteries: Vec<u8>,
}

impl SeatMirror {
    /// How many displays are currently lit — the count the last-console guard reads.
    #[must_use]
    pub fn lit_console_count(&self) -> usize {
        self.displays.iter().filter(|d| d.enabled).count()
    }

    /// Whether a display with this id is currently lit.
    #[must_use]
    pub fn is_lit(&self, id: &str) -> bool {
        self.displays.iter().any(|d| d.id == id && d.enabled)
    }
}

// ─────────────────────────── the typed verb allowlist (§9) ───────────────────

/// A logind power action a remote verb may request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PowerAction {
    /// Lock the seat (benign — never confirm-gated).
    Lock,
    /// Suspend to RAM.
    Suspend,
    /// Reboot the host (destructive — two-phase + leader-aware).
    Reboot,
    /// Power the host off (destructive — two-phase + leader-aware).
    PowerOff,
}

impl PowerAction {
    /// Whether this action takes the host down (destructive → two-phase confirm,
    /// interlock 3; and leader-aware, interlock 2). Lock/Suspend are benign.
    #[must_use]
    pub const fn is_destructive(self) -> bool {
        matches!(self, Self::Reboot | Self::PowerOff)
    }
}

/// A typed remote host verb — the allowlist IS this enum (§9: no generic exec).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verb", rename_all = "lowercase")]
pub enum HostVerb {
    /// Set the master output volume (0–100).
    Volume {
        /// Target strip id (`"master"` or a `PipeWire` node id).
        strip: String,
        /// The new level 0–100.
        level: u8,
    },
    /// Set the master (or a strip's) mute.
    Mute {
        /// Target strip id.
        strip: String,
        /// The new mute state.
        muted: bool,
    },
    /// Power a Bluetooth adapter's radio on/off.
    Bluetooth {
        /// The adapter object path.
        adapter: String,
        /// The new power state.
        on: bool,
    },
    /// Enable/disable a display output (guarded: never black the last console).
    Display {
        /// The monitor id.
        monitor: String,
        /// The desired lit state.
        enable: bool,
    },
    /// A logind power action (leader-aware + two-phase for the destructive ones).
    Power {
        /// The action.
        action: PowerAction,
        /// The explicit confirm flag the caller set (interlock 2/3).
        #[serde(default)]
        confirm: bool,
    },
}

impl HostVerb {
    /// A short stable key identifying the verb kind — the two-phase confirm token
    /// keys off this so a `propose`/`confirm` pair for the *same* action matches.
    #[must_use]
    pub fn kind_key(&self) -> String {
        match self {
            Self::Volume { strip, .. } => format!("volume:{strip}"),
            Self::Mute { strip, .. } => format!("mute:{strip}"),
            Self::Bluetooth { adapter, .. } => format!("bluetooth:{adapter}"),
            Self::Display { monitor, .. } => format!("display:{monitor}"),
            Self::Power { action, .. } => format!("power:{action:?}"),
        }
    }

    /// Whether this verb is destructive (needs the two-phase propose→confirm).
    #[must_use]
    pub const fn is_destructive(&self) -> bool {
        matches!(self, Self::Power { action, .. } if action.is_destructive())
    }
}

/// The request envelope a peer publishes on the action lane: the verb plus the
/// two-phase phase and the requester's identity (for the result lane + audit).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerbRequest {
    /// The typed verb.
    #[serde(flatten)]
    pub verb: HostVerb,
    /// `"propose"` (arm the two-phase confirm) or `"confirm"` (execute). Benign
    /// verbs ignore it and apply immediately. Defaults to `confirm`.
    #[serde(default = "default_phase")]
    pub phase: Phase,
    /// The requesting peer (echoed on the result lane).
    #[serde(default)]
    pub requester: String,
}

const fn default_phase() -> Phase {
    Phase::Confirm
}

/// The two-phase confirm phase of a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    /// Arm a destructive verb — records a pending token, applies nothing yet.
    Propose,
    /// Execute — a benign verb, or a destructive one whose propose is still live.
    Confirm,
}

// ─────────────────────────── the authorization fold (pure) ───────────────────

/// Why a remote verb was refused — a typed reason echoed on the result lane
/// (never a silent drop; the requester learns exactly which interlock fired).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "refusal", rename_all = "kebab-case")]
pub enum Refusal {
    /// The body wasn't a verb in the allowlist (§9).
    NotAllowlisted {
        /// The offending detail.
        detail: String,
    },
    /// Disabling this output would leave no lit console (interlock 1).
    WouldBlackLastConsole {
        /// The monitor id.
        monitor: String,
    },
    /// Reboot/poweroff on the etcd leader without the confirm flag (interlock 2).
    LeaderNeedsConfirm {
        /// The action.
        action: PowerAction,
    },
    /// A destructive verb's `confirm` arrived with no live `propose` (interlock 3).
    NoLiveProposal {
        /// The verb kind key.
        kind: String,
    },
}

/// The decision the fold reaches for one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Forward this verb to the shell's apply lane (approved).
    Apply(HostVerb),
    /// A destructive verb was armed — await its `confirm` within [`CONFIRM_TTL`].
    Proposed {
        /// The verb kind key that is now pending.
        kind: String,
    },
    /// Refused — echo this typed reason on the result lane.
    Refuse(Refusal),
}

/// The live two-phase confirm state: kind-key → the monotonic deadline (ms) its
/// `propose` lapses at. Pure — time is injected, so the whole gate is unit-tested.
#[derive(Debug, Clone, Default)]
pub struct ConfirmGate {
    pending: BTreeMap<String, u64>,
}

impl ConfirmGate {
    /// Arm a destructive verb: record its deadline (`now + ttl`).
    pub fn propose(&mut self, kind: &str, now_ms: u64, ttl: Duration) {
        let ttl_ms = u64::try_from(ttl.as_millis()).unwrap_or(u64::MAX);
        self.pending
            .insert(kind.to_owned(), now_ms.saturating_add(ttl_ms));
    }

    /// Consume a still-live proposal for `kind`: `true` if one was armed and hasn't
    /// lapsed (removing it), `false` otherwise. Expired entries are pruned.
    pub fn take_live(&mut self, kind: &str, now_ms: u64) -> bool {
        // `remove` runs (consuming the proposal) even in the `matches!` scrutinee.
        matches!(self.pending.remove(kind), Some(deadline) if deadline >= now_ms)
    }

    /// Drop every lapsed proposal (housekeeping; `take_live` also prunes on hit).
    pub fn prune(&mut self, now_ms: u64) {
        self.pending.retain(|_, deadline| *deadline >= now_ms);
    }

    /// How many proposals are currently armed (test/inspection).
    #[must_use]
    pub fn armed(&self) -> usize {
        self.pending.len()
    }
}

/// Authorize one remote verb against the mirror + the interlocks (the pure gate).
///
/// Order: allowlist is already enforced by parsing into [`HostVerb`]; here we apply
/// (1) never-black-the-last-console for a display-disable, (2) leader-aware confirm
/// for a destructive power verb, then (3) the two-phase propose/confirm for every
/// destructive verb. A benign verb applies immediately.
pub fn authorize(
    req: &VerbRequest,
    mirror: &SeatMirror,
    is_leader: bool,
    gate: &mut ConfirmGate,
    now_ms: u64,
) -> Decision {
    // (1) Never black the last console — a display disable that would leave 0 lit.
    if let HostVerb::Display { monitor, enable } = &req.verb {
        if !enable && mirror.is_lit(monitor) && mirror.lit_console_count() <= 1 {
            return Decision::Refuse(Refusal::WouldBlackLastConsole {
                monitor: monitor.clone(),
            });
        }
    }

    // (2) Leader-aware power — reboot/poweroff on the leader needs the confirm flag.
    if let HostVerb::Power { action, confirm } = &req.verb {
        if action.is_destructive() && is_leader && !confirm {
            return Decision::Refuse(Refusal::LeaderNeedsConfirm { action: *action });
        }
    }

    // (3) Two-phase confirm for the destructive verbs.
    if req.verb.is_destructive() {
        let kind = req.verb.kind_key();
        return match req.phase {
            Phase::Propose => {
                gate.propose(&kind, now_ms, CONFIRM_TTL);
                Decision::Proposed { kind }
            }
            Phase::Confirm => {
                if gate.take_live(&kind, now_ms) {
                    Decision::Apply(req.verb.clone())
                } else {
                    Decision::Refuse(Refusal::NoLiveProposal { kind })
                }
            }
        };
    }

    // Benign verb — apply immediately.
    Decision::Apply(req.verb.clone())
}

/// Parse a raw action-lane body into a [`VerbRequest`], enforcing the allowlist.
///
/// Anything that isn't a known typed verb is refused (§9 — the verb set is the
/// allowlist, no generic exec smuggled through).
///
/// # Errors
/// [`Refusal::NotAllowlisted`] when the body doesn't decode to a typed [`HostVerb`].
pub fn parse_request(body: &str) -> Result<VerbRequest, Refusal> {
    serde_json::from_str::<VerbRequest>(body).map_err(|e| Refusal::NotAllowlisted {
        detail: e.to_string(),
    })
}

// ─────────────────────────── the worker (Bus I/O over the fold) ──────────────

/// The `host_state` worker handle. Runs on every node; mirrors this node's seat
/// snapshot and authorizes remote verbs against it.
pub struct HostStateWorker {
    node_id: String,
    lock_path: PathBuf,
    bus_root: Option<PathBuf>,
    poll: Duration,
    gate: ConfirmGate,
    /// Cursor into the local snapshot topic (mirror only fresh snapshots).
    snapshot_cursor: Option<String>,
    /// Cursor into the remote-verb action topic.
    action_cursor: Option<String>,
}

impl HostStateWorker {
    /// Construct with production defaults: the fs leader lock under `workgroup_root`
    /// (matching `leader_election`), the default Bus root, and the [`POLL`] cadence.
    #[must_use]
    pub fn new(workgroup_root: PathBuf, node_id: String) -> Self {
        // Consume `workgroup_root` into the lease path (mirrors `leader_election`).
        let mut lock_path = workgroup_root;
        lock_path.push(LEADER_LOCK);
        Self {
            lock_path,
            node_id,
            bus_root: default_bus_root(),
            poll: POLL,
            gate: ConfirmGate::default(),
            snapshot_cursor: None,
            action_cursor: None,
        }
    }

    /// Override the Bus root (tests).
    #[must_use]
    pub fn with_bus_root(mut self, root: PathBuf) -> Self {
        self.bus_root = Some(root);
        self
    }

    /// Override the leader lockfile path (tests).
    #[must_use]
    pub fn with_lock_path(mut self, path: PathBuf) -> Self {
        self.lock_path = path;
        self
    }

    /// Override the poll cadence (tests).
    #[must_use]
    pub const fn with_poll(mut self, poll: Duration) -> Self {
        self.poll = poll;
        self
    }

    /// Whether this node currently holds the (unexpired) fs leadership lease — the
    /// production `is_leader` seam for the leader-aware-power interlock.
    fn is_leader(&self) -> bool {
        read_current_lease(&self.lock_path)
            .is_some_and(|lease| lease.node_id == self.node_id && !lease.is_expired(now_s()))
    }

    /// One poll pass over the Bus: mirror any fresh local snapshot to the replicated
    /// per-node topic, then drain + authorize any new remote verbs. Pulled out so a
    /// test drives it against a temp Bus without the run loop / clock.
    fn poll_once(&mut self, persist: &Persist) {
        self.mirror_snapshot(persist);
        self.drain_actions(persist);
    }

    /// Read the freshest local seat snapshot the shell published and re-publish it
    /// to the replicated `state/host/<node>/seat` topic so peers see this node.
    fn mirror_snapshot(&mut self, persist: &Persist) {
        let Ok(msgs) = persist.list_since(LOCAL_SNAPSHOT_TOPIC, self.snapshot_cursor.as_deref())
        else {
            return;
        };
        // Only the newest snapshot matters (the mirror is a level, not a stream).
        let Some(latest) = msgs.into_iter().next_back() else {
            return;
        };
        self.snapshot_cursor = Some(latest.ulid.clone());
        let Some(body) = latest.body.as_deref() else {
            return;
        };
        // Validate it decodes (never mirror a malformed body), then republish as-is.
        if serde_json::from_str::<SeatMirror>(body).is_ok() {
            let topic = mirror_topic(&self.node_id);
            if let Err(e) = persist.write(&topic, Priority::Default, None, Some(body)) {
                tracing::debug!(target: "mackesd::host_state", error = %e, "mirror publish failed");
            }
        }
    }

    /// The current mirrored snapshot (this node's), for the interlock checks. Reads
    /// the replicated topic we just wrote so a display verb guards against the live
    /// state.
    fn current_mirror(&self, persist: &Persist) -> SeatMirror {
        persist
            .list_since(&mirror_topic(&self.node_id), None)
            .ok()
            .and_then(|msgs| msgs.into_iter().next_back())
            .and_then(|m| m.body)
            .and_then(|b| serde_json::from_str::<SeatMirror>(&b).ok())
            .unwrap_or_default()
    }

    /// Drain the remote-verb action lane, authorize each against the mirror + the
    /// interlocks, and either forward an approved verb to the shell's apply lane or
    /// echo a typed refusal on the result lane.
    fn drain_actions(&mut self, persist: &Persist) {
        let topic = action_topic(&self.node_id);
        let Ok(msgs) = persist.list_since(&topic, self.action_cursor.as_deref()) else {
            return;
        };
        if msgs.is_empty() {
            return;
        }
        let mirror = self.current_mirror(persist);
        let leader = self.is_leader();
        let now = now_ms();
        self.gate.prune(now);
        for msg in msgs {
            self.action_cursor = Some(msg.ulid.clone());
            let Some(body) = msg.body.as_deref() else {
                continue;
            };
            match parse_request(body) {
                Ok(req) => {
                    let decision = authorize(&req, &mirror, leader, &mut self.gate, now);
                    self.emit_decision(persist, &req, decision);
                }
                Err(refusal) => self.emit_refusal(persist, "", &refusal),
            }
        }
    }

    /// Route a decision to the right lane: an approved verb to the shell's apply
    /// lane, a proposal/refusal to the requester's result lane.
    fn emit_decision(&self, persist: &Persist, req: &VerbRequest, decision: Decision) {
        match decision {
            Decision::Apply(verb) => {
                let body = serde_json::to_string(&verb).unwrap_or_default();
                if let Err(e) = persist.write(APPLY_TOPIC, Priority::High, None, Some(&body)) {
                    tracing::debug!(target: "mackesd::host_state", error = %e, "apply forward failed");
                }
                self.emit_result(persist, &req.requester, "applied", &body);
            }
            Decision::Proposed { kind } => {
                self.emit_result(persist, &req.requester, "proposed", &kind);
            }
            Decision::Refuse(refusal) => self.emit_refusal(persist, &req.requester, &refusal),
        }
    }

    /// Echo a typed refusal on the requester's result lane.
    fn emit_refusal(&self, persist: &Persist, requester: &str, refusal: &Refusal) {
        let body = serde_json::to_string(refusal).unwrap_or_default();
        self.emit_result(persist, requester, "refused", &body);
    }

    /// Write one `{outcome, detail, node}` record to the result lane.
    fn emit_result(&self, persist: &Persist, requester: &str, outcome: &str, detail: &str) {
        let body = serde_json::json!({
            "outcome": outcome,
            "detail": detail,
            "node": self.node_id,
            "requester": requester,
        })
        .to_string();
        let topic = result_topic(&self.node_id);
        if let Err(e) = persist.write(&topic, Priority::Default, None, Some(&body)) {
            tracing::debug!(target: "mackesd::host_state", error = %e, "result publish failed");
        }
    }
}

/// The default Bus root (same shape the other bus workers use).
fn default_bus_root() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("mde").join("bus"))
}

/// Monotonic-ish wall clock in ms for the two-phase deadlines (a coarse clock is
/// fine — the TTL is seconds). Unix epoch ms.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Unix epoch seconds (for the lease expiry check).
fn now_s() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[async_trait::async_trait]
impl Worker for HostStateWorker {
    fn name(&self) -> &'static str {
        "host_state"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let Some(root) = self.bus_root.clone() else {
            tracing::debug!(target: "mackesd::host_state", "no bus root; worker idle");
            return Ok(());
        };
        loop {
            match Persist::open(root.clone()) {
                Ok(persist) => self.poll_once(&persist),
                Err(e) => {
                    tracing::debug!(target: "mackesd::host_state", error = %e, "bus open failed");
                }
            }
            tokio::select! {
                () = tokio::time::sleep(self.poll) => {}
                () = shutdown.wait() => return Ok(()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mirror_two_lit() -> SeatMirror {
        SeatMirror {
            volume: 50,
            displays: vec![
                MirrorDisplay {
                    id: "eDP-1".into(),
                    enabled: true,
                },
                MirrorDisplay {
                    id: "HDMI-A-1".into(),
                    enabled: true,
                },
            ],
            ..Default::default()
        }
    }

    fn req(verb: HostVerb, phase: Phase) -> VerbRequest {
        VerbRequest {
            verb,
            phase,
            requester: "peer".into(),
        }
    }

    // ── allowlist (§9) ────────────────────────────────────────────────────────

    #[test]
    fn parse_accepts_typed_verbs_and_refuses_anything_else() {
        let ok = parse_request(r#"{"verb":"volume","strip":"master","level":40}"#).unwrap();
        assert_eq!(
            ok.verb,
            HostVerb::Volume {
                strip: "master".into(),
                level: 40
            }
        );
        // A non-verb body — the allowlist refuses it typed, never runs it.
        let err = parse_request(r#"{"verb":"exec","cmd":"rm -rf /"}"#).unwrap_err();
        assert!(matches!(err, Refusal::NotAllowlisted { .. }), "{err:?}");
    }

    #[test]
    fn a_benign_verb_applies_immediately() {
        let mut gate = ConfirmGate::default();
        let d = authorize(
            &req(
                HostVerb::Volume {
                    strip: "master".into(),
                    level: 30,
                },
                Phase::Confirm,
            ),
            &SeatMirror::default(),
            false,
            &mut gate,
            0,
        );
        assert!(matches!(
            d,
            Decision::Apply(HostVerb::Volume { level: 30, .. })
        ));
    }

    // ── interlock 1: never black the last console ──────────────────────────────

    #[test]
    fn disabling_the_only_lit_output_is_refused_but_one_of_two_is_allowed() {
        let mut gate = ConfirmGate::default();
        // Two lit → disabling one is allowed.
        let two = mirror_two_lit();
        let d = authorize(
            &req(
                HostVerb::Display {
                    monitor: "HDMI-A-1".into(),
                    enable: false,
                },
                Phase::Confirm,
            ),
            &two,
            false,
            &mut gate,
            0,
        );
        assert!(matches!(d, Decision::Apply(_)));

        // One lit → disabling it blacks the last console: refused typed.
        let one = SeatMirror {
            displays: vec![MirrorDisplay {
                id: "eDP-1".into(),
                enabled: true,
            }],
            ..Default::default()
        };
        let d = authorize(
            &req(
                HostVerb::Display {
                    monitor: "eDP-1".into(),
                    enable: false,
                },
                Phase::Confirm,
            ),
            &one,
            false,
            &mut gate,
            0,
        );
        assert!(
            matches!(d, Decision::Refuse(Refusal::WouldBlackLastConsole { .. })),
            "{d:?}"
        );
    }

    // ── interlock 2: leader-aware power ────────────────────────────────────────

    #[test]
    fn rebooting_the_leader_demands_the_confirm_flag() {
        let mut gate = ConfirmGate::default();
        // On the leader, no confirm flag → refused.
        let d = authorize(
            &req(
                HostVerb::Power {
                    action: PowerAction::Reboot,
                    confirm: false,
                },
                Phase::Confirm,
            ),
            &SeatMirror::default(),
            true,
            &mut gate,
            0,
        );
        assert!(
            matches!(
                d,
                Decision::Refuse(Refusal::LeaderNeedsConfirm {
                    action: PowerAction::Reboot
                })
            ),
            "{d:?}"
        );
        // Not the leader → the leader guard doesn't fire (still two-phase gated).
        let d = authorize(
            &req(
                HostVerb::Power {
                    action: PowerAction::Reboot,
                    confirm: false,
                },
                Phase::Confirm,
            ),
            &SeatMirror::default(),
            false,
            &mut gate,
            0,
        );
        assert!(
            matches!(d, Decision::Refuse(Refusal::NoLiveProposal { .. })),
            "{d:?}"
        );
    }

    // ── interlock 3: two-phase confirm ─────────────────────────────────────────

    #[test]
    fn destructive_verbs_are_two_phase_propose_then_confirm_within_ttl() {
        let mut gate = ConfirmGate::default();
        let poweroff = || HostVerb::Power {
            action: PowerAction::PowerOff,
            confirm: true,
        };

        // A bare confirm with no live proposal is refused.
        let d = authorize(
            &req(poweroff(), Phase::Confirm),
            &SeatMirror::default(),
            false,
            &mut gate,
            0,
        );
        assert!(
            matches!(d, Decision::Refuse(Refusal::NoLiveProposal { .. })),
            "{d:?}"
        );

        // Propose arms it (applies nothing yet).
        let d = authorize(
            &req(poweroff(), Phase::Propose),
            &SeatMirror::default(),
            false,
            &mut gate,
            1_000,
        );
        assert!(matches!(d, Decision::Proposed { .. }), "{d:?}");
        assert_eq!(gate.armed(), 1);

        // Confirm within the TTL → applied (and the proposal is consumed).
        let d = authorize(
            &req(poweroff(), Phase::Confirm),
            &SeatMirror::default(),
            false,
            &mut gate,
            2_000,
        );
        assert!(matches!(d, Decision::Apply(_)), "{d:?}");
        assert_eq!(gate.armed(), 0);
    }

    #[test]
    fn a_lapsed_proposal_no_longer_confirms() {
        let mut gate = ConfirmGate::default();
        let reboot = || HostVerb::Power {
            action: PowerAction::Reboot,
            confirm: true,
        };
        let ttl_ms = u64::try_from(CONFIRM_TTL.as_millis()).unwrap();

        authorize(
            &req(reboot(), Phase::Propose),
            &SeatMirror::default(),
            false,
            &mut gate,
            0,
        );
        // Confirm AFTER the TTL lapses → refused (the propose is gone).
        let d = authorize(
            &req(reboot(), Phase::Confirm),
            &SeatMirror::default(),
            false,
            &mut gate,
            ttl_ms + 1,
        );
        assert!(
            matches!(d, Decision::Refuse(Refusal::NoLiveProposal { .. })),
            "{d:?}"
        );
    }

    // ── the Bus I/O pass (mirror + authorize over a temp Bus) ──────────────────

    #[test]
    fn poll_once_mirrors_the_snapshot_and_forwards_an_approved_verb() {
        let dir = std::env::temp_dir().join(format!("host_state_test_{}", now_ms()));
        let persist = Persist::open(dir.clone()).expect("open temp bus");

        // The shell publishes a local snapshot; a peer requests a volume change.
        let snap = serde_json::to_string(&mirror_two_lit()).unwrap();
        persist
            .write(LOCAL_SNAPSHOT_TOPIC, Priority::Default, None, Some(&snap))
            .unwrap();
        persist
            .write(
                &action_topic("nodeA"),
                Priority::Default,
                None,
                Some(r#"{"verb":"volume","strip":"master","level":20,"requester":"peerB"}"#),
            )
            .unwrap();

        let mut w = HostStateWorker::new(std::env::temp_dir(), "nodeA".into()).with_bus_root(dir);
        w.poll_once(&persist);

        // The snapshot was mirrored to the replicated per-node topic…
        let mirrored = persist
            .list_since(&mirror_topic("nodeA"), None)
            .unwrap()
            .into_iter()
            .next_back()
            .and_then(|m| m.body)
            .expect("mirrored snapshot");
        assert!(mirrored.contains("\"volume\":50"));

        // …and the approved volume verb was forwarded to the shell's apply lane.
        let applied = persist.list_since(APPLY_TOPIC, None).unwrap();
        assert_eq!(applied.len(), 1);
        assert!(applied[0].body.as_deref().unwrap().contains("\"level\":20"));
    }

    #[test]
    fn poll_once_echoes_a_typed_refusal_for_a_non_allowlisted_body() {
        let dir = std::env::temp_dir().join(format!("host_state_refuse_{}", now_ms()));
        let persist = Persist::open(dir.clone()).expect("open temp bus");
        persist
            .write(
                &action_topic("nodeA"),
                Priority::Default,
                None,
                Some(r#"{"verb":"exec","cmd":"reboot"}"#),
            )
            .unwrap();
        let mut w = HostStateWorker::new(std::env::temp_dir(), "nodeA".into()).with_bus_root(dir);
        w.poll_once(&persist);

        let results = persist.list_since(&result_topic("nodeA"), None).unwrap();
        assert_eq!(results.len(), 1);
        let body = results[0].body.as_deref().unwrap();
        assert!(body.contains("refused"), "{body}");
        assert!(body.contains("not-allowlisted"), "{body}");
    }
}
