//! The VDI **Desktop** surface — a remote VM desktop rendered egui-native.
//!
//! E12 "Construct" brokers VM desktops *into* the one shell (§5 EMBED, lock 21):
//! there is no external viewer. The remote framebuffer is decoded by
//! `mde-vdi-rdp` (RDP-primary), `mde-vdi-vnc` (VNC / XAPI-console fallback), or
//! `mde-vdi-spice` (native QEMU/KVM console) into an [`egui::ColorImage`]; this
//! panel uploads that image to a `TextureHandle` and paints it as the shell body,
//! and forwards the frame's egui input straight back to the session's input
//! mapper.
//!
//! ```text
//!   session.frame_with_damage() ─▶ (ColorImage, FrameDamage) ─▶ TextureHandle
//!                                     └▶ set_partial only the changed rects (perf-7)
//!   ui.input events ────────────────────────────────────────▶ session.send_input()
//! ```
//!
//! This unit is the **first caller** of the two decoder crates — it gives their
//! `frame()`/`send_input()` surface a home. Until a session is attached (the live
//! wire transport is the gated E12-4 layer) the panel shows an honest "no desktop"
//! EmptyState, never a placeholder render of a fake desktop (§7).

use mackes_mesh_types::android_provider::{AndroidVdiProtocol, AndroidVdiSource};
use mackes_mesh_types::workloads::{
    WorkloadAttachmentProtocol, WorkloadBackend, WorkloadOperationAction,
};
use mde_bus::persist::Persist;
use mde_egui::egui::{self, Sense, TextureHandle, TextureOptions};

use mde_vdi_core::{sub_color_image, FrameDamage};
use mde_vdi_rdp::RdpSession;
use mde_vdi_spice::SpiceSession;
use mde_vdi_vnc::VncSession;

use crate::auth::DesktopAuth;

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[cfg(feature = "live-vdi")]
use {
    crate::clipboard_permissions::{
        ClipboardFailure, ClipboardGateReadiness, ClipboardGateTicket, ClipboardPermissionIngress,
        ClipboardTarget, ClipboardTargetKind,
    },
    mackes_mesh_types::vdi_clipboard::{
        vdi_clipboard_session_topic, ClipboardEnvelopeV2, VdiClipboardDisclosureV2,
        VdiClipboardFileDescriptorV1, VdiClipboardFilesMaterializationRequestV1,
        VdiClipboardFilesMaterializationResponseV1, VdiClipboardLeaseV2, VdiClipboardMessageV2,
        VdiClipboardReceiptV2, VdiClipboardText,
        MAX_VDI_CLIPBOARD_FILES_MATERIALIZATION_PACKET_BYTES, MAX_VDI_CLIPBOARD_LEASE_TTL_MS,
        MAX_VDI_RDP_CLIPBOARD_IMAGE_BYTES, VDI_CLIPBOARD_FILES_MATERIALIZATION_SOCKET,
        VDI_CLIPBOARD_GUEST_TO_HOST_TOPIC_PREFIX, VDI_CLIPBOARD_HOST_TO_GUEST_TOPIC_PREFIX,
        VDI_CLIPBOARD_LEASE_TOPIC_PREFIX, VDI_CLIPBOARD_RECEIPT_TOPIC_PREFIX,
        VDI_CLIPBOARD_TRANSPORT_V2_SCHEMA_VERSION,
    },
    mde_bus::hooks::config::Priority,
    mde_collab_types::{ClipboardClipBody, ClipboardUnavailableReason},
    mde_vdi_rdp::{
        clipboard::{
            RemoteClipboardFileChunk, RemoteClipboardFileList, RemoteClipboardImage,
            RemoteClipboardImageFormat,
        },
        ConnectError, PumpOutcome, RdpConfig, RdpConnection,
    },
    mde_vdi_spice::{BlockingSpiceTransport, SpiceConfig},
    mde_vdi_vnc::{PumpOutcome as VncPumpOutcome, VncConfig, VncConnection},
    std::thread,
    std::time::Duration,
    std::{
        collections::VecDeque,
        sync::mpsc,
        sync::{Arc, Mutex},
    },
};

/// A live VDI desktop the shell drives — RDP-primary, VNC the console fallback,
/// and SPICE for native QEMU/KVM consoles. The decoder crates expose the same
/// egui-facing surface
/// (`frame()` → [`egui::ColorImage`], `send_input(&egui::Event)`), so the panel
/// drives whichever is attached through one match.
///
/// The variants are matched + driven here, but a session is *constructed* only
/// once the gated live wire transport (E12-4) attaches one — until then the panel
/// runs on the no-session EmptyState, so a non-test build sees no constructor
/// (the tests build both variants to prove the decode → paint path end to end).
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "a session is constructed by the gated E12-4 wire transport; the render panel here is its first caller"
    )
)]
enum Session {
    /// An RDP desktop (`mde-vdi-rdp`, the primary protocol).
    Rdp(RdpSession),
    /// A VNC/RFB desktop (`mde-vdi-vnc`, the universal console fallback).
    Vnc(VncSession),
    /// A SPICE desktop (`mde-vdi-spice`, the native QEMU/KVM console).
    Spice(SpiceSession),
}

impl Session {
    /// The latest decoded desktop plus which rectangles changed since the last
    /// frame ([`FrameDamage`]), or `None` if nothing changed. The shell partial-
    /// uploads the damaged sub-rectangles instead of the whole framebuffer (perf-7).
    fn frame_with_damage(&mut self) -> Option<(egui::ColorImage, FrameDamage)> {
        match self {
            Session::Rdp(s) => s.frame_with_damage(),
            Session::Vnc(s) => s.frame_with_damage(),
            Session::Spice(s) => s.frame_with_damage(),
        }
    }

    /// Forward one egui input event to the guest — the session maps it to the
    /// protocol's pointer / key / wheel / text intents internally.
    fn send_input(&mut self, event: &egui::Event) {
        match self {
            Session::Rdp(s) => s.send_input(event),
            Session::Vnc(s) => s.send_input(event),
            Session::Spice(s) => s.send_input(event),
        }
    }
}

/// A dialable endpoint for a direct desktop transport. Mesh-brokered connects may
/// omit it while the broker resolves the overlay route; manual/mDNS/external rows
/// carry it so the live VDI transport can attach without re-parsing UI text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DesktopEndpoint {
    /// TCP host/address to dial. For mesh rows this should be the Nebula overlay
    /// address or name once the registry publishes it.
    pub host: String,
    /// TCP port for the chosen protocol.
    pub port: u16,
}

impl DesktopEndpoint {
    /// A non-empty host plus non-zero port.
    pub(crate) fn new(host: impl Into<String>, port: u16) -> Option<Self> {
        let host = host.into();
        if host.trim().is_empty() || port == 0 {
            return None;
        }
        Some(Self { host, port })
    }

    /// Log/UI-safe dial address.
    fn label(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

/// A desktop target the Chooser (CHOOSER-2, née the E12-5b picker) handed to the
/// surface: the desktop the operator chose, plus the direct endpoint if discovery
/// published one. Recorded so the surface reflects the pending connect by name
/// until the live transport attaches the decoder `session` — an honest
/// "connecting" caption, never a fake desktop (§7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RequestedTarget {
    /// The peer serving the VM (a scheduler node id).
    pub serving_peer: String,
    /// The VM's display name — the surface caption.
    pub name: String,
    /// Direct dial target for manual/mDNS/external endpoints, or for mesh rows once
    /// the registry has published an overlay address + port.
    pub endpoint: Option<DesktopEndpoint>,
}

impl RequestedTarget {
    /// A target from the peer serving the VM and the VM's name.
    pub(crate) fn new(serving_peer: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            serving_peer: serving_peer.into(),
            name: name.into(),
            endpoint: None,
        }
    }

    /// Attach a direct dial endpoint to the target.
    pub(crate) fn with_endpoint(mut self, endpoint: Option<DesktopEndpoint>) -> Self {
        self.endpoint = endpoint;
        self
    }
}

/// Broker lifecycle metadata attached to a mesh-brokered desktop connect. The
/// Chooser mints this with the broker `Open`; the live transport publishes
/// `active` / `disconnect` / `close` against the same id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BrokerSessionLifecycle {
    /// The broker roster key.
    pub(crate) id: String,
    /// The local Bus root that accepts `action/vdi/session` lifecycle writes.
    pub(crate) bus_root: Option<PathBuf>,
}

impl BrokerSessionLifecycle {
    /// Attach the minted broker id to the Bus root used for its `Open`.
    pub(crate) fn new(id: impl Into<String>, bus_root: Option<PathBuf>) -> Self {
        Self {
            id: id.into(),
            bus_root,
        }
    }
}

/// Shell surface that owns a retained broker session when it is focused.
/// Browser sessions return to the guest-Chromium boundary; every other desktop
/// session returns to the ordinary Desktop surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionFocusSurface {
    Browser,
    Desktop,
}

/// The desktop protocol a connect routes to — the VDI tier's *routable* set. The
/// Chooser's wire [`crate::chooser::Protocol`] additionally carries an `Unknown`
/// badge for a tag this build can't render; only a routable protocol reaches a
/// [`ConnectRequest`], so this enum has no unknown arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VdiProtocol {
    /// Guest-owned Cuttlefish WebRTC display. Remote Sessions authorization is
    /// supported; a seat-side decoder remains an explicit runtime capability.
    WebRtc,
    /// Sunshine stream consumed by Moonlight. The typed route exists now; the
    /// host decoder remains honestly gated until its live adapter is present.
    Moonlight,
    /// Remote Desktop Protocol — `mde-vdi-rdp` (the primary).
    Rdp,
    /// VNC / RFB — `mde-vdi-vnc` (the universal console fallback).
    Vnc,
    /// Spice — `mde-vdi-spice` (native QEMU/KVM console).
    Spice,
}

impl VdiProtocol {
    /// The decoder crate this protocol renders through.
    pub(crate) const fn client_crate(self) -> &'static str {
        match self {
            Self::WebRtc => "Cuttlefish WebRTC adapter",
            Self::Moonlight => "Moonlight adapter",
            Self::Rdp => "mde-vdi-rdp",
            Self::Vnc => "mde-vdi-vnc",
            Self::Spice => "mde-vdi-spice",
        }
    }

    /// Whether a decoder crate exists to render this protocol today.
    pub(crate) const fn has_client(self) -> bool {
        matches!(self, Self::Rdp | Self::Vnc | Self::Spice)
    }

    /// The short picker / caption label.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::WebRtc => "WebRTC",
            Self::Moonlight => "Sunshine/Moonlight",
            Self::Rdp => "RDP",
            Self::Vnc => "VNC",
            Self::Spice => "Spice",
        }
    }

    /// Operator-facing text for the text-clipboard lane behind this protocol.
    /// Keep this beside the routing label so the chooser/connecting surface
    /// cannot imply that every decoder has the same guest integration. RFB
    /// cut-text, RDP CLIPRDR, and SPICE vdagent are wired for bounded text.
    pub(crate) const fn clipboard_summary(self) -> &'static str {
        match self {
            Self::WebRtc => "clipboard unavailable: Cuttlefish WebRTC adapter is not attached",
            Self::Moonlight => "clipboard unavailable: Moonlight adapter is not attached",
            Self::Rdp => "clipboard: bidirectional RDP CLIPRDR text + HTML",
            Self::Vnc => "clipboard: bidirectional RFB cut text",
            Self::Spice => "clipboard: bidirectional SPICE vdagent UTF-8 text",
        }
    }
}

/// Fullscreen under the thin chrome bar (the E12 VDI idiom) or a windowed desktop
/// — a per-connection choice (design lock 9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DisplayMode {
    /// The desktop fills the shell body under the thin chrome bar.
    Fullscreen,
    /// The desktop runs in a window inside the shell.
    Windowed,
}

impl DisplayMode {
    /// The picker / caption label.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Fullscreen => "fullscreen",
            Self::Windowed => "windowed",
        }
    }
}

/// Span the guest across every local display or confine it to a single one — a
/// per-connection choice (design lock 12).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MonitorSpan {
    /// A single display.
    Single,
    /// Span all local displays.
    All,
}

impl MonitorSpan {
    /// The picker / caption label.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Single => "single display",
            Self::All => "span all displays",
        }
    }
}

/// A fully-specified desktop connect the Chooser's always-ask picker produces
/// (CHOOSER-4): the chosen [`VdiProtocol`] (always-asked when a source offered
/// several — lock 6), the [`DisplayMode`] (lock 9), the [`MonitorSpan`] (lock
/// 12), and the [`RequestedTarget`] the session attaches to. The Desktop surface
/// routes it to the matching decoder crate ([`VdiProtocol::client_crate`]); the
/// live wire transport that constructs the session is the gated E12-4 layer; a
/// request is still built truthfully while the transport resolves, and no
/// placeholder session is ever faked (§7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConnectRequest {
    /// The desktop the session attaches to (serving peer + VM/host name).
    pub target: RequestedTarget,
    /// The protocol the operator chose.
    pub protocol: VdiProtocol,
    /// Fullscreen vs windowed (lock 9).
    pub display: DisplayMode,
    /// Single vs span-all (lock 12).
    pub monitors: MonitorSpan,
    /// CHOOSER-6 — how the connect authenticates: mesh-identity SSO for a
    /// mesh-brokered source, or a sealed credential for an external endpoint. The
    /// gated live transport (E12-4) feeds a sealed credential's secret into the
    /// protocol config's password field; a mesh identity needs no prompt. The
    /// secret is redacted from `Debug` ([`DesktopAuth`]), so this request is
    /// log-safe.
    pub auth: DesktopAuth,
    /// Optional catalog identity for an App VM handoff. This is presentation
    /// metadata only; the broker session id remains the authority for routing.
    /// `None` preserves the ordinary whole-desktop chooser path.
    pub app_id: Option<String>,
    /// Exact guest-owned Android source carried through the authorized Remote
    /// Sessions route. It is identity evidence only; VDI never parses its host
    /// and port into a raw dial target.
    pub android_source: Option<AndroidVdiSource>,
    /// Optional broker lifecycle handle for mesh-rostered sessions. Direct
    /// off-mesh endpoints leave this empty.
    pub broker_session: Option<BrokerSessionLifecycle>,
    /// vdi-vm-8 — the guest desktop size hint in **device pixels** (the shell's real
    /// output size at connect time, [`body_device_px`]), so an RDP/SPICE guest renders
    /// at near-native resolution instead of a hardcoded 1024×768 that egui upscales
    /// (blurry on modern seats). RDP/SPICE pass it at connect ([`with_resolution`] /
    /// [`with_size`]); VNC's size is server-negotiated so it is ignored there. When
    /// absent (bus-driven / test paths) the transport falls back to its prior
    /// hardcoded size.
    ///
    /// On a MATERIAL panel resize *after* connect (a seat / monitor resolution change,
    /// not a chrome toggle) an RDP/SPICE session is re-dialed at the new panel size —
    /// the only live re-negotiation the thin transports expose (a fresh connect;
    /// `note_resize_target` + `poll_resize_renegotiate`). The LINEAR upscale bridges
    /// the sub-second re-dial gap and remains the fallback for smaller deltas and for
    /// VNC (server-authoritative). The pointer transform keeps clicks correct
    /// throughout.
    ///
    /// [`body_device_px`]: crate::vdi::body_device_px
    /// [`with_resolution`]: mde_vdi_rdp::RdpConfig::with_resolution
    /// [`with_size`]: mde_vdi_spice::SpiceConfig::with_size
    pub preferred_size: Option<(u16, u16)>,
}

impl ConnectRequest {
    /// Assemble a request from the picked target + the three display choices + the
    /// resolved auth (CHOOSER-6).
    pub(crate) const fn new(
        target: RequestedTarget,
        protocol: VdiProtocol,
        display: DisplayMode,
        monitors: MonitorSpan,
        auth: DesktopAuth,
    ) -> Self {
        Self {
            target,
            protocol,
            display,
            monitors,
            auth,
            app_id: None,
            android_source: None,
            broker_session: None,
            preferred_size: None,
        }
    }

    /// Attach the broker session lifecycle id minted by discovery.
    pub(crate) fn with_broker_session(mut self, broker: BrokerSessionLifecycle) -> Self {
        self.broker_session = Some(broker);
        self
    }

    /// Mark this brokered desktop as a catalog-backed App VM surface. The app
    /// identity is bounded and display-only here; provisioning and admission
    /// already happened through the typed Workloads declaration.
    pub(crate) fn with_app_id(mut self, app_id: impl Into<String>) -> Self {
        self.app_id = Some(app_id.into());
        self
    }

    /// Preserve a governed Android source without converting it into a raw
    /// endpoint. The broker session remains the attachment authority.
    fn with_android_source(mut self, source: AndroidVdiSource) -> Self {
        self.android_source = Some(source);
        self
    }

    /// Attach the initial desktop size hint (device pixels) for RDP/SPICE
    /// negotiation (vdi-vm-8). `None` keeps the transport's fallback size.
    #[must_use]
    pub(crate) const fn with_preferred_size(mut self, size: Option<(u16, u16)>) -> Self {
        self.preferred_size = size;
        self
    }
}

const fn request_focus_surface(_request: &ConnectRequest) -> SessionFocusSurface {
    SessionFocusSurface::Desktop
}

#[cfg(feature = "live-vdi")]
enum LiveRdpEvent {
    Connected(String),
    ClipboardPublished,
    ClipboardFilesMaterialized {
        count: usize,
        destination: String,
    },
    ClipboardRefused(RdpGuestImageRefusal),
    /// The host's TLS certificate changed since it was pinned (vdi-vm-6) — a
    /// non-fatal MITM warning; the session stays live (the Nebula link is the
    /// trust floor). Strict mode instead surfaces as [`LiveRdpEvent::Error`].
    CertWarning(String),
    Error(String),
    Ended(String),
}

#[cfg(feature = "live-vdi")]
struct LiveRdpHandle {
    input: SharedInputMailbox,
    stop_tx: mpsc::Sender<()>,
    event_rx: mpsc::Receiver<LiveRdpEvent>,
    frame_mailbox: LatestFrameMailbox,
}

#[cfg(feature = "live-vdi")]
enum LiveVncEvent {
    Connected(String),
    ClipboardPublished,
    Error(String),
    Ended(String),
}

#[cfg(feature = "live-vdi")]
struct LiveVncHandle {
    input: SharedInputMailbox,
    stop_tx: mpsc::Sender<()>,
    event_rx: mpsc::Receiver<LiveVncEvent>,
    frame_mailbox: LatestFrameMailbox,
}

#[cfg(feature = "live-vdi")]
enum LiveSpiceEvent {
    Connected(String),
    ClipboardPublished,
    ClipboardStatus(String),
    Error(String),
    Ended(String),
}

#[cfg(feature = "live-vdi")]
struct LiveSpiceHandle {
    input: SharedInputMailbox,
    stop_tx: mpsc::Sender<()>,
    event_rx: mpsc::Receiver<LiveSpiceEvent>,
    frame_mailbox: LatestFrameMailbox,
}

/// The live decoder may outpace the egui seat. Keep only the newest decoded
/// frame so a stalled consumer cannot turn the transport handoff into an
/// unbounded framebuffer queue.
#[cfg(feature = "live-vdi")]
#[derive(Clone, Default)]
struct LatestFrameMailbox(Arc<Mutex<Option<(egui::ColorImage, FrameDamage)>>>);

#[cfg(feature = "live-vdi")]
impl LatestFrameMailbox {
    fn publish(&self, frame: egui::ColorImage, damage: FrameDamage) {
        let mut slot = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *slot = Some((frame, damage));
    }

    fn take(&self) -> Option<(egui::ColorImage, FrameDamage)> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }
}

/// Bounded input admission for a live transport. Pointer motion and wheel
/// events are coalescible; key/button transitions are retained ahead of those
/// low-value events, with releases receiving priority when the queue is full.
#[cfg(feature = "live-vdi")]
const LIVE_INPUT_QUEUE_CAPACITY: usize = 256;

#[cfg(feature = "live-vdi")]
const LIVE_INPUT_TEXT_MAX_BYTES: usize = 64 * 1024;

#[cfg(feature = "live-vdi")]
#[derive(Default)]
struct BoundedInputMailbox {
    queue: VecDeque<egui::Event>,
    dropped: u64,
}

#[cfg(feature = "live-vdi")]
type SharedInputMailbox = Arc<Mutex<BoundedInputMailbox>>;

#[cfg(feature = "live-vdi")]
fn new_input_mailbox() -> SharedInputMailbox {
    Arc::new(Mutex::new(BoundedInputMailbox::default()))
}

#[cfg(feature = "live-vdi")]
impl BoundedInputMailbox {
    fn push(&mut self, event: egui::Event) -> bool {
        let Some(event) = bound_live_input(event) else {
            self.dropped = self.dropped.saturating_add(1);
            return false;
        };
        if self.queue.len() < LIVE_INPUT_QUEUE_CAPACITY {
            self.queue.push_back(event);
            return true;
        }

        if is_coalescible_input(&event) {
            if let Some(existing) = self
                .queue
                .iter_mut()
                .rev()
                .find(|existing| is_same_coalescible_kind(existing, &event))
            {
                *existing = event;
                return true;
            }
            self.dropped = self.dropped.saturating_add(1);
            return false;
        }

        // Make room for a transition before admitting another critical event.
        // Releases are allowed to evict an older non-release transition so a
        // stuck key/button cannot be hidden behind motion or wheel flood.
        let evict = if is_release_input(&event) {
            self.queue
                .iter()
                .position(|queued| is_coalescible_input(queued))
                .or_else(|| {
                    self.queue
                        .iter()
                        .position(|queued| !is_release_input(queued))
                })
        } else {
            self.queue
                .iter()
                .position(|queued| is_coalescible_input(queued))
        };
        if let Some(index) = evict {
            self.queue.remove(index);
            self.queue.push_back(event);
            return true;
        }

        self.dropped = self.dropped.saturating_add(1);
        false
    }

    fn drain(&mut self) -> Vec<egui::Event> {
        self.queue.drain(..).collect()
    }
}

#[cfg(feature = "live-vdi")]
fn bound_live_input(event: egui::Event) -> Option<egui::Event> {
    match &event {
        egui::Event::Text(text) if text.len() > LIVE_INPUT_TEXT_MAX_BYTES => None,
        _ => Some(event),
    }
}

#[cfg(feature = "live-vdi")]
fn is_coalescible_input(event: &egui::Event) -> bool {
    matches!(
        event,
        egui::Event::PointerMoved(_) | egui::Event::MouseWheel { .. }
    )
}

#[cfg(feature = "live-vdi")]
fn is_same_coalescible_kind(left: &egui::Event, right: &egui::Event) -> bool {
    matches!(
        (left, right),
        (egui::Event::PointerMoved(_), egui::Event::PointerMoved(_))
            | (
                egui::Event::MouseWheel { .. },
                egui::Event::MouseWheel { .. }
            )
    )
}

#[cfg(feature = "live-vdi")]
fn is_release_input(event: &egui::Event) -> bool {
    matches!(
        event,
        egui::Event::Key { pressed: false, .. } | egui::Event::PointerButton { pressed: false, .. }
    )
}

