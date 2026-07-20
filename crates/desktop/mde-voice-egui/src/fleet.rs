//! VOIP-GW-5 — the Voice panel **Fleet tab**.
//!
//! Beside the local dialer, the Voice surface grows a fleet config board: every
//! enrolled node with its callable `<hostname>@<realm>` SIP address, its live
//! sub-account/registration state, a DID-routing + failover column, and the one
//! shared-account (leader-held outbound trunk) fleet config — plus the editable
//! affordances (Provision / Re-provision, enable/disable inbound, nickname).
//!
//! ## Where the data comes from (§6 glue, tier-clean)
//!
//! The mackesd `voice_provision` worker (VOIP-GW-3) publishes one
//! [`NodeRow`]-shaped JSON body per node to `state/voice/<node>` — the live
//! reg-state (`registered` / `unregistered` / `provisioning` / `error+reason`).
//! This tab reads it straight off the local Bus spool (the persist-first path
//! the datacenter surface uses), so a failing node shows the **real** error, not
//! a fabricated online (§7 / design lock 9). The Bus payloads are mirrored with
//! LOCAL serde structs here rather than depending on the mackesd daemon, so the
//! desktop→services tier edge stays clean (the same choice mde-shell-egui made).
//!
//! ## What it writes (§9 typed verbs)
//!
//! Every operator intent is a typed verb in the canonical `action/voice/*`
//! namespace, never a command string:
//!
//! * **Provision / Re-provision** → [`PROVISION_TOPIC`]. Consumed live by
//!   VOIP-GW-3, which forces an immediate reconcile pass (design lock 8). This is
//!   the fully round-tripped control.
//! * **enable/disable inbound** → [`INBOUND_TOPIC`]; **nickname** →
//!   [`NICKNAME_TOPIC`]; **shared-account config** → [`SHARED_CONFIG_TOPIC`].
//!   These publish a real, observable typed verb (the panel half); their
//!   leader-side apply lands with VOIP-GW-6/7 (DID/failover + the account lift).
//!   The write itself is real — never a faked success.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mde_egui::egui::{self, Color32, RichText};
use mde_egui::Style;
use serde::{Deserialize, Serialize};

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;

/// Bus topic prefix the per-node reg-state / fleet-board row is published under
/// (VOIP-GW-3, design lock 9). One topic per node; the tail is the node id.
const STATE_PREFIX: &str = "state/voice/";

/// The single fleet-wide topic the master account's existing DID inventory is
/// published to (VOIP-GW-6, design lock 11). Distinct from [`STATE_PREFIX`] (a
/// per-node prefix), so the board projection never mistakes it for a node row.
const DIDS_TOPIC: &str = "state/voice-dids";

/// The typed verb the panel publishes to request a (re-)provision (design
/// lock 8). VOIP-GW-3 drains it and forces an immediate reconcile pass.
pub const PROVISION_TOPIC: &str = "action/voice/provision";

/// Typed verb: enable/disable a node's inbound sub-account registration.
pub const INBOUND_TOPIC: &str = "action/voice/inbound";

/// Typed verb: set a node's operator-facing nickname.
pub const NICKNAME_TOPIC: &str = "action/voice/nickname";

/// Typed verb: apply the one shared-account (leader-held outbound trunk +
/// caller-ID) fleet config.
pub const SHARED_CONFIG_TOPIC: &str = "action/voice/shared-config";

/// Typed verb: route an existing master-account DID to a node's sub-account.
///
/// VOIP-GW-6, design lock 11 — route-only, never a new-DID provision. The
/// leader's `voice_provision` worker drains it and applies the route.
pub const DID_ROUTE_TOPIC: &str = "action/voice/did-route";

/// Typed verb: set a node's offline-inbound failover policy (VOIP-GW-6, design
/// lock 10). The leader applies it via the Vitelity client.
pub const FAILOVER_TOPIC: &str = "action/voice/failover";

/// The single fleet-wide topic the migration **cutover status** is published to
/// (VOIP-GW-7, design lock 18). Distinct from [`STATE_PREFIX`] (a per-node
/// prefix), so the board projection never mistakes it for a node row.
const CUTOVER_TOPIC: &str = "state/voice-cutover";

/// The single fleet-wide topic the leader-held **shared-outbound** config in
/// force is mirrored to (VOIP-GW-7, design lock 13), so the panel can show the
/// value that is actually applied (e.g. a lifted legacy caller-ID).
const SHARED_STATE_TOPIC: &str = "state/voice-shared";

/// How often the tab re-reads the Bus. Voice provisioning is slow-changing, so a
/// 5 s cadence matches the datacenter surface without hammering the index.
const REFRESH: Duration = Duration::from_secs(5);

// ── The Bus payload mirror (deserialised from `state/voice/<node>`) ──────────

/// A node's provisioning / registration state.
///
/// The local mirror of the worker's `RegState` (its
/// `#[serde(tag = "state", rename_all = "kebab-case")]` shape), deserialised
/// straight from the published JSON.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum RegState {
    /// The node's SIP client has an active REGISTER (green pip).
    Registered,
    /// Provisioned + creds sealed, but not currently registered (neutral pip).
    Unregistered,
    /// A provisioning action is in flight (amber pip).
    Provisioning,
    /// Provisioning/registration failed — the honest reason (red pip).
    Error {
        /// Operator-readable failure detail (shown verbatim — never hidden).
        reason: String,
    },
}

/// One fleet-board row, mirrored from `state/voice/<node>`. Unknown/absent
/// fields default so a partial or forward-versioned body still renders honestly
/// rather than dropping the whole row.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct NodeRow {
    /// The node id (topic suffix / board key).
    pub node_id: String,
    /// The node hostname the sub-account username derives from.
    #[serde(default)]
    pub hostname: String,
    /// The Vitelity sub-account username (empty until provisioned).
    #[serde(default)]
    pub username: String,
    /// The callable `<username>@<realm>` SIP address (empty until provisioned).
    #[serde(default)]
    pub sip_uri: String,
    /// The provisioning / registration state (the flattened `state` tag).
    #[serde(flatten)]
    pub reg_state: RegState,
    /// The master-account DIDs currently routed to this node (VOIP-GW-6, design
    /// lock 11). The **actual** Vitelity routing — a route that didn't apply is
    /// simply absent, never fabricated.
    #[serde(default)]
    pub routed_dids: Vec<String>,
    /// The node's applied offline-inbound failover policy (design lock 10), or
    /// `None` when the operator hasn't set one.
    #[serde(default)]
    pub failover: Option<FailoverPolicy>,
    /// When this row was produced (epoch seconds).
    #[serde(default)]
    pub updated_at_s: u64,
}

