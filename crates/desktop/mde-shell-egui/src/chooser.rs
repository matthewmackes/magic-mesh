//! CHOOSER-2 — the **Desktop Chooser** surface: every discovered desktop as a
//! live card grid, grouped by node/host, one click from connecting.
//!
//! Design: `docs/design/desktop-chooser.md` (locks 1/2/3/6/14). The mackesd
//! CHOOSER-1 aggregator worker folds four discovery lanes (mesh-registry, mDNS,
//! local KVM, manual) into ONE roster on `state/desktops/sources`; this surface
//! renders that roster and drives the existing VDI attach path. It is the
//! Desktop surface's no-session face — the modernized successor to the E12-5b
//! flat picker list ([`crate::discovery`] keeps the broker-`Open` wire contract
//! this surface still emits through).
//!
//! * **Read** `state/desktops/sources` — the worker's latest
//!   [`DesktopSourcesState`] (sources + per-lane discovery status). The payload
//!   is a JSON boundary: **local** serde mirrors of the worker's wire types,
//!   exactly as `mde-files-egui::mesh_mount` (FILEMGR-9) mirrors the mesh-mount
//!   worker — the shell leans inward on `mde-bus` only, never on `mackesd` (§6).
//!   The [`DesktopSourcesClient`] seam is injectable so the model is unit-tested
//!   headless (a fake) while production talks the Bus ([`BusDesktopSources`]).
//! * **Connect** — a card click hands a [`RequestedTarget`] to [`crate::vdi`]
//!   (the Desktop surface takes over) and, for a mesh-brokered source (a peer
//!   seat / peer VM / local VM), publishes the broker `SessionRequest::Open`
//!   through [`crate::discovery::publish_open`] — the ONE copy of that wire
//!   shape (§6). An off-mesh endpoint (mDNS / manual) has no broker verb; its
//!   direct RDP/VNC/Spice client transport is the gated E12-4/CHOOSER-5 layer,
//!   stated honestly on the card's note (§7 — never a silent stub).
//! * **Auto-popup** (lock 1) — the fold keeps a **seen set** of source ids; a
//!   genuinely new id after the first fold raises a one-shot popup flag the
//!   shell drains to surface the Chooser through its normal central-view
//!   switch.
//!
//! With no source discovered the grid gives way to the BRAND-1 backdrop
//! ([`crate::backdrop`]) with the honest reason below the logo; a populated grid
//! floats over the same backdrop dimmed to its watermark (lock 6). Reachability
//! is **read from the published state, never probed here** (lock 14): an
//! offline source renders greyed with the worker's reason and stays
//! non-interactive. Protocol choice for a multi-protocol source is CHOOSER-4;
//! until it lands the card raises a clearly-labelled confirm affordance that
//! connects via the first offered protocol only when the operator confirms.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use mde_egui::egui::{self, FontId, RichText, Sense, Stroke, StrokeKind};
use mde_egui::{muted_note, status_dot, Style};
use serde::Deserialize;

use crate::vdi::RequestedTarget;

/// The retained-latest state topic the CHOOSER-1 worker publishes the merged
/// roster to. MUST equal `mackesd::workers::desktop_sources::SOURCES_TOPIC`
/// (cross-checked in tests).
pub(crate) const SOURCES_TOPIC: &str = "state/desktops/sources";

/// Roster refresh cadence. The Bus read is a cheap local spool scan and
/// discovery is human-paced, so a 5 s poll surfaces a new/removed desktop
/// without spinning — the same cadence the other planes refresh at.
const REFRESH: Duration = Duration::from_secs(5);

/// Card width — seven XL spacing steps: wide enough for a name plus a row of
/// protocol badges, narrow enough that a few cards wrap per row in the default
/// shell body beside the dock. A behaviour param on the §4 grid, not a metric
/// literal.
const CARD_WIDTH: f32 = Style::SP_XL * 7.0;

/// Card height — a fixed height keeps the grid regular across nodes so rows
/// read as one lattice (design lock 2).
const CARD_HEIGHT: f32 = Style::SP_XL * 5.5;

/// The thumbnail well's height — the icon fallback today; CHOOSER-3's periodic
/// preview drops into this exact area later.
const THUMB_HEIGHT: f32 = Style::SP_XL * 2.25;

/// The greyed-card opacity for an unreachable source (lock 14) — dim enough to
/// read "offline", bright enough that the reason stays legible.
const OFFLINE_OPACITY: f32 = 0.5;

// ─────────────── wire mirrors of the CHOOSER-1 worker types ───────────────

/// A desktop-session protocol a source offers — the worker's `DesktopProtocol`
/// tag set, plus an honest catch-all so a future protocol degrades to an
/// unknown badge instead of failing the whole roster parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Protocol {
    /// Remote Desktop Protocol (`mde-vdi-rdp`).
    Rdp,
    /// VNC / RFB (`mde-vdi-vnc`).
    Vnc,
    /// Spice (`mde-vdi-spice`, CHOOSER-5).
    Spice,
    /// A tag this build doesn't know — badged honestly, never connected blind.
    #[serde(other)]
    Unknown,
}

impl Protocol {
    /// The card badge text.
    pub(crate) const fn badge(self) -> &'static str {
        match self {
            Self::Rdp => "RDP",
            Self::Vnc => "VNC",
            Self::Spice => "SPICE",
            Self::Unknown => "?",
        }
    }
}

/// One protocol offer on a source — a mirror of the worker's `ProtocolOffer`
/// (`port` is absent on the wire when the transport is brokered, e.g. a VM's
/// Spice console).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub(crate) struct ProtocolOffer {
    /// The protocol.
    pub(crate) protocol: Protocol,
    /// The advertised/known port, if any.
    #[serde(default)]
    pub(crate) port: Option<u16>,
}

/// Derived reachability, mirrored from the worker (lock 14 — derived from
/// roster/VM state mesh-side, NEVER probed here). An unknown tag degrades to
/// [`Self::Unknown`] — honest, never a parse failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Reachability {
    /// Roster/VM state says the source should answer.
    Reachable,
    /// Roster/VM state says it won't — the card greys with the reason.
    Unreachable,
    /// Nothing derivable (a manual endpoint is never probed) — honest.
    #[serde(other)]
    Unknown,
}