#[cfg(feature = "live-vdi")]
impl LiveRdpHandle {
    fn spawn(
        request: &ConnectRequest,
        clipboard_permissions: Option<ClipboardPermissionIngress>,
    ) -> Result<Self, String> {
        let Some(endpoint) = request.target.endpoint.clone() else {
            return Err("discovery has not published a dialable endpoint for this desktop".into());
        };
        let credential = live_rdp_credential(request)?;
        if credential.username.trim().is_empty() {
            return Err("RDP requires a username in the sealed desktop credential".into());
        }

        let (width, height) = rdp_initial_resolution(request.preferred_size);
        let config = RdpConfig::new(
            endpoint.host.clone(),
            credential.username.clone(),
            credential.secret.expose().to_owned(),
        )
        .with_port(endpoint.port)
        .with_resolution(width, height);
        let clipboard_root = request
            .broker_session
            .as_ref()
            .and_then(|broker| broker.bus_root.clone())
            .or_else(mde_bus::client_data_dir);
        let clipboard_source = vdi_clipboard_source(request, "rdp");
        let clipboard_lease = vdi_clipboard_lease("rdp", &clipboard_source, unix_time_ms())?;
        let input = new_input_mailbox();
        let (stop_tx, stop_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let frame_mailbox = LatestFrameMailbox::default();

        thread::Builder::new()
            .name(format!("mde-live-rdp-{}", request.target.name))
            .spawn({
                let input = input.clone();
                let frame_mailbox = frame_mailbox.clone();
                move || {
                    run_live_rdp(
                        config,
                        input,
                        stop_rx,
                        event_tx,
                        clipboard_root,
                        clipboard_source,
                        clipboard_lease,
                        clipboard_permissions,
                        frame_mailbox,
                    )
                }
            })
            .map_err(|e| format!("failed to spawn live RDP worker: {e}"))?;

        Ok(Self {
            input,
            stop_tx,
            event_rx,
            frame_mailbox,
        })
    }

    fn send_input(&self, event: egui::Event) {
        let accepted = self
            .input
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(event);
        if !accepted {
            tracing::debug!("dropping excess live RDP input after bounded admission");
        }
    }

    fn stop(&self) {
        let _ = self.stop_tx.send(());
    }
}

#[cfg(feature = "live-vdi")]
impl LiveVncHandle {
    fn spawn(
        request: &ConnectRequest,
        clipboard_permissions: Option<ClipboardPermissionIngress>,
    ) -> Result<Self, String> {
        let config = live_vnc_config(request)?;
        let clipboard_root = request
            .broker_session
            .as_ref()
            .and_then(|broker| broker.bus_root.clone())
            .or_else(mde_bus::client_data_dir);
        let clipboard_source = vnc_clipboard_source(request);
        let clipboard_lease = vnc_clipboard_lease(&clipboard_source, unix_time_ms())?;
        let input = new_input_mailbox();
        let (stop_tx, stop_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let frame_mailbox = LatestFrameMailbox::default();

        let worker_input = input.clone();
        let worker_frame_mailbox = frame_mailbox.clone();
        thread::Builder::new()
            .name(format!("mde-live-vnc-{}", request.target.name))
            .spawn(move || {
                run_live_vnc(
                    config,
                    worker_input,
                    stop_rx,
                    event_tx,
                    clipboard_root,
                    clipboard_source,
                    clipboard_lease,
                    clipboard_permissions,
                    worker_frame_mailbox,
                )
            })
            .map_err(|e| format!("failed to spawn live VNC worker: {e}"))?;

        Ok(Self {
            input,
            stop_tx,
            event_rx,
            frame_mailbox,
        })
    }

    fn send_input(&self, event: egui::Event) {
        let accepted = self
            .input
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(event);
        if !accepted {
            tracing::debug!("dropping excess live VNC input after bounded admission");
        }
    }

    fn stop(&self) {
        let _ = self.stop_tx.send(());
    }
}

#[cfg(feature = "live-vdi")]
impl LiveSpiceHandle {
    fn spawn(
        request: &ConnectRequest,
        clipboard_permissions: Option<ClipboardPermissionIngress>,
    ) -> Result<Self, String> {
        let config = live_spice_config(request)?;
        let clipboard_root = request
            .broker_session
            .as_ref()
            .and_then(|broker| broker.bus_root.clone())
            .or_else(mde_bus::client_data_dir);
        let clipboard_source = vdi_clipboard_source(request, "spice");
        let clipboard_lease = vdi_clipboard_lease("spice", &clipboard_source, unix_time_ms())?;
        let input = new_input_mailbox();
        let (stop_tx, stop_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let frame_mailbox = LatestFrameMailbox::default();

        thread::Builder::new()
            .name(format!("mde-live-spice-{}", request.target.name))
            .spawn({
                let input = input.clone();
                let frame_mailbox = frame_mailbox.clone();
                move || {
                    run_live_spice(
                        config,
                        input,
                        stop_rx,
                        event_tx,
                        clipboard_root,
                        clipboard_source,
                        clipboard_lease,
                        clipboard_permissions,
                        frame_mailbox,
                    )
                }
            })
            .map_err(|e| format!("failed to spawn live SPICE worker: {e}"))?;

        Ok(Self {
            input,
            stop_tx,
            event_rx,
            frame_mailbox,
        })
    }

    fn send_input(&self, event: egui::Event) {
        let accepted = self
            .input
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(event);
        if !accepted {
            tracing::debug!("dropping excess live SPICE input after bounded admission");
        }
    }

    fn stop(&self) {
        let _ = self.stop_tx.send(());
    }
}

#[cfg(feature = "live-vdi")]
fn live_rdp_credential(request: &ConnectRequest) -> Result<&crate::auth::Credential, String> {
    match &request.auth {
        DesktopAuth::Sealed { credential, .. } => Ok(credential),
        DesktopAuth::MeshIdentity {
            guest: Some(guest), ..
        } => Ok(&guest.credential),
        DesktopAuth::MeshIdentity { guest: None, .. } => {
            Err("mesh-gated RDP needs a sealed guest credential for OS login".into())
        }
    }
}

#[cfg(feature = "live-vdi")]
fn live_vnc_config(request: &ConnectRequest) -> Result<VncConfig, String> {
    let Some(endpoint) = request.target.endpoint.clone() else {
        return Err("discovery has not published a dialable endpoint for this desktop".into());
    };
    let mut config = VncConfig::new(endpoint.host)
        .with_port(endpoint.port)
        .shared(true);
    match &request.auth {
        DesktopAuth::Sealed { credential, .. } => {
            if !credential.secret.expose().is_empty() {
                config = config.with_password(credential.secret.expose().to_owned());
            }
        }
        DesktopAuth::MeshIdentity {
            guest: Some(guest), ..
        } => {
            if !guest.credential.secret.expose().is_empty() {
                config = config.with_password(guest.credential.secret.expose().to_owned());
            }
        }
        DesktopAuth::MeshIdentity { guest: None, .. } => {
            // XCP-ng console fallback is mesh/dom0-route gated and usually exposes
            // RFB security type None; no guest credential is required for that path.
        }
    }
    Ok(config)
}

#[cfg(feature = "live-vdi")]
const CLIPBOARD_CAPTURE_TOPIC: &str = "event/clipboard/clip";

/// Stable source identity for one attached VNC desktop. Including the broker
/// lifecycle id prevents a guest cut from being mistaken for a host/seat copy
/// and makes a reconnect of the same desktop retain truthful attribution.
#[cfg(feature = "live-vdi")]
fn vnc_clipboard_source(request: &ConnectRequest) -> String {
    vdi_clipboard_source(request, "vnc")
}

#[cfg(feature = "live-vdi")]
fn vdi_clipboard_source(request: &ConnectRequest, protocol: &str) -> String {
    let session = request
        .broker_session
        .as_ref()
        .map(|broker| broker.id.as_str())
        .unwrap_or(request.target.name.as_str());
    format!("{protocol}:{}:{session}", request.target.serving_peer)
}

#[cfg(feature = "live-vdi")]
fn unix_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64)
}

/// Mint a process-monotonic attachment generation. The wall-clock seed keeps a
/// shell restart from reopening a prior generation; the atomic increment makes
/// same-tick reconnects distinct.
#[cfg(feature = "live-vdi")]
fn next_vdi_clipboard_generation(now_ms: u64) -> u64 {
    static LAST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seed = now_ms.saturating_mul(1_000).max(1);
    LAST.fetch_update(
        std::sync::atomic::Ordering::AcqRel,
        std::sync::atomic::Ordering::Acquire,
        |previous| Some(seed.max(previous.saturating_add(1))),
    )
    .map_or(seed, |previous| seed.max(previous.saturating_add(1)))
}

#[cfg(feature = "live-vdi")]
fn vnc_clipboard_lease(session_id: &str, now_ms: u64) -> Result<VdiClipboardLeaseV2, String> {
    vdi_clipboard_lease("vnc", session_id, now_ms)
}

#[cfg(feature = "live-vdi")]
fn vdi_clipboard_lease(
    protocol: &str,
    session_id: &str,
    now_ms: u64,
) -> Result<VdiClipboardLeaseV2, String> {
    let generation = next_vdi_clipboard_generation(now_ms);
    let permitted_mime_offers = if protocol.eq_ignore_ascii_case("rdp") {
        vec![
            VDI_GUEST_FILES_MIME.into(),
            "image/png".into(),
            "image/jpeg".into(),
            "text/html;charset=utf-8".into(),
            "text/html".into(),
            "text/plain;charset=utf-8".into(),
            "text/plain".into(),
        ]
    } else {
        vec!["text/plain;charset=utf-8".into(), "text/plain".into()]
    };
    let lease = VdiClipboardLeaseV2 {
        schema_version: VDI_CLIPBOARD_TRANSPORT_V2_SCHEMA_VERSION,
        session_id: session_id.to_owned(),
        generation,
        lease_id: format!("{protocol}-clip-{generation}"),
        issued_at_ms: now_ms,
        expires_at_ms: now_ms.saturating_add(MAX_VDI_CLIPBOARD_LEASE_TTL_MS),
        permitted_mime_offers,
    };
    lease.validate_at(now_ms).map_err(|error| {
        format!(
            "{} clipboard lease refused: {error}",
            protocol.to_uppercase()
        )
    })?;
    Ok(lease)
}

#[cfg(feature = "live-vdi")]
fn renew_vnc_clipboard_lease(
    previous: &VdiClipboardLeaseV2,
    now_ms: u64,
) -> Result<VdiClipboardLeaseV2, String> {
    renew_vdi_clipboard_lease("vnc", previous, now_ms)
}

#[cfg(feature = "live-vdi")]
fn renew_vdi_clipboard_lease(
    protocol: &str,
    previous: &VdiClipboardLeaseV2,
    now_ms: u64,
) -> Result<VdiClipboardLeaseV2, String> {
    let lease = VdiClipboardLeaseV2 {
        schema_version: VDI_CLIPBOARD_TRANSPORT_V2_SCHEMA_VERSION,
        session_id: previous.session_id.clone(),
        generation: previous.generation,
        lease_id: format!("{protocol}-clip-{}-{now_ms}", previous.generation),
        issued_at_ms: now_ms,
        expires_at_ms: now_ms.saturating_add(MAX_VDI_CLIPBOARD_LEASE_TTL_MS),
        permitted_mime_offers: previous.permitted_mime_offers.clone(),
    };
    lease.validate_at(now_ms).map_err(|error| {
        format!(
            "{} clipboard lease renewal refused: {error}",
            protocol.to_uppercase()
        )
    })?;
    Ok(lease)
}

#[cfg(feature = "live-vdi")]
fn vnc_guest_clipboard_message(
    lease: &VdiClipboardLeaseV2,
    message_sequence: u64,
    text: String,
    now_ms: u64,
) -> Result<VdiClipboardMessageV2, String> {
    vdi_guest_clipboard_message("vnc", lease, message_sequence, text, now_ms)
}

#[cfg(feature = "live-vdi")]
fn vdi_guest_clipboard_message(
    protocol: &str,
    lease: &VdiClipboardLeaseV2,
    message_sequence: u64,
    text: String,
    now_ms: u64,
) -> Result<VdiClipboardMessageV2, String> {
    let expires_at_ms = now_ms.saturating_add(60_000).min(lease.expires_at_ms);
    let envelope = ClipboardEnvelopeV2::new_inline_text(
        "vdi-guest",
        protocol,
        lease.session_id.clone(),
        message_sequence,
        now_ms,
        vec!["text/plain;charset=utf-8".into()],
        "",
        VdiClipboardText::new(text).map_err(|error| {
            format!(
                "{} guest clipboard refused: {error}",
                protocol.to_uppercase()
            )
        })?,
        expires_at_ms,
    )
    .map_err(|error| {
        format!(
            "{} guest clipboard refused: {error}",
            protocol.to_uppercase()
        )
    })?;
    let message = VdiClipboardMessageV2 {
        schema_version: VDI_CLIPBOARD_TRANSPORT_V2_SCHEMA_VERSION,
        session_id: lease.session_id.clone(),
        generation: lease.generation,
        lease_id: lease.lease_id.clone(),
        lease_expires_at_ms: lease.expires_at_ms,
        message_sequence,
        selected_mime: "text/plain;charset=utf-8".into(),
        disclosure: VdiClipboardDisclosureV2::Shareable,
        envelope,
    };
    message.admit(lease, None, now_ms).map_err(|error| {
        format!(
            "{} guest clipboard refused: {error}",
            protocol.to_uppercase()
        )
    })?;
    Ok(message)
}

#[cfg(feature = "live-vdi")]
fn rdp_guest_html_clipboard_message(
    lease: &VdiClipboardLeaseV2,
    message_sequence: u64,
    html: String,
    now_ms: u64,
) -> Result<VdiClipboardMessageV2, String> {
    let expires_at_ms = now_ms.saturating_add(60_000).min(lease.expires_at_ms);
    let envelope = ClipboardEnvelopeV2::new_inline_text(
        "vdi-guest",
        "rdp",
        lease.session_id.clone(),
        message_sequence,
        now_ms,
        vec!["text/html;charset=utf-8".into()],
        "",
        VdiClipboardText::new(html)
            .map_err(|error| format!("RDP guest HTML clipboard refused: {error}"))?,
        expires_at_ms,
    )
    .map_err(|error| format!("RDP guest HTML clipboard refused: {error}"))?;
    let message = VdiClipboardMessageV2 {
        schema_version: VDI_CLIPBOARD_TRANSPORT_V2_SCHEMA_VERSION,
        session_id: lease.session_id.clone(),
        generation: lease.generation,
        lease_id: lease.lease_id.clone(),
        lease_expires_at_ms: lease.expires_at_ms,
        message_sequence,
        selected_mime: "text/html;charset=utf-8".into(),
        disclosure: VdiClipboardDisclosureV2::Shareable,
        envelope,
    };
    message
        .admit(lease, None, now_ms)
        .map_err(|error| format!("RDP guest HTML clipboard refused: {error}"))?;
    Ok(message)
}

#[cfg(feature = "live-vdi")]
fn publish_vdi_clipboard_lease(root: &Path, lease: &VdiClipboardLeaseV2) -> Result<(), String> {
    let topic = vdi_clipboard_session_topic(VDI_CLIPBOARD_LEASE_TOPIC_PREFIX, &lease.session_id)
        .map_err(|error| error.to_string())?;
    let body = serde_json::to_string(lease).map_err(|_| "lease encoding failed".to_owned())?;
    Persist::open(root.to_path_buf())
        .map_err(|error| format!("could not open clipboard Bus: {error}"))?
        .write(&topic, Priority::Default, None, Some(&body))
        .map_err(|error| format!("clipboard lease publish failed: {error}"))?;
    Ok(())
}

#[cfg(feature = "live-vdi")]
fn read_vdi_clipboard_receipt(
    persist: &Persist,
    lease: &VdiClipboardLeaseV2,
) -> Result<Option<VdiClipboardReceiptV2>, String> {
    let topic = vdi_clipboard_session_topic(VDI_CLIPBOARD_RECEIPT_TOPIC_PREFIX, &lease.session_id)
        .map_err(|error| error.to_string())?;
    let Some(record) = persist
        .read_latest(&topic)
        .map_err(|error| format!("clipboard receipt read failed: {error}"))?
    else {
        return Ok(None);
    };
    let body = record
        .body
        .as_deref()
        .ok_or_else(|| "clipboard receipt omitted its body".to_owned())?;
    let receipt: VdiClipboardReceiptV2 =
        serde_json::from_str(body).map_err(|_| "clipboard receipt is malformed".to_owned())?;
    receipt
        .validate()
        .map_err(|error| format!("clipboard receipt refused: {error}"))?;
    if receipt.session_id != lease.session_id {
        return Ok(None);
    }
    Ok(Some(receipt))
}

#[cfg(feature = "live-vdi")]
#[derive(Debug, Clone, PartialEq, Eq)]
enum RdpClipboardPayload {
    Text(String),
    Html(String),
    Image,
    File {
        descriptor: VdiClipboardFileDescriptorV1,
    },
}

#[cfg(feature = "live-vdi")]
const VDI_GUEST_FILES_INGEST_SOCKET: &str = "vdi-clipboard-guest-files.sock";
#[cfg(feature = "live-vdi")]
const VDI_GUEST_FILES_PACKET_BYTES: usize = 384 * 1024;
#[cfg(feature = "live-vdi")]
const VDI_GUEST_FILES_MIME: &str = "application/x-mde-file-list";

#[cfg(feature = "live-vdi")]
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum RdpGuestFilesRequest {
    Begin {
        transaction_id: String,
        session_id: String,
        files: Vec<VdiClipboardFileDescriptorV1>,
        total_bytes: u64,
    },
    Chunk {
        transaction_id: String,
        file_index: usize,
        offset: u64,
        data_base64: String,
        complete: bool,
    },
    Commit {
        transaction_id: String,
    },
    Cancel {
        transaction_id: String,
    },
}

#[cfg(feature = "live-vdi")]
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
enum RdpGuestFilesResponse {
    Ready {
        transaction_id: String,
        next_file_index: usize,
        next_offset: u64,
    },
    Staged {
        transaction_id: String,
        content_hash: String,
        byte_count: u64,
        files_reference: String,
    },
    Committed {
        transaction_id: String,
        destination: String,
        file_count: usize,
    },
    Cancelled {
        transaction_id: String,
    },
    Refused {
        transaction_id: String,
        reason: String,
    },
}

#[cfg(feature = "live-vdi")]
struct RdpGuestFilesTransfer {
    transaction_id: String,
    files: Vec<VdiClipboardFileDescriptorV1>,
    total_bytes: u64,
    next_file_index: usize,
    staged_message: Option<VdiClipboardMessageV2>,
    permission: Option<ClipboardGateTicket>,
}

#[cfg(feature = "live-vdi")]
impl RdpGuestFilesTransfer {
    fn from_list(
        root: &Path,
        lease: &VdiClipboardLeaseV2,
        list: &RemoteClipboardFileList,
    ) -> Result<Self, String> {
        let files = list
            .files()
            .iter()
            .map(|file| {
                VdiClipboardFileDescriptorV1::new(
                    file.name(),
                    file.relative_path().map(str::to_owned),
                    "application/octet-stream",
                    file.size(),
                )
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("RDP guest file descriptor refused: {error:?}"))?;
        let total_bytes = files.iter().try_fold(0_u64, |total, file| {
            total
                .checked_add(file.byte_count)
                .ok_or_else(|| "RDP guest file aggregate overflow".to_owned())
        })?;
        let transaction_id = uuid::Uuid::new_v4().simple().to_string();
        let response = rdp_guest_files_authority_request(
            root,
            &RdpGuestFilesRequest::Begin {
                transaction_id: transaction_id.clone(),
                session_id: lease.session_id.clone(),
                files: files.clone(),
                total_bytes,
            },
        )?;
        match response {
            RdpGuestFilesResponse::Ready {
                transaction_id: returned,
                next_file_index: 0,
                next_offset: 0,
            } if returned == transaction_id => Ok(Self {
                transaction_id,
                files,
                total_bytes,
                next_file_index: 0,
                staged_message: None,
                permission: None,
            }),
            RdpGuestFilesResponse::Refused { reason, .. } => {
                Err(format!("Files authority refused RDP guest files: {reason}"))
            }
            _ => Err("Files authority returned an invalid begin acknowledgement".into()),
        }
    }

    fn stage_chunk(
        &mut self,
        root: &Path,
        chunk: &RemoteClipboardFileChunk,
    ) -> Result<Option<(String, u64, String)>, String> {
        use base64::Engine as _;
        if chunk.file_index() != self.next_file_index {
            return Err("RDP guest file chunk crossed the active file boundary".into());
        }
        let response = rdp_guest_files_authority_request(
            root,
            &RdpGuestFilesRequest::Chunk {
                transaction_id: self.transaction_id.clone(),
                file_index: chunk.file_index(),
                offset: chunk.offset(),
                data_base64: base64::engine::general_purpose::STANDARD.encode(chunk.data()),
                complete: chunk.is_complete(),
            },
        )?;
        match response {
            RdpGuestFilesResponse::Ready {
                transaction_id,
                next_file_index,
                next_offset: 0,
            } if transaction_id == self.transaction_id
                && chunk.is_complete()
                && next_file_index == self.next_file_index.saturating_add(1) =>
            {
                self.next_file_index = next_file_index;
                Ok(None)
            }
            RdpGuestFilesResponse::Ready {
                transaction_id,
                next_file_index,
                next_offset,
            } if transaction_id == self.transaction_id
                && !chunk.is_complete()
                && next_file_index == self.next_file_index
                && next_offset == chunk.offset().saturating_add(chunk.data().len() as u64) =>
            {
                Ok(None)
            }
            RdpGuestFilesResponse::Staged {
                transaction_id,
                content_hash,
                byte_count,
                files_reference,
            } if transaction_id == self.transaction_id
                && chunk.is_complete()
                && byte_count == self.total_bytes =>
            {
                self.next_file_index = self.files.len();
                Ok(Some((content_hash, byte_count, files_reference)))
            }
            RdpGuestFilesResponse::Refused { reason, .. } => {
                Err(format!("Files authority refused RDP guest chunk: {reason}"))
            }
            _ => Err("Files authority returned an invalid chunk acknowledgement".into()),
        }
    }

    fn cancel(&self, root: &Path) {
        let _ = rdp_guest_files_authority_request(
            root,
            &RdpGuestFilesRequest::Cancel {
                transaction_id: self.transaction_id.clone(),
            },
        );
    }
}

#[cfg(feature = "live-vdi")]
fn rdp_guest_files_authority_request(
    root: &Path,
    request: &RdpGuestFilesRequest,
) -> Result<RdpGuestFilesResponse, String> {
    use rustix::net::{
        connect_unix, recv, send, socket_with, AddressFamily, RecvFlags, SendFlags, SocketAddrUnix,
        SocketFlags, SocketType,
    };
    use std::os::unix::net::UnixStream;

    let body = serde_json::to_vec(request)
        .map_err(|_| "RDP guest Files request encoding failed".to_owned())?;
    if body.is_empty() || body.len() > VDI_GUEST_FILES_PACKET_BYTES {
        return Err("RDP guest Files request exceeded its packet bound".into());
    }
    let socket = socket_with(
        AddressFamily::UNIX,
        SocketType::SEQPACKET,
        SocketFlags::CLOEXEC,
        None,
    )
    .map_err(|error| format!("RDP guest Files socket failed: {error}"))?;
    connect_unix(
        &socket,
        &SocketAddrUnix::new(root.join(VDI_GUEST_FILES_INGEST_SOCKET))
            .map_err(|error| format!("RDP guest Files address failed: {error}"))?,
    )
    .map_err(|error| format!("RDP guest Files authority unavailable: {error}"))?;
    let stream: UnixStream = socket.into();
    let peer = rustix::net::sockopt::get_socket_peercred(&stream)
        .map_err(|error| format!("RDP guest Files credentials failed: {error}"))?;
    if peer.uid.as_raw() != 0 {
        return Err("RDP guest Files endpoint is not the root daemon authority".into());
    }
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|error| format!("RDP guest Files timeout failed: {error}"))?;
    let sent = send(&stream, &body, SendFlags::empty())
        .map_err(|error| format!("RDP guest Files request failed: {error}"))?;
    if sent != body.len() {
        return Err("RDP guest Files request was short".into());
    }
    let mut response = vec![0_u8; VDI_GUEST_FILES_PACKET_BYTES + 1];
    let received = recv(&stream, &mut response, RecvFlags::empty())
        .map_err(|error| format!("RDP guest Files response failed: {error}"))?;
    if received == 0 || received > VDI_GUEST_FILES_PACKET_BYTES {
        return Err("RDP guest Files response exceeded its packet bound".into());
    }
    serde_json::from_slice(&response[..received])
        .map_err(|_| "RDP guest Files response was malformed".into())
}

#[cfg(feature = "live-vdi")]
fn rdp_guest_files_clipboard_message(
    lease: &VdiClipboardLeaseV2,
    message_sequence: u64,
    file_count: usize,
    content_hash: String,
    byte_count: u64,
    files_reference: String,
    now_ms: u64,
) -> Result<VdiClipboardMessageV2, String> {
    let expires_at_ms = now_ms.saturating_add(60_000).min(lease.expires_at_ms);
    let envelope = ClipboardEnvelopeV2::new_files(
        "vdi-guest",
        "rdp",
        lease.session_id.clone(),
        message_sequence,
        now_ms,
        vec![VDI_GUEST_FILES_MIME.into()],
        format!("{file_count} guest file(s)"),
        content_hash,
        byte_count,
        files_reference,
        expires_at_ms,
    )
    .map_err(|error| format!("RDP guest Files envelope refused: {error}"))?;
    let message = VdiClipboardMessageV2 {
        schema_version: VDI_CLIPBOARD_TRANSPORT_V2_SCHEMA_VERSION,
        session_id: lease.session_id.clone(),
        generation: lease.generation,
        lease_id: lease.lease_id.clone(),
        lease_expires_at_ms: lease.expires_at_ms,
        message_sequence,
        selected_mime: VDI_GUEST_FILES_MIME.into(),
        disclosure: VdiClipboardDisclosureV2::Shareable,
        envelope,
    };
    message
        .admit(lease, None, now_ms)
        .map_err(|error| format!("RDP guest Files message refused: {error}"))?;
    Ok(message)
}

/// Typed, non-fatal refusal for a validated guest image that cannot yet enter
/// Files. Raw DIB bytes are deliberately absent: the transport value is dropped
/// unless a daemon-owned descriptor-ingest authority can mint its CAS identity.
#[cfg(feature = "live-vdi")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct RdpGuestImageRefusal {
    wire_format: RemoteClipboardImageFormat,
    byte_count: u64,
    reason: ClipboardUnavailableReason,
}