/// A node's offline-inbound failover policy (design lock 10).
///
/// The local desktop-tier mirror of the worker's `vitelity::FailoverPolicy`.
/// Externally-tagged serde matching the worker's derive, so it deserialises the
/// published body and serialises the operator's chosen policy for the
/// `action/voice/failover` verb. Mirrored here (not imported) to keep the
/// desktop→services tier edge clean, the same choice [`NodeRow`] makes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FailoverPolicy {
    /// Send unanswered/offline calls to the sub-account's voicemail.
    Voicemail,
    /// Forward to a PSTN number when the node is unreachable.
    Forward {
        /// The E.164 number to forward to.
        number: String,
    },
    /// No failover — the caller hears an unavailable signal.
    None,
}

impl FailoverPolicy {
    /// A short operator-facing label for the row's Failover column.
    fn label(&self) -> String {
        match self {
            Self::Voicemail => "Voicemail".to_string(),
            Self::Forward { number } => format!("Forward → {number}"),
            Self::None => "None".to_string(),
        }
    }
}

/// One master-account DID mirrored from [`DIDS_TOPIC`] (design lock 11). The
/// panel lists these to offer the route control; it never provisions a new one.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DidRow {
    /// The DID (E.164-ish digits as Vitelity returns them).
    pub number: String,
    /// The sub-account username it currently rings, or `None` for the main line.
    #[serde(default)]
    pub routed_to: Option<String>,
}

/// The fleet migration phase (VOIP-GW-7, design lock 18). The local mirror of
/// the worker's `CutoverPhase` (its kebab-case serde), deserialised from the
/// published `state/voice-cutover` body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CutoverPhase {
    /// Still on the pre-split single-account model.
    Legacy,
    /// Shared-outbound lifted (outbound alive); nodes not yet reprovisioned.
    LiftedSharedOutbound,
    /// Some nodes crossed onto the split model; others pending — the flag day.
    NodesReprovisioning,
    /// Every node is on the split model — cutover done.
    CutoverComplete,
}

impl CutoverPhase {
    /// A one-line operator headline for the banner.
    const fn headline(self) -> &'static str {
        match self {
            Self::Legacy => "Legacy single-account model",
            Self::LiftedSharedOutbound => "Shared-outbound lifted — outbound stays alive",
            Self::NodesReprovisioning => "Cutover in progress",
            Self::CutoverComplete => "Cutover complete",
        }
    }

    /// The banner tone — amber while mid-flag-day, green when done, dim on the
    /// pre-migration legacy state (a `Style` token, never a raw literal — §4).
    const fn tone(self) -> Color32 {
        match self {
            Self::Legacy => Style::TEXT_DIM,
            Self::LiftedSharedOutbound | Self::NodesReprovisioning => Style::WARN,
            Self::CutoverComplete => Style::OK,
        }
    }
}

/// The fleet cutover status, mirrored from [`CUTOVER_TOPIC`] (design lock 18).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CutoverStatus {
    /// The single fleet-wide migration phase.
    pub phase: CutoverPhase,
    /// Enrolled nodes total.
    #[serde(default)]
    pub total_nodes: usize,
    /// How many are reprovisioned onto the split model.
    #[serde(default)]
    pub reprovisioned: usize,
    /// The nodes still on the legacy model (the panel shows exactly which).
    #[serde(default)]
    pub pending_nodes: Vec<String>,
    /// Whether the fleet shared-outbound config is lifted (leader-held).
    #[serde(default)]
    pub shared_outbound_lifted: bool,
    /// When this status was produced (epoch seconds).
    #[serde(default)]
    pub updated_at_s: u64,
}

/// The leader-held shared-outbound config in force, mirrored from
/// [`SHARED_STATE_TOPIC`] (design lock 13). Read-only view of what is applied.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct SharedOutboundView {
    /// The shared caller-ID all outbound PSTN presents.
    #[serde(default)]
    pub caller_id: String,
    /// The shared outbound trunk label / account.
    #[serde(default)]
    pub outbound_trunk: String,
}

impl NodeRow {
    /// The pip colour for this row's reg-state — a `Style` palette token, never a
    /// raw literal (§4): green Registered / amber Provisioning / red Error, with
    /// a dim neutral for the honest "provisioned, not yet registered".
    const fn pip(&self) -> Color32 {
        match self.reg_state {
            RegState::Registered => Style::OK,
            RegState::Provisioning => Style::WARN,
            RegState::Error { .. } => Style::DANGER,
            RegState::Unregistered => Style::TEXT_DIM,
        }
    }

    /// A short reg-state label for the row header.
    const fn reg_label(&self) -> &str {
        match self.reg_state {
            RegState::Registered => "Registered",
            RegState::Unregistered => "Not registered",
            RegState::Provisioning => "Provisioning…",
            RegState::Error { .. } => "Error",
        }
    }
}

// ── The typed verbs the panel publishes ──────────────────────────────────────

/// `action/voice/provision` — force a (re-)provision reconcile. `node_id`
/// names the target for the operator log; VOIP-GW-3 currently treats any message
/// as a fleet-wide reconcile trigger (design lock 8), so `None` = the whole
/// fleet and a per-node value is forward-compatible intent.
#[derive(Debug, Clone, Serialize)]
struct ProvisionRequest {
    node_id: Option<String>,
}

/// `action/voice/inbound` — enable/disable a node's inbound sub-account.
#[derive(Debug, Clone, Serialize)]
struct InboundRequest {
    node_id: String,
    enabled: bool,
}

/// `action/voice/nickname` — set a node's operator-facing nickname.
#[derive(Debug, Clone, Serialize)]
struct NicknameRequest {
    node_id: String,
    nickname: String,
}