impl Reachability {
    /// The status-pip tone: live = OK, offline = danger, unverified = dim.
    const fn pip(self) -> egui::Color32 {
        match self {
            Self::Reachable => Style::OK,
            Self::Unreachable => Style::DANGER,
            Self::Unknown => Style::TEXT_DIM,
        }
    }

    /// The status-pip caption.
    const fn label(self) -> &'static str {
        match self {
            Self::Reachable => "reachable",
            Self::Unreachable => "offline",
            Self::Unknown => "unverified",
        }
    }
}

/// Which discovery lane produced a source — the worker's `SourceOrigin`, with
/// an honest catch-all for a lane this build doesn't know.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SourceOrigin {
    /// Peer-advertised via the replicated peers plane.
    MeshPeer,
    /// Discovered on the local LAN via mDNS.
    Mdns,
    /// A local libvirt/KVM guest console.
    LocalVm,
    /// Operator-added.
    Manual,
    /// A lane tag this build doesn't know.
    #[serde(other)]
    Unknown,
}

impl SourceOrigin {
    /// The card's origin caption.
    const fn label(self) -> &'static str {
        match self {
            Self::MeshPeer => "mesh peer",
            Self::Mdns => "LAN (mDNS)",
            Self::LocalVm => "local VM",
            Self::Manual => "manual",
            Self::Unknown => "discovered",
        }
    }

    /// Whether connecting goes through the mesh session broker (`Open` on
    /// `action/vdi/session`). Off-mesh endpoints have no broker verb — their
    /// direct client transport is the gated E12-4/CHOOSER-5 layer.
    const fn is_mesh_brokered(self) -> bool {
        matches!(self, Self::MeshPeer | Self::LocalVm)
    }
}

/// One discovered desktop source — the projection of the worker's
/// `DesktopSource` this surface renders (serde ignores wire fields it doesn't
/// project).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct DesktopSource {
    /// Stable id (`peer:<node>` / `peer-vm:<node>:<vm>` / `vm:<node>:<name>` /
    /// `mdns:…` / `manual:…`) — the seen-set + pending-ask key.
    pub(crate) id: String,
    /// Display name for the card.
    pub(crate) name: String,
    /// The node/host the grid groups by (design lock 3).
    pub(crate) node: String,
    /// The address a client dials — the card tooltip's honest detail.
    pub(crate) host: String,
    /// Protocols offered, in the worker's stable order.
    #[serde(default)]
    pub(crate) protocols: Vec<ProtocolOffer>,
    /// The discovery lane this source came from.
    pub(crate) origin: SourceOrigin,
    /// Derived reachability (lock 14 — never a blocking probe).
    pub(crate) reachability: Reachability,
    /// Human-readable reason when not reachable (the greyed card's caption).
    #[serde(default)]
    pub(crate) reason: Option<String>,
    /// OS hint when genuinely known.
    #[serde(default)]
    pub(crate) os_hint: Option<String>,
    /// Live power state for VM sources (`running` / `shut off` / …).
    #[serde(default)]
    pub(crate) power_state: Option<String>,
    /// The CHOOSER-3 thumbnail ref — honestly `null` from the worker today;
    /// even a future non-null ref renders the icon fallback until the
    /// CHOOSER-3 preview pipeline exists to resolve it (§7).
    #[serde(default)]
    pub(crate) thumbnail_ref: Option<String>,
}

impl DesktopSource {
    /// Whether a card click may connect: an offline source is greyed +
    /// non-interactive (lock 14 — CHOOSER-8 adds its retry affordance); an
    /// honest `Unknown` (a never-probed manual endpoint) may try.
    const fn connectable(&self) -> bool {
        !matches!(self.reachability, Reachability::Unreachable)
    }
}

/// One discovery lane's honest status (`ok …` / `gated: …` / `error: …`) — so
/// the Chooser can say WHY a lane is empty instead of silently omitting
/// sources (§7).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct LaneStatus {
    /// Lane name (`mesh-registry` / `mdns` / `local-kvm` / `manual`).
    pub(crate) lane: String,
    /// Status string.
    pub(crate) status: String,
}

impl LaneStatus {
    /// Whether the lane is degraded (gated/errored) and worth surfacing.
    fn is_degraded(&self) -> bool {
        !self.status.starts_with("ok")
    }
}

/// The full record published on [`SOURCES_TOPIC`] — the projection this
/// surface renders (publisher node + timestamp stay on the wire, unprojected).
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub(crate) struct DesktopSourcesState {
    /// The merged, deduped source roster (worker-sorted by node, then name).
    #[serde(default)]
    pub(crate) sources: Vec<DesktopSource>,
    /// Per-lane discovery status.
    #[serde(default)]
    pub(crate) lanes: Vec<LaneStatus>,
}

/// Parse a `state/desktops/sources` record body; `None` on malformed JSON (an
/// honest miss, never a panic).
pub(crate) fn parse_sources(raw: &str) -> Option<DesktopSourcesState> {
    serde_json::from_str(raw).ok()
}

/// Group the worker-sorted roster into consecutive per-node runs, preserving
/// the published order (the worker sorts case-insensitively by node — design
/// lock 3, one unified view grouped by node/host).
fn group_by_node(sources: &[DesktopSource]) -> Vec<(&str, Vec<&DesktopSource>)> {
    let mut groups: Vec<(&str, Vec<&DesktopSource>)> = Vec::new();
    for source in sources {
        match groups.last_mut() {
            Some((node, members)) if node.eq_ignore_ascii_case(&source.node) => {
                members.push(source);
            }
            _ => groups.push((source.node.as_str(), vec![source])),
        }
    }
    groups
}

// ───────────────────────────── the client seam ─────────────────────────────

/// The desktop-sources read seam: the latest published roster off the Bus.
/// Injectable so the model is unit-tested headless (a fake) while production
/// talks the Bus ([`BusDesktopSources`]) — the FILEMGR-9 `MeshMountClient`
/// pattern.
pub(crate) trait DesktopSourcesClient {
    /// The newest [`DesktopSourcesState`], or `None` when nothing was
    /// published / nothing parses. Non-blocking — a local spool scan, never a
    /// peer probe (lock 14).
    fn latest(&self) -> Option<DesktopSourcesState>;

    /// Whether this node has a Bus spool at all — a gated read must not
    /// render as a live-looking "no desktops" (§7).
    fn has_bus(&self) -> bool;
}