#[cfg(feature = "live-vdi")]
impl RdpGuestImageRefusal {
    fn files_provider_unavailable(
        wire_format: RemoteClipboardImageFormat,
        byte_count: u64,
    ) -> Self {
        Self {
            wire_format,
            byte_count,
            reason: ClipboardUnavailableReason::FilesProviderUnavailable,
        }
    }
}

#[cfg(feature = "live-vdi")]
impl core::fmt::Display for RdpGuestImageRefusal {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let format = match self.wire_format {
            RemoteClipboardImageFormat::Dib => "CF_DIB",
            RemoteClipboardImageFormat::DibV5 => "CF_DIBV5",
        };
        write!(
            formatter,
            "RDP guest {format} image ({} bytes) refused: governed Files/CAS descriptor ingestion is unavailable",
            self.byte_count
        )
    }
}

/// Fail closed until the daemon exposes an inverse of its descriptor-only
/// Files materializer. This consumes the admitted transport value, retains only
/// bounded metadata, and never fabricates a Files reference or writes a path.
#[cfg(feature = "live-vdi")]
fn refuse_rdp_guest_image_without_files_ingress(
    image: RemoteClipboardImage,
) -> RdpGuestImageRefusal {
    RdpGuestImageRefusal::files_provider_unavailable(
        image.format(),
        u64::try_from(image.data().len()).unwrap_or(u64::MAX),
    )
}

// Linux's stable MSG_CTRUNC ABI bit. rustix 0.38 retains this receive-result
// flag but does not expose a named RecvFlags constant.
#[cfg(feature = "live-vdi")]
const RDP_CLIPBOARD_MSG_CTRUNC: u32 = 0x08;

#[cfg(feature = "live-vdi")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RdpClipboardMaterialization {
    Pending,
    Refused,
    Complete,
}

/// Keep RDP text, HTML, and Files-backed images behind the same one-use
/// permission decision. The
/// callbacks are invoked only for a ticket that has reached `Materialize`.
#[cfg(feature = "live-vdi")]
fn materialize_rdp_host_clipboard<E>(
    readiness: ClipboardGateReadiness,
    payload: &RdpClipboardPayload,
    mut send: impl FnMut(&RdpClipboardPayload) -> Result<(), E>,
) -> Result<RdpClipboardMaterialization, E> {
    match readiness {
        ClipboardGateReadiness::Pending => Ok(RdpClipboardMaterialization::Pending),
        ClipboardGateReadiness::Refused => Ok(RdpClipboardMaterialization::Refused),
        ClipboardGateReadiness::Materialize => {
            send(payload)?;
            Ok(RdpClipboardMaterialization::Complete)
        }
    }
}

/// Read and admit the newest typed host-to-guest command for this exact live
/// lease before selecting a protocol representation.
#[cfg(feature = "live-vdi")]
fn read_latest_host_clipboard_command(
    root: &std::path::Path,
    lease: &VdiClipboardLeaseV2,
    now_ms: u64,
) -> Result<Option<VdiClipboardMessageV2>, String> {
    let persist = Persist::open(root.to_path_buf())
        .map_err(|error| format!("could not open clipboard Bus: {error}"))?;
    let topic =
        vdi_clipboard_session_topic(VDI_CLIPBOARD_HOST_TO_GUEST_TOPIC_PREFIX, &lease.session_id)
            .map_err(|error| error.to_string())?;
    let Some(message) = persist
        .read_latest(&topic)
        .map_err(|error| format!("clipboard Bus read failed: {error}"))?
    else {
        return Ok(None);
    };
    let Some(body) = message.body.as_deref() else {
        return Ok(None);
    };
    let command = VdiClipboardMessageV2::from_json_bytes(body.as_bytes())
        .map_err(|error| format!("VDI clipboard command refused: {error}"))?;
    let receipt = read_vdi_clipboard_receipt(&persist, lease)?;
    command
        .admit(lease, receipt.as_ref(), now_ms)
        .map_err(|error| format!("VDI clipboard command refused: {error}"))?;
    Ok(Some(command))
}

/// VNC and SPICE truthfully select only the UTF-8 plain-text representation.
#[cfg(feature = "live-vdi")]
fn read_latest_vnc_host_clipboard(
    root: &std::path::Path,
    lease: &VdiClipboardLeaseV2,
    now_ms: u64,
) -> Result<Option<(VdiClipboardMessageV2, String)>, String> {
    let Some(command) = read_latest_host_clipboard_command(root, lease, now_ms)? else {
        return Ok(None);
    };
    if !command.selected_mime.eq_ignore_ascii_case("text/plain")
        && !command
            .selected_mime
            .eq_ignore_ascii_case("text/plain;charset=utf-8")
    {
        return Err("VNC clipboard command refused: protocol supports plain text only".to_owned());
    }
    let text = command
        .envelope
        .inline_text
        .as_ref()
        .map(|text| text.as_str().to_owned())
        .ok_or_else(|| "VNC clipboard command refused: protocol does not carry Files".to_owned())?;
    Ok(Some((command, text)))
}

/// RDP selects bounded inline Unicode text/HTML or one Files-backed PNG/JPEG
/// admitted by the exact current typed lease. Files bytes remain unresolved
/// until the one-use permission ticket enters `Materialize`.
#[cfg(feature = "live-vdi")]
fn read_latest_rdp_host_clipboard(
    root: &std::path::Path,
    lease: &VdiClipboardLeaseV2,
    now_ms: u64,
) -> Result<Option<(VdiClipboardMessageV2, RdpClipboardPayload)>, String> {
    let Some(command) = read_latest_host_clipboard_command(root, lease, now_ms)? else {
        return Ok(None);
    };
    let payload = if let Some(descriptor) = rdp_host_file_descriptor(&command)? {
        if command.envelope.files_reference.is_none()
            || command.envelope.inline_text.is_some()
            || command.envelope.byte_count == 0
            || command.envelope.byte_count > MAX_VDI_RDP_CLIPBOARD_IMAGE_BYTES
        {
            return Err("RDP file clipboard command has no bounded Files payload".to_owned());
        }
        RdpClipboardPayload::File { descriptor }
    } else if command.selected_mime.eq_ignore_ascii_case("text/html")
        || command
            .selected_mime
            .eq_ignore_ascii_case("text/html;charset=utf-8")
    {
        RdpClipboardPayload::Html(
            command
                .envelope
                .inline_text
                .as_ref()
                .map(|text| text.as_str().to_owned())
                .ok_or_else(|| "RDP HTML clipboard command omitted inline text".to_owned())?,
        )
    } else if command.selected_mime.eq_ignore_ascii_case("text/plain")
        || command
            .selected_mime
            .eq_ignore_ascii_case("text/plain;charset=utf-8")
    {
        RdpClipboardPayload::Text(
            command
                .envelope
                .inline_text
                .as_ref()
                .map(|text| text.as_str().to_owned())
                .ok_or_else(|| "RDP text clipboard command omitted inline text".to_owned())?,
        )
    } else if command.selected_mime.eq_ignore_ascii_case("image/png")
        || command.selected_mime.eq_ignore_ascii_case("image/jpeg")
    {
        if command.envelope.files_reference.is_none()
            || command.envelope.inline_text.is_some()
            || command.envelope.byte_count == 0
            || command.envelope.byte_count > MAX_VDI_RDP_CLIPBOARD_IMAGE_BYTES
        {
            return Err("RDP image clipboard command has no bounded Files payload".to_owned());
        }
        RdpClipboardPayload::Image
    } else {
        return Err("RDP clipboard command refused: unsupported MIME representation".to_owned());
    };
    Ok(Some((command, payload)))
}

#[cfg(feature = "live-vdi")]
fn rdp_host_file_descriptor(
    command: &VdiClipboardMessageV2,
) -> Result<Option<VdiClipboardFileDescriptorV1>, String> {
    if !command
        .envelope
        .mime_offers
        .iter()
        .any(|mime| mime.eq_ignore_ascii_case(VDI_GUEST_FILES_MIME))
    {
        return Ok(None);
    }
    VdiClipboardFileDescriptorV1::new(
        command.envelope.preview.clone(),
        None,
        command.selected_mime.clone(),
        command.envelope.byte_count,
    )
    .map(Some)
    .map_err(|error| format!("RDP file descriptor refused: {error:?}"))
}

#[cfg(feature = "live-vdi")]
/// Ask the single daemon Files authority for one descriptor after the shell's
/// one-use permission CAS. The request and response carry metadata only; image
/// bytes are read from the verified descriptor and re-hashed locally.
#[cfg(feature = "live-vdi")]
fn materialize_rdp_image_from_files(
    root: &Path,
    command: &VdiClipboardMessageV2,
) -> Result<Vec<u8>, String> {
    use rustix::net::{
        connect_unix, recvmsg, send, socket_with, AddressFamily, RecvAncillaryBuffer,
        RecvAncillaryMessage, RecvFlags, SendFlags, SocketAddrUnix, SocketFlags, SocketType,
    };
    use std::io::{IoSliceMut, Read as _};
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;

    let authorization_id = uuid::Uuid::new_v4().to_string();
    let request =
        VdiClipboardFilesMaterializationRequestV1::from_message(command, authorization_id.clone())
            .map_err(|reason| format!("RDP image materialization request refused: {reason:?}"))?;
    let body = serde_json::to_vec(&request)
        .map_err(|_| "RDP image materialization request encoding failed".to_owned())?;
    if body.is_empty() || body.len() > MAX_VDI_CLIPBOARD_FILES_MATERIALIZATION_PACKET_BYTES {
        return Err("RDP image materialization request exceeded its packet cap".to_owned());
    }

    let socket = socket_with(
        AddressFamily::UNIX,
        SocketType::SEQPACKET,
        SocketFlags::CLOEXEC,
        None,
    )
    .map_err(|error| format!("RDP image materializer socket failed: {error}"))?;
    let path = root.join(VDI_CLIPBOARD_FILES_MATERIALIZATION_SOCKET);
    let address = SocketAddrUnix::new(&path)
        .map_err(|error| format!("RDP image materializer address failed: {error}"))?;
    connect_unix(&socket, &address)
        .map_err(|error| format!("RDP image materializer unavailable: {error}"))?;
    let stream: UnixStream = socket.into();
    let peer = rustix::net::sockopt::get_socket_peercred(&stream)
        .map_err(|error| format!("RDP image materializer credentials failed: {error}"))?;
    if peer.uid.as_raw() != 0 {
        return Err("RDP image materializer is not the root daemon authority".to_owned());
    }
    stream
        .set_read_timeout(Some(Duration::from_secs(1)))
        .map_err(|error| format!("RDP image materializer timeout failed: {error}"))?;
    let sent = send(&stream, &body, SendFlags::empty())
        .map_err(|error| format!("RDP image materialization request failed: {error}"))?;
    if sent != body.len() {
        return Err("RDP image materialization request was short".to_owned());
    }

    let mut response_bytes = [0_u8; MAX_VDI_CLIPBOARD_FILES_MATERIALIZATION_PACKET_BYTES];
    let mut iov = [IoSliceMut::new(&mut response_bytes)];
    let mut control = [0_u8; rustix::cmsg_space!(ScmRights(2))];
    let mut ancillary = RecvAncillaryBuffer::new(&mut control);
    let received = recvmsg(&stream, &mut iov, &mut ancillary, RecvFlags::empty())
        .map_err(|error| format!("RDP image materialization response failed: {error}"))?;
    if received.bytes == 0
        || received.flags.contains(RecvFlags::TRUNC)
        || received.flags.bits() & RDP_CLIPBOARD_MSG_CTRUNC != 0
    {
        return Err("RDP image materialization response was truncated".to_owned());
    }
    let mut descriptor: Option<OwnedFd> = None;
    for message in ancillary.drain() {
        if let RecvAncillaryMessage::ScmRights(mut descriptors) = message {
            let first = descriptors.next();
            if descriptor.is_some() || descriptors.next().is_some() {
                return Err("RDP image materializer returned multiple descriptors".to_owned());
            }
            descriptor = first;
        }
    }
    let response: VdiClipboardFilesMaterializationResponseV1 =
        serde_json::from_slice(&response_bytes[..received.bytes])
            .map_err(|_| "RDP image materialization response was malformed".to_owned())?;
    match response {
        VdiClipboardFilesMaterializationResponseV1::Refused {
            authorization_id: returned,
            reason,
        } => {
            if descriptor.is_some() || returned != authorization_id {
                return Err(
                    "RDP image materialization refusal was not bound to the request".into(),
                );
            }
            return Err(format!("RDP image materialization refused: {reason:?}"));
        }
        VdiClipboardFilesMaterializationResponseV1::Ready {
            authorization_id: returned,
            selected_mime,
            content_hash,
            byte_count,
        } => {
            if returned != authorization_id
                || !selected_mime.eq_ignore_ascii_case(&request.selected_mime)
                || content_hash != request.content_hash
                || byte_count != request.byte_count
            {
                return Err("RDP image materialization metadata mismatch".to_owned());
            }
        }
    }
    let descriptor =
        descriptor.ok_or_else(|| "RDP image materializer omitted its descriptor".to_owned())?;
    let mut file = std::fs::File::from(descriptor);
    let metadata = file
        .metadata()
        .map_err(|error| format!("RDP image descriptor metadata failed: {error}"))?;
    if !metadata.is_file() || metadata.len() != request.byte_count {
        return Err("RDP image descriptor size/type mismatch".to_owned());
    }
    let capacity = usize::try_from(request.byte_count)
        .map_err(|_| "RDP image byte count does not fit this seat".to_owned())?;
    let mut bytes = Vec::with_capacity(capacity);
    file.by_ref()
        .take(request.byte_count.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| format!("RDP image descriptor read failed: {error}"))?;
    if bytes.len() != capacity
        || ClipboardEnvelopeV2::content_hash_for(&bytes) != request.content_hash
    {
        return Err("RDP image descriptor digest/length mismatch".to_owned());
    }
    Ok(bytes)
}

#[cfg(feature = "live-vdi")]
fn rdp_image_to_dibv5(mime: &str, bytes: &[u8]) -> Result<Vec<u8>, String> {
    let (width, height, rgba) = if mime.eq_ignore_ascii_case("image/png") {
        decode_bounded_png_rgba(bytes)?
    } else if mime.eq_ignore_ascii_case("image/jpeg") {
        decode_bounded_jpeg_rgba(bytes)?
    } else {
        return Err("RDP image MIME is unsupported".to_owned());
    };
    encode_dibv5(width, height, &rgba)
}

#[cfg(feature = "live-vdi")]
fn bounded_rgba_len(width: u32, height: u32) -> Result<usize, String> {
    let bytes = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "RDP image geometry overflow".to_owned())?;
    if width == 0 || height == 0 || bytes.saturating_add(124) > MAX_VDI_RDP_CLIPBOARD_IMAGE_BYTES {
        return Err("RDP image expands beyond the DIB ceiling".to_owned());
    }
    usize::try_from(bytes).map_err(|_| "RDP image does not fit this seat".to_owned())
}

#[cfg(feature = "live-vdi")]
fn decode_bounded_png_rgba(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let max = usize::try_from(MAX_VDI_RDP_CLIPBOARD_IMAGE_BYTES).unwrap_or(usize::MAX);
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_limits(png::Limits { bytes: max });
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder
        .read_info()
        .map_err(|error| format!("RDP PNG header refused: {error}"))?;
    let width = reader.info().width;
    let height = reader.info().height;
    let rgba_len = bounded_rgba_len(width, height)?;
    let output_len = reader
        .output_buffer_size()
        .filter(|length| *length <= max)
        .ok_or_else(|| "RDP PNG output exceeds its decoder ceiling".to_owned())?;
    let mut decoded = vec![0_u8; output_len];
    let info = reader
        .next_frame(&mut decoded)
        .map_err(|error| format!("RDP PNG decode refused: {error}"))?;
    let source = decoded
        .get(..info.buffer_size())
        .ok_or_else(|| "RDP PNG decoder returned an invalid size".to_owned())?;
    let mut rgba = Vec::with_capacity(rgba_len);
    match info.color_type {
        png::ColorType::Rgba => rgba.extend_from_slice(source),
        png::ColorType::Rgb => source
            .chunks_exact(3)
            .for_each(|pixel| rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 0xff])),
        png::ColorType::Grayscale => source
            .iter()
            .for_each(|value| rgba.extend_from_slice(&[*value, *value, *value, 0xff])),
        png::ColorType::GrayscaleAlpha => source
            .chunks_exact(2)
            .for_each(|pixel| rgba.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]])),
        png::ColorType::Indexed => return Err("RDP PNG palette was not expanded".to_owned()),
    }
    if rgba.len() != rgba_len {
        return Err("RDP PNG pixel count mismatch".to_owned());
    }
    Ok((width, height, rgba))
}

#[cfg(feature = "live-vdi")]
fn decode_bounded_jpeg_rgba(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let mut reader =
        image::ImageReader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::Jpeg);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(MAX_VDI_RDP_CLIPBOARD_IMAGE_BYTES);
    limits.max_image_width = Some(8_192);
    limits.max_image_height = Some(8_192);
    reader.limits(limits);
    let decoded = reader
        .decode()
        .map_err(|error| format!("RDP JPEG decode refused: {error}"))?
        .to_rgba8();
    let width = decoded.width();
    let height = decoded.height();
    if decoded.as_raw().len() != bounded_rgba_len(width, height)? {
        return Err("RDP JPEG pixel count mismatch".to_owned());
    }
    Ok((width, height, decoded.into_raw()))
}

#[cfg(feature = "live-vdi")]
fn encode_dibv5(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let pixel_bytes = bounded_rgba_len(width, height)?;
    if rgba.len() != pixel_bytes || width > i32::MAX as u32 || height > i32::MAX as u32 {
        return Err("RDP image pixels do not match their geometry".to_owned());
    }
    let total = 124_usize
        .checked_add(pixel_bytes)
        .ok_or_else(|| "RDP DIB allocation overflow".to_owned())?;
    let mut dib = vec![0_u8; total];
    dib[0..4].copy_from_slice(&124_u32.to_le_bytes());
    dib[4..8].copy_from_slice(&(width as i32).to_le_bytes());
    dib[8..12].copy_from_slice(&(-(height as i32)).to_le_bytes());
    dib[12..14].copy_from_slice(&1_u16.to_le_bytes());
    dib[14..16].copy_from_slice(&32_u16.to_le_bytes());
    dib[16..20].copy_from_slice(&3_u32.to_le_bytes());
    dib[20..24].copy_from_slice(&(pixel_bytes as u32).to_le_bytes());
    dib[40..44].copy_from_slice(&0x00ff_0000_u32.to_le_bytes());
    dib[44..48].copy_from_slice(&0x0000_ff00_u32.to_le_bytes());
    dib[48..52].copy_from_slice(&0x0000_00ff_u32.to_le_bytes());
    dib[52..56].copy_from_slice(&0xff00_0000_u32.to_le_bytes());
    dib[56..60].copy_from_slice(&0x7352_4742_u32.to_le_bytes());
    for (source, target) in rgba.chunks_exact(4).zip(dib[124..].chunks_exact_mut(4)) {
        target.copy_from_slice(&[source[2], source[1], source[0], source[3]]);
    }
    Ok(dib)
}

#[cfg(feature = "live-vdi")]
fn publish_vdi_clipboard_receipt(
    root: &Path,
    receipt: &VdiClipboardReceiptV2,
) -> Result<(), String> {
    receipt
        .validate()
        .map_err(|error| format!("clipboard receipt refused: {error}"))?;
    let topic =
        vdi_clipboard_session_topic(VDI_CLIPBOARD_RECEIPT_TOPIC_PREFIX, &receipt.session_id)
            .map_err(|error| error.to_string())?;
    let body = serde_json::to_string(receipt).map_err(|_| "receipt encoding failed".to_owned())?;
    Persist::open(root.to_path_buf())
        .map_err(|error| format!("could not open clipboard Bus: {error}"))?
        .write(&topic, Priority::Default, None, Some(&body))
        .map_err(|error| format!("clipboard receipt publish failed: {error}"))?;
    Ok(())
}

/// Publish one accepted guest `ServerCutText` as a lease-bound V2 event while
/// retaining the canonical text event for deployed text consumers.
#[cfg(feature = "live-vdi")]
fn try_publish_vnc_clipboard_event(
    root: Option<&std::path::Path>,
    clip: &ClipboardClipBody,
    rich: &VdiClipboardMessageV2,
) -> Result<(), String> {
    try_publish_vdi_clipboard_event(root, Some(clip), rich)
}

#[cfg(feature = "live-vdi")]
fn try_publish_vdi_clipboard_event(
    root: Option<&std::path::Path>,
    legacy_text: Option<&ClipboardClipBody>,
    rich: &VdiClipboardMessageV2,
) -> Result<(), String> {
    let Some(root) = root else {
        return Err("VDI clipboard Bus root is unavailable".to_owned());
    };
    let persist = Persist::open(root.to_path_buf())
        .map_err(|error| format!("could not open clipboard Bus: {error}"))?;
    if let Some(clip) = legacy_text {
        let body = serde_json::to_string(clip)
            .map_err(|error| format!("VDI legacy clipboard encoding failed: {error}"))?;
        persist
            .write(
                CLIPBOARD_CAPTURE_TOPIC,
                Priority::Default,
                None,
                Some(&body),
            )
            .map_err(|error| format!("VDI legacy clipboard publish failed: {error}"))?;
    }
    let topic =
        vdi_clipboard_session_topic(VDI_CLIPBOARD_GUEST_TO_HOST_TOPIC_PREFIX, &rich.session_id)
            .map_err(|error| error.to_string())?;
    let body = serde_json::to_string(rich)
        .map_err(|error| format!("VDI V2 clipboard encoding failed: {error}"))?;
    persist
        .write(&topic, Priority::Default, None, Some(&body))
        .map_err(|error| format!("VDI V2 clipboard publish failed: {error}"))?;
    Ok(())
}

#[cfg(all(feature = "live-vdi", test))]
fn publish_vnc_clipboard_event(
    root: Option<&std::path::Path>,
    clip: &ClipboardClipBody,
    rich: &VdiClipboardMessageV2,
) {
    let _ = try_publish_vnc_clipboard_event(root, clip, rich);
}

#[cfg(feature = "live-vdi")]
fn live_spice_config(request: &ConnectRequest) -> Result<SpiceConfig, String> {
    let Some(endpoint) = request.target.endpoint.clone() else {
        return Err("discovery has not published a dialable endpoint for this desktop".into());
    };
    let (width, height) = spice_initial_size(request.preferred_size);
    let mut config = SpiceConfig::new(endpoint.host)
        .with_port(endpoint.port)
        .with_size(width, height);
    match &request.auth {
        DesktopAuth::Sealed { credential, .. } => {
            if !credential.secret.expose().is_empty() {
                config = config.with_password(credential.secret.expose().to_owned());
            }
        }
        DesktopAuth::MeshIdentity {
            guest: Some(guest), ..
        } => {
            if !guest.credential.secret.expose().is_empty() {
                config = config.with_password(guest.credential.secret.expose().to_owned());
            }
        }
        DesktopAuth::MeshIdentity { guest: None, .. } => {
            // Mesh-gated QEMU/KVM consoles commonly carry no SPICE ticket; if a
            // ticket is required, discovery/auth provides it as the optional guest
            // credential above.
        }
    }
    Ok(config)
}