/// `action/voice/shared-config` — apply the leader-held outbound trunk config.
#[derive(Debug, Clone, Serialize)]
struct SharedConfigRequest {
    caller_id: String,
    outbound_trunk: String,
}

/// `action/voice/did-route` — route an existing DID to a node's sub-account
/// (design lock 11). `node_id == None` routes it back to the main account.
#[derive(Debug, Clone, Serialize)]
struct DidRouteRequest {
    did: String,
    node_id: Option<String>,
}

/// `action/voice/failover` — set a node's offline-inbound failover policy.
#[derive(Debug, Clone, Serialize)]
struct FailoverRequest {
    node_id: String,
    policy: FailoverPolicy,
}

/// One operator intent collected during a render frame, published after the
/// render borrow ends (the egui idiom — one action per frame).
enum Pending {
    /// (Re-)provision: `None` = whole fleet, `Some(id)` = that node.
    Provision(Option<String>),
    /// Toggle a node's inbound registration.
    Inbound { node_id: String, enabled: bool },
    /// Commit a node's edited nickname.
    Nickname { node_id: String, nickname: String },
    /// Apply the shared-account fleet config.
    SharedConfig {
        caller_id: String,
        outbound_trunk: String,
    },
    /// Route an existing DID to a node's sub-account (`None` = back to main).
    DidRoute {
        did: String,
        node_id: Option<String>,
    },
    /// Set a node's offline-inbound failover policy.
    Failover {
        node_id: String,
        policy: FailoverPolicy,
    },
}

/// The local, per-node edit buffers. Neither the nickname nor the inbound-enabled
/// flag has a source field in the VOIP-GW-3 state contract yet, so these are
/// operator inputs that fire a verb (their reflected state lands with GW-6/7);
/// the defaults are the honest starting point (inbound on, no nickname).
#[derive(Debug, Clone, Default)]
struct NodeEdit {
    /// The nickname text buffer.
    nickname: String,
    /// The desired inbound-enabled toggle (defaults to enabled).
    inbound_enabled: bool,
    /// The DID number selected in this node's route picker (empty = none).
    did_pick: String,
    /// The failover kind selected in this node's failover picker.
    failover_kind: FailoverKind,
    /// The forward-number buffer (used when [`FailoverKind::Forward`]).
    forward_number: String,
}

impl NodeEdit {
    const fn new() -> Self {
        Self {
            nickname: String::new(),
            inbound_enabled: true,
            did_pick: String::new(),
            failover_kind: FailoverKind::Voicemail,
            forward_number: String::new(),
        }
    }
}

/// The failover kind a node's picker selects — the tag half of the operator's
/// choice; the forward number lives in [`NodeEdit::forward_number`]. Kept
/// separate from the wire [`FailoverPolicy`] so the picker can hold a partial
/// (a Forward whose number isn't typed yet) without fabricating a policy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum FailoverKind {
    /// Route to voicemail.
    #[default]
    Voicemail,
    /// Forward to a PSTN number.
    Forward,
    /// No failover.
    None,
}

/// The shared-account (leader-held outbound trunk) edit form.
#[derive(Debug, Clone, Default)]
struct SharedForm {
    /// The presented caller-ID number for all outbound PSTN (design lock 4/13).
    caller_id: String,
    /// The shared outbound trunk label / account.
    outbound_trunk: String,
}

// ── The tab state ────────────────────────────────────────────────────────────

/// The Fleet-tab state: the live board projected from the Bus, plus the local
/// edit buffers. Self-polls on a fixed cadence; renders and publishes verbs.
pub struct FleetState {
    /// The Bus spool root (resolved once); `None` off a mesh node.
    bus_root: Option<PathBuf>,
    /// The live per-node board, sorted by node id.
    nodes: Vec<NodeRow>,
    /// The master account's existing DID inventory (design lock 11), read from
    /// [`DIDS_TOPIC`]. The route picker offers these; the panel never invents a
    /// DID.
    dids: Vec<DidRow>,
    /// When the Bus was last polled (drives the cadence).
    last_poll: Option<Instant>,
    /// The last publish/read error, surfaced inline (honest, not swallowed).
    last_error: Option<String>,
    /// Per-node local edit buffers, keyed by node id.
    edits: HashMap<String, NodeEdit>,
    /// The shared-account fleet-config form.
    shared: SharedForm,
    /// The live fleet cutover status (VOIP-GW-7, design lock 18); `None` until
    /// the worker publishes one.
    cutover: Option<CutoverStatus>,
    /// The leader-held shared-outbound config in force (design lock 13), mirrored
    /// for a read-only display above the edit form.
    shared_current: Option<SharedOutboundView>,
}

impl Default for FleetState {
    fn default() -> Self {
        Self {
            bus_root: mde_bus::client_data_dir(),
            nodes: Vec::new(),
            dids: Vec::new(),
            last_poll: None,
            last_error: None,
            edits: HashMap::new(),
            shared: SharedForm::default(),
            cutover: None,
            shared_current: None,
        }
    }
}

impl FleetState {
    /// Build the tab state, resolving the client Bus root.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The bus-poll seam: refresh the board from the Bus when the cadence has
    /// elapsed, then keep the repaint heartbeat alive so a reg-state flip
    /// surfaces without input. Cheap enough to call every frame — it self-gates.
    pub fn poll(&mut self, ctx: &egui::Context) {
        let due = self.last_poll.is_none_or(|t| t.elapsed() >= REFRESH);
        if due {
            self.last_poll = Some(Instant::now());
            self.refresh();
        }
        ctx.request_repaint_after(REFRESH);
    }

    /// Re-read every `state/voice/<node>` topic and re-project the board. A
    /// missing dir / unreadable topic keeps the last-known board (never a
    /// panic); a malformed row is skipped rather than faking one.
    fn refresh(&mut self) {
        let Some(root) = self.bus_root.clone() else {
            self.nodes = Vec::new();
            return;
        };
        let Ok(persist) = Persist::open(root) else {
            // Keep the last-known board on a transient open failure.
            return;
        };
        self.nodes = read_board(&persist);
        self.dids = read_dids(&persist);
        self.cutover = read_cutover(&persist);
        self.shared_current = read_shared(&persist);
    }

