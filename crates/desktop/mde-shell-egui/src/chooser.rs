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
//! * **Connect** (CHOOSER-4) — activating a card raises the always-ask picker: the
//!   protocol when several are offered (lock 6 — never a silent default), the
//!   fullscreen/windowed choice (lock 9), and the single/span-all monitor choice
//!   (lock 12). Confirming hands a [`crate::vdi::ConnectRequest`] to [`crate::vdi`]
//!   (the Desktop surface takes over) and, for a mesh-brokered source (a peer
//!   seat / peer VM / local VM), publishes the broker `SessionRequest::Open`
//!   through [`crate::discovery::publish_open`] — the ONE copy of that wire
//!   shape (§6). An off-mesh endpoint (mDNS / manual) has no broker verb; its
//!   direct RDP/VNC transport is the gated E12-4 layer and a Spice route is gated
//!   on CHOOSER-5 — both stated honestly on the note (§7 — never a silent stub,
//!   never a faked session).
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
//! non-interactive. Activating a connectable card raises the CHOOSER-4 always-ask
//! picker (protocol · display · monitors) and nothing connects until the operator
//! confirms it (lock 6 — never a silent protocol default).

use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use mde_egui::egui::{
    self, FontId, RichText, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions,
};
use mde_egui::{muted_note, status_dot, Style};
use serde::Deserialize;

use crate::vdi::{ConnectRequest, DisplayMode, MonitorSpan, RequestedTarget, VdiProtocol};

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

/// The thumbnail well's height — CHOOSER-3's periodic preview fills this area
/// with a decoded snapshot; a source with no (or an undecodable) snapshot ref
/// falls back to the shared monitor glyph.
const THUMB_HEIGHT: f32 = Style::SP_XL * 2.25;

/// The greyed-card opacity for an unreachable source (lock 14) — dim enough to
/// read "offline", bright enough that the reason stays legible.
const OFFLINE_OPACITY: f32 = 0.5;

/// Max decoded thumbnails held live at once — the Q7 bound so a large roster
/// never piles up unbounded GPU textures. LRU-evicted: only the most recently
/// shown cards keep a live texture; the rest fall back to the icon and re-decode
/// (cheaply, from the cached ref) if scrolled back into view.
const THUMB_CACHE_CAP: usize = 48;