/// Clamp a vdi-vm-8 device-pixel size hint into a legal RDP desktop resolution.
///
/// `RdpConfig` requires 200..=8192 px per axis and an **even** width, so the hint
/// is clamped and the width forced even. Falls back to the prior hardcoded
/// 1024×768 when the shell published no hint (bus-driven / headless connect).
#[cfg(feature = "live-vdi")]
fn rdp_initial_resolution(preferred: Option<(u16, u16)>) -> (u16, u16) {
    match preferred {
        Some((w, h)) => (
            w.clamp(RdpConfig::MIN_DIM, RdpConfig::MAX_DIM) & !1u16,
            h.clamp(RdpConfig::MIN_DIM, RdpConfig::MAX_DIM),
        ),
        None => (1024, 768),
    }
}

/// Clamp a vdi-vm-8 device-pixel size hint into a legal SPICE framebuffer size.
///
/// `SpiceConfig` allows 16..=8192 px per axis. Falls back to 1024×768 when the
/// shell published no hint.
#[cfg(feature = "live-vdi")]
fn spice_initial_size(preferred: Option<(u16, u16)>) -> (u16, u16) {
    match preferred {
        Some((w, h)) => (
            w.clamp(SpiceConfig::MIN_DIM, SpiceConfig::MAX_DIM),
            h.clamp(SpiceConfig::MIN_DIM, SpiceConfig::MAX_DIM),
        ),
        None => (1024, 768),
    }
}

#[cfg(feature = "live-vdi")]
impl Drop for LiveRdpHandle {
    fn drop(&mut self) {
        let _ = self.stop_tx.send(());
    }
}

#[cfg(feature = "live-vdi")]
impl Drop for LiveVncHandle {
    fn drop(&mut self) {
        let _ = self.stop_tx.send(());
    }
}

#[cfg(feature = "live-vdi")]
impl Drop for LiveSpiceHandle {
    fn drop(&mut self) {
        let _ = self.stop_tx.send(());
    }
}

#[cfg(feature = "live-vdi")]
fn run_live_rdp(
    config: RdpConfig,
    input: SharedInputMailbox,
    stop_rx: mpsc::Receiver<()>,
    event_tx: mpsc::Sender<LiveRdpEvent>,
    clipboard_root: Option<PathBuf>,
    clipboard_source: String,
    mut clipboard_lease: VdiClipboardLeaseV2,
    clipboard_permissions: Option<ClipboardPermissionIngress>,
    frame_mailbox: LatestFrameMailbox,
) {
    let target = format!("{}:{}", config.host, config.port);
    let mut session = match RdpSession::new(config) {
        Ok(session) => session,
        Err(e) => {
            let _ = event_tx.send(LiveRdpEvent::Error(format!("RDP config rejected: {e}")));
            return;
        }
    };
    let mut conn = match RdpConnection::connect(&mut session) {
        Ok(conn) => conn,
        Err(e) => {
            let _ = event_tx.send(LiveRdpEvent::Error(format!("RDP connect failed: {e}")));
            return;
        }
    };
    let _ = event_tx.send(LiveRdpEvent::Connected(target));
    // vdi-vm-6: surface a trust-on-first-use certificate change (possible MITM)
    // as a non-fatal banner — the connection is already up on the Nebula link.
    if let Some(change) = conn.cert_pin_change() {
        let _ = event_tx.send(LiveRdpEvent::CertWarning(change.operator_message()));
    }
    if let Some((frame, damage)) = session.frame_with_damage() {
        frame_mailbox.publish(frame, damage);
    }

    if let Some(root) = clipboard_root.as_deref() {
        if let Err(error) = publish_vdi_clipboard_lease(root, &clipboard_lease) {
            let _ = event_tx.send(LiveRdpEvent::Error(error));
            return;
        }
    }

    let mut gated_host_clipboard = None::<(
        VdiClipboardMessageV2,
        RdpClipboardPayload,
        ClipboardGateTicket,
    )>;
    let mut last_gated_host_clipboard = None::<(String, u64, u64)>;
    let mut gated_guest_clipboard = None::<(
        Option<ClipboardClipBody>,
        VdiClipboardMessageV2,
        ClipboardGateTicket,
    )>;
    let mut pending_guest_clipboard = None::<(Option<ClipboardClipBody>, VdiClipboardMessageV2)>;
    let mut guest_files_transfer = None::<RdpGuestFilesTransfer>;
    let mut guest_message_sequence = 0_u64;

    loop {
        if stop_rx.try_recv().is_ok() {
            if let (Some(root), Some(transfer)) =
                (clipboard_root.as_deref(), guest_files_transfer.as_ref())
            {
                transfer.cancel(root);
            }
            let _ = conn.shutdown(&mut session);
            return;
        }

        let now_ms = unix_time_ms();
        if let Some(mut transfer) = guest_files_transfer.take() {
            let mut retain = true;
            if transfer.permission.is_none() {
                if let (Some(message), Some(ingress), Ok(target)) = (
                    transfer.staged_message.as_ref(),
                    clipboard_permissions.as_ref(),
                    ClipboardTarget::new(
                        ClipboardTargetKind::LocalSeat,
                        clipboard_lease.session_id.clone(),
                    ),
                ) {
                    match ingress.submit_vdi(message, &clipboard_lease, None, target, now_ms) {
                        Ok(ticket) => transfer.permission = Some(ticket),
                        Err(crate::clipboard_permissions::ClipboardPermissionError::Busy) => {}
                        Err(error) => {
                            if let Some(root) = clipboard_root.as_deref() {
                                transfer.cancel(root);
                            }
                            let _ = event_tx.send(LiveRdpEvent::Error(format!(
                                "RDP guest Files permission refused: {error:?}"
                            )));
                            retain = false;
                        }
                    }
                } else if transfer.staged_message.is_some() {
                    if let Some(root) = clipboard_root.as_deref() {
                        transfer.cancel(root);
                    }
                    let _ = event_tx.send(LiveRdpEvent::Error(
                        "RDP guest Files refused: clipboard permission authority is unavailable"
                            .into(),
                    ));
                    retain = false;
                }
            }
            if let Some(ticket) = transfer.permission.as_ref() {
                match ticket.try_begin_materialization() {
                    ClipboardGateReadiness::Pending => {}
                    ClipboardGateReadiness::Refused => {
                        if let Some(root) = clipboard_root.as_deref() {
                            transfer.cancel(root);
                        }
                        retain = false;
                    }
                    ClipboardGateReadiness::Materialize => {
                        let result = clipboard_root
                            .as_deref()
                            .ok_or_else(|| "VDI clipboard Bus root is unavailable".to_owned())
                            .and_then(|root| {
                                let response = rdp_guest_files_authority_request(
                                    root,
                                    &RdpGuestFilesRequest::Commit {
                                        transaction_id: transfer.transaction_id.clone(),
                                    },
                                )?;
                                match response {
                                    RdpGuestFilesResponse::Committed {
                                        transaction_id,
                                        destination,
                                        file_count,
                                    } if transaction_id == transfer.transaction_id
                                        && file_count == transfer.files.len() =>
                                    {
                                        let message = transfer.staged_message.as_ref().ok_or_else(
                                            || "RDP guest Files message disappeared".to_owned(),
                                        )?;
                                        try_publish_vdi_clipboard_event(Some(root), None, message)?;
                                        Ok((destination, file_count))
                                    }
                                    RdpGuestFilesResponse::Refused { reason, .. } => Err(format!(
                                        "Files authority refused RDP guest commit: {reason}"
                                    )),
                                    _ => Err(
                                        "Files authority returned an invalid commit acknowledgement"
                                            .into(),
                                    ),
                                }
                            });
                        match result {
                            Ok((destination, count)) => {
                                ticket.report_progress(transfer.total_bytes);
                                ticket.report_complete(now_ms);
                                let _ = event_tx.send(LiveRdpEvent::ClipboardFilesMaterialized {
                                    count,
                                    destination,
                                });
                            }
                            Err(error) => {
                                ticket.report_failure(ClipboardFailure::Transport, now_ms);
                                let _ = event_tx.send(LiveRdpEvent::Error(error));
                            }
                        }
                        retain = false;
                    }
                }
            }
            if retain {
                guest_files_transfer = Some(transfer);
            }
        }
        if gated_host_clipboard.is_none()
            && gated_guest_clipboard.is_none()
            && pending_guest_clipboard.is_none()
            && guest_files_transfer.is_none()
            && now_ms.saturating_add(30_000) >= clipboard_lease.expires_at_ms
        {
            match renew_vdi_clipboard_lease("rdp", &clipboard_lease, now_ms) {
                Ok(renewed) => {
                    clipboard_lease = renewed;
                    guest_message_sequence = 0;
                    if let Some(root) = clipboard_root.as_deref() {
                        if let Err(error) = publish_vdi_clipboard_lease(root, &clipboard_lease) {
                            let _ = event_tx.send(LiveRdpEvent::Error(error));
                            return;
                        }
                    }
                }
                Err(error) => {
                    let _ = event_tx.send(LiveRdpEvent::Error(error));
                    return;
                }
            }
        }

        if let Some((command, payload, ticket)) = gated_host_clipboard.take() {
            let readiness = ticket.try_begin_materialization();
            match materialize_rdp_host_clipboard(readiness, &payload, |payload| match payload {
                RdpClipboardPayload::Text(text) => conn.send_clipboard_to_guest(text.clone()),
                RdpClipboardPayload::Html(html) => conn.send_html_clipboard_to_guest(html.clone()),
                RdpClipboardPayload::Image => {
                    let root = clipboard_root.as_deref().ok_or_else(|| {
                        ConnectError::Clipboard(
                            "RDP image clipboard Bus root is unavailable".to_owned(),
                        )
                    })?;
                    let source = materialize_rdp_image_from_files(root, &command)
                        .map_err(ConnectError::Clipboard)?;
                    let dib = rdp_image_to_dibv5(&command.selected_mime, &source)
                        .map_err(ConnectError::Clipboard)?;
                    conn.send_dibv5_clipboard_to_guest(dib)
                }
                RdpClipboardPayload::File { descriptor } => {
                    let root = clipboard_root.as_deref().ok_or_else(|| {
                        ConnectError::Clipboard(
                            "RDP file clipboard Bus root is unavailable".to_owned(),
                        )
                    })?;
                    let source = materialize_rdp_image_from_files(root, &command)
                        .map_err(ConnectError::Clipboard)?;
                    conn.send_file_clipboard_to_guest(descriptor.name.clone(), source)
                }
            }) {
                Ok(RdpClipboardMaterialization::Pending) => {
                    gated_host_clipboard = Some((command, payload, ticket));
                }
                Ok(RdpClipboardMaterialization::Refused) => {}
                Ok(RdpClipboardMaterialization::Complete) => {
                    ticket.report_progress(command.envelope.byte_count);
                    if let Some(root) = clipboard_root.as_deref() {
                        if let Err(error) = publish_vdi_clipboard_receipt(root, &command.receipt())
                        {
                            ticket.report_failure(ClipboardFailure::Transport, now_ms);
                            let _ = event_tx.send(LiveRdpEvent::Error(error));
                            if let (Some(root), Some(transfer)) =
                                (clipboard_root.as_deref(), guest_files_transfer.as_ref())
                            {
                                transfer.cancel(root);
                            }
                            return;
                        }
                    }
                    ticket.report_complete(now_ms);
                }
                Err(error) => {
                    ticket.report_failure(ClipboardFailure::Transport, now_ms);
                    let _ = event_tx.send(LiveRdpEvent::Error(format!(
                        "RDP host clipboard refused: {error}"
                    )));
                    if let (Some(root), Some(transfer)) =
                        (clipboard_root.as_deref(), guest_files_transfer.as_ref())
                    {
                        transfer.cancel(root);
                    }
                    return;
                }
            }
        }

        if let Some((legacy, rich, ticket)) = gated_guest_clipboard.take() {
            match ticket.try_begin_materialization() {
                ClipboardGateReadiness::Pending => {
                    gated_guest_clipboard = Some((legacy, rich, ticket));
                }
                ClipboardGateReadiness::Refused => {}
                ClipboardGateReadiness::Materialize => {
                    match try_publish_vdi_clipboard_event(
                        clipboard_root.as_deref(),
                        legacy.as_ref(),
                        &rich,
                    ) {
                        Ok(()) => {
                            ticket.report_progress(rich.envelope.byte_count);
                            ticket.report_complete(now_ms);
                            let _ = event_tx.send(LiveRdpEvent::ClipboardPublished);
                        }
                        Err(_) => ticket.report_failure(ClipboardFailure::Transport, now_ms),
                    }
                }
            }
        }

        if gated_guest_clipboard.is_none() {
            if let Some((legacy, rich)) = pending_guest_clipboard.take() {
                if let (Some(ingress), Ok(target)) = (
                    clipboard_permissions.as_ref(),
                    ClipboardTarget::new(
                        ClipboardTargetKind::LocalSeat,
                        clipboard_lease.session_id.clone(),
                    ),
                ) {
                    match ingress.submit_vdi(&rich, &clipboard_lease, None, target, now_ms) {
                        Ok(ticket) => gated_guest_clipboard = Some((legacy, rich, ticket)),
                        Err(crate::clipboard_permissions::ClipboardPermissionError::Busy) => {
                            pending_guest_clipboard = Some((legacy, rich));
                        }
                        Err(_) => {}
                    }
                }
            }
        }

        if gated_host_clipboard.is_none() {
            if let Some(root) = clipboard_root.as_deref() {
                if let Ok(Some((command, payload))) =
                    read_latest_rdp_host_clipboard(root, &clipboard_lease, now_ms)
                {
                    let key = (
                        command.lease_id.clone(),
                        command.generation,
                        command.message_sequence,
                    );
                    if last_gated_host_clipboard.as_ref() != Some(&key) {
                        if let (Some(ingress), Ok(target)) = (
                            clipboard_permissions.as_ref(),
                            ClipboardTarget::new(
                                ClipboardTargetKind::Guest,
                                clipboard_lease.session_id.clone(),
                            ),
                        ) {
                            match ingress.submit_vdi(
                                &command,
                                &clipboard_lease,
                                None,
                                target,
                                now_ms,
                            ) {
                                Ok(ticket) => {
                                    last_gated_host_clipboard = Some(key);
                                    gated_host_clipboard = Some((command, payload, ticket));
                                }
                                Err(
                                    crate::clipboard_permissions::ClipboardPermissionError::Busy,
                                ) => {}
                                Err(_) => last_gated_host_clipboard = Some(key),
                            }
                        } else {
                            last_gated_host_clipboard = Some(key);
                        }
                    }
                }
            }
        }

        let mut had_input = false;
        let events = input
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .drain();
        for event in events {
            session.send_input(&event);
            had_input = true;
        }
        if had_input {
            if let Err(e) = conn.flush_input(&mut session) {
                let _ = event_tx.send(LiveRdpEvent::Error(format!("RDP input failed: {e}")));
                if let (Some(root), Some(transfer)) =
                    (clipboard_root.as_deref(), guest_files_transfer.as_ref())
                {
                    transfer.cancel(root);
                }
                return;
            }
        }

        match conn.pump_once(&mut session, Duration::from_millis(50)) {
            Ok(PumpOutcome::Processed { painted_rects }) => {
                if painted_rects > 0 {
                    if let Some((frame, damage)) = session.frame_with_damage() {
                        frame_mailbox.publish(frame, damage);
                    }
                }
            }
            Ok(PumpOutcome::TimedOut) => {}
            Ok(PumpOutcome::Terminated { reason }) => {
                let _ = event_tx.send(LiveRdpEvent::Ended(reason));
                if let (Some(root), Some(transfer)) =
                    (clipboard_root.as_deref(), guest_files_transfer.as_ref())
                {
                    transfer.cancel(root);
                }
                return;
            }
            Err(e) => {
                let _ = event_tx.send(LiveRdpEvent::Error(format!("RDP pump failed: {e}")));
                if let (Some(root), Some(transfer)) =
                    (clipboard_root.as_deref(), guest_files_transfer.as_ref())
                {
                    transfer.cancel(root);
                }
                return;
            }
        }

        if gated_guest_clipboard.is_none() && pending_guest_clipboard.is_none() {
            if guest_files_transfer.is_none() {
                if let Some(file_list) = conn.take_guest_file_list() {
                    match (clipboard_root.as_deref(), file_list) {
                        (Some(root), Ok(file_list)) => {
                            match RdpGuestFilesTransfer::from_list(
                                root,
                                &clipboard_lease,
                                &file_list,
                            ) {
                                Ok(transfer) => {
                                    if let Err(error) = conn.begin_guest_file_retrieval(0) {
                                        transfer.cancel(root);
                                        let _ = event_tx.send(LiveRdpEvent::Error(format!(
                                            "RDP guest file retrieval refused: {error}"
                                        )));
                                    } else {
                                        guest_files_transfer = Some(transfer);
                                    }
                                }
                                Err(error) => {
                                    let _ = event_tx.send(LiveRdpEvent::Error(error));
                                }
                            }
                        }
                        (_, Err(error)) => {
                            let _ = event_tx.send(LiveRdpEvent::Error(format!(
                                "RDP guest file list refused: {error}"
                            )));
                        }
                        (None, Ok(_)) => {
                            let _ = event_tx.send(LiveRdpEvent::Error(
                                "RDP guest files refused: Files authority root is unavailable"
                                    .into(),
                            ));
                        }
                    }
                }
            }
            if let (Some(root), Some(mut transfer)) =
                (clipboard_root.as_deref(), guest_files_transfer.take())
            {
                let mut retain = true;
                if transfer.staged_message.is_none() {
                    if let Some(chunk_result) = conn.take_guest_file_chunk() {
                        match chunk_result {
                            Ok(chunk) => match transfer.stage_chunk(root, &chunk) {
                                Ok(Some((content_hash, byte_count, files_reference))) => {
                                    guest_message_sequence =
                                        guest_message_sequence.saturating_add(1);
                                    match rdp_guest_files_clipboard_message(
                                        &clipboard_lease,
                                        guest_message_sequence,
                                        transfer.files.len(),
                                        content_hash,
                                        byte_count,
                                        files_reference,
                                        now_ms,
                                    ) {
                                        Ok(message) => transfer.staged_message = Some(message),
                                        Err(error) => {
                                            transfer.cancel(root);
                                            let _ = event_tx.send(LiveRdpEvent::Error(error));
                                            retain = false;
                                        }
                                    }
                                }
                                Ok(None) => {
                                    if chunk.is_complete()
                                        && transfer.next_file_index < transfer.files.len()
                                    {
                                        if let Err(error) = conn
                                            .begin_guest_file_retrieval(transfer.next_file_index)
                                        {
                                            transfer.cancel(root);
                                            let _ = event_tx.send(LiveRdpEvent::Error(format!(
                                                "RDP guest file retrieval refused: {error}"
                                            )));
                                            retain = false;
                                        }
                                    }
                                }
                                Err(error) => {
                                    transfer.cancel(root);
                                    let _ = event_tx.send(LiveRdpEvent::Error(error));
                                    retain = false;
                                }
                            },
                            Err(error) => {
                                transfer.cancel(root);
                                let _ = event_tx.send(LiveRdpEvent::Error(format!(
                                    "RDP guest file transfer refused: {error}"
                                )));
                                retain = false;
                            }
                        }
                    }
                }
                if retain {
                    guest_files_transfer = Some(transfer);
                }
            }
            if let Some(html) = conn.take_guest_html_clipboard() {
                guest_message_sequence = guest_message_sequence.saturating_add(1);
                if guest_message_sequence != 0 {
                    if let Ok(rich) = rdp_guest_html_clipboard_message(
                        &clipboard_lease,
                        guest_message_sequence,
                        html,
                        now_ms,
                    ) {
                        pending_guest_clipboard = Some((None, rich));
                    }
                }
            } else if let Some(text) = conn.take_guest_clipboard() {
                guest_message_sequence = guest_message_sequence.saturating_add(1);
                let clip = ClipboardClipBody::from_text(
                    text.clone(),
                    clipboard_source.clone(),
                    chrono::Utc::now().to_rfc3339(),
                );
                if clip.validate().is_ok() && guest_message_sequence != 0 {
                    if let Ok(rich) = vdi_guest_clipboard_message(
                        "rdp",
                        &clipboard_lease,
                        guest_message_sequence,
                        text,
                        now_ms,
                    ) {
                        pending_guest_clipboard = Some((Some(clip), rich));
                    }
                }
            } else if let Some(image) = conn.take_guest_image_clipboard() {
                let refusal = refuse_rdp_guest_image_without_files_ingress(image);
                let _ = event_tx.send(LiveRdpEvent::ClipboardRefused(refusal));
            }
        }
    }
}