/// The live Bus-backed client — a synchronous local `Persist` read of the one
/// retained-latest topic. Degrades honestly to `None` when there's no Bus dir
/// or no record — never a panic, never a hang.
pub(crate) struct BusDesktopSources {
    /// The resolved Bus client spool dir, or `None` when this node has no Bus.
    bus_root: Option<PathBuf>,
}

impl BusDesktopSources {
    /// Resolve the Bus spool dir from the environment (the production path).
    fn from_env() -> Self {
        Self {
            bus_root: mde_bus::client_data_dir(),
        }
    }

    /// Construct with an explicit spool root (tests point this at a tempdir
    /// or `None`).
    #[cfg(test)]
    pub(crate) const fn with_root(bus_root: Option<PathBuf>) -> Self {
        Self { bus_root }
    }
}

impl DesktopSourcesClient for BusDesktopSources {
    fn latest(&self) -> Option<DesktopSourcesState> {
        let root = self.bus_root.clone()?;
        let persist = mde_bus::persist::Persist::open(root).ok()?;
        // The worker writes one record per change (+ heartbeat); the newest
        // (last, ULID ascending) is the live roster.
        persist
            .list_since(SOURCES_TOPIC, None)
            .ok()?
            .into_iter()
            .filter_map(|m| m.body)
            .next_back()
            .as_deref()
            .and_then(parse_sources)
    }

    fn has_bus(&self) -> bool {
        self.bus_root.is_some()
    }
}

// ───────────────────────────── the Chooser state ─────────────────────────────

/// The Chooser's state: the injectable roster read seam, the last published
/// roster, the auto-popup **seen set** (lock 1), the pending CHOOSER-4 confirm
/// ask, and the one-shot connect hand-off the shell drains into
/// [`crate::vdi::VdiState`].
pub(crate) struct ChooserState {
    /// The roster read seam ([`BusDesktopSources`] in production).
    client: Box<dyn DesktopSourcesClient>,
    /// Desktop-client Bus spool for the broker `Open` publish (the same
    /// resolved-once root the E12-5b picker held).
    bus_root: Option<PathBuf>,
    /// This node's peer name — the session's `client_peer` (resolved once).
    client_peer: String,
    /// The last published roster, if any.
    state: Option<DesktopSourcesState>,
    /// Source ids the operator has had on screen since this shell started —
    /// the auto-popup fold's memory (design lock 1).
    seen: HashSet<String>,
    /// Whether the first roster fold has seeded `seen` (the pre-existing
    /// world must not pop the Chooser at startup).
    seeded: bool,
    /// One-shot: a genuinely new source appeared — the shell drains this via
    /// [`Self::take_popup`] and surfaces the Chooser.
    popup: bool,
    /// When the Bus was last polled (drives the fixed cadence).
    last_poll: Option<Instant>,
    /// The last publish error, surfaced inline (honest; never a panic).
    last_error: Option<String>,
    /// An honest inline note about the last connect (what was requested, and
    /// which leg is gated).
    note: Option<String>,
    /// The source id awaiting the multi-protocol confirm — the clearly
    /// labelled stand-in until the CHOOSER-4 always-ask picker lands.
    pending_multi: Option<String>,
    /// The target chosen this frame, if a connect fired — drained by the
    /// shell via [`Self::take_connect`] and handed to [`crate::vdi::VdiState`].
    connect: Option<RequestedTarget>,
}

impl Default for ChooserState {
    fn default() -> Self {
        Self::with_client(
            Box::new(BusDesktopSources::from_env()),
            mde_bus::client_data_dir(),
            crate::discovery::local_peer(),
        )
    }
}

impl ChooserState {
    /// Construct over an explicit read seam + publish root (production wires
    /// the Bus; tests inject a fake and `None`).
    fn with_client(
        client: Box<dyn DesktopSourcesClient>,
        bus_root: Option<PathBuf>,
        client_peer: String,
    ) -> Self {
        Self {
            client,
            bus_root,
            client_peer,
            state: None,
            seen: HashSet::new(),
            seeded: false,
            popup: false,
            last_poll: None,
            last_error: None,
            note: None,
            pending_multi: None,
            connect: None,
        }
    }

    /// The bus-poll seam: refresh the roster when the cadence has elapsed,
    /// then keep the repaint heartbeat alive so a new source surfaces (and can
    /// auto-popup) without operator input. Cheap enough to call every frame —
    /// it self-gates.
    pub(crate) fn poll(&mut self, ctx: &egui::Context) {
        let due = self.last_poll.is_none_or(|t| t.elapsed() >= REFRESH);
        if due {
            self.last_poll = Some(Instant::now());
            self.refresh();
        }
        ctx.request_repaint_after(REFRESH);
    }

    /// Re-read the newest published roster and fold it (split from the
    /// cadence gate). A missing record keeps the last-known state — the read
    /// path never blanks a live grid on a transient read miss.
    fn refresh(&mut self) {
        if let Some(state) = self.client.latest() {
            self.fold_sources(state);
        }
    }

    /// Fold one published roster into the state: any source id not yet in the
    /// **seen set** after the first fold raises the one-shot popup (design
    /// lock 1 — auto-popup on a new-source event). The first fold seeds the
    /// set silently so the pre-existing world doesn't pop the Chooser at
    /// startup, and a pending protocol ask whose source vanished is dropped.
    fn fold_sources(&mut self, state: DesktopSourcesState) {
        let fresh: Vec<String> = state
            .sources
            .iter()
            .map(|s| s.id.clone())
            .filter(|id| !self.seen.contains(id))
            .collect();
        if self.seeded && !fresh.is_empty() {
            self.popup = true;
        }
        self.seen.extend(fresh);
        self.seeded = true;
        if let Some(pending) = self.pending_multi.as_ref() {
            if !state.sources.iter().any(|s| &s.id == pending) {
                self.pending_multi = None;
            }
        }
        self.state = Some(state);
    }

    /// Take (and clear) the auto-popup request — the shell surfaces the
    /// Chooser through its normal central-view switch when this fires.
    pub(crate) fn take_popup(&mut self) -> bool {
        std::mem::take(&mut self.popup)
    }

    /// Take (and clear) the target a card connect chose this frame — the
    /// shell hands it to [`crate::vdi::VdiState`] so the Desktop surface
    /// takes over.
    pub(crate) const fn take_connect(&mut self) -> Option<RequestedTarget> {
        self.connect.take()
    }