/// The refresh throttle — a source's ref is re-decoded at most once per this
/// window even if the worker republishes a fresh snapshot faster. Combined with
/// the "decode only when the ref string actually changed" gate, this is the Q7
/// guarantee: NEVER a decode per card per frame, only periodic + cheap.
const THUMB_MIN_DECODE_INTERVAL: Duration = Duration::from_secs(2);

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

    /// The VDI route this protocol maps to, or `None` for a tag this build can't
    /// render (badged, never connected blind — §7). Spice routes to the CHOOSER-5
    /// client, which is honest-gated downstream in [`crate::vdi`].
    const fn route(self) -> Option<VdiProtocol> {
        match self {
            Self::Rdp => Some(VdiProtocol::Rdp),
            Self::Vnc => Some(VdiProtocol::Vnc),
            Self::Spice => Some(VdiProtocol::Spice),
            Self::Unknown => None,
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
    /// The CHOOSER-3 thumbnail ref — a `data:image/png;base64,…` snapshot the
    /// worker inlines periodically (a mesh peer's published snapshot, a local
    /// VM's framebuffer grab, an external endpoint's cheap probe). Resolved to a
    /// decoded, bounded-cached texture by [`ThumbnailCache`]; `null` (no live
    /// capture backend, the honest gate today) falls back to the monitor icon.
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

// ───────────────────── CHOOSER-3: the thumbnail cache ─────────────────────

/// One cached thumbnail slot for a source id.
struct ThumbSlot {
    /// The `thumbnail_ref` that produced `texture` — a changed ref means the
    /// worker published a fresh snapshot, so the slot re-decodes. `None` = the
    /// source carried no ref (a cached miss, so we don't retry every frame).
    ref_key: Option<String>,
    /// The decoded, uploaded texture — `None` when the ref was absent or
    /// undecodable, in which case the card draws the honest icon fallback (§7).
    texture: Option<TextureHandle>,
    /// When this slot was last (re)decoded — the throttle clock, so a churning
    /// ref can't force a decode every frame (Q7).
    decoded_at: Instant,
    /// Monotonic recency stamp for LRU eviction (bumped on every access).
    used: u64,
}

/// A bounded, throttled cache of decoded card thumbnails.
///
/// Q7 risk (design doc): periodic previews must be cheap — *never* a full decode
/// per card per frame. Two guards enforce that: a source is decoded only when its
/// `thumbnail_ref` string genuinely changed (worker-paced, not frame-paced) AND
/// no sooner than [`THUMB_MIN_DECODE_INTERVAL`] since its last decode; and the
/// live texture set is LRU-capped at [`THUMB_CACHE_CAP`]. Keyed by source id.
#[derive(Default)]
struct ThumbnailCache {
    /// Decoded slots, keyed by source id.
    slots: HashMap<String, ThumbSlot>,
    /// Monotonic access clock feeding the LRU recency stamps.
    clock: u64,
}

impl ThumbnailCache {
    /// The decoded texture for `source`, or `None` when there is no resolvable
    /// snapshot (the caller then draws the monitor-icon fallback). Decodes
    /// lazily, at most once per ref per throttle window — never per frame.
    fn texture_for(
        &mut self,
        ctx: &egui::Context,
        source: &DesktopSource,
    ) -> Option<TextureHandle> {
        let want = source.thumbnail_ref.as_deref();
        self.clock = self.clock.wrapping_add(1);
        let now = Instant::now();
        if Self::needs_decode(self.slots.get(&source.id), want, now) {
            // Decode + upload OUTSIDE any egui data lock. We are in the render
            // path here, NOT inside a `ctx.data_mut(…)` closure — and this cache
            // is a plain field, not egui memory — so `load_texture` (which
            // read-locks the context) can't re-enter a `data_mut` write lock and
            // DEADLOCK (the known parking_lot trap; cf. `backdrop::logo_texture`).
            let texture = want.and_then(|r| decode_thumbnail_ref(ctx, &source.id, r));
            self.slots.insert(
                source.id.clone(),
                ThumbSlot {
                    ref_key: want.map(str::to_owned),
                    texture,
                    decoded_at: now,
                    used: self.clock,
                },
            );
            self.evict();
        } else if let Some(slot) = self.slots.get_mut(&source.id) {
            slot.used = self.clock;
        }
        self.slots.get(&source.id).and_then(|s| s.texture.clone())
    }

    /// The pure decode-or-not decision (unit-tested without a GPU or a sleep):
    /// decode a never-seen slot; re-decode when the ref genuinely changed AND the
    /// throttle window has elapsed; otherwise keep the cached result (a stale but
    /// valid preview, or the icon fallback for a cached miss).
    fn needs_decode(slot: Option<&ThumbSlot>, want: Option<&str>, now: Instant) -> bool {
        slot.is_none_or(|s| {
            s.ref_key.as_deref() != want
                && now.duration_since(s.decoded_at) >= THUMB_MIN_DECODE_INTERVAL
        })
    }

    /// Evict the least-recently-used slots down to the cap. Dropping a slot drops
    /// its `TextureHandle`, freeing the GPU texture (Q7 bound).
    fn evict(&mut self) {
        while self.slots.len() > THUMB_CACHE_CAP {
            let Some(lru) = self
                .slots
                .iter()
                .min_by_key(|(_, s)| s.used)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            self.slots.remove(&lru);
        }
    }
}

/// Resolve a `thumbnail_ref` to an uploaded texture, or `None` (icon fallback).
///
/// The ref is a `data:image/png;base64,…` snapshot inlined on the state plane;
/// this base64-decodes it, PNG-decodes to an [`egui::ColorImage`], and uploads.
/// Fail-soft at every step — a malformed/unknown ref is an honest `None`, never
/// a panic (§7).
fn decode_thumbnail_ref(ctx: &egui::Context, id: &str, ref_str: &str) -> Option<TextureHandle> {
    let image = decode_data_uri_png(ref_str)?;
    Some(ctx.load_texture(
        format!("chooser-thumb::{id}"),
        image,
        TextureOptions::LINEAR,
    ))
}

/// Decode a `data:[image/png];base64,<data>` URI to an RGBA [`egui::ColorImage`].
/// Only base64 PNG snapshots are decoded (the format the capture pipeline emits);
/// any other shape returns `None` so the card degrades to the icon (§7).
fn decode_data_uri_png(ref_str: &str) -> Option<egui::ColorImage> {
    use base64::Engine;
    let rest = ref_str.strip_prefix("data:")?;
    let (meta, payload) = rest.split_once(',')?;
    // Must be base64, and PNG (or an unspecified mediatype we optimistically
    // try as PNG) — never blindly trust an unknown encoding.
    if !meta.contains("base64") {
        return None;
    }
    let mediatype = meta.split(';').next().unwrap_or("");
    if !(mediatype.is_empty() || mediatype.eq_ignore_ascii_case("image/png")) {
        return None;
    }
    let raw = base64::engine::general_purpose::STANDARD
        .decode(payload.trim())
        .ok()?;
    decode_png_rgba(&raw)
}

/// Decode 8-bit PNG bytes (RGBA or RGB) to an [`egui::ColorImage`], the same
/// `png`-crate path `backdrop::decode_rgba` uses; RGB is expanded opaque.
/// Fail-soft on any other shape (paletted/grayscale/16-bit → `None`).
fn decode_png_rgba(bytes: &[u8]) -> Option<egui::ColorImage> {
    let mut reader = png::Decoder::new(Cursor::new(bytes)).read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    if info.bit_depth != png::BitDepth::Eight {
        return None;
    }
    let w = usize::try_from(info.width).ok()?;
    let h = usize::try_from(info.height).ok()?;
    match info.color_type {
        png::ColorType::Rgba => {
            let needed = w.checked_mul(h)?.checked_mul(4)?;
            let px = buf.get(..needed)?;
            Some(egui::ColorImage::from_rgba_unmultiplied([w, h], px))
        }
        png::ColorType::Rgb => {
            let needed = w.checked_mul(h)?.checked_mul(3)?;
            let px = buf.get(..needed)?;
            let mut rgba = Vec::with_capacity(w.checked_mul(h)?.checked_mul(4)?);
            for c in px.chunks_exact(3) {
                rgba.extend_from_slice(&[c[0], c[1], c[2], u8::MAX]);
            }
            Some(egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba))
        }
        _ => None,
    }
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

/// The in-progress connect the operator is configuring in the CHOOSER-4 picker
/// (locks 6/9/12): which source, and the three choices — protocol (seeded to the
/// first routable offer, always-asked when several exist), display mode (seeded to
/// fullscreen — the E12 idiom), and monitor span (seeded to single). Raised when a
/// connectable card is activated; drained into a [`ConnectRequest`] on confirm.
struct ConnectDraft {
    /// The source id being configured — the picker's key back into the roster.
    source_id: String,
    /// The protocol selected in the picker.
    protocol: VdiProtocol,
    /// Fullscreen vs windowed (lock 9).
    display: DisplayMode,
    /// Single vs span-all displays (lock 12).
    monitors: MonitorSpan,
}

/// The Chooser's state: the injectable roster read seam, the last published
/// roster, the auto-popup **seen set** (lock 1), the pending CHOOSER-4 connect
/// picker, and the one-shot connect hand-off the shell drains into
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
    /// The connect the operator is configuring in the always-ask picker (lock 6/9/
    /// 12) — `None` when no card is being connected.
    pending: Option<ConnectDraft>,
    /// The request chosen this frame, if a connect fired — drained by the shell
    /// via [`Self::take_connect`] and handed to [`crate::vdi::VdiState`].
    connect: Option<ConnectRequest>,
    /// CHOOSER-3 — the bounded, throttled decode cache backing the card
    /// thumbnail wells (source `thumbnail_ref` → egui texture).
    thumbs: ThumbnailCache,
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
            pending: None,
            connect: None,
            thumbs: ThumbnailCache::default(),
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
        if let Some(draft) = self.pending.as_ref() {
            if !state.sources.iter().any(|s| s.id == draft.source_id) {
                self.pending = None;
            }
        }
        self.state = Some(state);
    }

    /// Take (and clear) the auto-popup request — the shell surfaces the
    /// Chooser through its normal central-view switch when this fires.
    pub(crate) fn take_popup(&mut self) -> bool {
        std::mem::take(&mut self.popup)
    }

    /// Take (and clear) the [`ConnectRequest`] a card connect chose this frame —
    /// the shell hands it to [`crate::vdi::VdiState`] so the Desktop surface
    /// takes over.
    pub(crate) const fn take_connect(&mut self) -> Option<ConnectRequest> {
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

    /// A connectable card was activated: raise the CHOOSER-4 always-ask picker,
    /// seeded to the first routable offer + the default display choices. Nothing
    /// connects here (lock 6 — always-ask; locks 9/12 make the display + monitor
    /// choice per-connection), so even a single-protocol source opens the picker.
    /// A source offering only a tag this build can't route opens no picker and
    /// says so honestly (§7). Offline cards never connect (lock 14).
    fn activate(&mut self, sources: &[DesktopSource], id: &str) {
        let Some(source) = sources.iter().find(|s| s.id == id) else {
            return;
        };
        if !source.connectable() {
            return;
        }
        // The routable offers seed the picker; with none, there is nothing to
        // connect to — say so rather than raise an empty picker (§7).
        let Some(first) = source.protocols.iter().find_map(|o| o.protocol.route()) else {
            self.note = Some(format!("{} offers no connectable protocol.", source.name));
            return;
        };
        self.pending = Some(ConnectDraft {
            source_id: source.id.clone(),
            protocol: first,
            display: DisplayMode::Fullscreen,
            monitors: MonitorSpan::Single,
        });
    }

    /// The operator confirmed the picker: build the [`ConnectRequest`] from the
    /// draft's chosen protocol + display + monitors and connect.
    fn confirm_connect(&mut self, sources: &[DesktopSource]) {
        let Some(draft) = self.pending.take() else {
            return;
        };
        // The roster can move under the picker; if the source vanished, drop the
        // draft silently (it's already taken).
        if let Some(source) = sources.iter().find(|s| s.id == draft.source_id) {
            self.connect_source(source, draft.protocol, draft.display, draft.monitors);
        }
    }

    /// The operator backed out of the picker.
    fn cancel_connect(&mut self) {
        self.pending = None;
    }

    /// Connect one source with the picked options: build the [`ConnectRequest`]
    /// for the Desktop surface, and — for a mesh-brokered source — publish the
    /// broker `SessionRequest::Open` through the ONE existing wire path
    /// ([`crate::discovery::publish_open`], §6). An off-mesh endpoint has no
    /// broker verb, so only the hand-off happens; either way the note says which
    /// leg is gated, and a Spice route is honest-gated on CHOOSER-5 — no session
    /// is ever faked (§7).
    fn connect_source(
        &mut self,
        source: &DesktopSource,
        protocol: VdiProtocol,
        display: DisplayMode,
        monitors: MonitorSpan,
    ) {
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
                "Requested {} from {} via {} ({} \u{00B7} {}) — brokering over the mesh.",
                source.name,
                source.node,
                protocol.label(),
                display.label(),
                monitors.label(),
            ));
        } else {
            self.note = Some(format!(
                "Direct {} connect to {} ({} \u{00B7} {}) — the live client transport attaches \
                 in E12-4.",
                protocol.label(),
                source.host,
                display.label(),
                monitors.label(),
            ));
        }
        // A Spice route is constructed honestly but its client is CHOOSER-5 —
        // name the gate so the note never implies a live Spice session (§7).
        if !protocol.has_client() {
            if let Some(note) = self.note.as_mut() {
                note.push_str(" The Spice client lands in CHOOSER-5 — no session is faked.");
            }
        }
        self.connect = Some(ConnectRequest::new(
            RequestedTarget::new(source.node.clone(), source.name.clone()),
            protocol,
            display,
            monitors,
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
    /// A card was clicked (raise the CHOOSER-4 connect picker).
    Activate(String),
    /// The connect picker was confirmed (connect with the chosen options).
    Confirm,
    /// The connect picker was dismissed.
    Cancel,
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

    // Pull the state fields the grid closure reads out to locals FIRST, then
    // borrow `thumbs` + `pending` mutably — so the closure captures only owned
    // values + two disjoint `&mut` fields, never `state` wholesale (borrow clean).
    let pending_id = state.pending.as_ref().map(|d| d.source_id.clone());
    let note = state.note.clone();
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
    let thumbs = &mut state.thumbs;
    let pending_draft = &mut state.pending;

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
                        let pending = pending_id.as_deref() == Some(source.id.as_str());
                        if let Some(a) = source_card(ui, source, pending, thumbs) {
                            action = Some(a);
                        }
                        ui.add_space(Style::SP_S);
                    }
                });
            }

            // The CHOOSER-4 always-ask connect picker — nothing connects unless
            // the operator confirms it (lock 6). The radios mutate the live draft.
            if let Some(draft) = pending_draft.as_mut() {
                if let Some(source) = sources.iter().find(|s| s.id == draft.source_id) {
                    if let Some(a) = connect_picker(ui, source, draft) {
                        action = Some(a);
                    }
                }
            }

            if let Some(note) = note.as_deref() {
                ui.add_space(Style::SP_S);
                muted_note(ui, note);
            }

            // Degraded discovery lanes, named under the grid (§7 — a lane
            // that found nothing says why, instead of silently omitting).
            if !degraded.is_empty() {
                ui.add_space(Style::SP_S);
                for line in degraded {
                    muted_note(ui, line);
                }
            }
        });

    match action {
        Some(CardAction::Activate(id)) => state.activate(&sources, &id),
        Some(CardAction::Confirm) => state.confirm_connect(&sources),
        Some(CardAction::Cancel) => state.cancel_connect(),
        None => {}
    }
}