#[cfg(feature = "live-vdi")]
fn run_live_vnc(
    config: VncConfig,
    input: SharedInputMailbox,
    stop_rx: mpsc::Receiver<()>,
    event_tx: mpsc::Sender<LiveVncEvent>,
    clipboard_root: Option<PathBuf>,
    clipboard_source: String,
    mut clipboard_lease: VdiClipboardLeaseV2,
    clipboard_permissions: Option<ClipboardPermissionIngress>,
    frame_mailbox: LatestFrameMailbox,
) {
    let target = format!("{}:{}", config.host, config.port);
    let mut session = match VncSession::new(config) {
        Ok(session) => session,
        Err(e) => {
            let _ = event_tx.send(LiveVncEvent::Error(format!("VNC config rejected: {e}")));
            return;
        }
    };
    let mut conn = match VncConnection::connect(&mut session) {
        Ok(conn) => conn,
        Err(e) => {
            let _ = event_tx.send(LiveVncEvent::Error(format!("VNC connect failed: {e}")));
            return;
        }
    };
    let negotiated = conn.negotiated().clone();
    let _ = event_tx.send(LiveVncEvent::Connected(format!(
        "{target} (RFB {}.{}, {}x{}, {:?})",
        negotiated.major, negotiated.minor, negotiated.width, negotiated.height, negotiated.name
    )));
    if let Some((frame, damage)) = session.frame_with_damage() {
        frame_mailbox.publish(frame, damage);
    }

    if let Some(root) = clipboard_root.as_deref() {
        if let Err(error) = publish_vdi_clipboard_lease(root, &clipboard_lease) {
            let _ = event_tx.send(LiveVncEvent::Error(error));
            return;
        }
    }

    // A receipt is persisted only after ClientCutText flushes. The command can
    // then remain latest-value-wins without replaying after reconnect.
    let mut gated_host_clipboard = None::<(VdiClipboardMessageV2, String, ClipboardGateTicket)>;
    let mut pending_host_clipboard = None::<(VdiClipboardMessageV2, ClipboardGateTicket)>;
    let mut last_gated_host_clipboard = None::<(String, u64, u64)>;
    let mut pending_guest_clipboard = None::<(ClipboardClipBody, VdiClipboardMessageV2)>;
    let mut gated_guest_clipboard = None::<(
        ClipboardClipBody,
        VdiClipboardMessageV2,
        ClipboardGateTicket,
    )>;
    let mut guest_message_sequence = 0_u64;

    loop {
        if stop_rx.try_recv().is_ok() {
            conn.shutdown();
            return;
        }

        let now_ms = unix_time_ms();
        if gated_host_clipboard.is_none()
            && pending_host_clipboard.is_none()
            && pending_guest_clipboard.is_none()
            && gated_guest_clipboard.is_none()
            && now_ms.saturating_add(30_000) >= clipboard_lease.expires_at_ms
        {
            match renew_vnc_clipboard_lease(&clipboard_lease, now_ms) {
                Ok(renewed) => {
                    clipboard_lease = renewed;
                    guest_message_sequence = 0;
                    if let Some(root) = clipboard_root.as_deref() {
                        if let Err(error) = publish_vdi_clipboard_lease(root, &clipboard_lease) {
                            let _ = event_tx.send(LiveVncEvent::Error(error));
                            return;
                        }
                    }
                }
                Err(error) => {
                    let _ = event_tx.send(LiveVncEvent::Error(error));
                    return;
                }
            }
        }

        if let Some((command, text, ticket)) = gated_host_clipboard.take() {
            match ticket.try_begin_materialization() {
                ClipboardGateReadiness::Pending => {
                    gated_host_clipboard = Some((command, text, ticket));
                }
                ClipboardGateReadiness::Refused => {}
                ClipboardGateReadiness::Materialize => {
                    if let Err(error) = session.send_clipboard_to_guest(text) {
                        ticket.report_failure(ClipboardFailure::Transport, now_ms);
                        let _ = event_tx.send(LiveVncEvent::Error(format!(
                            "VNC host clipboard refused: {error}"
                        )));
                        return;
                    }
                    ticket.report_progress(command.envelope.byte_count);
                    pending_host_clipboard = Some((command, ticket));
                }
            }
        }

        if let Some((legacy, rich, ticket)) = gated_guest_clipboard.take() {
            match ticket.try_begin_materialization() {
                ClipboardGateReadiness::Pending => {
                    gated_guest_clipboard = Some((legacy, rich, ticket));
                }
                ClipboardGateReadiness::Refused => {}
                ClipboardGateReadiness::Materialize => {
                    match try_publish_vnc_clipboard_event(clipboard_root.as_deref(), &legacy, &rich)
                    {
                        Ok(()) => {
                            ticket.report_progress(rich.envelope.byte_count);
                            ticket.report_complete(now_ms);
                            let _ = event_tx.send(LiveVncEvent::ClipboardPublished);
                        }
                        Err(_) => {
                            ticket.report_failure(ClipboardFailure::Transport, now_ms);
                        }
                    }
                }
            }
        }

        if gated_guest_clipboard.is_none() {
            if let Some((legacy, rich)) = pending_guest_clipboard.take() {
                if let (Some(ingress), Ok(target)) = (
                    clipboard_permissions.as_ref(),
                    ClipboardTarget::new(
                        ClipboardTargetKind::LocalSeat,
                        clipboard_lease.session_id.clone(),
                    ),
                ) {
                    match ingress.submit_vdi(&rich, &clipboard_lease, None, target, now_ms) {
                        Ok(ticket) => {
                            gated_guest_clipboard = Some((legacy, rich, ticket));
                        }
                        Err(crate::clipboard_permissions::ClipboardPermissionError::Busy) => {
                            pending_guest_clipboard = Some((legacy, rich));
                        }
                        Err(_) => {}
                    }
                }
            }
        }

        if gated_host_clipboard.is_none() && pending_host_clipboard.is_none() {
            if let Some(root) = clipboard_root.as_deref() {
                if let Ok(Some((command, text))) =
                    read_latest_vnc_host_clipboard(root, &clipboard_lease, now_ms)
                {
                    let key = (
                        command.lease_id.clone(),
                        command.generation,
                        command.message_sequence,
                    );
                    if last_gated_host_clipboard.as_ref() != Some(&key) {
                        if let (Some(ingress), Ok(target)) = (
                            clipboard_permissions.as_ref(),
                            ClipboardTarget::new(
                                ClipboardTargetKind::Guest,
                                clipboard_lease.session_id.clone(),
                            ),
                        ) {
                            match ingress.submit_vdi(
                                &command,
                                &clipboard_lease,
                                None,
                                target,
                                now_ms,
                            ) {
                                Ok(ticket) => {
                                    last_gated_host_clipboard = Some(key);
                                    gated_host_clipboard = Some((command, text, ticket));
                                }
                                Err(
                                    crate::clipboard_permissions::ClipboardPermissionError::Busy,
                                ) => {}
                                Err(_) => {
                                    last_gated_host_clipboard = Some(key);
                                }
                            }
                        } else {
                            // Fail closed when the shell permission controller is
                            // unavailable; never fall back to direct materialization.
                            last_gated_host_clipboard = Some(key);
                        }
                    }
                }
            }
        }

        let mut had_input = false;
        let events = input
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .drain();
        for event in events {
            session.send_input(&event);
            had_input = true;
        }
        if had_input || pending_host_clipboard.is_some() {
            if let Err(e) = conn.flush_input(&mut session) {
                if let Some((_, ticket)) = pending_host_clipboard.as_ref() {
                    ticket.report_failure(ClipboardFailure::Transport, now_ms);
                }
                let _ = event_tx.send(LiveVncEvent::Error(format!("VNC input failed: {e}")));
                return;
            }
            if let Some((command, ticket)) = pending_host_clipboard.take() {
                if let Some(root) = clipboard_root.as_deref() {
                    if let Err(error) = publish_vdi_clipboard_receipt(root, &command.receipt()) {
                        ticket.report_failure(ClipboardFailure::Transport, now_ms);
                        let _ = event_tx.send(LiveVncEvent::Error(error));
                        return;
                    }
                }
                ticket.report_complete(now_ms);
            }
        }

        match conn.pump_once(&mut session, Duration::from_millis(50)) {
            Ok(VncPumpOutcome::Processed { rects, .. }) => {
                if rects > 0 {
                    if let Some((frame, damage)) = session.frame_with_damage() {
                        frame_mailbox.publish(frame, damage);
                    }
                }
            }
            Ok(VncPumpOutcome::Clipboard { .. }) => {
                // `VncSession` has already performed bounded UTF-8 admission,
                // echo suppression, and duplicate suppression. Drain only the
                // accepted latest value and attach the VNC session source here,
                // before the UI publishes the canonical four-field event.
                if let Some(text) = session
                    .take_guest_clipboard()
                    .into_iter()
                    .last()
                    .map(mde_vdi_vnc::RfbCutText::into_text)
                {
                    guest_message_sequence = guest_message_sequence.saturating_add(1);
                    let clip = ClipboardClipBody::from_text(
                        text.clone(),
                        clipboard_source.clone(),
                        chrono::Utc::now().to_rfc3339(),
                    );
                    if clip.validate().is_ok() && guest_message_sequence != 0 {
                        if let Ok(rich) = vnc_guest_clipboard_message(
                            &clipboard_lease,
                            guest_message_sequence,
                            text,
                            now_ms,
                        ) {
                            pending_guest_clipboard = Some((clip, rich));
                        }
                    }
                }
            }
            Ok(VncPumpOutcome::TimedOut) => {}
            Ok(VncPumpOutcome::Terminated { reason }) => {
                let _ = event_tx.send(LiveVncEvent::Ended(reason));
                return;
            }
            Err(e) => {
                let _ = event_tx.send(LiveVncEvent::Error(format!("VNC pump failed: {e}")));
                return;
            }
        }
    }
}

#[cfg(feature = "live-vdi")]
fn run_live_spice(
    config: SpiceConfig,
    input: SharedInputMailbox,
    stop_rx: mpsc::Receiver<()>,
    event_tx: mpsc::Sender<LiveSpiceEvent>,
    clipboard_root: Option<PathBuf>,
    clipboard_source: String,
    mut clipboard_lease: VdiClipboardLeaseV2,
    clipboard_permissions: Option<ClipboardPermissionIngress>,
    frame_mailbox: LatestFrameMailbox,
) {
    let target = format!("{}:{}", config.host, config.port);
    let mut session = match SpiceSession::new(config.clone()) {
        Ok(session) => session,
        Err(e) => {
            let _ = event_tx.send(LiveSpiceEvent::Error(format!("SPICE config rejected: {e}")));
            return;
        }
    };
    let mut conn = match BlockingSpiceTransport::connect(&config) {
        Ok(conn) => conn,
        Err(e) => {
            let _ = event_tx.send(LiveSpiceEvent::Error(format!("SPICE connect failed: {e}")));
            return;
        }
    };
    let _ = event_tx.send(LiveSpiceEvent::Connected(target));
    if let Some((frame, damage)) = session.frame_with_damage() {
        frame_mailbox.publish(frame, damage);
    }

    if let Some(root) = clipboard_root.as_deref() {
        if let Err(error) = publish_vdi_clipboard_lease(root, &clipboard_lease) {
            let _ = event_tx.send(LiveSpiceEvent::Error(error));
            return;
        }
    }

    let mut last_clipboard_status = None;
    let mut gated_host_clipboard = None::<(VdiClipboardMessageV2, String, ClipboardGateTicket)>;
    let mut pending_host_delivery = None::<(VdiClipboardMessageV2, ClipboardGateTicket)>;
    let mut last_gated_host_clipboard = None::<(String, u64, u64)>;
    let mut pending_guest_clipboard = None::<(ClipboardClipBody, VdiClipboardMessageV2)>;
    let mut gated_guest_clipboard = None::<(
        ClipboardClipBody,
        VdiClipboardMessageV2,
        ClipboardGateTicket,
    )>;
    let mut guest_message_sequence = 0_u64;

    loop {
        if stop_rx.try_recv().is_ok() {
            let _ = event_tx.send(LiveSpiceEvent::Ended(
                "SPICE session stopped by shell".to_string(),
            ));
            return;
        }

        let now_ms = unix_time_ms();
        let status = conn.clipboard_status();
        if last_clipboard_status != Some(status) {
            last_clipboard_status = Some(status);
            let message = match status {
                mde_vdi_spice::ClipboardStatus::AgentDisconnected => {
                    "SPICE clipboard unavailable: guest agent disconnected"
                }
                mde_vdi_spice::ClipboardStatus::CapabilityPending => {
                    "SPICE clipboard waiting for guest-agent capability negotiation"
                }
                mde_vdi_spice::ClipboardStatus::Unsupported => {
                    "SPICE clipboard unavailable: guest agent did not advertise clipboard-by-demand"
                }
                mde_vdi_spice::ClipboardStatus::Ready => {
                    "SPICE clipboard ready: bidirectional UTF-8 text"
                }
            };
            let _ = event_tx.send(LiveSpiceEvent::ClipboardStatus(message.into()));
        }

        if gated_host_clipboard.is_none()
            && pending_host_delivery.is_none()
            && pending_guest_clipboard.is_none()
            && gated_guest_clipboard.is_none()
            && now_ms.saturating_add(30_000) >= clipboard_lease.expires_at_ms
        {
            match renew_vdi_clipboard_lease("spice", &clipboard_lease, now_ms) {
                Ok(renewed) => {
                    clipboard_lease = renewed;
                    guest_message_sequence = 0;
                    if let Some(root) = clipboard_root.as_deref() {
                        if let Err(error) = publish_vdi_clipboard_lease(root, &clipboard_lease) {
                            let _ = event_tx.send(LiveSpiceEvent::Error(error));
                            return;
                        }
                    }
                }
                Err(error) => {
                    let _ = event_tx.send(LiveSpiceEvent::Error(error));
                    return;
                }
            }
        }

        for clipboard_event in conn.take_clipboard_events() {
            match clipboard_event {
                mde_vdi_spice::ClipboardEvent::HostTextRequested => {
                    if let Some((_, ticket)) = pending_host_delivery.as_ref() {
                        match ticket.try_begin_materialization() {
                            ClipboardGateReadiness::Materialize => {
                                if let Err(error) = conn.send_offered_clipboard_text() {
                                    if let Some((_, ticket)) = pending_host_delivery.take() {
                                        ticket.report_failure(ClipboardFailure::Transport, now_ms);
                                    }
                                    let _ = event_tx.send(LiveSpiceEvent::ClipboardStatus(
                                        format!("SPICE host clipboard refused: {error}"),
                                    ));
                                }
                            }
                            ClipboardGateReadiness::Refused => {
                                let _ = conn.cancel_clipboard_offer();
                                pending_host_delivery = None;
                            }
                            ClipboardGateReadiness::Pending => {}
                        }
                    }
                }
                mde_vdi_spice::ClipboardEvent::HostTextSent => {
                    if let Some((command, ticket)) = pending_host_delivery.take() {
                        ticket.report_progress(command.envelope.byte_count);
                        if let Some(root) = clipboard_root.as_deref() {
                            if let Err(error) =
                                publish_vdi_clipboard_receipt(root, &command.receipt())
                            {
                                ticket.report_failure(ClipboardFailure::Transport, now_ms);
                                let _ = event_tx.send(LiveSpiceEvent::Error(error));
                                return;
                            }
                        }
                        ticket.report_complete(now_ms);
                    }
                }
                mde_vdi_spice::ClipboardEvent::GuestText(text) => {
                    if gated_guest_clipboard.is_none() && pending_guest_clipboard.is_none() {
                        guest_message_sequence = guest_message_sequence.saturating_add(1);
                        let clip = ClipboardClipBody::from_text(
                            text.clone(),
                            clipboard_source.clone(),
                            chrono::Utc::now().to_rfc3339(),
                        );
                        if clip.validate().is_ok() && guest_message_sequence != 0 {
                            if let Ok(rich) = vdi_guest_clipboard_message(
                                "spice",
                                &clipboard_lease,
                                guest_message_sequence,
                                text,
                                now_ms,
                            ) {
                                pending_guest_clipboard = Some((clip, rich));
                            }
                        }
                    }
                }
                mde_vdi_spice::ClipboardEvent::CapabilityLost => {
                    if let Some((_, ticket)) = pending_host_delivery.take() {
                        ticket.report_failure(ClipboardFailure::Transport, now_ms);
                    }
                }
            }
        }

        if let Some((command, text, ticket)) = gated_host_clipboard.take() {
            match ticket.readiness_before_materialization() {
                ClipboardGateReadiness::Pending => {
                    gated_host_clipboard = Some((command, text, ticket));
                }
                ClipboardGateReadiness::Refused => {}
                ClipboardGateReadiness::Materialize => match conn.offer_clipboard_text(text) {
                    Ok(()) => pending_host_delivery = Some((command, ticket)),
                    Err(error) => {
                        ticket.report_failure(ClipboardFailure::Transport, now_ms);
                        let _ = event_tx.send(LiveSpiceEvent::ClipboardStatus(format!(
                            "SPICE host clipboard refused: {error}"
                        )));
                    }
                },
            }
        }

        if pending_host_delivery.as_ref().is_some_and(|(_, ticket)| {
            ticket.readiness_before_materialization() == ClipboardGateReadiness::Refused
        }) {
            let _ = conn.cancel_clipboard_offer();
            pending_host_delivery = None;
        }

        if let Some((legacy, rich, ticket)) = gated_guest_clipboard.take() {
            match ticket.try_begin_materialization() {
                ClipboardGateReadiness::Pending => {
                    gated_guest_clipboard = Some((legacy, rich, ticket));
                }
                ClipboardGateReadiness::Refused => {}
                ClipboardGateReadiness::Materialize => {
                    match try_publish_vnc_clipboard_event(clipboard_root.as_deref(), &legacy, &rich)
                    {
                        Ok(()) => {
                            ticket.report_progress(rich.envelope.byte_count);
                            ticket.report_complete(now_ms);
                            let _ = event_tx.send(LiveSpiceEvent::ClipboardPublished);
                        }
                        Err(_) => ticket.report_failure(ClipboardFailure::Transport, now_ms),
                    }
                }
            }
        }

        if gated_guest_clipboard.is_none() {
            if let Some((legacy, rich)) = pending_guest_clipboard.take() {
                if let (Some(ingress), Ok(target)) = (
                    clipboard_permissions.as_ref(),
                    ClipboardTarget::new(
                        ClipboardTargetKind::LocalSeat,
                        clipboard_lease.session_id.clone(),
                    ),
                ) {
                    match ingress.submit_vdi(&rich, &clipboard_lease, None, target, now_ms) {
                        Ok(ticket) => gated_guest_clipboard = Some((legacy, rich, ticket)),
                        Err(crate::clipboard_permissions::ClipboardPermissionError::Busy) => {
                            pending_guest_clipboard = Some((legacy, rich));
                        }
                        Err(_) => {}
                    }
                }
            }
        }

        if status == mde_vdi_spice::ClipboardStatus::Ready
            && gated_host_clipboard.is_none()
            && pending_host_delivery.is_none()
        {
            if let Some(root) = clipboard_root.as_deref() {
                if let Ok(Some((command, text))) =
                    read_latest_vnc_host_clipboard(root, &clipboard_lease, now_ms)
                {
                    let key = (
                        command.lease_id.clone(),
                        command.generation,
                        command.message_sequence,
                    );
                    if last_gated_host_clipboard.as_ref() != Some(&key) {
                        if let (Some(ingress), Ok(target)) = (
                            clipboard_permissions.as_ref(),
                            ClipboardTarget::new(
                                ClipboardTargetKind::Guest,
                                clipboard_lease.session_id.clone(),
                            ),
                        ) {
                            match ingress.submit_vdi(
                                &command,
                                &clipboard_lease,
                                None,
                                target,
                                now_ms,
                            ) {
                                Ok(ticket) => {
                                    last_gated_host_clipboard = Some(key);
                                    gated_host_clipboard = Some((command, text, ticket));
                                }
                                Err(
                                    crate::clipboard_permissions::ClipboardPermissionError::Busy,
                                ) => {}
                                Err(_) => last_gated_host_clipboard = Some(key),
                            }
                        } else {
                            last_gated_host_clipboard = Some(key);
                        }
                    }
                }
            }
        }

        let mut had_input = false;
        let events = input
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .drain();
        for event in events {
            session.send_input(&event);
            had_input = true;
        }
        if had_input {
            if let Err(e) = conn.flush_input(&mut session) {
                let _ = event_tx.send(LiveSpiceEvent::Error(format!("SPICE input failed: {e}")));
                return;
            }
        }

        match conn.pump_frame(&mut session) {
            Ok(true) => {
                if let Some((frame, damage)) = session.frame_with_damage() {
                    frame_mailbox.publish(frame, damage);
                }
            }
            Ok(false) => {}
            Err(e) => {
                let _ = event_tx.send(LiveSpiceEvent::Error(format!("SPICE pump failed: {e}")));
                return;
            }
        }
    }
}

// ───────────────────── vdi-vm-4 / shell-ux-1: session state ──────────────────
//
// A live transport can DROP (the server closes, or a pump errors) at any time.
// Before this, a drop froze the desktop at its last frame with no recovery and no
// honest status (shell-ux-1). Now a drop that is NOT a user-initiated close drives
// a small session-state machine: the shell auto-reconnects to the SAME endpoint
// with bounded retries + capped backoff (vdi-vm-4), and paints an honest overlay
// with Retry / pick-a-different affordances the whole time (shell-ux-1).
//
// The user-close vs transport-drop distinction is STRUCTURAL, not a flag: a clean
// close ([`VdiState::clear_target`] / [`VdiState::request_connect`]) TAKES the live
// handle before any poll re-reads it AND resets the phase to `Live`, so a drop is
// only ever driven by [`VdiState::on_transport_drop`] for an INSTALLED handle whose
// worker thread died on its own — a real drop. (The transports confirm this: their
// worker sends `Error`/`Ended` ONLY on a pump error / server termination; a
// shell-requested stop returns with NO event, and by then its handle is gone.)

/// The bounded auto-reconnect budget (vdi-vm-4): after this many failed re-dials the
/// session gives up and shows the honest Failed overlay instead of retrying forever.
#[cfg(feature = "live-vdi")]
const MAX_RECONNECT_ATTEMPTS: u32 = 5;

/// vdi-vm-4 / shell-ux-1 — the live desktop session's connection phase. A transport
/// drop walks `Live → Reconnecting{attempt} → … → Failed{reason}`; a fresh frame
/// from a re-dialed transport walks it back to `Live`. Drives BOTH the auto-
/// reconnect scheduler and the honest overlay, so the two can never diverge.
#[cfg(feature = "live-vdi")]
#[derive(Debug, Clone, PartialEq, Eq, Default)]
enum SessionPhase {
    /// The transport is live, or connecting normally on the initial dial — no
    /// overlay, no pending reconnect.
    #[default]
    Live,
    /// A drop was detected; the shell is auto-reconnecting to the SAME endpoint.
    /// `attempt` is 1-based; `reason` is the honest last drop reason.
    Reconnecting { attempt: u32, reason: String },
    /// Auto-reconnect exhausted its budget (or a re-dial could not even start) — the
    /// honest failure reason, surfaced with Retry / pick-a-different.
    Failed { reason: String },
}

/// Bounded, log-safe VDI measurements exposed to diagnostics and acceptance
/// tooling. These counters describe the local decode/texture seam; they do not
/// masquerade as guest GPU, CPU, or audio evidence.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct VdiMetricsSnapshot {
    pub(crate) frames_received: u64,
    pub(crate) full_uploads: u64,
    pub(crate) partial_uploads: u64,
    pub(crate) partial_rects: u64,
    pub(crate) reconnects: u64,
    pub(crate) shell_repaints: u64,
    pub(crate) last_frame_interval_us: Option<u64>,
    pub(crate) last_frame_to_upload_us: Option<u64>,
    pub(crate) last_upload_us: Option<u64>,
    /// The shell process' share of aggregate host CPU time, in thousandths of
    /// one host-wide CPU (100_000 = 100%). This is host load, never guest load.
    pub(crate) last_host_process_cpu_permille: Option<u32>,
    /// The first DRM render device's reported busy percentage, in thousandths
    /// (100_000 = 100%). Missing sysfs telemetry stays `None`.
    pub(crate) last_host_gpu_busy_permille: Option<u32>,
}

#[derive(Debug, Default)]
struct VdiMetrics {
    snapshot: VdiMetricsSnapshot,
    previous_frame_at: Option<std::time::Instant>,
    queued_frame_at: Option<std::time::Instant>,
    host_load: HostLoadSampler,
}

/// Best-effort host-side load sampler for the VDI diagnostics seam. It reads
/// only procfs/sysfs counters, is throttled to four samples per second, and
/// never turns missing host telemetry into a guest capability claim.
#[derive(Debug, Default)]
struct HostLoadSampler {
    sampled_at: Option<std::time::Instant>,
    previous_process_ticks: Option<u64>,
    previous_total_ticks: Option<u64>,
}

impl HostLoadSampler {
    fn sample(&mut self, snapshot: &mut VdiMetricsSnapshot) {
        let now = std::time::Instant::now();
        if self.sampled_at.is_some_and(|previous| {
            now.duration_since(previous) < std::time::Duration::from_millis(250)
        }) {
            return;
        }
        self.sampled_at = Some(now);

        let process_ticks = std::fs::read_to_string("/proc/self/stat")
            .ok()
            .and_then(|stat| parse_process_cpu_ticks(&stat));
        let total_ticks = std::fs::read_to_string("/proc/stat")
            .ok()
            .and_then(|stat| {
                stat.lines()
                    .find(|line| line.starts_with("cpu "))
                    .map(str::to_owned)
            })
            .and_then(|line| parse_total_cpu_ticks(&line));
        if let (Some(process_ticks), Some(total_ticks)) = (process_ticks, total_ticks) {
            if let (Some(previous_process), Some(previous_total)) =
                (self.previous_process_ticks, self.previous_total_ticks)
            {
                let process_delta = process_ticks.saturating_sub(previous_process);
                let total_delta = total_ticks.saturating_sub(previous_total);
                if total_delta > 0 {
                    snapshot.last_host_process_cpu_permille = Some(
                        u32::try_from(
                            process_delta
                                .saturating_mul(100_000)
                                .checked_div(total_delta)
                                .unwrap_or(0)
                                .min(100_000),
                        )
                        .unwrap_or(100_000),
                    );
                }
            }
            self.previous_process_ticks = Some(process_ticks);
            self.previous_total_ticks = Some(total_ticks);
        }
        snapshot.last_host_gpu_busy_permille = read_host_gpu_busy_permille();
    }
}

fn parse_process_cpu_ticks(stat: &str) -> Option<u64> {
    // `/proc/<pid>/stat` has a parenthesized command that may contain spaces;
    // the fields after its final ')' start at field 3 (state), making utime
    // field 14 and stime field 15 offsets 11 and 12 in this suffix.
    let fields = stat
        .rsplit_once(')')?
        .1
        .split_whitespace()
        .collect::<Vec<_>>();
    let user = fields.get(11)?.parse::<u64>().ok()?;
    let system = fields.get(12)?.parse::<u64>().ok()?;
    user.checked_add(system)
}