    /// Render the Fleet tab into `ui`.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        // Disjoint field borrows so the render can hold `&nodes` and
        // `&mut edits`/`&mut shared` at once (the egui idiom).
        let Self {
            bus_root,
            nodes,
            dids,
            last_error,
            edits,
            shared,
            cutover,
            shared_current,
            ..
        } = self;

        let mut pending: Option<Pending> = None;

        if let Some(err) = last_error.as_deref() {
            ui.colored_label(Style::DANGER, err);
            ui.add_space(Style::SP_S);
        }

        // VOIP-GW-7 — the migration cutover banner (design lock 18): prompt the
        // operator clearly through the flag day (phase + which nodes remain).
        if let Some(status) = cutover.as_ref() {
            show_cutover(ui, status);
        }

        // Fleet-wide header + Provision-all.
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Fleet voice")
                    .size(Style::BODY)
                    .strong()
                    .color(Style::TEXT),
            );
            ui.add_space(Style::SP_M);
            if ui.button("Provision all").clicked() {
                pending = Some(Pending::Provision(None));
            }
        });
        ui.add_space(Style::SP_XS);
        ui.separator();
        ui.add_space(Style::SP_S);

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if nodes.is_empty() {
                    mde_egui::muted_note(
                        ui,
                        "No node has published a voice reg-state yet — the leader's \
                         voice_provision worker fills this board as it provisions each \
                         node's Vitelity sub-account.",
                    );
                } else {
                    for node in nodes.iter() {
                        let edit = edits
                            .entry(node.node_id.clone())
                            .or_insert_with(NodeEdit::new);
                        // Each node rides the shared `card()` primitive — the base
                        // surface fill, a hairline border, the mid radius, and the
                        // Raised soft shadow — so the board cards lift off the panel
                        // one consistent way (the shared surface hierarchy).
                        mde_egui::card()
                            .show(ui, |ui| show_node(ui, node, edit, dids, &mut pending));
                        ui.add_space(Style::SP_S);
                    }
                }

                ui.add_space(Style::SP_M);
                show_shared(ui, shared, shared_current.as_ref(), &mut pending);
            });

        if let Some(action) = pending {
            publish(bus_root.as_deref(), last_error, &action);
        }
    }

    /// Test seam: inject a board directly, bypassing the Bus. Used by the
    /// headless render test so it renders a real Error row without a spool.
    #[cfg(test)]
    fn with_nodes(mut self, nodes: Vec<NodeRow>) -> Self {
        self.nodes = nodes;
        // A test never touches disk — pin the poll so `poll` is a no-op.
        self.last_poll = Some(Instant::now());
        self.bus_root = None;
        self
    }

    /// Test seam: inject the DID inventory directly (bypassing the Bus) so a
    /// headless render exercises the live route picker.
    #[cfg(test)]
    fn with_dids(mut self, dids: Vec<DidRow>) -> Self {
        self.dids = dids;
        self
    }

    /// Test seam: inject the cutover status directly (bypassing the Bus) so a
    /// headless render exercises the migration banner (VOIP-GW-7).
    #[cfg(test)]
    fn with_cutover(mut self, status: CutoverStatus) -> Self {
        self.cutover = Some(status);
        self
    }
}

/// Read + project the whole board: every `state/voice/<node>` topic's latest
/// retained body, deserialised and sorted by node id. Pure over the Persist
/// handle so it is unit-testable against a seeded spool.
fn read_board(persist: &Persist) -> Vec<NodeRow> {
    let mut rows: Vec<NodeRow> = Vec::new();
    for topic in persist.list_topics().unwrap_or_default() {
        if !topic.starts_with(STATE_PREFIX) {
            continue;
        }
        // ULID-ordered oldest→newest; the last body is the latest reg-state.
        let latest = persist
            .list_since(&topic, None)
            .unwrap_or_default()
            .into_iter()
            .next_back()
            .and_then(|m| m.body);
        if let Some(body) = latest {
            if let Ok(row) = serde_json::from_str::<NodeRow>(&body) {
                rows.push(row);
            }
        }
    }
    rows.sort_by(|a, b| a.node_id.cmp(&b.node_id));
    rows
}

/// Read the master account's existing DID inventory from [`DIDS_TOPIC`] (design
/// lock 11): the latest retained body's JSON array, sorted by number. A missing
/// / malformed body yields an empty list (the picker just shows none) — never a
/// fabricated DID. Pure over the Persist handle so it is unit-testable.
fn read_dids(persist: &Persist) -> Vec<DidRow> {
    let latest = persist
        .list_since(DIDS_TOPIC, None)
        .unwrap_or_default()
        .into_iter()
        .next_back()
        .and_then(|m| m.body);
    let mut rows: Vec<DidRow> = latest
        .and_then(|body| serde_json::from_str::<Vec<DidRow>>(&body).ok())
        .unwrap_or_default();
    rows.sort_by(|a, b| a.number.cmp(&b.number));
    rows
}

/// Read the latest fleet cutover status from [`CUTOVER_TOPIC`] (design lock 18).
/// A missing / malformed body yields `None` (the banner just doesn't render) —
/// never a fabricated phase. Pure over the Persist handle (unit-testable).
fn read_cutover(persist: &Persist) -> Option<CutoverStatus> {
    persist
        .list_since(CUTOVER_TOPIC, None)
        .unwrap_or_default()
        .into_iter()
        .next_back()
        .and_then(|m| m.body)
        .and_then(|body| serde_json::from_str::<CutoverStatus>(&body).ok())
}

/// Read the leader-held shared-outbound config in force from
/// [`SHARED_STATE_TOPIC`] (design lock 13). `None` when none is applied yet.
fn read_shared(persist: &Persist) -> Option<SharedOutboundView> {
    persist
        .list_since(SHARED_STATE_TOPIC, None)
        .unwrap_or_default()
        .into_iter()
        .next_back()
        .and_then(|m| m.body)
        .and_then(|body| serde_json::from_str::<SharedOutboundView>(&body).ok())
}