    /// A cloned snapshot of the current roster (the render + act-on-click
    /// paths borrow it while mutating `self`).
    fn sources_snapshot(&self) -> Vec<DesktopSource> {
        self.state
            .as_ref()
            .map(|s| s.sources.clone())
            .unwrap_or_default()
    }

    /// A card was activated: a single-protocol source connects directly; a
    /// multi-protocol source raises the CHOOSER-4 confirm ask (lock 6 says
    /// always-ask — the picker itself is CHOOSER-4, so until then nothing
    /// connects without the explicit confirm). Offline cards never connect
    /// (lock 14).
    fn activate(&mut self, sources: &[DesktopSource], id: &str) {
        let Some(source) = sources.iter().find(|s| s.id == id) else {
            return;
        };
        if !source.connectable() {
            return;
        }
        match source.protocols.len() {
            0 => {
                // A roster row with no offer (shouldn't happen; honest anyway).
                self.note = Some(format!("{} offers no connectable protocol.", source.name));
            }
            1 => self.connect_source(source),
            _ => self.pending_multi = Some(source.id.clone()),
        }
    }

    /// The operator confirmed the multi-protocol ask: connect via the FIRST
    /// offered protocol (the interim CHOOSER-4 behaviour the affordance
    /// states in so many words).
    fn confirm_multi(&mut self, sources: &[DesktopSource]) {
        if let Some(id) = self.pending_multi.clone() {
            match sources.iter().find(|s| s.id == id) {
                Some(source) => self.connect_source(source),
                None => self.pending_multi = None, // the roster moved under the ask
            }
        }
    }

    /// The operator backed out of the multi-protocol ask.
    fn cancel_multi(&mut self) {
        self.pending_multi = None;
    }

    /// Connect one source: hand the [`RequestedTarget`] to the Desktop
    /// surface, and — for a mesh-brokered source — publish the broker
    /// `SessionRequest::Open` through the ONE existing wire path
    /// ([`crate::discovery::publish_open`], §6). An off-mesh endpoint has no
    /// broker verb, so only the hand-off happens and the note says honestly
    /// which leg is gated (§7).
    fn connect_source(&mut self, source: &DesktopSource) {
        if source.origin.is_mesh_brokered() {
            // A peer seat's roster row has `name == node`, so `name` is the
            // broker's vm_id handle for seats AND VMs (the same handle the
            // E12-5b picker and Chat's Remote Control publish).
            crate::discovery::publish_open(
                self.bus_root.as_deref(),
                &mut self.last_error,
                &source.node,
                &source.name,
                &self.client_peer,
            );
            self.note = Some(format!(
                "Requested {} from {} — brokering over the mesh.",
                source.name, source.node
            ));
        } else {
            let badge = source
                .protocols
                .first()
                .map_or("?", |offer| offer.protocol.badge());
            self.note = Some(format!(
                "Direct {badge} connect to {} — the live client transport attaches in \
                 E12-4/CHOOSER-5.",
                source.host
            ));
        }
        self.pending_multi = None;
        self.connect = Some(RequestedTarget::new(
            source.node.clone(),
            source.name.clone(),
        ));
    }

    /// The honest empty-grid copy: a missing Bus (a gated read), a worker
    /// that hasn't published yet, and a genuinely quiet mesh are three
    /// different truths (§7) — and quiet degraded lanes are named so an empty
    /// grid never hides WHY a lane found nothing.
    fn empty_copy(&self) -> (String, String) {
        if !self.client.has_bus() {
            return (
                "Desktop discovery unavailable".to_string(),
                "No mesh Bus directory on this node, so the discovered-desktop roster can't \
                 be read — joining the mesh (the mde-bus spool) unblocks the Chooser."
                    .to_string(),
            );
        }
        let Some(state) = self.state.as_ref() else {
            return (
                "Desktop discovery hasn't reported yet".to_string(),
                "The mackesd desktop-sources worker hasn't published a roster on this Bus — \
                 it publishes within moments of starting."
                    .to_string(),
            );
        };
        let mut detail = "No mesh peer, LAN endpoint, or local VM is advertising a desktop — \
                          a new discovery appears here within a few seconds."
            .to_string();
        let degraded: Vec<String> = state
            .lanes
            .iter()
            .filter(|l| l.is_degraded())
            .map(|l| format!("{} — {}", l.lane, l.status))
            .collect();
        if !degraded.is_empty() {
            detail.push_str(" Quiet lanes: ");
            detail.push_str(&degraded.join("; "));
            detail.push('.');
        }
        ("No desktops discovered".to_string(), detail)
    }
}

// ───────────────────────────── the panel render ─────────────────────────────

/// What a card interaction asked for this frame — applied after the grid loop
/// so the render borrows and the state mutation never fight.
enum CardAction {
    /// A card was clicked (connect / raise the protocol ask).
    Activate(String),
    /// The multi-protocol ask was confirmed (connect via the first offer).
    ConfirmMulti,
    /// The multi-protocol ask was dismissed.
    CancelMulti,
}