fn parse_total_cpu_ticks(line: &str) -> Option<u64> {
    let mut fields = line.split_whitespace();
    if fields.next()? != "cpu" {
        return None;
    }
    fields.try_fold(0_u64, |total, field| {
        total.checked_add(field.parse::<u64>().ok()?)
    })
}

fn read_host_gpu_busy_permille() -> Option<u32> {
    let mut highest = None;
    let entries = std::fs::read_dir("/sys/class/drm").ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("card") || name.contains('-') {
            continue;
        }
        let Ok(value) = std::fs::read_to_string(entry.path().join("device/gpu_busy_percent"))
        else {
            continue;
        };
        let Ok(percent) = value.trim().parse::<u32>() else {
            continue;
        };
        if percent <= 100 {
            let permille = percent.saturating_mul(1_000);
            highest = Some(highest.map_or(permille, |current: u32| current.max(permille)));
        }
    }
    highest
}

impl VdiMetrics {
    fn note_frame(&mut self) {
        let now = std::time::Instant::now();
        if let Some(previous) = self.previous_frame_at {
            self.snapshot.last_frame_interval_us =
                Some(u64::try_from(now.duration_since(previous).as_micros()).unwrap_or(u64::MAX));
        }
        self.previous_frame_at = Some(now);
        self.queued_frame_at = Some(now);
        self.snapshot.frames_received = self.snapshot.frames_received.saturating_add(1);
    }

    fn note_upload(&mut self, kind: FrameUpload, elapsed: std::time::Duration) {
        self.snapshot.last_upload_us = Some(u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX));
        if let Some(queued) = self.queued_frame_at.take() {
            self.snapshot.last_frame_to_upload_us =
                Some(u64::try_from(queued.elapsed().as_micros()).unwrap_or(u64::MAX));
        }
        match kind {
            FrameUpload::Full => {
                self.snapshot.full_uploads = self.snapshot.full_uploads.saturating_add(1);
            }
            FrameUpload::Partial { rects } => {
                self.snapshot.partial_uploads = self.snapshot.partial_uploads.saturating_add(1);
                self.snapshot.partial_rects =
                    self.snapshot.partial_rects.saturating_add(u64::from(rects));
            }
        }
    }

    fn note_reconnect(&mut self) {
        self.snapshot.reconnects = self.snapshot.reconnects.saturating_add(1);
    }

    fn note_shell_repaint(&mut self) {
        self.snapshot.shell_repaints = self.snapshot.shell_repaints.saturating_add(1);
        self.host_load.sample(&mut self.snapshot);
    }
}

/// vdi-vm-4 — capped exponential backoff before reconnect `attempt` (1-based): 0.5s,
/// 1s, 2s, 4s, then held at 8s. Bounds the reconnect storm against a flapping peer.
#[cfg(feature = "live-vdi")]
fn reconnect_backoff(attempt: u32) -> Duration {
    let shift = attempt.saturating_sub(1).min(4);
    let ms = 500u64.saturating_mul(1u64 << shift);
    Duration::from_millis(ms.min(8_000))
}

/// vdi-vm-4 — the pure transition on a detected transport drop. A drop from `Live`
/// opens attempt 1; each further drop while `Reconnecting` bumps the attempt until
/// `max` is spent, then the session is `Failed` with the honest reason. `Failed` is
/// terminal (an explicit operator Retry resets it — [`VdiState::retry_now`]).
#[cfg(feature = "live-vdi")]
fn next_phase_on_drop(current: &SessionPhase, reason: String, max: u32) -> SessionPhase {
    match current {
        SessionPhase::Live => SessionPhase::Reconnecting { attempt: 1, reason },
        SessionPhase::Reconnecting { attempt, .. } if *attempt < max => {
            SessionPhase::Reconnecting {
                attempt: attempt + 1,
                reason,
            }
        }
        SessionPhase::Reconnecting { .. } | SessionPhase::Failed { .. } => {
            SessionPhase::Failed { reason }
        }
    }
}

/// vdi-vm-8 — a live RDP/SPICE desktop is re-negotiated (re-dialed at the panel's
/// current size) only once the guest's real desktop diverges from the panel by more
/// than this many device pixels on either axis. Set well above the dock / menubar
/// chrome deltas — which the LINEAR upscale absorbs imperceptibly — so a chrome
/// toggle never triggers a disruptive re-dial; only a real seat / monitor resolution
/// change does.
#[cfg(feature = "live-vdi")]
const RESIZE_RENEGOTIATE_THRESHOLD_PX: u16 = 128;

/// vdi-vm-8 — the new panel size must hold steady this long before a resize re-dial
/// fires, so dragging / animating a resize collapses to a SINGLE re-negotiation
/// instead of a reconnect storm.
#[cfg(feature = "live-vdi")]
const RESIZE_SETTLE: Duration = Duration::from_millis(600);

/// vdi-vm-8 — two target sizes within this many device pixels count as "the same"
/// pending resize target, so sub-pixel layout jitter keeps the settle timer running
/// rather than restarting it every frame.
#[cfg(feature = "live-vdi")]
const RESIZE_TARGET_TOLERANCE_PX: u16 = 8;

/// vdi-vm-8 — a debounced resize re-negotiation in flight: the panel size to re-dial
/// at and the instant its settle window elapses. Armed by
/// [`VdiState::note_resize_target`] and fired by [`VdiState::poll_resize_renegotiate`].
#[cfg(feature = "live-vdi")]
#[derive(Debug, Clone, Copy)]
struct PendingResize {
    /// When the settle window elapses and the re-dial may fire.
    at: std::time::Instant,
    /// The panel size (device px) the transport will be re-dialed at.
    target: (u16, u16),
}

/// vdi-vm-8 — whether two desktop sizes differ by more than `tol` device pixels on
/// either axis. The pure predicate behind both the resize trigger (guest vs panel
/// beyond [`RESIZE_RENEGOTIATE_THRESHOLD_PX`]) and the "already dialed / same pending
/// target" checks (within [`RESIZE_TARGET_TOLERANCE_PX`]).
#[cfg(feature = "live-vdi")]
const fn size_diverges(a: (u16, u16), b: (u16, u16), tol: u16) -> bool {
    a.0.abs_diff(b.0) > tol || a.1.abs_diff(b.1) > tol
}

/// shell-ux-1 — an affordance the failure / reconnect overlay offers. Both are real
/// re-entries the session already owns a seam for (re-dial the retained request, or
/// fall back to the Chooser), never a dead-end.
#[cfg(feature = "live-vdi")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OverlayAction {
    /// Reconnect now — reset the attempt ladder and re-dial the SAME endpoint
    /// immediately, skipping the pending backoff.
    Retry,
    /// Abandon this desktop and return to the Chooser to pick a different one.
    PickDifferent,
}

/// shell-ux-1 — the honest status the overlay paints OVER the (possibly frozen) last
/// frame. Derived purely from the [`SessionPhase`] so it can never diverge from the
/// real session state, and so it is unit-testable without egui paint.
#[cfg(feature = "live-vdi")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionOverlay {
    /// The heading (in-progress reconnect vs terminal failure).
    title: String,
    /// The honest detail — the reconnect attempt + real drop reason, or the failure
    /// reason. Never a generic message (shell-ux-1).
    detail: String,
    /// Terminal-failure face (tints the heading DANGER) vs the reconnect face.
    failed: bool,
    /// The affordances offered — always Retry + pick-a-different, so neither face is
    /// a dead-end.
    actions: Vec<OverlayAction>,
}

/// shell-ux-1 — build the overlay for `phase`, or `None` when the session is `Live`
/// (the desktop paints normally). Pure; the panel renders the returned model and the
/// tests assert its honest content + affordances directly.
#[cfg(feature = "live-vdi")]
fn session_overlay(phase: &SessionPhase, max: u32) -> Option<SessionOverlay> {
    match phase {
        SessionPhase::Live => None,
        SessionPhase::Reconnecting { attempt, reason } => Some(SessionOverlay {
            title: "Reconnecting to the desktop\u{2026}".to_string(),
            detail: format!(
                "Attempt {attempt} of {max} on the same endpoint \u{2014} the connection dropped: {reason}"
            ),
            failed: false,
            actions: vec![OverlayAction::Retry, OverlayAction::PickDifferent],
        }),
        SessionPhase::Failed { reason } => Some(SessionOverlay {
            title: "Desktop disconnected".to_string(),
            detail: format!("Could not reconnect after {max} attempts \u{2014} {reason}"),
            failed: true,
            actions: vec![OverlayAction::Retry, OverlayAction::PickDifferent],
        }),
    }
}

/// Request the Browser VM's native, node-local Display1 attachment.
///
/// This is deliberately not a VNC, SPICE, RDP, or Moonlight connection. The
/// caller obtains fresh typed Workload status, then publishes the one
/// capability-bound `StartAndAttach(QemuDisplay1Dmabuf)` operation. `mackesd`
/// owns the domain and creates the one-use lease; the direct-DRM shell consumes
/// that lease through its authenticated local Display1 client.
///
/// A Display1 DMA-BUF and its Unix socket cannot cross a mesh link. Refuse a
/// remote placement instead of accidentally reviving a console relay.
pub(crate) fn request_browser_vm_display1_attach(
    target: &crate::web::BrowserVmTarget,
    local_node: &str,
    bus_root: Option<&Path>,
) -> Result<String, String> {
    if target.workload != "browser-vm" {
        return Err("Browser activation refused a non-browser workload identity.".to_owned());
    }
    if !target.serving_peer.eq_ignore_ascii_case(local_node.trim()) {
        return Err(format!(
            "Browser VM is placed on `{}` but this Display1 seat is `{}`. Native Display1 is node-local; no console relay was opened.",
            target.serving_peer, local_node
        ));
    }
    let root = bus_root.ok_or_else(|| "the local mesh Bus directory is unavailable".to_owned())?;
    let persist = Persist::open(root.to_path_buf())
        .map_err(|error| format!("the local mesh Bus could not be opened: {error}"))?;
    let status = crate::workload_api::read_status(&persist, &target.serving_peer, &target.workload)
        .ok_or_else(|| {
            "Browser VM has no fresh authoritative Workloads status; no attachment was requested."
                .to_owned()
        })?;
    if status.backend != WorkloadBackend::LibvirtVirtqemud {
        return Err(
            "Browser VM status names a non-VM backend; no attachment was requested.".to_owned(),
        );
    }
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or(0);
    let request = crate::workload_api::request_with_image(
        &target.workload,
        &target.serving_peer,
        status.backend,
        status.resources,
        WorkloadOperationAction::StartAndAttach,
        Some(WorkloadAttachmentProtocol::QemuDisplay1Dmabuf),
        status.generation,
        status.image_ref.as_deref(),
        now_ms,
    )?;
    crate::workload_api::publish(root, &request)
}

/// The Desktop surface's state: the active session (if any), the desktop texture
/// the framebuffer is uploaded into, the decode → upload hand-off slot, and the
/// picked target the discovery picker requested before a live session attaches.
#[derive(Default)]
pub(crate) struct VdiState {
    /// Pure in-memory presentation of the last admitted universal resource
    /// catalog. Snapshot ingestion happens outside [`vdi_panel`]; painting this
    /// model never reaches Bus, a backend, or the network.
    remote_sessions: RemoteSessionsModel,
    /// The connected desktop, or `None` when nothing is attached (the EmptyState).
    session: Option<Session>,
    /// The GPU texture the desktop framebuffer lives in — allocated on the first
    /// frame, then updated in place with [`TextureHandle::set`] every frame after
    /// (egui reuses the allocation, so a live desktop is not a per-frame upload
    /// churn).
    texture: Option<TextureHandle>,
    /// A decoded frame awaiting upload on the next paint. The decode side (the
    /// live session's `frame()`, or a synthetic frame in tests) writes it;
    /// `vdi_panel` drains it into `texture`. This is the single-threaded shape of
    /// the decode → UI hand-off the gated wire transport fills off-thread.
    incoming: Option<egui::ColorImage>,
    /// Which rectangles changed in `incoming` (perf-7). Written next to `incoming`
    /// by whoever produced the frame; the upload drains both together and
    /// `set_partial`s only the damaged sub-rectangles. `None` (or
    /// [`FrameDamage::Full`]) means "no reliable rect info" → a full `set`. Kept as a
    /// parallel slot so the existing `incoming` writers/tests that don't carry damage
    /// still compile and safely fall back to a full upload.
    incoming_damage: Option<FrameDamage>,
    /// Input may enter the guest only after the currently attached transport has
    /// produced a frame. A reconnect or resize re-dial may deliberately keep the
    /// prior texture visible, but that stale presentation must never authorize
    /// control of the replacement transport.
    presentation_input_authorized: bool,
    /// Local frame/texture/reconnect/repaint measurements. Guest hardware
    /// capability evidence remains a separate live-proof concern.
    metrics: VdiMetrics,
    /// Raised when the operator presses the reserved Esc chord over the desktop —
    /// the shell reads it to release the fullscreen desktop back to the chrome.
    return_to_chrome: bool,
    /// The connect the Chooser's picker chose (CHOOSER-4 — protocol + display +
    /// monitors + target), held until the gated live transport attaches a
    /// `session`. Drives the honest "connecting" caption (which names the chosen
    /// protocol + display) and tells the shell to show the Desktop surface.
    requested: Option<ConnectRequest>,
    /// Honest controller/authorization result available in every build,
    /// including builds without live decoder features.
    route_status: Option<String>,
    /// Exact brokered requests retained while another session has focus. The
    /// request is the authority for Browser profile/transport and authentication;
    /// roster labels are never used to reconstruct a connection.
    retained_requests: BTreeMap<String, ConnectRequest>,
    /// Focused tests prove authorization refusal cannot enter request
    /// installation, where a future WebRTC decoder would otherwise be spawned.
    #[cfg(test)]
    transport_install_attempted: bool,
    /// Live in-shell RDP transport for a direct endpoint. Kept separate from
    /// `session`, which remains the single-threaded decoder used by tests and VNC.
    #[cfg(feature = "live-vdi")]
    live_rdp: Option<LiveRdpHandle>,
    /// Live in-shell VNC transport for a direct endpoint / XAPI console fallback.
    #[cfg(feature = "live-vdi")]
    live_vnc: Option<LiveVncHandle>,
    /// Bounded metadata-only bridge into the shell clipboard permission model.
    /// The live worker fails closed when this has not been attached.
    #[cfg(feature = "live-vdi")]
    clipboard_permissions: Option<ClipboardPermissionIngress>,
    /// Live in-shell SPICE transport for native QEMU/KVM consoles.
    #[cfg(feature = "live-vdi")]
    live_spice: Option<LiveSpiceHandle>,
    /// Log-safe live transport status/error shown under the empty backdrop until a
    /// frame arrives.
    #[cfg(feature = "live-vdi")]
    live_status: Option<String>,
    /// Broker lifecycle currently marked active by the live transport.
    #[cfg(feature = "live-vdi")]
    active_broker_session: Option<BrokerSessionLifecycle>,
    /// VDI-VM-1 — set once brokered-console resolution honestly gated (the serving
    /// peer reported it can't broker a reachable endpoint). Pins the honest status
    /// and stops the per-frame poll from re-reading a doomed session.
    #[cfg(feature = "live-vdi")]
    broker_resolution_gated: bool,
    /// vdi-vm-4 / shell-ux-1 — the live session's connection phase. `Live` while the
    /// transport is up (or on the initial dial); a transport drop that is NOT a user
    /// close walks it through `Reconnecting{attempt}` to `Failed`, and a fresh frame
    /// walks it back to `Live`. Drives the auto-reconnect scheduler + the honest
    /// overlay so they can never disagree.
    #[cfg(feature = "live-vdi")]
    session_phase: SessionPhase,
    /// vdi-vm-4 — when the next bounded re-dial is due (set on a drop with the capped
    /// [`reconnect_backoff`], cleared once the re-dial fires / the session recovers /
    /// the operator closes). The per-frame [`Self::poll_reconnect`] fires at it.
    #[cfg(feature = "live-vdi")]
    reconnect_at: Option<std::time::Instant>,
    /// vdi-vm-8 — the size (device px) the current live transport was dialed at (the
    /// last `preferred_size` passed to [`Self::spawn_live_transport`]). Lets a resize
    /// check avoid re-arming for a geometry already requested but not yet repainted by
    /// the guest. `None` on the fallback / bus-driven paths that pass no size.
    #[cfg(feature = "live-vdi")]
    negotiated_size: Option<(u16, u16)>,
    /// vdi-vm-8 — a debounced resize re-negotiation in flight for a live RDP/SPICE
    /// desktop: set when the panel drifts materially from the guest's real size and
    /// cleared once it settles back or the session leaves `Live`. Fired by
    /// [`Self::poll_resize_renegotiate`]. VNC (server-authoritative) never arms it.
    #[cfg(feature = "live-vdi")]
    pending_resize: Option<PendingResize>,
}

impl VdiState {
    /// Replace the Remote Sessions browser input with one already bounded and
    /// semantically admitted universal resource-catalog snapshot.
    pub(crate) fn install_resource_catalog(
        &mut self,
        catalog: mackes_mesh_types::resources::ResourceCatalog,
    ) -> Result<(), String> {
        self.remote_sessions.install_catalog(catalog)
    }

    /// Preserve the last admitted cards while making a failed refresh explicit.
    pub(crate) fn mark_resource_catalog_reconnecting(&mut self, detail: impl Into<String>) {
        self.remote_sessions.mark_reconnecting(detail);
    }

    /// Make absence of a usable resource snapshot explicit.
    pub(crate) fn mark_resource_catalog_unavailable(&mut self, detail: impl Into<String>) {
        self.remote_sessions.mark_unavailable(detail);
    }

    #[cfg(feature = "live-vdi")]
    pub(crate) fn set_clipboard_permission_ingress(&mut self, ingress: ClipboardPermissionIngress) {
        self.clipboard_permissions = Some(ingress);
    }

    /// Return the current bounded VDI measurements without exposing transport
    /// credentials, endpoints, or raw decoder state.
    pub(crate) fn metrics_snapshot(&self) -> VdiMetricsSnapshot {
        self.metrics.snapshot
    }

    fn queue_frame(&mut self, img: egui::ColorImage, damage: FrameDamage) {
        self.metrics.note_frame();
        self.incoming = Some(img);
        self.incoming_damage = Some(damage);
        self.presentation_input_authorized = true;
    }

    /// Revoke all presentation state owned by a superseded attachment. The
    /// retained request is only reconnect metadata; it does not carry frame or
    /// input authority into the next transport generation.
    fn revoke_presentation_authority(&mut self, clear_frame: bool) {
        self.presentation_input_authorized = false;
        self.session = None;
        self.incoming = None;
        self.incoming_damage = None;
        if clear_frame {
            self.texture = None;
        }
    }

    /// Take (and clear) the "return to chrome" request raised by the Esc chord.
    /// The shell calls this after mounting the panel to leave the surface.
    pub(crate) fn take_return_to_chrome(&mut self) -> bool {
        std::mem::take(&mut self.return_to_chrome)
    }

    /// Raise the "return to chrome" request from a control other than the Esc chord
    /// (MENUBAR-ALL — the Desktop bar's **Session → Return to Mesh Control**). It is
    /// the SAME seam [`forward_input`] sets, drained by [`Self::take_return_to_chrome`]
    /// after the panel mounts, so the menu path adds no new behaviour (§6).
    pub(crate) const fn request_return_to_chrome(&mut self) {
        self.return_to_chrome = true;
    }

    /// A log-safe summary of the pending connect for the Desktop bar's status
    /// cluster: the VM/host name plus the chosen protocol's label, or `None` when no
    /// connect is pending (the surface is on the Chooser). Carries no secret.
    pub(crate) fn requested_summary(&self) -> Option<(&str, &'static str)> {
        self.requested
            .as_ref()
            .map(|r| (r.target.name.as_str(), r.protocol.label()))
    }

    /// Return a live thumbnail source for the current Desktop session.
    /// No texture means no frame has landed yet, so callers keep their static
    /// protocol-card fallback.
    pub(crate) fn session_preview_frame(&self) -> Option<crate::surfaces::SessionPreviewTexture> {
        let request = self.requested.as_ref()?;
        let texture = self.texture.clone()?;
        Some(crate::surfaces::SessionPreviewTexture::new(
            request
                .broker_session
                .as_ref()
                .map(|broker| broker.id.clone()),
            &request.target.name,
            request.protocol.label(),
            texture,
        ))
    }

    /// Focus a broker session already owned by this shell.
    ///
    /// The current session is a strict no-op. A previously parked session is
    /// restored from its exact retained request; no roster-derived VNC request,
    /// replacement `Open`, or terminal `Close` is emitted. A different current
    /// session is parked as nonterminal first so it remains available for a later
    /// focus switch.
    pub(crate) fn focus_broker_session(&mut self, id: &str) -> Option<SessionFocusSurface> {
        if self.requested_session_id() == Some(id) {
            return self.requested.as_ref().map(request_focus_surface);
        }

        let request = self.retained_requests.remove(id)?;
        self.park_current_request();
        let surface = request_focus_surface(&request);
        self.install_request(request);
        Some(surface)
    }

    /// Record the connect the Chooser's picker chose (CHOOSER-4). The surface then
    /// shows a "connecting" state naming the target + chosen protocol until the
    /// gated wire transport attaches the live decoder session.
    pub(crate) fn request_connect(&mut self, request: ConnectRequest) {
        if request
            .broker_session
            .as_ref()
            .is_some_and(|broker| self.requested_session_id() == Some(broker.id.as_str()))
        {
            // Re-selecting the exact broker identity is focus, not reconnect. Keep
            // the original request and live transport because the incoming value
            // may have been reconstructed from lossy roster presentation data.
            return;
        }
        self.park_current_request();
        self.install_request(request);
    }

    /// Install one exact request after any prior session has already been parked
    /// or explicitly closed. This never publishes broker lifecycle records.
    fn install_request(&mut self, request: ConnectRequest) {
        #[cfg(test)]
        {
            self.transport_install_attempted = true;
        }
        self.route_status = None;
        self.revoke_presentation_authority(true);
        #[cfg(feature = "live-vdi")]
        {
            self.live_status = None;
            self.broker_resolution_gated = false;
            // A fresh operator-initiated connect is a clean start, never a reconnect:
            // reset the phase to `Live` and cancel any pending re-dial before the new
            // handle is installed, so a leftover `Reconnecting`/`Failed` from a prior
            // session cannot bleed into (or auto-reconnect) this one (vdi-vm-4).
            self.session_phase = SessionPhase::Live;
            self.reconnect_at = None;
            // vdi-vm-8 — a fresh operator connect renegotiates from scratch; drop any
            // in-flight resize re-dial and the prior dialed size.
            self.pending_resize = None;
            self.negotiated_size = None;
            if let Some(live) = self.live_rdp.take() {
                live.stop();
            }
            if let Some(live) = self.live_vnc.take() {
                live.stop();
            }
            if let Some(live) = self.live_spice.take() {
                live.stop();
            }
            // A broker-session request without an endpoint belonged to the retired
            // raw console relay. Native presentation now requires an authenticated
            // lease from the typed Workload projection; fail closed here.
            if request.target.endpoint.is_none()
                && request.broker_session.is_some()
                && request.android_source.is_none()
            {
                self.live_status = Some(
                    "This legacy desktop session has no typed Workload attachment lease. Nothing was attached."
                        .to_string(),
                );
                self.broker_resolution_gated = true;
            } else {
                self.spawn_live_transport(&request);
            }
        }
        self.requested = Some(request);
    }

    /// Park the current brokered request without terminally closing its roster
    /// record. A live transport is disconnected before the exact request is
    /// retained; direct/off-mesh requests have no stable broker identity and are
    /// simply released.
    fn park_current_request(&mut self) {
        let Some(request) = self.requested.take() else {
            return;
        };

        self.revoke_presentation_authority(true);

        #[cfg(feature = "live-vdi")]
        {
            self.publish_broker_disconnect_if_active();
            if let Some(live) = self.live_rdp.take() {
                live.stop();
            }
            if let Some(live) = self.live_vnc.take() {
                live.stop();
            }
            if let Some(live) = self.live_spice.take() {
                live.stop();
            }
            self.live_status = None;
            self.broker_resolution_gated = false;
            self.session_phase = SessionPhase::Live;
            self.reconnect_at = None;
            self.pending_resize = None;
            self.negotiated_size = None;
        }

        if let Some(id) = request
            .broker_session
            .as_ref()
            .map(|broker| broker.id.clone())
        {
            self.retained_requests.insert(id, request);
        }
        self.route_status = None;
    }