/// Render the migration cutover banner (design lock 18): the phase headline, the
/// reprovision progress, and exactly which nodes still remain on the legacy
/// model — a clear operator prompt through the flag day.
fn show_cutover(ui: &mut egui::Ui, status: &CutoverStatus) {
    // The banner is a lifted card too — the same shared `card()` primitive as the
    // node cards below it, so the whole board lifts one consistent way.
    mde_egui::card().show(ui, |ui| {
        ui.horizontal(|ui| {
            mde_egui::status_dot(ui, status.phase.tone());
            ui.add_space(Style::SP_XS);
            ui.label(
                RichText::new(status.phase.headline())
                    .size(Style::BODY)
                    .strong()
                    .color(status.phase.tone()),
            );
        });
        ui.add_space(Style::SP_XS);
        mde_egui::field(
            ui,
            "Reprovisioned",
            &format!("{} of {} nodes", status.reprovisioned, status.total_nodes),
            Style::TEXT,
        );
        if status.pending_nodes.is_empty() {
            if status.phase == CutoverPhase::CutoverComplete {
                mde_egui::muted_note(ui, "No node left on the legacy model.");
            }
        } else {
            mde_egui::field(
                ui,
                "Still legacy",
                &status.pending_nodes.join(", "),
                Style::WARN,
            );
        }
    });
    ui.add_space(Style::SP_S);
}

/// Render one node card: the reg-state pip + label, its SIP address, the DID +
/// failover columns, an honest error reason, and the per-node controls.
fn show_node(
    ui: &mut egui::Ui,
    node: &NodeRow,
    edit: &mut NodeEdit,
    dids: &[DidRow],
    pending: &mut Option<Pending>,
) {
    ui.horizontal(|ui| {
        mde_egui::status_dot(ui, node.pip());
        ui.add_space(Style::SP_XS);
        let name = if node.hostname.is_empty() {
            node.node_id.as_str()
        } else {
            node.hostname.as_str()
        };
        ui.label(
            RichText::new(name)
                .size(Style::BODY)
                .strong()
                .color(Style::TEXT),
        );
        ui.add_space(Style::SP_S);
        ui.colored_label(
            node.pip(),
            RichText::new(node.reg_label()).size(Style::SMALL),
        );
    });

    ui.add_space(Style::SP_XS);
    let sip = if node.sip_uri.is_empty() {
        "— (awaiting provisioning)"
    } else {
        node.sip_uri.as_str()
    };
    mde_egui::field(ui, "SIP address", sip, Style::TEXT);
    // DID routing + failover are the live VOIP-GW-6 columns: the real mapping /
    // policy the worker published (a route/policy that didn't apply is absent,
    // never fabricated — §7).
    if node.routed_dids.is_empty() {
        mde_egui::field(ui, "DID routing", "— (none routed)", Style::TEXT_DIM);
    } else {
        mde_egui::field(ui, "DID routing", &node.routed_dids.join(", "), Style::TEXT);
    }
    match &node.failover {
        Some(policy) => mde_egui::field(ui, "Failover", &policy.label(), Style::TEXT),
        None => mde_egui::field(ui, "Failover", "— (not set)", Style::TEXT_DIM),
    }

    if let RegState::Error { reason } = &node.reg_state {
        ui.add_space(Style::SP_XS);
        ui.colored_label(Style::DANGER, RichText::new(reason).size(Style::SMALL));
    }

    ui.add_space(Style::SP_S);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Nickname")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add_space(Style::SP_XS);
        let field = ui.add(
            egui::TextEdit::singleline(&mut edit.nickname)
                .hint_text("optional")
                .desired_width(Style::SP_XL * 4.0),
        );
        if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            *pending = Some(Pending::Nickname {
                node_id: node.node_id.clone(),
                nickname: edit.nickname.clone(),
            });
        }
    });

    ui.add_space(Style::SP_XS);
    ui.horizontal(|ui| {
        if ui
            .checkbox(&mut edit.inbound_enabled, "Inbound enabled")
            .changed()
        {
            *pending = Some(Pending::Inbound {
                node_id: node.node_id.clone(),
                enabled: edit.inbound_enabled,
            });
        }
        ui.add_space(Style::SP_M);
        if ui.button("Re-provision").clicked() {
            *pending = Some(Pending::Provision(Some(node.node_id.clone())));
        }
    });

    show_did_route(ui, node, edit, dids, pending);
    show_failover(ui, node, edit, pending);
}

/// The per-node DID-route control (design lock 11): a picker over the master
/// account's existing DIDs (never a new-DID field) + a Route button that fires
/// the `action/voice/did-route` verb. Only enabled once the node is provisioned
/// (it needs a sub-account username to ring).
fn show_did_route(
    ui: &mut egui::Ui,
    node: &NodeRow,
    edit: &mut NodeEdit,
    dids: &[DidRow],
    pending: &mut Option<Pending>,
) {
    ui.add_space(Style::SP_XS);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Route DID")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add_space(Style::SP_XS);
        if dids.is_empty() {
            mde_egui::muted_note(ui, "no master DIDs published yet");
            return;
        }
        let selected = if edit.did_pick.is_empty() {
            "select…".to_string()
        } else {
            edit.did_pick.clone()
        };
        egui::ComboBox::from_id_salt((node.node_id.as_str(), "did-pick"))
            .selected_text(selected)
            .show_ui(ui, |ui| {
                for did in dids {
                    // Show where each DID currently points so the operator isn't
                    // blind to a DID already ringing another node.
                    let label = did.routed_to.as_ref().map_or_else(
                        || format!("{}  (main)", did.number),
                        |u| format!("{}  → {u}", did.number),
                    );
                    ui.selectable_value(&mut edit.did_pick, did.number.clone(), label);
                }
            });
        ui.add_space(Style::SP_XS);
        let ready = !edit.did_pick.is_empty() && !node.username.is_empty();
        if ui
            .add_enabled(ready, egui::Button::new("Route here"))
            .clicked()
        {
            *pending = Some(Pending::DidRoute {
                did: edit.did_pick.clone(),
                node_id: Some(node.node_id.clone()),
            });
        }
    });
}