/// Render one desktop card: the thumbnail well (the decoded live preview, or the
/// honest monitor-icon fallback), the display name, the VM power state when there
/// is one, the protocol badge row, and the status pip — greyed with the worker's
/// reason when the source is offline (lock 14). Returns the activate action when
/// the card is clicked.
fn source_card(
    ui: &mut egui::Ui,
    source: &DesktopSource,
    pending: bool,
    thumbs: &mut ThumbnailCache,
) -> Option<CardAction> {
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
                    card_body(ui, source, thumbs);
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

/// The card's thumbnail well: the source's decoded live preview when its
/// `thumbnail_ref` resolves (aspect-fit, letterboxed so a 16:9 desktop never
/// stretches), else the honest shared monitor glyph — never a fake screenshot
/// (§7). The decode is bounded + throttled by [`ThumbnailCache`] (Q7).
fn thumbnail_well(ui: &mut egui::Ui, source: &DesktopSource, thumbs: &mut ThumbnailCache) {
    let well = egui::vec2(ui.available_width(), THUMB_HEIGHT);
    let (rect, _) = ui.allocate_exact_size(well, Sense::hover());
    // The recessed plate the icon sat on / the snapshot is letterboxed over.
    ui.painter().rect_filled(rect, Style::RADIUS, Style::BG);
    if let Some(tex) = thumbs.texture_for(ui.ctx(), source) {
        // A live snapshot decoded: aspect-fit (letterbox) inside the well.
        let fit = fit_centered(rect.shrink(Style::SP_XS), tex.size_vec2());
        egui::Image::new(egui::load::SizedTexture::new(tex.id(), fit.size())).paint_at(ui, fit);
    } else {
        // Honest fallback: the shared monitor glyph, never a fake screenshot.
        let glyph = egui::Rect::from_center_size(
            rect.center(),
            egui::vec2(Style::SP_XL * 2.0, Style::SP_XL * 1.6),
        );
        crate::session::draw_monitor(&ui.painter().clone(), glyph);
    }
}

/// The largest rect of `img`'s aspect ratio centered inside `bounds` (letterbox
/// fit — never upscale-stretch a snapshot to the well's aspect). A degenerate
/// image size falls back to the full bounds.
fn fit_centered(bounds: egui::Rect, img: egui::Vec2) -> egui::Rect {
    if img.x <= 0.0 || img.y <= 0.0 {
        return bounds;
    }
    let scale = (bounds.width() / img.x).min(bounds.height() / img.y);
    egui::Rect::from_center_size(bounds.center(), egui::vec2(img.x * scale, img.y * scale))
}

/// The card's content rows, top to bottom inside the plate.
fn card_body(ui: &mut egui::Ui, source: &DesktopSource, thumbs: &mut ThumbnailCache) {
    thumbnail_well(ui, source, thumbs);
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

/// The CHOOSER-4 always-ask connect picker (§7, never a silent stub): a protocol
/// radio row when the source offered several routable protocols (lock 6 — never a
/// silent default), the fullscreen/windowed choice (lock 9), and the single/span-
/// all monitor choice (lock 12), then Connect / Cancel. The radios mutate the live
/// `draft`; §4 chrome via `Style` tokens. Returns the confirm/cancel action.
fn connect_picker(
    ui: &mut egui::Ui,
    source: &DesktopSource,
    draft: &mut ConnectDraft,
) -> Option<CardAction> {
    let mut action = None;
    ui.add_space(Style::SP_M);
    ui.separator();
    ui.add_space(Style::SP_S);
    ui.label(
        RichText::new(format!("Connect to {}", source.name))
            .color(Style::TEXT)
            .size(Style::BODY)
            .strong(),
    );
    ui.add_space(Style::SP_XS);

    // The routable offers this source advertises, in the worker's stable order.
    let routable: Vec<VdiProtocol> = source
        .protocols
        .iter()
        .filter_map(|o| o.protocol.route())
        .collect();

    // Protocol — always-ask as a radio row when several are routable (lock 6).
    // A single routable protocol is stated (no false choice) so WHAT will be used
    // is still explicit.
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Protocol")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        if routable.len() > 1 {
            for proto in &routable {
                ui.radio_value(&mut draft.protocol, *proto, proto.label());
            }
        } else {
            ui.label(RichText::new(draft.protocol.label()).color(Style::TEXT));
        }
    });

    // Display mode — fullscreen or windowed (lock 9).
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Display")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.radio_value(&mut draft.display, DisplayMode::Fullscreen, "Fullscreen");
        ui.radio_value(&mut draft.display, DisplayMode::Windowed, "Windowed");
    });

    // Monitor span — a single display or span all (lock 12).
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Monitors")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.radio_value(&mut draft.monitors, MonitorSpan::Single, "Single display");
        ui.radio_value(&mut draft.monitors, MonitorSpan::All, "Span all");
    });

    // A Spice route is built honestly but its client is CHOOSER-5 — say so, never
    // imply a live session (§7).
    if !draft.protocol.has_client() {
        ui.add_space(Style::SP_XS);
        muted_note(
            ui,
            format!(
                "The {} client lands in CHOOSER-5 — the request is recorded, but no session is \
                 faked.",
                draft.protocol.label()
            ),
        );
    }

    ui.add_space(Style::SP_S);
    ui.horizontal(|ui| {
        if ui
            .button(
                RichText::new(format!("Connect via {}", draft.protocol.label())).size(Style::BODY),
            )
            .clicked()
        {
            action = Some(CardAction::Confirm);
        }
        ui.add_space(Style::SP_S);
        if ui
            .button(RichText::new("Cancel").size(Style::BODY))
            .clicked()
        {
            action = Some(CardAction::Cancel);
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

    /// Encode a `w×h` opaque-grey RGBA PNG with the same `png` crate the shell
    /// decoder uses, so the thumbnail plumbing is driven end to end by a REAL
    /// snapshot (no opaque fixture blob).
    fn tiny_png(w: u32, h: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut bytes, w, h);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut writer = enc.write_header().expect("png header");
            let px = vec![200u8; w as usize * h as usize * 4];
            writer.write_image_data(&px).expect("png data");
        }
        bytes
    }

    /// Wrap PNG bytes as the `data:image/png;base64,…` ref the worker inlines on
    /// the state plane — the exact shape [`decode_thumbnail_ref`] resolves.
    fn png_data_uri(png: &[u8]) -> String {
        use base64::Engine;
        format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(png)
        )
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

    // ── the connect flow (CHOOSER-4) ──

    #[test]
    fn the_protocol_route_maps_wire_tags_to_vdi_routes() {
        // The routing fold: each renderable wire tag maps to its VDI route; an
        // unknown tag has none (badged, never connected blind — §7).
        assert_eq!(Protocol::Rdp.route(), Some(VdiProtocol::Rdp));
        assert_eq!(Protocol::Vnc.route(), Some(VdiProtocol::Vnc));
        assert_eq!(Protocol::Spice.route(), Some(VdiProtocol::Spice));
        assert_eq!(Protocol::Unknown.route(), None);
    }

    #[test]
    fn a_single_protocol_source_still_asks_display_options_then_hands_off_once() {
        let mut state = state_with(Some(roster(vec![source(
            "peer-vm:oak:web1",
            "oak",
            &[Protocol::Spice],
        )])));
        let sources = state.sources_snapshot();

        // Even a single protocol opens the picker: fullscreen/windowed + the
        // monitor span are per-connection choices (locks 9/12), so activate must
        // NOT connect — it seeds the draft to the one offer.
        state.activate(&sources, "peer-vm:oak:web1");
        assert!(
            state.take_connect().is_none(),
            "activate opens the picker, not a connect"
        );
        assert_eq!(
            state.pending.as_ref().map(|d| d.protocol),
            Some(VdiProtocol::Spice)
        );

        state.confirm_connect(&sources);
        // The broker publish had no Bus root → the honest inline error (the same
        // discipline as the E12-5b picker), but the Desktop hand-off still
        // happens so the surface reflects the pending connect.
        assert!(state
            .last_error
            .as_deref()
            .is_some_and(|e| e.contains("Bus")));
        let req = state.take_connect().expect("a request was handed off");
        assert_eq!(req.target.serving_peer, "oak");
        assert_eq!(req.target.name, "web1");
        assert_eq!(req.protocol, VdiProtocol::Spice);
        assert_eq!(req.display, DisplayMode::Fullscreen, "seeded to fullscreen");
        assert_eq!(
            req.monitors,
            MonitorSpan::Single,
            "seeded to single display"
        );
        assert!(state.take_connect().is_none(), "the hand-off drains once");
        // The Spice route is gated on CHOOSER-5 — the note says so, no fake.
        assert!(state
            .note
            .as_deref()
            .is_some_and(|n| n.contains("CHOOSER-5")));
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
        state.confirm_connect(&sources);
        // No broker verb was attempted (no Bus error), and the note names the
        // gated direct-transport leg honestly (§7).
        assert!(state.last_error.is_none());
        assert!(state
            .note
            .as_deref()
            .is_some_and(|n| n.contains("RDP") && n.contains("E12-4")));
        let req = state.take_connect().expect("hand-off");
        assert_eq!(req.target.name, "OfficePC");
        assert_eq!(req.protocol, VdiProtocol::Rdp);
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
        assert!(
            state.pending.is_none(),
            "greyed cards don't open the picker"
        );
    }

    #[test]
    fn an_unknown_only_source_offers_no_connectable_protocol() {
        // A source advertising only a tag this build can't route: activation opens
        // no picker and says so honestly — never a blind connect (§7).
        let mut state = state_with(Some(roster(vec![source(
            "peer:oak",
            "oak",
            &[Protocol::Unknown],
        )])));
        let sources = state.sources_snapshot();
        state.activate(&sources, "peer:oak");
        assert!(state.pending.is_none(), "no routable protocol → no picker");
        assert!(state.take_connect().is_none());
        assert!(state
            .note
            .as_deref()
            .is_some_and(|n| n.contains("no connectable protocol")));
    }

    #[test]
    fn the_picker_seeds_the_first_routable_offer_skipping_unknown() {
        // [Unknown, Rdp]: the unknown tag is badged but never routed — the picker
        // seeds to RDP (the first routable offer).
        let mut state = state_with(Some(roster(vec![source(
            "peer:oak",
            "oak",
            &[Protocol::Unknown, Protocol::Rdp],
        )])));
        let sources = state.sources_snapshot();
        state.activate(&sources, "peer:oak");
        assert_eq!(
            state.pending.as_ref().map(|d| d.protocol),
            Some(VdiProtocol::Rdp)
        );
    }

    #[test]
    fn a_multi_protocol_source_asks_the_protocol_and_connects_only_on_confirm() {
        let mut state = state_with(Some(roster(vec![source(
            "peer:oak",
            "oak",
            &[Protocol::Rdp, Protocol::Vnc],
        )])));
        let sources = state.sources_snapshot();

        // Activation raises the CHOOSER-4 picker seeded to the first offer — it
        // must NOT connect (lock 6 — always-ask, never a silent first-pick).
        state.activate(&sources, "peer:oak");
        assert_eq!(
            state.pending.as_ref().map(|d| d.source_id.as_str()),
            Some("peer:oak")
        );
        assert_eq!(
            state.pending.as_ref().map(|d| d.protocol),
            Some(VdiProtocol::Rdp)
        );
        assert!(state.take_connect().is_none(), "no silent first-pick");

        // Cancel backs out.
        state.cancel_connect();
        assert!(state.pending.is_none());

        // Ask again, pick VNC + windowed + span-all, then confirm — the request
        // is built from exactly those choices (the CHOOSER-4 construction fold).
        state.activate(&sources, "peer:oak");
        {
            let draft = state.pending.as_mut().expect("the picker is open");
            draft.protocol = VdiProtocol::Vnc;
            draft.display = DisplayMode::Windowed;
            draft.monitors = MonitorSpan::All;
        }
        state.confirm_connect(&sources);
        assert!(state.pending.is_none());
        let req = state.take_connect().expect("confirm connects");
        assert_eq!(req.target.serving_peer, "oak");
        assert_eq!(req.protocol, VdiProtocol::Vnc);
        assert_eq!(req.display, DisplayMode::Windowed);
        assert_eq!(req.monitors, MonitorSpan::All);
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
    fn the_raised_connect_picker_renders_the_chooser4_affordance() {
        let mut state = state_with(Some(roster(vec![source(
            "peer:oak",
            "oak",
            &[Protocol::Rdp, Protocol::Vnc],
        )])));
        let sources = state.sources_snapshot();
        state.activate(&sources, "peer:oak");
        assert!(
            run_panel(&mut state),
            "the connect-picker affordance produced no draw primitives"
        );
        // Rendering the picker is not a connect.
        assert!(state.take_connect().is_none());
    }

    #[test]
    fn a_gated_spice_picker_renders_the_chooser5_note() {
        // A Spice-only source: the picker renders, and the gated-Spice note is
        // present (§7 — the request is honest, no session faked).
        let mut state = state_with(Some(roster(vec![source(
            "peer-vm:oak:win11",
            "oak",
            &[Protocol::Spice],
        )])));
        let sources = state.sources_snapshot();
        state.activate(&sources, "peer-vm:oak:win11");
        assert!(
            run_panel(&mut state),
            "the gated-Spice picker produced no draw primitives"
        );
        assert!(state.take_connect().is_none(), "rendering is not a connect");
    }

    // ── CHOOSER-3: the thumbnail decode + bounded/throttled cache ──

    #[test]
    fn a_png_data_uri_ref_decodes_to_an_image_of_the_right_size() {
        let img = decode_data_uri_png(&png_data_uri(&tiny_png(4, 3)))
            .expect("a valid base64 PNG data URI decodes");
        assert_eq!(img.size, [4, 3], "the decode keeps the snapshot dimensions");
    }

    #[test]
    fn a_malformed_or_unsupported_ref_is_an_honest_none() {
        // Not a data URI at all.
        assert!(decode_data_uri_png("not a data uri").is_none());
        // A data URI, but not base64-encoded.
        assert!(decode_data_uri_png("data:image/png,QUJD").is_none());
        // A mediatype the shell doesn't decode (only PNG snapshots).
        assert!(decode_data_uri_png("data:image/jpeg;base64,QUJD").is_none());
        // Well-formed base64 whose bytes are not a PNG (`QUJD` == "ABC").
        assert!(decode_data_uri_png("data:image/png;base64,QUJD").is_none());
        // Garbage base64 payload.
        assert!(decode_data_uri_png("data:image/png;base64,%%%not-base64%%%").is_none());
    }

    #[test]
    fn source_to_thumbnail_plumbing_decodes_a_ref_and_falls_back_without_one() {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let mut cache = ThumbnailCache::default();

        // A source carrying a real snapshot ref → a decoded, uploaded texture.
        let mut with_thumb = source("peer:oak", "oak", &[Protocol::Rdp]);
        with_thumb.thumbnail_ref = Some(png_data_uri(&tiny_png(6, 4)));
        let tex = cache
            .texture_for(&ctx, &with_thumb)
            .expect("a real snapshot ref resolves to a texture");
        assert_eq!(tex.size(), [6, 4], "the well shows the decoded snapshot");

        // A second frame with the SAME ref must NOT re-decode — the cached
        // handle (same texture id) is returned (Q7: never decode per frame).
        let again = cache
            .texture_for(&ctx, &with_thumb)
            .expect("the cached texture is returned");
        assert_eq!(again.id(), tex.id(), "an unchanged ref reuses the cache");

        // A source WITHOUT a ref → no texture → the honest monitor-icon fallback.
        let bare = source("peer:elm", "elm", &[Protocol::Rdp]);
        assert!(
            cache.texture_for(&ctx, &bare).is_none(),
            "no ref → the icon fallback, never a fake preview (§7)"
        );
    }

    #[test]
    fn the_decode_gate_is_first_sight_then_change_plus_throttle() {
        let t0 = Instant::now();
        let slot = ThumbSlot {
            ref_key: Some("snap-a".to_string()),
            texture: None,
            decoded_at: t0,
            used: 0,
        };
        // Never-seen source: decode now.
        assert!(ThumbnailCache::needs_decode(None, Some("snap-a"), t0));
        assert!(
            ThumbnailCache::needs_decode(None, None, t0),
            "a no-ref miss is cached too"
        );
        // Same ref: never re-decode (this is the per-frame no-op).
        assert!(!ThumbnailCache::needs_decode(
            Some(&slot),
            Some("snap-a"),
            t0
        ));
        // Changed ref but within the throttle window: keep the (stale) cache.
        assert!(!ThumbnailCache::needs_decode(
            Some(&slot),
            Some("snap-b"),
            t0
        ));
        // Changed ref AND the throttle window elapsed: re-decode the new snapshot.
        let later = t0 + THUMB_MIN_DECODE_INTERVAL + Duration::from_secs(1);
        assert!(ThumbnailCache::needs_decode(
            Some(&slot),
            Some("snap-b"),
            later
        ));
        // …but an unchanged ref stays a no-op even past the window.
        assert!(!ThumbnailCache::needs_decode(
            Some(&slot),
            Some("snap-a"),
            later
        ));
    }

    #[test]
    fn the_cache_is_lru_bounded() {
        let ctx = egui::Context::default();
        let mut cache = ThumbnailCache::default();
        // Touch more distinct sources than the cap; each first sight inserts a
        // slot (a no-ref miss is enough to exercise the eviction).
        for i in 0..(THUMB_CACHE_CAP + 6) {
            let s = source(&format!("peer:n{i}"), "n", &[Protocol::Rdp]);
            let _ = cache.texture_for(&ctx, &s);
        }
        assert_eq!(
            cache.slots.len(),
            THUMB_CACHE_CAP,
            "the live texture set is bounded (Q7)"
        );
        // The earliest-touched ids were evicted; the most-recent survive.
        assert!(
            !cache.slots.contains_key("peer:n0"),
            "the LRU slot was evicted"
        );
        assert!(
            cache
                .slots
                .contains_key(&format!("peer:n{}", THUMB_CACHE_CAP + 5)),
            "the most-recently shown card is retained"
        );
    }

    #[test]
    fn a_thumbnailed_card_renders_the_decoded_preview_end_to_end() {
        let mut thumbnailed = source("peer:oak", "oak", &[Protocol::Rdp]);
        thumbnailed.thumbnail_ref = Some(png_data_uri(&tiny_png(8, 6)));
        let mut state = state_with(Some(roster(vec![thumbnailed])));
        assert!(
            run_panel(&mut state),
            "the thumbnailed card produced no draw primitives"
        );
        // The render ran the full source→texture path into the bounded cache.
        assert_eq!(state.thumbs.slots.len(), 1);
        assert!(
            state.thumbs.slots.values().all(|s| s.texture.is_some()),
            "the card's snapshot decoded to a live texture"
        );
    }
}