    /// Attach a focused App VM rail session to the existing brokered VDI path.
    /// App sessions use VNC as the universal console fallback while the serving
    /// console broker resolves the actual endpoint; this does not expose the
    /// guest desktop as a separate host window or execute a catalog command.
    pub(crate) fn request_app_connect(
        &mut self,
        handoff: crate::session_rail::AppSessionHandoff,
        client_peer: &str,
        bus_root: Option<PathBuf>,
        preferred_size: Option<(u16, u16)>,
    ) {
        // `SessionRailState` admits the complete request before producing this
        // handoff. Retain that declaration's identities here rather than
        // rebuilding a session id or app identity from presentation text.
        let app_id = handoff.request.app_id.clone();
        let session_id = handoff.request.session_id.clone();
        let request = ConnectRequest::new(
            RequestedTarget::new(handoff.serving_peer, handoff.vm_id),
            VdiProtocol::Vnc,
            DisplayMode::Fullscreen,
            MonitorSpan::Single,
            DesktopAuth::mesh_identity(client_peer),
        )
        .with_app_id(app_id)
        .with_broker_session(BrokerSessionLifecycle::new(session_id, bus_root))
        .with_preferred_size(preferred_size);
        self.request_connect(request);
    }

    /// Consume one governed Android source through the existing authorized
    /// Remote Sessions broker. No endpoint is parsed or dialed here: the full
    /// source remains attached to the request as exact identity evidence.
    pub(crate) fn request_android_webrtc_connect(
        &mut self,
        handoff: crate::iac::AndroidVdiHandoff,
        client_peer: &str,
        bus_root: Option<PathBuf>,
        now_ms: u64,
    ) -> Result<(), String> {
        let source = handoff.source;
        source
            .validate()
            .map_err(|error| format!("Android VDI source is invalid: {error}"))?;
        if source.protocol != AndroidVdiProtocol::WebRtc {
            return Err("Android VDI source requested an unsupported protocol.".to_owned());
        }
        if source.observed_at_unix_ms > now_ms || source.expires_at_unix_ms <= now_ms {
            return Err(
                "Android WebRTC readiness expired; retry the app from Workloads.".to_owned(),
            );
        }
        let request = ConnectRequest::new(
            RequestedTarget::new(&handoff.placement_node, &source.workload_id),
            VdiProtocol::WebRtc,
            DisplayMode::Fullscreen,
            MonitorSpan::Single,
            DesktopAuth::mesh_identity(client_peer),
        )
        .with_android_source(source.clone());
        let publication = match crate::discovery::publish_exact_open_record(
            bus_root.as_deref(),
            &source.session_id,
            &handoff.placement_node,
            &source.workload_id,
            client_peer,
        ) {
            Ok(publication) => publication,
            Err(error) => {
                self.retain_refused_android_request(
                    request,
                    format!(
                        "Remote Sessions attachment authorization failed: {error} Return to Workloads and retry."
                    ),
                );
                return Err(error);
            }
        };
        self.request_connect(
            request.with_broker_session(BrokerSessionLifecycle::new(publication.id, bus_root)),
        );
        self.route_status = Some(
            "Remote Sessions authorized this exact Android generation and session, but this shell has no Cuttlefish WebRTC decoder. Return to Workloads to stop or retry the session."
                .to_owned(),
        );
        Ok(())
    }

    /// Retain only enough typed identity to render an actionable refusal. This
    /// deliberately bypasses `request_connect`/`install_request`, so neither the
    /// current transport set nor a future WebRTC decoder can run without a
    /// successful Remote Sessions authorization record.
    fn retain_refused_android_request(&mut self, request: ConnectRequest, status: String) {
        self.park_current_request();
        self.requested = Some(request);
        self.route_status = Some(status);
        #[cfg(feature = "live-vdi")]
        {
            self.broker_resolution_gated = true;
            self.live_status = None;
        }
    }

    /// Spawn the live decoder transport for `request` (RDP / VNC / SPICE), routing
    /// the honest gate into `live_status` on failure. Shared by the direct-endpoint
    /// path ([`Self::request_connect`]).
    #[cfg(feature = "live-vdi")]
    fn spawn_live_transport(&mut self, request: &ConnectRequest) {
        // vdi-vm-8 — record the size this dial negotiates at so a later resize check
        // doesn't re-arm for a geometry already requested (see `note_resize_target`).
        self.negotiated_size = request.preferred_size;
        match request.protocol {
            VdiProtocol::WebRtc => {
                self.live_status = Some(
                    "Cuttlefish WebRTC is authorized through Remote Sessions, but its seat-side decoder is unavailable. Return to Workloads to stop or retry."
                        .to_owned(),
                );
                self.broker_resolution_gated = true;
            }
            VdiProtocol::Moonlight => {
                self.live_status = Some(
                    "Sunshine/Moonlight selected; the host Moonlight adapter is unavailable. RDP was not attempted."
                        .to_string(),
                );
                self.broker_resolution_gated = true;
            }
            VdiProtocol::Rdp => {
                match LiveRdpHandle::spawn(request, self.clipboard_permissions.clone()) {
                    Ok(handle) => {
                        self.live_status = Some("Opening live RDP transport".to_string());
                        self.live_rdp = Some(handle);
                    }
                    Err(reason) => {
                        self.live_status = Some(format!("Live RDP gated: {reason}"));
                    }
                }
            }
            VdiProtocol::Vnc => {
                match LiveVncHandle::spawn(request, self.clipboard_permissions.clone()) {
                    Ok(handle) => {
                        self.live_status = Some("Opening live VNC transport".to_string());
                        self.live_vnc = Some(handle);
                    }
                    Err(reason) => {
                        self.live_status = Some(format!("Live VNC gated: {reason}"));
                    }
                }
            }
            VdiProtocol::Spice => {
                match LiveSpiceHandle::spawn(request, self.clipboard_permissions.clone()) {
                    Ok(handle) => {
                        self.live_status = Some("Opening live SPICE transport".to_string());
                        self.live_spice = Some(handle);
                    }
                    Err(reason) => {
                        self.live_status = Some(format!("Live SPICE gated: {reason}"));
                    }
                }
            }
        }
    }

    /// The picked target, if any — the shell reads it to decide whether the Desktop
    /// surface shows the Chooser (none) or the connecting/desktop state.
    pub(crate) fn requested_target(&self) -> Option<&RequestedTarget> {
        self.requested.as_ref().map(|r| &r.target)
    }

    /// The broker session identity currently attached to the Desktop surface,
    /// when the live request came from the mesh session roster.
    pub(crate) fn requested_session_id(&self) -> Option<&str> {
        self.requested
            .as_ref()
            .and_then(|request| request.broker_session.as_ref())
            .map(|broker| broker.id.as_str())
    }

    /// Exact governed Android identity retained for the current request.
    pub(crate) fn requested_android_source(&self) -> Option<&AndroidVdiSource> {
        self.requested
            .as_ref()
            .and_then(|request| request.android_source.as_ref())
    }

    /// Clear the pending connect — the operator backed out before a live session
    /// attached, so the Desktop surface falls back to the Chooser.
    pub(crate) fn clear_target(&mut self) {
        // `Requested` is already a real broker roster record. Close the current
        // identity whether or not a transport reached Active so an unavailable
        // Sunshine attempt cannot leak when the operator chooses explicit RDP.
        self.publish_broker_close_current();
        self.revoke_presentation_authority(true);
        #[cfg(feature = "live-vdi")]
        {
            if let Some(live) = self.live_rdp.take() {
                live.stop();
            }
            if let Some(live) = self.live_vnc.take() {
                live.stop();
            }
            if let Some(live) = self.live_spice.take() {
                live.stop();
            }
            self.live_status = None;
            self.broker_resolution_gated = false;
            // A user-initiated close is NOT a transport drop: reset the phase to
            // `Live` and cancel any pending re-dial so backing out never enters (or
            // resumes) auto-reconnect (vdi-vm-4, requirement 3).
            self.session_phase = SessionPhase::Live;
            self.reconnect_at = None;
            // vdi-vm-8 — backing out cancels any pending resize re-dial too.
            self.pending_resize = None;
            self.negotiated_size = None;
        }
        self.requested = None;
        self.route_status = None;
    }

    /// vdi-vm-4 — a transport drop that is NOT a user-initiated close. Walks the
    /// session phase forward ([`next_phase_on_drop`]): a first drop opens
    /// `Reconnecting{1}` and schedules a bounded backoff re-dial; each further drop
    /// bumps the attempt; the last drop `Failed`s the session with the honest reason
    /// and stops retrying. The caller has already taken the dead handle.
    #[cfg(feature = "live-vdi")]
    fn on_transport_drop(&mut self, reason: String) {
        // The frozen texture remains useful context, but no longer proves that
        // the replacement transport is the session shown on screen.
        self.presentation_input_authorized = false;
        self.metrics.note_reconnect();
        let next = next_phase_on_drop(&self.session_phase, reason, MAX_RECONNECT_ATTEMPTS);
        match &next {
            SessionPhase::Reconnecting { attempt, .. } => {
                self.reconnect_at = Some(std::time::Instant::now() + reconnect_backoff(*attempt));
            }
            SessionPhase::Failed { reason } => {
                self.live_status = Some(format!(
                    "Desktop disconnected \u{2014} {reason}. Could not reconnect after {MAX_RECONNECT_ATTEMPTS} attempts."
                ));
                self.reconnect_at = None;
            }
            SessionPhase::Live => {
                self.reconnect_at = None;
            }
        }
        self.session_phase = next;
    }

    /// vdi-vm-4 — a fresh frame from a (re-dialed) transport: the desktop is live
    /// again, so walk the phase back to `Live` and cancel any pending re-dial.
    #[cfg(feature = "live-vdi")]
    fn note_live_frame(&mut self) {
        self.presentation_input_authorized = true;
        if self.session_phase != SessionPhase::Live {
            self.session_phase = SessionPhase::Live;
        }
        self.reconnect_at = None;
    }

    /// vdi-vm-4 — fire a due bounded re-dial: once `reconnect_at` elapses while the
    /// session is `Reconnecting`, re-dial the SAME retained [`ConnectRequest`] (the
    /// endpoint + credentials are already on it). A re-dial that cannot even start
    /// (the gate reason lands in `live_status`) counts as another drop, so the ladder
    /// keeps advancing toward the honest `Failed` instead of stalling.
    #[cfg(feature = "live-vdi")]
    fn poll_reconnect(&mut self) {
        let Some(at) = self.reconnect_at else {
            return;
        };
        if std::time::Instant::now() < at {
            return;
        }
        self.reconnect_at = None;
        if !matches!(self.session_phase, SessionPhase::Reconnecting { .. }) {
            return;
        }
        let Some(request) = self.requested.clone() else {
            self.session_phase = SessionPhase::Failed {
                reason: "no retained desktop connection to reconnect".to_string(),
            };
            return;
        };
        self.spawn_live_transport(&request);
        if !self.has_live_transport() {
            let reason = self
                .live_status
                .clone()
                .unwrap_or_else(|| "the re-dial could not start".to_string());
            self.on_transport_drop(reason);
        }
    }

    /// vdi-vm-8 — observe this frame's real panel size (`panel_px`, device px) against
    /// the guest's real desktop size (`guest_px`, the live texture size) and arm /
    /// disarm a debounced resize re-negotiation. The RDP/SPICE thin transports fix
    /// their desktop size at dial time and expose no in-session resize, so the only
    /// way to fit a materially-resized panel is a fresh dial at the new size — armed
    /// here, fired by [`Self::poll_resize_renegotiate`] once the size settles. VNC is
    /// server-authoritative and resizes itself, so it never arms this; smaller deltas
    /// stay on the LINEAR upscale (imperceptible, no disruptive re-dial).
    #[cfg(feature = "live-vdi")]
    fn note_resize_target(&mut self, panel_px: (u16, u16), guest_px: (u16, u16)) {
        // Only an RDP/SPICE session re-negotiates by re-dialing; VNC excludes itself.
        let renegotiable = self.live_rdp.is_some() || self.live_spice.is_some();
        if !renegotiable || self.session_phase != SessionPhase::Live {
            self.pending_resize = None;
            return;
        }
        // Already dialed (or dialing) at ~this size — the guest just hasn't repainted
        // at the new geometry yet; the upscale bridges it. Don't re-arm.
        if let Some(neg) = self.negotiated_size {
            if !size_diverges(neg, panel_px, RESIZE_TARGET_TOLERANCE_PX) {
                self.pending_resize = None;
                return;
            }
        }
        // The guest's real desktop already matches the panel closely enough — the paint
        // is ~1:1, so there is nothing worth a disruptive re-dial.
        if !size_diverges(guest_px, panel_px, RESIZE_RENEGOTIATE_THRESHOLD_PX) {
            self.pending_resize = None;
            return;
        }
        // Arm (or keep) the settle timer toward the current panel size: a materially
        // different target restarts it; a target within tolerance keeps it counting.
        match self.pending_resize {
            Some(p) if !size_diverges(p.target, panel_px, RESIZE_TARGET_TOLERANCE_PX) => {}
            _ => {
                self.pending_resize = Some(PendingResize {
                    at: std::time::Instant::now() + RESIZE_SETTLE,
                    target: panel_px,
                });
            }
        }
    }

    /// vdi-vm-8 — fire a settled resize re-negotiation: once the pending target's
    /// settle window elapses while the session is still `Live`, re-dial the SAME
    /// retained request at the new panel size. This is a DELIBERATE, operator-invisible
    /// re-negotiation, NOT a vdi-vm-4 drop: the phase stays `Live`, the attempt ladder
    /// is untouched, and the last frame + texture stay painted (LINEAR-upscaled to the
    /// new panel) so the sub-second re-dial gap shows the old desktop stretched rather
    /// than the connecting backdrop. A re-dial that cannot even start degrades into the
    /// honest vdi-vm-4 drop ladder rather than silently losing the session.
    #[cfg(feature = "live-vdi")]
    fn poll_resize_renegotiate(&mut self) {
        let Some(pending) = self.pending_resize else {
            return;
        };
        if std::time::Instant::now() < pending.at {
            return;
        }
        self.pending_resize = None;
        // Guard: only re-dial a still-live RDP/SPICE session (a drop this frame may have
        // flipped us out of `Live`, or swapped in a VNC-only handle).
        if self.session_phase != SessionPhase::Live
            || !(self.live_rdp.is_some() || self.live_spice.is_some())
        {
            return;
        }
        let Some(request) = self
            .requested
            .clone()
            .map(|r| r.with_preferred_size(Some(pending.target)))
        else {
            return;
        };
        // Stop the current transport and re-dial at the new geometry; KEEP texture /
        // incoming so the last frame bridges the gap (the upscale fallback covers it).
        self.presentation_input_authorized = false;
        if let Some(live) = self.live_rdp.take() {
            live.stop();
        }
        if let Some(live) = self.live_spice.take() {
            live.stop();
        }
        self.spawn_live_transport(&request);
        // Persist the new size so a later vdi-vm-4 re-dial keeps the resized geometry.
        self.requested = Some(request);
        if !self.has_live_transport() {
            let reason = self
                .live_status
                .clone()
                .unwrap_or_else(|| "the resize re-dial could not start".to_string());
            self.on_transport_drop(reason);
        }
    }

    /// shell-ux-1 — the operator pressed **Retry / Reconnect** on the overlay: reset
    /// the attempt ladder and re-dial the SAME retained endpoint immediately (skipping
    /// any pending backoff), carrying the last honest drop reason so the overlay stays
    /// truthful until the re-dial produces a frame. Resets a terminal `Failed`.
    #[cfg(feature = "live-vdi")]
    fn retry_now(&mut self) {
        let reason = match &self.session_phase {
            SessionPhase::Failed { reason } | SessionPhase::Reconnecting { reason, .. } => {
                reason.clone()
            }
            SessionPhase::Live => String::new(),
        };
        self.presentation_input_authorized = false;
        if let Some(live) = self.live_rdp.take() {
            live.stop();
        }
        if let Some(live) = self.live_vnc.take() {
            live.stop();
        }
        if let Some(live) = self.live_spice.take() {
            live.stop();
        }
        self.reconnect_at = None;
        let Some(request) = self.requested.clone() else {
            self.session_phase = SessionPhase::Failed {
                reason: "no retained desktop connection to reconnect".to_string(),
            };
            return;
        };
        self.session_phase = SessionPhase::Reconnecting { attempt: 1, reason };
        self.spawn_live_transport(&request);
        if !self.has_live_transport() {
            let reason = self
                .live_status
                .clone()
                .unwrap_or_else(|| "the re-dial could not start".to_string());
            self.on_transport_drop(reason);
        }
    }

    /// Whether any live transport handle is currently installed — i.e. an RDP,
    /// VNC, or SPICE session is actually streaming (or connecting), not merely
    /// requested. The shell host loop reads this to keep repainting while guest
    /// frames are inbound (WL-PERF-002) without waking an idle seat for a
    /// no-session / chooser-only desktop.
    #[cfg(feature = "live-vdi")]
    pub(crate) fn has_live_transport(&self) -> bool {
        self.live_rdp.is_some() || self.live_vnc.is_some() || self.live_spice.is_some()
    }

    #[cfg(feature = "live-vdi")]
    fn poll_live_rdp(&mut self) {
        let Some(live) = self.live_rdp.as_ref() else {
            return;
        };
        let mut publish_active = false;
        let mut got_frame = false;
        let mut drop_reason = None;
        while let Ok(event) = live.event_rx.try_recv() {
            match event {
                LiveRdpEvent::Connected(target) => {
                    self.live_status = Some(format!("Live RDP connected to {target}"));
                    publish_active = true;
                }
                LiveRdpEvent::CertWarning(message) => {
                    // Non-fatal: keep the session live, just raise the banner.
                    self.live_status = Some(message);
                }
                LiveRdpEvent::ClipboardPublished => {}
                LiveRdpEvent::ClipboardFilesMaterialized { count, destination } => {
                    self.live_status = Some(format!(
                        "Saved {count} guest clipboard file(s) to {destination}"
                    ));
                }
                LiveRdpEvent::ClipboardRefused(refusal) => {
                    // Non-fatal and explicit: the live desktop remains usable,
                    // while status never claims the image reached Files.
                    self.live_status = Some(refusal.to_string());
                }
                LiveRdpEvent::Error(reason) => {
                    self.live_status = Some(reason.clone());
                    drop_reason = Some(reason);
                }
                LiveRdpEvent::Ended(reason) => {
                    self.live_status = Some(format!("RDP session ended: {reason}"));
                    drop_reason = Some(reason);
                }
            }
        }
        if let Some((frame, damage)) = live.frame_mailbox.take() {
            self.incoming = Some(frame);
            self.incoming_damage = Some(damage);
            self.metrics.note_frame();
            got_frame = true;
        }
        // A fresh frame means the desktop is live again (recovering a reconnect).
        if got_frame {
            self.note_live_frame();
        }
        if publish_active {
            self.publish_broker_active();
        }
        // The worker thread has died on its own — take the dead handle and drive the
        // session through the auto-reconnect phase machine (vdi-vm-4).
        if let Some(reason) = drop_reason {
            self.live_rdp = None;
            self.publish_broker_disconnect_if_active();
            self.on_transport_drop(reason);
        }
    }

    #[cfg(feature = "live-vdi")]
    fn poll_live_vnc(&mut self) {
        let Some(live) = self.live_vnc.as_ref() else {
            return;
        };
        let mut publish_active = false;
        let mut got_frame = false;
        let mut drop_reason = None;
        while let Ok(event) = live.event_rx.try_recv() {
            match event {
                LiveVncEvent::Connected(target) => {
                    self.live_status = Some(format!("Live VNC connected to {target}"));
                    publish_active = true;
                }
                LiveVncEvent::ClipboardPublished => {}
                LiveVncEvent::Error(reason) => {
                    self.live_status = Some(reason.clone());
                    drop_reason = Some(reason);
                }
                LiveVncEvent::Ended(reason) => {
                    self.live_status = Some(format!("VNC session ended: {reason}"));
                    drop_reason = Some(reason);
                }
            }
        }
        if let Some((frame, damage)) = live.frame_mailbox.take() {
            self.incoming = Some(frame);
            self.incoming_damage = Some(damage);
            self.metrics.note_frame();
            got_frame = true;
        }
        if got_frame {
            self.note_live_frame();
        }
        if publish_active {
            self.publish_broker_active();
        }
        if let Some(reason) = drop_reason {
            self.live_vnc = None;
            self.publish_broker_disconnect_if_active();
            self.on_transport_drop(reason);
        }
    }

    #[cfg(feature = "live-vdi")]
    fn poll_live_spice(&mut self) {
        let Some(live) = self.live_spice.as_ref() else {
            return;
        };
        let mut publish_active = false;
        let mut got_frame = false;
        let mut drop_reason = None;
        while let Ok(event) = live.event_rx.try_recv() {
            match event {
                LiveSpiceEvent::Connected(target) => {
                    self.live_status = Some(format!("Live SPICE connected to {target}"));
                    publish_active = true;
                }
                LiveSpiceEvent::ClipboardPublished => {
                    self.live_status = Some("SPICE guest clipboard awaiting seat approval".into());
                }
                LiveSpiceEvent::ClipboardStatus(status) => {
                    self.live_status = Some(status);
                }
                LiveSpiceEvent::Error(reason) => {
                    self.live_status = Some(reason.clone());
                    drop_reason = Some(reason);
                }
                LiveSpiceEvent::Ended(reason) => {
                    self.live_status = Some(format!("SPICE session ended: {reason}"));
                    drop_reason = Some(reason);
                }
            }
        }
        if let Some((frame, damage)) = live.frame_mailbox.take() {
            self.incoming = Some(frame);
            self.incoming_damage = Some(damage);
            self.metrics.note_frame();
            got_frame = true;
        }
        if got_frame {
            self.note_live_frame();
        }
        if publish_active {
            self.publish_broker_active();
        }
        if let Some(reason) = drop_reason {
            self.live_spice = None;
            self.publish_broker_disconnect_if_active();
            self.on_transport_drop(reason);
        }
    }

    /// Publish the terminal close for the currently focused broker identity.
    /// Unlike the transport-active helper, this also covers a request that never
    /// advanced beyond `Requested`.
    fn publish_broker_close_current(&mut self) {
        #[cfg(any(test, feature = "live-vdi"))]
        {
            #[cfg(feature = "live-vdi")]
            let active = self.active_broker_session.take();
            #[cfg(not(feature = "live-vdi"))]
            let active: Option<BrokerSessionLifecycle> = None;

            let Some(broker) = active.or_else(|| {
                self.requested
                    .as_ref()
                    .and_then(|request| request.broker_session.clone())
            }) else {
                return;
            };
            let mut last_error = None;
            crate::discovery::publish_close(
                broker.bus_root.as_deref(),
                &mut last_error,
                broker.id.as_str(),
            );
            if let Some(reason) = last_error {
                #[cfg(feature = "live-vdi")]
                {
                    self.live_status = Some(format!("Broker lifecycle gated: {reason}"));
                }
                #[cfg(not(feature = "live-vdi"))]
                let _ = reason;
            }
        }
    }

    #[cfg(feature = "live-vdi")]
    fn publish_broker_active(&mut self) {
        if self.active_broker_session.is_some() {
            return;
        }
        let Some(broker) = self
            .requested
            .as_ref()
            .and_then(|request| request.broker_session.clone())
        else {
            return;
        };
        let mut last_error = None;
        crate::discovery::publish_active(
            broker.bus_root.as_deref(),
            &mut last_error,
            broker.id.as_str(),
        );
        if let Some(reason) = last_error {
            self.live_status = Some(format!("Broker lifecycle gated: {reason}"));
        } else {
            self.active_broker_session = Some(broker);
        }
    }