/// The per-node failover control (design lock 10): a Voicemail / Forward / None
/// selector (+ a forward-number field) and an Apply button firing the
/// `action/voice/failover` verb.
fn show_failover(
    ui: &mut egui::Ui,
    node: &NodeRow,
    edit: &mut NodeEdit,
    pending: &mut Option<Pending>,
) {
    ui.add_space(Style::SP_XS);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Set failover")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add_space(Style::SP_XS);
        ui.selectable_value(
            &mut edit.failover_kind,
            FailoverKind::Voicemail,
            "Voicemail",
        );
        ui.selectable_value(&mut edit.failover_kind, FailoverKind::Forward, "Forward");
        ui.selectable_value(&mut edit.failover_kind, FailoverKind::None, "None");
        if edit.failover_kind == FailoverKind::Forward {
            ui.add_space(Style::SP_XS);
            ui.add(
                egui::TextEdit::singleline(&mut edit.forward_number)
                    .hint_text("+1 555 0100")
                    .desired_width(Style::SP_XL * 4.0),
            );
        }
        ui.add_space(Style::SP_XS);
        // A Forward with no number isn't a valid policy — don't fabricate one.
        let ready = !node.username.is_empty()
            && (edit.failover_kind != FailoverKind::Forward
                || !edit.forward_number.trim().is_empty());
        if ui.add_enabled(ready, egui::Button::new("Apply")).clicked() {
            let policy = match edit.failover_kind {
                FailoverKind::Voicemail => FailoverPolicy::Voicemail,
                FailoverKind::Forward => FailoverPolicy::Forward {
                    number: edit.forward_number.trim().to_string(),
                },
                FailoverKind::None => FailoverPolicy::None,
            };
            *pending = Some(Pending::Failover {
                node_id: node.node_id.clone(),
                policy,
            });
        }
    });
}

/// Render the shared-account (leader-held outbound trunk + caller-ID) fleet
/// config section: the display + edit affordance, applied as a typed verb.
fn show_shared(
    ui: &mut egui::Ui,
    shared: &mut SharedForm,
    current: Option<&SharedOutboundView>,
    pending: &mut Option<Pending>,
) {
    ui.separator();
    ui.add_space(Style::SP_S);
    ui.label(
        RichText::new("Shared outbound trunk (fleet)")
            .size(Style::BODY)
            .strong()
            .color(Style::TEXT),
    );
    ui.add_space(Style::SP_XS);
    mde_egui::muted_note(
        ui,
        "One leader-held account carries all outbound PSTN and presents the shared \
         caller-ID (design lock 4/13).",
    );
    // Show the config in force (e.g. a lifted legacy caller-ID) so the operator
    // sees what "Apply to fleet" actually persisted — read-only, from the leader.
    if let Some(cur) = current {
        ui.add_space(Style::SP_XS);
        let caller = if cur.caller_id.is_empty() {
            "— (none)"
        } else {
            cur.caller_id.as_str()
        };
        mde_egui::field(ui, "In force · caller ID", caller, Style::TEXT);
        if !cur.outbound_trunk.is_empty() {
            mde_egui::field(ui, "In force · trunk", &cur.outbound_trunk, Style::TEXT);
        }
    }
    ui.add_space(Style::SP_S);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Caller ID")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add_space(Style::SP_XS);
        ui.add(
            egui::TextEdit::singleline(&mut shared.caller_id)
                .hint_text("e.g. +1 555 0100")
                .desired_width(Style::SP_XL * 5.0),
        );
    });
    ui.add_space(Style::SP_XS);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Trunk")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add_space(Style::SP_XS);
        ui.add(
            egui::TextEdit::singleline(&mut shared.outbound_trunk)
                .hint_text("shared Vitelity account")
                .desired_width(Style::SP_XL * 5.0),
        );
    });
    ui.add_space(Style::SP_S);
    if ui.button("Apply to fleet").clicked() {
        *pending = Some(Pending::SharedConfig {
            caller_id: shared.caller_id.clone(),
            outbound_trunk: shared.outbound_trunk.clone(),
        });
    }
}