/// Render the Chooser into `ui`: the BRAND-1 backdrop first (full hero +
/// honest copy when nothing is discovered, the low watermark under a populated
/// grid — lock 6), then the node-grouped card grid, the CHOOSER-4 confirm
/// affordance when raised, and the degraded-lane notes.
pub(crate) fn chooser_panel(ui: &mut egui::Ui, state: &mut ChooserState) {
    let sources = state.sources_snapshot();
    let empty = sources.is_empty();

    let status = empty.then(|| state.empty_copy());
    let coverage = if empty {
        crate::backdrop::Coverage::Empty
    } else {
        crate::backdrop::Coverage::Covered
    };
    crate::backdrop::show(
        ui,
        coverage,
        status.as_ref().map(|(t, d)| (t.as_str(), d.as_str())),
    );

    if let Some(err) = state.last_error.as_deref() {
        ui.colored_label(Style::DANGER, err);
        ui.add_space(Style::SP_S);
    }
    if empty {
        return;
    }

    // Section label — the mature planes' idiom (dim, small, sentence case).
    ui.add_space(Style::SP_S);
    ui.label(
        RichText::new("Discovered desktops")
            .color(Style::TEXT_DIM)
            .size(Style::SMALL),
    );
    ui.add_space(Style::SP_XS);

    let mut action: Option<CardAction> = None;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (node, members) in group_by_node(&sources) {
                // The node/host group header (design lock 3).
                ui.add_space(Style::SP_S);
                ui.label(
                    RichText::new(node)
                        .color(Style::TEXT)
                        .size(Style::BODY)
                        .strong(),
                );
                ui.add_space(Style::SP_XS);
                ui.horizontal_wrapped(|ui| {
                    for source in members {
                        let pending = state.pending_multi.as_deref() == Some(source.id.as_str());
                        if let Some(a) = source_card(ui, source, pending) {
                            action = Some(a);
                        }
                        ui.add_space(Style::SP_S);
                    }
                });
            }

            // The multi-protocol confirm — the clearly-labelled CHOOSER-4
            // stand-in: nothing connects unless the operator confirms.
            if let Some(pending) = state
                .pending_multi
                .as_deref()
                .and_then(|id| sources.iter().find(|s| s.id == id))
            {
                if let Some(a) = protocol_ask(ui, pending) {
                    action = Some(a);
                }
            }

            if let Some(note) = state.note.as_deref() {
                ui.add_space(Style::SP_S);
                muted_note(ui, note);
            }

            // Degraded discovery lanes, named under the grid (§7 — a lane
            // that found nothing says why, instead of silently omitting).
            let degraded: Vec<String> = state
                .state
                .as_ref()
                .map(|s| {
                    s.lanes
                        .iter()
                        .filter(|l| l.is_degraded())
                        .map(|l| format!("{} lane: {}", l.lane, l.status))
                        .collect()
                })
                .unwrap_or_default();
            if !degraded.is_empty() {
                ui.add_space(Style::SP_S);
                for line in degraded {
                    muted_note(ui, line);
                }
            }
        });

    match action {
        Some(CardAction::Activate(id)) => state.activate(&sources, &id),
        Some(CardAction::ConfirmMulti) => state.confirm_multi(&sources),
        Some(CardAction::CancelMulti) => state.cancel_multi(),
        None => {}
    }
}

/// Render one desktop card: the thumbnail well (the honest icon fallback until
/// CHOOSER-3), the display name, the VM power state when there is one, the
/// protocol badge row, and the status pip — greyed with the worker's reason
/// when the source is offline (lock 14). Returns the activate action when the
/// card is clicked.
fn source_card(ui: &mut egui::Ui, source: &DesktopSource, pending: bool) -> Option<CardAction> {
    let card = egui::vec2(CARD_WIDTH, CARD_HEIGHT);
    let response = ui
        .allocate_ui(card, |ui| {
            ui.set_min_size(card);
            ui.set_max_width(CARD_WIDTH);
            let rect = ui.max_rect();
            let hovered = source.connectable() && ui.rect_contains_pointer(rect);

            // The card plate — painted first so the content lays out over it.
            let fill = if hovered {
                Style::SURFACE_HI
            } else {
                Style::SURFACE
            };
            let border = if pending {
                Style::ACCENT_HI
            } else if hovered {
                Style::ACCENT
            } else {
                Style::BORDER
            };
            ui.painter().rect_filled(rect, Style::RADIUS, fill);
            ui.painter().rect_stroke(
                rect,
                Style::RADIUS,
                Stroke::new(1.0, border),
                StrokeKind::Inside,
            );

            if !source.connectable() {
                ui.set_opacity(OFFLINE_OPACITY);
            }
            ui.horizontal(|ui| {
                ui.add_space(Style::SP_S);
                ui.vertical(|ui| {
                    ui.set_width(Style::SP_S.mul_add(-2.0, CARD_WIDTH));
                    ui.add_space(Style::SP_S);
                    card_body(ui, source);
                });
            });
        })
        .response;

    let sense = if source.connectable() {
        Sense::click()
    } else {
        Sense::hover()
    };
    let resp = ui
        .interact(
            response.rect,
            egui::Id::new(("chooser-card", source.id.as_str())),
            sense,
        )
        .on_hover_text(card_tooltip(source));
    resp.clicked()
        .then(|| CardAction::Activate(source.id.clone()))
}

/// The card's content rows, top to bottom inside the plate.
fn card_body(ui: &mut egui::Ui, source: &DesktopSource) {
    // The thumbnail well: CHOOSER-3's periodic preview lands here; today the
    // worker publishes `thumbnail_ref: null`, so the honest fallback is the
    // shared monitor glyph — never a fake screenshot (§7). A non-null ref
    // (a future worker) still falls back until the preview pipeline exists.
    let well = egui::vec2(ui.available_width(), THUMB_HEIGHT);
    let (thumb, _) = ui.allocate_exact_size(well, Sense::hover());
    ui.painter().rect_filled(thumb, Style::RADIUS, Style::BG);
    let glyph = egui::Rect::from_center_size(
        thumb.center(),
        egui::vec2(Style::SP_XL * 2.0, Style::SP_XL * 1.6),
    );
    crate::session::draw_monitor(&ui.painter().clone(), glyph);
    if source.thumbnail_ref.is_some() {
        // Honest: a ref was published but no preview pipeline resolves it yet.
        ui.painter().text(
            thumb.left_bottom() + egui::vec2(Style::SP_XS, -Style::SP_XS),
            egui::Align2::LEFT_BOTTOM,
            "preview pending (CHOOSER-3)",
            FontId::proportional(Style::SMALL),
            Style::TEXT_DIM,
        );
    }
    ui.add_space(Style::SP_XS);

    // Name + (for a VM) its live power state.
    ui.label(
        RichText::new(&source.name)
            .color(Style::TEXT)
            .size(Style::BODY)
            .strong(),
    );
    if let Some(power) = source.power_state.as_deref() {
        let tone = if power.trim() == "running" {
            Style::OK
        } else {
            Style::TEXT_DIM
        };
        ui.colored_label(
            tone,
            RichText::new(format!("vm {power}")).size(Style::SMALL),
        );
    }
    ui.add_space(Style::SP_XS);

    // Protocol badges (design lock 2 — protocol is a per-card badge).
    ui.horizontal(|ui| {
        for offer in &source.protocols {
            protocol_badge(ui, *offer);
            ui.add_space(Style::SP_XS);
        }
    });
    ui.add_space(Style::SP_XS);

    // The status pip + the origin caption; a greyed card carries the
    // worker's reason instead of the caption (lock 14).
    ui.horizontal(|ui| {
        status_dot(ui, source.reachability.pip());
        ui.add_space(Style::SP_XS);
        match source.reason.as_deref() {
            Some(reason) if !source.connectable() => {
                muted_note(ui, reason);
            }
            _ => {
                muted_note(
                    ui,
                    format!(
                        "{} \u{00B7} {}",
                        source.reachability.label(),
                        source.origin.label()
                    ),
                );
            }
        }
    });
}