    #[cfg(feature = "live-vdi")]
    fn publish_broker_disconnect_if_active(&mut self) {
        let Some(broker) = self.active_broker_session.take() else {
            return;
        };
        let mut last_error = None;
        crate::discovery::publish_disconnect(
            broker.bus_root.as_deref(),
            &mut last_error,
            broker.id.as_str(),
        );
        if let Some(reason) = last_error {
            self.live_status = Some(format!("Broker lifecycle gated: {reason}"));
        }
    }
}

/// A remote desktop is scaled to fill the shell body, so sample it linearly —
/// crisper than nearest when the negotiated desktop size doesn't match the panel.
const DESKTOP_TEX: TextureOptions = TextureOptions::LINEAR;

/// Upload one decoded desktop frame into `texture` (perf-7).
///
/// * **No texture yet** (the first frame) → allocate it from the whole image.
/// * **Concrete per-rectangle damage AND an unchanged texture size** →
///   [`TextureHandle::set_partial`] each damaged sub-rectangle, moving only the
///   changed pixels to the GPU. The size guard is essential: `set_partial` cannot
///   resize a texture, so a dimension change must go through the reallocating full
///   `set`.
/// * **Anything else** ([`FrameDamage::Full`], no damage, a size change, or an
///   empty rect list) → a full [`TextureHandle::set`] of the whole image.
///
/// Correctness over optimisation: a full `set` is always valid, so every uncertain
/// path degrades to it and no upload a full `set` would have done is ever skipped.
/// The `(offset, sub_image)` pairs handed to `set_partial` come from the same
/// [`sub_color_image`] slice the unit tests prove pixel-identical to a full upload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameUpload {
    Full,
    Partial { rects: u32 },
}

fn upload_frame(
    ctx: &egui::Context,
    texture: &mut Option<TextureHandle>,
    img: egui::ColorImage,
    damage: Option<FrameDamage>,
) -> FrameUpload {
    match texture.as_mut() {
        // First frame / freshly-(re)allocated texture: allocate from the whole image.
        None => {
            *texture = Some(ctx.load_texture("vdi-desktop", img, DESKTOP_TEX));
            FrameUpload::Full
        }
        Some(handle) => {
            // Partial-upload only with concrete rectangles AND a matching texture
            // size — a resize (or any size mismatch) must reallocate through the
            // full `set` below, because `set_partial` cannot resize a texture.
            let rects = match &damage {
                Some(FrameDamage::Rects(rects))
                    if !rects.is_empty() && handle.size() == img.size =>
                {
                    rects
                }
                _ => {
                    handle.set(img, DESKTOP_TEX);
                    return FrameUpload::Full;
                }
            };
            let mut uploaded_rects = 0u32;
            for rect in rects {
                // Each rect is clamped to the frame bounds; a fully-clipped one
                // yields None and is skipped (a full `set` would not draw it either).
                if let Some((offset, sub)) = sub_color_image(&img, *rect) {
                    handle.set_partial(offset, sub, DESKTOP_TEX);
                    uploaded_rects = uploaded_rects.saturating_add(1);
                }
            }
            FrameUpload::Partial {
                rects: uploaded_rects,
            }
        }
    }
}

/// Render the Desktop surface into `ui`: upload any new framebuffer, paint it to
/// fill the body, and forward this frame's egui input to the guest. With no
/// session attached it draws the honest "no desktop" EmptyState instead.
pub(crate) fn vdi_panel(ui: &mut egui::Ui, state: &mut VdiState) {
    state.metrics.note_shell_repaint();
    #[cfg(feature = "live-vdi")]
    {
        // vdi-vm-4 — fire a due bounded re-dial BEFORE draining transport events, so a
        // just-re-dialed transport's events are picked up on the next frame.
        state.poll_reconnect();
        // vdi-vm-8 — fire a settled resize re-negotiation on the same schedule, so a
        // just-re-dialed (resized) transport's first frame is drained next frame too.
        state.poll_resize_renegotiate();
        state.poll_live_rdp();
        state.poll_live_vnc();
        state.poll_live_spice();
    }

    // 1. Pull the newest decoded frame — plus which rectangles changed (perf-7) —
    //    off the live session into the upload slot.
    if let Some(session) = state.session.as_mut() {
        if let Some((img, damage)) = session.frame_with_damage() {
            state.queue_frame(img, damage);
        }
    }

    // 2. Upload a pending frame. The texture is allocated on the first frame; after
    //    that, a frame carrying per-rectangle damage moves only its changed
    //    sub-rectangles to the GPU with `set_partial`, and everything else
    //    (first frame, a resize, a whole-surface / batch replace, or no reliable
    //    damage info) falls back to a full `set` — never a skipped upload.
    if let Some(img) = state.incoming.take() {
        let damage = state.incoming_damage.take();
        let started = std::time::Instant::now();
        let upload = upload_frame(ui.ctx(), &mut state.texture, img, damage);
        state.metrics.note_upload(upload, started.elapsed());
    }

    // 3. Paint the desktop (or the EmptyState) and drive input.
    match state.texture.as_ref() {
        Some(texture) => {
            let tex_id = texture.id();
            // The uploaded framebuffer IS the guest desktop at its own negotiated
            // resolution, so the texture size is exactly the guest desktop size — the
            // denominator the pointer transform needs to turn a panel click into a
            // guest pixel (vdi-vm-2). Read it before the immutable `texture` borrow
            // ends so `forward_input` can re-borrow `state` mutably.
            let desktop_px = texture.size();
            // a11y-05 — the accessible description of the desktop about to paint,
            // read off the retained request before the mutable re-borrow below.
            let desktop_label = desktop_a11y_value(state);
            // Allocate the interactive body rect first, then paint the texture over
            // it, so the desktop both fills the panel and captures pointer input.
            let size = ui.available_size();
            let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
            egui::Image::new(egui::load::SizedTexture::new(tex_id, rect.size())).paint_at(ui, rect);
            // Clicking the desktop focuses it so keystrokes route to the guest.
            if resp.clicked() {
                resp.request_focus();
            }
            // a11y-05 — the remote-desktop landmark (a named `Role::Group` region)
            // so a screen reader announces which desktop is focused. Pure metadata.
            install_desktop_accessibility(ui.ctx(), resp.id, desktop_label, rect);
            let desktop_size = (
                u16::try_from(desktop_px[0]).unwrap_or(u16::MAX),
                u16::try_from(desktop_px[1]).unwrap_or(u16::MAX),
            );
            forward_input(ui, state, rect, desktop_size);
            // vdi-vm-8 — refine the live desktop geometry to the panel's REAL pixel size
            // (device px). On a MATERIAL panel resize (a seat / monitor resolution
            // change) an RDP/SPICE session is re-dialed at the true panel size so the
            // desktop fits ~1:1; smaller deltas stay on the LINEAR upscale below, and
            // VNC is left to the server. `size` is the panel's egui-points extent.
            #[cfg(feature = "live-vdi")]
            {
                let panel_px =
                    target_desktop_size(size, ui.ctx().pixels_per_point(), seat_max_px(ui.ctx()));
                state.note_resize_target(panel_px, desktop_size);
            }
            // shell-ux-1 — if the session dropped, paint the honest reconnect / failure
            // overlay OVER this (now frozen) last frame, with working Retry and
            // Pick-a-different affordances wired to real session seams (vdi-vm-4).
            #[cfg(feature = "live-vdi")]
            if let Some(overlay) = session_overlay(&state.session_phase, MAX_RECONNECT_ATTEMPTS) {
                match paint_session_overlay(ui, rect, &overlay) {
                    Some(OverlayAction::Retry) => state.retry_now(),
                    Some(OverlayAction::PickDifferent) => state.clear_target(),
                    None => {}
                }
            }

            // App VM sessions retain Construct ownership of the visible surface:
            // expose the app identity and an explicit close affordance over the
            // guest framebuffer instead of making the operator manage a guest
            // desktop window.
            let app_id = state
                .requested
                .as_ref()
                .and_then(|request| request.app_id.clone());
            if let Some(app_id) = app_id {
                if paint_app_surface_chrome(ui, rect, &app_id) {
                    state.clear_target();
                    state.return_to_chrome = true;
                }
            }
        }
        None => {
            // No live desktop texture: the empty Desktop surface paints the BRAND-1
            // backdrop — the centered logo lockup (full opacity, breathing while
            // idle) with any honest status relocated to a small line BELOW the image
            // (lock 2), never over it. The backdrop owns the crossfade/breathe motion
            // (lock 10), so there is no bespoke caption ease here.
            match state.requested.as_ref() {
                // The Chooser's picker chose a connect but no live decoder is
                // attached yet (the wire transport is gated) — the status honestly
                // names the desktop + the chosen protocol/display below the logo,
                // never a placeholder render (§7).
                Some(req) => {
                    let title = req.android_source.as_ref().map_or_else(
                        || {
                            req.app_id.as_ref().map_or_else(
                                || {
                                    format!(
                                        "Connecting to {} via {}",
                                        req.target.name,
                                        req.protocol.label()
                                    )
                                },
                                |app_id| format!("Opening {app_id} via {}", req.protocol.label()),
                            )
                        },
                        |source| {
                            format!(
                                "Android {} · generation {} · session {}",
                                source.workload_id, source.generation, source.session_id
                            )
                        },
                    );
                    // CHOOSER-6 — name the auth mode honestly (SSO vs sealed cred);
                    // `auth.summary()` is log-safe and never carries the secret.
                    let auth = req.auth.summary();
                    let endpoint = req
                        .target
                        .endpoint
                        .as_ref()
                        .map_or_else(|| req.target.serving_peer.clone(), DesktopEndpoint::label);
                    let live_status = {
                        if let Some(status) = &state.route_status {
                            status.clone()
                        } else {
                            #[cfg(feature = "live-vdi")]
                            {
                                state
                                    .live_status
                                    .as_deref()
                                    .unwrap_or("Waiting for the live transport")
                                    .to_string()
                            }
                            #[cfg(not(feature = "live-vdi"))]
                            {
                                "the live transport is not compiled into this shell build"
                                    .to_string()
                            }
                        }
                    };
                    let detail = format!(
                        "Brokering the {} desktop from {} ({} \u{00B7} {} \u{00B7} {auth}) — {live_status}; {}.",
                        req.protocol.client_crate(),
                        endpoint,
                        req.display.label(),
                        req.monitors.label(),
                        req.protocol.clipboard_summary(),
                    );
                    crate::backdrop::show(
                        ui,
                        crate::backdrop::Coverage::Empty,
                        Some((title.as_str(), detail.as_str())),
                    );
                }
                None => resources::remote_sessions_panel(ui, &mut state.remote_sessions),
            }
        }
    }
}

/// Paint the minimal Construct-owned chrome for a catalog-backed app surface.
/// The guest compositor remains inside the framebuffer; the shell owns the
/// identity and close action so the app is never presented as an unmanaged
/// host window.
fn paint_app_surface_chrome(ui: &mut egui::Ui, body: egui::Rect, app_id: &str) -> bool {
    use egui::RichText;
    use mde_egui::Style;

    let mut close = false;
    egui::Area::new(egui::Id::new(("vdi-app-surface-chrome", app_id)))
        .order(egui::Order::Foreground)
        .fixed_pos(body.min + egui::vec2(Style::SP_M, Style::SP_S))
        .show(ui.ctx(), |ui| {
            egui::Frame::default()
                .fill(Style::SURFACE)
                .corner_radius(egui::CornerRadius::same(Style::RADIUS_M as u8))
                .inner_margin(egui::Margin::symmetric(
                    Style::SP_S as i8,
                    Style::SP_XS as i8,
                ))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("App VM · {app_id}"))
                                .size(Style::SMALL)
                                .strong()
                                .color(Style::TEXT),
                        );
                        ui.add_space(Style::SP_S);
                        if ui
                            .add(egui::Button::new(
                                RichText::new("Close app")
                                    .size(Style::SMALL)
                                    .color(Style::TEXT_DIM),
                            ))
                            .clicked()
                        {
                            close = true;
                        }
                    });
                });
        });
    close
}

/// shell-ux-1 — paint the honest reconnect / failure overlay OVER the (frozen) last
/// desktop frame that fills `body`, and return the affordance the operator pressed
/// this frame, if any. The content is the pure [`SessionOverlay`] model (asserted by
/// the unit tests); this function only renders it and reports the button press, so
/// the panel can route it to a real seam ([`VdiState::retry_now`] /
/// [`VdiState::clear_target`]). Never a dead-end (§7).
#[cfg(feature = "live-vdi")]
fn paint_session_overlay(
    ui: &mut egui::Ui,
    body: egui::Rect,
    overlay: &SessionOverlay,
) -> Option<OverlayAction> {
    use egui::RichText;
    use mde_egui::Style;

    // Dim the frozen desktop so the honest status reads clearly over it.
    ui.painter()
        .rect_filled(body, egui::CornerRadius::ZERO, Style::SCRIM);

    // The session-state accent carries the honest connection state: a transient
    // reconnect reads as a WARNING, a terminal failure as an ERROR (the shared
    // support-tone tokens, never a minted colour).
    let accent = if overlay.failed {
        Style::SUPPORT_ERROR
    } else {
        Style::SUPPORT_WARNING
    };
    let mut chosen = None;
    egui::Area::new(egui::Id::new("vdi-session-overlay"))
        .order(egui::Order::Foreground)
        .fixed_pos(body.center() - egui::vec2(220.0, 80.0))
        .show(ui.ctx(), |ui| {
            // The honest status sheet is a modal: the shared `dialog()` primitive
            // carries the surface fill, hairline border, large radius, generous
            // padding, and the translucent Modal depth — lifted off the dimmed
            // desktop with the look sourced only from `mde_egui` (§4, lock #2).
            mde_egui::dialog().show(ui, |ui| {
                ui.set_max_width(440.0);
                ui.label(
                    RichText::new(&overlay.title)
                        .size(Style::TITLE)
                        .strong()
                        .color(accent),
                );
                ui.add_space(Style::SP_XS);
                ui.label(
                    RichText::new(&overlay.detail)
                        .size(Style::BODY)
                        .color(Style::TEXT_DIM),
                );
                ui.add_space(Style::SP_M);
                ui.horizontal(|ui| {
                    for action in &overlay.actions {
                        let (label, fill) = match action {
                            OverlayAction::Retry => (
                                if overlay.failed {
                                    "Reconnect"
                                } else {
                                    "Retry now"
                                },
                                Style::ACCENT,
                            ),
                            OverlayAction::PickDifferent => {
                                ("Pick a different desktop", Style::SURFACE_HI)
                            }
                        };
                        let button = egui::Button::new(
                            RichText::new(label).size(Style::SMALL).color(Style::TEXT),
                        )
                        .fill(fill);
                        if ui.add(button).clicked() {
                            chosen = Some(*action);
                        }
                        ui.add_space(Style::SP_S);
                    }
                });
            });
        });
    chosen
}

/// Forward this frame's egui input to the attached guest, reserving the Esc chord.
///
/// Esc releases the desktop back to the mesh-control chrome instead of reaching
/// the guest, so the operator is never trapped in a fullscreen session. Pointer
/// positions are transformed from egui panel space into guest desktop pixels
/// (`rect` + `desktop_size`) in this ONE shared place, so all three transports
/// receive identically-mapped coordinates (vdi-vm-2). Every other event is handed
/// through unchanged; the session maps the ones it understands (pointer / button /
/// wheel / key / text) and drops the rest.
fn forward_input(ui: &egui::Ui, state: &mut VdiState, rect: egui::Rect, desktop_size: (u16, u16)) {
    let has_live = {
        #[cfg(feature = "live-vdi")]
        {
            state.live_rdp.is_some() || state.live_vnc.is_some() || state.live_spice.is_some()
        }
        #[cfg(not(feature = "live-vdi"))]
        {
            false
        }
    };
    if state.session.is_none() && !has_live {
        return;
    }
    for event in ui.input(|i| i.events.clone()) {
        if matches!(
            event,
            egui::Event::Key {
                key: egui::Key::Escape,
                pressed: true,
                ..
            }
        ) {
            state.return_to_chrome = true;
            continue;
        }
        // A retained texture from a disconnected or resizing generation is
        // display-only. Wait for the current transport's first frame before any
        // pointer/key event can control it.
        if !state.presentation_input_authorized {
            continue;
        }
        // Transform pointer coordinates into guest desktop pixels BEFORE handing the
        // event to any transport, so every transport applies the same mapping and
        // clicks land on the pixel under the cursor (vdi-vm-2).
        let event = remap_pointer_event(event, rect, desktop_size);
        if let Some(session) = state.session.as_mut() {
            session.send_input(&event);
        }
        #[cfg(feature = "live-vdi")]
        if let Some(live) = state.live_rdp.as_ref() {
            live.send_input(event.clone());
        }
        #[cfg(feature = "live-vdi")]
        if let Some(live) = state.live_vnc.as_ref() {
            live.send_input(event.clone());
        }
        #[cfg(feature = "live-vdi")]
        if let Some(live) = state.live_spice.as_ref() {
            live.send_input(event);
        }
    }
}

// ── accesskit (a11y-05 / shell-ux-6) ─────────────────────────────────────────
//
// The live remote desktop is one raw-painted cell: [`vdi_panel`] allocates the
// body rect (`Sense::click_and_drag`) and paints the guest framebuffer over it,
// so egui auto-generates no accesskit node — a screen reader landing on the
// Desktop surface heard nothing. The guest's OWN pixels are opaque to a host
// reader (that is the guest OS's own a11y stack), but the shell can announce
// the landmark: which remote desktop is focused, and that input routes into it.
// This installs a `Role::Group` landmark on the desktop cell — a named region
// (not a `Button`: a click focuses the desktop, it doesn't fire a discrete
// action) carrying the connected-desktop description as its value.

/// Convert an egui rect to an accesskit one (the shell-wide per-module helper).
fn accesskit_rect(rect: egui::Rect) -> egui::accesskit::Rect {
    egui::accesskit::Rect {
        x0: rect.min.x.into(),
        y0: rect.min.y.into(),
        x1: rect.max.x.into(),
        y1: rect.max.y.into(),
    }
}

/// The accessible description of the live desktop cell — the connected desktop's
/// name + the chosen protocol from the retained request, so a screen reader
/// announces which remote desktop is focused. Falls back to a plain "Connected
/// desktop" when no request record is retained (a bus-driven session).
fn desktop_a11y_value(state: &VdiState) -> String {
    match state.requested.as_ref() {
        Some(req) => req.app_id.as_ref().map_or_else(
            || format!("{} via {}", req.target.name, req.protocol.label()),
            |app_id| {
                format!(
                    "{app_id} on {} via {}",
                    req.target.name,
                    req.protocol.label()
                )
            },
        ),
        None => "Connected desktop".to_string(),
    }
}

/// Install the live desktop cell's accesskit landmark node.
fn install_desktop_accessibility(
    ctx: &egui::Context,
    id: egui::Id,
    value: impl Into<String>,
    rect: egui::Rect,
) {
    let _ = ctx.accesskit_node_builder(id, |node| {
        node.set_role(egui::accesskit::Role::Group);
        node.set_label("Remote desktop");
        node.set_value(value.into());
        node.set_bounds(accesskit_rect(rect));
    });
}

/// A small deterministic RGBA gradient standing in for a decoded desktop frame —
/// the render test drives the upload + paint path without a live server.
#[cfg(test)]
pub(crate) fn mock_frame() -> egui::ColorImage {
    const W: usize = 16;
    const H: usize = 12;
    let mut rgba = Vec::with_capacity(W * H * 4);
    for y in 0..H {
        let g = u8::try_from(y * 255 / (H - 1)).expect("gradient byte in 0..=255");
        for x in 0..W {
            let r = u8::try_from(x * 255 / (W - 1)).expect("gradient byte in 0..=255");
            rgba.extend_from_slice(&[r, g, 128, 255]);
        }
    }
    egui::ColorImage::from_rgba_unmultiplied([W, H], &rgba)
}

#[cfg(test)]
mod presentation_authority_tests {
    use super::*;

    #[test]
    fn replacement_request_revokes_stale_frame_input_authority_until_new_frame() {
        let mut state = VdiState::default();
        state.request_connect(ConnectRequest::new(
            RequestedTarget::new("node-old", "desktop-old"),
            VdiProtocol::Rdp,
            DisplayMode::Fullscreen,
            MonitorSpan::Single,
            DesktopAuth::mesh_identity("seat-a"),
        ));
        state.queue_frame(mock_frame(), FrameDamage::Full);
        assert!(state.presentation_input_authorized);
        assert!(state.incoming.is_some());

        // A hostile ordering replaces the attachment before the old decoded
        // frame is uploaded. Neither that frame nor its input authority may be
        // inherited by the newly selected desktop.
        state.request_connect(ConnectRequest::new(
            RequestedTarget::new("node-new", "desktop-new"),
            VdiProtocol::Rdp,
            DisplayMode::Fullscreen,
            MonitorSpan::Single,
            DesktopAuth::mesh_identity("seat-a"),
        ));
        assert_eq!(
            state.requested_target().map(|target| target.name.as_str()),
            Some("desktop-new")
        );
        assert!(!state.presentation_input_authorized);
        assert!(state.incoming.is_none());
        assert!(state.incoming_damage.is_none());
        assert!(state.session.is_none());

        state.queue_frame(mock_frame(), FrameDamage::Full);
        assert!(state.presentation_input_authorized);
    }
}

#[cfg(all(test, feature = "live-vdi"))]
mod guest_files_materialization_tests {
    use super::*;

    #[test]
    fn staged_guest_files_enter_the_permission_contract_without_host_paths() {
        let now = 1_700_000_000_000_u64;
        let lease = VdiClipboardLeaseV2 {
            schema_version: VDI_CLIPBOARD_TRANSPORT_V2_SCHEMA_VERSION,
            session_id: "rdp:oak:desktop-1".into(),
            generation: 9,
            lease_id: "rdp-lease-9".into(),
            issued_at_ms: now,
            expires_at_ms: now + 60_000,
            permitted_mime_offers: vec![VDI_GUEST_FILES_MIME.into()],
        };
        let message = rdp_guest_files_clipboard_message(
            &lease,
            1,
            2,
            ClipboardEnvelopeV2::content_hash_for(b"reportchart"),
            11,
            "files:v2:vdi-guest:tx-1".into(),
            now + 1,
        )
        .expect("staged Files result enters the same one-use permission contract");

        assert_eq!(message.selected_mime, VDI_GUEST_FILES_MIME);
        assert_eq!(message.envelope.byte_count, 11);
        assert_eq!(
            message.envelope.files_reference.as_deref(),
            Some("files:v2:vdi-guest:tx-1")
        );
        let encoded = serde_json::to_string(&message).unwrap();
        assert!(!encoded.contains("report.txt"));
        assert!(!encoded.contains("/home/"));
    }

    #[test]
    fn host_file_offer_uses_governed_metadata_not_a_host_path() {
        let now = 1_700_000_000_000_u64;
        let envelope = ClipboardEnvelopeV2::new_files(
            "host",
            "seat-1",
            "session-1",
            1,
            now,
            vec!["application/pdf".into(), VDI_GUEST_FILES_MIME.into()],
            "quarterly-report.pdf",
            ClipboardEnvelopeV2::content_hash_for(&[7; 42]),
            42,
            "files:v2:00000000-0000-0000-0000-000000000001",
            now + 60_000,
        )
        .expect("Files envelope");
        let command = VdiClipboardMessageV2 {
            schema_version: VDI_CLIPBOARD_TRANSPORT_V2_SCHEMA_VERSION,
            session_id: "session-1".into(),
            generation: 1,
            lease_id: "lease-1".into(),
            lease_expires_at_ms: now + 60_000,
            message_sequence: 1,
            selected_mime: "application/pdf".into(),
            disclosure: VdiClipboardDisclosureV2::Shareable,
            envelope,
        };
        let descriptor = rdp_host_file_descriptor(&command)
            .expect("valid host Files command")
            .expect("native arbitrary-file descriptor");
        assert_eq!(descriptor.name, "quarterly-report.pdf");
        assert_eq!(descriptor.mime, "application/pdf");
        assert!(VdiClipboardFileDescriptorV1::new("../host", None, "application/pdf", 42).is_err());
    }
}

mod pointer;
pub(crate) use pointer::body_device_px;
use pointer::*;

mod resources;
use resources::RemoteSessionsModel;

#[cfg(test)]
mod tests;