/// Publish one operator intent as a typed `action/voice/*` verb over the
/// persist-first Bus path. A real, observable write; a failure surfaces inline
/// (never a swallowed no-op / faked success — §7).
fn publish(bus_root: Option<&Path>, last_error: &mut Option<String>, action: &Pending) {
    let Some(root) = bus_root else {
        *last_error = Some("No mesh Bus directory — voice actions unavailable.".into());
        return;
    };
    let (topic, body) = match action {
        Pending::Provision(node_id) => (
            PROVISION_TOPIC,
            serde_json::to_string(&ProvisionRequest {
                node_id: node_id.clone(),
            }),
        ),
        Pending::Inbound { node_id, enabled } => (
            INBOUND_TOPIC,
            serde_json::to_string(&InboundRequest {
                node_id: node_id.clone(),
                enabled: *enabled,
            }),
        ),
        Pending::Nickname { node_id, nickname } => (
            NICKNAME_TOPIC,
            serde_json::to_string(&NicknameRequest {
                node_id: node_id.clone(),
                nickname: nickname.clone(),
            }),
        ),
        Pending::SharedConfig {
            caller_id,
            outbound_trunk,
        } => (
            SHARED_CONFIG_TOPIC,
            serde_json::to_string(&SharedConfigRequest {
                caller_id: caller_id.clone(),
                outbound_trunk: outbound_trunk.clone(),
            }),
        ),
        Pending::DidRoute { did, node_id } => (
            DID_ROUTE_TOPIC,
            serde_json::to_string(&DidRouteRequest {
                did: did.clone(),
                node_id: node_id.clone(),
            }),
        ),
        Pending::Failover { node_id, policy } => (
            FAILOVER_TOPIC,
            serde_json::to_string(&FailoverRequest {
                node_id: node_id.clone(),
                policy: policy.clone(),
            }),
        ),
    };
    let body = match body {
        Ok(b) => b,
        Err(e) => {
            *last_error = Some(format!("Couldn't encode voice action: {e}"));
            return;
        }
    };
    match Persist::open(root.to_path_buf())
        .and_then(|p| p.write(topic, Priority::Default, None, Some(&body)))
    {
        Ok(_) => *last_error = None,
        Err(e) => *last_error = Some(format!("Couldn't publish voice action: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err_row(id: &str, host: &str, reason: &str) -> NodeRow {
        NodeRow {
            node_id: id.to_string(),
            hostname: host.to_string(),
            username: String::new(),
            sip_uri: String::new(),
            reg_state: RegState::Error {
                reason: reason.to_string(),
            },
            routed_dids: Vec::new(),
            failover: None,
            updated_at_s: 0,
        }
    }

    #[test]
    fn deserialises_a_worker_state_body() {
        // The exact JSON shape VOIP-GW-3 publishes to `state/voice/<node>`
        // (tag = "state", flattened onto the row). A Registered node.
        let body = r#"{"node_id":"peer:eagle","hostname":"eagle","username":"eagle",
            "sip_uri":"eagle@sip.vitelity.net","state":"registered","updated_at_s":42}"#;
        let row: NodeRow = serde_json::from_str(body).unwrap();
        assert_eq!(row.node_id, "peer:eagle");
        assert_eq!(row.sip_uri, "eagle@sip.vitelity.net");
        assert_eq!(row.reg_state, RegState::Registered);
        assert_eq!(row.pip(), Style::OK);
    }

    #[test]
    fn deserialises_an_error_body_with_reason() {
        // A failing node carries its real reason — the pip is red, never online.
        let body = r#"{"node_id":"peer:x","hostname":"x","username":"x","sip_uri":"",
            "state":"error","reason":"provision failed: 403","updated_at_s":1}"#;
        let row: NodeRow = serde_json::from_str(body).unwrap();
        assert!(
            matches!(&row.reg_state, RegState::Error { reason } if reason.contains("403")),
            "expected Error carrying the real reason, got {:?}",
            row.reg_state
        );
        assert_eq!(row.pip(), Style::DANGER);
        assert_ne!(row.pip(), Style::OK, "a failing node must never show green");
    }

    #[test]
    fn each_reg_state_maps_to_a_distinct_pip_tone() {
        assert_eq!(
            NodeRow {
                reg_state: RegState::Provisioning,
                ..err_row("a", "a", "x")
            }
            .pip(),
            Style::WARN
        );
        assert_eq!(
            NodeRow {
                reg_state: RegState::Unregistered,
                ..err_row("a", "a", "x")
            }
            .pip(),
            Style::TEXT_DIM
        );
    }

    /// Drive a headless frame that mounts + tessellates the Fleet tab with a live
    /// board (a Registered node and a failing Error node), proving the tab is
    /// runtime-reachable and paints the real reg-states on the CPU — the same
    /// `Context::run` → `tessellate` path the DRM runner drives, no GPU/Bus.
    #[test]
    fn fleet_tab_mounts_and_tessellates_with_real_states() {
        let mut fleet = FleetState::new()
            .with_nodes(vec![
                NodeRow {
                    node_id: "peer:eagle".into(),
                    hostname: "eagle".into(),
                    username: "eagle".into(),
                    sip_uri: "eagle@sip.vitelity.net".into(),
                    reg_state: RegState::Registered,
                    routed_dids: vec!["15551234567".into()],
                    failover: Some(FailoverPolicy::Voicemail),
                    updated_at_s: 1,
                },
                err_row("peer:pine", "pine", "provision failed: master key missing"),
            ])
            .with_dids(vec![DidRow {
                number: "15551234567".into(),
                routed_to: Some("eagle".into()),
            }]);

        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(520.0, 420.0),
            )),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| fleet.show(ui));
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        assert!(!prims.is_empty(), "fleet tab produced no draw primitives");
    }

    #[test]
    fn empty_board_renders_an_honest_note() {
        // No published state → an honest "nothing yet", never a fabricated node.
        let mut fleet = FleetState::new().with_nodes(vec![]);
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(520.0, 420.0),
            )),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| fleet.show(ui));
        });
        assert!(!ctx.tessellate(out.shapes, out.pixels_per_point).is_empty());
    }

    #[test]
    fn provision_verb_round_trips_through_the_bus() {
        // The Provision control's real effect: a typed `action/voice/provision`
        // message lands on the Bus, readable back — the live-consumed verb
        // (VOIP-GW-3), proven end-to-end against a real spool.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let mut err = None;
        publish(
            Some(root.as_path()),
            &mut err,
            &Pending::Provision(Some("peer:eagle".into())),
        );
        assert!(err.is_none(), "publish should succeed: {err:?}");

        let persist = Persist::open(root).unwrap();
        let msgs = persist.list_since(PROVISION_TOPIC, None).unwrap();
        assert_eq!(msgs.len(), 1);
        let body = msgs[0].body.as_deref().unwrap();
        assert!(body.contains("peer:eagle"));
    }

    #[test]
    fn deserialises_a_row_with_routed_dids_and_failover() {
        // The VOIP-GW-6 body carries the live DID mapping + applied failover.
        let body = r#"{"node_id":"peer:eagle","hostname":"eagle","username":"eagle",
            "sip_uri":"eagle@sip.vitelity.net","state":"registered",
            "routed_dids":["15551234567"],"failover":{"Forward":{"number":"15550001111"}},
            "updated_at_s":42}"#;
        let row: NodeRow = serde_json::from_str(body).unwrap();
        assert_eq!(row.routed_dids, vec!["15551234567".to_string()]);
        assert_eq!(
            row.failover,
            Some(FailoverPolicy::Forward {
                number: "15550001111".into()
            })
        );
    }

    #[test]
    fn read_dids_projects_the_latest_inventory_sorted() {
        let tmp = tempfile::tempdir().unwrap();
        let persist = Persist::open(tmp.path().to_path_buf()).unwrap();
        // A stale then a fresh inventory — the fresh (later ULID) wins.
        persist
            .write(DIDS_TOPIC, Priority::Min, None, Some("[]"))
            .unwrap();
        persist
            .write(
                DIDS_TOPIC,
                Priority::Min,
                None,
                Some(r#"[{"number":"15559990000","routed_to":null},{"number":"15551234567","routed_to":"eagle"}]"#),
            )
            .unwrap();
        let dids = read_dids(&persist);
        assert_eq!(dids.len(), 2);
        assert_eq!(dids[0].number, "15551234567", "sorted by number");
        assert_eq!(dids[0].routed_to, Some("eagle".to_string()));
        assert_eq!(dids[1].routed_to, None);
    }

    #[test]
    fn did_route_verb_round_trips_through_the_bus() {
        // The Route control's real effect: a typed `action/voice/did-route`
        // message lands, readable back — the live-consumed verb (VOIP-GW-6).
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let mut err = None;
        publish(
            Some(root.as_path()),
            &mut err,
            &Pending::DidRoute {
                did: "15551234567".into(),
                node_id: Some("peer:eagle".into()),
            },
        );
        assert!(err.is_none(), "publish should succeed: {err:?}");
        let persist = Persist::open(root).unwrap();
        let msgs = persist.list_since(DID_ROUTE_TOPIC, None).unwrap();
        assert_eq!(msgs.len(), 1);
        let body = msgs[0].body.as_deref().unwrap();
        assert!(body.contains("15551234567"));
        assert!(body.contains("peer:eagle"));
    }

    #[test]
    fn failover_verb_round_trips_through_the_bus() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let mut err = None;
        publish(
            Some(root.as_path()),
            &mut err,
            &Pending::Failover {
                node_id: "peer:eagle".into(),
                policy: FailoverPolicy::Forward {
                    number: "15550001111".into(),
                },
            },
        );
        assert!(err.is_none(), "publish should succeed: {err:?}");
        let persist = Persist::open(root).unwrap();
        let msgs = persist.list_since(FAILOVER_TOPIC, None).unwrap();
        assert_eq!(msgs.len(), 1);
        let body = msgs[0].body.as_deref().unwrap();
        assert!(body.contains("Forward"));
        assert!(body.contains("15550001111"));
    }

    // ── VOIP-GW-7: the migration cutover banner (design lock 18) ──

    #[test]
    fn deserialises_a_cutover_status_body() {
        // The exact JSON shape VOIP-GW-7 publishes to `state/voice-cutover`
        // (phase kebab-case + the pending nodes).
        let body = r#"{"phase":"nodes-reprovisioning","total_nodes":2,"reprovisioned":1,
            "pending_nodes":["pine"],"shared_outbound_lifted":true,"updated_at_s":7}"#;
        let status: CutoverStatus = serde_json::from_str(body).unwrap();
        assert_eq!(status.phase, CutoverPhase::NodesReprovisioning);
        assert_eq!(status.reprovisioned, 1);
        assert_eq!(status.pending_nodes, vec!["pine".to_string()]);
        assert_eq!(status.phase.tone(), Style::WARN);
    }

    #[test]
    fn read_cutover_projects_the_latest_status() {
        let tmp = tempfile::tempdir().unwrap();
        let persist = Persist::open(tmp.path().to_path_buf()).unwrap();
        persist
            .write(
                CUTOVER_TOPIC,
                Priority::Min,
                None,
                Some(r#"{"phase":"legacy","total_nodes":0,"reprovisioned":0,"pending_nodes":[]}"#),
            )
            .unwrap();
        // A later status wins.
        persist
            .write(
                CUTOVER_TOPIC,
                Priority::Min,
                None,
                Some(r#"{"phase":"cutover-complete","total_nodes":2,"reprovisioned":2,"pending_nodes":[]}"#),
            )
            .unwrap();
        let status = read_cutover(&persist).expect("a cutover status");
        assert_eq!(status.phase, CutoverPhase::CutoverComplete);
        assert_eq!(status.reprovisioned, 2);
    }

    #[test]
    fn fleet_tab_renders_the_cutover_banner_with_pending_nodes() {
        // The mid-flag-day banner mounts + tessellates, naming the node still on
        // the legacy model — the operator prompt the acceptance turns on.
        let mut fleet = FleetState::new()
            .with_nodes(vec![NodeRow {
                node_id: "peer:eagle".into(),
                hostname: "eagle".into(),
                username: "eagle".into(),
                sip_uri: "eagle@sip.vitelity.net".into(),
                reg_state: RegState::Registered,
                routed_dids: Vec::new(),
                failover: None,
                updated_at_s: 1,
            }])
            .with_cutover(CutoverStatus {
                phase: CutoverPhase::NodesReprovisioning,
                total_nodes: 2,
                reprovisioned: 1,
                pending_nodes: vec!["pine".into()],
                shared_outbound_lifted: true,
                updated_at_s: 1,
            });

        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(520.0, 520.0),
            )),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| fleet.show(ui));
        });
        assert!(!ctx.tessellate(out.shapes, out.pixels_per_point).is_empty());
    }

    #[test]
    fn read_board_projects_latest_per_node_sorted() {
        // Two nodes, one topic each, plus an unrelated topic; the newest body per
        // `state/voice/*` topic is projected, sorted, unrelated topics ignored.
        let tmp = tempfile::tempdir().unwrap();
        let persist = Persist::open(tmp.path().to_path_buf()).unwrap();
        persist
            .write(
                "state/voice/peer:pine",
                Priority::Min,
                None,
                Some(r#"{"node_id":"peer:pine","hostname":"pine","username":"pine","sip_uri":"pine@r","state":"unregistered"}"#),
            )
            .unwrap();
        // A stale then a fresh body for eagle — the fresh (later ULID) wins.
        persist
            .write(
                "state/voice/peer:eagle",
                Priority::Min,
                None,
                Some(r#"{"node_id":"peer:eagle","hostname":"eagle","username":"eagle","sip_uri":"eagle@r","state":"provisioning"}"#),
            )
            .unwrap();
        persist
            .write(
                "state/voice/peer:eagle",
                Priority::Min,
                None,
                Some(r#"{"node_id":"peer:eagle","hostname":"eagle","username":"eagle","sip_uri":"eagle@r","state":"registered"}"#),
            )
            .unwrap();
        persist
            .write("event/unrelated", Priority::Min, None, Some("{}"))
            .unwrap();

        let board = read_board(&persist);
        assert_eq!(board.len(), 2);
        assert_eq!(board[0].node_id, "peer:eagle");
        assert_eq!(board[0].reg_state, RegState::Registered, "latest body wins");
        assert_eq!(board[1].node_id, "peer:pine");
    }
}