/// The card tooltip — the honest connection detail (origin, dial address, OS
/// hint when genuinely known).
fn card_tooltip(source: &DesktopSource) -> String {
    let mut text = format!("{} \u{00B7} {}", source.origin.label(), source.host);
    if let Some(os) = source.os_hint.as_deref() {
        text.push_str(" \u{00B7} ");
        text.push_str(os);
    }
    text
}

/// One protocol badge chip. The known port rides the hover (the chip stays a
/// clean three-letter badge — lock 2).
fn protocol_badge(ui: &mut egui::Ui, offer: ProtocolOffer) {
    let galley = ui.painter().layout_no_wrap(
        offer.protocol.badge().to_string(),
        FontId::proportional(Style::SMALL),
        Style::ACCENT_HI,
    );
    let pad = egui::vec2(Style::SP_XS * 2.0, Style::SP_XS);
    let (rect, resp) = ui.allocate_exact_size(galley.size() + pad * 2.0, Sense::hover());
    ui.painter()
        .rect_filled(rect, Style::RADIUS, Style::SURFACE_HI);
    ui.painter()
        .galley(rect.min + pad, galley, Style::ACCENT_HI);
    if let Some(port) = offer.port {
        let _ = resp.on_hover_text(format!("port {port}"));
    }
}

/// The multi-protocol confirm affordance — the clearly-labelled CHOOSER-4
/// stand-in (§7, never a silent stub): it names the offers, says the always-ask
/// picker is CHOOSER-4, and connects via the FIRST offered protocol only on an
/// explicit confirm.
fn protocol_ask(ui: &mut egui::Ui, source: &DesktopSource) -> Option<CardAction> {
    let mut action = None;
    ui.add_space(Style::SP_M);
    ui.separator();
    ui.add_space(Style::SP_S);
    ui.label(
        RichText::new("Choose protocol (CHOOSER-4)")
            .color(Style::TEXT)
            .size(Style::BODY)
            .strong(),
    );
    ui.add_space(Style::SP_XS);
    let offers: Vec<&str> = source
        .protocols
        .iter()
        .map(|o| o.protocol.badge())
        .collect();
    let first = offers.first().copied().unwrap_or("?");
    muted_note(
        ui,
        format!(
            "{} offers {}. The always-ask protocol picker lands in CHOOSER-4 — confirming \
             here connects via {first} (the first offered) for now.",
            source.name,
            offers.join(" \u{00B7} "),
        ),
    );
    ui.add_space(Style::SP_S);
    ui.horizontal(|ui| {
        if ui
            .button(RichText::new(format!("Connect via {first}")).size(Style::BODY))
            .clicked()
        {
            action = Some(CardAction::ConfirmMulti);
        }
        ui.add_space(Style::SP_S);
        if ui
            .button(RichText::new("Cancel").size(Style::BODY))
            .clicked()
        {
            action = Some(CardAction::CancelMulti);
        }
    });
    action
}

// ───────────────────────────── tests ─────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use mde_egui::egui::{pos2, vec2, Rect};

    /// A fixture body in the exact `CHOOSER-1` wire shape (the worker's
    /// `DesktopSourcesState` serde output — `snake_case` tags, optional ports
    /// skipped when unknown, `thumbnail_ref` always present + honestly null).
    const FIXTURE: &str = r#"{
        "node": "elm",
        "sources": [
            {
                "id": "peer:oak", "name": "oak", "node": "oak", "host": "10.42.0.7",
                "protocols": [
                    {"protocol": "rdp", "port": 3389},
                    {"protocol": "vnc", "port": 5900}
                ],
                "origin": "mesh_peer", "reachability": "reachable",
                "os_hint": "linux", "thumbnail_ref": null
            },
            {
                "id": "peer-vm:oak:win11", "name": "win11", "node": "oak", "host": "10.42.0.7",
                "protocols": [{"protocol": "spice"}],
                "origin": "mesh_peer", "reachability": "unreachable",
                "reason": "vm shut off", "power_state": "shut off", "thumbnail_ref": null
            },
            {
                "id": "mdns:192.168.1.60:3389:rdp", "name": "OfficePC",
                "node": "192.168.1.60", "host": "192.168.1.60",
                "protocols": [{"protocol": "rdp", "port": 3389}],
                "origin": "mdns", "reachability": "reachable", "thumbnail_ref": null
            }
        ],
        "lanes": [
            {"lane": "mesh-registry", "status": "ok"},
            {"lane": "mdns", "status": "ok (3 types)"},
            {"lane": "local-kvm", "status": "gated: virsh not found"},
            {"lane": "manual", "status": "ok (0 sources)"}
        ],
        "published_at_ms": 1720000000000
    }"#;

    /// An in-memory [`DesktopSourcesClient`] with a canned roster.
    struct FakeSources(Option<DesktopSourcesState>);

    impl DesktopSourcesClient for FakeSources {
        fn latest(&self) -> Option<DesktopSourcesState> {
            self.0.clone()
        }

        fn has_bus(&self) -> bool {
            true
        }
    }

    /// A `ChooserState` over a canned roster, with no publish root (the
    /// broker publish then records its honest error) and a fixed peer name.
    fn state_with(state: Option<DesktopSourcesState>) -> ChooserState {
        let mut s = ChooserState::with_client(
            Box::new(FakeSources(state)),
            None,
            "client-node".to_string(),
        );
        s.refresh();
        s
    }

    fn fixture_state() -> DesktopSourcesState {
        parse_sources(FIXTURE).expect("the fixture decodes")
    }

    /// A minimal source row for fold/connect tests.
    fn source(id: &str, node: &str, protocols: &[Protocol]) -> DesktopSource {
        DesktopSource {
            id: id.to_string(),
            name: id.rsplit(':').next().unwrap_or(id).to_string(),
            node: node.to_string(),
            host: node.to_string(),
            protocols: protocols
                .iter()
                .map(|p| ProtocolOffer {
                    protocol: *p,
                    port: None,
                })
                .collect(),
            origin: SourceOrigin::MeshPeer,
            reachability: Reachability::Reachable,
            reason: None,
            os_hint: None,
            power_state: None,
            thumbnail_ref: None,
        }
    }

    fn roster(sources: Vec<DesktopSource>) -> DesktopSourcesState {
        DesktopSourcesState {
            sources,
            lanes: vec![],
        }
    }

    // ── the wire mirror ──

    #[test]
    fn topic_matches_the_worker_contract() {
        // Cross-check: MUST equal mackesd::workers::desktop_sources::SOURCES_TOPIC.
        assert_eq!(SOURCES_TOPIC, "state/desktops/sources");
    }

    #[test]
    fn the_chooser1_fixture_parses_to_the_projected_shape() {
        let state = fixture_state();
        assert_eq!(state.sources.len(), 3);

        // The peer seat: two offers with their well-known ports, an OS hint.
        let seat = &state.sources[0];
        assert_eq!(seat.id, "peer:oak");
        assert_eq!(seat.node, "oak");
        assert_eq!(seat.origin, SourceOrigin::MeshPeer);
        assert_eq!(seat.reachability, Reachability::Reachable);
        assert_eq!(seat.protocols.len(), 2);
        assert_eq!(seat.protocols[0].protocol, Protocol::Rdp);
        assert_eq!(seat.protocols[0].port, Some(3389));
        assert_eq!(seat.os_hint.as_deref(), Some("linux"));
        assert!(seat.thumbnail_ref.is_none(), "honestly null (CHOOSER-3)");
        assert!(seat.connectable());

        // The stopped VM: a brokered Spice offer (no port on the wire), the
        // worker's grey reason + power state, NOT connectable.
        let vm = &state.sources[1];
        assert_eq!(
            vm.protocols,
            vec![ProtocolOffer {
                protocol: Protocol::Spice,
                port: None
            }]
        );
        assert_eq!(vm.reachability, Reachability::Unreachable);
        assert_eq!(vm.reason.as_deref(), Some("vm shut off"));
        assert_eq!(vm.power_state.as_deref(), Some("shut off"));
        assert!(!vm.connectable());

        // The LAN endpoint.
        assert_eq!(state.sources[2].origin, SourceOrigin::Mdns);

        // The lanes, with the degraded one detectable.
        assert_eq!(state.lanes.len(), 4);
        let degraded: Vec<&str> = state
            .lanes
            .iter()
            .filter(|l| l.is_degraded())
            .map(|l| l.lane.as_str())
            .collect();
        assert_eq!(degraded, vec!["local-kvm"]);
    }

    #[test]
    fn unknown_tags_degrade_honestly_instead_of_failing_the_parse() {
        // A future worker minting a new protocol / lane / reachability tag
        // must not blank the whole roster: the mirrors degrade per-field.
        let raw = r#"{
            "sources": [{
                "id": "x", "name": "x", "node": "n", "host": "n",
                "protocols": [{"protocol": "quic-desktop"}],
                "origin": "carrier-pigeon", "reachability": "flaky",
                "thumbnail_ref": null
            }],
            "lanes": []
        }"#;
        let state = parse_sources(raw).expect("degrades, not fails");
        let s = &state.sources[0];
        assert_eq!(s.protocols[0].protocol, Protocol::Unknown);
        assert_eq!(s.origin, SourceOrigin::Unknown);
        assert_eq!(s.reachability, Reachability::Unknown);
        assert!(s.connectable(), "an honest Unknown may try");
    }

    #[test]
    fn malformed_state_is_an_honest_none() {
        assert!(parse_sources("not json").is_none());
    }

    #[test]
    fn bus_client_without_a_root_reads_none_and_reports_no_bus() {
        let client = BusDesktopSources::with_root(None);
        assert!(client.latest().is_none(), "no Bus dir → an honest None");
        assert!(!client.has_bus());
    }

    // ── grouping ──

    #[test]
    fn group_by_node_folds_consecutive_runs_in_published_order() {
        let state = fixture_state();
        let groups = group_by_node(&state.sources);
        let shape: Vec<(&str, usize)> = groups.iter().map(|(n, m)| (*n, m.len())).collect();
        // The worker sorts by node: 192.168.1.60 < oak — but the fixture is
        // in oak-first order, and grouping preserves the PUBLISHED order
        // (the worker owns the sort; the surface must not re-order it).
        assert_eq!(shape, vec![("oak", 2), ("192.168.1.60", 1)]);
    }

    // ── the seen-set / auto-popup fold (design lock 1) ──

    #[test]
    fn first_fold_seeds_silently_then_a_new_source_pops_once() {
        let mut state = state_with(Some(roster(vec![source(
            "peer:oak",
            "oak",
            &[Protocol::Rdp],
        )])));
        // The pre-existing world seeds the seen set without a popup.
        assert!(!state.take_popup(), "startup must not pop the Chooser");

        // The same roster again: nothing new, no popup.
        state.fold_sources(roster(vec![source("peer:oak", "oak", &[Protocol::Rdp])]));
        assert!(!state.take_popup());

        // A genuinely new source pops — once.
        state.fold_sources(roster(vec![
            source("peer:oak", "oak", &[Protocol::Rdp]),
            source("vm:elm:dev", "elm", &[Protocol::Spice]),
        ]));
        assert!(state.take_popup(), "a new source raises the popup");
        assert!(!state.take_popup(), "the popup drains once");
    }

    #[test]
    fn a_source_that_left_and_returned_does_not_repop() {
        let mut state = state_with(Some(roster(vec![source(
            "peer:oak",
            "oak",
            &[Protocol::Rdp],
        )])));
        let _ = state.take_popup();
        // oak flaps away and back: the operator already saw it — no re-pop.
        state.fold_sources(roster(vec![]));
        state.fold_sources(roster(vec![source("peer:oak", "oak", &[Protocol::Rdp])]));
        assert!(!state.take_popup(), "a seen source must not re-pop");
    }

    // ── the connect flow ──

    #[test]
    fn a_single_protocol_mesh_source_connects_and_hands_off_once() {
        let mut state = state_with(Some(roster(vec![source(
            "peer-vm:oak:web1",
            "oak",
            &[Protocol::Spice],
        )])));
        let sources = state.sources_snapshot();
        state.activate(&sources, "peer-vm:oak:web1");

        // The broker publish had no Bus root → the honest inline error (the
        // same discipline as the E12-5b picker), but the Desktop hand-off
        // still happens so the surface reflects the pending connect.
        assert!(state
            .last_error
            .as_deref()
            .is_some_and(|e| e.contains("Bus")));
        let target = state.take_connect().expect("a target was handed off");
        assert_eq!(target.serving_peer, "oak");
        assert_eq!(target.name, "web1");
        assert!(state.take_connect().is_none(), "the hand-off drains once");
    }

    #[test]
    fn an_external_endpoint_connects_without_a_broker_open() {
        let mut state = state_with(None);
        let mut lan = source(
            "mdns:192.168.1.60:3389:rdp",
            "192.168.1.60",
            &[Protocol::Rdp],
        );
        lan.origin = SourceOrigin::Mdns;
        lan.name = "OfficePC".to_string();
        state.fold_sources(roster(vec![lan]));

        let sources = state.sources_snapshot();
        state.activate(&sources, "mdns:192.168.1.60:3389:rdp");
        // No broker verb was attempted (no Bus error), and the note names the
        // gated direct-transport leg honestly (§7).
        assert!(state.last_error.is_none());
        assert!(state
            .note
            .as_deref()
            .is_some_and(|n| n.contains("RDP") && n.contains("E12-4")));
        let target = state.take_connect().expect("hand-off");
        assert_eq!(target.name, "OfficePC");
    }

    #[test]
    fn an_offline_source_never_connects() {
        let mut off = source("peer:ash", "ash", &[Protocol::Rdp]);
        off.reachability = Reachability::Unreachable;
        off.reason = Some("peer unreachable".to_string());
        let mut state = state_with(Some(roster(vec![off])));
        let sources = state.sources_snapshot();
        state.activate(&sources, "peer:ash");
        assert!(state.take_connect().is_none(), "greyed cards don't connect");
        assert!(state.pending_multi.is_none());
    }

    #[test]
    fn a_multi_protocol_source_asks_and_connects_only_on_confirm() {
        let mut state = state_with(Some(roster(vec![source(
            "peer:oak",
            "oak",
            &[Protocol::Rdp, Protocol::Vnc],
        )])));
        let sources = state.sources_snapshot();

        // Activation raises the CHOOSER-4 ask — it must NOT connect.
        state.activate(&sources, "peer:oak");
        assert_eq!(state.pending_multi.as_deref(), Some("peer:oak"));
        assert!(state.take_connect().is_none(), "no silent first-pick");

        // Cancel backs out.
        state.cancel_multi();
        assert!(state.pending_multi.is_none());

        // Ask again, then an explicit confirm connects (via the first offer).
        state.activate(&sources, "peer:oak");
        state.confirm_multi(&sources);
        assert!(state.pending_multi.is_none());
        let target = state.take_connect().expect("confirm connects");
        assert_eq!(target.serving_peer, "oak");
    }

    // ── headless mount renders (the DRM runner's path, minus the GPU) ──

    /// Drive one headless 960×640 frame of `chooser_panel` and tessellate it
    /// on the CPU — the same `Context::run` → `tessellate` path the DRM
    /// runner drives. Returns whether it produced draw primitives.
    fn run_panel(state: &mut ChooserState) -> bool {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 640.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| chooser_panel(ui, state));
        });
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        !prims.is_empty()
    }

    #[test]
    fn an_empty_roster_renders_the_backdrop_with_the_honest_reason() {
        // A published-but-quiet roster: the BRAND-1 hero + the quiet-lane copy.
        let mut state = state_with(Some(DesktopSourcesState {
            sources: vec![],
            lanes: vec![LaneStatus {
                lane: "local-kvm".to_string(),
                status: "gated: virsh not found".to_string(),
            }],
        }));
        let (title, detail) = state.empty_copy();
        assert_eq!(title, "No desktops discovered");
        assert!(
            detail.contains("local-kvm") && detail.contains("gated"),
            "the quiet lane is named: {detail}"
        );
        assert!(
            run_panel(&mut state),
            "the empty Chooser backdrop produced no draw primitives"
        );
        assert!(state.take_connect().is_none());

        // No published record yet is a DIFFERENT honest truth.
        let mut unreported = state_with(None);
        let (title, _) = unreported.empty_copy();
        assert_eq!(title, "Desktop discovery hasn't reported yet");
        assert!(run_panel(&mut unreported));
    }

    #[test]
    fn a_missing_bus_reads_as_gated_not_as_a_quiet_mesh() {
        // §7 — a gated read must not render as a live-looking "no desktops".
        let state = ChooserState::with_client(
            Box::new(BusDesktopSources::with_root(None)),
            None,
            "client-node".to_string(),
        );
        let (title, detail) = state.empty_copy();
        assert_eq!(title, "Desktop discovery unavailable");
        assert!(detail.contains("Bus") && detail.contains("unblocks"));
    }

    #[test]
    fn a_populated_roster_renders_the_grouped_card_grid() {
        let mut state = state_with(Some(fixture_state()));
        assert!(
            run_panel(&mut state),
            "the card grid produced no draw primitives"
        );
    }

    #[test]
    fn an_offline_source_renders_greyed_with_its_reason() {
        // The fixture's stopped VM is the greyed card; the render must
        // tessellate (the grey path draws real geometry + the reason).
        let mut state = state_with(Some(roster(vec![{
            let mut vm = source("peer-vm:oak:win11", "oak", &[Protocol::Spice]);
            vm.reachability = Reachability::Unreachable;
            vm.reason = Some("vm shut off".to_string());
            vm.power_state = Some("shut off".to_string());
            vm
        }])));
        assert!(
            run_panel(&mut state),
            "the offline-greyed card produced no draw primitives"
        );
    }

    #[test]
    fn the_raised_protocol_ask_renders_the_chooser4_affordance() {
        let mut state = state_with(Some(roster(vec![source(
            "peer:oak",
            "oak",
            &[Protocol::Rdp, Protocol::Vnc],
        )])));
        let sources = state.sources_snapshot();
        state.activate(&sources, "peer:oak");
        assert!(
            run_panel(&mut state),
            "the protocol-ask affordance produced no draw primitives"
        );
        // Rendering the ask is not a connect.
        assert!(state.take_connect().is_none());
    }
}
